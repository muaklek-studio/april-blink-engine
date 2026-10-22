//! Night Relics end to end, headless: losing to the ghost, carrying relics to the altar,
//! winning at dawn, and restarting many times without leaking anything. The scripts run
//! through the script ABI, as in the shipped game.

use ab_engine::platform::PlatformEvent;
use ab_engine::prelude::*;
use ab_script::{script_type, Script};
use ab_slice::{Game, Ghost, Phase, Player, PLACED};

const DT: f32 = 1.0 / 60.0;

fn engine() -> (Engine, NodeHandle) {
    let game = ab_engine::Game::with_scripts("game.toml", ab_slice::ab_script_library)
        .project(env!("CARGO_MANIFEST_DIR"));
    let settings = game.settings().unwrap();
    let mut config = settings.engine_config(game.asset_root(&settings));
    config.audio_output = ab_engine::audio::AudioOutput::Silent;
    let mut engine = Engine::new(&config);
    let scene = game.setup(&mut engine, &settings, None).unwrap();
    (engine, scene)
}

fn frames(engine: &mut Engine, count: usize) {
    for _ in 0..count {
        engine.update(DT);
    }
}

/// Presses and releases a key over two frames.
fn tap(engine: &mut Engine, code: KeyCode) {
    engine.handle_event(&PlatformEvent::Key {
        code,
        pressed: true,
        repeat: false,
    });
    engine.update(DT);
    engine.handle_event(&PlatformEvent::Key {
        code,
        pressed: false,
        repeat: false,
    });
    engine.update(DT);
}

/// The `T` script on `node`, read through the engine.
fn script<T: Script>(engine: &mut Engine, node: NodeHandle) -> &mut T {
    let ty = script_type::<T>().expect("registered");
    let instance = engine
        .tree_mut()
        .script_instance(node, ty.cast())
        .expect("attached");
    // SAFETY: the engine returned an instance of `T`'s script type; the borrow of the engine
    // keeps it attached and not running meanwhile.
    unsafe { &mut *instance.cast::<T>() }
}

fn game(engine: &mut Engine, node: NodeHandle) -> &mut Game {
    script::<Game>(engine, node)
}

fn handle(node: ab_script::Node) -> NodeHandle {
    NodeHandle::from_bits(node.to_bits())
}

/// Puts the player at `at` looking at `target`.
fn stand(engine: &mut Engine, player: NodeHandle, at: Vec3, target: Vec3) {
    let tree = engine.tree_mut();
    tree.set_global_position(player, at).unwrap();
    tree.update_transforms();
    let camera = tree.get_node(player, "Shake/Camera").unwrap();
    let eye = tree.global_position(camera).unwrap();
    script::<Player>(engine, player).look_at(eye, target);
    frames(engine, 3);
}

#[test]
fn the_ghost_catches_a_player_who_stands_still() {
    let (mut engine, game_node) = engine();
    frames(&mut engine, 2);
    assert_eq!(game(&mut engine, game_node).phase(), Phase::Playing);
    let ghost = handle(game(&mut engine, game_node).ghost().unwrap());
    assert!(
        engine.tree().world().resource::<NavGrid>().is_some(),
        "navigation baked"
    );

    let mut caught_at = None;
    for frame in 0..60 * 40 {
        engine.update(DT);
        if game(&mut engine, game_node).phase() == Phase::Lost {
            caught_at = Some(frame);
            break;
        }
    }
    let caught_at = caught_at.expect("caught within 40 s");
    // 8 s of grace, then about 11 m at 2.3 m/s.
    assert!(
        caught_at > 60 * 8,
        "not during the grace period ({caught_at})"
    );
    assert!(script::<Ghost>(&mut engine, ghost).is_hunting());
    // The round is frozen.
    let player = handle(game(&mut engine, game_node).player().unwrap());
    assert!(!engine.tree().is_physics_processing(player));
}

