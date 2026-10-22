//! Frame profiling: smoothed timings of the engine's frame sections and systems.

use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;

/// Weight of the newest sample in the smoothed averages.
const SMOOTHING: f32 = 0.1;
/// Frame deltas kept for FPS statistics (about two seconds at 60 fps).
const FRAME_WINDOW: usize = 120;
/// Frames kept in [`Profiler::history`] (five seconds at 60 fps).
pub const HISTORY: usize = 300;
/// How fast a peak falls back when no new maximum comes (per recorded sample).
const PEAK_DECAY: f32 = 0.995;

/// One measured section.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileEntry {
    pub name: String,
    /// Milliseconds in the last frame.
    pub last_ms: f32,
    /// Exponentially smoothed milliseconds.
    pub average_ms: f32,
    /// Recent maximum (decays slowly), to spot spikes the average hides.
    pub peak_ms: f32,
}

/// The main costs of one frame, for graphs.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameSample {
    /// Wall-clock time since the previous frame.
    pub delta_ms: f32,
    /// CPU time of the engine update.
    pub update_ms: f32,
    pub physics_ms: f32,
    pub scripts_ms: f32,
    /// Rendering of the previous frame (recorded by the window or the editor).
    pub render_ms: f32,
}

/// ECS resource: timings of the last frames. The engine records its sections (`frame`,
/// `pre_update`, `fixed`, `scripts.physics_process`, `update`, `scripts.process`,
/// `post_update`, `ui_layout`, `render`), every system (`system.<name>`) and every script
/// type (`script.<Type>`); games record their own with [`Profiler::record`] or
/// [`profile`](crate::profile).
#[derive(Default, Clone)]
pub struct Profiler {
    /// Freeze the numbers (to read them) without stopping the game.
    pub paused: bool,
    entries: Vec<ProfileEntry>,
    frame_deltas: VecDeque<f32>,
    history: VecDeque<FrameSample>,
}

/// Frame-rate statistics over the last couple of seconds.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameStatistics {
    pub fps: f32,
    pub average_ms: f32,
    pub min_ms: f32,
    pub max_ms: f32,
}

impl Profiler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a measurement for `name`.
    pub fn record(&mut self, name: &str, duration: Duration) {
        if self.paused {
            return;
        }
        let ms = duration.as_secs_f32() * 1000.0;
        match self.entries.iter_mut().find(|e| e.name == name) {
            Some(entry) => {
                entry.last_ms = ms;
                entry.average_ms += (ms - entry.average_ms) * SMOOTHING;
                entry.peak_ms = ms.max(entry.peak_ms * PEAK_DECAY);
            }
            None => self.entries.push(ProfileEntry {
                name: name.to_owned(),
                last_ms: ms,
                average_ms: ms,
                peak_ms: ms,
            }),
        }
    }

    /// Last-frame milliseconds of `name`.
    pub fn last_ms(&self, name: &str) -> Option<f32> {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.last_ms)
    }

    /// Adds a frame to the history (oldest dropped after [`HISTORY`] frames).
    pub fn push_sample(&mut self, sample: FrameSample) {
        if self.paused {
            return;
        }
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(sample);
    }

    /// The last frames, oldest first.
    pub fn history(&self) -> &VecDeque<FrameSample> {
        &self.history
    }

    /// Forgets every entry and sample.
    pub fn reset(&mut self) {
        self.entries.clear();
        self.frame_deltas.clear();
        self.history.clear();
    }

    /// Smoothed milliseconds of `name`.
    pub fn average_ms(&self, name: &str) -> Option<f32> {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.average_ms)
    }

    pub fn entries(&self) -> &[ProfileEntry] {
        &self.entries
    }

    /// Entries whose name starts with `prefix`, slowest first.
    pub fn slowest(&self, prefix: &str, count: usize) -> Vec<&ProfileEntry> {
        let mut found: Vec<&ProfileEntry> = self
            .entries
            .iter()
            .filter(|e| e.name.starts_with(prefix))
            .collect();
        found.sort_by(|a, b| b.average_ms.total_cmp(&a.average_ms));
        found.truncate(count);
        found
    }

    pub(crate) fn push_frame(&mut self, delta: f32) {
        if delta <= 0.0 || self.paused {
            return;
        }
        if self.frame_deltas.len() == FRAME_WINDOW {
            self.frame_deltas.pop_front();
        }
        self.frame_deltas.push_back(delta);
    }

    pub fn frame_statistics(&self) -> FrameStatistics {
        if self.frame_deltas.is_empty() {
            return FrameStatistics::default();
        }
        let total: f32 = self.frame_deltas.iter().sum();
        let average = total / self.frame_deltas.len() as f32;
        let (min, max) = self
            .frame_deltas
            .iter()
            .fold((f32::MAX, 0.0f32), |(lo, hi), &d| (lo.min(d), hi.max(d)));
        FrameStatistics {
            fps: 1.0 / average,
            average_ms: average * 1000.0,
            min_ms: min * 1000.0,
            max_ms: max * 1000.0,
        }
    }
}

