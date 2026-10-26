//! Animation state machines: which clip a character plays, and when it changes.
//!
//! A state machine file (`.abstate`, TOML) lists parameters, states and transitions:
//!
//! ```toml
//! entry = "Idle"
//! fade = 0.2
//!
//! [[parameter]]
//! name = "speed"            # set by the game every frame
//! [[parameter]]
//! name = "grounded"
//! kind = "bool"
//! default = 1.0
//! [[parameter]]
//! name = "jump"
//! kind = "trigger"          # set once, used by the transition it fires
//!
//! [[state]]
//! name = "Idle"
//! clip = "Idle"
//! [[state]]
//! name = "Move"
//! blend = { parameter = "speed", clips = [["Walk", 1.5], ["Run", 4.2]] }
//! [[state]]
//! name = "Jump"
//! clip = "Jump"
//! looping = false
//!
//! [[transition]]
//! from = "Idle"
//! to = "Move"
//! when = [{ parameter = "speed", op = ">", value = 0.1 }]
//! [[transition]]
//! from = "*"                # from any state
//! to = "Jump"
//! when = [{ parameter = "jump" }]
//! [[transition]]
//! from = "Jump"
//! to = "Idle"
//! exit_time = 0.9           # once 90% of the clip has played
//! ```
//!
//! A node with `state_machine = "anims/hero.abstate"` drives the `AnimationPlayer` of its
//! model (the first in its subtree). Transitions are checked in order each frame; the first
//! whose conditions all hold is taken, crossfading over its `fade` (or the file's). A blend
//! state mixes the two clips around the parameter's value, as locomotion does. Each change
//! emits the node's `state_changed` signal with the new state's name; the last changes are
//! kept for debugging ([`StateMachine::history`]).

use ab_animation::AnimationPlayer;
use ab_scene::{NodeHandle, SceneTree};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// File extension of state machine files.
pub const STATE_MACHINE_EXTENSION: &str = "abstate";
/// The signal emitted on each change of state (argument: the new state's name).
pub const STATE_CHANGED: &str = "state_changed";
/// Changes kept in [`StateMachine::history`].
const HISTORY: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterKind {
    /// A number.
    #[default]
    Float,
    /// On (1) or off (0).
    Bool,
    /// Set once; used up by the transition it lets through.
    Trigger,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterDesc {
    pub name: String,
    #[serde(default)]
    pub kind: ParameterKind,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub default: f32,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

/// `blend = { parameter = "speed", clips = [["Walk", 1.5], ["Run", 4.2]] }`: clips placed
/// along a parameter, the two around its value mixed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlendDesc {
    pub parameter: String,
    pub clips: Vec<(String, f32)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateDesc {
    pub name: String,
    /// The clip it plays (or `blend`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend: Option<BlendDesc>,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub speed: f32,
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub looping: bool,
    /// Where the editor draws it.
    #[serde(default, skip_serializing_if = "is_origin")]
    pub position: [f32; 2],
}

fn one() -> f32 {
    1.0
}
fn is_one(v: &f32) -> bool {
    *v == 1.0
}
fn yes() -> bool {
    true
}
fn is_yes(v: &bool) -> bool {
    *v
}
fn is_origin(v: &[f32; 2]) -> bool {
    *v == [0.0, 0.0]
}

/// How a condition compares its parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Compare {
    #[serde(rename = ">")]
    Greater,
    #[serde(rename = "<")]
    Less,
    #[serde(rename = ">=")]
    AtLeast,
    #[serde(rename = "<=")]
    AtMost,
    #[serde(rename = "==")]
    Equal,
    #[serde(rename = "!=")]
    NotEqual,
}

impl Compare {
    pub const ALL: [Compare; 6] = [
        Compare::Greater,
        Compare::Less,
        Compare::AtLeast,
        Compare::AtMost,
        Compare::Equal,
        Compare::NotEqual,
    ];

    pub fn symbol(self) -> &'static str {
        match self {
            Compare::Greater => ">",
            Compare::Less => "<",
            Compare::AtLeast => ">=",
            Compare::AtMost => "<=",
            Compare::Equal => "==",
            Compare::NotEqual => "!=",
        }
    }

    fn holds(self, a: f32, b: f32) -> bool {
        match self {
            Compare::Greater => a > b,
            Compare::Less => a < b,
            Compare::AtLeast => a >= b,
            Compare::AtMost => a <= b,
            Compare::Equal => (a - b).abs() < 1e-4,
            Compare::NotEqual => (a - b).abs() >= 1e-4,
        }
    }
}