#[test]
fn relics_go_from_the_table_to_the_altar() {
    let (mut engine, game_node) = engine();
    frames(&mut engine, 2);
    let (player, ghost) = {
        let g = game(&mut engine, game_node);
        (handle(g.player().unwrap()), handle(g.ghost().unwrap()))
    };
    // Keep the ghost out of this test.
    engine.tree_mut().set_physics_process(ghost, false).unwrap();
    let dawn_before = game(&mut engine, game_node).dawn();

    // Relic0 sits on the table at (-9, 0.8, 5).
    let level = handle(game(&mut engine, game_node).level().unwrap());
    let relic = engine.tree().get_node(level, "Relics/Relic0").unwrap();
    frames(&mut engine, 60); // let it settle on the table
    let relic_at = engine.tree().global_position(relic).unwrap();
    stand(&mut engine, player, Vec3::new(-9.0, 0.95, 6.6), relic_at);
    let prompt = script::<Player>(&mut engine, player).prompt().to_owned();
    assert!(prompt.contains("Pick up"), "{prompt:?}");
    tap(&mut engine, KeyCode::KeyE);
    assert!(script::<Player>(&mut engine, player).is_carrying());
    let camera = engine.tree().get_node(player, "Shake/Camera").unwrap();
    assert_eq!(
        engine.tree().parent(relic),
        Some(camera),
        "held in front of the camera"
    );

    // Walk to the altar at (0, 0.5, -5) and place it.
    stand(
        &mut engine,
        player,
        Vec3::new(0.0, 0.95, -3.2),
        Vec3::new(0.0, 0.9, -5.0),
    );
    let prompt = script::<Player>(&mut engine, player).prompt().to_owned();
    assert!(prompt.contains("altar"), "{prompt:?}");
    tap(&mut engine, KeyCode::KeyE);
    frames(&mut engine, 2);
    assert!(!script::<Player>(&mut engine, player).is_carrying());
    let tree = engine.tree();
    assert!(tree.is_in_group(relic, PLACED));
    let slot = tree.parent(relic).unwrap();
    assert!(tree.is_in_group(slot, ab_slice::ALTAR_SLOT));
    let g = game(&mut engine, game_node);
    assert_eq!(g.placed(), 1);
    assert!(g.dawn() < dawn_before - 29.0, "dawn came 30 s closer");
    assert!(
        script::<Ghost>(&mut engine, ghost).speed > 2.5,
        "the ghost sped up"
    );

    // Dropping: pick up another and drop it on the floor.
    let other = engine.tree().get_node(level, "Relics/Relic1").unwrap();
    let other_at = engine.tree().global_position(other).unwrap();
    stand(&mut engine, player, Vec3::new(-10.0, 0.95, -4.0), other_at);
    tap(&mut engine, KeyCode::KeyE);
    assert!(script::<Player>(&mut engine, player).is_carrying());
    stand(
        &mut engine,
        player,
        Vec3::new(-8.0, 0.95, -2.0),
        Vec3::new(-8.0, 0.0, -3.0),
    );
    tap(&mut engine, KeyCode::KeyE);
    frames(&mut engine, 120);
    let y = engine.tree().global_position(other).unwrap().y;
    assert!(!script::<Player>(&mut engine, player).is_carrying());
    assert!((0.1..0.4).contains(&y), "fell to the floor: {y}");
}

#[test]
fn surviving_until_dawn_wins_and_restart_does_not_leak() {
    let (mut engine, game_node) = engine();
    frames(&mut engine, 3);

    let counts = |engine: &Engine| {
        let tree = engine.tree();
        let world = tree.world();
        let physics = world.resource::<PhysicsWorld>().unwrap();
        let assets = world.resource::<RenderAssets>().unwrap();
        (
            tree.node_count(),
            physics.body_count(),
            physics.collider_count(),
            assets.mesh_count(),
            assets.material_count(),
            tree.connection_count(),
        )
    };
    let first = counts(&engine);

    for round in 0..12 {
        game(&mut engine, game_node).set_dawn(0.05);
        frames(&mut engine, 3);
        assert_eq!(
            game(&mut engine, game_node).phase(),
            Phase::Won,
            "round {round}"
        );
        tap(&mut engine, KeyCode::KeyR);
        frames(&mut engine, 3);
        assert_eq!(
            game(&mut engine, game_node).phase(),
            Phase::Playing,
            "round {round}"
        );
    }
    assert_eq!(
        counts(&engine),
        first,
        "same nodes, bodies, meshes and connections"
    );
}
