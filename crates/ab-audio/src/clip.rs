//! Non-destructive clip edits: trim, gain, normalize, fades and loop points, applied when
//! an event loads (the sound file stays as it is).

use crate::dsp::{db_to_gain, gain_to_db};
use crate::stream::{AudioStream, StreamData};
use kira::sound::static_sound::StaticSoundData;
use kira::Frame;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Peak level normalizing aims for, dBFS.
pub const NORMALIZE_PEAK_DB: f32 = -1.0;

/// How a clip is cut from its file. Times are seconds in the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipEdit {
    pub volume_db: f32,
    /// Brings the clip's peak to [`NORMALIZE_PEAK_DB`] (before `volume_db`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub normalize: bool,
    pub start: f32,
    /// Where it ends (0: the end of the file).
    pub end: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    /// Where a looping play goes back to.
    pub loop_start: f32,
    /// Where a looping play jumps back (0: the clip's end).
    pub loop_end: f32,
}

impl Default for ClipEdit {
    fn default() -> Self {
        Self {
            volume_db: 0.0,
            normalize: false,
            start: 0.0,
            end: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            loop_start: 0.0,
            loop_end: 0.0,
        }
    }
}

/// Playback settings a stream carries for kira (see [`AudioStream`]): where to start and
/// stop in a streamed file, its loop and gain. Decoded clips have their cut baked in and
/// use only the loop.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct StreamCut {
    /// Seconds into the file, and where to end (`None`: the end).
    pub slice: Option<(f64, Option<f64>)>,
    /// Loop from, to (`None`: the end), relative to where the stream starts.
    pub loop_region: Option<(f64, Option<f64>)>,
    pub volume_db: f32,
    pub fade_in: f32,
}

/// Equal-power fade shape over `0..=1`.
fn fade_curve(t: f32) -> f32 {
    (t.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2).sin()
}

fn finite(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

impl ClipEdit {
    /// Whether it changes anything.
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }

    /// `stream` cut as described, and for a streamed file the time to stop at with the
    /// fade-out before it (the player fades it when it gets there).
    pub(crate) fn apply(&self, stream: &AudioStream) -> (AudioStream, Option<(f64, f32)>) {
        let duration = stream.duration().as_secs_f32();
        let start = finite(self.start).min(duration);
        let end = match finite(self.end) {
            e if e > start && e < duration => e,
            _ => duration,
        };
        let length = (end - start).max(0.0);
        let fade_in = finite(self.fade_in).min(length);
        let fade_out = finite(self.fade_out).min(length - fade_in);
        let loop_start = (finite(self.loop_start) - start).clamp(0.0, length);
        let loop_end = match finite(self.loop_end) {
            e if e > start + loop_start && e < end => Some(f64::from(e - start)),
            _ => None,
        };
        let loop_region =
            (loop_start > 0.0 || loop_end.is_some()).then_some((f64::from(loop_start), loop_end));

        match &stream.data {
            StreamData::Decoded(data) => {
                let rate = data.sample_rate as f32;
                let from = ((start * rate) as usize).min(data.frames.len());
                let to = ((end * rate) as usize).clamp(from, data.frames.len());
                let mut frames: Vec<Frame> = data.frames[from..to].to_vec();
                let mut gain = db_to_gain(if self.volume_db.is_finite() {
                    self.volume_db
                } else {
                    0.0
                });
                if self.normalize {
                    let peak = peak(&frames);
                    if peak > 1e-6 {
                        gain *= db_to_gain(NORMALIZE_PEAK_DB) / peak;
                    }
                }
                let fade_in_frames = (fade_in * rate) as usize;
                let fade_out_frames = (fade_out * rate) as usize;
                let n = frames.len();
                for (i, frame) in frames.iter_mut().enumerate() {
                    let mut g = gain;
                    if i < fade_in_frames {
                        g *= fade_curve(i as f32 / fade_in_frames as f32);
                    }
                    if n - i <= fade_out_frames {
                        g *= fade_curve((n - i - 1) as f32 / fade_out_frames as f32);
                    }
                    *frame *= g;
                }
                let data = StaticSoundData {
                    sample_rate: data.sample_rate,
                    frames: Arc::from(frames),
                    settings: data.settings,
                    slice: None,
                };
                let mut out = AudioStream {
                    data: StreamData::Decoded(Box::new(data)),
                    cut: StreamCut::default(),
                };
                out.cut.loop_region = loop_region;
                (out, None)
            }
            StreamData::Streaming { .. } => {
                let mut out = stream.clone();
                let trimmed = start > 0.0 || end < duration;
                out.cut = StreamCut {
                    slice: trimmed.then_some((f64::from(start), Some(f64::from(end)))),
                    loop_region,
                    volume_db: if self.volume_db.is_finite() {
                        self.volume_db
                    } else {
                        0.0
                    },
                    fade_in,
                };
                let end_fade = (fade_out > 0.0).then_some((f64::from(length), fade_out));
                (out, end_fade)
            }
        }
    }
}

