//! The game's mixer: buses in a tree under the master, return buses fed by sends, and
//! snapshots that move many settings at once. Its settings are the project's `audio.toml`:
//!
//! ```toml
//! max_voices = 64
//!
//! [master]
//! volume_db = 0.0
//! [[master.effect]]
//! type = "limiter"
//! ceiling_db = -0.3
//!
//! [[bus]]
//! name = "SFX"
//! [[bus.send]]
//! to = "Reverb"
//! level_db = -12.0
//!
//! [[bus]]
//! name = "Footsteps"
//! parent = "SFX"
//!
//! [[return]]
//! name = "Reverb"
//! [[return.effect]]
//! type = "reverb"
//!
//! [[snapshot]]
//! name = "Pause"
//! fade_in = 0.3
//! [[snapshot.bus]]
//! bus = "SFX"
//! volume_db = -20.0
//! ```

use crate::chain::Chain;
use crate::dsp::meter::Levels;
use crate::dsp::SILENCE_DB;
use crate::effect::EffectDesc;
use kira::backend::Backend;
use kira::track::{SendTrackBuilder, SendTrackHandle, SendTrackId, TrackBuilder, TrackHandle};
use kira::{AudioManager, Decibels, Tween};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

/// Name of the bus everything ends up in.
pub const MASTER: &str = "Master";

/// Sounds a bus can play at once (its own, not its children's).
const BUS_SOUNDS: usize = 256;
/// 3D voices (each a track) a bus can hold.
const BUS_SUB_TRACKS: usize = 512;

/// The contents of `audio.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MixerSettings {
    /// Most sounds heard at once; past it the least important stop (see
    /// [`crate::SoundEventDesc::priority`]).
    pub max_voices: u32,
    pub master: MasterSettings,
    #[serde(rename = "bus", skip_serializing_if = "Vec::is_empty")]
    pub buses: Vec<BusDesc>,
    /// Buses fed only through sends (reverbs, delays shared by many buses).
    #[serde(rename = "return", skip_serializing_if = "Vec::is_empty")]
    pub returns: Vec<ReturnDesc>,
    #[serde(rename = "snapshot", skip_serializing_if = "Vec::is_empty")]
    pub snapshots: Vec<SnapshotDesc>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MasterSettings {
    pub volume_db: f32,
    #[serde(rename = "effect", skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectDesc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BusDesc {
    pub name: String,
    /// The bus it feeds (the master when unset).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub volume_db: f32,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub mute: bool,
    /// Only soloed buses (and what they feed or are fed by) are heard. An editor aid.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub solo: bool,
    #[serde(rename = "effect", skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectDesc>,
    #[serde(rename = "send", skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<SendDesc>,
}

impl Default for BusDesc {
    fn default() -> Self {
        Self {
            name: String::new(),
            parent: None,
            volume_db: 0.0,
            mute: false,
            solo: false,
            effects: Vec::new(),
            sends: Vec::new(),
        }
    }
}

impl BusDesc {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::default()
        }
    }

    /// The bus it feeds.
    pub fn parent_name(&self) -> &str {
        self.parent
            .as_deref()
            .filter(|p| !p.is_empty())
            .unwrap_or(MASTER)
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReturnDesc {
    pub name: String,
    pub volume_db: f32,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub mute: bool,
    #[serde(rename = "effect", skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectDesc>,
}

/// Part of a bus's output sent to a return, after its fader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendDesc {
    pub to: String,
    #[serde(default)]
    pub level_db: f32,
}

/// Settings that take over while the snapshot is active, faded in and out: a pause menu
/// turning the game down, an underwater muffle, a cave's reverb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SnapshotDesc {
    pub name: String,
    /// Seconds to take over.
    pub fade_in: f32,
    /// Seconds to let go.
    pub fade_out: f32,
    #[serde(rename = "bus", skip_serializing_if = "Vec::is_empty")]
    pub buses: Vec<BusOverride>,
}

impl Default for SnapshotDesc {
    fn default() -> Self {
        Self {
            name: String::new(),
            fade_in: 0.5,
            fade_out: 0.5,
            buses: Vec::new(),
        }
    }
}

