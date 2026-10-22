use ab_core::{Affine3A, Quat, Transform3D, Vec3};
use ab_scene::{NodeHandle, NodeScript, SceneError, SceneTree};
use std::cell::RefCell;
use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

fn approx(a: Vec3, b: Vec3) -> bool {
    a.abs_diff_eq(b, 1e-4)
}

// ---------------------------------------------------------------------------------------------
// Hierarchy
// ---------------------------------------------------------------------------------------------

#[test]
fn add_and_remove_children() {
    let mut tree = SceneTree::new();
    let root = tree.root();
    let a = tree.create_node("A");
    let b = tree.create_node("B");

    tree.add_child(root, a).unwrap();
    tree.add_child(a, b).unwrap();
    assert_eq!(tree.parent(b), Some(a));
    assert_eq!(tree.children(root), vec![a]);
    assert!(tree.is_inside_tree(b));
    assert!(tree.is_ancestor_of(root, b));

    tree.remove_child(a, b).unwrap();
    assert_eq!(tree.parent(b), None);
    assert_eq!(tree.child_count(a), 0);
    assert!(!tree.is_inside_tree(b));
    assert!(tree.is_valid(b), "removed nodes stay alive");
}

#[test]
fn hierarchy_errors() {
    let mut tree = SceneTree::new();
    let root = tree.root();
    let a = tree.create_node("A");
    let b = tree.create_node("B");
    tree.add_child(root, a).unwrap();
    tree.add_child(a, b).unwrap();

    assert_eq!(
        tree.add_child(root, b),
        Err(SceneError::AlreadyHasParent {
            child: b,
            parent: a
        })
    );
    assert_eq!(
        tree.reparent(a, b, false),
        Err(SceneError::WouldCreateCycle {
            parent: b,
            child: a
        })
    );
    assert_eq!(tree.add_child(a, root), Err(SceneError::RootNode));
    assert_eq!(
        tree.remove_child(root, b),
        Err(SceneError::NotAChild {
            parent: root,
            child: b
        })
    );
    assert_eq!(tree.queue_free(root), Err(SceneError::RootNode));
    assert_eq!(
        tree.set_position(a, Vec3::ONE),
        Err(SceneError::NotNode3D(a))
    );
}

#[test]
fn sibling_names_are_made_unique() {
    let mut tree = SceneTree::new();
    let root = tree.root();
    let names: Vec<String> = (0..3)
        .map(|_| {
            let n = tree.create_node("Enemy");
            tree.add_child(root, n).unwrap();
            tree.name(n).unwrap().to_owned()
        })
        .collect();
    assert_eq!(names, ["Enemy", "Enemy2", "Enemy3"]);

    let bad = tree.create_node("a/b.c");
    assert_eq!(&*tree.name(bad).unwrap(), "a_b_c");
    let unnamed = tree.create_node3d("");
    assert_eq!(&*tree.name(unnamed).unwrap(), "Node3D");

    let first = tree.get_child(root, 0).unwrap();
    tree.set_name(first, "Enemy3").unwrap();
    assert_eq!(&*tree.name(first).unwrap(), "Enemy4");
}

#[test]
fn get_node_paths() {
    let mut tree = SceneTree::new();
    let root = tree.root();
    let level = tree.create_node("Level");
    let player = tree.create_node3d("Player");
    let camera = tree.create_node3d("Camera");
    let enemy = tree.create_node3d("Enemy");
    tree.add_child(root, level).unwrap();
    tree.add_child(level, player).unwrap();
    tree.add_child(player, camera).unwrap();
    tree.add_child(level, enemy).unwrap();

    assert_eq!(tree.get_node(level, "Player/Camera"), Some(camera));
    assert_eq!(tree.get_node(camera, "../../Enemy"), Some(enemy));
    assert_eq!(tree.get_node(camera, "."), Some(camera));
    assert_eq!(tree.get_node(enemy, "/root/Level/Player"), Some(player));
    assert_eq!(tree.get_node(enemy, "/"), Some(root));
    assert_eq!(tree.get_node(level, "Missing"), None);
    assert_eq!(tree.get_node(level, "/notroot/Level"), None);
    assert_eq!(tree.get_node(root, ".."), None);

    assert_eq!(
        tree.path_of(camera).as_deref(),
        Some("/root/Level/Player/Camera")
    );
    assert_eq!(tree.find_child(root, "Camera", true), Some(camera));
    assert_eq!(tree.find_child(root, "Camera", false), None);
}

