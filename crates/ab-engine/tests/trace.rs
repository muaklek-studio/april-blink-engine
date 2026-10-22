//! Damage and object tracing: attribution, ignored hits, kills, spawns and frees, watched
//! objects and the F9 overlay.

use ab_engine::platform::PlatformEvent;
use ab_engine::prelude::*;
use ab_engine::trace::{DamageKind, ObjectEventKind, Outcome, DAMAGE, DAMAGED, DIED};

/// The script templates' health, in short: answers `damage` with `damaged` and `died`.
struct Health {
    current: f32,
    invulnerable: bool,
}

impl NodeScript for Health {
    fn ready(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        tree.connect(node, DAMAGE, node);
        tree.report_health(node, self.current, 100.0);
    }

    fn on_signal(&mut self, tree: &mut SceneTree, node: NodeHandle, signal: &Signal) {
        let amount = signal.arg::<f32>().copied().unwrap_or(0.0);
        if !signal.is(DAMAGE) || self.invulnerable || self.current <= 0.0 {
            return;
        }
        self.current = (self.current - amount).max(0.0);
        tree.report_health(node, self.current, 100.0);
        tree.emit(Signal::new(node, DAMAGED).with_arg(amount));
        if self.current <= 0.0 {
            tree.emit(Signal::new(node, DIED));
        }
    }
}

fn engine() -> Engine {
    let config = EngineConfig {
        audio_output: ab_engine::audio::AudioOutput::Silent,
        ..Default::default()
    };
    Engine::new(&config)
}

fn spawn(engine: &mut Engine, name: &str, health: Option<Health>) -> NodeHandle {
    let tree = engine.tree_mut();
    let node = tree.create_node3d(name);
    // Scripts first: `ready` runs when the node enters the tree.
    if let Some(health) = health {
        tree.add_script(node, health).unwrap();
    }
    let root = tree.root();
    tree.add_child(root, node).unwrap();
    node
}

fn step(engine: &mut Engine) {
    engine.update(1.0 / 60.0);
}

#[test]
fn damage_is_attributed_answered_and_totalled() {
    let mut engine = engine();
    let player = spawn(&mut engine, "Player", None);
    let enemy = spawn(
        &mut engine,
        "Enemy",
        Some(Health {
            current: 60.0,
            invulnerable: false,
        }),
    );
    step(&mut engine);
    engine.tree_mut().deal_damage(Some(player), enemy, 25.0);
    step(&mut engine);

    let tracer = engine.tree().world().resource::<Tracer>().unwrap();
    let hit = tracer.damage().last().unwrap().clone();
    assert_eq!(hit.kind, DamageKind::Damage);
    assert_eq!(hit.outcome, Outcome::Applied);
    assert_eq!((hit.source, hit.target), (Some(player), enemy));
    assert_eq!(hit.source_name, "Player");
    assert_eq!(hit.applied, 25.0);
    assert_eq!(hit.health, Some((35.0, 100.0)));
    assert!(hit.describe().contains("Player -> Enemy  -25  (hp 35/100)"));
    let totals = tracer.totals();
    assert_eq!(totals[0].name, "Player");
    assert_eq!((totals[0].dealt, totals[0].hits), (25.0, 1));
    assert_eq!(totals[1].taken, 25.0);
    drop(tracer);

    // A plain signal from a script without the attacker: still traced, source unknown.
    engine
        .tree_mut()
        .emit(Signal::new(enemy, DAMAGE).with_arg(5.0_f32));
    step(&mut engine);
    let tracer = engine.tree().world().resource::<Tracer>().unwrap();
    let hit = tracer.damage().last().unwrap();
    assert_eq!((hit.source, hit.source_name.as_str()), (None, "?"));
    assert_eq!(hit.outcome, Outcome::Applied);
    let csv = tracer.damage_csv();
    assert!(csv.starts_with("time,kind,source,target"));
    assert_eq!(csv.lines().count(), 3, "{csv}");
}

#[test]
fn unanswered_damage_is_ignored_and_kills_are_credited() {
    let mut engine = engine();
    let player = spawn(&mut engine, "Player", None);
    let wall = spawn(&mut engine, "Wall", None);
    let ghost = spawn(
        &mut engine,
        "Ghost",
        Some(Health {
            current: 100.0,
            invulnerable: true,
        }),
    );
    let grunt = spawn(
        &mut engine,
        "Grunt",
        Some(Health {
            current: 20.0,
            invulnerable: false,
        }),
    );
    step(&mut engine);
    let tree = engine.tree_mut();
    tree.deal_damage(Some(player), wall, 10.0);
    tree.deal_damage(Some(player), ghost, 10.0);
    tree.deal_damage(Some(player), grunt, 50.0);
    step(&mut engine);
    // The answers have had a frame to come.
    step(&mut engine);

    let tracer = engine.tree().world().resource::<Tracer>().unwrap();
    let outcome = |target: NodeHandle| {
        tracer
            .damage()
            .find(|e| e.target == target && e.kind == DamageKind::Damage)
            .unwrap()
            .outcome
    };
    assert_eq!(outcome(wall), Outcome::Ignored);
    assert_eq!(outcome(ghost), Outcome::Ignored);
    assert_eq!(outcome(grunt), Outcome::Applied);
    let death = tracer
        .damage()
        .find(|e| e.kind == DamageKind::Death)
        .unwrap();
    assert_eq!((death.source, death.target), (Some(player), grunt));
    assert!(death.describe().contains("Player killed Grunt  KO"));
    let totals = tracer.totals();
    let of = |name: &str| totals.iter().find(|t| t.name == name).unwrap().clone();
    assert_eq!(of("Player").kills, 1);
    assert_eq!(of("Grunt").deaths, 1);
    assert_eq!(of("Ghost").ignored, 1);
}

