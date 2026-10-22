//! The script templates, loaded through the C ABI into a real engine and played with
//! simulated input: each does what its documentation says.

use ab_engine::input::{Actions, Binding, InputMap, KeyCode};
use ab_engine::prelude::*;
use ab_engine::scene::NodeHandle as Handle;
use ab_engine::scene_file::{instantiate_scene, SceneFile};
use ab_engine::script_host::ScriptLibrary;
use ab_script::script_type;
use ab_script_templates::{enemy, health};

ab_script::export!(ab_script_templates::register);

const DT: f32 = 1.0 / 60.0;

/// Every action the templates use, each bound to some key (the tests hold them from code).
const ACTIONS: &[&str] = &[
    "move_forward",
    "move_back",
    "move_left",
    "move_right",
    "jump",
    "sprint",
    "crouch",
    "dash",
    "attack",
    "fire",
    "aim",
    "reload",
    "lock_on",
];

fn engine() -> Engine {
    let config = EngineConfig {
        asset_root: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(),
        audio_output: ab_engine::audio::AudioOutput::Silent,
        ..Default::default()
    };
    let mut engine = Engine::new(&config);
    let library = ScriptLibrary::from_entry(ab_script_library, "templates").unwrap();
    let world = engine.tree_mut().world_mut();
    library.register(world.resource_or_insert_with(ab_engine::scene::ScriptRegistry::new));
    let mut map = InputMap::new();
    for action in ACTIONS {
        map.bind(action, Binding::Key(KeyCode::F24));
    }
    world.resource_mut::<Actions>().unwrap().set_map(map);
    engine
}

/// Builds a scene from TOML under the root (a floor is always there).
fn scene(engine: &mut Engine, toml: &str) -> Handle {
    let floor = "[[node]]\nname = \"World\"\ntype = \"Node3D\"\n\n\
        [[node]]\nname = \"Floor\"\nparent = \".\"\ntype = \"StaticBody3D\"\nposition = [0.0, -0.5, 0.0]\n\n\
        [[node]]\nname = \"Shape\"\nparent = \"Floor\"\ntype = \"CollisionShape3D\"\nshape = { box = [80.0, 1.0, 80.0] }\n";
    let text = format!("{floor}\n{toml}");
    let file = SceneFile::from_toml(&text).unwrap();
    let tree = engine.tree_mut();
    let root = instantiate_scene(tree, &file, "test").unwrap();
    tree.add_child(tree.root(), root).unwrap();
    root
}

fn hold(engine: &Engine, action: &str, held: bool) {
    let mut actions = engine.tree().world().resource_mut::<Actions>().unwrap();
    if held {
        actions.hold(action, 1.0);
    } else {
        actions.release(action);
    }
}

fn frames(engine: &mut Engine, count: usize) {
    for _ in 0..count {
        engine.update(DT);
    }
}

fn node(engine: &Engine, root: Handle, path: &str) -> Handle {
    engine.tree().get_node(root, path).unwrap()
}

/// The script of type `T` on `node`, read through the engine.
fn script<T: ab_script::Script>(engine: &mut Engine, node: Handle) -> &T {
    let ty = script_type::<T>().expect("registered");
    let instance = engine
        .tree_mut()
        .script_instance(node, ty.cast())
        .expect("attached");
    // SAFETY: the engine returned an instance of `T`'s script type; the borrow of the engine
    // keeps it attached and unused meanwhile.
    unsafe { &*instance.cast::<T>() }
}

fn script_mut<T: ab_script::Script>(engine: &mut Engine, node: Handle) -> &mut T {
    let ty = script_type::<T>().expect("registered");
    let instance = engine
        .tree_mut()
        .script_instance(node, ty.cast())
        .expect("attached");
    // SAFETY: as in `script`, with the engine borrowed mutably.
    unsafe { &mut *instance.cast::<T>() }
}

/// Keeps an enemy where it is (it still sees, attacks and dies).
fn stand_still(engine: &mut Engine, enemy: Handle) {
    let ai = script_mut::<enemy::EnemyAi>(engine, enemy);
    ai.walk_speed = 0.0;
    ai.run_speed = 0.0;
}