#[test]
fn move_child_reorders() {
    let mut tree = SceneTree::new();
    let root = tree.root();
    let [a, b, c] = ["A", "B", "C"].map(|n| tree.create_node(n));
    for n in [a, b, c] {
        tree.add_child(root, n).unwrap();
    }
    tree.move_child(root, c, 0).unwrap();
    assert_eq!(tree.children(root), vec![c, a, b]);
    tree.move_child(root, c, 99).unwrap();
    assert_eq!(tree.children(root), vec![a, b, c]);
}

// ---------------------------------------------------------------------------------------------
// Transforms
// ---------------------------------------------------------------------------------------------

#[test]
fn moving_parent_moves_children() {
    let mut tree = SceneTree::new();
    let root = tree.root();
    let parent = tree.create_node3d("Parent");
    let child = tree.create_node3d("Child");
    let grandchild = tree.create_node3d("Grandchild");
    tree.add_child(root, parent).unwrap();
    tree.add_child(parent, child).unwrap();
    tree.add_child(child, grandchild).unwrap();
    tree.set_position(child, Vec3::new(1.0, 0.0, 0.0)).unwrap();
    tree.set_position(grandchild, Vec3::new(0.0, 1.0, 0.0))
        .unwrap();
    tree.update_transforms();
    assert!(approx(
        tree.global_position(grandchild).unwrap(),
        Vec3::new(1.0, 1.0, 0.0)
    ));

    tree.set_position(parent, Vec3::new(10.0, 0.0, 0.0))
        .unwrap();
    // Up to date even before propagation runs.
    assert!(approx(
        tree.global_position(grandchild).unwrap(),
        Vec3::new(11.0, 1.0, 0.0)
    ));
    tree.update_transforms();
    let cached = tree
        .component::<ab_scene::Transform>(grandchild)
        .unwrap()
        .global;
    assert!(approx(cached.translation.into(), Vec3::new(11.0, 1.0, 0.0)));
}

#[test]
fn rotation_and_scale_cascade() {
    let mut tree = SceneTree::new();
    let parent = tree.create_node3d("Parent");
    let child = tree.create_node3d("Child");
    tree.add_child(tree.root(), parent).unwrap();
    tree.add_child(parent, child).unwrap();
    tree.set_transform(
        parent,
        Transform3D::from_trs(
            Vec3::ZERO,
            Quat::from_rotation_y(FRAC_PI_2),
            Vec3::splat(2.0),
        ),
    )
    .unwrap();
    tree.set_position(child, Vec3::X).unwrap();
    // (1,0,0) scaled by 2 then rotated 90° about Y -> (0,0,-2)
    assert!(approx(
        tree.global_position(child).unwrap(),
        Vec3::new(0.0, 0.0, -2.0)
    ));
}

#[test]
fn plain_node_breaks_transform_inheritance() {
    let mut tree = SceneTree::new();
    let mover = tree.create_node3d("Mover");
    let group = tree.create_node("Group");
    let leaf = tree.create_node3d("Leaf");
    tree.add_child(tree.root(), mover).unwrap();
    tree.add_child(mover, group).unwrap();
    tree.add_child(group, leaf).unwrap();
    tree.set_position(mover, Vec3::splat(5.0)).unwrap();
    tree.update_transforms();
    assert!(approx(tree.global_position(leaf).unwrap(), Vec3::ZERO));
}

#[test]
fn set_global_position_under_transformed_parent() {
    let mut tree = SceneTree::new();
    let parent = tree.create_node3d("Parent");
    let child = tree.create_node3d("Child");
    tree.add_child(tree.root(), parent).unwrap();
    tree.add_child(parent, child).unwrap();
    tree.set_transform(
        parent,
        Transform3D::from_trs(
            Vec3::new(3.0, 0.0, 0.0),
            Quat::from_rotation_z(0.7),
            Vec3::splat(0.5),
        ),
    )
    .unwrap();

    let target = Vec3::new(-2.0, 4.0, 1.0);
    tree.set_global_position(child, target).unwrap();
    assert!(approx(tree.global_position(child).unwrap(), target));

    tree.global_translate(child, Vec3::Y).unwrap();
    assert!(approx(
        tree.global_position(child).unwrap(),
        target + Vec3::Y
    ));
}

