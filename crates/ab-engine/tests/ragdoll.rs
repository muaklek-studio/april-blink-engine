//! Ragdolls and debug tools through the engine: the humanoid prefab, a skinned model going
//! limp, physics debug lines, script profiling and time scale.

use ab_engine::platform::PlatformEvent;
use ab_engine::prelude::*;
use ab_engine::ragdoll::{self, HumanoidRagdoll};
use ab_engine::scene_file::{instantiate_scene, save_scene};

const DT: f32 = 1.0 / 60.0;

fn engine() -> Engine {
    let config = EngineConfig {
        asset_root: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(),
        audio_output: ab_engine::audio::AudioOutput::Silent,
        ..Default::default()
    };
    let mut engine = Engine::new(&config);
    let tree = engine.tree_mut();
    let floor = tree.create_physics_body3d("Floor", PhysicsBody3D::fixed());
    tree.add_collision_shape(
        floor,
        Shape::Box {
            size: Vec3::new(40.0, 1.0, 40.0),
        },
    )
    .unwrap();
    tree.set_position(floor, Vec3::new(0.0, -0.5, 0.0)).unwrap();
    tree.add_child(tree.root(), floor).unwrap();
    engine
}

fn frames(engine: &mut Engine, count: usize) {
    for _ in 0..count {
        engine.update(DT);
    }
}

fn part(engine: &Engine, doll: NodeHandle, name: &str) -> NodeHandle {
    engine.tree().get_node(doll, name).unwrap()
}

#[test]
fn the_humanoid_collapses_in_one_piece_and_round_trips() {
    let mut engine = engine();
    let tree = engine.tree_mut();
    let doll = ragdoll::spawn_humanoid_ragdoll(tree, &HumanoidRagdoll::default()).unwrap();
    tree.add_child(tree.root(), doll).unwrap();
    assert_eq!(tree.ragdoll_parts(doll).len(), 11);
    // A shove at chest height.
    frames(&mut engine, 1);
    engine.tree_mut().apply_ragdoll_impulse(
        doll,
        Vec3::new(0.0, 0.0, -60.0),
        Vec3::new(0.0, 1.4, 0.0),
    );
    frames(&mut engine, 240);

    let physics = engine.tree().world().resource::<PhysicsWorld>().unwrap();
    assert_eq!(physics.joint_count(), 10);
    assert_eq!(physics.body_count(), 12, "11 parts + the floor");
    drop(physics);
    let at = |name: &str| {
        engine
            .tree()
            .global_position(part(&engine, doll, name))
            .unwrap()
    };
    let chest = at("Chest");
    assert!(chest.y < 0.6, "fell down: {chest}");
    for name in ["Pelvis", "Chest", "Head", "ShinL", "LowerArmR"] {
        assert!(at(name).y > 0.0, "{name} above the floor: {}", at(name));
    }
    assert!(at("Head").distance(chest) < 0.55, "the head stays on");
    assert!(at("ShinL").distance(at("ThighL")) < 0.55, "legs stay on");

    // Saved and loaded again it is the same ragdoll.
    let tree = engine.tree_mut();
    let saved = save_scene(tree, doll);
    let joints: Vec<_> = saved
        .nodes
        .iter()
        .filter_map(|n| n.joint.as_ref())
        .collect();
    assert_eq!(joints.len(), 10);
    assert!(joints.iter().all(|j| j.body_a.starts_with("../")));
    let copy = instantiate_scene(tree, &saved, "copy").unwrap();
    assert_eq!(tree.ragdoll_parts(copy).len(), 11);
    let neck = tree.get_node(copy, "Neck").unwrap();
    assert_eq!(
        tree.component::<Joint3D>(neck).unwrap().body_b,
        tree.get_node(copy, "Head")
    );
}

/// Pushes a free limb part of a pinned ragdoll and returns how far its centre moved in Z.
fn swing(free: &str, push: f32) -> f32 {
    let mut engine = engine();
    let tree = engine.tree_mut();
    let doll = ragdoll::spawn_humanoid_ragdoll(tree, &HumanoidRagdoll::default()).unwrap();
    tree.add_child(tree.root(), doll).unwrap();
    // Everything but the limb is pinned in place.
    for node in tree.ragdoll_parts(doll) {
        if tree.name(node).is_some_and(|n| *n != *free) {
            tree.component_mut::<PhysicsBody3D>(node).unwrap().kind = BodyKind::Static;
        }
    }
    frames(&mut engine, 1);
    let limb = part(&engine, doll, free);
    let start = engine.tree().global_position(limb).unwrap();
    engine
        .tree_mut()
        .component_mut::<PhysicsBody3D>(limb)
        .unwrap()
        .apply_central_impulse(Vec3::new(0.0, 0.0, push));
    frames(&mut engine, 12);
    engine.tree().global_position(limb).unwrap().z - start.z
}