/// A character body (capsule 1.8 m, centred) with the given scripts, at `position`.
fn character(name: &str, scripts: &[&str], position: [f32; 3], extra: &str) -> String {
    let list = scripts
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let [x, y, z] = position;
    format!(
        "\n[[node]]\nname = \"{name}\"\nparent = \".\"\ntype = \"CharacterBody3D\"\nposition = [{x}, {y}, {z}]\nscripts = [{list}]\n\n\
         [[node]]\nname = \"Shape\"\nparent = \"{name}\"\ntype = \"CollisionShape3D\"\nshape = {{ capsule = [0.35, 1.8] }}\n{extra}"
    )
}

fn model(parent: &str) -> String {
    format!(
        "\n[[node]]\nname = \"Model\"\nparent = \"{parent}\"\ninstance = \"models/mannequin.gltf\"\nposition = [0.0, -0.9, 0.0]\n"
    )
}

#[test]
fn first_person_walks_where_it_looks_jumps_and_has_a_camera() {
    let mut engine = engine();
    let root = scene(
        &mut engine,
        &character("Player", &["FirstPersonController"], [0.0, 1.0, 0.0], ""),
    );
    frames(&mut engine, 30);
    let player = node(&engine, root, "Player");
    let camera = node(&engine, root, "Player/Head/Bob/Camera");
    assert!(engine.tree().component::<Camera3D>(camera).unwrap().current);
    assert!(engine.tree().is_in_group(player, "player"));
    let start = engine.tree().global_position(player).unwrap();
    hold(&engine, "move_forward", true);
    frames(&mut engine, 60);
    let walked = engine.tree().global_position(player).unwrap();
    assert!(
        walked.z < start.z - 2.5,
        "forward is -Z: {start} → {walked}"
    );
    hold(&engine, "move_forward", false);
    hold(&engine, "jump", true);
    frames(&mut engine, 2);
    hold(&engine, "jump", false);
    frames(&mut engine, 10);
    let up = engine.tree().global_position(player).unwrap();
    assert!(up.y > walked.y + 0.4, "jumped: {up}");
}

#[test]
fn third_person_turns_its_model_and_animates_it() {
    let mut engine = engine();
    let toml = character(
        "Hero",
        &["ThirdPersonController"],
        [0.0, 1.0, 0.0],
        &model("Hero"),
    );
    let root = scene(&mut engine, &toml);
    frames(&mut engine, 20);
    let hero = node(&engine, root, "Hero");
    assert!(engine.tree().get_node(root, "CameraPivot/Camera").is_some());
    hold(&engine, "move_right", true);
    hold(&engine, "sprint", true);
    frames(&mut engine, 90);
    let at = engine.tree().global_position(hero).unwrap();
    assert!(at.x > 3.0, "ran to the camera's right: {at}");
    // Faces where it runs (-Z of the body along +X).
    let facing = engine
        .tree()
        .global_transform(hero)
        .unwrap()
        .transform_vector3(Vec3::NEG_Z);
    assert!(facing.x > 0.8, "{facing}");
    let player = engine
        .tree()
        .subtree(hero)
        .into_iter()
        .find(|&n| engine.tree().component::<AnimationPlayer>(n).is_some())
        .unwrap();
    let anim = engine.tree().component::<AnimationPlayer>(player).unwrap();
    assert_eq!(anim.current_name(), Some("Run"));
}

#[test]
fn movement_jump_and_dash_work_together() {
    let mut engine = engine();
    let root = scene(
        &mut engine,
        &character("P", &["Movement", "Jump", "Dash"], [0.0, 1.0, 0.0], ""),
    );
    frames(&mut engine, 20);
    let p = node(&engine, root, "P");
    let start = engine.tree().global_position(p).unwrap();
    hold(&engine, "dash", true);
    frames(&mut engine, 1);
    hold(&engine, "dash", false);
    frames(&mut engine, 15);
    let dashed = engine.tree().global_position(p).unwrap();
    assert!(
        start.distance(dashed) > 1.5,
        "dashed ahead: {start} → {dashed}"
    );
    assert!(!engine.tree().is_in_group(p, "movement_busy"), "over");
    hold(&engine, "jump", true);
    frames(&mut engine, 12);
    let air = engine.tree().global_position(p).unwrap();
    assert!(air.y > dashed.y + 0.5, "{air}");
}

