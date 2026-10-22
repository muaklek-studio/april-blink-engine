//! Frame timing.

/// Frame timing information, exposed to gameplay code as an ECS resource.
#[derive(Debug, Clone)]
pub struct Time {
    /// Seconds since the previous frame (already clamped, see [`Time::MAX_DELTA`]).
    pub delta: f32,
    /// Fixed physics step in seconds.
    pub fixed_delta: f32,
    /// Seconds since the engine started. `f64` so precision holds over long sessions.
    pub elapsed: f64,
    /// Number of completed `process` frames.
    pub frame_count: u64,
    /// Number of completed fixed (physics) steps.
    pub physics_frame_count: u64,
    /// Game speed: 1 = normal, 0.25 = slow motion, 0 = frozen. Scales `delta` (and so the
    /// number of physics steps); `unscaled_delta` keeps real time for UI and debug tools.
    pub time_scale: f32,
    /// Seconds since the previous frame, clamped but not scaled.
    pub unscaled_delta: f32,
}

impl Time {
    /// Largest frame delta fed to gameplay. Longer stalls (debugger breaks, window drags) are
    /// clamped so that a single frame never makes objects jump across the level.
    pub const MAX_DELTA: f32 = 0.25;

    pub fn new(fixed_delta: f32) -> Self {
        Self {
            delta: 0.0,
            fixed_delta,
            elapsed: 0.0,
            frame_count: 0,
            physics_frame_count: 0,
            time_scale: 1.0,
            unscaled_delta: 0.0,
        }
    }

    /// Starts a new frame with the measured wall-clock delta and returns the game delta
    /// (clamped, then scaled by `time_scale`).
    pub fn begin_frame(&mut self, raw_delta: f32) -> f32 {
        self.unscaled_delta = raw_delta.clamp(0.0, Self::MAX_DELTA);
        self.delta = self.unscaled_delta * self.time_scale.max(0.0);
        self.elapsed += self.delta as f64;
        self.delta
    }

    pub fn end_frame(&mut self) {
        self.frame_count += 1;
    }

    /// Frames per second derived from the last delta (0 when unknown).
    pub fn fps(&self) -> f32 {
        if self.delta > 0.0 {
            1.0 / self.delta
        } else {
            0.0
        }
    }
}

impl Default for Time {
    fn default() -> Self {
        Self::new(1.0 / 60.0)
    }
}

/// Accumulator for fixed-timestep updates (physics, `physics_process`).
///
/// Each frame, call [`FixedTimestep::advance`] with the frame delta and run the returned number
/// of fixed steps. [`FixedTimestep::alpha`] gives the interpolation factor between the last two
/// fixed states for smooth rendering.
#[derive(Debug, Clone)]
pub struct FixedTimestep {
    step: f32,
    accumulator: f32,
    max_steps: u32,
}

impl FixedTimestep {
    /// `hz` steps per second; at most `max_steps` steps per frame (prevents the
    /// "spiral of death" when a frame takes longer than the steps it schedules).
    pub fn new(hz: u32, max_steps: u32) -> Self {
        assert!(hz > 0, "fixed timestep rate must be > 0");
        assert!(max_steps > 0, "max_steps must be > 0");
        Self {
            step: 1.0 / hz as f32,
            accumulator: 0.0,
            max_steps,
        }
    }

    pub fn step(&self) -> f32 {
        self.step
    }

    /// Adds `delta` seconds and returns how many fixed steps to run this frame.
    /// Time beyond `max_steps` is dropped rather than carried over.
    pub fn advance(&mut self, delta: f32) -> u32 {
        self.accumulator += delta.max(0.0);
        let steps = (self.accumulator / self.step) as u32;
        if steps > self.max_steps {
            self.accumulator = 0.0;
            self.max_steps
        } else {
            self.accumulator -= steps as f32 * self.step;
            steps
        }
    }

    /// Fraction (0..1) of a step left in the accumulator.
    pub fn alpha(&self) -> f32 {
        (self.accumulator / self.step).clamp(0.0, 1.0)
    }
}

impl Default for FixedTimestep {
    fn default() -> Self {
        Self::new(60, 8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_frame_clamps_large_deltas() {
        let mut time = Time::default();
        assert_eq!(time.begin_frame(5.0), Time::MAX_DELTA);
        assert_eq!(time.begin_frame(-1.0), 0.0);
        assert!((time.elapsed - Time::MAX_DELTA as f64).abs() < 1e-9);
    }

    #[test]
    fn time_scale_slows_the_game_not_real_time() {
        let mut time = Time {
            time_scale: 0.25,
            ..Default::default()
        };
        assert_eq!(time.begin_frame(0.1), 0.025);
        assert_eq!(time.unscaled_delta, 0.1);
        time.time_scale = 0.0;
        assert_eq!(time.begin_frame(0.1), 0.0, "frozen");
    }

    #[test]
    fn fixed_timestep_accumulates_fractional_frames() {
        let mut fixed = FixedTimestep::new(60, 8);
        let half = fixed.step() * 0.5;
        assert_eq!(fixed.advance(half), 0);
        assert!((fixed.alpha() - 0.5).abs() < 1e-4);
        assert_eq!(fixed.advance(half), 1);
    }

    #[test]
    fn fixed_timestep_runs_multiple_steps_for_slow_frames() {
        let mut fixed = FixedTimestep::new(50, 8);
        assert_eq!(fixed.advance(0.0601), 3);
    }

    #[test]
    fn fixed_timestep_caps_steps_and_drops_backlog() {
        let mut fixed = FixedTimestep::new(60, 4);
        assert_eq!(fixed.advance(1.0), 4);
        assert_eq!(fixed.alpha(), 0.0);
    }

    #[test]
    fn one_second_at_60hz_is_60_steps() {
        let mut fixed = FixedTimestep::new(60, 8);
        let total: u32 = (0..144).map(|_| fixed.advance(1.0 / 144.0)).sum();
        assert!((59..=60).contains(&total), "got {total}");
    }
}