#[test]
fn elbows_bend_forward_and_knees_backward() {
    // Forward is -Z.
    assert!(swing("LowerArmL", -3.0) < -0.08, "elbow bends forward");
    assert!(
        swing("LowerArmL", 3.0) < 0.04,
        "elbow does not bend backward"
    );
    assert!(swing("ShinR", 4.0) > 0.08, "knee bends backward");
    assert!(swing("ShinR", -4.0) > -0.04, "knee does not bend forward");
}

#[test]
fn a_skinned_model_follows_its_animation_then_goes_limp() {
    let mut engine = engine();
    let tree = engine.tree_mut();
    let totem = spawn_gltf(tree, "models/totem.gltf").unwrap();
    tree.add_child(tree.root(), totem).unwrap();
    let doll = ragdoll::ragdoll_for_model(tree, totem, SkeletonRagdoll::new(vec![])).unwrap();
    let parts = tree.ragdoll_parts(doll);
    assert_eq!(parts.len(), 3, "one capsule per totem segment");
    frames(&mut engine, 30);
    assert!(!engine.tree().is_ragdoll_active(doll));

    let tree = engine.tree_mut();
    let top_bone = tree.find_child(totem, "Joint2", true).unwrap();
    let before = tree.global_position(top_bone).unwrap();
    ragdoll::go_limp(tree, doll, Some((Vec3::new(30.0, 0.0, 0.0), before))).unwrap();
    frames(&mut engine, 180);
    let after = engine.tree().global_position(top_bone).unwrap();
    assert!(after.y < before.y - 0.3, "toppled: {before} → {after}");
    assert!(after.y > -0.2, "lying on the floor");
}

#[test]
fn physics_view_profiling_and_time_scale() {
    let mut engine = engine();
    #[derive(Default)]
    struct Spinner;
    impl NodeScript for Spinner {
        fn process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {
            let _ = tree.rotate(node, Vec3::Y, delta);
        }
    }
    let tree = engine.tree_mut();
    let spinner = tree.create_node3d("Spinner");
    tree.set_script(spinner, Spinner).unwrap();
    tree.add_child(tree.root(), spinner).unwrap();
    frames(&mut engine, 2);
    assert_eq!(
        engine
            .tree()
            .world()
            .resource::<DebugDraw>()
            .unwrap()
            .line_count(),
        0,
        "physics view off by default"
    );
    // F4 toggles it.
    engine.handle_event(&PlatformEvent::Key {
        code: KeyCode::F4,
        pressed: true,
        repeat: false,
    });
    frames(&mut engine, 1);
    let world = engine.tree().world();
    assert!(world.resource::<DebugSettings>().unwrap().physics);
    assert!(
        world.resource::<DebugDraw>().unwrap().line_count() >= 12,
        "floor box drawn"
    );

    // Scripts are profiled per type; frames are kept for graphs.
    let profiler = world.resource::<Profiler>().unwrap();
    assert!(profiler.average_ms("script.Spinner").is_some());
    assert_eq!(profiler.history().len(), 3);
    drop(profiler);

    // Time scale: half speed turns half as far.
    let angle = |engine: &Engine| {
        engine
            .tree()
            .rotation(spinner)
            .unwrap()
            .to_euler(ab_engine::core::EulerRot::YXZ)
            .0
    };
    let start = angle(&engine);
    frames(&mut engine, 30);
    let normal = angle(&engine) - start;
    engine
        .tree()
        .world()
        .resource_mut::<Time>()
        .unwrap()
        .time_scale = 0.5;
    let start = angle(&engine);
    frames(&mut engine, 30);
    let slow = angle(&engine) - start;
    assert!((slow - normal * 0.5).abs() < 0.01, "{slow} vs {normal}");
}

