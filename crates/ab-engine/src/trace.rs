//! Object and damage tracing: who hit whom, for how much, whether it landed, who killed whom;
//! which nodes were spawned and freed; where watched objects went and what they emitted.
//!
//! * The [`Tracer`] resource records it all (on by default outside shipping builds). Damage
//!   follows the signal convention of the script templates: a `damage` / `heal` signal (an
//!   amount) on the target, answered by `damaged` / `healed` (the amount taken) and `died`.
//!   Deal damage with [`Tracing::deal_damage`] to have the attacker recorded too; a `damage`
//!   with no answer is shown as *ignored* (invulnerable, already dead, nothing listening).
//! * [`TraceOverlay`] (F9, see [`EngineConfig::debug_trace_key`](crate::EngineConfig)): the
//!   recent damage, per-object totals and the watched objects on screen, plus hit lines,
//!   trails and floating damage numbers in the world.
//! * The editor's Debug panel shows the same records as tables while playing.
//!
//! ```no_run
//! use ab_engine::prelude::*;
//! use ab_engine::trace::Tracing;
//!
//! fn shoot(tree: &mut SceneTree, gun: NodeHandle, enemy: NodeHandle) {
//!     tree.deal_damage(Some(gun), enemy, 25.0);
//!     tree.trace_note(enemy, "staggered");
//!     tree.trace_watch(enemy, true); // its trail and every signal it emits
//! }
//! ```

use ab_core::{Color, Time, Vec2, Vec3};
use ab_rendering::{Camera3D, CameraOverride, CameraView, DebugDraw};
use ab_scene::{NodeHandle, SceneTree, Signal, SignalTap};
use ab_ui::{
    CanvasLayer, Control, HAlign, Label, LayoutPreset, MouseFilter, Panel, StyleBox, TextStyle,
    UiNodes, UiState, VAlign,
};
use std::collections::{HashMap, VecDeque};

/// The damage convention's signal names.
pub const DAMAGE: &str = "damage";
pub const HEAL: &str = "heal";
pub const DAMAGED: &str = "damaged";
pub const HEALED: &str = "healed";
pub const DIED: &str = "died";

/// Damage events kept.
pub const MAX_DAMAGE_EVENTS: usize = 1000;
/// Object events (spawns, frees, signals, notes) kept.
pub const MAX_OBJECT_EVENTS: usize = 2000;
/// Trail points kept per watched object.
pub const MAX_TRAIL: usize = 300;
/// Seconds hit lines and damage numbers stay in the world.
pub const HIT_SHOW_SECONDS: f32 = 1.2;
/// Canvas layers from this one up are the engine's debug views: not traced.
pub const DEBUG_LAYER: i32 = 1000;
const OVERLAY_LAYER: i32 = 1002;
const POPUPS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DamageKind {
    Damage,
    Heal,
    Death,
}

/// What became of a damage or heal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Sent this frame; the answer may still come.
    Pending,
    /// Answered with `damaged` / `healed` (or a death).
    Applied,
    /// Nobody answered: invulnerable, already dead, or no health script.
    Ignored,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DamageEvent {
    pub id: u64,
    /// Game time in seconds.
    pub time: f32,
    pub frame: u64,
    pub kind: DamageKind,
    /// The attacker, when dealt with [`Tracing::deal_damage`].
    pub source: Option<NodeHandle>,
    /// `?` when unknown.
    pub source_name: String,
    pub target: NodeHandle,
    pub target_name: String,
    /// Asked for.
    pub amount: f32,
    /// Actually taken (from the `damaged` / `healed` answer).
    pub applied: f32,
    pub outcome: Outcome,
    /// The target's position then.
    pub position: Option<Vec3>,
    /// The target's health after it (current, max), when reported.
    pub health: Option<(f32, f32)>,
}

impl DamageEvent {
    /// One line: `12.30  Player -> Enemy  -25 (hp 50/100)`.
    pub fn describe(&self) -> String {
        let arrow = match self.kind {
            DamageKind::Death => "killed",
            _ => "->",
        };
        let amount = match (self.kind, self.outcome) {
            (DamageKind::Death, _) => "KO".to_owned(),
            (_, Outcome::Ignored) => format!("{} IGNORED", number(self.amount)),
            (_, Outcome::Pending) => format!("{} ?", number(self.amount)),
            (DamageKind::Damage, Outcome::Applied) => format!("-{}", number(self.applied)),
            (DamageKind::Heal, Outcome::Applied) => format!("+{}", number(self.applied)),
        };
        let health = self
            .health
            .map(|(current, max)| format!("  (hp {}/{})", number(current), number(max)))
            .unwrap_or_default();
        format!(
            "{:7.2}  {} {arrow} {}  {amount}{health}",
            self.time, self.source_name, self.target_name
        )
    }
}

