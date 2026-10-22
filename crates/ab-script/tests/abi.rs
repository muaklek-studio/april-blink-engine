//! Scripts written with the SDK, run by a real engine through the C ABI: events, transforms,
//! signals, typed access between scripts, UI, physics queries, hot-reload state, panics.

use ab_engine::script_host::ScriptLibrary;
use ab_engine::{Engine, EngineConfig};
use ab_script::prelude::*;
use ab_script::{script_type, PRESSED};

/// Moves along +X and reports when it passes x = 1.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Mover {
    speed: f32,
    announced: bool,
    ready_name: String,
}

impl Script for Mover {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        self.speed = 2.0;
        self.ready_name = tree.name(node).unwrap_or_default();
        tree.add_to_group(node, "movers").unwrap();
        tree.set_position(node, Vec3::new(0.0, 1.0, 0.0)).unwrap();
    }

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        tree.translate(node, Vec3::X * self.speed * delta).unwrap();
        let x = tree.position(node).unwrap().x;
        if x > 1.0 && !self.announced {
            self.announced = true;
            tree.emit_with(node, "moved", x).unwrap();
        }
    }
}

/// Listens to its sibling `Mover` and reads it directly.
#[derive(Debug, Default)]
struct Listener {
    heard: Vec<(String, f32)>,
    mover_speed: f32,
    label: Option<Node>,
    /// Set by the signal; acted on in `process` (during the signal the mover's own
    /// `process` is running, so its script is not reachable).
    stop: Option<Node>,
}

impl Script for Listener {
    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        let mover = tree.get_node(node, "../Mover").expect("sibling");
        tree.connect(mover, "moved", node).unwrap();
        // UI: a label in a canvas layer.
        let layer = tree.create_canvas_layer("Hud", 1).unwrap();
        let label = tree.create_label("Status", "waiting").unwrap();
        tree.add_child(tree.root(), layer).unwrap();
        tree.add_child(layer, label).unwrap();
        tree.control(label)
            .set_preset(LayoutPreset::TopLeft, Vec2::new(300.0, 40.0), 8.0)
            .unwrap();
        tree.label(label).set_font_size(30.0).unwrap();
        self.label = Some(label);
    }

    fn on_signal(&mut self, tree: &mut Tree<'_>, _node: Node, signal: &Signal<'_>) {
        self.heard.push((
            signal.name().to_owned(),
            signal.arg::<f32>().unwrap_or(-1.0),
        ));
        assert!(
            tree.script::<Mover>(signal.source()).is_none(),
            "a running script is detached"
        );
        self.stop = Some(signal.source());
        let label = self.label.unwrap();
        tree.label(label).set_text("moved!").unwrap();
    }

    fn process(&mut self, tree: &mut Tree<'_>, _node: Node, _delta: f32) {
        let Some(mover) = self.stop.take() else {
            return;
        };
        self.mover_speed = tree.script::<Mover>(mover).expect("reachable now").speed;
        tree.script_mut::<Mover>(mover).unwrap().speed = 0.0; // stop it
    }
}

/// Counts frames; the count survives a hot reload.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Counter {
    frames: u32,
    #[serde(skip)]
    reloaded: bool,
}

impl Script for Counter {
    serde_state!();

    fn process(&mut self, _tree: &mut Tree<'_>, _node: Node, _delta: f32) {
        self.frames += 1;
    }

    fn reloaded(&mut self, _tree: &mut Tree<'_>, _node: Node) {
        self.reloaded = true;
    }
}

/// Panics on its first frame.
#[derive(Debug, Default)]
struct Panicker {
    frames: u32,
}

impl Script for Panicker {
    fn process(&mut self, _tree: &mut Tree<'_>, _node: Node, _delta: f32) {
        self.frames += 1;
        panic!("boom on purpose");
    }
}

/// Checks errors, properties and physics from inside a callback; records what it saw.
#[derive(Debug, Default)]
struct Prober {
    report: Vec<String>,
}

