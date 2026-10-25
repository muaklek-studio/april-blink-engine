//! Loading a level ahead of time, so building it does not stall the frame.
//!
//! [`preload_scene`] reads the scene file on a background thread, follows its instanced
//! scenes (with their overrides) and material files, and requests every model, texture and
//! sound they use from the [`AssetServer`] (which loads them on its own threads). Watch
//! [`preload_progress`] (a loading screen, or the next area while the player walks there);
//! once it is ready, [`load_scene`](super::load_scene) finds everything in the cache.
//!
//! ```no_run
//! # fn demo(tree: &mut ab_scene::SceneTree) {
//! use ab_engine::scene_file::{load_scene, preload_progress, preload_scene};
//!
//! preload_scene(tree, "levels/two.abscene");
//! // … each frame:
//! if preload_progress(tree, "levels/two.abscene").is_some_and(|p| p.ready) {
//!     let level = load_scene(tree, "levels/two.abscene").unwrap();
//!     tree.add_child(tree.root(), level).unwrap();
//! }
//! # }
//! ```
//!
//! Animation clip files are small and still read when the scene is built; generators build
//! their pieces then too.

use super::format::{MaterialDesc, SceneFile};
use super::is_scene_path;
use super::load::MAX_DEPTH;
use ab_asset::{normalize_path, AssetRequest, AssetServer, AssetSource, LoadStatus};
use ab_core::jobs::{self, Task};
use ab_ecs::World;
use ab_rendering::ColorSpace;
use ab_scene::SceneTree;
use std::collections::{HashMap, HashSet};
use tracing::warn;

/// How far a [`preload_scene`] has got.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PreloadProgress {
    /// Assets loaded (or failed: building the scene reports those).
    pub loaded: usize,
    /// Assets the scene uses; 0 while the scene file is still being read.
    pub total: usize,
    /// Everything is loaded: building the scene will not wait on the disk.
    pub ready: bool,
    /// The scene file itself could not be read (the preload is over; loading the scene
    /// reports the same error).
    pub error: Option<String>,
}

impl PreloadProgress {
    /// 0..=1, for a progress bar.
    pub fn fraction(&self) -> f32 {
        match (self.ready, self.total) {
            (true, _) => 1.0,
            (false, 0) => 0.0,
            (false, total) => self.loaded as f32 / total as f32,
        }
    }
}

struct Preload {
    /// Reading the scene files, until it has finished.
    scan: Option<Task<Result<Vec<AssetRequest>, String>>>,
    /// The assets the scene uses (normalised paths).
    dependencies: Vec<AssetRequest>,
    error: Option<String>,
}

/// ECS resource: the scenes being preloaded, by normalised path. Updated by
/// [`update_preloads`].
#[derive(Default)]
pub struct ScenePreloads {
    scenes: HashMap<String, Preload>,
}

