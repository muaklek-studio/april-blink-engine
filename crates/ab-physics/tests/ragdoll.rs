//! Joints and ragdolls.

use ab_core::{Affine3A, Quat, Time, Vec3};
use ab_physics::{
    physics_system, BodyKind, Joint3D, JointKind, PhysicsBody3D, PhysicsDebugOptions, PhysicsNodes,
    PhysicsWorld, Ragdoll, RagdollBone, Shape, SkeletonRagdoll,
};
use ab_scene::{NodeHandle, SceneTree};

const DT: f32 = 1.0 / 60.0;

fn new_tree() -> SceneTree {
    let mut tree = SceneTree::new();
    tree.world_mut().insert_resource(Time::new(DT));
    tree.world_mut().insert_resource(PhysicsWorld::new());
    tree
}

fn steps(tree: &mut SceneTree, count: usize) {
    for _ in 0..count {
        physics_system(tree.world_mut());
        tree.process(DT);
    }
}

fn body(tree: &mut SceneTree, name: &str, kind: BodyKind, shape: Shape, at: Vec3) -> NodeHandle {
    let node = tree.create_physics_body3d(name, PhysicsBody3D::new(kind));
    tree.add_collision_shape(node, shape).unwrap();
    tree.set_position(node, at).unwrap();
    tree.add_child(tree.root(), node).unwrap();
    node
}

fn joint(tree: &mut SceneTree, joint: Joint3D, at: Vec3) -> NodeHandle {
    let node = tree.create_joint3d("Joint", joint);
    tree.set_position(node, at).unwrap();
    tree.add_child(tree.root(), node).unwrap();
    node
}

fn floor(tree: &mut SceneTree) {
    body(
        tree,
        "Floor",
        BodyKind::Static,
        Shape::Box {
            size: Vec3::new(40.0, 1.0, 40.0),
        },
        Vec3::new(0.0, -0.5, 0.0),
    );
}

#[test]
fn a_pin_joint_hangs_a_body_from_a_fixed_anchor() {
    let mut tree = new_tree();
    let anchor = body(
        &mut tree,
        "Anchor",
        BodyKind::Static,
        Shape::Sphere { radius: 0.1 },
        Vec3::new(0.0, 5.0, 0.0),
    );
    // Starts to the side: swings like a pendulum, staying 1 m from the anchor.
    let bob = body(
        &mut tree,
        "Bob",
        BodyKind::Rigid,
        Shape::Sphere { radius: 0.2 },
        Vec3::new(1.0, 5.0, 0.0),
    );
    let pin = joint(
        &mut tree,
        Joint3D::pin(anchor, bob),
        Vec3::new(0.0, 5.0, 0.0),
    );
    steps(&mut tree, 1);
    assert!(tree.component::<Joint3D>(pin).unwrap().is_active());
    assert_eq!(
        tree.world()
            .resource::<PhysicsWorld>()
            .unwrap()
            .joint_count(),
        1
    );
    let mut lowest = f32::MAX;
    for _ in 0..90 {
        steps(&mut tree, 1);
        let at = tree.global_position(bob).unwrap();
        let distance = at.distance(Vec3::new(0.0, 5.0, 0.0));
        assert!(
            (distance - 1.0).abs() < 0.05,
            "rope length kept: {distance}"
        );
        lowest = lowest.min(at.y);
    }
    assert!(lowest < 4.1, "swung down: {lowest}");

    // Removing the joint node frees the bob.
    tree.queue_free(pin).unwrap();
    steps(&mut tree, 60);
    assert!(
        tree.global_position(bob).unwrap().y < 2.0,
        "fell after release"
    );
    assert_eq!(
        tree.world()
            .resource::<PhysicsWorld>()
            .unwrap()
            .joint_count(),
        0
    );
}