impl Script for Prober {
    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        // A plain Node has no transform: a clear error, not a crash.
        let err = tree.set_position(node, Vec3::ONE).unwrap_err();
        self.report.push(format!("plain node: {err}"));
        // Unknown property: `None` (and a warning in the log).
        self.report.push(format!(
            "unknown property: {:?}",
            tree.get::<f32>(node, "nope.x")
        ));
        // Freed nodes are reported as such.
        let ghost = tree.create_node3d("Ghost").unwrap();
        tree.queue_free(ghost).unwrap();
        self.report
            .push(format!("valid before free: {}", tree.is_valid(ghost)));
        // A light made from scene text, then changed.
        let lamp = tree
            .instantiate_toml(
                "[[node]]\nname = \"Lamp\"\ntype = \"OmniLight3D\"\nlight = { energy = 2.0 }\n",
            )
            .unwrap();
        tree.add_child(tree.root(), lamp).unwrap();
        tree.light(lamp).set_energy(5.0).unwrap();
        self.report
            .push(format!("lamp energy: {:?}", tree.light(lamp).energy()));
        self.report.push(format!(
            "button text on a light: {}",
            tree.button(lamp).set_text("x").is_err()
        ));
        let _ = PRESSED;
    }

    fn physics_process(&mut self, tree: &mut Tree<'_>, _node: Node, _delta: f32) {
        if self.report.iter().any(|r| r.starts_with("ray")) {
            return;
        }
        let hit = tree.physics().cast_ray(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::NEG_Y,
            20.0,
            &QueryOptions::new(),
        );
        if let Some(hit) = hit {
            self.report
                .push(format!("ray hit at y {:.2}", hit.position.y));
            // Blood over the floor: particles, and splats where the droplets land.
            let root = tree.root();
            let blood = Blood {
                splats: 3,
                ..Blood::along(Vec3::new(1.0, 0.2, 0.0))
            };
            let spawned = tree.spawn_blood(root, Vec3::new(0.0, 1.0, 0.0), blood);
            self.report.push(format!("blood: {}", spawned.is_ok()));
        }
    }
}

/// Spawns the sample mannequin and runs it through its animations and locomotion.
#[derive(Debug, Default)]
struct Animator {
    model: Option<Node>,
    report: Vec<String>,
    frames: u32,
}

impl Script for Animator {
    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        let model = tree
            .instantiate_toml("[[node]]\nname = \"Hero\"\ninstance = \"models/mannequin.gltf\"\n")
            .unwrap();
        tree.add_child(node, model).unwrap();
        self.model = Some(model);
        let clips = tree.animation(model).clips();
        self.report.push(format!("clips: {}", clips.join(" ")));
        let missing = tree.animation(model).play("Dance").unwrap_err();
        self.report.push(format!("missing: {missing}"));
        tree.animation(model).crossfade("Walk", 0.2).unwrap();
        self.report
            .push(format!("current: {:?}", tree.animation(model).current()));
    }

    fn process(&mut self, tree: &mut Tree<'_>, _node: Node, _delta: f32) {
        let model = self.model.expect("ready");
        self.frames += 1;
        let mut loco = tree.locomotion(model);
        match self.frames {
            1..=10 => loco.update(4.0, true, 0.0, false).unwrap(),
            11 => loco.play_action("Attack").unwrap(),
            12 => self.report.push(format!(
                "action: {} {:?}",
                loco.is_action_playing(),
                loco.state()
            )),
            // The 0.6 s attack ends, then the fall shows.
            13..=60 => loco.update(0.0, false, -3.0, false).unwrap(),
            _ => {}
        }
        if self.frames == 60 {
            self.report.push(format!("air: {:?}", loco.state()));
        }
    }
}

fn register(registry: &mut Registry) {
    registry
        .register::<Animator>("Animator")
        .register::<Mover>("Mover")
        .register::<Listener>("Listener")
        .register::<Counter>("Counter")
        .register::<Panicker>("Panicker")
        .register::<Prober>("Prober");
}

ab_script::export!(register);

fn engine() -> Engine {
    let config = EngineConfig {
        asset_root: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(),
        audio_output: ab_engine::audio::AudioOutput::Silent,
        ..Default::default()
    };
    let mut engine = Engine::new(&config);
    let library = ScriptLibrary::from_entry(ab_script_library, "abi test").unwrap();
    assert_eq!(
        library.names().collect::<Vec<_>>(),
        ["Animator", "Mover", "Listener", "Counter", "Panicker", "Prober"]
    );
    let world = engine.tree_mut().world_mut();
    library.register(world.resource_or_insert_with(ab_engine::scene::ScriptRegistry::new));
    engine
}

fn add(
    engine: &mut Engine,
    name: &str,
    script: &str,
    node3d: bool,
) -> ab_engine::scene::NodeHandle {
    let tree = engine.tree_mut();
    let node = if node3d {
        tree.create_node3d(name)
    } else {
        tree.create_node(name)
    };
    tree.set_script_named(node, script).unwrap();
    tree.add_child(tree.root(), node).unwrap();
    node
}

/// The script of type `T` on `node`, read through the engine (as tests and tools do).
fn script<T: Script>(engine: &mut Engine, node: ab_engine::scene::NodeHandle) -> &T {
    let ty = script_type::<T>().expect("registered");
    let instance = engine
        .tree_mut()
        .script_instance(node, ty.cast())
        .expect("attached");
    // SAFETY: the engine returned an instance of `T`'s script type; the borrow of the engine
    // keeps it attached and unused meanwhile.
    unsafe { &*instance.cast::<T>() }
}