impl fmt::Debug for Profiler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Profiler")
            .field("entries", &self.entries.len())
            .field("frames", &self.frame_deltas.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_smooths() {
        let mut profiler = Profiler::new();
        profiler.record("a", Duration::from_millis(10));
        assert_eq!(profiler.average_ms("a"), Some(10.0));
        profiler.record("a", Duration::from_millis(20));
        let a = profiler.average_ms("a").unwrap();
        assert!((a - 11.0).abs() < 1e-4, "{a}");
        profiler.record("system.b", Duration::from_millis(30));
        profiler.record("system.c", Duration::from_millis(5));
        let slow: Vec<&str> = profiler
            .slowest("system.", 1)
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(slow, ["system.b"]);
        assert!(profiler.average_ms("missing").is_none());
        // Peaks hold spikes the average smooths away, then decay.
        profiler.record("spiky", Duration::from_millis(1));
        profiler.record("spiky", Duration::from_millis(40));
        profiler.record("spiky", Duration::from_millis(1));
        let entry = profiler
            .entries()
            .iter()
            .find(|e| e.name == "spiky")
            .unwrap();
        assert!(entry.peak_ms > 39.0 && entry.average_ms < 10.0);
        assert_eq!(profiler.last_ms("spiky"), Some(1.0));
        // Paused: frozen.
        profiler.paused = true;
        profiler.record("spiky", Duration::from_millis(90));
        assert_eq!(profiler.last_ms("spiky"), Some(1.0));
    }

    #[test]
    fn history_is_bounded() {
        let mut profiler = Profiler::new();
        for i in 0..HISTORY + 20 {
            profiler.push_sample(FrameSample {
                delta_ms: i as f32,
                ..Default::default()
            });
        }
        assert_eq!(profiler.history().len(), HISTORY);
        assert_eq!(profiler.history()[0].delta_ms, 20.0, "oldest dropped");
        profiler.reset();
        assert!(profiler.history().is_empty() && profiler.entries().is_empty());
    }

    #[test]
    fn frame_statistics() {
        let mut profiler = Profiler::new();
        assert_eq!(profiler.frame_statistics(), FrameStatistics::default());
        for delta in [0.010, 0.020, 0.030] {
            profiler.push_frame(delta);
        }
        let stats = profiler.frame_statistics();
        assert!((stats.average_ms - 20.0).abs() < 1e-3);
        assert!((stats.fps - 50.0).abs() < 1e-2);
        assert!((stats.min_ms - 10.0).abs() < 1e-3 && (stats.max_ms - 30.0).abs() < 1e-3);
        for _ in 0..200 {
            profiler.push_frame(0.016);
        }
        assert!(
            (profiler.frame_statistics().max_ms - 16.0).abs() < 1e-3,
            "old frames drop out"
        );
    }
}
