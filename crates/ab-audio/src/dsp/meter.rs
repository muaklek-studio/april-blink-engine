//! Levels a bus reports from the audio thread: meters for the mixer window, the key level
//! of sidechains and a window of recent signal for the spectrum display.

use kira::Frame;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

/// An `f32` shared between threads.
#[derive(Debug, Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(value: f32) -> Self {
        Self(AtomicU32::new(value.to_bits()))
    }

    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn store(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Raises the value to `value` if that is higher.
    pub fn raise(&self, value: f32) {
        let mut current = self.0.load(Ordering::Relaxed);
        while value > f32::from_bits(current) {
            match self.0.compare_exchange_weak(
                current,
                value.to_bits(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// The value, replaced by 0.
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Ordering::Relaxed))
    }
}

/// Samples kept for the spectrum display (a power of two).
pub const ANALYZER_LEN: usize = 4096;

/// A bus's levels. The audio thread writes them each block; readers never block it.
#[derive(Debug)]
pub struct Levels {
    /// Highest sample per side since the meter last read it.
    peak: [AtomicF32; 2],
    /// Mean square per side, smoothed over ~300 ms.
    mean_square: [AtomicF32; 2],
    /// Strongest gain reduction of the bus's dynamics since last read, dB.
    reduction_db: AtomicF32,
    /// A sample went past full scale since last read.
    clipped: AtomicBool,
    /// Peak of the last block, for sidechains.
    key: AtomicF32,
    /// Recent mono signal (a ring; `analyzer_write` is the next slot).
    analyzer: Mutex<Vec<f32>>,
    analyzer_write: AtomicU32,
    /// Spectrum capture is on only while someone looks at it.
    pub(crate) analyzing: AtomicBool,
    /// The output's sample rate (0 until the audio thread starts).
    pub(crate) sample_rate: AtomicU32,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            peak: Default::default(),
            mean_square: Default::default(),
            reduction_db: AtomicF32::new(0.0),
            clipped: AtomicBool::new(false),
            key: AtomicF32::new(0.0),
            analyzer: Mutex::new(vec![0.0; ANALYZER_LEN]),
            analyzer_write: AtomicU32::new(0),
            analyzing: AtomicBool::new(false),
            sample_rate: AtomicU32::new(0),
        }
    }
}

/// What a meter shows, read with [`Levels::read`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MeterReading {
    /// Peak per side since the last reading, linear.
    pub peak: [f32; 2],
    /// RMS per side, linear.
    pub rms: [f32; 2],
    /// Gain reduction of the bus's compressors and limiters, dB (≥ 0).
    pub reduction_db: f32,
    pub clipped: bool,
}

impl Levels {
    /// The levels since the last reading (peaks and clipping reset).
    pub fn read(&self) -> MeterReading {
        MeterReading {
            peak: [self.peak[0].take(), self.peak[1].take()],
            rms: [
                self.mean_square[0].load().max(0.0).sqrt(),
                self.mean_square[1].load().max(0.0).sqrt(),
            ],
            reduction_db: self.reduction_db.take(),
            clipped: self.clipped.swap(false, Ordering::Relaxed),
        }
    }

    /// Sample rate of the signal measured (48 kHz until the output reports its own).
    pub fn sample_rate(&self) -> u32 {
        match self.sample_rate.load(Ordering::Relaxed) {
            0 => 48_000,
            rate => rate,
        }
    }

    /// Peak of the last block, linear (what sidechains key from).
    pub fn key(&self) -> f32 {
        self.key.load()
    }

    /// The last [`ANALYZER_LEN`] samples (mono), oldest first, into `out`; also keeps
    /// capturing for the next reads.
    pub fn recent_signal(&self, out: &mut Vec<f32>) {
        self.analyzing.store(true, Ordering::Relaxed);
        out.clear();
        let Ok(ring) = self.analyzer.lock() else {
            return;
        };
        let start = self.analyzer_write.load(Ordering::Relaxed) as usize % ANALYZER_LEN;
        out.extend_from_slice(&ring[start..]);
        out.extend_from_slice(&ring[..start]);
    }

    pub(crate) fn report_reduction(&self, db: f32) {
        self.reduction_db.raise(db);
    }
}

/// Audio-thread side of a meter: smoothing state, written into [`Levels`] per block.
#[derive(Debug)]
pub(crate) struct Meter {
    mean_square: [f32; 2],
    coefficient: f32,
}

impl Meter {
    /// RMS integration time.
    const WINDOW: f32 = 0.3;

    pub fn new(sample_rate: f32) -> Self {
        Self {
            mean_square: [0.0; 2],
            coefficient: super::time_coefficient(Self::WINDOW, sample_rate),
        }
    }

    pub fn measure(&mut self, frames: &[Frame], levels: &Levels) {
        let mut peak = [0.0f32; 2];
        for frame in frames {
            peak[0] = peak[0].max(frame.left.abs());
            peak[1] = peak[1].max(frame.right.abs());
            let c = self.coefficient;
            self.mean_square[0] =
                frame.left * frame.left + (self.mean_square[0] - frame.left * frame.left) * c;
            self.mean_square[1] =
                frame.right * frame.right + (self.mean_square[1] - frame.right * frame.right) * c;
        }
        for (side, (mean_square, peak)) in self.mean_square.iter_mut().zip(peak).enumerate() {
            if *mean_square < 1e-20 {
                *mean_square = 0.0;
            }
            // A NaN from upstream must not stick in the meter.
            let p = if peak.is_finite() { peak } else { 0.0 };
            levels.peak[side].raise(p);
            levels.mean_square[side].store(*mean_square);
            if p > 1.0 {
                levels.clipped.store(true, Ordering::Relaxed);
            }
        }
        levels.key.store(peak[0].max(peak[1]));

        if levels.analyzing.load(Ordering::Relaxed) {
            if let Ok(mut ring) = levels.analyzer.try_lock() {
                let mut write = levels.analyzer_write.load(Ordering::Relaxed) as usize;
                for frame in frames {
                    ring[write % ANALYZER_LEN] = (frame.left + frame.right) * 0.5;
                    write = (write + 1) % ANALYZER_LEN;
                }
                levels.analyzer_write.store(write as u32, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::sine;
    use super::*;

    #[test]
    fn a_sine_reads_its_peak_and_rms() {
        let levels = Levels::default();
        let mut meter = Meter::new(48_000.0);
        let signal = sine(1_000.0, 0.5, 2.0);
        for block in signal.chunks(128) {
            meter.measure(block, &levels);
        }
        let reading = levels.read();
        assert!((reading.peak[0] - 0.5).abs() < 1e-3);
        assert!((reading.rms[0] - 0.5 / std::f32::consts::SQRT_2).abs() < 0.01);
        assert!(!reading.clipped);
        assert_eq!(levels.read().peak[0], 0.0, "peaks reset when read");
        assert!((levels.key() - 0.5).abs() < 0.01);
    }

    #[test]
    fn the_analyzer_returns_the_latest_samples_in_order() {
        let levels = Levels::default();
        let mut meter = Meter::new(48_000.0);
        let mut out = Vec::new();
        levels.recent_signal(&mut out);
        let ramp: Vec<Frame> = (0..ANALYZER_LEN + 100)
            .map(|i| Frame::from_mono(i as f32))
            .collect();
        for block in ramp.chunks(128) {
            meter.measure(block, &levels);
        }
        levels.recent_signal(&mut out);
        assert_eq!(out.len(), ANALYZER_LEN);
        assert_eq!(out[0], 100.0);
        assert_eq!(*out.last().unwrap(), (ANALYZER_LEN + 99) as f32);
    }
}
