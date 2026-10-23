//! Renders assets on their own for thumbnails and the asset viewer: a model or a scene
//! framed by the camera, or a material on a sphere or a cube, under a studio light.
//!
//! The previews have a world of their own (an [`Engine`] on the project's assets) and a
//! scene renderer of their own: the renderer keeps GPU copies of a world's meshes by index,
//! so it cannot be shared with the editor's scene.

use ab_engine::core::{Affine3A, Color, EulerRot, Quat, Vec3};
use ab_engine::prelude::RenderNodes as _;
use ab_engine::rendering::{
    Aabb, Bounds, Camera3D, CameraOverride, DirectionalLight3D, Environment, GpuContext,
    MeshInstance3D, OffscreenTarget, RenderAssets, RendererConfig, SceneRenderer,
};
use ab_engine::scene::NodeHandle;
use ab_engine::scene_file::{instantiate_scene, MaterialDesc, MeshDesc, NodeDesc, SceneFile};
use ab_engine::{Engine, EngineConfig};
use eframe::egui;
use std::path::{Path, PathBuf};

/// What a preview shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Subject {
    /// A glTF model (asset path).
    Model(String),
    /// A scene file, as an instance.
    Scene(String),
    /// A material file on a shape.
    Material(String, Shape),
}

impl Subject {
    pub fn path(&self) -> &str {
        match self {
            Subject::Model(p) | Subject::Scene(p) | Subject::Material(p, _) => p,
        }
    }
}

/// The shape a material is shown on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Shape {
    #[default]
    Sphere,
    Cube,
    Plane,
}

/// How the preview camera looks at the subject.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orbit {
    /// Radians around the vertical axis.
    pub yaw: f32,
    /// Radians up (positive) or down.
    pub pitch: f32,
    /// 1 frames the subject; smaller is closer.
    pub zoom: f32,
    /// Radians around the vertical axis for the light.
    pub light_yaw: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            yaw: 0.6,
            pitch: -0.35,
            zoom: 1.0,
            light_yaw: -0.8,
        }
    }
}

/// What the loaded subject is made of, for the viewer's details.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubjectInfo {
    pub nodes: usize,
    pub meshes: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub materials: usize,
    pub textures: usize,
    pub skinned: bool,
    pub animations: Vec<String>,
    /// World size of the subject's bounds.
    pub size: Option<Vec3>,
}

/// The preview world grows with every model loaded (assets are append-only): start a fresh
/// one past this many meshes.
const MESH_BUDGET: usize = 4000;

pub struct Previewer {
    gpu: GpuContext,
    renderer: SceneRenderer,
    engine: Engine,
    project: PathBuf,
    loaded: Option<Subject>,
    root: Option<NodeHandle>,
    pub info: SubjectInfo,
}

impl std::fmt::Debug for Previewer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Previewer")
            .field("loaded", &self.loaded)
            .finish()
    }
}

fn preview_engine(project: &Path) -> Engine {
    let mut engine = Engine::new(&EngineConfig {
        asset_root: project.to_path_buf(),
        audio_output: ab_engine::audio::AudioOutput::Silent,
        ..Default::default()
    });
    let world = engine.tree_mut().world_mut();
    world.insert_resource(Environment {
        background: Color::rgb(0.11, 0.115, 0.13),
        ambient_color: Color::rgb(0.75, 0.8, 0.9),
        ambient_energy: 0.55,
        ..Default::default()
    });
    engine
}

impl Previewer {
    pub fn new(gpu: GpuContext, project: &Path) -> Self {
        let config = RendererConfig {
            vsync: false,
            ..Default::default()
        };
        let renderer = SceneRenderer::new(&gpu, OffscreenTarget::FORMAT, &config);
        Self {
            gpu,
            renderer,
            engine: preview_engine(project),
            project: project.to_path_buf(),
            loaded: None,
            root: None,
            info: SubjectInfo::default(),
        }
    }

    pub fn gpu(&self) -> &GpuContext {
        &self.gpu
    }