/// `assets/samples/ragdolls.abscene`: a floor, steps, a mannequin that falls on Play and one
/// that stands until hit. Regenerate with `AB_WRITE_SAMPLES=1 cargo test -p ab-engine
/// --test ragdoll`.
fn playground() -> ab_engine::scene_file::SceneFile {
    use ab_engine::scene_file::SceneFile;
    let mut text = String::from(
        "[environment]\nbackground = \"#14171d\"\nambient_color = \"#b4c3dc\"\nambient_energy = 0.45\n\n\
         [[node]]\nname = \"Playground\"\ntype = \"Node3D\"\n\n\
         [[node]]\nname = \"Sun\"\nparent = \".\"\ntype = \"DirectionalLight3D\"\nrotation = [-50.0, 35.0, 0.0]\nlight = { energy = 3.0, shadows = true }\n\n\
         [[node]]\nname = \"Camera\"\nparent = \".\"\ntype = \"Camera3D\"\nposition = [5.5, 3.2, 6.5]\nrotation = [-18.0, 38.0, 0.0]\ncamera = { fov = 60.0, current = true }\n",
    );
    for (i, (center, size)) in [
        ([0.0, -0.5, 0.0], [20.0, 1.0, 20.0]),
        ([0.0, 0.25, -1.0], [3.0, 0.5, 2.0]),
        ([0.0, 0.75, -2.0], [3.0, 0.5, 2.0]),
        ([0.0, 1.25, -3.0], [3.0, 0.5, 2.0]),
    ]
    .into_iter()
    .enumerate()
    {
        let [x, y, z] = center;
        let [sx, sy, sz] = size;
        text.push_str(&format!(
            "\n[[node]]\nname = \"Step{i}\"\nparent = \".\"\ntype = \"StaticBody3D\"\nposition = [{x:?}, {y:?}, {z:?}]\n\n\
             [[node]]\nname = \"Mesh\"\nparent = \"Step{i}\"\ntype = \"MeshInstance3D\"\nscale = [{sx:?}, {sy:?}, {sz:?}]\nmesh = {{ primitive = \"cube\" }}\nmaterial = {{ color = \"#5a5e66\", roughness = 0.85 }}\n\n\
             [[node]]\nname = \"Shape\"\nparent = \"Step{i}\"\ntype = \"CollisionShape3D\"\nshape = {{ box = [{sx:?}, {sy:?}, {sz:?}] }}\n"
        ));
    }
    let mut scene = SceneFile::from_toml(&text).unwrap();
    for (name, active, position, rotation) in [
        ("Falling", true, [0.0, 3.2, -1.0], [70.0, 20.0, 0.0]),
        ("Standing", false, [3.0, 0.0, 1.0], [0.0, -30.0, 0.0]),
    ] {
        let mut doll = ragdoll::humanoid_ragdoll(&HumanoidRagdoll {
            name: name.into(),
            active,
            ..Default::default()
        });
        for node in &mut doll.nodes {
            node.parent = Some(match node.parent.as_deref() {
                None => ".".to_owned(),
                Some(".") => name.to_owned(),
                Some(path) => format!("{name}/{path}"),
            });
        }
        doll.nodes[0].position = Some(position);
        doll.nodes[0].rotation = Some(rotation);
        scene.nodes.extend(doll.nodes);
    }
    scene
}

#[test]
fn the_ragdoll_sample_scene_is_current_and_plays() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/samples/ragdolls.abscene"
    );
    let scene = playground();
    if std::env::var_os("AB_WRITE_SAMPLES").is_some() {
        std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
        std::fs::write(path, scene.to_toml()).unwrap();
    }
    let text = std::fs::read_to_string(path).expect("run with AB_WRITE_SAMPLES=1 to create it");
    let saved = ab_engine::scene_file::SceneFile::from_toml(&text).unwrap();
    assert_eq!(saved, scene, "regenerate with AB_WRITE_SAMPLES=1");

    let mut engine = engine();
    let tree = engine.tree_mut();
    let root = ab_engine::scene_file::load_scene(tree, "samples/ragdolls.abscene").unwrap();
    tree.add_child(tree.root(), root).unwrap();
    frames(&mut engine, 180);
    let tree = engine.tree();
    let falling = tree.get_node(root, "Falling").unwrap();
    let standing = tree.get_node(root, "Standing").unwrap();
    assert!(tree.is_ragdoll_active(falling) && !tree.is_ragdoll_active(standing));
    let head = tree.get_node(standing, "Head").unwrap();
    assert!(
        tree.global_position(head).unwrap().y > 1.5,
        "still standing"
    );
    let chest = tree.get_node(falling, "Chest").unwrap();
    assert!(
        tree.global_position(chest).unwrap().y < 2.0,
        "tumbled down the steps"
    );
}