/// What a snapshot changes on one bus (a bus, a return or the master); unset fields stay.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BusOverride {
    pub bus: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_db: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    /// The whole chain; with the same effects as the bus, their numbers blend.
    #[serde(rename = "effect", skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<EffectDesc>>,
    #[serde(rename = "send", skip_serializing_if = "Option::is_none")]
    pub sends: Option<Vec<SendDesc>>,
}

impl Default for MixerSettings {
    /// A layout most games start from: music, effects, voice, UI and ambience under a
    /// limited master, with a shared reverb.
    fn default() -> Self {
        use crate::effect::{EffectKind, LimiterSettings, ReverbSettings};
        let reverb_send = |level_db| SendDesc {
            to: "Reverb".into(),
            level_db,
        };
        Self {
            max_voices: 64,
            master: MasterSettings {
                volume_db: 0.0,
                effects: vec![EffectDesc::new(EffectKind::Limiter(
                    LimiterSettings::default(),
                ))],
            },
            buses: vec![
                BusDesc::new("Music"),
                BusDesc {
                    sends: vec![reverb_send(-12.0)],
                    ..BusDesc::new("SFX")
                },
                BusDesc {
                    sends: vec![reverb_send(-18.0)],
                    ..BusDesc::new("Voice")
                },
                BusDesc::new("UI"),
                BusDesc {
                    sends: vec![reverb_send(-18.0)],
                    ..BusDesc::new("Ambience")
                },
            ],
            returns: vec![ReturnDesc {
                name: "Reverb".into(),
                effects: vec![EffectDesc::new(EffectKind::Reverb(
                    ReverbSettings::default(),
                ))],
                ..ReturnDesc::default()
            }],
            snapshots: Vec::new(),
        }
    }
}

impl MixerSettings {
    /// What is wrong with the settings, as messages (empty when they are usable).
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut names = HashSet::from([MASTER]);
        for name in self
            .buses
            .iter()
            .map(|b| b.name.as_str())
            .chain(self.returns.iter().map(|r| r.name.as_str()))
        {
            if name.trim().is_empty() {
                problems.push("a bus has no name".to_owned());
            } else if !names.insert(name) {
                problems.push(format!("two buses are called \"{name}\""));
            }
        }
        let buses: HashSet<&str> = self.buses.iter().map(|b| b.name.as_str()).collect();
        let returns: HashSet<&str> = self.returns.iter().map(|r| r.name.as_str()).collect();
        for bus in &self.buses {
            let parent = bus.parent_name();
            if parent != MASTER && !buses.contains(parent) {
                problems.push(format!(
                    "bus \"{}\" feeds \"{parent}\", which is not a bus",
                    bus.name
                ));
            }
            for send in &bus.sends {
                if !returns.contains(send.to.as_str()) {
                    problems.push(format!(
                        "bus \"{}\" sends to \"{}\", which is not a return",
                        bus.name, send.to
                    ));
                }
            }
        }
        if self.ordered_buses().is_none() {
            problems.push("buses feed each other in a loop".to_owned());
        }
        let all_effects = self
            .master
            .effects
            .iter()
            .chain(self.buses.iter().flat_map(|b| &b.effects))
            .chain(self.returns.iter().flat_map(|r| &r.effects));
        for effect in all_effects {
            if let Some(key) = effect.kind.sidechain() {
                if !names.contains(key) {
                    problems.push(format!(
                        "a sidechain listens to \"{key}\", which is not a bus"
                    ));
                }
            }
        }
        let mut snapshots = HashSet::new();
        for snapshot in &self.snapshots {
            if !snapshots.insert(snapshot.name.as_str()) {
                problems.push(format!("two snapshots are called \"{}\"", snapshot.name));
            }
            for o in &snapshot.buses {
                if !names.contains(o.bus.as_str()) {
                    problems.push(format!(
                        "snapshot \"{}\" changes \"{}\", which is not a bus",
                        snapshot.name, o.bus
                    ));
                }
            }
        }
        problems
    }

    /// Bus indices with every parent before its children; `None` on a loop.
    fn ordered_buses(&self) -> Option<Vec<usize>> {
        let index: HashMap<&str, usize> = self
            .buses
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.as_str(), i))
            .collect();
        let mut order = Vec::with_capacity(self.buses.len());
        let mut state = vec![0u8; self.buses.len()]; // 0 new, 1 visiting, 2 done
        fn visit(
            i: usize,
            buses: &[BusDesc],
            index: &HashMap<&str, usize>,
            state: &mut [u8],
            order: &mut Vec<usize>,
        ) -> bool {
            match state[i] {
                2 => return true,
                1 => return false,
                _ => {}
            }
            state[i] = 1;
            if let Some(&parent) = index.get(buses[i].parent_name()) {
                if !visit(parent, buses, index, state, order) {
                    return false;
                }
            }
            state[i] = 2;
            order.push(i);
            true
        }
        for i in 0..self.buses.len() {
            if !visit(i, &self.buses, &index, &mut state, &mut order) {
                return None;
            }
        }
        Some(order)
    }

    /// Every bus name: the master, the buses, then the returns.
    pub fn bus_names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(MASTER)
            .chain(self.buses.iter().map(|b| b.name.as_str()))
            .chain(self.returns.iter().map(|r| r.name.as_str()))
    }

    /// What decides the shape of the kira graph: when it changes, the graph is rebuilt.
    fn layout(&self) -> Vec<(String, String, Vec<String>)> {
        let mut layout: Vec<_> = self
            .buses
            .iter()
            .map(|b| {
                (
                    b.name.clone(),
                    b.parent_name().to_owned(),
                    b.sends.iter().map(|s| s.to.clone()).collect(),
                )
            })
            .collect();
        layout.extend(
            self.returns
                .iter()
                .map(|r| (r.name.clone(), String::from("<return>"), Vec::new())),
        );
        layout
    }
}