fn peak(frames: &[Frame]) -> f32 {
    frames
        .iter()
        .map(|f| f.left.abs().max(f.right.abs()))
        .fold(0.0, f32::max)
}

/// The shape of a sound for drawing: per column, the lowest and highest sample (mono mix).
pub fn waveform(stream: &AudioStream, columns: usize) -> Option<Vec<[f32; 2]>> {
    let StreamData::Decoded(data) = &stream.data else {
        return None;
    };
    let frames = &data.frames;
    if frames.is_empty() || columns == 0 {
        return Some(vec![[0.0, 0.0]; columns]);
    }
    Some(
        (0..columns)
            .map(|c| {
                let from = c * frames.len() / columns;
                let to = ((c + 1) * frames.len() / columns)
                    .max(from + 1)
                    .min(frames.len());
                frames[from..to]
                    .iter()
                    .fold([f32::MAX, f32::MIN], |[lo, hi], f| {
                        let s = (f.left + f.right) * 0.5;
                        [lo.min(s), hi.max(s)]
                    })
            })
            .collect(),
    )
}

/// The highest sample of a decoded sound, in dBFS (`None` while streaming).
pub fn peak_db(stream: &AudioStream) -> Option<f32> {
    match &stream.data {
        StreamData::Decoded(data) => Some(gain_to_db(peak(&data.frames))),
        StreamData::Streaming { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ramp() -> AudioStream {
        // 1 s at 1 kHz, every sample 0.5.
        AudioStream::from_samples(1000, &[0.5; 1000])
    }

    fn frames(stream: &AudioStream) -> Vec<f32> {
        let StreamData::Decoded(data) = &stream.data else {
            panic!("decoded")
        };
        data.frames.iter().map(|f| f.left).collect()
    }

    #[test]
    fn trimming_keeps_the_part_between_start_and_end() {
        let edit = ClipEdit {
            start: 0.25,
            end: 0.75,
            ..ClipEdit::default()
        };
        let (out, end) = edit.apply(&ramp());
        assert_eq!(out.duration(), Duration::from_millis(500));
        assert!(end.is_none());
    }

    #[test]
    fn fades_start_and_end_in_silence_and_normalize_sets_the_peak() {
        let edit = ClipEdit {
            fade_in: 0.1,
            fade_out: 0.1,
            normalize: true,
            ..ClipEdit::default()
        };
        let (out, _) = edit.apply(&ramp());
        let s = frames(&out);
        assert_eq!(s[0], 0.0);
        assert!(s[999].abs() < 1e-6);
        assert!((s[500] - db_to_gain(NORMALIZE_PEAK_DB)).abs() < 1e-4);
        assert!(s[50] > s[10], "rising");
    }

    #[test]
    fn loop_points_are_relative_to_the_trimmed_start() {
        let edit = ClipEdit {
            start: 0.2,
            loop_start: 0.5,
            loop_end: 0.9,
            ..ClipEdit::default()
        };
        let (out, _) = edit.apply(&ramp());
        let (from, to) = out.cut.loop_region.unwrap();
        assert!((from - 0.3).abs() < 1e-6);
        assert!((to.unwrap() - 0.7).abs() < 1e-6);
    }

    #[test]
    fn nonsense_times_fall_back_to_the_whole_clip() {
        let edit = ClipEdit {
            start: f32::NAN,
            end: -3.0,
            fade_in: 99.0,
            ..ClipEdit::default()
        };
        let (out, _) = edit.apply(&ramp());
        assert_eq!(out.duration(), Duration::from_secs(1));
    }

    #[test]
    fn the_waveform_has_one_range_per_column() {
        let w = waveform(&ramp(), 10).unwrap();
        assert_eq!(w.len(), 10);
        assert!(w.iter().all(|&[lo, hi]| lo == 0.5 && hi == 0.5));
        assert_eq!(peak_db(&ramp()).map(|p| p.round()), Some(-6.0));
    }
}
