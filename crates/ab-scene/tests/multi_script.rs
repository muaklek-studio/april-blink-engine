//! Several scripts on one node: order, typed access between them, signals, adding and
//! removing while running, hot reload, panics.

use ab_scene::{NodeHandle, NodeScript, SceneTree, ScriptRegistry, Signal};
use std::cell::RefCell;
use std::rc::Rc;

type Log = Rc<RefCell<Vec<String>>>;

/// Records its callbacks as "<tag>:<callback>".
struct Recorder {
    tag: &'static str,
    log: Log,
}

impl Recorder {
    fn note(&self, what: &str) {
        self.log.borrow_mut().push(format!("{}:{what}", self.tag));
    }
}

impl NodeScript for Recorder {
    fn enter_tree(&mut self, _: &mut SceneTree, _: NodeHandle) {
        self.note("enter");
    }
    fn ready(&mut self, _: &mut SceneTree, _: NodeHandle) {
        self.note("ready");
    }
    fn process(&mut self, _: &mut SceneTree, _: NodeHandle, _: f32) {
        self.note("process");
    }
    fn physics_process(&mut self, _: &mut SceneTree, _: NodeHandle, _: f32) {
        self.note("physics");
    }
    fn on_signal(&mut self, _: &mut SceneTree, _: NodeHandle, signal: &Signal) {
        self.note(signal.name);
    }
    fn exit_tree(&mut self, _: &mut SceneTree, _: NodeHandle) {
        self.note("exit");
    }
}

fn recorder(tag: &'static str, log: &Log) -> Recorder {
    Recorder {
        tag,
        log: log.clone(),
    }
}

#[test]
fn every_callback_runs_each_script_in_order() {
    let log = Log::default();
    let mut tree = SceneTree::new();
    let node = tree.create_node("Player");
    tree.add_script(node, recorder("a", &log)).unwrap();
    tree.add_script(node, recorder("b", &log)).unwrap();
    assert_eq!(tree.script_count(node), 2);
    tree.add_child(tree.root(), node).unwrap();
    tree.process(0.016);
    tree.physics_process(0.016);
    let source = tree.create_node("Button");
    tree.add_child(tree.root(), source).unwrap();
    tree.connect(source, "pressed", node);
    tree.emit(Signal::new(source, "pressed"));
    tree.queue_free(node).unwrap();
    tree.process(0.016);
    assert_eq!(
        *log.borrow(),
        [
            "a:enter",
            "b:enter",
            "a:ready",
            "b:ready",
            "a:process",
            "b:process",
            "a:physics",
            "b:physics",
            "a:pressed",
            "b:pressed",
            // queue_free: the frame still processes, then the node leaves.
            "a:process",
            "b:process",
            "a:exit",
            "b:exit",
        ]
    );
}

#[derive(Default)]
struct Health {
    hp: i32,
}

impl NodeScript for Health {
    fn ready(&mut self, _: &mut SceneTree, _: NodeHandle) {
        self.hp = 10;
    }
}

/// Hurts the `Health` on its own node every frame: scripts of one node reach each other.
#[derive(Default)]
struct Poison;

impl NodeScript for Poison {
    fn process(&mut self, tree: &mut SceneTree, node: NodeHandle, _: f32) {
        // Not itself (it is running), but its neighbour.
        assert!(tree.script::<Poison>(node).is_none());
        tree.script_mut::<Health>(node).unwrap().hp -= 1;
    }
}

#[test]
fn scripts_on_one_node_find_each_other_by_type() {
    let mut tree = SceneTree::new();
    let node = tree.create_node("Enemy");
    tree.add_script(node, Health::default()).unwrap();
    tree.add_script(node, Poison).unwrap();
    tree.add_child(tree.root(), node).unwrap();
    for _ in 0..3 {
        tree.process(0.016);
    }
    assert_eq!(tree.script::<Health>(node).unwrap().hp, 7);
    assert!(tree.script::<Poison>(node).is_some());
    // call_script reaches the second script too.
    let hp = tree.call_script::<Health, _>(node, |health, tree| {
        assert!(
            tree.script::<Poison>(node).is_some(),
            "the other stays attached"
        );
        health.hp
    });
    assert_eq!(hp, Some(7));
}