#[test]
fn a_hinge_respects_its_limits_and_a_fixed_joint_holds() {
    let mut tree = new_tree();
    let wall = body(
        &mut tree,
        "Wall",
        BodyKind::Static,
        Shape::Box {
            size: Vec3::splat(0.2),
        },
        Vec3::new(0.0, 3.0, 0.0),
    );
    // A door hanging along +X from a hinge whose axis (the joint's X) points down Z.
    let door = body(
        &mut tree,
        "Door",
        BodyKind::Rigid,
        Shape::Box {
            size: Vec3::new(1.0, 0.1, 0.1),
        },
        Vec3::new(0.6, 3.0, 0.0),
    );
    let hinge = joint(
        &mut tree,
        Joint3D::hinge(wall, door, -30.0, 30.0),
        Vec3::new(0.1, 3.0, 0.0),
    );
    tree.set_rotation(hinge, Quat::from_rotation_y(std::f32::consts::FRAC_PI_2))
        .unwrap();
    steps(&mut tree, 120);
    // Gravity pulls it down, the limit stops it at 30°.
    let door_at = tree.global_position(door).unwrap();
    let angle = (3.0 - door_at.y).atan2(door_at.x - 0.1).to_degrees();
    assert!(
        (20.0..36.0).contains(&angle),
        "stopped at the limit: {angle}°"
    );

    let a = body(
        &mut tree,
        "A",
        BodyKind::Rigid,
        Shape::Box {
            size: Vec3::splat(0.5),
        },
        Vec3::new(5.0, 2.0, 0.0),
    );
    let b = body(
        &mut tree,
        "B",
        BodyKind::Rigid,
        Shape::Box {
            size: Vec3::splat(0.5),
        },
        Vec3::new(5.5, 2.0, 0.0),
    );
    joint(&mut tree, Joint3D::fixed(a, b), Vec3::new(5.25, 2.0, 0.0));
    floor(&mut tree);
    steps(&mut tree, 120);
    let gap = tree.global_position(b).unwrap() - tree.global_position(a).unwrap();
    assert!((gap.length() - 0.5).abs() < 0.03, "welded: {gap}");
}

/// A three-bone chain standing up, like the totem sample.
fn skeleton(tree: &mut SceneTree) -> (NodeHandle, Vec<NodeHandle>) {
    let model = tree.create_node3d("Model");
    tree.add_child(tree.root(), model).unwrap();
    let mut bones = Vec::new();
    let mut parent = model;
    for (i, y) in [0.0, 0.6, 0.6].into_iter().enumerate() {
        let bone = tree.create_node3d(&format!("Bone{i}"));
        tree.set_position(bone, Vec3::new(0.0, y, 0.0)).unwrap();
        tree.add_child(parent, bone).unwrap();
        bones.push(bone);
        parent = bone;
    }
    tree.set_position(model, Vec3::new(0.0, 0.05, 0.0)).unwrap();
    (model, bones)
}

#[test]
fn a_skeleton_ragdoll_follows_the_bones_then_goes_limp() {
    let mut tree = new_tree();
    floor(&mut tree);
    let (model, bones) = skeleton(&mut tree);
    let settings = SkeletonRagdoll::new(bones.clone());
    let ragdoll = tree
        .create_skeleton_ragdoll(model, "Ragdoll", &settings)
        .unwrap();
    let parts = tree.ragdoll_parts(ragdoll);
    assert_eq!(parts.len(), 3, "a capsule per bone");
    assert_eq!(
        tree.world()
            .get::<RagdollBone>(parts[0].entity())
            .unwrap()
            .bone,
        bones[0]
    );
    steps(&mut tree, 2);
    assert_eq!(
        tree.world()
            .resource::<PhysicsWorld>()
            .unwrap()
            .joint_count(),
        2
    );

    // Inactive: kinematic, following the animated bones.
    tree.set_position(model, Vec3::new(2.0, 0.05, 0.0)).unwrap();
    steps(&mut tree, 2);
    let top = tree.global_position(parts[2]).unwrap();
    assert!((top.x - 2.0).abs() < 1e-3, "followed the model: {top}");
    assert_eq!(
        tree.component::<PhysicsBody3D>(parts[0]).unwrap().kind,
        BodyKind::Kinematic
    );

    // Active: the chain topples and drives the bones.
    tree.set_ragdoll_active(ragdoll, true).unwrap();
    assert!(tree.is_ragdoll_active(ragdoll));
    let pushed =
        tree.apply_ragdoll_impulse(ragdoll, Vec3::new(8.0, 0.0, 0.0), Vec3::new(2.0, 1.4, 0.0));
    assert_eq!(pushed, Some(parts[2]), "the nearest part takes the hit");
    steps(&mut tree, 180);
    assert_eq!(
        tree.component::<PhysicsBody3D>(parts[0]).unwrap().kind,
        BodyKind::Rigid
    );
    let tip = tree.global_position(bones[2]).unwrap();
    assert!(tip.y < 0.6, "fell over: {tip}");
    assert!(tip.y > -0.1, "on the floor, not through it: {tip}");
    let part_tip = tree.global_position(parts[2]).unwrap();
    assert!(
        part_tip.distance(tip) < 0.7,
        "the bone moves with its part: {part_tip} vs {tip}"
    );
    // Joints keep the chain together.
    let gap = tree
        .global_position(bones[1])
        .unwrap()
        .distance(tree.global_position(bones[0]).unwrap());
    assert!(
        (gap - 0.6).abs() < 0.1,
        "bones stay apart by their length: {gap}"
    );
}

