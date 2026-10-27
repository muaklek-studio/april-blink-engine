//! Compressor (with sidechain) and look-ahead limiter.

use super::{db_to_gain, gain_to_db, sane, time_coefficient, Context, Processor, SILENCE_DB};
use crate::effect::{CompressorSettings, EffectKind, LimiterSettings};
use kira::Frame;

/// Output level of the compressor's static curve for an input level, both in dB.
pub fn compressor_curve(input_db: f32, threshold_db: f32, ratio: f32, knee_db: f32) -> f32 {
    let over = input_db - threshold_db;
    let slope = 1.0 / ratio.max(1.0) - 1.0;
    if knee_db > 0.0 && over.abs() <= knee_db / 2.0 {
        input_db + slope * (over + knee_db / 2.0).powi(2) / (2.0 * knee_db)
    } else if over > 0.0 {
        input_db + slope * over
    } else {
        input_db
    }
}

/// Feed-forward compressor: stereo-linked peak detection, smoothing in the dB domain.
#[derive(Debug)]
pub(crate) struct Compressor {
    threshold_db: f32,
    ratio: f32,
    knee_db: f32,
    attack: f32,
    release: f32,
    makeup: f32,
    mix: f32,
    /// Current gain change, dB (≤ 0).
    envelope_db: f32,
    reduction_db: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self {
            threshold_db: 0.0,
            ratio: 1.0,
            knee_db: 0.0,
            attack: 0.0,
            release: 0.0,
            makeup: 1.0,
            mix: 1.0,
            envelope_db: 0.0,
            reduction_db: 0.0,
        }
    }
}

impl Compressor {
    fn set(&mut self, s: &CompressorSettings, sample_rate: f32) {
        self.threshold_db = sane(s.threshold_db, -80.0, 0.0, -18.0);
        self.ratio = sane(s.ratio, 1.0, 100.0, 4.0);
        self.knee_db = sane(s.knee_db, 0.0, 24.0, 0.0);
        self.attack = time_coefficient(sane(s.attack_ms, 0.0, 500.0, 10.0) / 1000.0, sample_rate);
        self.release = time_coefficient(
            sane(s.release_ms, 1.0, 5_000.0, 120.0) / 1000.0,
            sample_rate,
        );
        self.makeup = db_to_gain(sane(s.makeup_db, -24.0, 36.0, 0.0));
        self.mix = sane(s.mix, 0.0, 1.0, 1.0);
    }

    #[inline]
    fn follow(&mut self, level_db: f32) -> f32 {
        let target =
            compressor_curve(level_db, self.threshold_db, self.ratio, self.knee_db) - level_db;
        let coefficient = if target < self.envelope_db {
            self.attack
        } else {
            self.release
        };
        self.envelope_db = target + (self.envelope_db - target) * coefficient;
        self.envelope_db
    }
}

impl Processor for Compressor {
    fn configure(&mut self, settings: &EffectKind, sample_rate: f32) {
        if let EffectKind::Compressor(s) = settings {
            self.set(s, sample_rate);
        }
    }

    fn process(&mut self, frames: &mut [Frame], context: &Context) {
        let key_db = context.key.map(gain_to_db);
        let mut deepest = 0.0f32;
        for frame in frames.iter_mut() {
            let level_db =
                key_db.unwrap_or_else(|| gain_to_db(frame.left.abs().max(frame.right.abs())));
            let change_db = self.follow(level_db);
            deepest = deepest.min(change_db);
            let gain = db_to_gain(change_db) * self.makeup;
            let wet = *frame * gain;
            *frame = *frame * (1.0 - self.mix) + wet * self.mix;
        }
        self.reduction_db = -deepest;
    }

    fn reset(&mut self) {
        self.envelope_db = 0.0;
        self.reduction_db = 0.0;
    }

    fn gain_reduction_db(&self) -> f32 {
        self.reduction_db
    }
}

/// Fixed-capacity ring buffer of frames: the limiter's look-ahead delay.
#[derive(Debug)]
struct DelayLine {
    frames: Vec<Frame>,
    write: usize,
}

impl DelayLine {
    fn new(capacity: usize) -> Self {
        Self {
            frames: vec![Frame::ZERO; capacity.max(1)],
            write: 0,
        }
    }

    /// Pushes `input` and returns the frame from `delay` frames ago.
    #[inline]
    fn push(&mut self, input: Frame, delay: usize) -> Frame {
        let len = self.frames.len();
        self.frames[self.write] = input;
        let read = (self.write + len - delay.min(len - 1)) % len;
        self.write = (self.write + 1) % len;
        self.frames[read]
    }
}