fn number(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round())
    } else {
        format!("{v:.1}")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObjectEventKind {
    Spawned,
    Freed,
    /// A signal it emitted (watched objects).
    Signal {
        name: &'static str,
        value: Option<f64>,
        text: Option<String>,
    },
    /// Written by game code ([`Tracing::trace_note`]).
    Note(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectEvent {
    pub time: f32,
    pub frame: u64,
    pub node: NodeHandle,
    pub name: String,
    pub class: &'static str,
    pub kind: ObjectEventKind,
}

impl ObjectEvent {
    pub fn describe(&self) -> String {
        let what = match &self.kind {
            ObjectEventKind::Spawned => format!("spawned ({})", self.class),
            ObjectEventKind::Freed => "freed".to_owned(),
            ObjectEventKind::Signal { name, value, text } => {
                let arg = value
                    .map(|v| format!(" {}", number(v as f32)))
                    .or_else(|| text.as_ref().map(|t| format!(" \"{t}\"")))
                    .unwrap_or_default();
                format!("signal {name}{arg}")
            }
            ObjectEventKind::Note(text) => format!("note: {text}"),
        };
        format!("{:7.2}  {}  {what}", self.time, self.name)
    }
}

/// A watched object: where it is, how fast it goes, where it has been.
#[derive(Debug, Clone, PartialEq)]
pub struct Watch {
    pub node: NodeHandle,
    pub name: String,
    pub position: Option<Vec3>,
    pub velocity: Vec3,
    pub trail: VecDeque<Vec3>,
    /// False once the node is freed (the watch stays to show where it ended).
    pub alive: bool,
}

/// Damage per object, for balancing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CombatTotals {
    pub node: Option<NodeHandle>,
    pub name: String,
    pub dealt: f32,
    pub taken: f32,
    pub healed: f32,
    /// Damage it dealt that landed.
    pub hits: u32,
    /// Damage sent to it that it ignored.
    pub ignored: u32,
    pub kills: u32,
    pub deaths: u32,
}

#[derive(Debug, Clone, Copy)]
struct Attribution {
    target: NodeHandle,
    kind: DamageKind,
    source: NodeHandle,
    amount: f32,
    frame: u64,
}

/// ECS resource: the traces. See the [module docs](self).
#[derive(Debug)]
pub struct Tracer {
    /// Record anything at all.
    pub enabled: bool,
    /// Record nodes being spawned and freed (a walk over the tree each frame).
    pub lifecycle: bool,
    /// Record every signal of every node, not only of watched ones (busy).
    pub all_signals: bool,
    /// Draw hit lines and trails in the world (also while the overlay is shown).
    pub draw: bool,
    /// Floating damage numbers (also while the overlay is shown).
    pub popups: bool,
    damage: VecDeque<DamageEvent>,
    objects: VecDeque<ObjectEvent>,
    watches: Vec<Watch>,
    totals: HashMap<NodeHandle, CombatTotals>,
    health: HashMap<NodeHandle, (f32, f32)>,
    attributions: Vec<Attribution>,
    known: Option<HashMap<NodeHandle, (String, &'static str)>>,
    next_id: u64,
    frame: u64,
    time: f32,
}

impl Default for Tracer {
    fn default() -> Self {
        Self {
            enabled: !crate::SHIPPING,
            lifecycle: true,
            all_signals: false,
            draw: false,
            popups: false,
            damage: VecDeque::new(),
            objects: VecDeque::new(),
            watches: Vec::new(),
            totals: HashMap::new(),
            health: HashMap::new(),
            attributions: Vec::new(),
            known: None,
            next_id: 0,
            frame: 0,
            time: 0.0,
        }
    }
}

impl Tracer {
    /// Damage, heals and deaths, oldest first.
    pub fn damage(&self) -> impl DoubleEndedIterator<Item = &DamageEvent> + ExactSizeIterator {
        self.damage.iter()
    }

    /// Spawns, frees, signals of watched objects and notes, oldest first.
    pub fn objects(&self) -> impl DoubleEndedIterator<Item = &ObjectEvent> + ExactSizeIterator {
        self.objects.iter()
    }

    pub fn watches(&self) -> &[Watch] {
        &self.watches
    }

    pub fn is_watched(&self, node: NodeHandle) -> bool {
        self.watches.iter().any(|w| w.node == node)
    }

    pub fn unwatch(&mut self, node: NodeHandle) {
        self.watches.retain(|w| w.node != node);
    }

    /// Totals per object, the most damage dealt first.
    pub fn totals(&self) -> Vec<CombatTotals> {
        let mut totals: Vec<CombatTotals> = self.totals.values().cloned().collect();
        totals.sort_by(|a, b| {
            b.dealt
                .total_cmp(&a.dealt)
                .then(b.taken.total_cmp(&a.taken))
                .then(a.name.cmp(&b.name))
        });
        totals
    }

    /// Last reported health of `node` (current, max).
    pub fn health(&self, node: NodeHandle) -> Option<(f32, f32)> {
        self.health.get(&node).copied()
    }

    /// Forgets every record (watches stay, their trails restart).
    pub fn clear(&mut self) {
        self.damage.clear();
        self.objects.clear();
        self.totals.clear();
        self.attributions.clear();
        for watch in &mut self.watches {
            watch.trail.clear();
        }
    }

    /// Forgets everything, watches too, and takes the scene as it is next frame as the
    /// starting point (a new play session).
    pub fn restart(&mut self) {
        self.clear();
        self.watches.clear();
        self.health.clear();
        self.known = None;
    }

    /// The damage log as CSV (for a spreadsheet).
    pub fn damage_csv(&self) -> String {
        let mut out = String::from("time,kind,source,target,amount,applied,outcome,hp,max_hp\n");
        for e in &self.damage {
            let (hp, max) = e.health.map_or((String::new(), String::new()), |(c, m)| {
                (c.to_string(), m.to_string())
            });
            out.push_str(&format!(
                "{:.3},{:?},{},{},{},{},{:?},{hp},{max}\n",
                e.time,
                e.kind,
                csv(&e.source_name),
                csv(&e.target_name),
                e.amount,
                e.applied,
                e.outcome
            ));
        }
        out
    }

    /// The overlay's text.
    pub fn report(&self) -> String {
        let mut out = String::from("TRACE (F9)\n");
        if !self.enabled {
            out.push_str("recording is off\n");
            return out;
        }
        if !self.watches.is_empty() {
            out.push_str("\nWATCHED\n");
            for w in &self.watches {
                let position = w.position.map_or("-".to_owned(), |p| {
                    format!("({:.1}, {:.1}, {:.1})", p.x, p.y, p.z)
                });
                let health = self
                    .health(w.node)
                    .map(|(c, m)| format!("  hp {}/{}", number(c), number(m)))
                    .unwrap_or_default();
                let state = if w.alive { "" } else { "  [freed]" };
                out.push_str(&format!(
                    " {}  {position}  speed {:.1}{health}{state}\n",
                    w.name,
                    w.velocity.length()
                ));
                let events = self
                    .objects
                    .iter()
                    .rev()
                    .filter(|e| e.node == w.node)
                    .take(3)
                    .collect::<Vec<_>>();
                for e in events.into_iter().rev() {
                    out.push_str(&format!("   {}\n", e.describe()));
                }
            }
        }
        out.push_str("\nDAMAGE\n");
        if self.damage.is_empty() {
            out.push_str(" none yet\n");
        }
        for e in self.damage.iter().rev().take(10) {
            out.push_str(&format!(" {}\n", e.describe()));
        }
        let totals = self.totals();
        if !totals.is_empty() {
            out.push_str("\nTOTALS        dealt  taken  kills\n");
            for t in totals.iter().take(6) {
                out.push_str(&format!(
                    " {:<12} {:>5}  {:>5}  {:>5}\n",
                    shorten(&t.name, 12),
                    number(t.dealt),
                    number(t.taken),
                    t.kills
                ));
            }
        }
        let spawned = self
            .objects
            .iter()
            .filter(|e| e.time > self.time - 1.0)
            .fold((0, 0), |(s, f), e| match e.kind {
                ObjectEventKind::Spawned => (s + 1, f),
                ObjectEventKind::Freed => (s, f + 1),
                _ => (s, f),
            });
        if self.lifecycle {
            let live = self.known.as_ref().map_or(0, HashMap::len);
            out.push_str(&format!(
                "\nNODES {live}  +{} -{} in the last second\n",
                spawned.0, spawned.1
            ));
        }
        out
    }

    fn push_damage(&mut self, mut event: DamageEvent) {
        event.id = self.next_id;
        self.next_id += 1;
        self.damage.push_back(event);
        while self.damage.len() > MAX_DAMAGE_EVENTS {
            self.damage.pop_front();
        }
    }

    fn push_object(&mut self, event: ObjectEvent) {
        self.objects.push_back(event);
        while self.objects.len() > MAX_OBJECT_EVENTS {
            self.objects.pop_front();
        }
    }

    fn totals_of(&mut self, node: NodeHandle, name: &str) -> &mut CombatTotals {
        let entry = self.totals.entry(node).or_default();
        if entry.name.is_empty() {
            entry.name = name.to_owned();
            entry.node = Some(node);
        }
        entry
    }
}

fn csv(text: &str) -> String {
    if text.contains([',', '"', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut s: String = text.chars().take(max - 1).collect();
        s.push('~');
        s
    }
}

/// Tracing on the [`SceneTree`] (`use ab_engine::trace::Tracing;`).
pub trait Tracing {
    /// Sends `damage` (`amount`) to `target`, recording `source` as the attacker.
    fn deal_damage(&mut self, source: Option<NodeHandle>, target: NodeHandle, amount: f32);
    /// Sends `heal` (`amount`) to `target`, recording `source` as the healer.
    fn heal(&mut self, source: Option<NodeHandle>, target: NodeHandle, amount: f32);
    /// Adds a line to `node`'s history ("state: chasing", "reloading").
    fn trace_note(&mut self, node: NodeHandle, text: impl Into<String>);
    /// Starts or stops following `node`: its trail, speed and every signal it emits.
    fn trace_watch(&mut self, node: NodeHandle, watch: bool);
    /// Health scripts report their health here (shown with damage and watched objects).
    fn report_health(&mut self, node: NodeHandle, current: f32, max: f32);
}

fn send(
    tree: &mut SceneTree,
    kind: DamageKind,
    source: Option<NodeHandle>,
    target: NodeHandle,
    amount: f32,
) {
    if let Some(source) = source {
        if let Some(mut tracer) = tree.world().resource_mut::<Tracer>() {
            if tracer.enabled {
                let frame = tracer.frame;
                tracer.attributions.push(Attribution {
                    target,
                    kind,
                    source,
                    amount,
                    frame,
                });
            }
        }
    }
    let name = if kind == DamageKind::Heal {
        HEAL
    } else {
        DAMAGE
    };
    tree.emit(Signal::new(target, name).with_arg(amount));
}

impl Tracing for SceneTree {
    fn deal_damage(&mut self, source: Option<NodeHandle>, target: NodeHandle, amount: f32) {
        send(self, DamageKind::Damage, source, target, amount);
    }

    fn heal(&mut self, source: Option<NodeHandle>, target: NodeHandle, amount: f32) {
        send(self, DamageKind::Heal, source, target, amount);
    }

    fn trace_note(&mut self, node: NodeHandle, text: impl Into<String>) {
        let name = self.name(node).map(|n| n.to_owned()).unwrap_or_default();
        let class = self.class_name(node).unwrap_or("Node");
        if let Some(mut tracer) = self.world().resource_mut::<Tracer>() {
            if tracer.enabled {
                let (time, frame) = (tracer.time, tracer.frame);
                tracer.push_object(ObjectEvent {
                    time,
                    frame,
                    node,
                    name,
                    class,
                    kind: ObjectEventKind::Note(text.into()),
                });
            }
        }
    }

    fn trace_watch(&mut self, node: NodeHandle, watch: bool) {
        let name = self.name(node).map(|n| n.to_owned()).unwrap_or_default();
        let position = self.global_position(node);
        let Some(mut tracer) = self.world().resource_mut::<Tracer>() else {
            return;
        };
        if !watch {
            tracer.unwatch(node);
        } else if !tracer.is_watched(node) && self.is_valid(node) {
            tracer.watches.push(Watch {
                node,
                name,
                position,
                velocity: Vec3::ZERO,
                trail: VecDeque::new(),
                alive: true,
            });
        }
    }

    fn report_health(&mut self, node: NodeHandle, current: f32, max: f32) {
        if let Some(mut tracer) = self.world().resource_mut::<Tracer>() {
            tracer.health.insert(node, (current, max));
        }
    }
}

fn node_name(tree: &SceneTree, node: NodeHandle) -> String {
    tree.name(node)
        .map(|n| n.to_owned())
        .unwrap_or_else(|| "?".to_owned())
}

/// Turns the frame's signals into records, follows spawns, frees and watched objects, and
/// draws. The engine calls it every frame after the scripts' `process`.
pub fn update(tree: &mut SceneTree, delta: f32) {
    let overlay = tree
        .world()
        .resource::<TraceOverlay>()
        .is_some_and(|o| o.visible);
    let enabled = tree.world().resource::<Tracer>().is_some_and(|t| t.enabled);
    {
        let world = tree.world_mut();
        let tap = world.resource_or_insert_with(SignalTap::default);
        tap.enabled = enabled;
    }
    if enabled {
        record(tree, delta);
    } else if let Some(mut tap) = tree.world().resource_mut::<SignalTap>() {
        tap.take();
    }
    let (draw, popups) = tree
        .world()
        .resource::<Tracer>()
        .map_or((false, false), |t| {
            (
                t.enabled && (t.draw || overlay),
                t.enabled && (t.popups || overlay),
            )
        });
    if draw {
        draw_world(tree);
    }
    update_overlay(tree, overlay, popups);
}

fn record(tree: &mut SceneTree, delta: f32) {
    let time = tree
        .world()
        .resource::<Time>()
        .map_or(0.0, |t| t.elapsed as f32);
    let tapped = tree
        .world()
        .resource_mut::<SignalTap>()
        .map(|mut t| t.take())
        .unwrap_or_default();
    let Some(mut tracer) = tree.world().resource_mut::<Tracer>() else {
        return;
    };
    tracer.frame += 1;
    tracer.time = time;
    let frame = tracer.frame;

    for signal in tapped {
        let target = signal.source;
        let watched = tracer.all_signals || tracer.is_watched(target);
        if watched {
            let event = ObjectEvent {
                time,
                frame,
                node: target,
                name: node_name(tree, target),
                class: tree.class_name(target).unwrap_or("Node"),
                kind: ObjectEventKind::Signal {
                    name: signal.name,
                    value: signal.value,
                    text: signal.text.clone(),
                },
            };
            tracer.push_object(event);
        }
        let amount = signal.value.unwrap_or(0.0) as f32;
        match signal.name {
            DAMAGE | HEAL => {
                let kind = if signal.name == DAMAGE {
                    DamageKind::Damage
                } else {
                    DamageKind::Heal
                };
                let attribution = tracer
                    .attributions
                    .iter()
                    .position(|a| {
                        a.target == target && a.kind == kind && (a.amount - amount).abs() < 1e-3
                    })
                    .map(|i| tracer.attributions.remove(i));
                let source = attribution.map(|a| a.source);
                tracer.push_damage(DamageEvent {
                    id: 0,
                    time,
                    frame,
                    kind,
                    source,
                    source_name: source.map_or("?".to_owned(), |s| node_name(tree, s)),
                    target,
                    target_name: node_name(tree, target),
                    amount,
                    applied: 0.0,
                    outcome: Outcome::Pending,
                    position: tree.global_position(target),
                    health: None,
                });
            }
            DAMAGED | HEALED => {
                let kind = if signal.name == DAMAGED {
                    DamageKind::Damage
                } else {
                    DamageKind::Heal
                };
                let health = tracer.health(target);
                let pending = tracer.damage.iter().rposition(|e| {
                    e.target == target && e.kind == kind && e.outcome == Outcome::Pending
                });
                let (source, source_name) = match pending {
                    Some(i) => {
                        let e = &mut tracer.damage[i];
                        e.outcome = Outcome::Applied;
                        e.applied = amount;
                        e.health = health;
                        (e.source, e.source_name.clone())
                    }
                    None => {
                        // Taken without a `damage` signal (a script's own hazard).
                        tracer.push_damage(DamageEvent {
                            id: 0,
                            time,
                            frame,
                            kind,
                            source: None,
                            source_name: "?".to_owned(),
                            target,
                            target_name: node_name(tree, target),
                            amount,
                            applied: amount,
                            outcome: Outcome::Applied,
                            position: tree.global_position(target),
                            health,
                        });
                        (None, "?".to_owned())
                    }
                };
                let target_name = node_name(tree, target);
                match kind {
                    DamageKind::Damage => {
                        tracer.totals_of(target, &target_name).taken += amount;
                        if let Some(source) = source {
                            let t = tracer.totals_of(source, &source_name);
                            t.dealt += amount;
                            t.hits += 1;
                        }
                    }
                    _ => tracer.totals_of(target, &target_name).healed += amount,
                }
            }
            DIED => {
                let killer = tracer
                    .damage
                    .iter()
                    .rev()
                    .find(|e| {
                        e.target == target
                            && e.kind == DamageKind::Damage
                            && e.outcome == Outcome::Applied
                    })
                    .and_then(|e| e.source.map(|s| (s, e.source_name.clone())));
                let target_name = node_name(tree, target);
                tracer.totals_of(target, &target_name).deaths += 1;
                if let Some((killer, name)) = &killer {
                    tracer.totals_of(*killer, name).kills += 1;
                }
                let health = tracer.health(target);
                tracer.push_damage(DamageEvent {
                    id: 0,
                    time,
                    frame,
                    kind: DamageKind::Death,
                    source: killer.as_ref().map(|k| k.0),
                    source_name: killer.map_or("?".to_owned(), |k| k.1),
                    target,
                    target_name,
                    amount: 0.0,
                    applied: 0.0,
                    outcome: Outcome::Applied,
                    position: tree.global_position(target),
                    health,
                });
            }
            _ => {}
        }
    }

    // Damage nobody answered by the next frame was ignored.
    let mut ignored: Vec<(NodeHandle, String)> = Vec::new();
    for e in tracer.damage.iter_mut().rev().take(256) {
        if e.outcome == Outcome::Pending && e.frame < frame {
            e.outcome = Outcome::Ignored;
            if e.kind == DamageKind::Damage {
                ignored.push((e.target, e.target_name.clone()));
            }
        }
    }
    for (node, name) in ignored {
        tracer.totals_of(node, &name).ignored += 1;
    }
    tracer.attributions.retain(|a| a.frame + 2 >= frame);

    if tracer.lifecycle {
        lifecycle(tree, &mut tracer, time, frame);
    } else {
        tracer.known = None;
    }

    // Watched objects.
    for watch in &mut tracer.watches {
        if !watch.alive {
            continue;
        }
        if !tree.is_valid(watch.node) {
            watch.alive = false;
            continue;
        }
        let Some(position) = tree.global_position(watch.node) else {
            continue;
        };
        if let Some(previous) = watch.position {
            if delta > 0.0 {
                watch.velocity = (position - previous) / delta;
            }
        }
        watch.position = Some(position);
        let moved = watch
            .trail
            .back()
            .is_none_or(|last| last.distance_squared(position) > 0.05 * 0.05);
        if moved {
            watch.trail.push_back(position);
            while watch.trail.len() > MAX_TRAIL {
                watch.trail.pop_front();
            }
        }
    }
}

/// The game's nodes: everything under the root but the engine's debug views.
fn game_nodes(tree: &SceneTree) -> Vec<NodeHandle> {
    let mut nodes = Vec::new();
    for child in tree.children(tree.root()) {
        let debug = tree
            .component::<CanvasLayer>(child)
            .is_some_and(|c| c.layer >= DEBUG_LAYER);
        if !debug {
            nodes.extend(tree.subtree(child));
        }
    }
    nodes
}

fn lifecycle(tree: &SceneTree, tracer: &mut Tracer, time: f32, frame: u64) {
    let nodes = game_nodes(tree);
    let Some(mut known) = tracer.known.take() else {
        // The first look: what is already there is the level, not news.
        tracer.known = Some(
            nodes
                .into_iter()
                .map(|n| {
                    (
                        n,
                        (node_name(tree, n), tree.class_name(n).unwrap_or("Node")),
                    )
                })
                .collect(),
        );
        return;
    };
    let mut seen: HashMap<NodeHandle, ()> = HashMap::with_capacity(nodes.len());
    for node in nodes {
        seen.insert(node, ());
        if let std::collections::hash_map::Entry::Vacant(slot) = known.entry(node) {
            let name = node_name(tree, node);
            let class = tree.class_name(node).unwrap_or("Node");
            slot.insert((name.clone(), class));
            tracer.push_object(ObjectEvent {
                time,
                frame,
                node,
                name,
                class,
                kind: ObjectEventKind::Spawned,
            });
        }
    }
    let gone: Vec<NodeHandle> = known
        .keys()
        .filter(|n| !seen.contains_key(n))
        .copied()
        .collect();
    for node in gone {
        let (name, class) = known.remove(&node).expect("listed above");
        tracer.health.remove(&node);
        tracer.push_object(ObjectEvent {
            time,
            frame,
            node,
            name,
            class,
            kind: ObjectEventKind::Freed,
        });
    }
    tracer.known = Some(known);
}

const TRAIL_COLOR: Color = Color::rgb(0.3, 0.85, 1.0);
const DAMAGE_COLOR: Color = Color::rgb(1.0, 0.25, 0.2);
const HEAL_COLOR: Color = Color::rgb(0.3, 1.0, 0.4);
const IGNORED_COLOR: Color = Color::rgb(0.6, 0.6, 0.65);
const DEATH_COLOR: Color = Color::rgb(1.0, 1.0, 1.0);

fn color_of(e: &DamageEvent) -> Color {
    match (e.kind, e.outcome) {
        (DamageKind::Death, _) => DEATH_COLOR,
        (_, Outcome::Ignored) => IGNORED_COLOR,
        (DamageKind::Heal, _) => HEAL_COLOR,
        (DamageKind::Damage, _) => DAMAGE_COLOR,
    }
}

fn draw_world(tree: &SceneTree) {
    let world = tree.world();
    let (Some(tracer), Some(mut debug)) = (
        world.resource::<Tracer>(),
        world.resource_mut::<DebugDraw>(),
    ) else {
        return;
    };
    let mut brush = debug.on_top();
    for watch in &tracer.watches {
        let trail: Vec<Vec3> = watch.trail.iter().copied().collect();
        brush.path(&trail, TRAIL_COLOR);
        if let Some(at) = watch.position.filter(|_| watch.alive) {
            brush.circle(at, Vec3::Y, 0.4, TRAIL_COLOR);
            if watch.velocity.length_squared() > 1e-4 {
                brush.arrow(at, at + watch.velocity * 0.25, TRAIL_COLOR);
            }
        }
    }
    for e in tracer.damage.iter().rev() {
        if tracer.time - e.time > HIT_SHOW_SECONDS {
            break;
        }
        let Some(at) = e.position.map(|p| p + Vec3::Y) else {
            continue;
        };
        let color = color_of(e);
        if e.kind == DamageKind::Death {
            brush.sphere(at, 0.5, color);
        } else {
            brush.cross(at, 0.35, color);
        }
        if let Some(from) = e.source.and_then(|s| tree.global_position(s)) {
            if e.kind != DamageKind::Death {
                brush.arrow(from + Vec3::Y, at, color);
            }
        }
    }
}

// =============================================================================================
// Overlay
// =============================================================================================

/// ECS resource: the on-screen trace view (F9).
#[derive(Debug, Clone, Default)]
pub struct TraceOverlay {
    pub visible: bool,
    nodes: Option<OverlayNodes>,
}

#[derive(Debug, Clone)]
struct OverlayNodes {
    layer: NodeHandle,
    panel: NodeHandle,
    text: NodeHandle,
    popups: Vec<NodeHandle>,
}

impl TraceOverlay {
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }
}

fn create_overlay(tree: &mut SceneTree) -> OverlayNodes {
    let layer = tree.create_canvas_layer("TraceOverlay", OVERLAY_LAYER);
    let panel = tree.create_panel(
        "Panel",
        Control::new()
            .with_preset(LayoutPreset::RightWide, Vec2::new(560.0, 0.0), 16.0)
            .with_mouse_filter(MouseFilter::Ignore),
        Panel {
            style: StyleBox::flat(Color::rgba(0.0, 0.0, 0.0, 0.72))
                .with_corner_radius(6.0)
                .with_border(1.0, Color::rgba(1.0, 0.45, 0.35, 0.5)),
        },
    );
    let text = tree.create_label(
        "Text",
        Control::new().with_preset(LayoutPreset::FullRect, Vec2::ZERO, 12.0),
        Label::new("")
            .with_style(TextStyle::new(15.0).with_color(Color::from_srgb8(255, 214, 200, 255))),
    );
    tree.add_child(tree.root(), layer)
        .expect("fresh node under the root");
    tree.add_child(layer, panel).expect("fresh node");
    tree.add_child(panel, text).expect("fresh node");
    let popups = (0..POPUPS)
        .map(|i| {
            let label = tree.create_label(
                &format!("Popup{i}"),
                Control::new(),
                Label::new("")
                    .with_style(TextStyle::new(28.0).with_outline(3.0, Color::BLACK))
                    .with_align(HAlign::Center, VAlign::Center),
            );
            tree.add_child(layer, label).expect("fresh node");
            let _ = tree.set_visible(label, false);
            label
        })
        .collect();
    OverlayNodes {
        layer,
        panel,
        text,
        popups,
    }
}

/// The camera the frame is seen through (the editor's, else the scene's current one).
fn camera_view(tree: &SceneTree, aspect: f32) -> Option<CameraView> {
    let forced = tree
        .world()
        .resource::<CameraOverride>()
        .map(|o| (o.camera, o.global));
    let (camera, global) = forced.or_else(|| {
        let mut fallback = None;
        for node in tree.subtree(tree.root()) {
            let Some(camera) = tree.component::<Camera3D>(node).map(|c| *c) else {
                continue;
            };
            let Some(global) = tree.global_transform(node) else {
                continue;
            };
            if camera.current {
                return Some((camera, global));
            }
            fallback.get_or_insert((camera, global));
        }
        fallback
    })?;
    Some(CameraView::new(
        &global,
        camera.fov_y_degrees.clamp(1.0, 179.0).to_radians(),
        aspect,
        camera.near,
        camera.far,
    ))
}

fn update_overlay(tree: &mut SceneTree, visible: bool, popups: bool) {
    let nodes = tree
        .world()
        .resource::<TraceOverlay>()
        .and_then(|o| o.nodes.clone())
        .filter(|n| tree.is_valid(n.layer) && tree.is_valid(n.text));
    let nodes = match nodes {
        None if !visible && !popups => return,
        Some(nodes) => nodes,
        None => create_overlay(tree),
    };
    if let Some(mut overlay) = tree.world().resource_mut::<TraceOverlay>() {
        overlay.nodes = Some(nodes.clone());
    }
    let _ = tree.set_visible(nodes.layer, visible || popups);
    let _ = tree.set_visible(nodes.panel, visible);
    if visible {
        let report = tree
            .world()
            .resource::<Tracer>()
            .map_or_else(|| "TRACE (F9)\nno tracer".to_owned(), |t| t.report());
        if let Some(mut label) = tree.component_mut::<Label>(nodes.text) {
            label.text = report;
        }
    }

    // Damage numbers rising from what was hit.
    let mut shown: Vec<(Vec3, String, Color, f32)> = Vec::new();
    if popups {
        if let Some(tracer) = tree.world().resource::<Tracer>() {
            for e in tracer.damage.iter().rev() {
                let age = tracer.time - e.time;
                if age > HIT_SHOW_SECONDS || shown.len() >= POPUPS {
                    break;
                }
                let Some(at) = e.position else {
                    continue;
                };
                let text = match (e.kind, e.outcome) {
                    (DamageKind::Death, _) => "KO".to_owned(),
                    (_, Outcome::Ignored) => "BLOCK".to_owned(),
                    (_, Outcome::Pending) => continue,
                    (DamageKind::Damage, _) => format!("-{}", number(e.applied)),
                    (DamageKind::Heal, _) => format!("+{}", number(e.applied)),
                };
                let rise = Vec3::Y * (1.6 + age * 0.9);
                shown.push((at + rise, text, color_of(e), age / HIT_SHOW_SECONDS));
            }
        }
    }
    let (viewport, scale) = tree
        .world()
        .resource::<UiState>()
        .map_or((Vec2::ZERO, 0.0), |s| (s.viewport(), s.scale()));
    let view = (scale > 0.0 && viewport.y > 0.0)
        .then(|| camera_view(tree, viewport.x / viewport.y))
        .flatten();
    for (i, &label) in nodes.popups.iter().enumerate() {
        let placed = shown.get(i).zip(view.as_ref()).and_then(|(p, view)| {
            let clip = view.view_projection * p.0.extend(1.0);
            if clip.w <= 0.0 {
                return None;
            }
            let ndc = clip.truncate() / clip.w;
            let units = viewport / scale;
            let center = Vec2::new((ndc.x * 0.5 + 0.5) * units.x, (0.5 - ndc.y * 0.5) * units.y);
            Some((p, center))
        });
        let _ = tree.set_visible(label, placed.is_some());
        let Some(((_, text, color, fade), center)) = placed else {
            continue;
        };
        if let Some(mut control) = tree.component_mut::<Control>(label) {
            control.offsets = [
                center.x - 80.0,
                center.y - 20.0,
                center.x + 80.0,
                center.y + 20.0,
            ];
            control.modulate = Color::rgba(1.0, 1.0, 1.0, (1.0 - fade * fade).clamp(0.0, 1.0));
        }
        if let Some(mut l) = tree.component_mut::<Label>(label) {
            l.text.clone_from(text);
            l.style.color = *color;
        }
    }
}