/// A scene with a Mixamo-style character (plain bones, T-pose) and a humanoid ragdoll for it.
fn character_scene(bones_override: &str) -> String {
    let rig: &[(&str, &str, [f32; 3])] = &[
        ("mixamorig_Hips", "", [0.0, 1.0, 0.0]),
        ("mixamorig_Spine", "mixamorig_Hips", [0.0, 0.1, 0.0]),
        ("mixamorig_Spine2", "mixamorig_Spine", [0.0, 0.25, 0.0]),
        ("mixamorig_Neck", "mixamorig_Spine2", [0.0, 0.2, 0.0]),
        ("mixamorig_Head", "mixamorig_Neck", [0.0, 0.08, 0.0]),
        ("mixamorig_LeftArm", "mixamorig_Spine2", [0.18, 0.13, 0.0]),
        (
            "mixamorig_LeftForeArm",
            "mixamorig_LeftArm",
            [0.27, 0.0, 0.0],
        ),
        (
            "mixamorig_LeftHand",
            "mixamorig_LeftForeArm",
            [0.25, 0.0, 0.0],
        ),
        ("mixamorig_RightArm", "mixamorig_Spine2", [-0.18, 0.13, 0.0]),
        (
            "mixamorig_RightForeArm",
            "mixamorig_RightArm",
            [-0.27, 0.0, 0.0],
        ),
        (
            "mixamorig_RightHand",
            "mixamorig_RightForeArm",
            [-0.25, 0.0, 0.0],
        ),
        ("mixamorig_LeftUpLeg", "mixamorig_Hips", [0.09, -0.05, 0.0]),
        (
            "mixamorig_LeftLeg",
            "mixamorig_LeftUpLeg",
            [0.0, -0.43, 0.0],
        ),
        ("mixamorig_LeftFoot", "mixamorig_LeftLeg", [0.0, -0.43, 0.0]),
        (
            "mixamorig_RightUpLeg",
            "mixamorig_Hips",
            [-0.09, -0.05, 0.0],
        ),
        (
            "mixamorig_RightLeg",
            "mixamorig_RightUpLeg",
            [0.0, -0.43, 0.0],
        ),
        (
            "mixamorig_RightFoot",
            "mixamorig_RightLeg",
            [0.0, -0.43, 0.0],
        ),
    ];
    let mut toml = String::from(
        "[[node]]\nname = \"Level\"\ntype = \"Node3D\"\n\n\
         [[node]]\nname = \"Hero\"\nparent = \".\"\ntype = \"Node3D\"\nposition = [3.0, 0.0, 0.0]\n\n\
         [[node]]\nname = \"Model\"\nparent = \"Hero\"\ntype = \"Node3D\"\n",
    );
    let path_of = |name: &str| -> String {
        let mut path = vec![name.to_owned()];
        let mut current = name;
        while let Some((_, parent, _)) = rig.iter().find(|(n, ..)| *n == current) {
            if parent.is_empty() {
                break;
            }
            path.push((*parent).to_owned());
            current = parent;
        }
        path.reverse();
        format!("Hero/Model/{}", path.join("/"))
    };
    for (name, parent, [x, y, z]) in rig {
        let parent = if parent.is_empty() {
            "Hero/Model".to_owned()
        } else {
            path_of(parent)
        };
        toml.push_str(&format!(
            "\n[[node]]\nname = \"{name}\"\nparent = \"{parent}\"\ntype = \"Node3D\"\nposition = [{x}, {y}, {z}]\n"
        ));
    }
    toml.push_str(&format!(
        "\n[[node]]\nname = \"Ragdoll\"\nparent = \"Hero\"\ntype = \"Ragdoll\"\nragdoll = {{ skeleton = \"../Model\", mass = 80.0{bones_override} }}\n"
    ));
    toml
}

