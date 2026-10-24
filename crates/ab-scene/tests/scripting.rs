//! Phase 10 scripting features: typed script access, signals, groups, timers, hot-reload
//! script swapping.

use ab_scene::{
    serde_script_state, NodeHandle, NodeScript, SceneTree, ScriptRegistry, Signal, SignalQueue,
    Timer,
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;

// ---------------------------------------------------------------------------------------------
// Typed script access
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Health {
    hp: i32,
}

impl Health {
    fn damage(&mut self, tree: &mut SceneTree, me: NodeHandle, amount: i32) {
        self.hp -= amount;
        if self.hp <= 0 {
            tree.queue_free(me).unwrap();
        }
    }
}

impl NodeScript for Health {}

/// Hits its sibling "Enemy" once in `ready`.
struct Attacker;

impl NodeScript for Attacker {
    fn ready(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        let enemy = tree.get_node(node, "../Enemy").unwrap();
        assert_eq!(tree.script::<Health>(enemy).unwrap().hp, 10);
        // A script can't be borrowed as the wrong type.
        assert!(tree.script::<Attacker>(enemy).is_none());
        // ...nor as itself while it runs.
        assert!(tree.script::<Attacker>(node).is_none());
        tree.call_script::<Health, _>(enemy, |health, tree| health.damage(tree, enemy, 4));
    }
}

#[test]
fn scripts_find_and_call_each_other() {
    let mut tree = SceneTree::new();
    let enemy = tree.create_node("Enemy");
    tree.set_script(enemy, Health { hp: 10 }).unwrap();
    tree.add_child(tree.root(), enemy).unwrap();
    let attacker = tree.create_node("Attacker");
    tree.set_script(attacker, Attacker).unwrap();
    tree.add_child(tree.root(), attacker).unwrap();

    assert_eq!(tree.script::<Health>(enemy).unwrap().hp, 6);
    tree.script_mut::<Health>(enemy).unwrap().hp = 3;
    let r = tree.call_script::<Health, _>(enemy, |health, tree| {
        health.damage(tree, enemy, 5);
        health.hp
    });
    assert_eq!(r, Some(-2));
    tree.process(0.0);
    assert!(!tree.is_valid(enemy), "freed itself");
    assert!(tree.call_script::<Health, _>(enemy, |_, _| ()).is_none());

    // Components by path.
    let label = tree.create_node3d("Label");
    tree.add_child(attacker, label).unwrap();
    assert!(tree
        .get_component::<ab_scene::Transform>(tree.root(), "Attacker/Label")
        .is_some());
    assert!(tree
        .get_component::<ab_scene::Transform>(tree.root(), "Attacker/Missing")
        .is_none());
}

// ---------------------------------------------------------------------------------------------
// Signals
// ---------------------------------------------------------------------------------------------

/// Counts "hit" signals from its sibling "Source" (connected in `ready`).
#[derive(Default)]
struct Listener {
    hits: Vec<i32>,
}

impl NodeScript for Listener {
    fn ready(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        let source = tree.get_node(node, "../Source").unwrap();
        tree.connect(source, "hit", node);
    }

    fn on_signal(&mut self, _tree: &mut SceneTree, _node: NodeHandle, signal: &Signal) {
        if signal.is("hit") {
            self.hits.push(*signal.arg::<i32>().unwrap());
        }
    }
}

#[test]
fn signals_reach_connected_scripts_and_closures() {
    let mut tree = SceneTree::new();
    let source = tree.create_node("Source");
    tree.add_child(tree.root(), source).unwrap();
    let listener = tree.create_node("Listener");
    tree.set_script(listener, Listener::default()).unwrap();
    tree.add_child(tree.root(), listener).unwrap();
    assert!(tree.is_connected(source, "hit", listener));

    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let closure = tree.connect_fn(source, "hit", move |_, signal| {
        sink.borrow_mut().push(*signal.arg::<i32>().unwrap())
    });
    let once_log = log.clone();
    tree.connect_fn_once(source, "hit", move |_, _| once_log.borrow_mut().push(-1));

    tree.emit(Signal::new(source, "hit").with_arg(5));
    tree.emit(Signal::new(source, "hit").with_arg(7));
    tree.emit(Signal::new(source, "other"));
    assert_eq!(tree.script::<Listener>(listener).unwrap().hits, [5, 7]);
    assert_eq!(*log.borrow(), [5, -1, 7], "closure twice, one-shot once");

    assert!(tree.disconnect(closure));
    assert!(!tree.disconnect(closure));
    tree.emit(Signal::new(source, "hit").with_arg(9));
    assert_eq!(log.borrow().len(), 3);
    assert_eq!(tree.script::<Listener>(listener).unwrap().hits, [5, 7, 9]);

    // Signals queued by systems arrive on the next process.
    tree.world()
        .resource_mut::<SignalQueue>()
        .unwrap()
        .push(Signal::new(source, "hit").with_arg(11));
    assert_eq!(tree.script::<Listener>(listener).unwrap().hits.len(), 3);
    tree.process(0.0);
    assert_eq!(
        tree.script::<Listener>(listener).unwrap().hits,
        [5, 7, 9, 11]
    );

    // Freeing the target drops its connection.
    tree.queue_free(listener).unwrap();
    tree.process(0.0);
    assert!(!tree.is_connected(source, "hit", listener));
    assert_eq!(tree.connection_count(), 0);
    tree.emit(Signal::new(source, "hit").with_arg(1));
}

#[test]
fn handlers_can_emit_disconnect_and_recurse_safely() {
    let mut tree = SceneTree::new();
    let a = tree.create_node("A");
    tree.add_child(tree.root(), a).unwrap();
    let b = tree.create_node("B");
    tree.add_child(tree.root(), b).unwrap();
    let count = Rc::new(RefCell::new(0));

    // Re-emitting its own signal does not re-enter the running handler.
    let c = count.clone();
    tree.connect_fn(a, "self", move |tree, signal| {
        *c.borrow_mut() += 1;
        tree.emit(Signal::new(signal.source, "self"));
    });
    tree.emit(Signal::new(a, "self"));
    assert_eq!(*count.borrow(), 1);

    // Ping-pong between two nodes stops as soon as it would re-enter a running handler.
    *count.borrow_mut() = 0;
    for (from, to) in [(a, b), (b, a)] {
        let c = count.clone();
        tree.connect_fn(from, "ping", move |tree, _| {
            *c.borrow_mut() += 1;
            tree.emit(Signal::new(to, "ping"));
        });
    }
    tree.emit(Signal::new(a, "ping"));
    assert_eq!(*count.borrow(), 2);

    // A long chain of different handlers is cut off at depth 32.
    *count.borrow_mut() = 0;
    let chain: Vec<NodeHandle> = (0..40)
        .map(|i| {
            let n = tree.create_node(&format!("Link{i}"));
            tree.add_child(tree.root(), n).unwrap();
            n
        })
        .collect();
    for i in 0..chain.len() - 1 {
        let (c, next) = (count.clone(), chain[i + 1]);
        tree.connect_fn(chain[i], "relay", move |tree, _| {
            *c.borrow_mut() += 1;
            tree.emit(Signal::new(next, "relay"));
        });
    }
    tree.emit(Signal::new(chain[0], "relay"));
    assert_eq!(*count.borrow(), 32);

    // A handler disconnecting another one before it runs.
    let ran = Rc::new(RefCell::new(false));
    let second = Rc::new(RefCell::new(None));
    let second_id = second.clone();
    tree.connect_fn(b, "go", move |tree, _| {
        tree.disconnect(second_id.borrow().unwrap());
    });
    let flag = ran.clone();
    *second.borrow_mut() = Some(tree.connect_fn(b, "go", move |_, _| *flag.borrow_mut() = true));
    tree.emit(Signal::new(b, "go"));
    assert!(!*ran.borrow());
}

// ---------------------------------------------------------------------------------------------
// Groups and timers
// ---------------------------------------------------------------------------------------------

#[test]
fn groups() {
    let mut tree = SceneTree::new();
    let enemies: Vec<NodeHandle> = (0..3)
        .map(|i| {
            let n = tree.create_node(&format!("Enemy{i}"));
            tree.add_to_group(n, "enemies").unwrap();
            tree.add_to_group(n, "enemies").unwrap();
            n
        })
        .collect();
    assert!(
        tree.nodes_in_group("enemies").is_empty(),
        "not in the tree yet"
    );
    for &e in &enemies {
        tree.add_child(tree.root(), e).unwrap();
    }
    assert_eq!(tree.nodes_in_group("enemies"), enemies);
    assert!(tree.is_in_group(enemies[1], "enemies"));
    assert!(tree.remove_from_group(enemies[1], "enemies"));
    assert!(!tree.remove_from_group(enemies[1], "enemies"));
    assert_eq!(tree.nodes_in_group("enemies"), [enemies[0], enemies[2]]);
    assert!(tree.nodes_in_group("nobody").is_empty());
}

#[test]
fn timers_fire_and_repeat() {
    let mut tree = SceneTree::new();
    let ticks = Rc::new(RefCell::new(0));
    let repeating = tree.create_timer_node("Tick", Timer::new(0.5).autostart());
    tree.add_child(tree.root(), repeating).unwrap();
    let t = ticks.clone();
    tree.connect_fn(repeating, Timer::TIMEOUT, move |_, _| *t.borrow_mut() += 1);

    let fired = Rc::new(RefCell::new(false));
    let once = tree.create_timer(0.3);
    let f = fired.clone();
    tree.connect_fn(once, Timer::TIMEOUT, move |_, _| *f.borrow_mut() = true);

    for _ in 0..10 {
        tree.process(0.1);
    }
    assert_eq!(*ticks.borrow(), 2, "every 0.5 s for 1 s");
    assert!(*fired.borrow());
    assert!(
        !tree.is_valid(once),
        "one-shot scene timers free themselves"
    );

    let timer = tree.component::<Timer>(repeating).unwrap().clone();
    assert!(!timer.is_stopped());
    assert!(timer.time_left() > 0.0 && timer.time_left() <= 0.5);
    tree.component_mut::<Timer>(repeating).unwrap().paused = true;
    for _ in 0..10 {
        tree.process(0.1);
    }
    assert_eq!(*ticks.borrow(), 2, "paused");
}

/// Stops its autostart timer child in `ready`.
#[derive(Default)]
struct StopsTimer;

impl NodeScript for StopsTimer {
    fn ready(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        let timer = tree.get_node(node, "Timer").unwrap();
        tree.component_mut::<Timer>(timer).unwrap().stop();
    }
}

#[test]
fn a_timer_stopped_in_ready_stays_stopped() {
    let mut tree = SceneTree::new();
    let owner = tree.create_node3d("Owner");
    tree.set_script(owner, StopsTimer).unwrap();
    let timer = tree.create_timer_node("Timer", Timer::new(0.1).autostart());
    tree.add_child(owner, timer).unwrap();
    let fired = Rc::new(RefCell::new(0));
    let f = fired.clone();
    tree.connect_fn(timer, Timer::TIMEOUT, move |_, _| *f.borrow_mut() += 1);
    tree.add_child(tree.root(), owner).unwrap();
    for _ in 0..5 {
        tree.process(0.1);
    }
    assert_eq!(
        *fired.borrow(),
        0,
        "autostart must not undo the script's stop()"
    );
    assert!(tree.component::<Timer>(timer).unwrap().is_stopped());
}

#[test]
fn timers_ignore_nan_waits_and_deltas() {
    let mut tree = SceneTree::new();
    let timer = tree.create_timer_node("Odd", Timer::new(f32::NAN).autostart());
    tree.add_child(tree.root(), timer).unwrap();
    let fired = Rc::new(RefCell::new(0));
    let f = fired.clone();
    tree.connect_fn(timer, Timer::TIMEOUT, move |_, _| *f.borrow_mut() += 1);
    for _ in 0..5 {
        tree.process(0.1);
    }
    assert!(*fired.borrow() <= 1, "a NaN wait must not fire every frame");

    let ok = tree.create_timer_node("Ok", Timer::new(0.5).autostart());
    tree.add_child(tree.root(), ok).unwrap();
    tree.process(0.1);
    tree.process(f32::NAN);
    assert!(tree.component::<Timer>(ok).unwrap().time_left().is_finite());
}

// ---------------------------------------------------------------------------------------------
// Hot reload swapping
// ---------------------------------------------------------------------------------------------

/// "Version 1" of a script.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct CounterV1 {
    count: u32,
    #[serde(skip)]
    cache: Vec<u8>,
}

