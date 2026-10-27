//! Algorithmic reverb (Schroeder–Moorer, as in Jezar's Freeverb): eight damped combs and
//! four all-passes per side, with pre-delay and a band-limited tail.

use super::biquad::{Coefficients, StereoFilter};
use super::{sane, Context, Processor};
use crate::effect::{EffectKind, FilterShape, ReverbSettings};
use kira::Frame;

/// Comb and all-pass lengths at 44.1 kHz (Freeverb's tuning).
const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
/// The right side's lines are this much longer, which decorrelates the two.
const STEREO_SPREAD: usize = 23;
const INPUT_GAIN: f32 = 0.015;

#[derive(Debug)]
struct Comb {
    buffer: Vec<f32>,
    index: usize,
    store: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len.max(1)],
            index: 0,
            store: 0.0,
        }
    }

    #[inline]
    fn tick(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let out = self.buffer[self.index];
        self.store = out * (1.0 - damp) + self.store * damp;
        if self.store.abs() < 1e-20 {
            self.store = 0.0;
        }
        self.buffer[self.index] = input + self.store * feedback;
        self.index = (self.index + 1) % self.buffer.len();
        out
    }
}

#[derive(Debug)]
struct AllPass {
    buffer: Vec<f32>,
    index: usize,
}

impl AllPass {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len.max(1)],
            index: 0,
        }
    }

    #[inline]
    fn tick(&mut self, input: f32) -> f32 {
        let buffered = self.buffer[self.index];
        let mut stored = input + buffered * 0.5;
        if stored.abs() < 1e-20 {
            stored = 0.0;
        }
        self.buffer[self.index] = stored;
        self.index = (self.index + 1) % self.buffer.len();
        buffered - input
    }
}

#[derive(Debug)]
struct Side {
    combs: Vec<Comb>,
    allpasses: Vec<AllPass>,
}

impl Side {
    fn new(scale: f32, extra: usize) -> Self {
        let len = |n: usize| ((n + extra) as f32 * scale).round() as usize;
        Self {
            combs: COMBS.iter().map(|&n| Comb::new(len(n))).collect(),
            allpasses: ALLPASSES.iter().map(|&n| AllPass::new(len(n))).collect(),
        }
    }

    #[inline]
    fn tick(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let mut out = 0.0;
        for comb in &mut self.combs {
            out += comb.tick(input, feedback, damp);
        }
        for allpass in &mut self.allpasses {
            out = allpass.tick(out);
        }
        out
    }

    fn clear(&mut self) {
        for comb in &mut self.combs {
            comb.buffer.fill(0.0);
            comb.store = 0.0;
        }
        for allpass in &mut self.allpasses {
            allpass.buffer.fill(0.0);
        }
    }
}

#[derive(Debug)]
pub(crate) struct Reverb {
    left: Side,
    right: Side,
    pre_delay: Vec<f32>,
    pre_index: usize,
    pre_len: usize,
    feedback: f32,
    damp: f32,
    width: f32,
    mix: f32,
    /// Low cut, then high cut, on the wet signal.
    tone: [StereoFilter; 2],
}

impl Reverb {
    pub fn new(sample_rate: f32) -> Self {
        let scale = sample_rate.max(1.0) / 44_100.0;
        let pre = (ReverbSettings::MAX_PRE_DELAY_MS / 1000.0 * sample_rate.max(1.0)) as usize + 1;
        Self {
            left: Side::new(scale, 0),
            right: Side::new(scale, STEREO_SPREAD),
            pre_delay: vec![0.0; pre],
            pre_index: 0,
            pre_len: 0,
            feedback: 0.84,
            damp: 0.2,
            width: 1.0,
            mix: 1.0,
            tone: [StereoFilter::default(); 2],
        }
    }
}

