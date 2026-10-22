//! Scene files: loading every node type, instancing, errors, saving and resource sharing.

use ab_engine::prelude::*;
use ab_engine::scene_file::{instantiate_scene, SceneFileError};
use std::path::PathBuf;

/// Deletes the temporary asset directory when the test ends.
struct TempAssets(PathBuf);

impl Drop for TempAssets {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Scene files in a temporary asset directory, plus the sample crate model and a sound.
fn engine_with(files: &[(&str, &str)]) -> (Engine, TempAssets) {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ab-scene-file-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(dir.join("models")).unwrap();
    std::fs::create_dir_all(dir.join("audio")).unwrap();
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    std::fs::copy(
        assets.join("models/crate.glb"),
        dir.join("models/crate.glb"),
    )
    .unwrap();
    std::fs::copy(assets.join("audio/thud.wav"), dir.join("audio/thud.wav")).unwrap();
    for (path, text) in files {
        std::fs::write(dir.join(path), text).unwrap();
    }
    let config = EngineConfig {
        asset_root: dir.clone(),
        audio_output: ab_engine::audio::AudioOutput::Silent,
        ..Default::default()
    };
    let engine = Engine::new(&config);
    engine
        .tree()
        .world()
        .resource_mut::<ScriptRegistry>()
        .unwrap()
        .register::<Spin>("Spin")
        .register::<Glow>("Glow");
    (engine, TempAssets(dir))
}

#[derive(Default)]
struct Spin;
impl NodeScript for Spin {}

#[derive(Default)]
struct Glow;
impl NodeScript for Glow {}

const LEVEL: &str = r##"
[environment]
background = "#102030"
fog_density = 0.04
exposure = 1.5
bloom = 0.2
bloom_threshold = 0.8
vignette = 0.4
grain = 0.1
tint = "#ffeedd"

[[node]]
name = "Level"
type = "Node3D"

[[node]]
name = "Floor"
parent = "."
type = "StaticBody3D"
position = [0.0, -0.5, 0.0]
scale = [10.0, 1.0, 10.0]
groups = ["solid"]
body = { friction = 0.9 }

[[node]]
name = "Mesh"
parent = "Floor"
type = "MeshInstance3D"
mesh = { primitive = "cube" }
material = { color = "#6a5f55", roughness = 0.9 }

[[node]]
name = "Shape"
parent = "Floor"
type = "CollisionShape3D"
shape = { box = [1.0, 1.0, 1.0] }

[[node]]
name = "Ball"
parent = "."
type = "RigidBody3D"
position = [0.0, 3.0, 0.0]
rotation = [0.0, 90.0, 0.0]
body = { mass = 4.0, bounce = 0.5 }
script = "Spin"

[[node]]
name = "Shape"
parent = "Ball"
type = "CollisionShape3D"
shape = { sphere = 0.5 }

[[node]]
name = "Mesh"
parent = "Ball"
type = "MeshInstance3D"
mesh = { primitive = "sphere", radius = 0.5 }
material = { color = [1.0, 0.1, 0.1], metallic = 1.0, emission = "#ff8040", emission_energy = 2.0 }
cast_shadows = false

[[node]]
name = "Trigger"
parent = "."
type = "Area3D"
position = [3.0, 1.0, 0.0]

[[node]]
name = "Shape"
parent = "Trigger"
type = "CollisionShape3D"
shape = { capsule = [0.5, 2.0] }

[[node]]
name = "Player"
parent = "."
type = "CharacterBody3D"
character = { floor_max_angle = 50.0, max_step_height = 0.3 }

[[node]]
name = "Camera"
parent = "Player"
type = "Camera3D"
position = [0.0, 1.6, 0.0]
camera = { fov = 70.0, current = true }

[[node]]
name = "Sun"
parent = "."
type = "DirectionalLight3D"
rotation = [-60.0, 30.0, 0.0]
light = { color = "#ffeedd", energy = 2.0, shadows = true }

[[node]]
name = "Lamp"
parent = "."
type = "OmniLight3D"
light = { energy = 8.0, range = 6.0 }

[[node]]
name = "Spot"
parent = "."
type = "SpotLight3D"
light = { angle = 25.0, shadows = true }

[[node]]
name = "Hum"
parent = "Lamp"
type = "AudioStreamPlayer3D"
audio = { stream = "audio/thud.wav", volume_db = -6.0, looping = true, max_distance = 12.0 }

[[node]]
name = "Clock"
parent = "."
type = "Timer"
timer = { wait_time = 2.0, one_shot = true }

[[node]]
name = "Crate"
parent = "."
instance = "models/crate.glb"
position = [2.0, 0.5, 2.0]
groups = ["crates"]

[[node]]
name = "Hidden"
parent = "."
type = "Node"
visible = false

[[node]]
name = "Chimney"
parent = "."
type = "Particles3D"
position = [3.0, 4.0, 0.0]
particles = { preset = "smoke", amount = 60, color_start = "#40404080", emission = 1.5 }

[[node]]
name = "Shake"
parent = "."
type = "CameraShake"
"##;

#[test]
fn loads_every_node_type() {
    let (mut engine, _assets) = engine_with(&[("level.toml", LEVEL)]);
    let tree = engine.tree_mut();
    let level = load_scene(tree, "level.toml").unwrap();
    tree.add_child(tree.root(), level).unwrap();

    let node = |path: &str| {
        tree.get_node(level, path)
            .unwrap_or_else(|| panic!("{path}"))
    };
    assert_eq!(tree.class_name(node("Floor")), Some("StaticBody3D"));
    assert_eq!(tree.scale(node("Floor")), Some(Vec3::new(10.0, 1.0, 10.0)));
    assert!(tree.is_in_group(node("Floor"), "solid"));
    assert_eq!(
        tree.component::<PhysicsBody3D>(node("Floor"))
            .unwrap()
            .friction,
        0.9
    );
    assert!(
        tree.component::<MeshInstance3D>(node("Floor/Mesh"))
            .unwrap()
            .cast_shadows
    );
    let ball = node("Ball");
    assert_eq!(tree.component::<PhysicsBody3D>(ball).unwrap().mass, 4.0);
    assert_eq!(tree.script_name(ball).as_deref(), Some("Spin"));
    let forward = tree.rotation(ball).unwrap() * Vec3::NEG_Z;
    assert!(
        forward.abs_diff_eq(Vec3::NEG_X, 1e-5),
        "90° about Y: {forward}"
    );
    assert!(
        !tree
            .component::<MeshInstance3D>(node("Ball/Mesh"))
            .unwrap()
            .cast_shadows
    );
    assert_eq!(tree.class_name(node("Trigger")), Some("Area3D"));
    let character = tree
        .component::<CharacterBody3D>(node("Player"))
        .unwrap()
        .clone();
    assert!((character.floor_max_angle - 50f32.to_radians()).abs() < 1e-6);
    assert_eq!(
        tree.component::<Camera3D>(node("Player/Camera"))
            .unwrap()
            .fov_y_degrees,
        70.0
    );
    assert!(
        tree.component::<DirectionalLight3D>(node("Sun"))
            .unwrap()
            .shadow_enabled
    );
    assert_eq!(
        tree.component::<OmniLight3D>(node("Lamp")).unwrap().range,
        6.0
    );
    assert_eq!(
        tree.component::<SpotLight3D>(node("Spot"))
            .unwrap()
            .angle_degrees,
        25.0
    );
    let hum = tree
        .component::<AudioPlayer>(node("Lamp/Hum"))
        .unwrap()
        .clone();
    assert!(hum.looping && hum.positional.unwrap().max_distance == 12.0);
    assert!(tree.component::<Timer>(node("Clock")).unwrap().one_shot);
    let crate_node = node("Crate");
    assert_eq!(tree.position(crate_node), Some(Vec3::new(2.0, 0.5, 2.0)));
    assert!(tree.is_in_group(crate_node, "crates"));
    assert!(tree.child_count(crate_node) > 0, "glTF content");
    assert!(!tree.is_visible(node("Hidden")));
    let smoke = tree
        .component::<ParticleEmitter>(node("Chimney"))
        .unwrap()
        .clone();
    assert_eq!(smoke.settings.amount, 60);
    assert_eq!(smoke.settings.emission, 1.5);
    assert!(!smoke.settings.additive && smoke.emitting);
    assert!(tree.component::<CameraShake>(node("Shake")).is_some());

    {
        let environment = tree.world().resource::<Environment>().unwrap();
        assert!(environment.fog.enabled && environment.fog.density == 0.04);
        assert_eq!(environment.post.exposure, 1.5);
        let post = &environment.post;
        assert!(post.bloom.is_enabled() && post.bloom.threshold == 0.8);
        assert_eq!((post.vignette_intensity, post.grain), (0.4, 0.1));
        assert!(post.tint.b < post.tint.r, "warm tint");
    }

    // It simulates: the ball falls onto the floor.
    for _ in 0..120 {
        engine.update(1.0 / 60.0);
    }
    let y = engine.tree().position(ball).unwrap().y;
    assert!((y - 0.5).abs() < 0.05, "ball rests on the floor: {y}");
}

#[test]
fn nested_scenes_and_shared_assets() {
    let room = r##"
[[node]]
name = "Room"
type = "Node3D"

[[node]]
name = "Pillar"
parent = "."
type = "MeshInstance3D"
mesh = { primitive = "cube" }
material = { color = "#808080" }
"##;
    let house = r##"
[[node]]
name = "House"
type = "Node3D"

[[node]]
name = "Kitchen"
parent = "."
instance = "room.toml"

[[node]]
name = "Bedroom"
parent = "."
instance = "room.toml"
position = [5.0, 0.0, 0.0]
"##;
    let (mut engine, _assets) = engine_with(&[("room.toml", room), ("house.toml", house)]);
    let tree = engine.tree_mut();
    let house_node = load_scene(tree, "house.toml").unwrap();
    assert!(tree.get_node(house_node, "Kitchen/Pillar").is_some());
    let bedroom = tree.get_node(house_node, "Bedroom").unwrap();
    assert_eq!(tree.position(bedroom), Some(Vec3::new(5.0, 0.0, 0.0)));
    let counts = |tree: &SceneTree| {
        let assets = tree.world().resource::<RenderAssets>().unwrap();
        (assets.mesh_count(), assets.material_count())
    };
    let before = counts(tree);
    // Loading the same scene again reuses meshes and materials.
    load_scene(tree, "house.toml").unwrap();
    assert_eq!(counts(tree), before);

    // Saving writes instances, not their contents.
    let saved = save_scene(tree, house_node);
    assert_eq!(saved.nodes.len(), 3);
    assert_eq!(saved.nodes[1].instance.as_deref(), Some("room.toml"));
}

#[test]
fn save_and_reload_round_trips() {
    let (mut engine, _assets) = engine_with(&[("level.toml", LEVEL)]);
    let tree = engine.tree_mut();
    let level = load_scene(tree, "level.toml").unwrap();
    let first = save_scene(tree, level);
    assert_eq!(first.nodes.len(), LEVEL.matches("[[node]]").count());
    let text = first.to_toml();
    let reparsed = SceneFile::from_toml(&text).unwrap();
    let again = instantiate_scene(tree, &reparsed, "saved").unwrap();
    let second = save_scene(tree, again);
    assert_eq!(first, second, "saved:\n{text}");
    assert!(text.contains("type = \"RigidBody3D\""));
    assert!(text.contains("instance = \"models/crate.glb\""));
    assert!(text.contains("script = \"Spin\""));
}

#[test]
fn nodes_with_several_scripts_load_and_save() {
    let scene = "[[node]]\nname = \"A\"\nscripts = [\"Spin\", \"Glow\"]\n\n\
                 [[node]]\nname = \"B\"\nparent = \".\"\nscript = \"Glow\"\nscripts = [\"Spin\"]\n";
    let (mut engine, _assets) = engine_with(&[("multi.toml", scene)]);
    let tree = engine.tree_mut();
    let a = load_scene(tree, "multi.toml").unwrap();
    assert_eq!(tree.script_names(a), ["Spin", "Glow"]);
    assert!(tree.script::<Spin>(a).is_some() && tree.script::<Glow>(a).is_some());
    let b = tree.get_node(a, "B").unwrap();
    assert_eq!(
        tree.script_names(b),
        ["Glow", "Spin"],
        "`script` first, then `scripts`"
    );

    let saved = save_scene(tree, a);
    let text = saved.to_toml();
    assert!(text.contains("scripts = [\"Spin\", \"Glow\"]"), "{text}");
    assert_eq!(saved.nodes[1].scripts, ["Glow", "Spin"]);
    assert_eq!(
        saved.nodes[1].script, None,
        "several scripts are saved as one list"
    );
    // One script stays `script = ...`.
    tree.remove_script_named(b, "Spin");
    assert_eq!(save_scene(tree, a).nodes[1].script.as_deref(), Some("Glow"));
}

#[test]
fn errors_name_the_file_and_node_and_leave_nothing_behind() {
    let bad_parent = "[[node]]\nname = \"A\"\n\n[[node]]\nname = \"B\"\nparent = \"Nope\"\n";
    let bad_type =
        "[[node]]\nname = \"A\"\n\n[[node]]\nname = \"B\"\nparent = \".\"\ntype = \"Teapot\"\n";
    let bad_script = "[[node]]\nname = \"A\"\nscript = \"Missing\"\n";
    let bad_toml = "[[node]]\nname = \"A\"\nsize = 3\n";
    let cycle = "[[node]]\nname = \"Loop\"\ninstance = \"cycle.toml\"\n";
    let preset =
        "[[node]]\nname = \"P\"\ntype = \"Particles3D\"\nparticles = { preset = \"lava\" }\n";
    let (mut engine, _assets) = engine_with(&[
        ("parent.toml", bad_parent),
        ("type.toml", bad_type),
        ("script.toml", bad_script),
        ("toml.toml", bad_toml),
        ("cycle.toml", cycle),
        ("preset.toml", preset),
    ]);
    let tree = engine.tree_mut();
    let nodes = tree.node_count();
    let message =
        |tree: &mut SceneTree, path: &str| load_scene(tree, path).unwrap_err().to_string();

    let m = message(tree, "parent.toml");
    assert!(
        m.contains("parent.toml") && m.contains("`B`") && m.contains("`Nope`"),
        "{m}"
    );
    let m = message(tree, "type.toml");
    assert!(m.contains("unknown type `Teapot`"), "{m}");
    let m = message(tree, "script.toml");
    assert!(m.contains("no script is registered as `Missing`"), "{m}");
    let m = message(tree, "toml.toml");
    assert!(
        m.contains("toml.toml") && m.contains("size") && m.contains("line 3"),
        "{m}"
    );
    let m = message(tree, "preset.toml");
    assert!(
        m.contains("unknown particle preset `lava`") && m.contains("smoke"),
        "{m}"
    );
    let m = message(tree, "cycle.toml");
    assert!(m.contains("nested deeper"), "{m}");
    assert!(matches!(
        load_scene(tree, "missing.toml"),
        Err(SceneFileError::Asset { .. })
    ));
    assert_eq!(
        tree.node_count(),
        nodes,
        "failed loads free what they built"
    );
}