#[test]
fn a_gun_and_crosshair_shoot_an_enemy_dead() {
    let mut engine = engine();
    // The enemy stands 8 m in front of the player, facing away (it does not notice).
    let mut toml = character(
        "Player",
        &["FirstPersonController", "Gun", "Crosshair"],
        [0.0, 1.0, 0.0],
        "",
    );
    toml += &character(
        "Enemy",
        &["Health", "EnemyAi"],
        [0.0, 1.0, -8.0],
        &model("Enemy"),
    );
    let root = scene(&mut engine, &toml);
    let enemy = node(&engine, root, "Enemy");
    frames(&mut engine, 1);
    stand_still(&mut engine, enemy);
    frames(&mut engine, 30);
    assert!(engine.tree().is_in_group(enemy, "damageable"));
    assert!(engine.tree().is_in_group(enemy, "enemies"));
    hold(&engine, "fire", true);
    frames(&mut engine, 90);

    hold(&engine, "fire", false);
    frames(&mut engine, 5);
    let health = script::<health::Health>(&mut engine, enemy);
    assert!(health.is_dead(), "hp {}", health.current);
    let ai = script::<enemy::EnemyAi>(&mut engine, enemy);
    assert_eq!(ai.state, enemy::EnemyState::Dead);
    assert!(!engine.tree().is_in_group(enemy, "enemies"));
    // It went limp: a ragdoll was made from its model.
    let doll = engine
        .tree()
        .subtree(enemy)
        .into_iter()
        .find(|&n| engine.tree().component::<Ragdoll>(n).is_some());
    assert!(doll.is_some_and(|d| engine.tree().is_ragdoll_active(d)));

    // The trace has the shots, the health after each and the kill, credited to the player.
    let tracer = engine
        .tree()
        .world()
        .resource::<ab_engine::Tracer>()
        .unwrap();
    use ab_engine::trace::{DamageKind, Outcome};
    let hits: Vec<_> = tracer
        .damage()
        .filter(|e| e.kind == DamageKind::Damage && e.outcome == Outcome::Applied)
        .collect();
    assert!(!hits.is_empty());
    assert!(hits
        .iter()
        .all(|e| e.source_name == "Player" && e.target_name == "Enemy"));
    assert!(hits
        .iter()
        .all(|e| e.health.is_some_and(|(_, max)| max == 100.0)));
    let kill = tracer
        .damage()
        .find(|e| e.kind == DamageKind::Death)
        .expect("the death is traced");
    assert_eq!(
        (kill.source_name.as_str(), kill.health),
        ("Player", Some((0.0, 100.0)))
    );
    let totals = tracer.totals();
    assert_eq!((totals[0].name.as_str(), totals[0].kills), ("Player", 1));
    assert!(totals[0].dealt >= 100.0, "{:?}", totals[0]);
}

#[test]
fn an_enemy_chases_and_hurts_the_player() {
    let mut engine = engine();
    let mut toml = character("Player", &["Health", "Movement"], [0.0, 1.0, 0.0], "");
    // Turned to face the player (its forward, -Z, rotated to +Z): it sees it at once.
    toml += &character("Enemy", &["Health", "EnemyAi"], [0.0, 1.0, -10.0], "").replacen(
        "position = [0, 1, -10]",
        "position = [0, 1, -10]\nrotation = [0.0, 180.0, 0.0]",
        1,
    );
    let root = scene(&mut engine, &toml);
    frames(&mut engine, 300);
    let player = node(&engine, root, "Player");
    let enemy = node(&engine, root, "Enemy");
    let distance = engine
        .tree()
        .global_position(player)
        .unwrap()
        .distance(engine.tree().global_position(enemy).unwrap());
    assert!(distance < 2.5, "caught up: {distance}");
    let hp = script::<health::Health>(&mut engine, player).current;
    assert!(hp < 100.0, "hurt: {hp}");
}

#[test]
fn melee_combos_damage_what_is_in_front() {
    let mut engine = engine();
    let mut toml = character("Fighter", &["MeleeCombat"], [0.0, 1.0, 0.0], "");
    toml += &character("Dummy", &["Health"], [0.0, 1.0, -1.2], "");
    let root = scene(&mut engine, &toml);
    frames(&mut engine, 10);
    let dummy = node(&engine, root, "Dummy");
    for _ in 0..3 {
        hold(&engine, "attack", true);
        frames(&mut engine, 1);
        hold(&engine, "attack", false);
        frames(&mut engine, 30);
    }
    frames(&mut engine, 40);
    let hp = script::<health::Health>(&mut engine, dummy).current;
    // 15 + 20 + 35 for the full combo.
    assert!(hp <= 100.0 - 50.0, "the combo landed: {hp}");
    let tracer = engine
        .tree()
        .world()
        .resource::<ab_engine::Tracer>()
        .unwrap();
    let fighter = tracer
        .totals()
        .into_iter()
        .find(|t| t.name == "Fighter")
        .unwrap();
    assert_eq!(fighter.dealt, 100.0 - hp, "{fighter:?}");
}