/// `{ parameter = "speed", op = ">", value = 0.1 }`. A trigger needs no `op`; a bool
/// without one is "is on" (`value = 0` for "is off").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub parameter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<Compare>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f32>,
}

/// The `from` of a transition that leaves any state.
pub const ANY_STATE: &str = "*";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionDesc {
    /// A state's name, or `"*"` for any state.
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<Condition>,
    /// 0..1: only once this much of the state's clip has played (a non-looping clip:
    /// usually near 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_time: Option<f32>,
    /// Crossfade seconds (default: the file's `fade`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade: Option<f32>,
}

/// A state machine file's contents (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StateMachineFile {
    /// The state it starts in (default: the first).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// Default crossfade seconds.
    pub fade: f32,
    #[serde(rename = "parameter", skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ParameterDesc>,
    #[serde(rename = "state", skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<StateDesc>,
    #[serde(rename = "transition", skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<TransitionDesc>,
}

impl Default for StateMachineFile {
    fn default() -> Self {
        Self {
            entry: None,
            fade: 0.2,
            parameters: Vec::new(),
            states: Vec::new(),
            transitions: Vec::new(),
        }
    }
}

impl StateMachineFile {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let file: StateMachineFile = toml::from_str(text).map_err(|e| e.to_string())?;
        file.check()?;
        Ok(file)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("state machines always serialize")
    }

    pub fn state(&self, name: &str) -> Option<usize> {
        self.states.iter().position(|s| s.name == name)
    }

    pub fn parameter(&self, name: &str) -> Option<usize> {
        self.parameters.iter().position(|p| p.name == name)
    }

    /// What does not hold together, named: unknown states and parameters, a state with
    /// no clip, names used twice.
    pub fn check(&self) -> Result<(), String> {
        for (i, s) in self.states.iter().enumerate() {
            if self.states[..i].iter().any(|o| o.name == s.name) {
                return Err(format!("two states are named `{}`", s.name));
            }
            if s.name == ANY_STATE || s.name.is_empty() {
                return Err(format!("`{}` cannot name a state", s.name));
            }
            match (&s.clip, &s.blend) {
                (None, None) => return Err(format!("state `{}` has no clip or blend", s.name)),
                (Some(_), Some(_)) => {
                    return Err(format!("state `{}` has both a clip and a blend", s.name))
                }
                (None, Some(b)) => {
                    if b.clips.is_empty() {
                        return Err(format!("state `{}`: the blend has no clips", s.name));
                    }
                    if self.parameter(&b.parameter).is_none() {
                        return Err(format!(
                            "state `{}`: no parameter `{}`",
                            s.name, b.parameter
                        ));
                    }
                }
                (Some(_), None) => {}
            }
        }
        for (i, p) in self.parameters.iter().enumerate() {
            if self.parameters[..i].iter().any(|o| o.name == p.name) {
                return Err(format!("two parameters are named `{}`", p.name));
            }
        }
        if let Some(entry) = &self.entry {
            if self.state(entry).is_none() {
                return Err(format!("entry: no state `{entry}`"));
            }
        }
        for t in &self.transitions {
            if t.from != ANY_STATE && self.state(&t.from).is_none() {
                return Err(format!("transition from `{}`: no such state", t.from));
            }
            if self.state(&t.to).is_none() {
                return Err(format!("transition to `{}`: no such state", t.to));
            }
            for c in &t.when {
                if self.parameter(&c.parameter).is_none() {
                    return Err(format!(
                        "transition {} → {}: no parameter `{}`",
                        t.from, t.to, c.parameter
                    ));
                }
            }
        }
        Ok(())
    }
}

/// A change of state, for debugging.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub from: Option<String>,
    pub to: String,
    /// Seconds since the machine started.
    pub at: f32,
}

/// A node's state machine and where it is (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct StateMachine {
    /// The file it was read from.
    pub file: String,
    pub desc: StateMachineFile,
    /// Each parameter's value, in the file's order.
    pub values: Vec<f32>,
    pub current: Option<usize>,
    /// Seconds in the current state.
    pub state_time: f32,
    /// Seconds since it started.
    pub time: f32,
    /// The last changes, oldest first.
    pub history: VecDeque<Change>,
    /// Off: the player is left alone.
    pub enabled: bool,
    player: Option<NodeHandle>,
    /// The clip the blend plays as its main one.
    blend_main: Option<String>,
}

