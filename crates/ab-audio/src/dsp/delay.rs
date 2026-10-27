//! Echo with feedback, optionally bouncing between the sides.

use super::biquad::{Coefficients, StereoFilter};
use super::{sane, time_coefficient, Context, Processor};
use crate::effect::{DelaySettings, EffectKind, FilterShape};
use kira::Frame;

#[derive(Debug)]
pub(crate) struct Delay {
    lines: [Vec<f32>; 2],
    write: usize,
    /// Delay in frames, gliding towards `target` so time changes do not click.
    current: f32,
    target: f32,
    glide: f32,
    feedback: f32,
    mix: f32,
    ping_pong: bool,
    damping: StereoFilter,
}

impl Delay {
    pub fn new(sample_rate: f32) -> Self {
        let len = (DelaySettings::MAX_TIME_MS / 1000.0 * sample_rate.max(1.0)) as usize + 4;
        Self {
            lines: [vec![0.0; len], vec![0.0; len]],
            write: 0,
            current: 1.0,
            target: 1.0,
            glide: time_coefficient(0.05, sample_rate),
            feedback: 0.0,
            mix: 0.0,
            ping_pong: false,
            damping: StereoFilter::default(),
        }
    }

    /// The line's value `delay` frames (fractional) before the write head.
    #[inline]
    fn read(&self, channel: usize, delay: f32) -> f32 {
        let line = &self.lines[channel];
        let len = line.len();
        let position = self.write as f32 + len as f32 - delay;
        let index = position.floor();
        let frac = position - index;
        let a = line[index as usize % len];
        let b = line[(index as usize + 1) % len];
        a + (b - a) * frac
    }
}

impl Processor for Delay {
    fn configure(&mut self, settings: &EffectKind, sample_rate: f32) {
        let EffectKind::Delay(s) = settings else {
            return;
        };
        let max = (self.lines[0].len() - 3) as f32;
        let time = sane(s.time_ms, 1.0, DelaySettings::MAX_TIME_MS, 350.0);
        self.target = (time / 1000.0 * sample_rate).clamp(1.0, max);
        self.feedback = sane(s.feedback, 0.0, 0.95, 0.0);
        self.mix = sane(s.mix, 0.0, 1.0, 0.0);
        self.ping_pong = s.ping_pong;
        self.glide = time_coefficient(0.05, sample_rate);
        let high_cut = sane(s.high_cut_hz, 500.0, 20_000.0, 20_000.0);
        // At the top of the range the echoes stay as bright as the input.
        self.damping.set(if high_cut >= 20_000.0 {
            Coefficients::IDENTITY
        } else {
            Coefficients::new(
                FilterShape::HighCut,
                high_cut,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                sample_rate,
            )
        });
    }

    fn process(&mut self, frames: &mut [Frame], _: &Context) {
        let len = self.lines[0].len();
        for frame in frames.iter_mut() {
            self.current = self.target + (self.current - self.target) * self.glide;
            let wet_l = self.read(0, self.current);
            let wet_r = self.read(1, self.current);
            let back_l = self.damping.tick(0, wet_l) * self.feedback;
            let back_r = self.damping.tick(1, wet_r) * self.feedback;
            let (in_l, in_r) = if self.ping_pong {
                ((frame.left + frame.right) * 0.5 + back_r, back_l)
            } else {
                (frame.left + back_l, frame.right + back_r)
            };
            self.lines[0][self.write] = in_l;
            self.lines[1][self.write] = in_r;
            self.write = (self.write + 1) % len;
            frame.left = frame.left * (1.0 - self.mix) + wet_l * self.mix;
            frame.right = frame.right * (1.0 - self.mix) + wet_r * self.mix;
        }
        self.damping.flush();
    }

    fn reset(&mut self) {
        for line in &mut self.lines {
            line.fill(0.0);
        }
        self.damping.reset();
        self.current = self.target;
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{run, RATE};
    use super::*;

    fn delay(settings: DelaySettings) -> Delay {
        let mut d = Delay::new(RATE);
        d.configure(&EffectKind::Delay(settings), RATE);
        d.reset();
        d
    }

    #[test]
    fn an_impulse_comes_back_after_the_delay_time_quieter_each_time() {
        let mut d = delay(DelaySettings {
            time_ms: 100.0,
            feedback: 0.5,
            mix: 1.0,
            high_cut_hz: 20_000.0,
            ping_pong: false,
        });
        let mut signal = vec![Frame::ZERO; (RATE * 0.35) as usize];
        signal[0] = Frame::from_mono(1.0);
        run(&mut d, &mut signal, None);
        let at = |ms: f32| {
            let i = (ms / 1000.0 * RATE) as usize;
            signal[i - 2..i + 3]
                .iter()
                .map(|f| f.left.abs())
                .fold(0.0, f32::max)
        };
        assert!(at(100.0) > 0.9);
        assert!((at(200.0) - 0.5).abs() < 0.1);
        assert!(at(150.0) < 1e-3);
    }

    #[test]
    fn ping_pong_alternates_sides() {
        let mut d = delay(DelaySettings {
            time_ms: 50.0,
            feedback: 0.8,
            mix: 1.0,
            high_cut_hz: 20_000.0,
            ping_pong: true,
        });
        let mut signal = vec![Frame::ZERO; (RATE * 0.12) as usize];
        signal[0] = Frame::from_mono(1.0);
        run(&mut d, &mut signal, None);
        let first = signal[(RATE * 0.05) as usize];
        let second = signal[(RATE * 0.1) as usize];
        assert!(first.left > 0.5 && first.right.abs() < 1e-3);
        assert!(second.right > 0.5 && second.left.abs() < 1e-3);
    }
}
