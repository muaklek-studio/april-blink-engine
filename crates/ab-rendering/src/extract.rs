//! Render extraction: walks the scene once per frame and produces a GPU-agnostic
//! [`RenderList`]. Pure CPU, so it is unit-testable without a GPU. The walk is serial (it
//! is cheap and keeps tree order); skinning palettes and frustum culling then run in
//! parallel on the [`ab_core::jobs`] workers.

use crate::assets::{MaterialHandle, MeshHandle, RenderAssets};
use crate::camera::{CameraView, Frustum};
use crate::components::{
    Camera3D, DirectionalLight3D, MeshInstance3D, OmniLight3D, Skin, SpotLight3D,
};
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
}

impl RenderList {
    pub fn clear(&mut self) {
        self.camera = None;
        self.draws.clear();
        self.directional = None;
        self.omni_lights.clear();
        self.spot_lights.clear();
        self.joint_matrices.clear();
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
}

/// Draws culled per worker job: culling one is a handful of multiplies.
const CULL_PER_JOB: usize = 512;
/// Skins posed per worker job.
const SKINS_PER_JOB: usize = 2;

/// Writes `joint global × inverse bind` for every skin of `skinned`, in order, into
/// `palettes` (already sized: each skin's joints back to back), in parallel over skins.
fn fill_palettes(
    skins: &SparseSet<Skin>,
    transforms: &SparseSet<Transform>,
    skinned: &[Entity],
    palettes: &mut [Mat4],
) {
    let mut work = Vec::with_capacity(skinned.len());
    let mut rest = palettes;
    for skin in skinned.iter().filter_map(|&e| skins.get(e)) {
        let (palette, tail) = std::mem::take(&mut rest).split_at_mut(skin.joints.len());
        work.push((skin, palette));
        rest = tail;
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

    // Tree roots: in-tree nodes without a parent (normally just the SceneTree root).
    let roots: Vec<Entity> = world
        .query_filtered::<(Entity, &NodeInfo), Without<Parent>>()
        .iter()
        .filter(|(_, info)| info.in_tree)
        .map(|(e, _)| e)
        .collect();

    let mut fallback_camera = None;
    let mut chosen_camera = None;
    // Skinned instances, in the order of their palettes in `out.joint_matrices`.
    let mut skinned: Vec<Entity> = Vec::new();
    let mut stack: Vec<Entity> = roots.into_iter().rev().collect();
    while let Some(entity) = stack.pop() {
        let Some(info) = infos.get(entity) else {
            continue;
        };
        // Hidden nodes hide their whole subtree.
        if !info.visible {
            continue;
        }
        if let Some(Children(kids)) = children.get(entity) {
            stack.extend(kids.iter().rev().map(|k| k.entity()));
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
                let start = out.joint_matrices.len();
                out.joint_matrices
                    .resize(start + skin.joints.len(), Mat4::IDENTITY);
                skinned.push(entity);
                start as u32..out.joint_matrices.len() as u32
            });
            out.draws.push(DrawItem {
                model: global,
                mesh: instance.mesh,
                material: instance.material,
                cast_shadows: instance.cast_shadows,
                in_view: true,
                joints,
            });
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

    let forced = world
        .resource::<CameraOverride>()
        .map(|o| (o.camera, o.global));
    let Some((camera, global)) = forced.or(chosen_camera).or(fallback_camera) else {
        return;
    };
    let view = CameraView::new(
        &global,
        camera.fov_y_degrees.clamp(1.0, 179.0).to_radians(),
        aspect,
        camera.near,
        camera.far,
    );

    if let Some(assets) = world.resource::<RenderAssets>() {
        let frustum = Frustum::from_view_projection(&view.view_projection);
        let assets = &*assets;
        jobs::par_for_each_mut(&mut out.draws, CULL_PER_JOB, |draw| {
            // Skinned meshes move with their joints, so their bind-pose bounds are meaningless.
            if draw.joints.is_none() {
                draw.in_view = assets
                    .mesh_aabb(draw.mesh)
                    .is_some_and(|aabb| frustum.intersects_aabb(&aabb.transformed(&draw.model)));
            }
        });
    }
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
    fn no_camera_means_no_view() {
        let (mut tree, mesh, material) = scene();
        let m = tree.create_mesh_instance3d("M", MeshInstance3D::new(mesh, material));
        tree.add_child(tree.root(), m).unwrap();
        let list = run(&mut tree);
        assert!(list.camera.is_none());
        assert_eq!(list.draws.len(), 1);
    }
}