    /// Shows `subject` (read again from disk when `reload`, after its file changed).
    pub fn load(&mut self, subject: &Subject, reload: bool) -> Result<(), String> {
        if self.loaded.as_ref() == Some(subject) && !reload {
            return Ok(());
        }
        let too_big = self
            .engine
            .tree()
            .world()
            .resource::<RenderAssets>()
            .is_some_and(|a| a.mesh_count() > MESH_BUDGET);
        if too_big {
            // A fresh world needs a fresh renderer: the old one indexes the old meshes.
            *self = Self::new(self.gpu.clone(), &self.project.clone());
        }
        let tree = self.engine.tree_mut();
        if let Some(root) = self.root.take() {
            let _ = tree.queue_free(root);
            tree.flush_deletions();
        }
        self.loaded = None;
        if reload {
            if let Some(mut assets) = tree.world().resource_mut::<ab_engine::asset::AssetServer>() {
                assets.evict(subject.path());
            }
        }
        let scene = SceneFile {
            environment: None,
            nodes: subject_nodes(subject),
        };
        let root = instantiate_scene(tree, &scene, "preview").map_err(|e| e.to_string())?;
        // A model or scene that does not load becomes a placeholder: nothing to show.
        let missing = tree.get_node(root, "Subject").and_then(|s| {
            tree.component::<ab_engine::scene_file::prefab::MissingInstance>(s)
                .map(|m| m.error.clone())
        });
        if let Some(error) = missing {
            let _ = tree.queue_free(root);
            tree.flush_deletions();
            return Err(error);
        }
        let top = tree.root();
        tree.add_child(top, root).map_err(|e| e.to_string())?;
        let sun = tree.create_directional_light3d(
            "Sun",
            DirectionalLight3D {
                energy: 2.6,
                shadow_enabled: false,
                ..Default::default()
            },
        );
        tree.add_child(root, sun).map_err(|e| e.to_string())?;
        tree.update_transforms();
        self.info = describe(tree, root);
        self.root = Some(root);
        self.loaded = Some(subject.clone());
        Ok(())
    }

    /// Draws the loaded subject into `target`.
    pub fn render(&mut self, target: &OffscreenTarget, orbit: &Orbit) {
        let (width, height) = target.size();
        let tree = self.engine.tree_mut();
        let bounds = self
            .root
            .and_then(|r| tree.subtree_bounds(r))
            .unwrap_or(Aabb {
                min: Vec3::splat(-0.5),
                max: Vec3::splat(0.5),
            });
        let center = bounds.center();
        let radius = (bounds.size().length() * 0.5).max(0.05);
        let fov: f32 = 35.0;
        let distance = radius / (fov.to_radians() * 0.5).sin() * orbit.zoom.clamp(0.1, 10.0);
        let rotation =
            Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch.clamp(-1.5, 1.5), 0.0);
        let eye = center + rotation * Vec3::Z * distance;
        // The light keeps its angle to the camera unless turned on its own.
        if let Some(sun) = self.root.and_then(|r| tree.get_node(r, "Sun")) {
            let light = Quat::from_euler(EulerRot::YXZ, orbit.light_yaw, -0.75, 0.0);
            let _ = tree.set_transform(sun, ab_engine::core::Transform3D::from_rotation(light));
        }
        tree.update_transforms();
        let world = tree.world_mut();
        world.insert_resource(CameraOverride {
            camera: Camera3D {
                fov_y_degrees: fov,
                near: (distance - radius * 2.0).max(0.01),
                far: distance + radius * 4.0,
                current: true,
            },
            global: Affine3A::from_rotation_translation(rotation, eye),
            ortho_height: None,
        });
        self.renderer.render(
            &self.gpu,
            self.engine.tree().world(),
            target.view(),
            width,
            height,
        );
    }

    /// A still image of `subject`, `size` pixels square.
    pub fn thumbnail(
        &mut self,
        subject: &Subject,
        size: u32,
        reload: bool,
    ) -> Result<egui::ColorImage, String> {
        self.load(subject, reload)?;
        let target = OffscreenTarget::new(&self.gpu, size, size);
        self.render(&target, &Orbit::default());
        let pixels = target.read_pixels(&self.gpu).map_err(|e| e.to_string())?;
        let bytes: Vec<u8> = pixels.iter().flatten().copied().collect();
        Ok(egui::ColorImage::from_rgba_unmultiplied(
            [size as usize, size as usize],
            &bytes,
        ))
    }
}