#[test]
fn a_hand_built_ragdoll_switches_modes() {
    let mut tree = new_tree();
    floor(&mut tree);
    let root = tree.create_ragdoll("Dummy", Ragdoll::inactive());
    tree.add_child(tree.root(), root).unwrap();
    let torso = tree.create_physics_body3d("Torso", PhysicsBody3D::rigid());
    tree.add_collision_shape(
        torso,
        Shape::Box {
            size: Vec3::new(0.4, 0.6, 0.2),
        },
    )
    .unwrap();
    tree.set_position(torso, Vec3::new(0.0, 1.3, 0.0)).unwrap();
    tree.add_child(root, torso).unwrap();
    let head = tree.create_physics_body3d("Head", PhysicsBody3D::rigid());
    tree.add_collision_shape(head, Shape::Sphere { radius: 0.12 })
        .unwrap();
    tree.set_position(head, Vec3::new(0.0, 1.75, 0.0)).unwrap();
    tree.add_child(root, head).unwrap();
    let neck = tree.create_joint3d("Neck", Joint3D::cone_twist(torso, head, 30.0, 20.0));
    tree.set_global_transform(neck, Affine3A::from_translation(Vec3::new(0.0, 1.6, 0.0)))
        .unwrap();
    tree.add_child(root, neck).unwrap();

    steps(&mut tree, 30);
    assert_eq!(
        tree.global_position(torso).unwrap().y,
        1.3,
        "inactive: frozen in place"
    );
    tree.set_ragdoll_active(root, true).unwrap();
    steps(&mut tree, 120);
    let torso_at = tree.global_position(torso).unwrap();
    assert!(torso_at.y < 0.6, "collapsed: {torso_at}");
    let neck_gap = tree.global_position(head).unwrap().distance(torso_at);
    assert!((neck_gap - 0.45).abs() < 0.08, "still attached: {neck_gap}");
    assert!(matches!(
        tree.component::<Joint3D>(neck).unwrap().kind,
        JointKind::ConeTwist { .. }
    ));
}

#[test]
fn debug_lines_cover_shapes_and_joints() {
    let mut tree = new_tree();
    let anchor = body(
        &mut tree,
        "Anchor",
        BodyKind::Static,
        Shape::Sphere { radius: 0.1 },
        Vec3::new(0.0, 5.0, 0.0),
    );
    let bob = body(
        &mut tree,
        "Bob",
        BodyKind::Rigid,
        Shape::Box {
            size: Vec3::splat(0.4),
        },
        Vec3::new(0.0, 4.0, 0.0),
    );
    joint(
        &mut tree,
        Joint3D::pin(anchor, bob),
        Vec3::new(0.0, 5.0, 0.0),
    );
    steps(&mut tree, 1);
    let physics = tree.world().resource::<PhysicsWorld>().unwrap();
    let mut count = 0;
    let mut box_lines = 0;
    physics.debug_lines(&PhysicsDebugOptions::default(), |a, b, _| {
        count += 1;
        if (a.y - 3.8).abs() < 0.25 && (b.y - 3.8).abs() < 0.25 {
            box_lines += 1;
        }
    });
    assert!(count > 12, "{count} lines");
    assert!(box_lines >= 4, "the box's bottom edges are drawn");
    let mut none = 0;
    physics.debug_lines(
        &PhysicsDebugOptions {
            shapes: false,
            joints: false,
            contacts: false,
            aabbs: false,
        },
        |_, _, _| none += 1,
    );
    assert_eq!(none, 0);
}