#[test]
fn scripts_run_signal_each_other_and_build_ui() {
    let mut engine = engine();
    let mover = add(&mut engine, "Mover", "Mover", true);
    let listener = add(&mut engine, "Listener", "Listener", false);
    for _ in 0..60 {
        engine.update(1.0 / 60.0);
    }
    let mover_state = script::<Mover>(&mut engine, mover);
    assert_eq!(mover_state.ready_name, "Mover");
    assert!(mover_state.announced);
    assert_eq!(mover_state.speed, 0.0, "the listener stopped it");
    let x = engine.tree().position(mover).unwrap().x;
    assert!((1.0..1.3).contains(&x), "stopped right after x = 1: {x}");
    assert!(engine.tree().is_in_group(mover, "movers"));

    let heard = &script::<Listener>(&mut engine, listener).heard;
    assert_eq!(heard.len(), 1);
    assert_eq!(heard[0].0, "moved");
    assert!(heard[0].1 > 1.0);
    assert_eq!(script::<Listener>(&mut engine, listener).mover_speed, 2.0);

    let label = script::<Listener>(&mut engine, listener).label.unwrap();
    let label = ab_engine::scene::NodeHandle::from_bits(label.to_bits());
    let text = engine
        .tree()
        .component::<ab_engine::ui::Label>(label)
        .unwrap()
        .text
        .clone();
    assert_eq!(text, "moved!");
}

#[test]
fn a_panicking_script_is_disabled_and_the_game_goes_on() {
    let mut engine = engine();
    let bad = add(&mut engine, "Bad", "Panicker", false);
    let counter = add(&mut engine, "Counter", "Counter", false);
    for _ in 0..10 {
        engine.update(1.0 / 60.0);
    }
    assert!(engine.tree().has_failed_script(bad));
    assert_eq!(
        script::<Panicker>(&mut engine, bad).frames,
        1,
        "not run again"
    );
    assert_eq!(script::<Counter>(&mut engine, counter).frames, 10);
}

#[test]
fn hot_reload_keeps_script_state() {
    let mut engine = engine();
    let counter = add(&mut engine, "Counter", "Counter", false);
    for _ in 0..5 {
        engine.update(1.0 / 60.0);
    }
    // A "new build": the library's scripts registered again, then swapped in.
    let library = ScriptLibrary::from_entry(ab_script_library, "abi test v2").unwrap();
    {
        let world = engine.tree_mut().world_mut();
        library.register(
            &mut world
                .resource_mut::<ab_engine::scene::ScriptRegistry>()
                .unwrap(),
        );
    }
    assert_eq!(engine.tree_mut().reload_scripts(), 1);
    let state = script::<Counter>(&mut engine, counter);
    assert_eq!(state.frames, 5, "state carried over");
    assert!(state.reloaded);
    engine.update(1.0 / 60.0);
    assert_eq!(script::<Counter>(&mut engine, counter).frames, 6);
}

#[test]
fn errors_properties_and_physics_queries() {
    let mut engine = engine();
    // A floor to hit.
    {
        use ab_engine::physics::{PhysicsBody3D, PhysicsNodes, Shape};
        let tree = engine.tree_mut();
        let floor = tree.create_physics_body3d("Floor", PhysicsBody3D::fixed());
        tree.add_collision_shape(
            floor,
            Shape::cuboid(ab_engine::core::Vec3::new(10.0, 1.0, 10.0)),
        )
        .unwrap();
        tree.set_position(floor, ab_engine::core::Vec3::new(0.0, -0.5, 0.0))
            .unwrap();
        tree.add_child(tree.root(), floor).unwrap();
    }
    let prober = add(&mut engine, "Prober", "Prober", false);
    for _ in 0..5 {
        engine.update(1.0 / 60.0);
    }
    let report = script::<Prober>(&mut engine, prober).report.join("\n");
    assert!(
        report.contains("plain node: node.set_position:"),
        "{report}"
    );
    assert!(report.contains("unknown property: None"), "{report}");
    assert!(report.contains("valid before free: true"), "{report}");
    assert!(report.contains("lamp energy: Some(5.0)"), "{report}");
    assert!(report.contains("button text on a light: true"), "{report}");
    assert!(report.contains("ray hit at y 0.00"), "{report}");
    assert!(report.contains("blood: true"), "{report}");
    let tree = engine.tree();
    let splats = tree
        .subtree(tree.root())
        .into_iter()
        .filter(|&n| tree.name(n).is_some_and(|n| n.starts_with("BloodSplat")))
        .count();
    assert_eq!(splats, 3, "the droplets land on the floor");
}

#[test]
fn scripts_animate_models_and_drive_their_locomotion() {
    let mut engine = engine();
    let animator = add(&mut engine, "Animator", "Animator", true);
    for _ in 0..65 {
        engine.update(1.0 / 60.0);
    }
    let report = script::<Animator>(&mut engine, animator).report.join("\n");
    assert!(
        report.contains("clips: Idle Walk Run Jump Attack Climb Fall"),
        "{report}"
    );
    assert!(report.contains("no animation `Dance`"), "{report}");
    assert!(report.contains("current: Some(\"Walk\")"), "{report}");
    assert!(report.contains("action: true Action"), "{report}");
    assert!(report.contains("air: Fall"), "{report}");
}