impl NodeScript for CounterV1 {
    serde_script_state!();

    fn ready(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        let source = tree.get_node(node, "../Button").unwrap();
        tree.connect(source, "pressed", node);
    }

    fn process(&mut self, _: &mut SceneTree, _: NodeHandle, _: f32) {
        self.count += 1;
        self.cache.push(0);
    }
}

/// "Version 2": same saved field, a new one, different behaviour.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct CounterV2 {
    count: u32,
    presses: u32,
    reloaded: bool,
}

impl NodeScript for CounterV2 {
    serde_script_state!();

    fn process(&mut self, _: &mut SceneTree, _: NodeHandle, _: f32) {
        self.count += 100;
    }

    fn on_signal(&mut self, _: &mut SceneTree, _: NodeHandle, signal: &Signal) {
        if signal.is("pressed") {
            self.presses += 1;
        }
    }

    fn reloaded(&mut self, _: &mut SceneTree, _: NodeHandle) {
        self.reloaded = true;
    }
}

/// Without state support: restarts from defaults.
#[derive(Default)]
struct Plain {
    value: u32,
}

impl NodeScript for Plain {}

#[test]
fn reload_swaps_named_scripts_and_keeps_state() {
    let mut tree = SceneTree::new();
    tree.world()
        .resource_mut::<ScriptRegistry>()
        .unwrap()
        .register::<CounterV1>("Counter")
        .register::<Plain>("Plain");

    let button = tree.create_node("Button");
    tree.add_child(tree.root(), button).unwrap();
    let counter = tree.create_node("Counter");
    tree.set_script_named(counter, "Counter").unwrap();
    tree.add_child(tree.root(), counter).unwrap();
    assert_eq!(tree.script_name(counter).as_deref(), Some("Counter"));
    let plain = tree.create_node("Plain");
    tree.set_script_named(plain, "Plain").unwrap();
    tree.script_mut::<Plain>(plain).unwrap().value = 5;
    tree.add_child(tree.root(), plain).unwrap();
    // Attached directly: never swapped.
    let fixed = tree.create_node("Fixed");
    tree.set_script(fixed, CounterV1::default()).unwrap();
    tree.add_child(tree.root(), fixed).unwrap();
    assert!(tree.set_script_named(fixed, "Missing").is_err());

    for _ in 0..3 {
        tree.process(0.016);
    }
    assert_eq!(tree.script::<CounterV1>(counter).unwrap().count, 3);

    // "Rebuild": the registry now hands out version 2 under the same names.
    tree.world()
        .resource_mut::<ScriptRegistry>()
        .unwrap()
        .register::<CounterV2>("Counter");
    assert_eq!(tree.reload_scripts(), 2);

    let v2 = tree.script::<CounterV2>(counter).unwrap();
    assert_eq!(v2.count, 3, "state carried over");
    assert!(v2.reloaded);
    drop(v2);
    assert_eq!(
        tree.script::<Plain>(plain).unwrap().value,
        0,
        "no state: fresh"
    );
    assert!(tree.script::<CounterV1>(fixed).is_some());

    // New behaviour, and the old connection now reaches the new script.
    tree.process(0.016);
    tree.emit(Signal::new(button, "pressed"));
    let v2 = tree.script::<CounterV2>(counter).unwrap();
    assert_eq!(v2.count, 103);
    assert_eq!(v2.presses, 1);
}

