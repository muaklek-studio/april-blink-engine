use crate::event::SoundEvent;
use crate::stream::AudioStream;
use serde::{Deserialize, Serialize};

/// How loudness falls between `min_distance` and `max_distance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rolloff {
    /// Like real sound (each doubling of distance about 6 dB quieter), reaching silence at
    /// `max_distance`.
    #[default]
    Inverse,
    /// Evenly from full to silent.
    Linear,
    /// Slow at first, then falling fast near the end.
    Logarithmic,
    /// Through [`Attenuation::curve`].
    Custom,
}

/// Distance falloff and 3D behaviour of a positional sound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Attenuation {
    /// Up to this distance (m) from the listener the sound plays at full volume.
    pub min_distance: f32,
    /// From this distance on it is silent (and a sound starting there is not played).
    pub max_distance: f32,
    pub rolloff: Rolloff,
    /// Points `[distance, volume]` of a custom rolloff, both 0..=1 (distance from
    /// `min_distance` to `max_distance`; volume linear).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub curve: Vec<[f32; 2]>,
    /// 0: heard from its direction; 1: from everywhere (big, close sources).
    pub spread: f32,
    /// Pitch shift of moving sources: 0 off, 1 as in air.
    pub doppler: f32,
    /// Muffled when something solid is between it and the listener.
    pub occlusion: bool,
}

impl Default for Attenuation {
    fn default() -> Self {
        Self {
            min_distance: 1.0,
            max_distance: 40.0,
            rolloff: Rolloff::Inverse,
            curve: Vec::new(),
            spread: 0.0,
            doppler: 0.0,
            occlusion: false,
        }
    }
}

impl Attenuation {
    /// Linear volume at `distance` metres from the listener.
    pub fn gain(&self, distance: f32) -> f32 {
        let min = if self.min_distance.is_finite() {
            self.min_distance.max(0.0)
        } else {
            1.0
        };
        let max = if self.max_distance.is_finite() {
            self.max_distance.max(min + 1e-3)
        } else {
            min + 1e-3
        };
        // NaN too: better heard than lost.
        if distance.is_nan() || distance <= min {
            return 1.0;
        }
        if distance >= max {
            return 0.0;
        }
        let x = (distance - min) / (max - min);
        let gain = match self.rolloff {
            Rolloff::Linear => 1.0 - x,
            Rolloff::Inverse => {
                // min/d, shifted and scaled to reach 0 at max.
                let reference = min.max(0.1);
                let floor = reference / max.max(reference);
                ((reference / distance.max(reference)) - floor) / (1.0 - floor).max(1e-6)
            }
            Rolloff::Logarithmic => {
                let reference = min.max(0.1);
                1.0 - (distance.max(reference) / reference).ln() / (max / reference).ln().max(1e-6)
            }
            Rolloff::Custom => custom_gain(&self.curve, x),
        };
        gain.clamp(0.0, 1.0)
    }
}

/// Piecewise-linear lookup in `[x, y]` points (unsorted allowed); linear fall without any.
fn custom_gain(points: &[[f32; 2]], x: f32) -> f32 {
    let mut sorted: Vec<[f32; 2]> = points
        .iter()
        .copied()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .collect();
    if sorted.is_empty() {
        return 1.0 - x;
    }
    sorted.sort_by(|a, b| a[0].total_cmp(&b[0]));
    if x <= sorted[0][0] {
        return sorted[0][1];
    }
    for pair in sorted.windows(2) {
        let ([x0, y0], [x1, y1]) = (pair[0], pair[1]);
        if x <= x1 {
            let t = if x1 > x0 { (x - x0) / (x1 - x0) } else { 1.0 };
            return y0 + (y1 - y0) * t;
        }
    }
    sorted[sorted.len() - 1][1]
}

