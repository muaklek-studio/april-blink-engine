//! Sound events: what the game asks for ("footstep", "explosion") rather than a file. An
//! event picks one of its clips each time, varies its volume and pitch, plays on a bus,
//! limits how many of itself play at once and how 3D it is. Its settings are an asset,
//! `*.absound`:
//!
//! ```toml
//! bus = "SFX"
//! volume_random_db = 2.0
//! pitch_random_semitones = 1.5
//! max_voices = 4
//! steal = "oldest"
//! cooldown = 0.05
//!
//! [spatial]
//! min_distance = 1.0
//! max_distance = 30.0
//! rolloff = "inverse"
//! occlusion = true
//!
//! [[clip]]
//! path = "audio/step_1.wav"
//!
//! [[clip]]
//! path = "audio/step_2.wav"
//! start = 0.02
//! fade_out = 0.05
//! ```

use crate::clip::ClipEdit;
use crate::components::Attenuation;
use crate::stream::AudioStream;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// File extension of sound event assets.
pub const SOUND_EVENT_EXTENSION: &str = "absound";

/// How an event chooses its next clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickMode {
    /// At random by weight, never the same clip twice in a row.
    #[default]
    RandomNoRepeat,
    /// At random by weight.
    Random,
    /// Every clip once in a random order, then a new order.
    Shuffle,
    /// In order, then from the start again.
    Sequence,
}

/// What happens when an event already plays `max_voices` times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StealMode {
    /// The oldest stops (quickly faded) for the new one.
    #[default]
    Oldest,
    /// The quietest one where the listener is stops.
    Quietest,
    /// The new one does not play.
    None,
}

/// One clip of an event: a sound file and how it is cut.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClipDesc {
    /// Asset path of the sound file.
    pub path: String,
    /// Relative chance of being picked.
    pub weight: f32,
    #[serde(flatten)]
    pub edit: ClipEdit,
}

impl Default for ClipDesc {
    fn default() -> Self {
        Self {
            path: String::new(),
            weight: 1.0,
            edit: ClipEdit::default(),
        }
    }
}

impl ClipDesc {
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_owned(),
            ..Self::default()
        }
    }
}

/// The contents of a `*.absound` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SoundEventDesc {
    /// Bus it plays on (the master when empty or unknown).
    pub bus: String,
    pub volume_db: f32,
    /// Each play is up to this much quieter or louder, dB.
    pub volume_random_db: f32,
    pub pitch_semitones: f32,
    /// Each play is up to this much lower or higher, semitones.
    pub pitch_random_semitones: f32,
    pub mode: PickMode,
    /// Plays until stopped (players and ambience).
    pub looping: bool,
    /// Most copies playing at once (0: no limit).
    pub max_voices: u32,
    pub steal: StealMode,
    /// 0 matters most, 255 least: when the mixer runs out of voices
    /// ([`crate::MixerSettings::max_voices`]) the least important stop first.
    pub priority: u8,
    /// Seconds before the event can play again (rapid fire collapses into one).
    pub cooldown: f32,
    /// Heard from a place in the world; unset for 2D (music, UI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spatial: Option<Attenuation>,
    #[serde(rename = "clip", skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<ClipDesc>,
}

impl Default for SoundEventDesc {
    fn default() -> Self {
        Self {
            bus: String::new(),
            volume_db: 0.0,
            volume_random_db: 0.0,
            pitch_semitones: 0.0,
            pitch_random_semitones: 0.0,
            mode: PickMode::default(),
            looping: false,
            max_voices: 0,
            steal: StealMode::default(),
            priority: 128,
            cooldown: 0.0,
            spatial: None,
            clips: Vec::new(),
        }
    }
}

impl SoundEventDesc {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }
}

/// A clip ready to play: its sound with the edit applied.
#[derive(Debug, Clone)]
pub(crate) struct PreparedClip {
    pub stream: AudioStream,
    pub weight: f32,
    /// Seconds into a streamed file to stop at, and the fade-out before it (decoded clips
    /// have both baked in).
    pub stream_end: Option<(f64, f32)>,
}

/// Chooses clips (see [`PickMode`]); one per event, shared by everything playing it.
#[derive(Debug, Default)]
struct Picker {
    last: Option<usize>,
    bag: Vec<usize>,
    next: usize,
}

/// A loaded sound event, cheap to clone. Make one with [`SoundEvent::new`] from its
/// settings and the files of its clips (the asset server does this for `*.absound`).
#[derive(Clone)]
pub struct SoundEvent {
    inner: Arc<Inner>,
}