#[test]
fn reparent_keeps_global_transform_when_asked() {
    let mut tree = SceneTree::new();
    let a = tree.create_node3d("A");
    let b = tree.create_node3d("B");
    let item = tree.create_node3d("Item");
    tree.add_child(tree.root(), a).unwrap();
    tree.add_child(tree.root(), b).unwrap();
    tree.add_child(a, item).unwrap();
    tree.set_position(a, Vec3::new(1.0, 0.0, 0.0)).unwrap();
    tree.set_position(b, Vec3::new(0.0, 0.0, 7.0)).unwrap();
    tree.set_position(item, Vec3::Y).unwrap();

    tree.reparent(item, b, true).unwrap();
    assert_eq!(tree.parent(item), Some(b));
    assert!(approx(
        tree.global_position(item).unwrap(),
        Vec3::new(1.0, 1.0, 0.0)
    ));

    // Without keeping the global transform the local one is preserved: (1,1,-7) under A at (1,0,0).
    assert!(approx(
        tree.position(item).unwrap(),
        Vec3::new(1.0, 1.0, -7.0)
    ));
    tree.reparent(item, a, false).unwrap();
    assert!(approx(
        tree.global_position(item).unwrap(),
        Vec3::new(2.0, 1.0, -7.0)
    ));
}

#[test]
fn translate_is_object_local() {
    let mut tree = SceneTree::new();
    let n = tree.create_node3d("N");
    tree.set_rotation(n, Quat::from_rotation_y(FRAC_PI_2))
        .unwrap();
    tree.translate(n, Vec3::NEG_Z).unwrap();
    assert!(approx(tree.position(n).unwrap(), Vec3::NEG_X));
}

#[test]
fn look_at_faces_target() {
    let mut tree = SceneTree::new();
    let parent = tree.create_node3d("Parent");
    let eye = tree.create_node3d("Eye");
    tree.add_child(tree.root(), parent).unwrap();
    tree.add_child(parent, eye).unwrap();
    tree.set_rotation(parent, Quat::from_rotation_y(1.0))
        .unwrap();
    tree.set_position(eye, Vec3::new(0.0, 0.0, 5.0)).unwrap();

    let target = Vec3::new(3.0, 1.0, -2.0);
    tree.look_at(eye, target, Vec3::Y).unwrap();
    let global: Affine3A = tree.global_transform(eye).unwrap();
    let forward = global.transform_vector3(Vec3::NEG_Z).normalize();
    let expected = (target - Vec3::from(global.translation)).normalize();
    assert!(approx(forward, expected));
}

// ---------------------------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------------------------

type Log = Rc<RefCell<Vec<String>>>;

struct Recorder {
    name: &'static str,
    log: Log,
}

impl Recorder {
    fn push(&self, event: &str) {
        self.log
            .borrow_mut()
            .push(format!("{}:{}", self.name, event));
    }
}

impl NodeScript for Recorder {
    fn enter_tree(&mut self, _: &mut SceneTree, _: NodeHandle) {
        self.push("enter");
    }
    fn ready(&mut self, _: &mut SceneTree, _: NodeHandle) {
        self.push("ready");
    }
    fn process(&mut self, _: &mut SceneTree, _: NodeHandle, _: f32) {
        self.push("process");
    }
    fn physics_process(&mut self, _: &mut SceneTree, _: NodeHandle, _: f32) {
        self.push("physics");
    }
    fn exit_tree(&mut self, _: &mut SceneTree, _: NodeHandle) {
        self.push("exit");
    }
}

/// Builds `Parent { A { A1 }, B }` (detached) with recorders on every node.
fn scripted_subtree(tree: &mut SceneTree, log: &Log) -> [NodeHandle; 4] {
    let nodes = ["Parent", "A", "A1", "B"].map(|name| {
        let n = tree.create_node3d(name);
        tree.set_script(
            n,
            Recorder {
                name,
                log: log.clone(),
            },
        )
        .unwrap();
        n
    });
    let [parent, a, a1, b] = nodes;
    tree.add_child(parent, a).unwrap();
    tree.add_child(a, a1).unwrap();
    tree.add_child(parent, b).unwrap();
    nodes
}

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.borrow_mut())
}

#[test]
fn lifecycle_order_matches_godot() {
    let log = Log::default();
    let mut tree = SceneTree::new();
    let [parent, ..] = scripted_subtree(&mut tree, &log);
    assert!(take(&log).is_empty(), "nothing fires while detached");

    tree.add_child(tree.root(), parent).unwrap();
    assert_eq!(
        take(&log),
        [
            "Parent:enter",
            "A:enter",
            "A1:enter",
            "B:enter", // pre-order
            "A1:ready",
            "A:ready",
            "B:ready",
            "Parent:ready", // post-order
        ]
    );

    tree.physics_process(1.0 / 60.0);
    tree.process(1.0 / 60.0);
    assert_eq!(
        take(&log),
        [
            "Parent:physics",
            "A:physics",
            "A1:physics",
            "B:physics",
            "Parent:process",
            "A:process",
            "A1:process",
            "B:process",
        ]
    );

    tree.remove_child(tree.root(), parent).unwrap();
    assert_eq!(take(&log), ["B:exit", "A1:exit", "A:exit", "Parent:exit"]);

    tree.process(0.016);
    assert!(take(&log).is_empty(), "detached nodes are not processed");
}