/// A bus's settings at this moment (after snapshots).
#[derive(Debug, Clone, PartialEq)]
struct BusState {
    volume_db: f32,
    mute: bool,
    effects: Vec<EffectDesc>,
    sends: Vec<SendDesc>,
}

impl BusState {
    fn apply(&mut self, o: &BusOverride, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        if let Some(v) = o.volume_db {
            self.volume_db = blend_db(self.volume_db, v, amount);
        }
        if let Some(mute) = o.mute {
            if amount >= 0.5 {
                self.mute = mute;
            }
        }
        if let Some(effects) = &o.effects {
            self.effects = if effects.len() == self.effects.len() {
                self.effects
                    .iter()
                    .zip(effects)
                    .map(|(a, b)| a.blend(b, amount))
                    .collect()
            } else if amount >= 0.5 {
                effects.clone()
            } else {
                std::mem::take(&mut self.effects)
            };
        }
        if let Some(sends) = &o.sends {
            for send in &mut self.sends {
                let target = sends
                    .iter()
                    .find(|s| s.to == send.to)
                    .map_or(SILENCE_DB, |s| s.level_db);
                send.level_db = blend_db(send.level_db, target, amount);
            }
        }
    }
}

/// Moves `a` towards `b` in gain (a fade from silence is heard as a fade, not a jump).
fn blend_db(a: f32, b: f32, t: f32) -> f32 {
    use crate::dsp::{db_to_gain, gain_to_db};
    if t >= 1.0 {
        return b;
    }
    if a > -40.0 && b > -40.0 {
        a + (b - a) * t
    } else {
        gain_to_db(db_to_gain(a) + (db_to_gain(b) - db_to_gain(a)) * t)
    }
}

struct BusNode {
    name: String,
    track: TrackHandle,
    chain: Chain,
    sends: Vec<(String, SendTrackId, f32)>,
}

struct ReturnNode {
    name: String,
    /// Kept for the track's lifetime.
    _track: SendTrackHandle,
    chain: Chain,
}

/// How far a snapshot has taken over.
#[derive(Debug, Clone, Copy, Default)]
struct SnapshotLevel {
    /// 0..=1 now.
    level: f32,
    /// Where it is going: 1 started, 0 stopped.
    target: f32,
    /// Extra amount from zones this frame, 0..=1.
    zone: f32,
}

