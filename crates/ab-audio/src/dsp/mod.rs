//! Signal processing for the mixer, written for the audio thread: nothing here allocates,
//! locks or logs while processing. Buffers (delay lines, reverb tanks) are sized when a
//! processor is built, on the main thread, for the output's sample rate.

pub mod biquad;
pub(crate) mod delay;
pub mod dynamics;
pub mod meter;
pub(crate) mod reverb;
pub(crate) mod simple;

use crate::effect::EffectKind;
use kira::Frame;

/// The quietest level the mixer tells apart from silence, in decibels.
pub const SILENCE_DB: f32 = -100.0;

/// Decibels to a linear gain; [`SILENCE_DB`] and below are 0.
pub fn db_to_gain(db: f32) -> f32 {
    if db <= SILENCE_DB || db.is_nan() {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// A linear gain to decibels, floored at [`SILENCE_DB`].
pub fn gain_to_db(gain: f32) -> f32 {
    let gain = gain.abs();
    if gain <= 1e-5 || gain.is_nan() {
        SILENCE_DB
    } else {
        (20.0 * gain.log10()).max(SILENCE_DB)
    }
}

/// Coefficient of a one-pole smoother reaching ~63% of a step in `seconds`.
pub(crate) fn time_coefficient(seconds: f32, sample_rate: f32) -> f32 {
    if seconds <= 0.0 || sample_rate <= 0.0 {
        0.0
    } else {
        (-1.0 / (seconds * sample_rate)).exp()
    }
}

/// A number from settings made safe for the audio thread: finite and within `min..=max`.
pub(crate) fn sane(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

/// What a processor needs to know besides its input.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Context {
    /// The level of the processor's sidechain bus this block, as a linear peak (`None`
    /// without a sidechain).
    pub key: Option<f32>,
}

/// One effect in a chain.
pub(crate) trait Processor: Send {
    /// Takes new settings of the same kind and shape (see [`EffectKind::same_shape`]).
    /// Must not allocate.
    fn configure(&mut self, settings: &EffectKind, sample_rate: f32);

    fn process(&mut self, frames: &mut [Frame], context: &Context);

    /// Forgets the signal it has seen (tails, envelopes), as after a seek.
    fn reset(&mut self);

    /// How much the processor turned the signal down during the last block, in dB (≥ 0).
    fn gain_reduction_db(&self) -> f32 {
        0.0
    }
}

/// Builds the processor for `kind`, sized for `sample_rate`. Allocates.
pub(crate) fn build(kind: &EffectKind, sample_rate: f32) -> Box<dyn Processor> {
    let mut processor: Box<dyn Processor> = match kind {
        EffectKind::Eq(eq) => Box::new(biquad::Equalizer::new(eq.bands.len())),
        EffectKind::Filter(_) => Box::new(biquad::Equalizer::new(1)),
        EffectKind::Compressor(_) => Box::new(dynamics::Compressor::default()),
        EffectKind::Limiter(_) => Box::new(dynamics::Limiter::new(sample_rate)),
        EffectKind::Reverb(_) => Box::new(reverb::Reverb::new(sample_rate)),
        EffectKind::Delay(_) => Box::new(delay::Delay::new(sample_rate)),
        EffectKind::Saturation(_) => Box::new(simple::Saturation::default()),
        EffectKind::Utility(_) => Box::new(simple::Utility::default()),
    };
    processor.configure(kind, sample_rate);
    processor
}

/// Mid/side helpers for stereo width.
pub(crate) fn widen(frame: Frame, width: f32) -> Frame {
    let mid = (frame.left + frame.right) * 0.5;
    let side = (frame.left - frame.right) * 0.5 * width;
    Frame {
        left: mid + side,
        right: mid - side,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::f32::consts::TAU;

    pub(crate) const RATE: f32 = 48_000.0;

    /// `seconds` of a stereo sine at `freq` Hz and linear `amplitude`.
    pub(crate) fn sine(freq: f32, amplitude: f32, seconds: f32) -> Vec<Frame> {
        let n = (seconds * RATE) as usize;
        (0..n)
            .map(|i| Frame::from_mono((TAU * freq * i as f32 / RATE).sin() * amplitude))
            .collect()
    }

    pub(crate) fn peak(frames: &[Frame]) -> f32 {
        frames
            .iter()
            .map(|f| f.left.abs().max(f.right.abs()))
            .fold(0.0, f32::max)
    }

    /// Runs `frames` through `processor` in kira-sized blocks.
    pub(crate) fn run(processor: &mut dyn Processor, frames: &mut [Frame], key: Option<f32>) {
        let context = Context { key };
        for block in frames.chunks_mut(128) {
            processor.process(block, &context);
        }
    }

    #[test]
    fn decibels_round_trip_and_floor_at_silence() {
        assert!((db_to_gain(-6.0206) - 0.5).abs() < 1e-4);
        assert!((gain_to_db(0.5) + 6.0206).abs() < 1e-3);
        assert_eq!(db_to_gain(SILENCE_DB), 0.0);
        assert_eq!(gain_to_db(0.0), SILENCE_DB);
        assert_eq!(db_to_gain(f32::NAN), 0.0);
    }

    #[test]
    fn width_zero_is_mono_and_one_is_unchanged() {
        let frame = Frame {
            left: 1.0,
            right: 0.0,
        };
        let mono = widen(frame, 0.0);
        assert_eq!(mono.left, mono.right);
        assert_eq!(widen(frame, 1.0).left, 1.0);
    }
}
