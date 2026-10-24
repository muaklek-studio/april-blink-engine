//! Scene files: levels and prefabs described in TOML, instantiated into the tree, and saved
//! back from it.
//!
//! Scene files end in `.abscene` ([`SCENE_EXTENSION`]); the text inside is TOML.
//!
//! The format is a flat list of nodes like Godot's `.tscn`: the first node is the scene's
//! root, every other names its `parent` by path from the root (`"."` is the root itself).
//!
//! A file starts with its `format_version` ([`SCENE_FORMAT_VERSION`]; files without one are
//! version 1). Older versions are upgraded when read; newer ones are refused.
//!
//! ```toml
//! format_version = 1
//!
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
//! Colors are `"#rrggbb[aa]"` (sRGB) or `[r, g, b(, a)]` (linear).
//!
//! UI nodes (`CanvasLayer`, `Control`, `Panel`, `Label`, `Button`, `ProgressBar`,
//! `VBoxContainer`, `HBoxContainer`, `TextureRect`) place themselves with `control` and read
//! `canvas`, `label`, `button`, `panel`, `progress`, `box` or `image`: see [`ui`].

mod format;
mod load;
pub mod generator;
pub mod path;
pub mod prefab;
mod save;
pub mod ui;

pub use format::{
    AnimationDesc, AudioDesc, BodyDesc, BoxDesc, ButtonDesc, CameraDesc, CanvasDesc, CharacterDesc,
    ColorDesc, ControlDesc, EnvironmentDesc, ImageDesc, JointDesc, LabelDesc, LightDesc,
    GeneratorDesc, MaterialDesc, MeshDesc, NodeDesc, PathDesc, PanelDesc, ParticlesDesc, ProgressDesc, RagdollDesc,
    SceneFile, ShapeDesc, TimerDesc, SCENE_FORMAT_VERSION,
};
pub use load::{
    apply_environment, install_clips, instantiate_scene, load_animations, load_scene,
    material_handle, mesh_and_material, particle_emitter, resolve_material, SceneAssetCache, SceneFileError,
};
pub use save::{relative_path, save_contents, save_scene};

/// File extension of scene files (TOML text inside).
pub const SCENE_EXTENSION: &str = "abscene";

/// File extension of material files: the keys of a `material = { … }` table, in TOML.
pub const MATERIAL_EXTENSION: &str = "abmat";

/// Whether `path` names a scene file (`.abscene`).
pub fn is_scene_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(SCENE_EXTENSION))
}

/// `path` with the scene extension, added unless it is there. Other extensions are kept as
/// typed (`level.v2` becomes `level.v2.abscene`).
pub fn with_scene_extension(path: &str) -> String {
    if is_scene_path(path) {
        path.to_owned()
    } else {
        format!("{path}.{SCENE_EXTENSION}")
    }
}

/// Marks nodes built from another node's description (a path's road mesh, a generator's
/// output): rebuilt on load, never saved, not edited by hand.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Generated;

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
    pub animation: Option<AnimationDesc>,
    /// A `TextureRect`'s image file.
    pub image: Option<ImageDesc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_extensions() {
        assert!(is_scene_path("levels/one.abscene"));
        assert!(is_scene_path("levels/ONE.AbScene"));
        assert!(!is_scene_path("anims/door.toml"));
        assert!(!is_scene_path("models/crate.glb"));
        assert!(!is_scene_path("abscene"));
        assert_eq!(with_scene_extension("scenes/a"), "scenes/a.abscene");
        assert_eq!(with_scene_extension("scenes/a.abscene"), "scenes/a.abscene");
        assert_eq!(
            with_scene_extension("my.levels/a.v2"),
            "my.levels/a.v2.abscene"
        );
    }
}