struct Inner {
    id: u64,
    path: String,
    desc: SoundEventDesc,
    clips: Vec<PreparedClip>,
    picker: Mutex<Picker>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl SoundEvent {
    /// `streams` holds the loaded file of each clip in `desc.clips` (`None` for a file that
    /// failed to load: that clip is skipped).
    pub fn new(path: &str, desc: SoundEventDesc, streams: Vec<Option<AudioStream>>) -> Self {
        let clips = desc
            .clips
            .iter()
            .zip(streams)
            .filter_map(|(clip, stream)| {
                let stream = stream?;
                let (stream, stream_end) = clip.edit.apply(&stream);
                Some(PreparedClip {
                    stream,
                    weight: if clip.weight.is_finite() {
                        clip.weight.max(0.0)
                    } else {
                        0.0
                    },
                    stream_end,
                })
            })
            .collect();
        Self {
            inner: Arc::new(Inner {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                path: path.to_owned(),
                desc,
                clips,
                picker: Mutex::new(Picker::default()),
            }),
        }
    }

    /// An event of one sound and default settings.
    pub fn from_stream(stream: AudioStream) -> Self {
        Self::new(
            "",
            SoundEventDesc {
                clips: vec![ClipDesc::default()],
                ..SoundEventDesc::default()
            },
            vec![Some(stream)],
        )
    }

    pub fn desc(&self) -> &SoundEventDesc {
        &self.inner.desc
    }

    /// The asset it was loaded from (empty when made in code).
    pub fn path(&self) -> &str {
        &self.inner.path
    }

    /// Identifies the event across clones (voice limits count per event).
    pub(crate) fn id(&self) -> u64 {
        self.inner.id
    }

    /// Clips that can play.
    pub fn clip_count(&self) -> usize {
        self.inner.clips.len()
    }

    /// Bytes its sounds hold.
    pub fn memory_bytes(&self) -> usize {
        self.inner
            .clips
            .iter()
            .map(|c| c.stream.memory_bytes())
            .sum()
    }

    /// The next clip; `random` is uniform in `0..1`.
    pub(crate) fn pick(&self, mut random: impl FnMut() -> f32) -> Option<&PreparedClip> {
        let clips = &self.inner.clips;
        if clips.is_empty() {
            return None;
        }
        let mut picker = self
            .inner
            .picker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let weighted = |exclude: Option<usize>, r: f32| -> usize {
            let total: f32 = clips
                .iter()
                .enumerate()
                .filter(|(i, _)| Some(*i) != exclude)
                .map(|(_, c)| c.weight)
                .sum();
            if total <= 0.0 {
                return (0..clips.len()).find(|&i| Some(i) != exclude).unwrap_or(0);
            }
            let mut at = r * total;
            for (i, clip) in clips.iter().enumerate() {
                if Some(i) == exclude {
                    continue;
                }
                if at < clip.weight {
                    return i;
                }
                at -= clip.weight;
            }
            (0..clips.len())
                .rev()
                .find(|&i| Some(i) != exclude)
                .unwrap_or(0)
        };
        let index = match self.inner.desc.mode {
            PickMode::Random => weighted(None, random()),
            PickMode::RandomNoRepeat => {
                let exclude = picker.last.filter(|_| clips.len() > 1);
                weighted(exclude, random())
            }
            PickMode::Sequence => {
                let i = picker.next % clips.len();
                picker.next = i + 1;
                i
            }
            PickMode::Shuffle => {
                if picker.bag.is_empty() {
                    let mut bag: Vec<usize> = (0..clips.len()).collect();
                    // Fisher–Yates; the first of a new round is not the last of the old.
                    for i in (1..bag.len()).rev() {
                        let j = ((random() * (i + 1) as f32) as usize).min(i);
                        bag.swap(i, j);
                    }
                    if bag.len() > 1 && bag.last() == picker.last.as_ref() {
                        bag.swap(0, clips.len() - 1);
                    }
                    picker.bag = bag;
                }
                picker.bag.pop().unwrap_or(0)
            }
        };
        picker.last = Some(index);
        clips.get(index)
    }
}

impl fmt::Debug for SoundEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SoundEvent")
            .field("path", &self.inner.path)
            .field("clips", &self.inner.clips.len())
            .finish_non_exhaustive()
    }
}

impl PartialEq for SoundEvent {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(mode: PickMode, clips: usize) -> SoundEvent {
        let desc = SoundEventDesc {
            mode,
            clips: (0..clips)
                .map(|i| ClipDesc::new(&format!("{i}.wav")))
                .collect(),
            ..SoundEventDesc::default()
        };
        let streams = (0..clips)
            .map(|i| Some(AudioStream::from_samples(1000, &[i as f32 * 0.1; 10])))
            .collect();
        SoundEvent::new("test.absound", desc, streams)
    }

    fn picks(event: &SoundEvent, n: usize) -> Vec<usize> {
        let mut seed = 12345u32;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32).min(0.9999)
        };
        (0..n)
            .map(|_| {
                let clip = event.pick(&mut random).unwrap();
                event
                    .inner
                    .clips
                    .iter()
                    .position(|c| std::ptr::eq(c, clip))
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn the_module_example_parses() {
        let text = include_str!("event.rs")
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
        let desc = SoundEventDesc::from_toml(&text).unwrap();
        assert_eq!(desc.clips.len(), 2);
        assert_eq!(desc.clips[1].edit.start, 0.02);
        assert!(desc.spatial.as_ref().unwrap().occlusion);
        let again = SoundEventDesc::from_toml(&desc.to_toml()).unwrap();
        assert_eq!(again, desc);
    }

    #[test]
    fn random_no_repeat_never_plays_a_clip_twice_in_a_row() {
        let p = picks(&event(PickMode::RandomNoRepeat, 3), 200);
        assert!(p.windows(2).all(|w| w[0] != w[1]));
        for i in 0..3 {
            assert!(p.contains(&i));
        }
    }

    #[test]
    fn shuffle_plays_every_clip_once_per_round() {
        let p = picks(&event(PickMode::Shuffle, 4), 40);
        for round in p.chunks(4) {
            let mut sorted = round.to_vec();
            sorted.sort();
            assert_eq!(sorted, [0, 1, 2, 3]);
        }
        assert!(p.windows(2).all(|w| w[0] != w[1]), "not even across rounds");
    }

    #[test]
    fn sequence_goes_in_order() {
        assert_eq!(picks(&event(PickMode::Sequence, 3), 5), [0, 1, 2, 0, 1]);
    }

    #[test]
    fn a_single_clip_repeats_whatever_the_mode() {
        for mode in [PickMode::RandomNoRepeat, PickMode::Shuffle] {
            assert_eq!(picks(&event(mode, 1), 3), [0, 0, 0]);
        }
    }
}