#[test]
fn spawns_frees_watches_and_notes_are_recorded() {
    let mut engine = engine();
    let _level = spawn(&mut engine, "Level", None);
    step(&mut engine);
    let runner = spawn(&mut engine, "Runner", None);
    step(&mut engine);
    {
        let tracer = engine.tree().world().resource::<Tracer>().unwrap();
        // What was there at the start is the level, not a spawn.
        assert!(!tracer.objects().any(|e| e.name == "Level"));
        let spawned = tracer.objects().last().unwrap();
        assert_eq!(spawned.kind, ObjectEventKind::Spawned);
        assert_eq!((spawned.node, spawned.class), (runner, "Node3D"));
    }

    let tree = engine.tree_mut();
    tree.trace_watch(runner, true);
    for i in 1..=10 {
        let tree = engine.tree_mut();
        tree.set_position(runner, Vec3::new(i as f32 * 0.1, 0.0, 0.0))
            .unwrap();
        step(&mut engine);
    }
    {
        let tracer = engine.tree().world().resource::<Tracer>().unwrap();
        let velocity = tracer.watches()[0].velocity;
        assert!((velocity.x - 6.0).abs() < 0.5, "{velocity:?}");
    }
    let tree = engine.tree_mut();
    tree.emit(Signal::new(runner, "jumped").with_arg(2.5_f32));
    tree.trace_note(runner, "state: fleeing");
    step(&mut engine);
    {
        let tracer = engine.tree().world().resource::<Tracer>().unwrap();
        let watch = &tracer.watches()[0];
        assert!(watch.trail.len() >= 9, "{:?}", watch.trail);
        assert_eq!(watch.velocity, Vec3::ZERO, "it stood still this frame");
        let lines: Vec<String> = tracer.objects().map(|e| e.describe()).collect();
        assert!(
            lines
                .iter()
                .any(|l| l.ends_with("Runner  signal jumped 2.5")),
            "{lines:?}"
        );
        assert!(lines
            .iter()
            .any(|l| l.ends_with("Runner  note: state: fleeing")));
        assert!(tracer.report().contains("WATCHED\n Runner"));
    }

    let tree = engine.tree_mut();
    tree.queue_free(runner).unwrap();
    step(&mut engine);
    step(&mut engine);
    let tracer = engine.tree().world().resource::<Tracer>().unwrap();
    assert!(tracer
        .objects()
        .any(|e| e.node == runner && e.kind == ObjectEventKind::Freed));
    assert!(!tracer.watches()[0].alive);
}

#[test]
fn f9_shows_the_trace_overlay_without_tracing_itself() {
    let mut engine = engine();
    let player = spawn(&mut engine, "Player", None);
    let enemy = spawn(
        &mut engine,
        "Enemy",
        Some(Health {
            current: 100.0,
            invulnerable: false,
        }),
    );
    step(&mut engine);
    let key = |code, pressed| PlatformEvent::Key {
        code,
        pressed,
        repeat: false,
    };
    engine.handle_event(&key(KeyCode::F9, true));
    engine.tree_mut().deal_damage(Some(player), enemy, 12.0);
    step(&mut engine);
    engine.handle_event(&key(KeyCode::F9, false));
    step(&mut engine);

    let tree = engine.tree();
    let overlay = tree
        .find_child(tree.root(), "TraceOverlay", false)
        .expect("F9 shows it");
    let label = tree.find_child(overlay, "Text", true).unwrap();
    let text = tree.component::<Label>(label).unwrap().text.clone();
    assert!(text.contains("DAMAGE\n"), "{text}");
    assert!(text.contains("Player -> Enemy  -12  (hp 88/100)"), "{text}");
    assert!(text.contains("TOTALS"), "{text}");
    // The overlay's own nodes are not reported as spawned.
    let tracer = tree.world().resource::<Tracer>().unwrap();
    assert!(
        !tracer.objects().any(|e| e.name.starts_with("Popup")),
        "{:?}",
        tracer.objects().collect::<Vec<_>>()
    );
}

#[test]
fn turned_off_it_records_nothing() {
    let mut engine = engine();
    engine
        .tree()
        .world()
        .resource_mut::<Tracer>()
        .unwrap()
        .enabled = false;
    let player = spawn(&mut engine, "Player", None);
    let enemy = spawn(
        &mut engine,
        "Enemy",
        Some(Health {
            current: 100.0,
            invulnerable: false,
        }),
    );
    step(&mut engine);
    engine.tree_mut().deal_damage(Some(player), enemy, 12.0);
    step(&mut engine);
    let tracer = engine.tree().world().resource::<Tracer>().unwrap();
    assert_eq!(tracer.damage().len(), 0);
    assert_eq!(tracer.objects().len(), 0);
}