/// The mixer as kira runs it (see [`MixerSettings`]).
pub(crate) struct Mixer {
    settings: MixerSettings,
    master: Chain,
    buses: Vec<BusNode>,
    returns: Vec<ReturnNode>,
    layout: Vec<(String, String, Vec<String>)>,
    snapshots: HashMap<String, SnapshotLevel>,
    /// Volume changes asked for by the game (on top of the settings), dB.
    offsets: HashMap<String, Fade>,
    /// Bumped each time the graph is rebuilt (sounds on the old tracks are gone).
    pub generation: u64,
    warned: HashSet<String>,
}

/// A value gliding to a target at a fixed rate.
#[derive(Debug, Clone, Copy)]
struct Fade {
    value: f32,
    target: f32,
    /// Units per second.
    rate: f32,
}

impl Fade {
    fn step(&mut self, delta: f32) {
        let step = self.rate * delta;
        if (self.target - self.value).abs() <= step || !step.is_finite() {
            self.value = self.target;
        } else {
            self.value += step.copysign(self.target - self.value);
        }
    }
}

impl Mixer {
    /// A mixer whose master chain is `master` (already on kira's main track).
    pub fn new(master: Chain) -> Self {
        Self {
            settings: MixerSettings {
                buses: Vec::new(),
                returns: Vec::new(),
                master: MasterSettings::default(),
                ..MixerSettings::default()
            },
            master,
            buses: Vec::new(),
            returns: Vec::new(),
            layout: Vec::new(),
            snapshots: HashMap::new(),
            offsets: HashMap::new(),
            generation: 0,
            warned: HashSet::new(),
        }
    }

    pub fn settings(&self) -> &MixerSettings {
        &self.settings
    }

    /// Takes new settings; rebuilds the buses if their layout changed (sounds playing on
    /// them stop). Settings with [`MixerSettings::problems`] keep only what works.
    pub fn configure<B: Backend>(
        &mut self,
        manager: &mut AudioManager<B>,
        settings: MixerSettings,
    ) {
        for problem in settings.problems() {
            warn!("audio.toml: {problem}");
        }
        let layout = settings.layout();
        self.settings = settings;
        if layout != self.layout {
            self.rebuild(manager);
            self.layout = layout;
        }
        self.apply();
    }

    fn rebuild<B: Backend>(&mut self, manager: &mut AudioManager<B>) {
        // Children go before their parents' handles: drop everything at once.
        self.buses.clear();
        self.returns.clear();
        self.generation += 1;

        let mut return_ids = HashMap::new();
        let mut seen = HashSet::from([MASTER.to_owned()]);
        for r in &self.settings.returns {
            if !seen.insert(r.name.clone()) {
                continue;
            }
            let (chain, builder) = Chain::new();
            match manager.add_send_track(SendTrackBuilder::new().with_effect(builder)) {
                Ok(track) => {
                    return_ids.insert(r.name.clone(), track.id());
                    self.returns.push(ReturnNode {
                        name: r.name.clone(),
                        _track: track,
                        chain,
                    });
                }
                Err(err) => warn!("cannot create return bus \"{}\": {err}", r.name),
            }
        }

        let Some(order) = self.settings.ordered_buses() else {
            return;
        };
        let mut index: HashMap<String, usize> = HashMap::new();
        for i in order {
            let desc = &self.settings.buses[i];
            if !seen.insert(desc.name.clone()) {
                continue;
            }
            let (chain, builder) = Chain::new();
            let mut track = TrackBuilder::new()
                .sound_capacity(BUS_SOUNDS)
                .sub_track_capacity(BUS_SUB_TRACKS)
                .with_effect(builder);
            let mut sends = Vec::new();
            for send in &desc.sends {
                if let Some(&id) = return_ids.get(&send.to) {
                    track = track.with_send(id, Decibels::SILENCE);
                    sends.push((send.to.clone(), id, SILENCE_DB));
                }
            }
            let parent = desc.parent_name();
            let created = match index.get(parent) {
                Some(&p) => self.buses[p].track.add_sub_track(track),
                None => manager.add_sub_track(track),
            };
            match created {
                Ok(track) => {
                    index.insert(desc.name.clone(), self.buses.len());
                    self.buses.push(BusNode {
                        name: desc.name.clone(),
                        track,
                        chain,
                        sends,
                    });
                }
                Err(err) => warn!("cannot create bus \"{}\": {err}", desc.name),
            }
        }
    }