/// Minimum of the last `window` values in O(1) amortized: a monotonic queue over a ring.
#[derive(Debug)]
struct RunningMin {
    /// (sample index, value), values increasing from front to back.
    queue: Vec<(u64, f32)>,
    head: usize,
    len: usize,
}

impl RunningMin {
    fn new(capacity: usize) -> Self {
        Self {
            queue: vec![(0, 0.0); capacity.max(1) + 1],
            head: 0,
            len: 0,
        }
    }

    fn at(&self, i: usize) -> (u64, f32) {
        self.queue[(self.head + i) % self.queue.len()]
    }

    fn push(&mut self, index: u64, value: f32, window: u64) -> f32 {
        let cap = self.queue.len();
        while self.len > 0 && self.at(self.len - 1).1 >= value {
            self.len -= 1;
        }
        if self.len == cap {
            self.head = (self.head + 1) % cap;
            self.len -= 1;
        }
        let slot = (self.head + self.len) % cap;
        self.queue[slot] = (index, value);
        self.len += 1;
        while self.len > 1 && self.at(0).0 + window <= index {
            self.head = (self.head + 1) % cap;
            self.len -= 1;
        }
        self.at(0).1
    }

    fn clear(&mut self) {
        self.len = 0;
    }
}

/// Look-ahead peak limiter: the gain each frame needs is known `lookahead` frames before
/// the frame is heard, so the gain glides down in time; a final clip keeps the ceiling
/// whatever happens.
#[derive(Debug)]
pub(crate) struct Limiter {
    delay: DelayLine,
    minimum: RunningMin,
    lookahead: usize,
    input: f32,
    ceiling: f32,
    attack: f32,
    release: f32,
    gain: f32,
    index: u64,
    reduction_db: f32,
}

impl Limiter {
    pub fn new(sample_rate: f32) -> Self {
        let capacity =
            (LimiterSettings::MAX_LOOKAHEAD_MS / 1000.0 * sample_rate.max(1.0)).ceil() as usize + 1;
        Self {
            delay: DelayLine::new(capacity + 1),
            minimum: RunningMin::new(capacity + 1),
            lookahead: 1,
            input: 1.0,
            ceiling: 1.0,
            attack: 0.0,
            release: 0.0,
            gain: 1.0,
            index: 0,
            reduction_db: 0.0,
        }
    }
}

impl Processor for Limiter {
    fn configure(&mut self, settings: &EffectKind, sample_rate: f32) {
        let EffectKind::Limiter(s) = settings else {
            return;
        };
        let lookahead_ms = sane(s.lookahead_ms, 0.5, LimiterSettings::MAX_LOOKAHEAD_MS, 3.0);
        let lookahead = (lookahead_ms / 1000.0 * sample_rate).round() as usize;
        self.lookahead = lookahead.clamp(1, self.delay.frames.len() - 1);
        // Reach the needed gain within the look-ahead: a third of it per time constant.
        self.attack = time_coefficient(self.lookahead as f32 / 3.0 / sample_rate, sample_rate);
        self.release =
            time_coefficient(sane(s.release_ms, 1.0, 2_000.0, 80.0) / 1000.0, sample_rate);
        self.input = db_to_gain(sane(s.input_db, -24.0, 36.0, 0.0));
        self.ceiling = db_to_gain(sane(s.ceiling_db, -30.0, 0.0, -0.3)).max(1e-3);
    }

    fn process(&mut self, frames: &mut [Frame], _: &Context) {
        let mut lowest = 1.0f32;
        for frame in frames.iter_mut() {
            let input = *frame * self.input;
            let peak = input.left.abs().max(input.right.abs());
            let needed = if peak > self.ceiling {
                self.ceiling / peak
            } else {
                1.0
            };
            self.index += 1;
            let target = self
                .minimum
                .push(self.index, needed, self.lookahead as u64 + 1);
            let coefficient = if target < self.gain {
                self.attack
            } else {
                self.release
            };
            self.gain = target + (self.gain - target) * coefficient;
            lowest = lowest.min(self.gain);
            let out = self.delay.push(input, self.lookahead) * self.gain;
            frame.left = out.left.clamp(-self.ceiling, self.ceiling);
            frame.right = out.right.clamp(-self.ceiling, self.ceiling);
        }
        self.reduction_db = (-gain_to_db(lowest)).clamp(0.0, -SILENCE_DB);
    }

    fn reset(&mut self) {
        self.delay.frames.fill(Frame::ZERO);
        self.minimum.clear();
        self.gain = 1.0;
        self.reduction_db = 0.0;
    }