/// Plays a sound: Godot's `AudioStreamPlayer` (no position: music, UI) or, with
/// `positional` set, `AudioStreamPlayer3D` (panned and attenuated from the node's global
/// position relative to the [`AudioListener3D`]).
///
/// It plays its `event` if it has one (clip choice, randomization, bus, voice limits; the
/// player's volume, pitch and positional settings then apply on top), else its `stream`.
/// Playback stops when the node leaves the tree; `autoplay` starts it whenever the node
/// enters it.
#[derive(Debug, Clone)]
pub struct AudioPlayer {
    pub stream: Option<AudioStream>,
    pub event: Option<SoundEvent>,
    /// Bus to play on; empty: the event's, else the master.
    pub bus: String,
    /// Gain in decibels (0 = unchanged, -6 ≈ half amplitude). Applied live.
    pub volume_db: f32,
    /// Playback speed and pitch together (2 = an octave up, twice as fast). Applied live.
    pub pitch_scale: f32,
    /// Restart from the beginning when the end is reached (music, ambience).
    pub looping: bool,
    /// Start playing as soon as the node is in the tree.
    pub autoplay: bool,
    /// `Some` for a 3D player.
    pub positional: Option<Attenuation>,
    pub(crate) command: Option<Command>,
    pub(crate) playing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    Play,
    Stop,
}

impl AudioPlayer {
    /// Signal emitted (on the player node) when its stream ends on its own.
    pub const FINISHED: &'static str = "finished";

    /// A non-positional player.
    pub fn new(stream: AudioStream) -> Self {
        Self {
            stream: Some(stream),
            ..Self::empty()
        }
    }

    /// A non-positional player of a sound event.
    pub fn with_event(event: SoundEvent) -> Self {
        Self {
            positional: event.desc().spatial.clone(),
            looping: event.desc().looping,
            event: Some(event),
            ..Self::empty()
        }
    }

    /// A player with nothing to play yet.
    pub fn empty() -> Self {
        Self {
            stream: None,
            event: None,
            bus: String::new(),
            volume_db: 0.0,
            pitch_scale: 1.0,
            looping: false,
            autoplay: false,
            positional: None,
            command: None,
            playing: false,
        }
    }

    /// A positional (3D) player with default attenuation.
    pub fn positional(stream: AudioStream) -> Self {
        Self {
            positional: Some(Attenuation::default()),
            ..Self::new(stream)
        }
    }

    pub fn with_volume_db(mut self, volume_db: f32) -> Self {
        self.volume_db = volume_db;
        self
    }

    pub fn with_pitch_scale(mut self, pitch_scale: f32) -> Self {
        self.pitch_scale = pitch_scale;
        self
    }

    pub fn with_looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }

    pub fn with_autoplay(mut self, autoplay: bool) -> Self {
        self.autoplay = autoplay;
        self
    }

    pub fn with_attenuation(mut self, attenuation: Attenuation) -> Self {
        self.positional = Some(attenuation);
        self
    }

    pub fn with_bus(mut self, bus: &str) -> Self {
        self.bus = bus.to_owned();
        self
    }

    /// Plays the stream from the start (restarting it if already playing). Takes effect at
    /// the next audio update, at the end of the frame.
    pub fn play(&mut self) {
        self.command = Some(Command::Play);
        self.playing = true;
    }

    pub fn stop(&mut self) {
        self.command = Some(Command::Stop);
        self.playing = false;
    }

    /// Whether the stream is playing (or about to start).
    pub fn is_playing(&self) -> bool {
        self.playing
    }
}

/// Where the player "hears" positional sounds from (Godot's `AudioListener3D`), usually a
/// child of the camera. The first listener in the tree with `current` set wins, else the
/// first listener; without any, the listener sits at the origin looking down -Z.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioListener3D {
    pub current: bool,
}

impl Default for AudioListener3D {
    fn default() -> Self {
        Self { current: true }
    }
}

/// Shape of an [`AudioZone`], around its node.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum ZoneShape {
    Sphere {
        radius: f32,
    },
    /// A box of these half sizes, turned with the node.
    Box {
        half_extents: [f32; 3],
    },
}

