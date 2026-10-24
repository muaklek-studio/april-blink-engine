//! Every scene shipped in `assets/` loads, survives a save round trip, and runs for a few
//! seconds without a panic or a node flying off to NaN or infinity.

use ab_engine::prelude::*;
use ab_engine::scene_file::SceneFile;
use std::path::{Path, PathBuf};

fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// The scene files under `dir`, relative to the asset folder.
fn scenes(dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            scenes(&path, out);
        } else if path.extension().is_some_and(|e| e == "abscene") {
            let relative = path.strip_prefix(assets()).unwrap();
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[derive(Default)]
struct Inert;
impl NodeScript for Inert {}

#[test]
fn sample_scenes_load_round_trip_and_run() {
    let mut found = Vec::new();
    scenes(&assets(), &mut found);
    found.sort();
    assert!(!found.is_empty(), "no scenes in {}", assets().display());

    for path in &found {
        let text = std::fs::read_to_string(assets().join(path)).unwrap();
        let scene = SceneFile::from_toml(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(
            SceneFile::from_toml(&scene.to_toml()).unwrap(),
            scene,
            "{path} changes when saved"
        );

        let config = EngineConfig {
            asset_root: assets(),
            audio_output: ab_engine::audio::AudioOutput::Silent,
            ..Default::default()
        };
        let mut engine = Engine::new(&config);
        // The games' own scripts are not linked here: they load as scripts that do nothing.
        engine
            .tree()
            .world()
            .resource_mut::<ScriptRegistry>()
            .unwrap()
            .set_fallback(Some(std::sync::Arc::new(|| {
                Box::new(Inert) as Box<dyn NodeScript>
            })));
        let root = load_scene(engine.tree_mut(), path).unwrap_or_else(|e| panic!("{path}: {e}"));
        for _ in 0..180 {
            engine.update(1.0 / 60.0);
        }
        let tree = engine.tree();
        let nodes = tree.subtree(root);
        // Instances and generators may add nodes; none described may be missing.
        assert!(nodes.len() >= scene.nodes.len(), "{path}: nodes missing");
        for node in nodes {
            if let Some(transform) = tree.global_transform(node) {
                let name = tree.name(node).map(|n| n.to_owned()).unwrap_or_default();
                assert!(
                    transform.is_finite(),
                    "{path}: node {name} has a non-finite transform after 3 s: {transform:?}"
                );
            }
        }
    }
}
