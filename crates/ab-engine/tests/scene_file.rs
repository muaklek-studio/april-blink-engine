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
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
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
fog_patchiness = 0.6
fog_wind = [2.0, 0.0, -1.0]
fog_sun_scattering = 0.8
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
    let (mut engine, _assets) = engine_with(&[("level.abscene", LEVEL)]);
    let tree = engine.tree_mut();
    let level = load_scene(tree, "level.abscene").unwrap();
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
        let fog = &environment.fog;
        assert_eq!((fog.patchiness, fog.sun_scattering), (0.6, 0.8));
        assert_eq!(fog.wind, Vec3::new(2.0, 0.0, -1.0));
        assert_eq!(fog.patch_size, 14.0, "unset fields keep their defaults");
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
instance = "room.abscene"

[[node]]
name = "Bedroom"
parent = "."
instance = "room.abscene"
position = [5.0, 0.0, 0.0]
"##;
    let (mut engine, _assets) = engine_with(&[("room.abscene", room), ("house.abscene", house)]);
    let tree = engine.tree_mut();
    let house_node = load_scene(tree, "house.abscene").unwrap();
    assert!(tree.get_node(house_node, "Kitchen/Pillar").is_some());
    let bedroom = tree.get_node(house_node, "Bedroom").unwrap();
    assert_eq!(tree.position(bedroom), Some(Vec3::new(5.0, 0.0, 0.0)));
    let counts = |tree: &SceneTree| {
        let assets = tree.world().resource::<RenderAssets>().unwrap();
        (assets.mesh_count(), assets.material_count())
    };
    let before = counts(tree);
    // Loading the same scene again reuses meshes and materials.
    load_scene(tree, "house.abscene").unwrap();
    assert_eq!(counts(tree), before);

    // Saving writes instances, not their contents.
    let saved = save_scene(tree, house_node);
    assert_eq!(saved.nodes.len(), 3);
    assert_eq!(saved.nodes[1].instance.as_deref(), Some("room.abscene"));
}

#[test]
fn save_and_reload_round_trips() {
    let (mut engine, _assets) = engine_with(&[("level.abscene", LEVEL)]);
    let tree = engine.tree_mut();
    let level = load_scene(tree, "level.abscene").unwrap();
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
    let (mut engine, _assets) = engine_with(&[("multi.abscene", scene)]);
    let tree = engine.tree_mut();
    let a = load_scene(tree, "multi.abscene").unwrap();
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
    let cycle = "[[node]]\nname = \"Loop\"\ninstance = \"cycle.abscene\"\n";
    let preset =
        "[[node]]\nname = \"P\"\ntype = \"Particles3D\"\nparticles = { preset = \"lava\" }\n";
    let (mut engine, _assets) = engine_with(&[
        ("parent.abscene", bad_parent),
        ("type.abscene", bad_type),
        ("script.abscene", bad_script),
        ("toml.abscene", bad_toml),
        ("cycle.abscene", cycle),
        ("preset.abscene", preset),
    ]);
    let tree = engine.tree_mut();
    let nodes = tree.node_count();
    let message =
        |tree: &mut SceneTree, path: &str| load_scene(tree, path).unwrap_err().to_string();

    let m = message(tree, "parent.abscene");
    assert!(
        m.contains("parent.abscene") && m.contains("`B`") && m.contains("`Nope`"),
        "{m}"
    );
    let m = message(tree, "type.abscene");
    assert!(m.contains("unknown type `Teapot`"), "{m}");
    let m = message(tree, "script.abscene");
    assert!(m.contains("no script is registered as `Missing`"), "{m}");
    let m = message(tree, "toml.abscene");
    assert!(
        m.contains("toml.abscene") && m.contains("size") && m.contains("line 3"),
        "{m}"
    );
    let m = message(tree, "preset.abscene");
    assert!(
        m.contains("unknown particle preset `lava`") && m.contains("smoke"),
        "{m}"
    );
    let m = message(tree, "cycle.abscene");
    assert!(m.contains("nested deeper"), "{m}");
    assert!(matches!(
        load_scene(tree, "missing.abscene"),
        Err(SceneFileError::Asset { .. })
    ));
    assert_eq!(
        tree.node_count(),
        nodes,
        "failed loads free what they built"
    );
}