    fn gain_reduction_db(&self) -> f32 {
        self.reduction_db
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{peak, run, sine, RATE};
    use super::*;

    fn compressor(settings: CompressorSettings) -> Compressor {
        let mut c = Compressor::default();
        c.configure(&EffectKind::Compressor(settings), RATE);
        c
    }

    #[test]
    fn the_curve_divides_what_is_over_the_threshold() {
        assert_eq!(compressor_curve(-30.0, -20.0, 4.0, 0.0), -30.0);
        assert_eq!(compressor_curve(-8.0, -20.0, 4.0, 0.0), -17.0);
        // The knee is continuous with both sides.
        let below = compressor_curve(-23.0, -20.0, 4.0, 6.0);
        let above = compressor_curve(-17.0, -20.0, 4.0, 6.0);
        assert!((below + 23.0).abs() < 1e-4);
        assert!((above - compressor_curve(-17.0, -20.0, 4.0, 0.0)).abs() < 1e-4);
    }

    #[test]
    fn a_loud_signal_settles_at_the_ratio() {
        let mut c = compressor(CompressorSettings {
            threshold_db: -20.0,
            ratio: 4.0,
            knee_db: 0.0,
            attack_ms: 1.0,
            release_ms: 50.0,
            ..Default::default()
        });
        // 0 dBFS in: 20 dB over → 5 dB over → -15 dBFS out.
        let mut signal = sine(1_000.0, 1.0, 0.5);
        run(&mut c, &mut signal, None);
        let out = gain_to_db(peak(&signal[signal.len() - 4_800..]));
        assert!((out + 15.0).abs() < 1.0, "{out}");
        assert!((c.gain_reduction_db() - 15.0).abs() < 1.0);
    }

    #[test]
    fn a_sidechain_ducks_a_quiet_bus_under_a_loud_one() {
        let mut c = compressor(CompressorSettings {
            threshold_db: -30.0,
            ratio: 10.0,
            knee_db: 0.0,
            attack_ms: 5.0,
            release_ms: 50.0,
            sidechain: Some("Voice".into()),
            ..Default::default()
        });
        let mut music = sine(200.0, 0.1, 0.5);
        run(&mut c, &mut music, Some(1.0));
        let ducked = gain_to_db(peak(&music[music.len() - 4_800..]));
        assert!(ducked < -40.0, "music at {ducked} dB under a 0 dB voice");

        let mut music = sine(200.0, 0.1, 0.5);
        run(&mut c, &mut music, Some(0.0));
        let free = gain_to_db(peak(&music[music.len() - 4_800..]));
        assert!(
            (free + 20.0).abs() < 0.5,
            "back once the voice stops: {free}"
        );
    }

    #[test]
    fn the_limiter_never_lets_a_peak_past_the_ceiling() {
        let mut limiter = Limiter::new(RATE);
        limiter.configure(
            &EffectKind::Limiter(LimiterSettings {
                input_db: 12.0,
                ceiling_db: -1.0,
                ..Default::default()
            }),
            RATE,
        );
        let mut signal = sine(80.0, 0.9, 0.5);
        // A sudden spike.
        signal[10_000] = Frame::from_mono(1.0);
        run(&mut limiter, &mut signal, None);
        assert!(peak(&signal) <= db_to_gain(-1.0) + 1e-6);
        assert!(limiter.gain_reduction_db() > 10.0);
    }

    #[test]
    fn the_limiter_leaves_quiet_signal_alone_after_its_delay() {
        let mut limiter = Limiter::new(RATE);
        limiter.configure(&EffectKind::Limiter(LimiterSettings::default()), RATE);
        let original = sine(440.0, 0.25, 0.1);
        let mut signal = original.clone();
        run(&mut limiter, &mut signal, None);
        let delay = limiter.lookahead;
        for (out, input) in signal[delay..].iter().zip(&original) {
            assert!((out.left - input.left).abs() < 1e-6);
        }
    }

    #[test]
    fn the_running_minimum_forgets_old_values() {
        let mut m = RunningMin::new(4);
        assert_eq!(m.push(1, 0.5, 3), 0.5);
        assert_eq!(m.push(2, 0.9, 3), 0.5);
        assert_eq!(m.push(3, 0.8, 3), 0.5);
        assert_eq!(m.push(4, 1.0, 3), 0.8, "0.5 left the window");
        assert_eq!(m.push(5, 1.0, 3), 0.8);
        assert_eq!(m.push(6, 1.0, 3), 1.0);
    }
}