/// A place that sounds different: while the listener is inside, the zone's mixer
/// snapshot takes over (a cave's long reverb, muffled sound under water), fading in over
/// `fade_distance` metres at the edge.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioZone {
    pub snapshot: String,
    pub shape: ZoneShape,
    pub fade_distance: f32,
}

impl Default for AudioZone {
    fn default() -> Self {
        Self {
            snapshot: String::new(),
            shape: ZoneShape::Box {
                half_extents: [5.0, 3.0, 5.0],
            },
            fade_distance: 2.0,
        }
    }
}

impl AudioZone {
    /// How far inside a point is (0 outside, 1 at least `fade_distance` in), given the
    /// point in the zone's own space.
    pub fn amount(&self, local: ab_core::Vec3) -> f32 {
        let depth = match self.shape {
            ZoneShape::Sphere { radius } => radius - local.length(),
            ZoneShape::Box { half_extents } => {
                let h = ab_core::Vec3::from(half_extents);
                let d = h - local.abs();
                d.min_element()
            }
        };
        if !depth.is_finite() || depth <= 0.0 {
            0.0
        } else if self.fade_distance <= 0.0 {
            1.0
        } else {
            (depth / self.fade_distance).min(1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_core::Vec3;

    fn attenuation(rolloff: Rolloff) -> Attenuation {
        Attenuation {
            min_distance: 1.0,
            max_distance: 21.0,
            rolloff,
            ..Attenuation::default()
        }
    }

    #[test]
    fn every_rolloff_is_full_inside_min_silent_past_max_and_falls_between() {
        for rolloff in [
            Rolloff::Inverse,
            Rolloff::Linear,
            Rolloff::Logarithmic,
            Rolloff::Custom,
        ] {
            let a = attenuation(rolloff);
            assert_eq!(a.gain(0.5), 1.0);
            assert_eq!(a.gain(21.0), 0.0);
            let mut last = 1.0;
            for d in 2..21 {
                let g = a.gain(d as f32);
                assert!(g <= last, "{rolloff:?} rises at {d} m");
                last = g;
            }
        }
        assert!((attenuation(Rolloff::Linear).gain(11.0) - 0.5).abs() < 1e-6);
        // Inverse: twice the distance, about half the amplitude (close to the source).
        let inverse = attenuation(Rolloff::Inverse);
        assert!((inverse.gain(2.0) / inverse.gain(1.0) - 0.5).abs() < 0.03);
    }

    #[test]
    fn a_custom_curve_interpolates_its_points() {
        let a = Attenuation {
            curve: vec![[1.0, 0.0], [0.0, 1.0], [0.5, 0.8]],
            ..attenuation(Rolloff::Custom)
        };
        assert!((a.gain(6.0) - 0.9).abs() < 1e-5);
        assert!((a.gain(16.0) - 0.4).abs() < 1e-5);
    }

    #[test]
    fn zones_fade_in_from_their_edge() {
        let zone = AudioZone {
            shape: ZoneShape::Sphere { radius: 10.0 },
            fade_distance: 4.0,
            ..AudioZone::default()
        };
        assert_eq!(zone.amount(Vec3::new(11.0, 0.0, 0.0)), 0.0);
        assert_eq!(zone.amount(Vec3::new(8.0, 0.0, 0.0)), 0.5);
        assert_eq!(zone.amount(Vec3::ZERO), 1.0);
        let boxed = AudioZone {
            shape: ZoneShape::Box {
                half_extents: [2.0, 2.0, 2.0],
            },
            fade_distance: 1.0,
            ..AudioZone::default()
        };
        assert_eq!(boxed.amount(Vec3::new(1.5, 0.0, 0.0)), 0.5);
    }

    #[test]
    fn attenuation_reads_from_toml() {
        let a: Attenuation = toml::from_str("max_distance = 12.0\nrolloff = \"linear\"").unwrap();
        assert_eq!(a.max_distance, 12.0);
        assert_eq!(a.rolloff, Rolloff::Linear);
        assert_eq!(a.min_distance, 1.0);
    }
}