impl std::fmt::Debug for ScenePreloads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScenePreloads")
            .field("scenes", &self.scenes.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Starts loading everything the scene file at `path` needs in the background (see the
/// module docs). Calling it again for the same scene restarts the preload, picking up
/// changed files.
pub fn preload_scene(tree: &mut SceneTree, path: &str) {
    let key = normalize_path(path).unwrap_or_else(|| path.to_owned());
    let world = tree.world_mut();
    let Some(source) = world.resource::<AssetServer>().map(|s| s.source()) else {
        warn!("cannot preload {path}: the AssetServer resource is missing");
        return;
    };
    let scan_path = key.clone();
    let scan = jobs::spawn_background("ab-scene-preload", move || {
        scan(source.as_ref(), &scan_path)
    });
    world
        .resource_or_insert_with(ScenePreloads::default)
        .scenes
        .insert(
            key,
            Preload {
                scan: Some(scan),
                dependencies: Vec::new(),
                error: None,
            },
        );
}

/// How far the preload of `path` has got; `None` if [`preload_scene`] was not called for
/// it.
pub fn preload_progress(tree: &SceneTree, path: &str) -> Option<PreloadProgress> {
    let key = normalize_path(path).unwrap_or_else(|| path.to_owned());
    let world = tree.world();
    let preloads = world.resource::<ScenePreloads>()?;
    let preload = preloads.scenes.get(&key)?;
    if let Some(error) = &preload.error {
        return Some(PreloadProgress {
            ready: true,
            error: Some(error.clone()),
            ..PreloadProgress::default()
        });
    }
    if preload.scan.is_some() {
        return Some(PreloadProgress::default());
    }
    let server = world.resource::<AssetServer>()?;
    let total = preload.dependencies.len();
    // Settled: loaded, or failed (building the scene reports it). One evicted meanwhile
    // (its file changed) is requested again by `update_preloads`.
    let loaded = preload
        .dependencies
        .iter()
        .filter(|d| matches!(server.status(d), LoadStatus::Loaded | LoadStatus::Failed))
        .count();
    Some(PreloadProgress {
        loaded,
        total,
        ready: loaded == total,
        error: None,
    })
}

/// Forgets the preload of `path` (what it loaded stays cached in the [`AssetServer`]).
pub fn forget_preload(tree: &mut SceneTree, path: &str) {
    let key = normalize_path(path).unwrap_or_else(|| path.to_owned());
    if let Some(mut preloads) = tree.world().resource_mut::<ScenePreloads>() {
        preloads.scenes.remove(&key);
    }
}

/// Requests the assets of scenes whose files have been read, and again any of them that
/// was evicted since (its file changed). The engine runs it in `PreUpdate`.
pub fn update_preloads(world: &mut World) {
    let world: &World = world;
    let (Some(mut preloads), Some(mut server)) = (
        world.resource_mut::<ScenePreloads>(),
        world.resource_mut::<AssetServer>(),
    ) else {
        return;
    };
    for (path, preload) in &mut preloads.scenes {
        if let Some(scan) = &mut preload.scan {
            if !scan.is_finished() {
                continue;
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan.try_take()))
                .unwrap_or_else(|_| Some(Err("reading the scene failed".to_owned())));
            preload.scan = None;
            match result {
                Some(Ok(dependencies)) => preload.dependencies = dependencies,
                Some(Err(error)) => {
                    warn!("cannot preload {path}: {error}");
                    preload.error = Some(error);
                }
                None => preload.error = Some("reading the scene failed".to_owned()),
            }
        }
        for dependency in &preload.dependencies {
            if server.status(dependency) == LoadStatus::NotLoaded {
                // Paths were checked by the scan: requesting cannot fail.
                let _ = server.request(dependency);
            }
        }
    }
}

/// Reads the scene at `path` and the scenes and materials it names, listing every asset
/// they use (each once). Only the top scene must be readable: a missing instance is
/// skipped (building the scene puts a placeholder there).
fn scan(source: &dyn AssetSource, path: &str) -> Result<Vec<AssetRequest>, String> {
    let mut scan = Scan {
        source,
        scenes: HashMap::new(),
        found: HashSet::new(),
        dependencies: Vec::new(),
    };
    let scene = scan.read_scene(path)?;
    scan.walk(&scene, 0);
    Ok(scan.dependencies)
}

struct Scan<'a> {
    source: &'a dyn AssetSource,
    /// Scene files read so far (a prefab placed a hundred times is read once).
    scenes: HashMap<String, SceneFile>,
    found: HashSet<AssetRequest>,
    dependencies: Vec<AssetRequest>,
}

