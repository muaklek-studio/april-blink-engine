//! Render extraction: walks the scene once per frame and produces a GPU-agnostic
//! [`RenderList`]. Pure CPU, so it is unit-testable without a GPU. The walk is serial (it
//! is cheap and keeps tree order); skinning palettes and frustum culling then run in
//! parallel on the [`ab_core::jobs`] workers.

use crate::assets::{MaterialHandle, MeshHandle, RenderAssets};
use crate::bounds::posed_bounds;
use crate::camera::{CameraView, Frustum};
use crate::components::{
    Camera3D, DirectionalLight3D, MeshInstance3D, OmniLight3D, Skin, SpotLight3D,
};
use crate::lod::{screen_size, HlodGroup, HlodProxy, HlodSwitch, LodPick, MeshLod};
use crate::post_volume::PostVolume;
use ab_core::{jobs, Affine3A, Mat4, Vec3};
use ab_ecs::{Entity, SparseSet, Without, World};
use ab_scene::{Children, NodeInfo, Parent, Transform};
use std::ops::Range;

#[derive(Debug, Clone, PartialEq)]
pub struct DrawItem {
    pub model: Affine3A,
    pub mesh: MeshHandle,
    pub material: MaterialHandle,
    pub cast_shadows: bool,
    /// Passed frustum culling against the active camera.
    pub in_view: bool,
    /// For skinned instances: this instance's joint matrices in
    /// [`RenderList::joint_matrices`].
    pub joints: Option<Range<u32>>,
    /// Levels of detail ([`crate::MeshLod`]).
    pub lod: Option<LodPick>,
    /// Inside an [`crate::HlodGroup`].
    pub hlod: Option<HlodSwitch>,
    /// The level drawn this frame (0: the mesh itself).
    pub lod_level: u8,
    /// Hidden behind other geometry (occlusion culling).
    pub occluded: bool,
    /// Its box in the world, as culled (set by the culling).
    pub world_bounds: Option<crate::mesh::Aabb>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DirectionalLightData {
    /// Unit direction the light travels in.
    pub direction: Vec3,
    /// Linear color × energy.
    pub radiance: Vec3,
    pub shadow: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OmniLightData {
    pub position: Vec3,
    pub range: f32,
    pub radiance: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpotLightData {
    pub position: Vec3,
    pub direction: Vec3,
    pub range: f32,
    /// Half-angle of the cone.
    pub angle: f32,
    pub cos_outer: f32,
    pub cos_inner: f32,
    pub radiance: Vec3,
    pub shadow: bool,
}

/// Everything visible this frame, in tree order.
#[derive(Debug, Clone, Default)]
pub struct RenderList {
    pub camera: Option<CameraView>,
    pub draws: Vec<DrawItem>,
    pub directional: Option<DirectionalLightData>,
    /// Sorted nearest-to-camera first, so dropping the tail (GPU light limits) loses the
    /// least important lights.
    pub omni_lights: Vec<OmniLightData>,
    pub spot_lights: Vec<SpotLightData>,
    /// Skinning palettes of every skinned instance: `joint global × inverse bind`.
    pub joint_matrices: Vec<Mat4>,
    /// Post-processing volumes and where they are.
    pub post_volumes: Vec<(Affine3A, PostVolume)>,
    /// The occlusion buffer of the frame (with [`crate::OcclusionCulling::debug`]).
    pub occlusion: Option<crate::occlusion::OcclusionDebug>,
    /// With a [`RenderIsolation`]: every node drawn from (its particles are drawn too).
    pub isolated: Option<std::collections::HashSet<Entity>>,
}

impl RenderList {
    pub fn clear(&mut self) {
        self.camera = None;
        self.draws.clear();
        self.directional = None;
        self.omni_lights.clear();
        self.spot_lights.clear();
        self.joint_matrices.clear();
        self.post_volumes.clear();
        self.occlusion = None;
        self.isolated = None;
    }

    pub fn visible_draw_count(&self) -> usize {
        self.draws.iter().filter(|d| d.in_view).count()
    }
}

fn radiance(color: ab_core::Color, energy: f32) -> Vec3 {
    Vec3::new(color.r, color.g, color.b) * energy.max(0.0)
}

fn forward(global: &Affine3A) -> Vec3 {
    global
        .transform_vector3(Vec3::NEG_Z)
        .normalize_or(Vec3::NEG_Z)
}

/// ECS resource: render from this camera instead of the scene's current `Camera3D` (an
/// editor's viewport camera, a debug free-look camera).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraOverride {
    pub camera: Camera3D,
    /// Global transform of the camera (looks down its -Z).
    pub global: ab_core::Affine3A,
    /// An orthographic view this many world units tall instead of a perspective one.
    pub ortho_height: Option<f32>,
}

/// ECS resource: draw only `root` and what is under it, as if it were alone (an editor's
/// close look at one model or effect). The scene's other lights are left out too; with
/// `studio_light`, a key light from over the camera's shoulder lights it instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderIsolation {
    pub root: Entity,
    pub studio_light: bool,
    /// A floor to stand it on: drawn with this mesh and material, placed by the transform
    /// (it takes shadows; it is not part of the scene).
    pub floor: Option<(MeshHandle, MaterialHandle, Affine3A)>,
}

/// Draws culled per worker job: culling one is a handful of multiplies.
const CULL_PER_JOB: usize = 512;
/// Skins posed per worker job.
const SKINS_PER_JOB: usize = 2;

/// Writes `joint global × inverse bind` for every skin of `skinned`, in order, into
/// `palettes` (already sized: each instance's palette back to back, `(entity, length)`), in
/// parallel over skins. Palette entries past the skin's joints (vertices naming joints the
/// skin does not have) are left as they are (identity).
fn fill_palettes(
    skins: &SparseSet<Skin>,
    transforms: &SparseSet<Transform>,
    skinned: &[(Entity, usize)],
    palettes: &mut [Mat4],
) {
    let mut work = Vec::with_capacity(skinned.len());
    let mut rest = palettes;
    for &(entity, len) in skinned {
        let (palette, tail) = std::mem::take(&mut rest).split_at_mut(len);
        rest = tail;
        if let Some(skin) = skins.get(entity) {
            work.push((skin, palette));
        }
    }
    jobs::par_for_each_mut(&mut work, SKINS_PER_JOB, |(skin, palette)| {
        for (i, (out, joint)) in palette.iter_mut().zip(&skin.joints).enumerate() {
            let joint_global = transforms
                .get(joint.entity())
                .map_or(Mat4::IDENTITY, |t| Mat4::from(t.global));
            let inverse_bind = skin
                .inverse_bind_matrices
                .get(i)
                .copied()
                .unwrap_or(Mat4::IDENTITY);
            *out = joint_global * inverse_bind;
        }
    });
}

/// Fills `out` from every visible node inside the tree. `aspect` is the target's
/// width / height. Reuses `out`'s allocations.
pub fn extract(world: &World, aspect: f32, out: &mut RenderList) {
    out.clear();
    let (Some(infos), Some(children), Some(transforms)) = (
        world.storage::<NodeInfo>(),
        world.storage::<Children>(),
        world.storage::<Transform>(),
    ) else {
        return;
    };
    let meshes = world.storage::<MeshInstance3D>();
    let skins = world.storage::<Skin>();
    let cameras = world.storage::<Camera3D>();
    let suns = world.storage::<DirectionalLight3D>();
    let omnis = world.storage::<OmniLight3D>();
    let spots = world.storage::<SpotLight3D>();
    let volumes = world.storage::<PostVolume>();
    let lods = world.storage::<MeshLod>();
    let hlods = world.storage::<HlodGroup>();
    let proxies = world.storage::<HlodProxy>();

    // Tree roots: in-tree nodes without a parent (normally just the SceneTree root).
    let isolation = world
        .resource::<RenderIsolation>()
        .map(|i| *i)
        .filter(|i| infos.get(i.root).is_some_and(|info| info.in_tree));
    let roots: Vec<Entity> = match isolation {
        Some(i) => vec![i.root],
        None => world
            .query_filtered::<(Entity, &NodeInfo), Without<Parent>>()
            .iter()
            .filter(|(_, info)| info.in_tree)
            .map(|(e, _)| e)
            .collect(),
    };
    let mut isolated = isolation.map(|_| std::collections::HashSet::new());
    if let Some((mesh, material, model)) = isolation.and_then(|i| i.floor) {
        out.draws.push(DrawItem {
            model,
            mesh,
            material,
            cast_shadows: false,
            in_view: true,
            joints: None,
            lod: None,
            hlod: None,
            lod_level: 0,
            occluded: false,
            world_bounds: None,
        });
    }

    let mut fallback_camera = None;
    let mut chosen_camera = None;
    // Skinned instances, in the order of their palettes in `out.joint_matrices`.
    let mut skinned: Vec<(Entity, usize)> = Vec::new();
    let assets = world.resource::<RenderAssets>();
    // Each node with the HLOD group it is in (and whether under a proxy).
    let mut stack: Vec<(Entity, Option<HlodSwitch>)> =
        roots.into_iter().rev().map(|r| (r, None)).collect();
    while let Some((entity, mut hlod)) = stack.pop() {
        let Some(info) = infos.get(entity) else {
            continue;
        };
        // Hidden nodes hide their whole subtree (an isolated root shows, hidden or not).
        if !info.visible && isolation.is_none_or(|i| i.root != entity) {
            continue;
        }
        if let Some(set) = &mut isolated {
            set.insert(entity);
        }
        if let Some(group) = hlods.as_ref().and_then(|h| h.get(entity)) {
            hlod = Some(HlodSwitch {
                center: group.center,
                distance: group.distance,
                proxy: false,
            });
        }
        if proxies.as_ref().is_some_and(|p| p.get(entity).is_some()) {
            if let Some(h) = &mut hlod {
                h.proxy = true;
            }
        }
        if let Some(Children(kids)) = children.get(entity) {
            stack.extend(kids.iter().rev().map(|k| (k.entity(), hlod)));
        }
        let Some(transform) = transforms.get(entity) else {
            continue;
        };
        let global = transform.global;

        if let Some(camera) = cameras.as_ref().and_then(|c| c.get(entity)) {
            if camera.current && chosen_camera.is_none() {
                chosen_camera = Some((*camera, global));
            }
            fallback_camera.get_or_insert((*camera, global));
        }
        if let Some(instance) = meshes.as_ref().and_then(|m| m.get(entity)) {
            // Palettes are filled in parallel once the walk is done.
            let joints = skins.as_ref().and_then(|s| s.get(entity)).map(|skin| {
                // At least as many matrices as the mesh's vertices name joints, so a skin with
                // fewer joints than its mesh expects never lets the shader read the next
                // instance's palette or past the buffer.
                let mesh_joints = assets
                    .as_ref()
                    .map_or(0, |a| a.mesh_joint_bounds(instance.mesh).len());
                let len = skin.joints.len().max(mesh_joints);
                let start = out.joint_matrices.len();
                out.joint_matrices.resize(start + len, Mat4::IDENTITY);
                skinned.push((entity, len));
                start as u32..out.joint_matrices.len() as u32
            });
            out.draws.push(DrawItem {
                model: global,
                mesh: instance.mesh,
                material: instance.material,
                cast_shadows: instance.cast_shadows,
                in_view: true,
                // Skinned meshes keep their mesh: their bounds follow its joints.
                lod: lods
                    .as_ref()
                    .and_then(|l| l.get(entity))
                    .filter(|_| joints.is_none())
                    .map(LodPick::of),
                hlod,
                lod_level: 0,
                occluded: false,
                world_bounds: None,
                joints,
            });
        }
        if let Some(volume) = volumes.as_ref().and_then(|v| v.get(entity)) {
            out.post_volumes.push((global, *volume));
        }
        if let Some(sun) = suns.as_ref().and_then(|s| s.get(entity)) {
            out.directional.get_or_insert(DirectionalLightData {
                direction: forward(&global),
                radiance: radiance(sun.color, sun.energy),
                shadow: sun.shadow_enabled,
            });
        }
        if let Some(omni) = omnis.as_ref().and_then(|o| o.get(entity)) {
            out.omni_lights.push(OmniLightData {
                position: global.translation.into(),
                range: omni.range.max(0.01),
                radiance: radiance(omni.color, omni.energy),
            });
        }
        if let Some(spot) = spots.as_ref().and_then(|s| s.get(entity)) {
            let angle = spot.angle_degrees.clamp(0.1, 89.9).to_radians();
            let inner = angle * (1.0 - spot.edge_softness.clamp(0.0, 1.0));
            out.spot_lights.push(SpotLightData {
                position: global.translation.into(),
                direction: forward(&global),
                range: spot.range.max(0.01),
                angle,
                cos_outer: angle.cos(),
                cos_inner: inner.cos().max(angle.cos() + 1e-4),
                radiance: radiance(spot.color, spot.energy),
                shadow: spot.shadow_enabled,
            });
        }
    }

    if let Some(skins) = &skins {
        fill_palettes(skins, &transforms, &skinned, &mut out.joint_matrices);
    }

    // Isolated, the walk did not pass the scene's cameras.
    if isolation.is_some() && chosen_camera.is_none() {
        if let Some(cameras) = &cameras {
            for (entity, camera) in cameras.iter() {
                let in_tree = infos.get(entity).is_some_and(|i| i.in_tree);
                let Some(global) = transforms.get(entity).map(|t| t.global) else {
                    continue;
                };
                if !in_tree {
                    continue;
                }
                if camera.current && chosen_camera.is_none() {
                    chosen_camera = Some((*camera, global));
                }
                fallback_camera.get_or_insert((*camera, global));
            }
        }
    }
    let forced = world
        .resource::<CameraOverride>()
        .map(|o| (o.camera, o.global, o.ortho_height));
    let Some((camera, global, ortho_height)) = forced
        .or(chosen_camera.map(|(c, g)| (c, g, None)))
        .or(fallback_camera.map(|(c, g)| (c, g, None)))
    else {
        return;
    };
    let view = match ortho_height {
        Some(height) => CameraView::orthographic(&global, height, aspect, camera.near, camera.far),
        None => CameraView::new(
            &global,
            camera.fov_y_degrees.clamp(1.0, 179.0).to_radians(),
            aspect,
            camera.near,
            camera.far,
        ),
    };

    if let Some(assets) = assets {
        let frustum = Frustum::from_view_projection(&view.view_projection);
        let assets = &*assets;
        let palettes = &out.joint_matrices;
        let view_ref = &view;
        jobs::par_for_each_mut(&mut out.draws, CULL_PER_JOB, |draw| {
            // Near or far from its HLOD group: the other half is not drawn at all.
            if let Some(h) = draw.hlod {
                let far = view_ref.position.distance(h.center) > h.distance;
                if far != h.proxy {
                    draw.in_view = false;
                    draw.cast_shadows = false;
                    return;
                }
            }
            // The level for its size on screen (its own mesh's bounds decide).
            if let Some(lod) = draw.lod {
                let size = assets.mesh_aabb(draw.mesh).map_or(1.0, |aabb| {
                    screen_size(&aabb.transformed(&draw.model), view_ref)
                });
                match lod.pick(draw.mesh, size) {
                    Some((mesh, level)) => {
                        draw.mesh = mesh;
                        draw.lod_level = level;
                    }
                    None => {
                        draw.in_view = false;
                        draw.cast_shadows = false;
                        draw.lod_level = u8::MAX;
                        return;
                    }
                }
            }
            let bounds = match &draw.joints {
                // Skinned meshes move with their joints: bounds of the posed joints.
                Some(range) => {
                    let joints = assets.mesh_joint_bounds(draw.mesh);
                    let palette = &palettes[range.start as usize..range.end as usize];
                    if joints.is_empty() {
                        return; // no skin data: always drawn
                    }
                    posed_bounds(joints, palette)
                }
                None => assets
                    .mesh_aabb(draw.mesh)
                    .map(|aabb| aabb.transformed(&draw.model)),
            };
            draw.in_view = bounds.is_some_and(|b| frustum.intersects_aabb(&b));
            draw.world_bounds = bounds;
        });
        let occlusion = world
            .resource::<crate::occlusion::OcclusionCulling>()
            .map(|o| *o)
            .filter(|o| o.enabled);
        if let Some(settings) = occlusion {
            let bounds: Vec<_> = out.draws.iter().map(|d| d.world_bounds).collect();
            out.occlusion =
                crate::occlusion::cull(&settings, &view, assets, &mut out.draws, &bounds);
        }
    }
    if isolation.is_some_and(|i| i.studio_light) {
        // Over the camera's right shoulder, from above.
        let direction = (view.forward * 0.55 - Vec3::Y * 0.75
            + view.forward.cross(Vec3::Y).normalize_or_zero() * -0.35)
            .normalize_or(Vec3::NEG_Y);
        out.directional = Some(DirectionalLightData {
            direction,
            radiance: Vec3::splat(2.6),
            shadow: true,
        });
    }
    out.isolated = isolated;
    let eye = view.position;
    out.omni_lights.sort_by(|a, b| {
        a.position
            .distance_squared(eye)
            .total_cmp(&b.position.distance_squared(eye))
    });
    out.spot_lights.sort_by(|a, b| {
        a.position
            .distance_squared(eye)
            .total_cmp(&b.position.distance_squared(eye))
    });
    out.camera = Some(view);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::Material;
    use crate::mesh::Mesh;
    use crate::nodes::RenderNodes;
    use ab_scene::SceneTree;

    fn scene() -> (SceneTree, MeshHandle, MaterialHandle) {
        let mut tree = SceneTree::new();
        let mut assets = RenderAssets::new();
        let mesh = assets.add_mesh(Mesh::cube(1.0)).unwrap();
        let material = assets.add_material(Material::default());
        tree.world_mut().insert_resource(assets);
        (tree, mesh, material)
    }

    fn run(tree: &mut SceneTree) -> RenderList {
        tree.update_transforms();
        let mut list = RenderList::default();
        extract(tree.world(), 1.0, &mut list);
        list
    }

    #[test]
    fn walls_hide_what_is_behind_lods_simplify_and_hlods_switch() {
        use crate::lod::{HlodGroup, HlodProxy, LodLevel, MeshLod};
        use crate::occlusion::OcclusionCulling;
        let (mut tree, mesh, material) = scene();
        let root = tree.root();
        let camera = tree.create_camera3d("Camera", Camera3D::default());
        tree.set_position(camera, Vec3::new(0.0, 0.0, 10.0))
            .unwrap();
        tree.add_child(root, camera).unwrap();
        // A wall 6 × 6 m at z = 0, a box behind it and one beside it.
        let wall = tree.create_mesh_instance3d("Wall", MeshInstance3D::new(mesh, material));
        tree.set_scale(wall, Vec3::new(6.0, 6.0, 0.2)).unwrap();
        let behind = tree.create_mesh_instance3d("Behind", MeshInstance3D::new(mesh, material));
        tree.set_position(behind, Vec3::new(0.0, 0.0, -10.0))
            .unwrap();
        let beside = tree.create_mesh_instance3d("Beside", MeshInstance3D::new(mesh, material));
        tree.set_position(beside, Vec3::new(8.0, 0.0, -10.0))
            .unwrap();
        for n in [wall, behind, beside] {
            tree.add_child(root, n).unwrap();
        }
        let list = run(&mut tree);
        assert_eq!(list.visible_draw_count(), 3, "off by default");
        tree.world_mut().insert_resource(OcclusionCulling {
            enabled: true,
            debug: true,
            ..Default::default()
        });
        let list = run(&mut tree);
        let names: Vec<bool> = list.draws.iter().map(|d| d.in_view).collect();
        assert_eq!(
            names,
            [true, false, true],
            "only the box behind the wall is culled"
        );
        assert!(list.draws[1].occluded);
        let debug = list.occlusion.unwrap();
        assert_eq!((debug.occluders, debug.culled), (1, 1));
        tree.world_mut().remove_resource::<OcclusionCulling>();

        // A far box with levels: drawn simpler, then not at all.
        let simple = {
            let mut assets = tree.world_mut().resource_mut::<RenderAssets>().unwrap();
            assets.add_mesh(Mesh::cube(0.5)).unwrap()
        };
        tree.insert_component(
            beside,
            MeshLod {
                levels: vec![LodLevel {
                    screen_size: 0.5,
                    mesh: simple,
                }],
                cull_below: 0.0,
            },
        )
        .unwrap();
        let list = run(&mut tree);
        assert_eq!((list.draws[2].mesh, list.draws[2].lod_level), (simple, 1));
        tree.insert_component(
            beside,
            MeshLod {
                levels: Vec::new(),
                cull_below: 0.5,
            },
        )
        .unwrap();
        let list = run(&mut tree);
        assert!(!list.draws[2].in_view && !list.draws[2].cast_shadows);

        // An HLOD group far away draws its proxy, near its own meshes.
        let group = tree.create_node3d("Group");
        tree.insert_component(
            group,
            HlodGroup {
                distance: 50.0,
                center: Vec3::new(0.0, 0.0, -100.0),
            },
        )
        .unwrap();
        let part = tree.create_mesh_instance3d("Part", MeshInstance3D::new(mesh, material));
        let proxy = tree.create_mesh_instance3d("Proxy", MeshInstance3D::new(mesh, material));
        tree.insert_component(proxy, HlodProxy).unwrap();
        tree.add_child(root, group).unwrap();
        tree.add_child(group, part).unwrap();
        tree.add_child(group, proxy).unwrap();
        tree.set_position(group, Vec3::new(0.0, 0.0, -100.0))
            .unwrap();
        tree.insert_component(beside, MeshLod::default()).unwrap();
        let list = run(&mut tree);
        let (part_drawn, proxy_drawn) = (list.draws[3].in_view, list.draws[4].in_view);
        assert_eq!((part_drawn, proxy_drawn), (false, true));
        tree.set_position(camera, Vec3::new(0.0, 0.0, -80.0))
            .unwrap();
        let list = run(&mut tree);
        assert!(list.draws[3].in_view && !list.draws[4].in_view);
    }

    #[test]
    fn an_isolated_node_is_drawn_alone_under_a_studio_light() {
        let (mut tree, mesh, material) = scene();
        let root = tree.root();
        let camera = tree.create_camera3d("Camera", Camera3D::default());
        tree.set_position(camera, Vec3::new(0.0, 0.0, 5.0)).unwrap();
        let sun = tree.create_directional_light3d("Sun", DirectionalLight3D::default());
        let lamp = tree.create_omni_light3d("Lamp", OmniLight3D::default());
        let subject = tree.create_node3d("Subject");
        let part = tree.create_mesh_instance3d("Part", MeshInstance3D::new(mesh, material));
        let other = tree.create_mesh_instance3d("Other", MeshInstance3D::new(mesh, material));
        for n in [camera, sun, lamp, subject, other] {
            tree.add_child(root, n).unwrap();
        }
        tree.add_child(subject, part).unwrap();
        // Hidden in the scene, still shown alone.
        tree.set_visible(subject, false).unwrap();
        let list = run(&mut tree);
        assert_eq!(
            list.draws.len(),
            1,
            "the hidden subject is not drawn in the scene"
        );
        tree.world_mut().insert_resource(RenderIsolation {
            root: subject.entity(),
            studio_light: true,
            floor: None,
        });
        let list = run(&mut tree);
        assert_eq!(list.draws.len(), 1);
        assert!(list.camera.is_some(), "the scene's camera still frames it");
        assert!(
            list.omni_lights.is_empty(),
            "the scene's lights are left out"
        );
        let light = list.directional.unwrap();
        assert!(
            light.direction.y < 0.0 && light.shadow,
            "a key light from above"
        );
        let set = list.isolated.unwrap();
        assert!(set.contains(&part.entity()) && !set.contains(&other.entity()));
        // Its particles only.
        let particles = crate::ParticleDrawList {
            instances: vec![
                crate::ParticleInstance {
                    position: Vec3::ZERO,
                    size: 1.0,
                    color: ab_core::Color::WHITE,
                    velocity: Vec3::ZERO,
                    stretch: 0.0,
                    rotation: 0.0,
                    hardness: 0.5,
                    additive: false,
                };
                2
            ],
            sources: vec![(part.entity(), 0..1), (other.entity(), 1..2)],
        };
        assert_eq!(particles.filtered(|e| set.contains(&e)).instances.len(), 1);
        // A floor to stand it on: drawn, taking shadows, casting none.
        tree.world_mut().insert_resource(RenderIsolation {
            root: subject.entity(),
            studio_light: true,
            floor: Some((mesh, material, Affine3A::IDENTITY)),
        });
        let list = run(&mut tree);
        assert_eq!(list.draws.len(), 2);
        assert!(!list.draws[0].cast_shadows);
    }

    #[test]
    fn extracts_visible_meshes_in_tree_order_and_culls() {
        let (mut tree, mesh, material) = scene();
        let root = tree.root();
        let camera = tree.create_camera3d("Camera", Camera3D::default());
        tree.set_position(camera, Vec3::new(0.0, 0.0, 5.0)).unwrap();
        tree.add_child(root, camera).unwrap();

        let front = tree.create_mesh_instance3d("Front", MeshInstance3D::new(mesh, material));
        let behind = tree.create_mesh_instance3d("Behind", MeshInstance3D::new(mesh, material));
        let hidden = tree.create_mesh_instance3d("Hidden", MeshInstance3D::new(mesh, material));
        let detached = tree.create_mesh_instance3d("Detached", MeshInstance3D::new(mesh, material));
        tree.set_position(behind, Vec3::new(0.0, 0.0, 20.0))
            .unwrap();
        for n in [front, behind, hidden] {
            tree.add_child(root, n).unwrap();
        }
        tree.set_visible(hidden, false).unwrap();
        let _ = detached;

        let list = run(&mut tree);
        assert!(list.camera.is_some());
        assert_eq!(list.draws.len(), 2, "hidden and detached nodes are skipped");
        assert!(list.draws[0].in_view);
        assert!(!list.draws[1].in_view, "behind the camera");
        assert_eq!(list.visible_draw_count(), 1);
    }

    #[test]
    fn large_scenes_cull_every_draw() {
        let (mut tree, mesh, material) = scene();
        let root = tree.root();
        let camera = tree.create_camera3d("Camera", Camera3D::default());
        tree.add_child(root, camera).unwrap();
        let count = CULL_PER_JOB * 3;
        for i in 0..count {
            let node = tree.create_mesh_instance3d("Cube", MeshInstance3D::new(mesh, material));
            // Even ones in front of the camera, odd ones behind it.
            let z = if i % 2 == 0 { -5.0 } else { 5.0 };
            tree.set_position(node, Vec3::new(0.0, 0.0, z)).unwrap();
            tree.add_child(root, node).unwrap();
        }
        let list = run(&mut tree);
        assert_eq!(list.draws.len(), count);
        for (i, draw) in list.draws.iter().enumerate() {
            assert_eq!(draw.in_view, i % 2 == 0, "draw {i}");
        }
    }

    #[test]
    fn hidden_parent_hides_children_and_lights() {
        let (mut tree, mesh, material) = scene();
        let group = tree.create_node3d("Group");
        tree.add_child(tree.root(), group).unwrap();
        let child = tree.create_mesh_instance3d("Child", MeshInstance3D::new(mesh, material));
        let lamp = tree.create_omni_light3d("Lamp", OmniLight3D::default());
        tree.add_child(group, child).unwrap();
        tree.add_child(group, lamp).unwrap();
        tree.set_visible(group, false).unwrap();
        let list = run(&mut tree);
        assert!(list.draws.is_empty() && list.omni_lights.is_empty());
    }

    #[test]
    fn current_camera_wins_and_lights_are_extracted() {
        let (mut tree, ..) = scene();
        let root = tree.root();
        let first = tree.create_camera3d("First", Camera3D::default());
        let current = tree.create_camera3d(
            "Current",
            Camera3D {
                current: true,
                ..Default::default()
            },
        );
        tree.set_position(current, Vec3::new(1.0, 2.0, 3.0))
            .unwrap();
        tree.add_child(root, first).unwrap();
        tree.add_child(root, current).unwrap();

        let sun = tree.create_directional_light3d(
            "Sun",
            DirectionalLight3D {
                energy: 2.0,
                ..Default::default()
            },
        );
        tree.rotate(sun, Vec3::X, -std::f32::consts::FRAC_PI_2)
            .unwrap();
        tree.add_child(root, sun).unwrap();

        let far = tree.create_omni_light3d("Far", OmniLight3D::default());
        let near = tree.create_omni_light3d("Near", OmniLight3D::default());
        tree.set_position(far, Vec3::new(50.0, 0.0, 0.0)).unwrap();
        tree.set_position(near, Vec3::new(1.0, 2.0, 2.0)).unwrap();
        tree.add_child(root, far).unwrap();
        tree.add_child(root, near).unwrap();

        let spot = tree.create_spot_light3d(
            "Spot",
            SpotLight3D {
                angle_degrees: 30.0,
                ..Default::default()
            },
        );
        tree.add_child(root, spot).unwrap();

        let list = run(&mut tree);
        assert!(list
            .camera
            .unwrap()
            .position
            .abs_diff_eq(Vec3::new(1.0, 2.0, 3.0), 1e-5));
        let sun = list.directional.unwrap();
        assert!(
            sun.direction.abs_diff_eq(Vec3::NEG_Y, 1e-5),
            "{}",
            sun.direction
        );
        assert!(sun.radiance.abs_diff_eq(Vec3::splat(2.0), 1e-6));
        assert!(
            list.omni_lights[0]
                .position
                .abs_diff_eq(Vec3::new(1.0, 2.0, 2.0), 1e-5),
            "sorted by distance"
        );
        let spot = list.spot_lights[0];
        assert!((spot.cos_outer - 30f32.to_radians().cos()).abs() < 1e-6);
        assert!(spot.cos_inner > spot.cos_outer);
    }

    #[test]
    fn skinned_instances_get_joint_palettes_and_skip_culling() {
        let (mut tree, mesh, material) = scene();
        let root = tree.root();
        let camera = tree.create_camera3d("Camera", Camera3D::default());
        tree.add_child(root, camera).unwrap();
        let joint = tree.create_node3d("Joint");
        tree.set_position(joint, Vec3::new(0.0, 0.0, 50.0)).unwrap();
        tree.add_child(root, joint).unwrap();
        let body = tree.create_mesh_instance3d("Body", MeshInstance3D::new(mesh, material));
        tree.insert_component(
            body,
            Skin {
                joints: vec![joint],
                inverse_bind_matrices: vec![Mat4::from_translation(Vec3::new(0.0, 0.0, -1.0))],
            },
        )
        .unwrap();
        tree.add_child(root, body).unwrap();

        let list = run(&mut tree);
        let draw = &list.draws[0];
        assert_eq!(draw.joints, Some(0..1));
        assert!(draw.in_view, "skinned meshes are not culled");
        let expected = Mat4::from_translation(Vec3::new(0.0, 0.0, 49.0));
        assert!(list.joint_matrices[0].abs_diff_eq(expected, 1e-5));
    }

    #[test]
    fn palettes_cover_every_joint_the_mesh_names() {
        let (mut tree, _, material) = scene();
        // Vertices name joint 2, but the skin only has one joint.
        let mesh = Mesh::cube(1.0).with_skin(vec![
            crate::mesh::SkinWeights {
                joints: [0, 2, 0, 0],
                weights: [0.5, 0.5, 0.0, 0.0],
            };
            24
        ]);
        let mesh = tree
            .world()
            .resource_mut::<RenderAssets>()
            .unwrap()
            .add_mesh(mesh)
            .unwrap();
        let root = tree.root();
        let joint = tree.create_node3d("Joint");
        tree.add_child(root, joint).unwrap();
        let mut bodies = Vec::new();
        for name in ["A", "B"] {
            let body = tree.create_mesh_instance3d(name, MeshInstance3D::new(mesh, material));
            tree.insert_component(
                body,
                Skin {
                    joints: vec![joint],
                    inverse_bind_matrices: vec![Mat4::IDENTITY],
                },
            )
            .unwrap();
            tree.add_child(root, body).unwrap();
            bodies.push(body);
        }
        let list = run(&mut tree);
        assert_eq!(list.draws[0].joints, Some(0..3));
        assert_eq!(
            list.draws[1].joints,
            Some(3..6),
            "no overlap with the first palette"
        );
        assert_eq!(list.joint_matrices.len(), 6);
        assert_eq!(list.joint_matrices[2], Mat4::IDENTITY, "padding");
    }

    #[test]
    fn no_camera_means_no_view() {
        let (mut tree, mesh, material) = scene();
        let m = tree.create_mesh_instance3d("M", MeshInstance3D::new(mesh, material));
        tree.add_child(tree.root(), m).unwrap();
        let list = run(&mut tree);
        assert!(list.camera.is_none());
        assert_eq!(list.draws.len(), 1);
    }
}