#[test]
fn animation_players_play_clip_files_and_save_back() {
    use ab_engine::scene_file::save_scene;
    let dir = std::env::temp_dir().join(format!("ab-anim-scene-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("anims")).unwrap();
    std::fs::write(
        dir.join("anims/lift.toml"),
        "name = \"Lift\"\n\n[[track]]\ntarget = \"Platform\"\nproperty = \"position\"\nkeys = [[0.0, [0.0, 0.0, 0.0]], [2.0, [0.0, 4.0, 0.0]]]\n",
    )
    .unwrap();
    let config = EngineConfig {
        asset_root: dir.clone(),
        audio_output: ab_engine::audio::AudioOutput::Silent,
        ..Default::default()
    };
    let mut engine = Engine::new(&config);
    let scene = SceneFile::from_toml(
        "[[node]]\nname = \"Level\"\ntype = \"Node3D\"\n\n\
         [[node]]\nname = \"Platform\"\nparent = \".\"\ntype = \"Node3D\"\n\n\
         [[node]]\nname = \"Animator\"\nparent = \".\"\ntype = \"AnimationPlayer\"\nanimation = { clips = [\"anims/lift.toml\"], autoplay = \"Lift\", looping = false }\n",
    )
    .unwrap();
    let tree = engine.tree_mut();
    let level = instantiate_scene(tree, &scene, "test").unwrap();
    tree.add_child(tree.root(), level).unwrap();
    for _ in 0..60 {
        engine.update(1.0 / 60.0);
    }
    let platform = engine.tree().get_node(level, "Platform").unwrap();
    let y = engine.tree().position(platform).unwrap().y;
    assert!((y - 2.0).abs() < 0.1, "half way up after a second: {y}");
    for _ in 0..90 {
        engine.update(1.0 / 60.0);
    }
    let y = engine.tree().position(platform).unwrap().y;
    assert!((y - 4.0).abs() < 1e-3, "stops at the top: {y}");
    let saved = save_scene(engine.tree(), level);
    let animator = saved.nodes.iter().find(|n| n.name == "Animator").unwrap();
    assert_eq!(animator.kind.as_deref(), Some("AnimationPlayer"));
    let animation = animator.animation.as_ref().unwrap();
    assert_eq!(animation.clips, ["anims/lift.toml"]);
    assert_eq!(animation.autoplay.as_deref(), Some("Lift"));
    // A missing clip file is a clear error.
    let broken = SceneFile::from_toml(
        "[[node]]\nname = \"A\"\ntype = \"AnimationPlayer\"\nanimation = { clips = [\"anims/none.toml\"] }\n",
    )
    .unwrap();
    let err = instantiate_scene(engine.tree_mut(), &broken, "broken").unwrap_err();
    assert!(err.to_string().contains("anims/none.toml"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

const HUD: &str = r##"
[[node]]
name = "Hud"
type = "CanvasLayer"
canvas = { layer = 2 }

[[node]]
name = "Health"
parent = "."
type = "ProgressBar"
control = { preset = "top_left", size = [360, 28], margin = 32 }
progress = { value = 75, fill_color = "#d23c3c", show_percentage = true }

[[node]]
name = "Menu"
parent = "."
type = "Panel"
control = { preset = "center", size = [400, 300] }
panel = { color = "#101820e0", corner_radius = 12, border_width = 2, border_color = "#ffffff40" }

[[node]]
name = "Items"
parent = "Menu"
type = "VBoxContainer"
control = { preset = "full_rect", margin = 24 }
box = { separation = 12, alignment = "center" }

[[node]]
name = "Title"
parent = "Menu/Items"
type = "Label"
label = { text = "Paused", size = 48, align = "center", outline = 3 }

[[node]]
name = "Resume"
parent = "Menu/Items"
type = "Button"
control = { min_size = [0, 56] }
button = { text = "Resume", color = "#2f6fd0" }

[[node]]
name = "Logo"
parent = "."
type = "TextureRect"
control = { preset = "bottom_right", size = [128, 128], margin = 16, mouse = "stop" }
image = { texture = "logo.png", stretch = "keep_aspect", tint = "#ffffffc0" }
"##;

#[test]
fn ui_scenes_load_lay_out_and_save_back() {
    let (mut engine, assets) = engine_with(&[("hud.abscene", HUD)]);
    let logo =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/models/totem_albedo.png");
    std::fs::copy(logo, assets.0.join("logo.png")).unwrap();
    let tree = engine.tree_mut();
    let hud = load_scene(tree, "hud.abscene").unwrap();
    tree.add_child(tree.root(), hud).unwrap();
    let tree = engine.tree();
    ab_engine::ui::prepare_ui(tree.world(), 1920, 1080);
    let rect = |path: &str| {
        let node = tree.get_node(hud, path).unwrap();
        tree.component::<ab_engine::ui::Control>(node)
            .unwrap()
            .rect()
    };
    assert_eq!(rect("Health").position, Vec2::new(32.0, 32.0));
    assert_eq!(rect("Health").size, Vec2::new(360.0, 28.0));
    let menu = rect("Menu");
    assert_eq!(menu.center(), Vec2::new(960.0, 540.0));
    let title = rect("Menu/Items/Title");
    assert!(menu.contains(title.position) && title.size.x > 300.0);
    let resume = tree.get_node(hud, "Menu/Items/Resume").unwrap();
    assert_eq!(
        tree.component::<ab_engine::ui::Button>(resume)
            .unwrap()
            .text,
        "Resume"
    );
    assert!(rect("Menu/Items/Resume").size.y >= 56.0);

    let first = save_scene(tree, hud);
    let text = first.to_toml();
    assert_eq!(first.nodes.len(), 7, "saved:\n{text}");
    let reparsed = SceneFile::from_toml(&text).unwrap();
    let tree = engine.tree_mut();
    let again = instantiate_scene(tree, &reparsed, "saved").unwrap();
    assert_eq!(first, save_scene(tree, again), "saved:\n{text}");
    assert!(text.contains("type = \"TextureRect\""));
    assert!(text.contains("texture = \"logo.png\""));
    assert!(text.contains("text = \"Paused\""));
}

#[test]
fn keyed_ui_texts_translate_and_save_their_own_text() {
    let hud = "[[node]]\nname = \"Hud\"\ntype = \"Control\"\n\n\
               [[node]]\nname = \"Score\"\nparent = \".\"\ntype = \"Label\"\nlabel = { text = \"Score\", key = \"hud.score\" }\n\n\
               [[node]]\nname = \"Play\"\nparent = \".\"\ntype = \"Button\"\nbutton = { text = \"Play\", key = \"hud.play\" }\n";
    let en = "[language]\ncode = \"en\"\nname = \"English\"\n[hud]\nscore = \"Score\"\nplay = \"Play\"\n";
    let th =
        "[language]\ncode = \"th\"\nname = \"ไทย\"\n[hud]\nscore = \"คะแนน\"\nplay = \"เล่น\"\n";
    let (mut engine, _assets) = engine_with(&[
        ("hud.abscene", hud),
        ("locales/en.toml", en),
        ("locales/th.toml", th),
    ]);
    let settings = ab_engine::localization::LocalizationSettings {
        languages: vec!["en".into(), "th".into()],
        default_language: Some("th".into()),
        ..Default::default()
    };
    ab_engine::localization::install(engine.tree_mut().world_mut(), &settings).unwrap();
    let tree = engine.tree_mut();
    let hud = load_scene(tree, "hud.abscene").unwrap();
    let root = tree.root();
    tree.add_child(root, hud).unwrap();
    engine.update(1.0 / 60.0);
    let tree = engine.tree();
    let score = tree.get_node(hud, "Score").unwrap();
    let play = tree.get_node(hud, "Play").unwrap();
    assert_eq!(
        tree.component::<ab_engine::ui::Label>(score).unwrap().text,
        "คะแนน"
    );
    assert_eq!(
        tree.component::<ab_engine::ui::Button>(play).unwrap().text,
        "เล่น"
    );

    // Saved: the key and the scene's own text, not the Thai on screen.
    let saved = save_scene(tree, hud);
    let label = saved.nodes[1].label.as_ref().unwrap();
    assert_eq!(
        (label.text.as_str(), label.key.as_deref()),
        ("Score", Some("hud.score"))
    );
    let button = saved.nodes[2].button.as_ref().unwrap();
    assert_eq!(
        (button.text.as_str(), button.key.as_deref()),
        ("Play", Some("hud.play"))
    );
}

#[test]
fn material_files_are_shared_and_overridden_per_node() {
    let brick = "color = \"#aa4433\"\nroughness = 0.8\nalbedo_texture = \"albedo.png\"\nnormal_texture = \"albedo.png\"\nnormal_scale = 0.5\n";
    let scene = "[[node]]\nname = \"Wall\"\ntype = \"Node3D\"\n\n\
                 [[node]]\nname = \"A\"\nparent = \".\"\ntype = \"MeshInstance3D\"\nmesh = { primitive = \"cube\" }\nmaterial = { file = \"materials/brick.abmat\" }\n\n\
                 [[node]]\nname = \"B\"\nparent = \".\"\ntype = \"MeshInstance3D\"\nmesh = { primitive = \"cube\" }\nmaterial = { file = \"materials/brick.abmat\", roughness = 0.2 }\n";
    let png = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/models/totem_albedo.png"
    ))
    .unwrap();
    let (mut engine, assets) =
        engine_with(&[("materials/brick.abmat", brick), ("wall.abscene", scene)]);
    std::fs::write(assets.0.join("albedo.png"), png).unwrap();
    let tree = engine.tree_mut();
    let wall = load_scene(tree, "wall.abscene").unwrap();
    let material = |name: &str| {
        let node = tree.get_node(wall, name).unwrap();
        let handle = tree.component::<MeshInstance3D>(node).unwrap().material;
        tree.world()
            .resource::<RenderAssets>()
            .unwrap()
            .material(handle)
            .copied()
            .unwrap()
    };
    let (a, b) = (material("A"), material("B"));
    assert!(a.albedo_texture.is_some() && a.normal_texture.is_some());
    assert_eq!(a.normal_scale, 0.5);
    assert_eq!((a.roughness, b.roughness), (0.8, 0.2));
    assert_eq!(a.albedo, b.albedo, "both from the file");

    // Saved as written: the file and the node's own keys, not the file's contents.
    let saved = save_scene(tree, wall);
    let b = saved.nodes[2].material.as_ref().unwrap();
    assert_eq!(b.file.as_deref(), Some("materials/brick.abmat"));
    assert_eq!((b.roughness, b.color.is_none()), (Some(0.2), true));

    // A missing material file names itself.
    let broken = "[[node]]\nname = \"C\"\ntype = \"MeshInstance3D\"\nmesh = { primitive = \"cube\" }\nmaterial = { file = \"materials/none.abmat\" }\n";
    std::fs::write(assets.0.join("broken.abscene"), broken).unwrap();
    let err = load_scene(tree, "broken.abscene").unwrap_err().to_string();
    assert!(err.contains("materials/none.abmat"), "{err}");
}

#[test]
fn prefab_instances_keep_their_own_changes_when_the_prefab_changes() {
    let lamp = "[[node]]\nname = \"Lamp\"\ntype = \"Node3D\"\n\n\
                [[node]]\nname = \"Light\"\nparent = \".\"\ntype = \"OmniLight3D\"\nlight = { energy = 2.0, range = 5.0 }\n";
    let room = "[[node]]\nname = \"Room\"\ntype = \"Node3D\"\n\n\
                [[node]]\nname = \"A\"\nparent = \".\"\ninstance = \"lamp.abscene\"\n\n\
                [[node]]\nname = \"B\"\nparent = \".\"\ninstance = \"lamp.abscene\"\nposition = [3.0, 0.0, 0.0]\n\
                overrides = [{ path = \"Light\", light = { energy = 9.0 } }, { path = \"Gone\", visible = false }]\n\n\
                [[node]]\nname = \"C\"\nparent = \".\"\ninstance = \"missing.abscene\"\noverrides = [{ path = \"X\", visible = false }]\n";
    let (mut engine, assets) = engine_with(&[("lamp.abscene", lamp), ("room.abscene", room)]);
    let energy = |tree: &SceneTree, room: NodeHandle, path: &str| {
        let light = tree.get_node(room, path).unwrap();
        tree.component::<ab_engine::rendering::OmniLight3D>(light)
            .unwrap()
            .energy
    };
    let tree = engine.tree_mut();
    let room_node = load_scene(tree, "room.abscene").unwrap();
    assert_eq!(energy(tree, room_node, "A/Light"), 2.0);
    assert_eq!(energy(tree, room_node, "B/Light"), 9.0, "B's override");
    // The missing prefab: a placeholder, the rest of the room loads.
    let c = tree.get_node(room_node, "C").unwrap();
    assert!(tree
        .component::<ab_engine::scene_file::prefab::MissingInstance>(c)
        .is_some());

    // A's light is changed in the level: saved as A's override, the prefab untouched.
    let a_light = tree.get_node(room_node, "A/Light").unwrap();
    tree.component_mut::<ab_engine::rendering::OmniLight3D>(a_light)
        .unwrap()
        .range = 12.0;
    let saved = save_scene(tree, room_node);
    let a = &saved.nodes[1];
    assert_eq!(a.overrides.len(), 1, "{:?}", a.overrides);
    assert_eq!(a.overrides[0]["path"].as_str(), Some("Light"));
    assert_eq!(a.overrides[0]["light"]["range"].as_float(), Some(12.0));
    assert!(
        a.overrides[0]["light"].get("energy").is_none(),
        "only what changed"
    );
    // B keeps the override that matches nothing; C saves back as written.
    let b = &saved.nodes[2];
    assert!(b
        .overrides
        .iter()
        .any(|o| o["path"].as_str() == Some("Gone")));
    let c = &saved.nodes[3];
    assert_eq!(c.instance.as_deref(), Some("missing.abscene"));
    assert_eq!(c.overrides.len(), 1);

    // The prefab changes: every instance gets it, overrides still win.
    std::fs::write(assets.0.join("room.abscene"), saved.to_toml()).unwrap();
    std::fs::write(
        assets.0.join("lamp.abscene"),
        lamp.replace("energy = 2.0, range = 5.0", "energy = 4.0, range = 5.0"),
    )
    .unwrap();
    let again = load_scene(tree, "room.abscene").unwrap();
    assert_eq!(
        energy(tree, again, "A/Light"),
        4.0,
        "the prefab's new energy"
    );
    assert_eq!(energy(tree, again, "B/Light"), 9.0, "B's own energy");
    let a_light = tree.get_node(again, "A/Light").unwrap();
    assert_eq!(
        tree.component::<ab_engine::rendering::OmniLight3D>(a_light)
            .unwrap()
            .range,
        12.0
    );
}

#[test]
fn paths_build_roads_pipes_and_fences_and_save_only_their_points() {
    let post = "[[node]]\nname = \"Post\"\ntype = \"MeshInstance3D\"\nmesh = { primitive = \"cube\" }\n";
    let scene = "[[node]]\nname = \"Land\"\ntype = \"Node3D\"\n\n\
                 [[node]]\nname = \"Road\"\nparent = \".\"\ntype = \"Path3D\"\npath = { points = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [20.0, 0.0, 5.0]], mode = \"ribbon\", width = 4.0, collide = true }\nmaterial = { color = \"#333333\" }\n\n\
                 [[node]]\nname = \"Fence\"\nparent = \".\"\ntype = \"Path3D\"\npath = { points = [[0.0, 0.0, 5.0], [10.0, 0.0, 5.0]], mode = \"instances\", piece = \"post.abscene\", spacing = 2.5 }\n\n\
                 [[node]]\nname = \"Cable\"\nparent = \".\"\ntype = \"Path3D\"\npath = { points = [[0.0, 5.0, 0.0], [8.0, 5.0, 0.0]], mode = \"tube\", radius = 0.03, sag = 0.5 }\n";
    let (mut engine, _assets) = engine_with(&[("post.abscene", post), ("land.abscene", scene)]);
    let tree = engine.tree_mut();
    let land = load_scene(tree, "land.abscene").unwrap();
    let road = tree.get_node(land, "Road").unwrap();
    assert_eq!(tree.class_name(road), Some("Path3D"));
    assert!(tree.get_node(road, "Built/Mesh").is_some());
    assert!(tree.get_node(road, "Built/Shape").is_some(), "the road can be driven on");
    let fence = tree.get_node(land, "Fence").unwrap();
    assert_eq!(tree.children(tree.get_node(fence, "Built").unwrap()).len(), 5);
    assert!(tree.get_node(land, "Cable/Built/Mesh").is_some());

    let saved = save_scene(tree, land);
    assert_eq!(saved.nodes.len(), 4, "only the paths, not what they built");
    let road = &saved.nodes[1];
    assert_eq!(road.path.as_ref().unwrap().points.len(), 3);
    assert!(road.material.is_some());
    // Round trip: the same scene again.
    let again = SceneFile::from_toml(&saved.to_toml()).unwrap();
    assert_eq!(again, saved);

    let bad = "[[node]]\nname = \"P\"\ntype = \"Path3D\"\npath = { mode = \"spiral\" }\n";
    std::fs::write(_assets.0.join("bad.abscene"), bad).unwrap();
    let err = load_scene(tree, "bad.abscene").unwrap_err().to_string();
    assert!(err.contains("spiral"), "{err}");
}

#[test]
fn generators_build_on_load_repeat_by_seed_and_save_their_recipe() {
    let world = r##"
[[node]]
name = "World"
type = "Node3D"

[[node]]
name = "Hills"
parent = "."
type = "Generator"
generator = { kind = "terrain", seed = 4, size = 60.0, resolution = 24, height = 8.0 }

[[node]]
name = "Trees"
parent = "."
type = "Generator"
generator = { kind = "scatter", seed = 9, count = 20, width = 40.0, depth = 40.0, min_distance = 2.0 }

[[node]]
name = "Crypt"
parent = "."
type = "Generator"
position = [100.0, 0.0, 0.0]
generator = { kind = "dungeon", seed = 2, width = 16, depth = 16, rooms = 4 }

[[node]]
name = "Village"
parent = "."
type = "Generator"
position = [0.0, 0.0, 200.0]
generator = { kind = "town", seed = 1, blocks_x = 2, blocks_z = 1, houses = 1 }
"##;
    let (mut engine, assets) = engine_with(&[("world.abscene", world)]);
    let tree = engine.tree_mut();
    let root = load_scene(tree, "world.abscene").unwrap();
    let built = |tree: &SceneTree, name: &str| {
        let node = tree.get_node(root, format!("{name}/Built")).unwrap();
        tree.children(node)
    };
    assert_eq!(built(tree, "Hills").len(), 1, "one terrain");
    assert!(tree.get_node(root, "Hills/Built/Terrain/Shape").is_some(), "with collision");
    let trees = built(tree, "Trees");
    assert!(trees.len() >= 15, "{}", trees.len());
    // Standing on the hills, not at y = 0.
    let heights: Vec<f32> = trees.iter().map(|&t| tree.transform(t).unwrap().translation.y).collect();
    assert!(heights.iter().any(|&y| y > 0.5), "{heights:?}");
    let crypt = built(tree, "Crypt");
    assert!(crypt.len() > 20, "floors, walls and lights: {}", crypt.len());
    let village = built(tree, "Village");
    let roads = village.iter().filter(|&&n| tree.class_name(n) == Some("Path3D")).count();
    assert_eq!(roads, 3 + 2, "roads around 2 × 1 blocks");
    let house = village.iter().copied().find(|&n| tree.class_name(n) == Some("Generator")).unwrap();
    assert!(tree.get_node(house, "Built/Roof").is_some(), "the town's buildings are built too");

    // The same seed: the same result.
    let again = load_scene(tree, "world.abscene").unwrap();
    let positions = |r: NodeHandle| {
        let built = tree.get_node(r, "Trees/Built").unwrap();
        tree.children(built).iter().map(|&t| tree.transform(t).unwrap().translation).collect::<Vec<_>>()
    };
    assert_eq!(positions(root), positions(again));

    // Saved: the recipes only.
    let saved = save_scene(tree, root);
    assert_eq!(saved.nodes.len(), 5);
    assert_eq!(saved.nodes[2].generator.as_ref().unwrap().params["count"].as_integer(), Some(20));
    assert_eq!(SceneFile::from_toml(&saved.to_toml()).unwrap(), saved);

    let bad = "[[node]]\nname = \"G\"\ntype = \"Generator\"\ngenerator = { kind = \"galaxy\" }\n";
    std::fs::write(assets.0.join("bad.abscene"), bad).unwrap();
    let err = load_scene(tree, "bad.abscene").unwrap_err().to_string();
    assert!(err.contains("galaxy") && err.contains("terrain"), "{err}");
}

/// A script exporting properties, as script libraries describe theirs.
#[derive(Default)]
struct Mover {
    speed: f64,
    tint: Option<ab_script_api::Color>,
}

impl NodeScript for Mover {
    fn set_exported_property(&mut self, name: &str, value: ab_script_api::Value) -> bool {
        match name {
            "speed" => value.as_float().map(|v| self.speed = v).is_some(),
            "tint" => value.as_color().map(|c| self.tint = Some(c)).is_some(),
            _ => false,
        }
    }
}

#[test]
fn exported_script_properties_reach_the_script_and_save_back() {
    use ab_engine::scene::ScriptPropertyDesc;
    use ab_script_api as ffi;
    let scene = "[[node]]\nname = \"A\"\nscript = \"Mover\"\n\
                 script_properties = { Mover = { speed = 5, tint = \"#ff0000\" } }\n";
    let (mut engine, _assets) = engine_with(&[("props.abscene", scene)]);
    let prop = |name: &str, kind| ScriptPropertyDesc {
        name: name.into(),
        kind,
        default: None,
        flags: 0,
        doc: String::new(),
    };
    engine
        .tree()
        .world()
        .resource_mut::<ScriptRegistry>()
        .unwrap()
        .register::<Mover>("Mover")
        .register_metadata(
            "Mover",
            vec![prop("speed", ffi::Tag::FLOAT), prop("tint", ffi::Tag::COLOR)],
        );
    let tree = engine.tree_mut();
    let a = load_scene(tree, "props.abscene").unwrap();
    {
        let mover = tree.script::<Mover>(a).unwrap();
        // An integer in the file is the float property's value; the hex color a color.
        assert_eq!(mover.speed, 5.0);
        assert_eq!(mover.tint.map(|c| (c.r, c.g, c.b)), Some((1.0, 0.0, 0.0)));
    }

    // Saved back as written, and the same after another round trip.
    let first = save_scene(tree, a);
    let text = first.to_toml();
    assert!(text.contains("speed = 5"), "{text}");
    let again = instantiate_scene(tree, &SceneFile::from_toml(&text).unwrap(), "again").unwrap();
    assert_eq!(save_scene(tree, again).nodes, first.nodes, "{text}");

    // A hot reload builds a new instance: it gets the scene's values again.
    tree.reload_scripts();
    assert_eq!(tree.script::<Mover>(a).unwrap().speed, 5.0);

    // Changing them reaches the running instance.
    let mut changed = toml::Table::new();
    changed.insert("speed".into(), toml::Value::Float(2.5));
    assert!(tree.set_script_properties(a, "Mover", changed));
    assert_eq!(tree.script::<Mover>(a).unwrap().speed, 2.5);
    assert!(!tree.set_script_properties(a, "Spin", toml::Table::new()));

    // Values for a script the node does not have are an error, not silently dropped.
    let bad = "[[node]]\nname = \"B\"\nscript = \"Spin\"\n\
               script_properties = { Mover = { speed = 1.0 } }\n";
    let err = instantiate_scene(tree, &SceneFile::from_toml(bad).unwrap(), "bad").unwrap_err();
    assert!(err.to_string().contains("script_properties"), "{err}");
}