impl Scan<'_> {
    fn read(&self, path: &str) -> Result<String, String> {
        let normalized = normalize_path(path)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| format!("`{path}` is not a valid asset path"))?;
        let bytes = self
            .source
            .read(&normalized)
            .map_err(|e| format!("cannot read `{path}`: {e}"))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn read_scene(&mut self, path: &str) -> Result<SceneFile, String> {
        if let Some(scene) = self.scenes.get(path) {
            return Ok(scene.clone());
        }
        let scene = SceneFile::from_toml(&self.read(path)?)
            .map_err(|e| format!("invalid scene `{path}`: {e}"))?;
        self.scenes.insert(path.to_owned(), scene.clone());
        Ok(scene)
    }

    /// Lists `dependency` (once, by normalised path).
    fn add(&mut self, dependency: AssetRequest) {
        if dependency.path().trim().is_empty() {
            return;
        }
        let Some(path) = normalize_path(dependency.path()).filter(|p| !p.is_empty()) else {
            warn!("preload: `{}` is not a valid asset path", dependency.path());
            return;
        };
        let dependency = match dependency {
            AssetRequest::Gltf(_) => AssetRequest::Gltf(path),
            AssetRequest::Texture(_, space) => AssetRequest::Texture(path, space),
            AssetRequest::Audio(_) => AssetRequest::Audio(path),
        };
        if self.found.insert(dependency.clone()) {
            self.dependencies.push(dependency);
        }
    }

    fn walk(&mut self, scene: &SceneFile, depth: u32) {
        for node in &scene.nodes {
            if let Some(instance) = &node.instance {
                self.instance(instance, &node.overrides, depth);
            }
            if let Some(material) = &node.material {
                self.material(material);
            }
            if let Some(audio) = &node.audio {
                self.add(AssetRequest::Audio(audio.stream.clone()));
            }
            if let Some(image) = &node.image {
                self.add(AssetRequest::Texture(
                    image.texture.clone(),
                    ColorSpace::Srgb,
                ));
            }
            if let Some(piece) = node.path.as_ref().and_then(|p| p.piece.as_ref()) {
                self.instance(piece, &[], depth);
            }
        }
    }

    fn instance(&mut self, path: &str, overrides: &[toml::Table], depth: u32) {
        if !is_scene_path(path) {
            self.add(AssetRequest::Gltf(path.to_owned()));
            return;
        }
        if depth + 1 > MAX_DEPTH {
            return; // A cycle: building the scene reports it.
        }
        match self.read_scene(path) {
            Ok(mut scene) => {
                super::prefab::apply_overrides(&mut scene, overrides);
                self.walk(&scene, depth + 1);
            }
            Err(err) => warn!("preload: {err}"),
        }
    }

    fn material(&mut self, material: &MaterialDesc) {
        let material = match material.file.as_deref().filter(|p| !p.is_empty()) {
            Some(file) => {
                match self
                    .read(file)
                    .and_then(|text| MaterialDesc::from_toml(&text).map_err(|e| e.to_string()))
                {
                    Ok(base) => material.over(&base),
                    Err(err) => {
                        warn!("preload: material {file}: {err}");
                        material.clone()
                    }
                }
            }
            None => material.clone(),
        };
        let textures = [
            (material.albedo_texture, ColorSpace::Srgb),
            (material.normal_texture, ColorSpace::Linear),
            (material.metallic_roughness_texture, ColorSpace::Linear),
            (material.emission_texture, ColorSpace::Srgb),
        ];
        for (texture, space) in textures {
            if let Some(texture) = texture {
                self.add(AssetRequest::Texture(texture, space));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_asset::MemorySource;

    fn scanned(source: MemorySource, path: &str) -> Vec<AssetRequest> {
        scan(&source, path).unwrap()
    }

    #[test]
    fn finds_models_textures_and_sounds_through_prefabs_and_materials() {
        let mut source = MemorySource::new();
        source
            .insert(
                "levels/one.abscene",
                r#"
                [[node]]
                name = "Level"
                type = "Node3D"

                [[node]]
                name = "Crate"
                parent = "."
                instance = "models/crate.glb"

                [[node]]
                name = "Lamp"
                parent = "."
                instance = "props/lamp.abscene"
                overrides = [{ path = "Hum", audio = { stream = "audio/buzz.ogg" } }]

                [[node]]
                name = "Lamp2"
                parent = "."
                instance = "props/lamp.abscene"

                [[node]]
                name = "Wall"
                parent = "."
                type = "MeshInstance3D"
                mesh = { primitive = "cube" }
                material = { file = "materials/brick.abmat", normal_texture = "tex/n.png" }

                [[node]]
                name = "Missing"
                parent = "."
                instance = "props/gone.abscene"
                "#,
            )
            .insert(
                "props/lamp.abscene",
                r#"
                [[node]]
                name = "Lamp"
                type = "Node3D"

                [[node]]
                name = "Hum"
                parent = "."
                type = "AudioStreamPlayer3D"
                audio = { stream = "audio/hum.wav" }

                [[node]]
                name = "Model"
                parent = "."
                instance = "models/crate.glb"
                "#,
            )
            .insert(
                "materials/brick.abmat",
                r#"albedo_texture = "tex/brick.png""#,
            );

        let found = scanned(source, "levels/one.abscene");
        assert_eq!(
            found,
            [
                AssetRequest::Gltf("models/crate.glb".into()),
                AssetRequest::Audio("audio/buzz.ogg".into()),
                AssetRequest::Audio("audio/hum.wav".into()),
                AssetRequest::Texture("tex/brick.png".into(), ColorSpace::Srgb),
                AssetRequest::Texture("tex/n.png".into(), ColorSpace::Linear),
            ]
        );
    }

    #[test]
    fn a_scene_instancing_itself_ends() {
        let mut source = MemorySource::new();
        source.insert(
            "loop.abscene",
            r#"
            [[node]]
            name = "Loop"
            instance = "loop.abscene"
            "#,
        );
        assert!(scanned(source, "loop.abscene").is_empty());
        assert!(scan(&MemorySource::new(), "none.abscene").is_err());
    }
}