#[test]
fn ready_runs_once_across_reparenting() {
    let log = Log::default();
    let mut tree = SceneTree::new();
    let holder = tree.create_node("Holder");
    tree.add_child(tree.root(), holder).unwrap();
    let [parent, ..] = scripted_subtree(&mut tree, &log);
    tree.add_child(tree.root(), parent).unwrap();
    take(&log);

    tree.reparent(parent, holder, true).unwrap();
    let events = take(&log);
    assert!(events.iter().all(|e| !e.ends_with(":ready")), "{events:?}");
    assert!(events.contains(&"Parent:exit".to_owned()));
    assert!(events.contains(&"Parent:enter".to_owned()));
    assert!(tree.is_node_ready(parent));
}

#[test]
fn process_flags_are_respected() {
    let log = Log::default();
    let mut tree = SceneTree::new();
    let [parent, a, a1, b] = scripted_subtree(&mut tree, &log);
    tree.add_child(tree.root(), parent).unwrap();
    take(&log);

    tree.set_process(a, false).unwrap();
    tree.set_physics_process(b, false).unwrap();
    tree.physics_process(0.016);
    tree.process(0.016);
    let events = take(&log);
    assert!(!events.contains(&"A:process".to_owned()));
    assert!(events.contains(&"A:physics".to_owned()));
    assert!(!events.contains(&"B:physics".to_owned()));
    assert!(
        events.contains(&"A1:process".to_owned()),
        "flags are per node, not inherited"
    );
    let _ = a1;
}

#[test]
fn queue_free_is_deferred_to_end_of_frame() {
    let log = Log::default();
    let mut tree = SceneTree::new();
    let [parent, a, a1, b] = scripted_subtree(&mut tree, &log);
    tree.add_child(tree.root(), parent).unwrap();
    take(&log);
    let before = tree.node_count();

    tree.queue_free(a).unwrap();
    tree.queue_free(a).unwrap();
    assert!(tree.is_valid(a), "still alive until the frame ends");
    assert!(tree.is_queued_for_deletion(a));

    tree.process(0.016);
    let events = take(&log);
    assert!(
        events.contains(&"A:process".to_owned()),
        "queued nodes still process this frame"
    );
    assert!(
        events.ends_with(&["A1:exit".to_owned(), "A:exit".to_owned()]),
        "{events:?}"
    );

    assert!(!tree.is_valid(a));
    assert!(!tree.is_valid(a1), "the whole subtree is freed");
    assert!(tree.is_valid(b));
    assert_eq!(tree.children(parent), vec![b]);
    assert_eq!(tree.node_count(), before - 2);

    // Stale handles fail cleanly, even once the slot is reused.
    let reused = tree.create_node("Reused");
    assert!(tree.get_node(a, ".").is_none());
    assert_eq!(tree.add_child(a, reused), Err(SceneError::InvalidNode(a)));
}

#[test]
fn scripts_can_build_the_tree_from_ready() {
    struct Spawner;
    impl NodeScript for Spawner {
        fn ready(&mut self, tree: &mut SceneTree, node: NodeHandle) {
            let child = tree.create_node3d("Spawned");
            tree.add_child(node, child).unwrap();
            tree.set_position(child, Vec3::X).unwrap();
        }
        fn process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {
            tree.translate(node, Vec3::Y * delta).unwrap();
            if let Some(child) = tree.get_node(node, "Spawned") {
                tree.queue_free(child).unwrap();
            }
        }
    }

    let mut tree = SceneTree::new();
    let spawner = tree.create_node3d("Spawner");
    tree.set_script(spawner, Spawner).unwrap();
    tree.add_child(tree.root(), spawner).unwrap();

    let child = tree
        .get_node(tree.root(), "Spawner/Spawned")
        .expect("spawned in ready");
    assert!(tree.is_inside_tree(child));
    assert!(tree.is_node_ready(child));

    tree.process(0.5);
    assert!(!tree.is_valid(child));
    assert!(approx(
        tree.position(spawner).unwrap(),
        Vec3::new(0.0, 0.5, 0.0)
    ));
    assert!(
        tree.has_script(spawner),
        "script is restored after its callback"
    );
}

#[test]
fn visibility_is_inherited() {
    let mut tree = SceneTree::new();
    let parent = tree.create_node3d("Parent");
    let child = tree.create_node3d("Child");
    tree.add_child(tree.root(), parent).unwrap();
    tree.add_child(parent, child).unwrap();
    tree.set_visible(parent, false).unwrap();
    assert!(tree.is_visible(child));
    assert!(!tree.is_visible_in_tree(child));
}