impl Processor for Reverb {
    fn configure(&mut self, settings: &EffectKind, sample_rate: f32) {
        let EffectKind::Reverb(s) = settings else {
            return;
        };
        let room = sane(s.room_size, 0.0, 1.0, 0.6);
        self.feedback = room * 0.28 + 0.7;
        self.damp = sane(s.damping, 0.0, 1.0, 0.5) * 0.4;
        self.width = sane(s.width, 0.0, 1.0, 1.0);
        self.mix = sane(s.mix, 0.0, 1.0, 1.0);
        let pre = sane(s.pre_delay_ms, 0.0, ReverbSettings::MAX_PRE_DELAY_MS, 0.0);
        self.pre_len = ((pre / 1000.0 * sample_rate) as usize).min(self.pre_delay.len() - 1);
        let q = std::f32::consts::FRAC_1_SQRT_2;
        self.tone[0].set(Coefficients::new(
            FilterShape::LowCut,
            sane(s.low_cut_hz, 20.0, 2_000.0, 20.0),
            q,
            0.0,
            sample_rate,
        ));
        self.tone[1].set(Coefficients::new(
            FilterShape::HighCut,
            sane(s.high_cut_hz, 1_000.0, 20_000.0, 20_000.0),
            q,
            0.0,
            sample_rate,
        ));
    }

    fn process(&mut self, frames: &mut [Frame], _: &Context) {
        let wet1 = self.width / 2.0 + 0.5;
        let wet2 = (1.0 - self.width) / 2.0;
        let len = self.pre_delay.len();
        for frame in frames.iter_mut() {
            let mono = (frame.left + frame.right) * INPUT_GAIN;
            self.pre_delay[self.pre_index] = mono;
            let read = (self.pre_index + len - self.pre_len) % len;
            let input = self.pre_delay[read];
            self.pre_index = (self.pre_index + 1) % len;

            let l = self.left.tick(input, self.feedback, self.damp);
            let r = self.right.tick(input, self.feedback, self.damp);
            // Freeverb's wet scale (3) restores unity-ish loudness.
            let (mut wet_l, mut wet_r) = ((l * wet1 + r * wet2) * 3.0, (r * wet1 + l * wet2) * 3.0);
            for filter in &mut self.tone {
                wet_l = filter.tick(0, wet_l);
                wet_r = filter.tick(1, wet_r);
            }
            frame.left = frame.left * (1.0 - self.mix) + wet_l * self.mix;
            frame.right = frame.right * (1.0 - self.mix) + wet_r * self.mix;
        }
        self.tone.iter_mut().for_each(StereoFilter::flush);
    }

    fn reset(&mut self) {
        self.left.clear();
        self.right.clear();
        self.pre_delay.fill(0.0);
        self.tone.iter_mut().for_each(StereoFilter::reset);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{peak, run, RATE};
    use super::*;

    #[test]
    fn an_impulse_leaves_a_tail_that_dies_away() {
        let mut reverb = Reverb::new(RATE);
        reverb.configure(
            &EffectKind::Reverb(ReverbSettings {
                pre_delay_ms: 0.0,
                ..Default::default()
            }),
            RATE,
        );
        let mut signal = vec![Frame::ZERO; (RATE * 4.0) as usize];
        signal[0] = Frame::from_mono(1.0);
        run(&mut reverb, &mut signal, None);
        let second = |s: usize| peak(&signal[s * RATE as usize..(s + 1) * RATE as usize]);
        assert!(second(0) > 0.01, "a tail");
        assert!(second(3) < second(0) * 0.1, "that decays");
        assert!(signal.iter().all(|f| f.left.is_finite()));
    }

    #[test]
    fn pre_delay_holds_the_tail_back() {
        let mut reverb = Reverb::new(RATE);
        reverb.configure(
            &EffectKind::Reverb(ReverbSettings {
                pre_delay_ms: 100.0,
                ..Default::default()
            }),
            RATE,
        );
        let mut signal = vec![Frame::ZERO; (RATE * 0.2) as usize];
        signal[0] = Frame::from_mono(1.0);
        run(&mut reverb, &mut signal, None);
        // The shortest comb adds ~25 ms on top of the pre-delay.
        assert_eq!(peak(&signal[1..(RATE * 0.1) as usize]), 0.0);
        assert!(peak(&signal[(RATE * 0.1) as usize..]) > 0.0);
    }
}