#[test]
fn auto_aim_turns_the_view_onto_an_enemy() {
    let mut engine = engine();
    let mut toml = character(
        "Player",
        &["FirstPersonController", "AutoAim"],
        [0.0, 1.0, 0.0],
        "",
    );
    // Slightly to the left of straight ahead; a dummy that does not move.
    toml += &character("Enemy", &["Health"], [-1.2, 1.0, -10.0], "");
    toml = toml.replace(
        "scripts = [\"Health\"]",
        "scripts = [\"Health\"]\ngroups = [\"enemies\"]",
    );
    let root = scene(&mut engine, &toml);
    frames(&mut engine, 10);
    let player = node(&engine, root, "Player");
    hold(&engine, "aim", true);
    frames(&mut engine, 60);
    let forward = engine
        .tree()
        .global_transform(player)
        .unwrap()
        .transform_vector3(Vec3::NEG_Z);
    let to_enemy = (Vec3::new(-1.2, 0.0, -10.0)).normalize();
    assert!(forward.dot(to_enemy) > 0.999, "{forward} vs {to_enemy}");
}

#[test]
fn the_car_drives_and_stays_on_its_wheels() {
    let mut engine = engine();
    let toml = "\n[[node]]\nname = \"Car\"\nparent = \".\"\ntype = \"RigidBody3D\"\nposition = [0.0, 1.0, 0.0]\nbody = { mass = 1200.0 }\nscript = \"Car\"\n\n\
        [[node]]\nname = \"Shape\"\nparent = \"Car\"\ntype = \"CollisionShape3D\"\nshape = { box = [1.8, 0.6, 4.0] }\n";
    let root = scene(&mut engine, toml);
    frames(&mut engine, 60);
    let car = node(&engine, root, "Car");
    let settled = engine.tree().global_position(car).unwrap();
    assert!(
        settled.y > 0.4 && settled.y < 1.2,
        "on its suspension: {settled}"
    );
    hold(&engine, "move_forward", true);
    frames(&mut engine, 120);
    let moved = engine.tree().global_position(car).unwrap();
    assert!(
        moved.z < settled.z - 8.0,
        "drove forward: {settled} → {moved}"
    );
    let up = engine
        .tree()
        .global_transform(car)
        .unwrap()
        .transform_vector3(Vec3::Y);
    assert!(up.y > 0.9, "upright: {up}");
    hold(&engine, "move_left", true);
    frames(&mut engine, 60);
    let turned = engine.tree().global_position(car).unwrap();
    assert!(turned.x < moved.x - 1.0, "steered left: {moved} → {turned}");
}

#[test]
fn an_animated_character_follows_its_body() {
    let mut engine = engine();
    let toml = character(
        "Npc",
        &["AnimatedCharacter"],
        [0.0, 1.0, 0.0],
        &model("Npc"),
    );
    let root = scene(&mut engine, &toml);
    let npc = node(&engine, root, "Npc");
    // Another script moves the body; here the test does, at a set velocity.
    let drive = |engine: &mut Engine, velocity: Vec3, count: usize| {
        for _ in 0..count {
            engine
                .tree_mut()
                .component_mut::<ab_engine::physics::CharacterBody3D>(npc)
                .unwrap()
                .velocity = velocity;
            let _ = engine.tree_mut().move_and_slide(npc, DT);
            engine.update(DT);
        }
    };
    drive(&mut engine, Vec3::new(0.0, -1.0, 0.0), 30);
    let player = engine
        .tree()
        .subtree(npc)
        .into_iter()
        .find(|&n| engine.tree().component::<AnimationPlayer>(n).is_some())
        .unwrap();
    let clip = |engine: &Engine| {
        engine
            .tree()
            .component::<AnimationPlayer>(player)
            .unwrap()
            .current_name()
            .map(str::to_owned)
    };
    assert_eq!(clip(&engine).as_deref(), Some("Idle"));
    drive(&mut engine, Vec3::new(0.0, -1.0, -4.2), 20);
    assert_eq!(clip(&engine).as_deref(), Some("Run"));
}