/// A Mixamo-style character skeleton in T-pose (facing +Z, 1.8 m tall), as bone nodes under
/// an `Armature` node. Returns the armature and the bones.
fn mixamo_skeleton(tree: &mut SceneTree) -> (NodeHandle, Vec<NodeHandle>) {
    // (name, parent, global position)
    let rig: &[(&str, &str, [f32; 3])] = &[
        ("mixamorig:Hips", "", [0.0, 1.0, 0.0]),
        ("mixamorig:Spine", "mixamorig:Hips", [0.0, 1.1, 0.0]),
        ("mixamorig:Spine1", "mixamorig:Spine", [0.0, 1.22, 0.0]),
        ("mixamorig:Spine2", "mixamorig:Spine1", [0.0, 1.35, 0.0]),
        ("mixamorig:Neck", "mixamorig:Spine2", [0.0, 1.55, 0.0]),
        ("mixamorig:Head", "mixamorig:Neck", [0.0, 1.63, 0.0]),
        (
            "mixamorig:LeftShoulder",
            "mixamorig:Spine2",
            [0.07, 1.48, 0.0],
        ),
        (
            "mixamorig:LeftArm",
            "mixamorig:LeftShoulder",
            [0.18, 1.48, 0.0],
        ),
        (
            "mixamorig:LeftForeArm",
            "mixamorig:LeftArm",
            [0.45, 1.48, 0.0],
        ),
        (
            "mixamorig:LeftHand",
            "mixamorig:LeftForeArm",
            [0.7, 1.48, 0.0],
        ),
        (
            "mixamorig:LeftHandIndex1",
            "mixamorig:LeftHand",
            [0.78, 1.48, 0.02],
        ),
        (
            "mixamorig:RightShoulder",
            "mixamorig:Spine2",
            [-0.07, 1.48, 0.0],
        ),
        (
            "mixamorig:RightArm",
            "mixamorig:RightShoulder",
            [-0.18, 1.48, 0.0],
        ),
        (
            "mixamorig:RightForeArm",
            "mixamorig:RightArm",
            [-0.45, 1.48, 0.0],
        ),
        (
            "mixamorig:RightHand",
            "mixamorig:RightForeArm",
            [-0.7, 1.48, 0.0],
        ),
        ("mixamorig:LeftUpLeg", "mixamorig:Hips", [0.09, 0.95, 0.0]),
        (
            "mixamorig:LeftLeg",
            "mixamorig:LeftUpLeg",
            [0.09, 0.52, 0.0],
        ),
        ("mixamorig:LeftFoot", "mixamorig:LeftLeg", [0.09, 0.09, 0.0]),
        (
            "mixamorig:LeftToeBase",
            "mixamorig:LeftFoot",
            [0.09, 0.02, 0.14],
        ),
        ("mixamorig:RightUpLeg", "mixamorig:Hips", [-0.09, 0.95, 0.0]),
        (
            "mixamorig:RightLeg",
            "mixamorig:RightUpLeg",
            [-0.09, 0.52, 0.0],
        ),
        (
            "mixamorig:RightFoot",
            "mixamorig:RightLeg",
            [-0.09, 0.09, 0.0],
        ),
        (
            "mixamorig:RightToeBase",
            "mixamorig:RightFoot",
            [-0.09, 0.02, 0.14],
        ),
    ];
    let armature = tree.create_node3d("Armature");
    tree.add_child(tree.root(), armature).unwrap();
    let mut bones: Vec<NodeHandle> = Vec::new();
    for (name, parent, at) in rig {
        let node = tree.create_node3d(name);
        let parent = rig
            .iter()
            .position(|(n, ..)| n == parent)
            .map_or(armature, |i| bones[i]);
        tree.add_child(parent, node).unwrap();
        tree.update_transforms();
        tree.set_global_position(node, Vec3::from(*at)).unwrap();
        bones.push(node);
    }
    tree.update_transforms();
    (armature, bones)
}