impl StateMachine {
    pub fn new(file: impl Into<String>, desc: StateMachineFile) -> Self {
        Self {
            values: desc.parameters.iter().map(|p| p.default).collect(),
            file: file.into(),
            desc,
            current: None,
            state_time: 0.0,
            time: 0.0,
            history: VecDeque::new(),
            enabled: true,
            player: None,
            blend_main: None,
        }
    }

    /// Sets a parameter (`1.0` sets a trigger or a bool). Whether there is one by that name.
    pub fn set(&mut self, name: &str, value: f32) -> bool {
        match self.desc.parameter(name) {
            Some(i) => {
                self.values[i] = value;
                true
            }
            None => false,
        }
    }

    pub fn get(&self, name: &str) -> Option<f32> {
        self.desc.parameter(name).map(|i| self.values[i])
    }

    /// The current state's name.
    pub fn state_name(&self) -> Option<&str> {
        self.current.map(|i| self.desc.states[i].name.as_str())
    }

    fn holds(&self, c: &Condition) -> bool {
        let Some(i) = self.desc.parameter(&c.parameter) else {
            return false;
        };
        let value = self.values[i];
        match (self.desc.parameters[i].kind, c.op) {
            (ParameterKind::Trigger, _) => value > 0.5,
            (ParameterKind::Bool, None) => (value > 0.5) == (c.value.unwrap_or(1.0) > 0.5),
            (_, op) => op
                .unwrap_or(Compare::Greater)
                .holds(value, c.value.unwrap_or(0.0)),
        }
    }

    /// The transition to take now, given how far through its clip the state is (0..1).
    fn next(&self, progress: f32) -> Option<usize> {
        let current = self.current?;
        let name = &self.desc.states[current].name;
        self.desc.transitions.iter().position(|t| {
            let from_here = t.from == *name || (t.from == ANY_STATE && t.to != *name);
            from_here
                && t.exit_time.is_none_or(|e| progress >= e)
                && t.when.iter().all(|c| self.holds(c))
        })
    }
}

/// The clips a blend plays at `value`: `(main, other, weight of other)`.
fn blend_mix(clips: &[(String, f32)], value: f32) -> Option<(usize, Option<usize>, f32)> {
    debug_assert!(
        clips.windows(2).all(|w| w[0].1 <= w[1].1),
        "sorted by the caller"
    );
    let last = clips.len().checked_sub(1)?;
    if value <= clips[0].1 {
        return Some((0, None, 0.0));
    }
    let i = clips
        .windows(2)
        .position(|w| value < w[1].1)
        .unwrap_or(last);
    if i == last {
        return Some((last, None, 0.0));
    }
    let (low, high) = (clips[i].1, clips[i + 1].1);
    Some((
        i,
        Some(i + 1),
        ((value - low) / (high - low).max(1e-4)).clamp(0.0, 1.0),
    ))
}

/// Enters `state`: its clip crossfaded in.
fn enter(machine: &mut StateMachine, player: &mut AnimationPlayer, state: usize, fade: f32) {
    let s = machine.desc.states[state].clone();
    machine.current = Some(state);
    machine.state_time = 0.0;
    machine.blend_main = None;
    let clip = s.clip.clone().or_else(|| {
        s.blend
            .as_ref()
            .and_then(|b| b.clips.first())
            .map(|c| c.0.clone())
    });
    if let Some(handle) = clip.and_then(|c| player.clip_named(&c)) {
        // Entering the state it plays already (a jump again): from the start.
        if player.current == Some(handle) {
            player.time = 0.0;
            player.playing = true;
        } else {
            player.crossfade(handle, fade);
        }
    }
    player.looping = s.looping;
    player.speed = s.speed;
    player.blend = None;
}