#[test]
fn the_fallback_stands_in_for_unknown_names() {
    fn stand_in() -> Box<dyn NodeScript> {
        Box::new(Plain { value: 7 })
    }
    let mut tree = SceneTree::new();
    let node = tree.create_node("Game");
    assert!(tree.set_script_named(node, "Game").is_err());
    tree.world()
        .resource_mut::<ScriptRegistry>()
        .unwrap()
        .set_fallback(Some(std::sync::Arc::new(stand_in)));
    tree.set_script_named(node, "Game").unwrap();
    assert_eq!(tree.script_name(node).as_deref(), Some("Game"));
    assert_eq!(tree.script::<Plain>(node).unwrap().value, 7);
    let registry = tree.world().resource::<ScriptRegistry>().unwrap();
    assert!(!registry.contains("Game"), "only real entries count");
}

#[test]
fn script_time_is_measured_per_type() {
    let mut tree = SceneTree::new();
    for i in 0..3 {
        let node = tree.create_node(&format!("P{i}"));
        tree.set_script(node, Plain::default()).unwrap();
        tree.add_child(tree.root(), node).unwrap();
    }
    tree.process(0.016);
    tree.physics_process(0.016);
    let timings = tree
        .world()
        .resource_mut::<ab_scene::ScriptProfile>()
        .unwrap()
        .take();
    assert_eq!(timings.len(), 1);
    assert_eq!(timings[0].name, "Plain");
    assert_eq!(
        timings[0].calls, 6,
        "3 scripts x (process + physics_process)"
    );

    tree.world()
        .resource_mut::<ab_scene::ScriptProfile>()
        .unwrap()
        .enabled = false;
    tree.process(0.016);
    let timings = tree
        .world()
        .resource_mut::<ab_scene::ScriptProfile>()
        .unwrap()
        .take();
    assert!(timings.is_empty(), "off: nothing measured");
}

