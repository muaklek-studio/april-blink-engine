//! Preloading a level in the background, then building it from the cache.

use ab_engine::asset::AssetServer;
use ab_engine::prelude::*;
use ab_engine::rendering::RenderAssets;
use ab_engine::scene_file::{preload_progress, preload_scene};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn copy(from: &Path, to: &Path, files: &[&str]) {
    for file in files {
        let target = to.join(file);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(from.join(file), target).unwrap();
    }
}

#[test]
fn preloaded_level_builds_without_loading() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("preload-assets");
    let _ = std::fs::remove_dir_all(&root);
    let samples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    copy(
        &samples,
        &root,
        &[
            "models/totem.gltf",
            "models/totem_albedo.png",
            "models/crate.glb",
            "audio/hum.wav",
            "materials/totem.abmat",
        ],
    );
    std::fs::create_dir_all(root.join("props")).unwrap();
    std::fs::write(
        root.join("props/totem.abscene"),
        r#"
        [[node]]
        name = "Totem"
        instance = "models/totem.gltf"

        [[node]]
        name = "Hum"
        parent = "."
        type = "AudioStreamPlayer3D"
        audio = { stream = "audio/hum.wav" }
        "#,
    )
    .unwrap();
    std::fs::write(
        root.join("level.abscene"),
        r#"
        [[node]]
        name = "Level"
        type = "Node3D"

        [[node]]
        name = "Crate"
        parent = "."
        instance = "models/crate.glb"

        [[node]]
        name = "Totem"
        parent = "."
        instance = "props/totem.abscene"

        [[node]]
        name = "Floor"
        parent = "."
        type = "MeshInstance3D"
        mesh = { primitive = "plane" }
        material = { file = "materials/totem.abmat" }
        "#,
    )
    .unwrap();

    let config = EngineConfig {
        asset_root: root,
        audio_output: ab_engine::audio::AudioOutput::Silent,
        ..Default::default()
    };
    let mut engine = Engine::new(&config);
    preload_scene(engine.tree_mut(), "level.abscene");
    let start = Instant::now();
    let progress = loop {
        engine.update(1.0 / 60.0);
        let progress = preload_progress(engine.tree(), "level.abscene").unwrap();
        if progress.ready {
            break progress;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "preload hangs");
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(progress.error, None);
    assert_eq!(progress.total, progress.loaded);
    assert!(progress.total >= 3, "models and the sound: {progress:?}");
    let world = engine.tree().world();
    let server = world.resource::<AssetServer>().unwrap();
    assert!(server.is_loaded("models/crate.glb"));
    assert!(server.is_loaded("models/totem.gltf"));
    assert!(server.is_loaded("audio/hum.wav"));
    drop(server);
    let textures = world.resource::<RenderAssets>().unwrap().texture_count();

    let level = load_scene(engine.tree_mut(), "level.abscene").unwrap();
    let tree = engine.tree_mut();
    tree.add_child(tree.root(), level).unwrap();
    let render = engine.tree().world().resource::<RenderAssets>().unwrap();
    assert_eq!(
        render.texture_count(),
        textures,
        "every texture came from the cache"
    );
    drop(render);

    let missing = preload_progress(engine.tree(), "nothing.abscene");
    assert!(missing.is_none());

    // A file changes (hot reload evicts it): the preload is no longer ready, loads it
    // again, and is ready once more.
    let world = engine.tree().world();
    world
        .resource_mut::<AssetServer>()
        .unwrap()
        .evict("audio/hum.wav");
    let progress = preload_progress(engine.tree(), "level.abscene").unwrap();
    assert!(!progress.ready, "{progress:?}");
    let start = Instant::now();
    while !preload_progress(engine.tree(), "level.abscene")
        .unwrap()
        .ready
    {
        engine.update(1.0 / 60.0);
        assert!(start.elapsed() < Duration::from_secs(20), "reload hangs");
        std::thread::sleep(Duration::from_millis(2));
    }
    let world = engine.tree().world();
    assert!(world
        .resource::<AssetServer>()
        .unwrap()
        .is_loaded("audio/hum.wav"));
}
