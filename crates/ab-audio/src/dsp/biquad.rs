//! Second-order filters (Robert Bristow-Johnson's cookbook) and the equalizer built from
//! them.

use super::{db_to_gain, sane, Context, Processor};
use crate::effect::{EffectKind, EqBand, FilterShape};
use kira::Frame;
use std::f64::consts::PI;

/// Normalized biquad coefficients (`a0` = 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Coefficients {
    /// Passes everything unchanged.
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// One stage of `shape` at `freq` Hz.
    pub fn new(shape: FilterShape, freq: f32, q: f32, gain_db: f32, sample_rate: f32) -> Self {
        let rate = f64::from(sample_rate.max(1.0));
        let freq = f64::from(sane(freq, 1.0, 100_000.0, 1_000.0)).min(rate * 0.49);
        let q = f64::from(sane(q, 0.025, 40.0, 0.707));
        let gain_db = f64::from(sane(gain_db, -48.0, 48.0, 0.0));
        let w0 = 2.0 * PI * freq / rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a = 10f64.powf(gain_db / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match shape {
            FilterShape::HighCut => (
                (1.0 - cos) / 2.0,
                1.0 - cos,
                (1.0 - cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            FilterShape::LowCut => (
                (1.0 + cos) / 2.0,
                -(1.0 + cos),
                (1.0 + cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            FilterShape::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            FilterShape::Notch => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            FilterShape::Peak => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
            FilterShape::LowShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos + s),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - s),
                    (a + 1.0) + (a - 1.0) * cos + s,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - s,
                )
            }
            FilterShape::HighShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + s),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - s),
                    (a + 1.0) - (a - 1.0) * cos + s,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - s,
                )
            }
        };
        Self {
            b0: (b0 / a0) as f32,
            b1: (b1 / a0) as f32,
            b2: (b2 / a0) as f32,
            a1: (a1 / a0) as f32,
            a2: (a2 / a0) as f32,
        }
    }

    /// Gain at `freq` Hz, in dB.
    pub fn response_db(&self, freq: f32, sample_rate: f32) -> f32 {
        let w = 2.0 * PI * f64::from(freq) / f64::from(sample_rate.max(1.0));
        // H(e^jw) = (b0 + b1 z^-1 + b2 z^-2) / (1 + a1 z^-1 + a2 z^-2)
        let (c1, s1) = (w.cos(), -w.sin());
        let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
        let [b0, b1, b2, a1, a2] = [self.b0, self.b1, self.b2, self.a1, self.a2].map(f64::from);
        let num = (b0 + b1 * c1 + b2 * c2, b1 * s1 + b2 * s2);
        let den = (1.0 + a1 * c1 + a2 * c2, a1 * s1 + a2 * s2);
        let magnitude = ((num.0 * num.0 + num.1 * num.1) / (den.0 * den.0 + den.1 * den.1)).sqrt();
        super::gain_to_db(magnitude as f32)
    }
}

/// The stages of one band: cut filters cascade Butterworth sections for steeper slopes,
/// with the band's Q as the resonance of the last.
pub fn band_stages(band: &EqBand, sample_rate: f32) -> ([Coefficients; MAX_STAGES], usize) {
    let mut stages = [Coefficients::IDENTITY; MAX_STAGES];
    let count = band.stages();
    if !band.enabled {
        return (stages, 0);
    }
    for (i, stage) in stages.iter_mut().enumerate().take(count) {
        let q = if band.shape.has_slope() {
            // Butterworth section i of an order-2n filter, the last carrying the resonance.
            let n = count as f32;
            let butterworth =
                1.0 / (2.0 * (std::f32::consts::PI * (2.0 * i as f32 + 1.0) / (4.0 * n)).cos());
            if i + 1 == count {
                butterworth * band.q / std::f32::consts::FRAC_1_SQRT_2
            } else {
                butterworth
            }
        } else {
            band.q
        };
        *stage = Coefficients::new(band.shape, band.freq, q, band.gain_db, sample_rate);
    }
    (stages, count)
}

/// Gain of a band at `freq` Hz, in dB (the equalizer's curve).
pub fn band_response_db(band: &EqBand, freq: f32, sample_rate: f32) -> f32 {
    let (stages, count) = band_stages(band, sample_rate);
    stages[..count]
        .iter()
        .map(|s| s.response_db(freq, sample_rate))
        .sum()
}