/// Advances every node's state machine. The engine calls it each frame before the
/// animations are sampled.
pub fn update(tree: &mut SceneTree, delta: f32) {
    let nodes: Vec<NodeHandle> = tree
        .world()
        .query::<(ab_ecs::Entity, &StateMachine)>()
        .iter()
        .map(|(e, _)| NodeHandle::from_entity(e))
        .collect();
    for node in nodes {
        if !tree.is_valid(node) || !tree.is_inside_tree(node) {
            continue;
        }
        let Some(mut machine) = tree.component::<StateMachine>(node).map(|m| m.clone()) else {
            continue;
        };
        if !machine.enabled {
            continue;
        }
        let player = machine
            .player
            .filter(|&p| tree.is_valid(p) && tree.component::<AnimationPlayer>(p).is_some())
            .or_else(|| {
                tree.subtree(node)
                    .into_iter()
                    .find(|&n| tree.component::<AnimationPlayer>(n).is_some())
            });
        let Some(player_node) = player else {
            continue;
        };
        machine.player = Some(player_node);
        let changed = {
            let Some(mut player) = tree.component_mut::<AnimationPlayer>(player_node) else {
                continue;
            };
            let length = tree_clip_length(tree, &player);
            step(&mut machine, &mut player, length, delta)
        };
        let new_state = changed.then(|| machine.state_name().unwrap_or_default().to_owned());
        let _ = tree.insert_component(node, machine);
        if let Some(name) = new_state {
            tree.emit(ab_scene::Signal::new(node, STATE_CHANGED).with_arg(name));
        }
    }
}

fn tree_clip_length(tree: &SceneTree, player: &AnimationPlayer) -> f32 {
    let Some(library) = tree.world().resource::<ab_animation::AnimationLibrary>() else {
        return 0.0;
    };
    player
        .current
        .and_then(|c| library.get(c))
        .map_or(0.0, |c| c.duration)
}

/// One frame of `machine` on `player`; whether the state changed. `length`: seconds of the
/// clip playing.
pub fn step(
    machine: &mut StateMachine,
    player: &mut AnimationPlayer,
    length: f32,
    delta: f32,
) -> bool {
    machine.time += delta;
    machine.state_time += delta;
    let mut changed = false;
    if machine.current.is_none() {
        let entry = machine
            .desc
            .entry
            .as_deref()
            .and_then(|e| machine.desc.state(e))
            .or((!machine.desc.states.is_empty()).then_some(0));
        let Some(entry) = entry else {
            return false;
        };
        enter(machine, player, entry, 0.0);
        machine.history.push_back(Change {
            from: None,
            to: machine.desc.states[entry].name.clone(),
            at: machine.time,
        });
        changed = true;
    } else {
        let progress = if length > 0.0 {
            if player.looping {
                machine.state_time * player.speed.abs() / length
            } else {
                player.time / length
            }
        } else {
            1.0
        };
        if let Some(t) = machine.next(progress) {
            let transition = machine.desc.transitions[t].clone();
            // Triggers it used are spent.
            for c in &transition.when {
                if let Some(i) = machine.desc.parameter(&c.parameter) {
                    if machine.desc.parameters[i].kind == ParameterKind::Trigger {
                        machine.values[i] = 0.0;
                    }
                }
            }
            let from = machine.state_name().map(str::to_owned);
            let to = machine.desc.state(&transition.to).expect("checked on load");
            enter(
                machine,
                player,
                to,
                transition.fade.unwrap_or(machine.desc.fade).max(0.0),
            );
            machine.history.push_back(Change {
                from,
                to: transition.to,
                at: machine.time,
            });
            while machine.history.len() > HISTORY {
                machine.history.pop_front();
            }
            changed = true;
        }
    }
    // A blend state follows its parameter.
    let state = machine.current.map(|i| machine.desc.states[i].clone());
    if let Some(mut blend) = state.and_then(|s| s.blend) {
        // A file written by hand may list them in any order.
        blend.clips.sort_by(|a, b| a.1.total_cmp(&b.1));
        let value = machine.get(&blend.parameter).unwrap_or(0.0);
        if let Some((main, other, weight)) = blend_mix(&blend.clips, value) {
            let main_name = &blend.clips[main].0;
            if machine.blend_main.as_deref() != Some(main_name.as_str()) {
                if let Some(handle) = player.clip_named(main_name) {
                    player.crossfade(handle, machine.desc.fade.max(0.0) * 0.5);
                }
                machine.blend_main = Some(main_name.clone());
            }
            player.blend = other
                .and_then(|o| player.clip_named(&blend.clips[o].0))
                .map(|h| (h, weight));
        }
    }
    changed
}