#[test]
fn a_humanoid_ragdoll_maps_any_rig_and_falls_in_one_piece() {
    use ab_physics::{create_humanoid_ragdoll, HumanBone, HumanoidMap, HumanoidRagdollSettings};
    let mut tree = new_tree();
    floor(&mut tree);
    let (armature, bones) = mixamo_skeleton(&mut tree);
    let map = HumanoidMap::detect(&tree, &bones);
    assert!(map.is_complete(), "missing {:?}", map.missing());
    assert_eq!(map.iter().count(), 17);
    assert_eq!(
        map.get(HumanBone::LeftUpperArm)
            .and_then(|b| tree.name(b).map(|n| n.to_owned()))
            .as_deref(),
        Some("mixamorig_LeftArm"),
        "node names have `_` for `:`"
    );

    let settings = HumanoidRagdollSettings {
        active: true,
        ..Default::default()
    };
    let doll = create_humanoid_ragdoll(&mut tree, armature, "Ragdoll", &map, &settings).unwrap();
    let parts = tree.ragdoll_parts(doll);
    assert_eq!(
        parts.len(),
        14,
        "pelvis, belly, chest, head, 2 × (2 arm + 2 leg + foot)"
    );
    let total: f32 = parts
        .iter()
        .map(|&p| tree.component::<PhysicsBody3D>(p).unwrap().mass)
        .sum();
    assert!((total - 70.0).abs() < 0.01, "{total}");
    let joints = tree
        .children(doll)
        .into_iter()
        .filter(|&n| tree.component::<Joint3D>(n).is_some())
        .count();
    assert_eq!(
        joints, 13,
        "waist, spine, neck, 2 × (shoulder, elbow, hip, knee, ankle)"
    );

    let head_bone = map.get(HumanBone::Head).unwrap();
    let start = tree.global_position(head_bone).unwrap();
    steps(&mut tree, 300);
    // Down on the floor, in one piece: every part near the others, nothing under the floor
    // or flung away.
    let head = tree.global_position(head_bone).unwrap();
    assert!(head.y < start.y - 0.8, "the head fell: {head}");
    let centre = tree.global_position(parts[0]).unwrap();
    for &part in &parts {
        let at = tree.global_position(part).unwrap();
        assert!(at.is_finite() && at.y > -0.05, "{at}");
        assert!(
            at.distance(centre) < 1.3,
            "{} drifted to {at}",
            tree.name(part).unwrap()
        );
    }
    // The bones went with their bodies.
    let hips = tree
        .global_position(map.get(HumanBone::Hips).unwrap())
        .unwrap();
    assert!(hips.distance(centre) < 0.3, "{hips} vs {centre}");
}

#[test]
fn an_inactive_humanoid_ragdoll_follows_the_animation() {
    use ab_physics::{create_humanoid_ragdoll, HumanBone, HumanoidMap, HumanoidRagdollSettings};
    let mut tree = new_tree();
    let (armature, bones) = mixamo_skeleton(&mut tree);
    let map = HumanoidMap::detect(&tree, &bones);
    let doll = create_humanoid_ragdoll(
        &mut tree,
        armature,
        "Ragdoll",
        &map,
        &HumanoidRagdollSettings::default(),
    )
    .unwrap();
    steps(&mut tree, 2);
    let forearm = map.get(HumanBone::LeftLowerArm).unwrap();
    let part = tree
        .ragdoll_parts(doll)
        .into_iter()
        .find(|&p| {
            tree.component::<RagdollBone>(p)
                .is_some_and(|b| b.bone == forearm)
        })
        .unwrap();
    let before = tree.global_position(part).unwrap();
    // "Animate" the elbow: the hit box follows.
    tree.set_rotation(forearm, Quat::from_rotation_z(-1.2))
        .unwrap();
    steps(&mut tree, 2);
    let after = tree.global_position(part).unwrap();
    assert!(after.distance(before) > 0.05, "{before} → {after}");
    assert!(!tree.is_ragdoll_active(doll));
}

