//! Scene files: levels and prefabs described in TOML, instantiated into the tree, and saved
//! back from it.
//!
//! The format is a flat list of nodes like Godot's `.tscn`: the first node is the scene's
//! root, every other names its `parent` by path from the root (`"."` is the root itself).
//!
//! ```toml
//! [environment]
//! background = "#05060a"
//! fog_density = 0.05
//! bloom = 0.15          # also: exposure, contrast, saturation, tint, vignette,
//! vignette = 0.3        # vignette_smoothness, bloom_threshold, chromatic_aberration, grain
//!
//! [[node]]
//! name = "Room"
//! type = "Node3D"
//!
//! [[node]]
//! name = "Floor"
//! parent = "."
//! type = "StaticBody3D"
//! position = [0.0, -0.5, 0.0]
//! scale = [10.0, 1.0, 10.0]
//!
//! [[node]]
//! name = "Mesh"
//! parent = "Floor"
//! type = "MeshInstance3D"
//! mesh = { primitive = "cube" }
//! material = { color = "#6a5f55", roughness = 0.9 }
//!
//! [[node]]
//! name = "Shape"
//! parent = "Floor"
//! type = "CollisionShape3D"
//! shape = { box = [1.0, 1.0, 1.0] }
//!
//! [[node]]
//! name = "Crate"
//! parent = "."
//! instance = "models/crate.glb"   # a glTF model or another scene file
//! position = [2.0, 0.5, 0.0]
//! groups = ["crates"]
//! script = "Spinner"              # by ScriptRegistry name (several: scripts = ["A", "B"])
//! ```
//!
//! Common keys: `name`, `parent`, `type` or `instance`, `position`, `rotation` (degrees,
//! applied Y, X, Z like Godot), `scale`, `visible`, `groups`, `script`. Each node type
//! reads its own table: `mesh` + `material` (+ `cast_shadows`) for `MeshInstance3D`,
//! `camera`, `light`, `body` for the physics bodies, `character`, `shape`, `audio`, `timer`
//! `particles` for `Particles3D`, `joint` for `Joint3D` and `ragdoll` for `Ragdoll`
//! (`AudioListener3D` and `CameraShake` need none):
//!
//! ```toml
//! [[node]]
//! name = "Chimney"
//! parent = "."
//! type = "Particles3D"
//! position = [3.0, 4.0, 0.0]
//! particles = { preset = "smoke", amount = 60, color_start = "#40404080" }
//! ```
//!
//! Joints name their bodies by path from the joint; angles are degrees:
//!
//! ```toml
//! [[node]]
//! name = "Neck"
//! parent = "Dummy"
//! type = "Joint3D"
//! position = [0.0, 1.55, 0.0]
//! joint = { kind = "cone", body_a = "../Chest", body_b = "../Head", swing = 35.0, twist = 40.0 }
//! ```
//! Colors are `"#rrggbb[aa]"` (sRGB) or `[r, g, b(, a)]` (linear). UI is built in code and
//! is not part of scene files.

mod format;
mod load;
mod save;

pub use format::{
    AudioDesc, BodyDesc, CameraDesc, CharacterDesc, ColorDesc, EnvironmentDesc, JointDesc,
    LightDesc, MaterialDesc, MeshDesc, NodeDesc, ParticlesDesc, RagdollDesc, SceneFile, ShapeDesc,
    TimerDesc,
};
pub use load::{
    apply_environment, instantiate_scene, load_scene, mesh_and_material, particle_emitter,
    SceneAssetCache, SceneFileError,
};
pub use save::{relative_path, save_scene};

/// Marks the root of an instanced model or scene, so saving writes `instance = path`
/// instead of its contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneInstance(pub String);

/// Descriptions a node was loaded from that cannot be recovered from its components
/// (procedural meshes, materials, audio file paths); used when saving.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SceneOrigin {
    pub mesh: Option<MeshDesc>,
    pub material: Option<MaterialDesc>,
    pub audio_stream: Option<String>,
    pub particles: Option<ParticlesDesc>,
}