/// Most cascaded sections in a band (48 dB/oct).
pub const MAX_STAGES: usize = 4;

/// Transposed direct form II, one state pair per channel.
#[derive(Debug, Clone, Copy, Default)]
struct State {
    s1: [f32; 2],
    s2: [f32; 2],
}

impl State {
    #[inline]
    fn tick(&mut self, c: &Coefficients, channel: usize, x: f32) -> f32 {
        let y = c.b0 * x + self.s1[channel];
        self.s1[channel] = c.b1 * x - c.a1 * y + self.s2[channel];
        self.s2[channel] = c.b2 * x - c.a2 * y;
        y
    }

    /// Clears denormal leftovers of a decayed tail (they slow some CPUs to a crawl).
    fn flush(&mut self) {
        for v in self.s1.iter_mut().chain(self.s2.iter_mut()) {
            if v.abs() < 1e-20 {
                *v = 0.0;
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Band {
    coefficients: [Coefficients; MAX_STAGES],
    count: usize,
    state: [State; MAX_STAGES],
}

/// Equalizer of a fixed number of bands (also the one-band filter effect).
#[derive(Debug)]
pub(crate) struct Equalizer {
    bands: Vec<Band>,
    output: f32,
}

impl Equalizer {
    pub fn new(bands: usize) -> Self {
        Self {
            bands: vec![
                Band {
                    coefficients: [Coefficients::IDENTITY; MAX_STAGES],
                    count: 0,
                    state: [State::default(); MAX_STAGES],
                };
                bands
            ],
            output: 1.0,
        }
    }

    fn set_band(&mut self, index: usize, band: &EqBand, sample_rate: f32) {
        if let Some(slot) = self.bands.get_mut(index) {
            let (coefficients, count) = band_stages(band, sample_rate);
            slot.coefficients = coefficients;
            slot.count = count;
        }
    }
}

impl Processor for Equalizer {
    fn configure(&mut self, settings: &EffectKind, sample_rate: f32) {
        match settings {
            EffectKind::Eq(eq) => {
                for (i, band) in eq.bands.iter().enumerate() {
                    self.set_band(i, band, sample_rate);
                }
                self.output = db_to_gain(sane(eq.output_db, -48.0, 24.0, 0.0));
            }
            EffectKind::Filter(band) => {
                self.set_band(0, band, sample_rate);
                self.output = 1.0;
            }
            _ => {}
        }
    }

    fn process(&mut self, frames: &mut [Frame], _: &Context) {
        for band in &mut self.bands {
            for frame in frames.iter_mut() {
                let (mut l, mut r) = (frame.left, frame.right);
                for (c, state) in band.coefficients[..band.count].iter().zip(&mut band.state) {
                    l = state.tick(c, 0, l);
                    r = state.tick(c, 1, r);
                }
                frame.left = l;
                frame.right = r;
            }
            band.state.iter_mut().for_each(State::flush);
        }
        if self.output != 1.0 {
            for frame in frames.iter_mut() {
                *frame *= self.output;
            }
        }
    }

    fn reset(&mut self) {
        for band in &mut self.bands {
            band.state = [State::default(); MAX_STAGES];
        }
    }
}

/// One biquad stage over a stereo signal.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StereoFilter {
    coefficients: Coefficients,
    state: State,
}

impl Default for StereoFilter {
    fn default() -> Self {
        Self {
            coefficients: Coefficients::IDENTITY,
            state: State::default(),
        }
    }
}

impl StereoFilter {
    pub fn set(&mut self, coefficients: Coefficients) {
        self.coefficients = coefficients;
    }

    #[inline]
    pub fn tick(&mut self, channel: usize, x: f32) -> f32 {
        self.state.tick(&self.coefficients, channel, x)
    }

    pub fn flush(&mut self) {
        self.state.flush();
    }

    pub fn reset(&mut self) {
        self.state = State::default();
    }
}

/// A single low-pass for a voice (occlusion), cheap to retune every block.
#[derive(Debug, Clone, Default)]
pub(crate) struct VoiceFilter {
    coefficients: Option<Coefficients>,
    cutoff: f32,
    state: State,
}

impl VoiceFilter {
    /// Cutoffs at or above this pass everything untouched.
    pub const OPEN_HZ: f32 = 20_000.0;

    pub fn set_cutoff(&mut self, cutoff: f32, sample_rate: f32) {
        if cutoff == self.cutoff {
            return;
        }
        self.cutoff = cutoff;
        self.coefficients = (cutoff < Self::OPEN_HZ).then(|| {
            Coefficients::new(
                FilterShape::HighCut,
                cutoff,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                sample_rate,
            )
        });
    }

    pub fn process(&mut self, frames: &mut [Frame]) {
        let Some(c) = self.coefficients else {
            return;
        };
        for frame in frames.iter_mut() {
            frame.left = self.state.tick(&c, 0, frame.left);
            frame.right = self.state.tick(&c, 1, frame.right);
        }
        self.state.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{peak, run, sine, RATE};
    use super::*;
    use crate::effect::EqSettings;

    fn filter(shape: FilterShape, freq: f32, slope: u8) -> EqBand {
        EqBand {
            shape,
            freq,
            slope,
            ..EqBand::default()
        }
    }

    #[test]
    fn a_bell_boosts_its_centre_by_its_gain_and_leaves_far_frequencies() {
        let band = EqBand {
            gain_db: 6.0,
            ..filter(FilterShape::Peak, 1_000.0, 12)
        };
        assert!((band_response_db(&band, 1_000.0, RATE) - 6.0).abs() < 0.05);
        assert!(band_response_db(&band, 50.0, RATE).abs() < 0.2);
        assert!(band_response_db(&band, 15_000.0, RATE).abs() < 0.3);
    }

    #[test]
    fn cut_filters_are_3db_down_at_the_corner_and_get_steeper_with_the_slope() {
        for slope in [12, 24, 48] {
            let band = filter(FilterShape::HighCut, 1_000.0, slope);
            assert!(
                (band_response_db(&band, 1_000.0, RATE) + 3.01).abs() < 0.1,
                "{slope} dB/oct: Butterworth corner"
            );
            let octave_up = band_response_db(&band, 4_000.0, RATE);
            let expected = -(f32::from(slope) * 2.0);
            assert!(
                (octave_up - expected).abs() < 3.0,
                "{slope} dB/oct: {octave_up} two octaves up"
            );
        }
    }

    #[test]
    fn shelves_reach_their_gain_beyond_the_corner() {
        let low = EqBand {
            gain_db: -9.0,
            ..filter(FilterShape::LowShelf, 200.0, 12)
        };
        assert!((band_response_db(&low, 20.0, RATE) + 9.0).abs() < 0.2);
        assert!(band_response_db(&low, 10_000.0, RATE).abs() < 0.1);
    }

    #[test]
    fn the_equalizer_filters_real_signal() {
        let mut eq = Equalizer::new(1);
        let settings = EffectKind::Eq(EqSettings {
            bands: vec![filter(FilterShape::HighCut, 500.0, 48)],
            output_db: 0.0,
        });
        eq.configure(&settings, RATE);
        let mut low = sine(100.0, 0.5, 0.2);
        let mut high = sine(5_000.0, 0.5, 0.2);
        run(&mut eq, &mut low, None);
        eq.reset();
        run(&mut eq, &mut high, None);
        let settled = |f: &[Frame]| peak(&f[f.len() / 2..]);
        assert!((settled(&low) - 0.5).abs() < 0.02);
        assert!(settled(&high) < 0.001, "-80 dB and more");
    }

    #[test]
    fn a_disabled_band_passes_everything() {
        let band = EqBand {
            enabled: false,
            gain_db: 12.0,
            ..EqBand::default()
        };
        assert_eq!(band_response_db(&band, 1_000.0, RATE), 0.0);
    }

    #[test]
    fn nonsense_settings_stay_finite() {
        let band = EqBand {
            freq: f32::NAN,
            q: 0.0,
            gain_db: f32::INFINITY,
            ..EqBand::default()
        };
        let mut eq = Equalizer::new(1);
        eq.configure(&EffectKind::Filter(band), RATE);
        let mut signal = sine(440.0, 1.0, 0.05);
        run(&mut eq, &mut signal, None);
        assert!(signal
            .iter()
            .all(|f| f.left.is_finite() && f.right.is_finite()));
    }
}