#[test]
fn a_rig_missing_a_limb_is_refused_with_its_name() {
    use ab_physics::{create_humanoid_ragdoll, HumanBone, HumanoidRagdollSettings};
    let mut tree = new_tree();
    let (armature, bones) = mixamo_skeleton(&mut tree);
    let mut map = ab_physics::HumanoidMap::detect(&tree, &bones);
    map.set(HumanBone::RightLowerLeg, None);
    let err = create_humanoid_ragdoll(
        &mut tree,
        armature,
        "Ragdoll",
        &map,
        &HumanoidRagdollSettings::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("right_lower_leg"), "{err}");
}

#[test]
fn inactive_hit_boxes_follow_the_bones_in_the_same_step() {
    let mut tree = new_tree();
    let (model, bones) = skeleton(&mut tree);
    let ragdoll = tree
        .create_skeleton_ragdoll(model, "Ragdoll", &SkeletonRagdoll::new(bones.clone()))
        .unwrap();
    steps(&mut tree, 2);
    let hits_at = |tree: &SceneTree, x: f32| {
        tree.world()
            .resource::<PhysicsWorld>()
            .unwrap()
            .cast_ray(
                Vec3::new(x, 0.9, 5.0),
                Vec3::NEG_Z,
                10.0,
                &Default::default(),
            )
            .is_some()
    };
    assert!(hits_at(&tree, 0.0));
    // The animation moves the root bone (the ragdoll node is its sibling, so only the
    // ragdoll follows it): the hit boxes are there after the very next step, not a step late.
    tree.set_position(bones[0], Vec3::new(2.0, 0.0, 0.0))
        .unwrap();
    steps(&mut tree, 1);
    assert!(hits_at(&tree, 2.0), "hit boxes lag behind the bones");
    assert!(!hits_at(&tree, 0.0), "hit boxes left behind");
    assert!(!tree.is_ragdoll_active(ragdoll));
}

#[test]
fn limp_bones_under_bones_without_a_body_stay_on_their_parts() {
    let mut tree = new_tree();
    let model = tree.create_node3d("Model");
    tree.add_child(tree.root(), model).unwrap();
    // Bone1 has no body of its own (not in the ragdoll's list): Bone2 moves through it.
    let mut all = Vec::new();
    let mut parent = model;
    for (i, y) in [0.0, 0.3, 0.3, 0.3].into_iter().enumerate() {
        let bone = tree.create_node3d(&format!("Bone{i}"));
        tree.set_position(bone, Vec3::new(0.0, y, 0.0)).unwrap();
        tree.add_child(parent, bone).unwrap();
        all.push(bone);
        parent = bone;
    }
    tree.set_position(model, Vec3::new(0.0, 5.0, 0.0)).unwrap();
    tree.update_transforms();
    let mut settings = SkeletonRagdoll::new(vec![all[0], all[2], all[3]]);
    settings.active = true;
    let ragdoll = tree
        .create_skeleton_ragdoll(model, "Ragdoll", &settings)
        .unwrap();
    // Falling freely: every step moves the whole chain.
    steps(&mut tree, 30);
    for part in tree.ragdoll_parts(ragdoll) {
        let bone = *tree.component::<RagdollBone>(part).unwrap();
        let expected = tree.global_transform(part).unwrap() * bone.offset;
        let actual = tree.global_transform(bone.bone).unwrap();
        let gap = Vec3::from(expected.translation).distance(Vec3::from(actual.translation));
        assert!(
            gap < 1e-3,
            "{} is {gap} m off its part",
            tree.name(bone.bone).unwrap()
        );
    }
}