/// Sets up `node`'s state machine from its file (scene loading).
pub(crate) fn configure(
    tree: &mut SceneTree,
    node: NodeHandle,
    file: Option<&str>,
) -> Result<(), String> {
    let Some(path) = file.filter(|p| !p.trim().is_empty()) else {
        return Ok(());
    };
    let bytes = tree
        .world()
        .resource::<ab_asset::AssetServer>()
        .ok_or("the AssetServer resource is missing")?
        .read(path)
        .map_err(|e| format!("state machine {path}: {e}"))?;
    let desc = StateMachineFile::from_toml(&String::from_utf8_lossy(&bytes))
        .map_err(|e| format!("state machine {path}: {e}"))?;
    tree.insert_component(node, StateMachine::new(path, desc))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_animation::{AnimationClip, AnimationLibrary};

    const HERO: &str = r#"
entry = "Idle"
fade = 0.1

[[parameter]]
name = "speed"
[[parameter]]
name = "grounded"
kind = "bool"
default = 1.0
[[parameter]]
name = "jump"
kind = "trigger"

[[state]]
name = "Idle"
clip = "Idle"
[[state]]
name = "Move"
blend = { parameter = "speed", clips = [["Walk", 1.5], ["Run", 4.0]] }
[[state]]
name = "Jump"
clip = "Jump"
looping = false

[[transition]]
from = "Idle"
to = "Move"
when = [{ parameter = "speed", op = ">", value = 0.1 }]
[[transition]]
from = "Move"
to = "Idle"
when = [{ parameter = "speed", op = "<=", value = 0.1 }]
[[transition]]
from = "*"
to = "Jump"
when = [{ parameter = "jump" }, { parameter = "grounded" }]
[[transition]]
from = "Jump"
to = "Idle"
exit_time = 0.9
"#;

    fn player() -> AnimationPlayer {
        let mut library = AnimationLibrary::new();
        let mut p = AnimationPlayer::new(Vec::new());
        p.clips = ["Idle", "Walk", "Run", "Jump"]
            .iter()
            .map(|n| {
                (
                    (*n).to_owned(),
                    library.add(AnimationClip::new(*n, Vec::new())),
                )
            })
            .collect();
        p
    }

    #[test]
    fn idle_walk_run_jump_and_back() {
        let mut m = StateMachine::new("hero.abstate", StateMachineFile::from_toml(HERO).unwrap());
        let mut p = player();
        assert!(step(&mut m, &mut p, 1.0, 0.016));
        assert_eq!(m.state_name(), Some("Idle"));
        assert_eq!(p.current_name(), Some("Idle"));

        // Moving: blended walk and run by speed.
        m.set("speed", 2.75);
        assert!(step(&mut m, &mut p, 1.0, 0.016));
        assert_eq!(m.state_name(), Some("Move"));
        assert_eq!(p.current_name(), Some("Walk"));
        let (other, weight) = p.blend.unwrap();
        assert_eq!(other, p.clip_named("Run").unwrap());
        assert!((weight - 0.5).abs() < 1e-4);
        m.set("speed", 5.0);
        step(&mut m, &mut p, 1.0, 0.016);
        assert_eq!(p.current_name(), Some("Run"));
        assert!(p.blend.is_none());

        // The trigger, from any state, when grounded; used up.
        m.set("grounded", 0.0);
        m.set("jump", 1.0);
        assert!(!step(&mut m, &mut p, 1.0, 0.016), "not in the air");
        m.set("grounded", 1.0);
        assert!(step(&mut m, &mut p, 1.0, 0.016));
        assert_eq!(m.state_name(), Some("Jump"));
        assert_eq!(m.get("jump"), Some(0.0));
        assert!(!p.looping);

        // Back to idle near the end of the jump clip.
        p.time = 0.5;
        assert!(!step(&mut m, &mut p, 1.0, 0.016));
        p.time = 0.95;
        assert!(step(&mut m, &mut p, 1.0, 0.016));
        assert_eq!(m.state_name(), Some("Idle"));
        let names: Vec<&str> = m.history.iter().map(|c| c.to.as_str()).collect();
        assert_eq!(names, ["Idle", "Move", "Jump", "Idle"]);
    }

    #[test]
    fn files_are_checked_and_round_trip() {
        let file = StateMachineFile::from_toml(HERO).unwrap();
        assert_eq!(StateMachineFile::from_toml(&file.to_toml()).unwrap(), file);
        let broken = HERO.replace("to = \"Move\"", "to = \"Swim\"");
        assert!(StateMachineFile::from_toml(&broken)
            .unwrap_err()
            .contains("Swim"));
        let broken = HERO.replace("parameter = \"jump\" }", "parameter = \"fly\" }");
        assert!(StateMachineFile::from_toml(&broken)
            .unwrap_err()
            .contains("fly"));
        assert!(StateMachineFile::from_toml("[[state]]\nname = \"A\"\n")
            .unwrap_err()
            .contains("no clip"));
    }
}