    /// Starts (`true`) or stops a snapshot, fading over its own times.
    pub fn set_snapshot(&mut self, name: &str, active: bool) -> bool {
        if !self.settings.snapshots.iter().any(|s| s.name == name) {
            return false;
        }
        self.snapshots.entry(name.to_owned()).or_default().target = f32::from(u8::from(active));
        true
    }

    /// How far a snapshot has taken over (0..=1), zones included.
    pub fn snapshot_level(&self, name: &str) -> f32 {
        self.snapshots
            .get(name)
            .map_or(0.0, |s| s.level.max(s.zone).min(1.0))
    }

    /// A zone's pull on a snapshot this frame (the strongest zone counts).
    pub fn set_zone_amount(&mut self, name: &str, amount: f32) {
        if let Some(s) = self.snapshots.get_mut(name) {
            s.zone = s.zone.max(amount.clamp(0.0, 1.0));
        } else if self.settings.snapshots.iter().any(|s| s.name == name) {
            self.snapshots.insert(
                name.to_owned(),
                SnapshotLevel {
                    zone: amount.clamp(0.0, 1.0),
                    ..SnapshotLevel::default()
                },
            );
        }
    }

    /// Fades a bus's volume by `offset_db` on top of its settings, over `seconds`.
    pub fn set_volume_offset(&mut self, bus: &str, offset_db: f32, seconds: f32) {
        let fade = self.offsets.entry(bus.to_owned()).or_insert(Fade {
            value: 0.0,
            target: 0.0,
            rate: 0.0,
        });
        fade.target = offset_db;
        let distance = (fade.target - fade.value).abs();
        fade.rate = if seconds > 0.0 {
            distance / seconds
        } else {
            f32::INFINITY
        };
    }

    /// Advances snapshot fades and offsets by `delta` seconds and applies the result.
    pub fn update(&mut self, delta: f32) {
        for (name, s) in &mut self.snapshots {
            let Some(desc) = self.settings.snapshots.iter().find(|d| &d.name == name) else {
                continue;
            };
            let time = if s.target > s.level {
                desc.fade_in
            } else {
                desc.fade_out
            };
            if time <= 0.0 {
                s.level = s.target;
            } else {
                let step = delta / time;
                s.level += (s.target - s.level).clamp(-step, step);
            }
        }
        for fade in self.offsets.values_mut() {
            fade.step(delta);
        }
        self.apply();
        for s in self.snapshots.values_mut() {
            s.zone = 0.0;
        }
    }

    /// The settings of `name` now: its own, then each snapshot in order.
    fn state(&self, name: &str) -> Option<BusState> {
        let s = &self.settings;
        let mut state = if name == MASTER {
            BusState {
                volume_db: s.master.volume_db,
                mute: false,
                effects: s.master.effects.clone(),
                sends: Vec::new(),
            }
        } else if let Some(b) = s.buses.iter().find(|b| b.name == name) {
            BusState {
                volume_db: b.volume_db,
                mute: b.mute,
                effects: b.effects.clone(),
                sends: b.sends.clone(),
            }
        } else {
            let r = s.returns.iter().find(|r| r.name == name)?;
            BusState {
                volume_db: r.volume_db,
                mute: r.mute,
                effects: r.effects.clone(),
                sends: Vec::new(),
            }
        };
        for snapshot in &s.snapshots {
            let amount = self.snapshot_level(&snapshot.name);
            for o in snapshot.buses.iter().filter(|o| o.bus == name) {
                state.apply(o, amount);
            }
        }
        if let Some(offset) = self.offsets.get(name) {
            state.volume_db += offset.value;
        }
        Some(state)
    }