/// The preview scene of `subject`: its root first.
fn subject_nodes(subject: &Subject) -> Vec<NodeDesc> {
    let root = NodeDesc {
        name: "Preview".into(),
        kind: Some("Node3D".into()),
        ..Default::default()
    };
    let child = |desc: NodeDesc| NodeDesc {
        parent: Some(".".into()),
        ..desc
    };
    let body = match subject {
        Subject::Model(path) | Subject::Scene(path) => NodeDesc {
            name: "Subject".into(),
            instance: Some(path.clone()),
            ..Default::default()
        },
        Subject::Material(path, shape) => NodeDesc {
            name: "Subject".into(),
            kind: Some("MeshInstance3D".into()),
            mesh: Some(match shape {
                Shape::Sphere => MeshDesc::Sphere {
                    radius: 0.5,
                    segments: 64,
                    rings: 32,
                },
                Shape::Cube => MeshDesc::Cube { size: 0.8 },
                Shape::Plane => MeshDesc::Plane {
                    size: 1.2,
                    subdivisions: 1,
                },
            }),
            rotation: matches!(shape, Shape::Plane).then_some([60.0, 0.0, 0.0]),
            material: Some(MaterialDesc {
                file: Some(path.clone()),
                ..Default::default()
            }),
            ..Default::default()
        },
    };
    vec![root, child(body)]
}

fn describe(tree: &ab_engine::scene::SceneTree, root: NodeHandle) -> SubjectInfo {
    let assets = tree.world().resource::<RenderAssets>();
    let mut info = SubjectInfo::default();
    let mut materials = Vec::new();
    for node in tree.subtree(root) {
        info.nodes += 1;
        if tree.component::<ab_engine::rendering::Skin>(node).is_some() {
            info.skinned = true;
        }
        let Some(instance) = tree.component::<MeshInstance3D>(node).map(|m| *m) else {
            continue;
        };
        info.meshes += 1;
        if !materials.contains(&instance.material) {
            materials.push(instance.material);
        }
        if let Some(mesh) = assets.as_ref().and_then(|a| a.mesh(instance.mesh)) {
            info.vertices += mesh.vertices.len();
            info.triangles += mesh.indices.len() / 3;
        }
    }
    info.materials = materials.len();
    if let Some(assets) = &assets {
        let mut textures = Vec::new();
        for material in materials.iter().filter_map(|&m| assets.material(m)) {
            for t in [
                material.albedo_texture,
                material.normal_texture,
                material.metallic_roughness_texture,
                material.emission_texture,
            ]
            .into_iter()
            .flatten()
            {
                if !textures.contains(&t) {
                    textures.push(t);
                }
            }
        }
        info.textures = textures.len();
    }
    if let Some(player) = tree.subtree(root).into_iter().find_map(|n| {
        tree.component::<ab_engine::animation::AnimationPlayer>(n)
            .map(|p| p.clip_names().map(str::to_owned).collect::<Vec<_>>())
    }) {
        info.animations = player;
    }
    info.size = tree.subtree_bounds(root).map(|b| b.size());
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    fn previewer() -> Option<Previewer> {
        let gpu = GpuContext::new_headless().ok()?;
        let assets = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        Some(Previewer::new(gpu, &assets))
    }

    #[test]
    fn models_are_described_and_drawn() {
        let Some(mut previewer) = previewer() else {
            eprintln!("no GPU; skipped");
            return;
        };
        let crate_model = Subject::Model("models/crate.glb".into());
        previewer.load(&crate_model, false).unwrap();
        let info = previewer.info.clone();
        assert!(info.meshes >= 1 && info.triangles >= 12, "{info:?}");
        assert!(info.size.is_some_and(|s| s.length() > 0.1));
        let image = previewer.thumbnail(&crate_model, 64, false).unwrap();
        assert_eq!(image.size, [64, 64]);
        // Something is drawn in the middle, unlike the corner's background.
        let corner = image.pixels[0];
        let middle = image.pixels[32 * 64 + 32];
        assert_ne!(corner, middle);

        let rigged = Subject::Model("models/mannequin.gltf".into());
        previewer.load(&rigged, false).unwrap();
        assert!(previewer.info.skinned && !previewer.info.animations.is_empty());
        assert!(previewer
            .load(&Subject::Model("models/none.glb".into()), false)
            .is_err());
    }
}