#[test]
fn a_scene_file_maps_a_character_and_saves_only_the_rig() {
    let mut engine = engine();
    let scene = SceneFile::from_toml(&character_scene("")).unwrap();
    let tree = engine.tree_mut();
    let level = instantiate_scene(tree, &scene, "test").unwrap();
    tree.add_child(tree.root(), level).unwrap();
    let doll = tree.get_node(level, "Hero/Ragdoll").unwrap();
    let parts = tree.ragdoll_parts(doll);
    assert_eq!(parts.len(), 14);
    let mass: f32 = parts
        .iter()
        .map(|&p| tree.component::<PhysicsBody3D>(p).unwrap().mass)
        .sum();
    assert!((mass - 80.0).abs() < 0.01);
    // The bodies sit on the character, which stands 3 m along x.
    let pelvis = tree.get_node(doll, "Pelvis").unwrap();
    let at = tree.global_position(pelvis).unwrap();
    assert!(
        (at.x - 3.0).abs() < 0.05 && (at.y - 1.0).abs() < 0.2,
        "{at}"
    );

    // Saved: the rig, not its bodies; loading again rebuilds them.
    let saved = save_scene(tree, level);
    let desc = saved.nodes.iter().find(|n| n.name == "Ragdoll").unwrap();
    let rig = desc.ragdoll.as_ref().unwrap();
    assert_eq!(rig.skeleton.as_deref(), Some("../Model"));
    assert_eq!(rig.mass, Some(80.0));
    assert!(!saved.nodes.iter().any(|n| n.name == "Pelvis"));
    let again = instantiate_scene(
        tree,
        &SceneFile::from_toml(&saved.to_toml()).unwrap(),
        "again",
    )
    .unwrap();
    let doll = tree.get_node(again, "Hero/Ragdoll").unwrap();
    assert_eq!(tree.ragdoll_parts(doll).len(), 14);

    // Limp: it falls and carries the bones.
    frames(&mut engine, 1);
    let doll = engine.tree().get_node(level, "Hero/Ragdoll").unwrap();
    ragdoll::go_limp(engine.tree_mut(), doll, None).unwrap();
    let head = engine.tree().get_node(level, "Hero/Model/mixamorig_Hips/mixamorig_Spine/mixamorig_Spine2/mixamorig_Neck/mixamorig_Head").unwrap();
    let start = engine.tree().global_position(head).unwrap();
    frames(&mut engine, 180);
    let end = engine.tree().global_position(head).unwrap();
    assert!(end.y < start.y - 0.8, "{start} → {end}");
}

#[test]
fn a_bone_set_by_hand_wins_and_a_missing_one_is_reported() {
    let mut engine = engine();
    // No left foot: still a ragdoll (feet are optional), with one foot body less.
    let scene = SceneFile::from_toml(&character_scene(", bones = { left_foot = \"\" }")).unwrap();
    let tree = engine.tree_mut();
    let level = instantiate_scene(tree, &scene, "test").unwrap();
    let doll = tree.get_node(level, "Hero/Ragdoll").unwrap();
    assert_eq!(tree.ragdoll_parts(doll).len(), 13);
    // Without a knee there is no ragdoll, and loading still succeeds.
    let scene =
        SceneFile::from_toml(&character_scene(", bones = { left_lower_leg = \"\" }")).unwrap();
    let level = instantiate_scene(tree, &scene, "test").unwrap();
    let doll = tree.get_node(level, "Hero/Ragdoll").unwrap();
    assert!(tree.ragdoll_parts(doll).is_empty());
}

#[test]
fn the_sample_mannequin_gets_a_full_humanoid_ragdoll() {
    let mut engine = engine();
    let tree = engine.tree_mut();
    let model = spawn_gltf(tree, "models/mannequin.gltf").unwrap();
    tree.add_child(tree.root(), model).unwrap();
    tree.update_transforms();
    let map = ragdoll::humanoid_map(tree, model, &Default::default());
    assert!(map.is_complete(), "missing {:?}", map.missing());
    assert_eq!(map.iter().count(), 17, "every bone, by its Mixamo name");
    let doll = ragdoll::humanoid_ragdoll_for_model(
        tree,
        model,
        &ab_engine::physics::HumanoidRagdollSettings::default(),
    )
    .unwrap();
    assert_eq!(tree.ragdoll_parts(doll).len(), 14);
    // It plays its clips as hit boxes, then falls when limp.
    frames(&mut engine, 30);
    ragdoll::go_limp(engine.tree_mut(), doll, None).unwrap();
    frames(&mut engine, 120);
    let parts = engine.tree().ragdoll_parts(doll);
    let pelvis = engine.tree().global_position(parts[0]).unwrap();
    assert!(pelvis.y < 0.6 && pelvis.is_finite(), "{pelvis}");
}