    /// Buses heard while something is soloed: the soloed, what they feed and what feeds
    /// them. `None` when nothing is soloed.
    fn audible(&self) -> Option<HashSet<String>> {
        let buses = &self.settings.buses;
        if !buses.iter().any(|b| b.solo) {
            return None;
        }
        let parent = |name: &str| {
            buses
                .iter()
                .find(|b| b.name == name)
                .map(|b| b.parent_name().to_owned())
        };
        let mut heard = HashSet::new();
        for bus in buses {
            let mut chain = vec![bus.name.clone()];
            let mut at = bus.name.clone();
            while let Some(p) = parent(&at) {
                if chain.contains(&p) || p == MASTER {
                    break;
                }
                chain.push(p.clone());
                at = p;
            }
            if chain
                .iter()
                .any(|n| buses.iter().any(|b| &b.name == n && b.solo))
            {
                // A soloed ancestor makes the whole branch heard; a soloed bus its parents.
                heard.extend(chain);
            }
        }
        Some(heard)
    }

    fn apply(&mut self) {
        let keys = self.key_levels();
        let key = |name: &str| keys.get(name).cloned();
        if let Some(state) = self.state(MASTER) {
            self.master.set_fader_db(state.volume_db);
            self.master.set_effects(&state.effects, &key);
            self.master.follow_sample_rate(&key);
        }
        let audible = self.audible();
        let states: Vec<_> = self.buses.iter().map(|b| self.state(&b.name)).collect();
        for (bus, state) in self.buses.iter_mut().zip(states) {
            let Some(state) = state else { continue };
            let soloed_out = audible.as_ref().is_some_and(|a| !a.contains(&bus.name));
            let silent = state.mute || soloed_out;
            bus.chain
                .set_fader_db(if silent { SILENCE_DB } else { state.volume_db });
            bus.chain.set_effects(&state.effects, &key);
            bus.chain.follow_sample_rate(&key);
            for (to, id, level) in &mut bus.sends {
                let wanted = state
                    .sends
                    .iter()
                    .find(|s| &s.to == to)
                    .map_or(SILENCE_DB, |s| s.level_db);
                if (wanted - *level).abs() > 0.01 {
                    *level = wanted;
                    let volume = if wanted <= SILENCE_DB {
                        Decibels::SILENCE
                    } else {
                        Decibels(wanted)
                    };
                    let tween = Tween {
                        duration: Duration::from_millis(20),
                        ..Tween::default()
                    };
                    if bus.track.set_send(*id, volume, tween).is_err() {
                        // Not routed to that return after all: try again next frame.
                        *level = f32::NAN;
                    }
                }
            }
        }
        let states: Vec<_> = self.returns.iter().map(|r| self.state(&r.name)).collect();
        for (ret, state) in self.returns.iter_mut().zip(states) {
            let Some(state) = state else { continue };
            ret.chain.set_fader_db(if state.mute {
                SILENCE_DB
            } else {
                state.volume_db
            });
            ret.chain.set_effects(&state.effects, &key);
            ret.chain.follow_sample_rate(&key);
        }
    }

    fn key_levels(&self) -> HashMap<String, Arc<Levels>> {
        std::iter::once((MASTER.to_owned(), self.master.levels().clone()))
            .chain(
                self.buses
                    .iter()
                    .map(|b| (b.name.clone(), b.chain.levels().clone())),
            )
            .chain(
                self.returns
                    .iter()
                    .map(|r| (r.name.clone(), r.chain.levels().clone())),
            )
            .collect()
    }

    /// Clears every effect's tail (reverbs, delays) on the audio thread.
    pub fn flush(&self) {
        self.master.flush();
        for bus in &self.buses {
            bus.chain.flush();
        }
        for ret in &self.returns {
            ret.chain.flush();
        }
    }

    /// The levels of a bus (master, bus or return) for meters.
    pub fn levels(&self, name: &str) -> Option<&Arc<Levels>> {
        if name == MASTER {
            return Some(self.master.levels());
        }
        self.buses
            .iter()
            .find(|b| b.name == name)
            .map(|b| b.chain.levels())
            .or_else(|| {
                self.returns
                    .iter()
                    .find(|r| r.name == name)
                    .map(|r| r.chain.levels())
            })
    }