/// Adds a recorder next to itself on its first frame, then removes itself.
struct Spawner {
    log: Log,
    frames: u32,
}

impl NodeScript for Spawner {
    fn process(&mut self, tree: &mut SceneTree, node: NodeHandle, _: f32) {
        self.frames += 1;
        self.log
            .borrow_mut()
            .push(format!("spawner:{}", self.frames));
        if self.frames == 1 {
            tree.add_script(node, recorder("late", &self.log)).unwrap();
        }
        if self.frames == 2 {
            // Removing every script, itself included, while it runs.
            tree.remove_script(node);
            tree.add_script(node, recorder("after", &self.log)).unwrap();
        }
    }
}

#[test]
fn scripts_added_or_removed_while_running() {
    let log = Log::default();
    let mut tree = SceneTree::new();
    let node = tree.create_node("Node");
    tree.add_child(tree.root(), node).unwrap();
    tree.add_script(
        node,
        Spawner {
            log: log.clone(),
            frames: 0,
        },
    )
    .unwrap();
    for _ in 0..3 {
        tree.process(0.016);
    }
    // Added scripts start next frame (attached to a node in the tree, no enter/ready, as
    // with set_script). "late" was removed in frame 2 before its turn, so it never ran; the
    // spawner removed itself and "after" runs from frame 3.
    assert_eq!(*log.borrow(), ["spawner:1", "spawner:2", "after:process"]);
    assert_eq!(tree.script_count(node), 1);
}

#[derive(Default)]
struct Fragile {
    frames: u32,
}

impl NodeScript for Fragile {
    fn process(&mut self, _: &mut SceneTree, _: NodeHandle, _: f32) {
        self.frames += 1;
        assert!(self.frames < 2, "fragile breaks on its second frame");
    }
}

#[derive(Default)]
struct Steady {
    frames: u32,
}

impl NodeScript for Steady {
    fn process(&mut self, _: &mut SceneTree, _: NodeHandle, _: f32) {
        self.frames += 1;
    }
}

#[test]
fn named_scripts_reload_and_remove_one_by_one() {
    let mut tree = SceneTree::new();
    tree.world()
        .resource_mut::<ScriptRegistry>()
        .unwrap()
        .register::<Fragile>("Fragile")
        .register::<Steady>("Steady");
    let node = tree.create_node("Node");
    tree.add_child(tree.root(), node).unwrap();
    tree.add_script_named(node, "Fragile").unwrap();
    tree.add_script_named(node, "Steady").unwrap();
    assert!(tree.add_script_named(node, "Missing").is_err());
    assert_eq!(tree.script_names(node), ["Fragile", "Steady"]);
    assert_eq!(
        tree.script_name(node).as_deref(),
        Some("Fragile"),
        "the first"
    );

    for _ in 0..4 {
        tree.process(0.016);
    }
    assert!(tree.has_failed_script(node), "one of them panicked");
    assert_eq!(
        tree.script::<Steady>(node).unwrap().frames,
        4,
        "the other kept running"
    );

    // A rebuild that fixes Fragile: both named scripts are swapped.
    tree.world()
        .resource_mut::<ScriptRegistry>()
        .unwrap()
        .register::<Steady>("Fragile");
    assert_eq!(tree.reload_scripts(), 2);
    assert!(!tree.has_failed_script(node));
    assert_eq!(tree.script_names(node), ["Fragile", "Steady"]);

    assert!(tree.remove_script_named(node, "Fragile"));
    assert!(!tree.remove_script_named(node, "Fragile"));
    assert_eq!(tree.script_names(node), ["Steady"]);
    assert!(tree.remove_script_named(node, "Steady"));
    assert!(!tree.has_script(node), "no scripts left: no slot");

    // set_script_named replaces everything.
    tree.add_script_named(node, "Steady").unwrap();
    tree.add_script_named(node, "Steady").unwrap();
    tree.set_script_named(node, "Fragile").unwrap();
    assert_eq!(tree.script_names(node), ["Fragile"]);
}
