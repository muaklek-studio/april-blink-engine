//! Stateless effects: saturation and the gain/pan/width utility.

use super::{db_to_gain, sane, widen, Context, Processor};
use crate::effect::EffectKind;
use kira::Frame;

/// Soft clipper (tanh), level-matched so more drive sounds denser rather than louder.
#[derive(Debug)]
pub(crate) struct Saturation {
    drive: f32,
    mix: f32,
    output: f32,
}

impl Default for Saturation {
    fn default() -> Self {
        Self {
            drive: 1.0,
            mix: 1.0,
            output: 1.0,
        }
    }
}

impl Processor for Saturation {
    fn configure(&mut self, settings: &EffectKind, _: f32) {
        let EffectKind::Saturation(s) = settings else {
            return;
        };
        self.drive = db_to_gain(sane(s.drive_db, 0.0, 48.0, 0.0));
        self.mix = sane(s.mix, 0.0, 1.0, 1.0);
        self.output = db_to_gain(sane(s.output_db, -48.0, 12.0, 0.0));
    }

    fn process(&mut self, frames: &mut [Frame], _: &Context) {
        // tanh(drive·x) / tanh(drive) keeps a full-scale peak at full scale.
        let norm = 1.0 / self.drive.tanh().max(1e-6);
        for frame in frames.iter_mut() {
            let shape = |x: f32| (x * self.drive).tanh() * norm;
            let wet = Frame {
                left: shape(frame.left),
                right: shape(frame.right),
            };
            *frame = (*frame * (1.0 - self.mix) + wet * self.mix) * self.output;
        }
    }

    fn reset(&mut self) {}
}

/// Gain, balance (pan of a stereo signal) and stereo width.
#[derive(Debug)]
pub(crate) struct Utility {
    gain: [f32; 2],
    width: f32,
}

impl Default for Utility {
    fn default() -> Self {
        Self {
            gain: [1.0, 1.0],
            width: 1.0,
        }
    }
}

impl Processor for Utility {
    fn configure(&mut self, settings: &EffectKind, _: f32) {
        let EffectKind::Utility(s) = settings else {
            return;
        };
        let gain = db_to_gain(sane(s.gain_db, -96.0, 24.0, 0.0));
        let pan = sane(s.pan, -1.0, 1.0, 0.0);
        // Balance: the far side fades out, the near side stays (no boost, no clipping).
        self.gain = [gain * (1.0 - pan.max(0.0)), gain * (1.0 + pan.min(0.0))];
        self.width = sane(s.width, 0.0, 2.0, 1.0);
    }

    fn process(&mut self, frames: &mut [Frame], _: &Context) {
        for frame in frames.iter_mut() {
            let wide = widen(*frame, self.width);
            frame.left = wide.left * self.gain[0];
            frame.right = wide.right * self.gain[1];
        }
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::super::tests::{peak, run, sine};
    use super::*;
    use crate::effect::{SaturationSettings, UtilitySettings};

    #[test]
    fn saturation_keeps_full_scale_and_squashes_the_rest() {
        let mut s = Saturation::default();
        s.configure(
            &EffectKind::Saturation(SaturationSettings {
                drive_db: 18.0,
                mix: 1.0,
                output_db: 0.0,
            }),
            48_000.0,
        );
        let mut loud = sine(100.0, 1.0, 0.05);
        run(&mut s, &mut loud, None);
        assert!((peak(&loud) - 1.0).abs() < 1e-3);
        let mut quiet = sine(100.0, 0.1, 0.05);
        run(&mut s, &mut quiet, None);
        assert!(peak(&quiet) > 0.5, "quiet parts come up");
    }

    #[test]
    fn panning_hard_left_silences_the_right() {
        let mut u = Utility::default();
        u.configure(
            &EffectKind::Utility(UtilitySettings {
                pan: -1.0,
                ..Default::default()
            }),
            48_000.0,
        );
        let mut signal = sine(100.0, 0.5, 0.02);
        run(&mut u, &mut signal, None);
        assert!(signal.iter().all(|f| f.right.abs() < 1e-6));
        assert!((peak(&signal) - 0.5).abs() < 1e-3);
    }
}