    /// The track sounds of `bus` play on; `None` for the master (kira's main track) and,
    /// with a warning once, for a bus that does not exist.
    pub fn track_mut(&mut self, bus: Option<&str>) -> Option<&mut TrackHandle> {
        let name = bus.filter(|b| !b.is_empty() && *b != MASTER)?;
        let found = self.buses.iter().position(|b| b.name == name);
        match found {
            Some(i) => Some(&mut self.buses[i].track),
            None => {
                if self.warned.insert(name.to_owned()) {
                    warn!("no audio bus \"{name}\"; playing on the master");
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::{EffectKind, EqBand, FilterShape};

    #[test]
    fn the_default_layout_reads_back_and_has_no_problems() {
        let settings = MixerSettings::default();
        assert!(settings.problems().is_empty(), "{:?}", settings.problems());
        let text = toml::to_string(&settings).unwrap();
        let again: MixerSettings = toml::from_str(&text).unwrap();
        assert_eq!(again, settings);
    }

    #[test]
    fn the_module_example_parses() {
        let text = include_str!("mixer.rs")
            .split("//! ```toml\n")
            .nth(1)
            .unwrap()
            .split("//! ```")
            .next()
            .unwrap()
            .lines()
            .map(|l| l.trim_start_matches("//!").trim_start())
            .collect::<Vec<_>>()
            .join("\n");
        let settings: MixerSettings = toml::from_str(&text).unwrap();
        assert!(settings.problems().is_empty(), "{:?}", settings.problems());
        assert_eq!(settings.buses[1].parent_name(), "SFX");
    }

    #[test]
    fn problems_name_what_is_wrong() {
        let settings = MixerSettings {
            buses: vec![
                BusDesc {
                    parent: Some("B".into()),
                    ..BusDesc::new("A")
                },
                BusDesc {
                    parent: Some("A".into()),
                    sends: vec![SendDesc {
                        to: "Nowhere".into(),
                        level_db: 0.0,
                    }],
                    ..BusDesc::new("B")
                },
                BusDesc::new("C"),
                BusDesc::new("C"),
            ],
            ..MixerSettings::default()
        };
        let problems = settings.problems().join("\n");
        assert!(problems.contains("loop"), "{problems}");
        assert!(problems.contains("two buses are called \"C\""));
        assert!(problems.contains("Nowhere"));
    }

    #[test]
    fn snapshots_blend_volumes_in_gain_and_effects_by_number() {
        let lowpass = |freq| {
            vec![EffectDesc::new(EffectKind::Filter(EqBand {
                shape: FilterShape::HighCut,
                freq,
                ..EqBand::default()
            }))]
        };
        let mut state = BusState {
            volume_db: 0.0,
            mute: false,
            effects: lowpass(20_000.0),
            sends: vec![SendDesc {
                to: "Reverb".into(),
                level_db: -12.0,
            }],
        };
        state.apply(
            &BusOverride {
                bus: "SFX".into(),
                volume_db: Some(-12.0),
                effects: Some(lowpass(200.0)),
                sends: Some(Vec::new()),
                ..BusOverride::default()
            },
            0.5,
        );
        assert!((state.volume_db + 6.0).abs() < 1e-4);
        let EffectKind::Filter(band) = &state.effects[0].kind else {
            unreachable!()
        };
        assert!((band.freq - 2_000.0).abs() < 1.0);
        assert!(
            state.sends[0].level_db < -12.0,
            "a send not in the snapshot fades out"
        );
        assert!((blend_db(SILENCE_DB, 0.0, 0.5) + 6.02).abs() < 0.01);
    }

    #[test]
    fn fades_reach_their_target_without_overshooting() {
        let mut fade = Fade {
            value: 0.0,
            target: -10.0,
            rate: 20.0,
        };
        fade.step(0.25);
        assert_eq!(fade.value, -5.0);
        fade.step(1.0);
        assert_eq!(fade.value, -10.0);
    }
}