#[test]
fn node_handles_serialize_for_state() {
    let mut tree = SceneTree::new();
    let node = tree.create_node("N");
    let json = serde_json::to_string(&node).unwrap();
    let back: NodeHandle = serde_json::from_str(&json).unwrap();
    assert_eq!(back, node);
}

// ---------------------------------------------------------------------------------------------
// Panics
// ---------------------------------------------------------------------------------------------

/// Panics on its third frame.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Fragile {
    frames: u32,
}

impl NodeScript for Fragile {
    serde_script_state!();

    fn process(&mut self, _: &mut SceneTree, _: NodeHandle, _: f32) {
        self.frames += 1;
        assert!(self.frames < 3, "bug on frame {}", self.frames);
    }
}

/// The "fixed" version.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Fixed {
    frames: u32,
}

impl NodeScript for Fixed {
    serde_script_state!();

    fn process(&mut self, _: &mut SceneTree, _: NodeHandle, _: f32) {
        self.frames += 1;
    }
}

#[test]
fn a_panicking_script_is_disabled_not_fatal() {
    let mut tree = SceneTree::new();
    tree.world()
        .resource_mut::<ScriptRegistry>()
        .unwrap()
        .register::<Fragile>("Fragile");
    let fragile = tree.create_node("Fragile");
    tree.set_script_named(fragile, "Fragile").unwrap();
    tree.add_child(tree.root(), fragile).unwrap();
    let healthy = tree.create_node("Healthy");
    tree.set_script(healthy, Fixed::default()).unwrap();
    tree.add_child(tree.root(), healthy).unwrap();

    for _ in 0..5 {
        tree.process(0.016);
    }
    assert!(tree.has_failed_script(fragile));
    assert!(!tree.has_failed_script(healthy));
    assert_eq!(
        tree.script::<Fragile>(fragile).unwrap().frames,
        3,
        "stopped at the panic"
    );
    assert_eq!(
        tree.script::<Fixed>(healthy).unwrap().frames,
        5,
        "others keep running"
    );

    // Hot reload with the fix brings it back, with its state.
    tree.world()
        .resource_mut::<ScriptRegistry>()
        .unwrap()
        .register::<Fixed>("Fragile");
    assert_eq!(tree.reload_scripts(), 1);
    assert!(!tree.has_failed_script(fragile));
    tree.process(0.016);
    assert_eq!(tree.script::<Fixed>(fragile).unwrap().frames, 4);
}

#[test]
fn a_panicking_signal_handler_is_disconnected() {
    let mut tree = SceneTree::new();
    let source = tree.create_node("Source");
    tree.add_child(tree.root(), source).unwrap();
    let calls = Rc::new(RefCell::new(0));
    let c = calls.clone();
    tree.connect_fn(source, "boom", move |_, _| {
        *c.borrow_mut() += 1;
        panic!("handler bug");
    });
    tree.emit(Signal::new(source, "boom"));
    tree.emit(Signal::new(source, "boom"));
    assert_eq!(*calls.borrow(), 1);
    assert_eq!(tree.connection_count(), 0);
}
