//! Full-screen gameplay effects: flashes, fades, hit stop and impact distortion.

use ab_core::{Color, Time};
use ab_ecs::World;
use ab_rendering::ScreenOverlay;

/// A value easing from `from` to `to` over `duration` seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ramp {
    from: f32,
    to: f32,
    duration: f32,
    elapsed: f32,
}

impl Ramp {
    const fn constant(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            duration: 0.0,
            elapsed: 0.0,
        }
    }

    fn new(from: f32, to: f32, duration: f32) -> Self {
        Self {
            from,
            to,
            duration: duration.max(0.0),
            elapsed: 0.0,
        }
    }

    fn progress(&self) -> f32 {
        if self.duration <= 0.0 {
            1.0
        } else {
            (self.elapsed / self.duration).clamp(0.0, 1.0)
        }
    }

    fn value(&self) -> f32 {
        let t = self.progress();
        // Smoothstep: no visible kink at either end of a fade.
        let t = t * t * (3.0 - 2.0 * t);
        self.from + (self.to - self.from) * t
    }

    fn is_finished(&self) -> bool {
        self.progress() >= 1.0
    }

    fn advance(&mut self, delta: f32) {
        self.elapsed = (self.elapsed + delta).min(self.duration);
    }
}

/// A burst that starts at full strength and fades out (quadratically).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pulse {
    strength: f32,
    duration: f32,
    elapsed: f32,
}

impl Pulse {
    fn value(&self) -> f32 {
        let remaining = (1.0 - self.elapsed / self.duration).clamp(0.0, 1.0);
        self.strength * remaining * remaining
    }

    fn is_finished(&self) -> bool {
        self.elapsed >= self.duration
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct HitStop {
    remaining: f32,
    scale: f32,
    /// `Time::time_scale` to restore afterwards.
    restore: f32,
}

/// Gameplay screen effects, as a world resource ([`crate::register`] inserts it):
///
/// - [`flash`](Self::flash): a color that fades out (damage, pickups, lightning);
/// - [`fade_out`](Self::fade_out) / [`fade_in`](Self::fade_in): to and from a color
///   (scene changes, death, cutscenes);
/// - [`hit_stop`](Self::hit_stop): freezes or slows the game for a moment on a heavy hit;
/// - [`distort`](Self::distort): a chromatic-aberration kick that fades out.
///
/// Every effect runs on real (unscaled) time, so fades still finish while the game is paused
/// or slowed. [`effects_system`](crate::effects_system) writes the result into the
/// [`ScreenOverlay`] the renderer draws, on top of the scene's `Environment`.
///
/// ```
/// use ab_core::Color;
/// use ab_ecs::World;
/// use ab_effects::ScreenEffects;
///
/// let mut world = World::new();
/// ab_effects::register(&mut world);
/// let mut screen = world.resource_mut::<ScreenEffects>().unwrap();
/// screen.flash(Color::rgba(1.0, 0.1, 0.1, 0.6), 0.3); // took damage
/// screen.hit_stop(0.08);
/// screen.fade_out(Color::BLACK, 1.0); // then fade to black
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenEffects {
    fade_color: Color,
    fade: Ramp,
    flash_color: Color,
    flash: Option<Pulse>,
    distortion: Option<Pulse>,
    hit_stop: Option<HitStop>,
}

impl Default for ScreenEffects {
    fn default() -> Self {
        Self {
            fade_color: Color::BLACK,
            fade: Ramp::constant(0.0),
            flash_color: Color::WHITE,
            flash: None,
            distortion: None,
            hit_stop: None,
        }
    }
}

fn seconds(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

impl ScreenEffects {
    /// Flashes `color` over the screen and fades it out over `duration` seconds. The color's
    /// alpha is the peak strength (1 = solid at first). A new flash replaces the current one.
    pub fn flash(&mut self, color: Color, duration: f32) {
        let duration = seconds(duration);
        if duration == 0.0 {
            return;
        }
        self.flash_color = Color { a: 1.0, ..color };
        self.flash = Some(Pulse {
            strength: color.a.clamp(0.0, 1.0),
            duration,
            elapsed: 0.0,
        });
    }

    /// Fades the screen to a solid `color` over `duration` seconds (0 = at once).
    pub fn fade_out(&mut self, color: Color, duration: f32) {
        self.fade_to(color, 1.0, duration);
    }

    /// Fades from the current fade color back to the scene over `duration` seconds.
    pub fn fade_in(&mut self, duration: f32) {
        self.fade_to(self.fade_color, 0.0, duration);
    }

    /// Fades towards `amount` (0 = scene, 1 = solid `color`) over `duration` seconds,
    /// starting from where the current fade is.
    pub fn fade_to(&mut self, color: Color, amount: f32, duration: f32) {
        let current = self.fade_amount();
        // Changing color mid-fade would jump; start from the new color at the same amount.
        self.fade_color = Color { a: 1.0, ..color };
        let amount = if amount.is_finite() {
            amount.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.fade = Ramp::new(current, amount, seconds(duration));
    }

    /// How covered the screen is by the fade right now (0..1).
    pub fn fade_amount(&self) -> f32 {
        self.fade.value()
    }

    /// True while a fade is still moving.
    pub fn is_fading(&self) -> bool {
        !self.fade.is_finished()
    }

    /// True once a [`fade_out`](Self::fade_out) has fully covered the screen: the moment to
    /// change scenes.
    pub fn is_faded_out(&self) -> bool {
        self.fade.is_finished() && self.fade_amount() >= 1.0
    }

    /// Slows the game to a stop for `duration` real seconds, then restores the time scale.
    /// The classic 0.05–0.1 s freeze that makes hits feel heavy.
    pub fn hit_stop(&mut self, duration: f32) {
        self.hit_stop_scaled(duration, 0.0);
    }

    /// Like [`hit_stop`](Self::hit_stop), but runs the game at `time_scale` (e.g. 0.1)
    /// instead of freezing it.
    pub fn hit_stop_scaled(&mut self, duration: f32, time_scale: f32) {
        let duration = seconds(duration);
        if duration == 0.0 {
            return;
        }
        let scale = seconds(time_scale);
        match &mut self.hit_stop {
            // Overlapping hits extend the stop; the scale to restore stays the original one.
            Some(stop) => {
                stop.remaining = stop.remaining.max(duration);
                stop.scale = stop.scale.min(scale);
            }
            None => {
                self.hit_stop = Some(HitStop {
                    remaining: duration,
                    scale,
                    restore: f32::NAN,
                })
            }
        }
    }

    /// True while a hit stop holds the game.
    pub fn is_hit_stopped(&self) -> bool {
        self.hit_stop.is_some()
    }

    /// Splits colors towards the screen edges (`strength` about 0.5..3) and fades back over
    /// `duration` seconds: explosions, heavy landings, getting hit.
    pub fn distort(&mut self, strength: f32, duration: f32) {
        let duration = seconds(duration);
        if duration == 0.0 || !strength.is_finite() {
            return;
        }
        let strength = strength.max(0.0);
        // Never weaker than a distortion still playing.
        if self.distortion.is_some_and(|d| d.value() > strength) {
            return;
        }
        self.distortion = Some(Pulse {
            strength,
            duration,
            elapsed: 0.0,
        });
    }

    /// Ends every effect at once: no fade, no flash, normal time.
    pub fn clear(&mut self, time: Option<&mut Time>) {
        if let (Some(stop), Some(time)) = (self.hit_stop.take(), time) {
            if stop.restore.is_finite() {
                time.time_scale = stop.restore;
            }
        }
        *self = Self::default();
    }

    /// Advances every effect by `delta` real seconds, applies or ends a hit stop on `time`,
    /// and returns what to draw.
    pub fn update(&mut self, delta: f32, time: Option<&mut Time>) -> ScreenOverlay {
        let delta = seconds(delta);
        self.fade.advance(delta);
        for pulse in [&mut self.flash, &mut self.distortion] {
            if let Some(p) = pulse {
                p.elapsed += delta;
                if p.is_finished() {
                    *pulse = None;
                }
            }
        }
        if let Some(stop) = &mut self.hit_stop {
            if let Some(time) = time {
                if !stop.restore.is_finite() {
                    stop.restore = time.time_scale;
                } else {
                    stop.remaining -= delta;
                }
                if stop.remaining <= 0.0 {
                    // Leave the scale alone if something else changed it meanwhile.
                    if time.time_scale == stop.scale {
                        time.time_scale = stop.restore;
                    }
                    self.hit_stop = None;
                } else {
                    time.time_scale = stop.scale;
                }
            } else {
                self.hit_stop = None;
            }
        }
        self.overlay()
    }

    /// The overlay for the current state: the flash composited over the fade.
    pub fn overlay(&self) -> ScreenOverlay {
        let fade = self.fade_amount();
        let flash = self.flash.map_or(0.0, |f| f.value());
        let amount = 1.0 - (1.0 - fade) * (1.0 - flash);
        let color = if amount <= 0.0 {
            Color::TRANSPARENT
        } else {
            let fade_weight = fade * (1.0 - flash) / amount;
            let flash_weight = flash / amount;
            let mix = |f: f32, l: f32| f * fade_weight + l * flash_weight;
            Color::rgba(
                mix(self.fade_color.r, self.flash_color.r),
                mix(self.fade_color.g, self.flash_color.g),
                mix(self.fade_color.b, self.flash_color.b),
                amount,
            )
        };
        ScreenOverlay {
            color,
            chromatic_aberration: self.distortion.map_or(0.0, |d| d.value()),
        }
    }
}

/// Flashes the screen (see [`ScreenEffects::flash`]).
pub fn flash_screen(world: &World, color: Color, duration: f32) {
    if let Some(mut screen) = world.resource_mut::<ScreenEffects>() {
        screen.flash(color, duration);
    }
}

/// A short freeze of the game (see [`ScreenEffects::hit_stop`]).
pub fn hit_stop(world: &World, duration: f32) {
    if let Some(mut screen) = world.resource_mut::<ScreenEffects>() {
        screen.hit_stop(duration);
    }
}

/// Runs [`ScreenEffects`] for one frame and publishes the [`ScreenOverlay`].
pub(crate) fn update_screen_effects(world: &World) {
    let Some(mut screen) = world.resource_mut::<ScreenEffects>() else {
        return;
    };
    let mut time = world.resource_mut::<Time>();
    let delta = time.as_ref().map_or(0.0, |t| t.unscaled_delta);
    let overlay = screen.update(delta, time.as_deref_mut());
    if let Some(mut target) = world.resource_mut::<ScreenOverlay>() {
        *target = overlay;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn flash_fades_out_and_ends() {
        let mut screen = ScreenEffects::default();
        assert_eq!(screen.overlay(), ScreenOverlay::default());
        screen.flash(Color::rgba(1.0, 0.0, 0.0, 0.8), 1.0);
        let start = screen.update(0.0, None);
        assert!(close(start.color.a, 0.8) && close(start.color.r, 1.0));
        let half = screen.update(0.5, None);
        assert!(close(half.color.a, 0.2), "{half:?}");
        assert_eq!(screen.update(0.6, None).color.a, 0.0);
        assert!(screen.flash.is_none());
    }

    #[test]
    fn fades_out_and_back_in() {
        let mut screen = ScreenEffects::default();
        screen.fade_out(Color::BLACK, 1.0);
        assert!(screen.is_fading() && !screen.is_faded_out());
        let mid = screen.update(0.5, None);
        assert!(close(mid.color.a, 0.5));
        screen.update(0.5, None);
        assert!(screen.is_faded_out());
        assert!(close(screen.overlay().color.a, 1.0));
        screen.fade_in(0.0);
        assert_eq!(screen.update(0.0, None).color.a, 0.0);
        // Changing direction mid-fade continues from where it is.
        screen.fade_out(Color::WHITE, 2.0);
        screen.update(1.0, None);
        screen.fade_in(1.0);
        assert!(close(screen.fade_amount(), 0.5));
    }

    #[test]
    fn flash_over_a_fade_composites() {
        let mut screen = ScreenEffects::default();
        screen.fade_to(Color::BLACK, 0.5, 0.0);
        screen.flash(Color::rgba(1.0, 1.0, 1.0, 0.5), 1.0);
        let overlay = screen.update(0.0, None);
        assert!(close(overlay.color.a, 0.75));
        // Half the result is the flash's white: 0.5 of 0.75 coverage.
        assert!(close(overlay.color.r, 0.5 / 0.75), "{overlay:?}");
    }

    #[test]
    fn hit_stop_freezes_then_restores_the_time_scale() {
        let mut screen = ScreenEffects::default();
        let mut time = Time {
            time_scale: 0.5,
            ..Time::new(1.0 / 60.0)
        };
        screen.hit_stop(0.1);
        assert!(screen.is_hit_stopped());
        screen.update(0.016, Some(&mut time));
        assert_eq!(time.time_scale, 0.0);
        // A second hit extends it.
        screen.hit_stop(0.2);
        screen.update(0.1, Some(&mut time));
        assert_eq!(time.time_scale, 0.0);
        screen.update(0.11, Some(&mut time));
        assert_eq!(time.time_scale, 0.5, "restored to the game's own scale");
        assert!(!screen.is_hit_stopped());
    }

    #[test]
    fn hit_stop_respects_scale_changes_made_meanwhile() {
        let mut screen = ScreenEffects::default();
        let mut time = Time::new(1.0 / 60.0);
        screen.hit_stop_scaled(0.1, 0.2);
        screen.update(0.0, Some(&mut time));
        assert_eq!(time.time_scale, 0.2);
        time.time_scale = 0.25; // e.g. the game entered slow motion
        screen.update(0.2, Some(&mut time));
        assert_eq!(time.time_scale, 0.25);
    }

    #[test]
    fn distortion_and_bad_input() {
        let mut screen = ScreenEffects::default();
        screen.distort(2.0, 0.5);
        assert!(close(screen.update(0.0, None).chromatic_aberration, 2.0));
        screen.distort(0.5, 1.0); // weaker: ignored while the strong one plays
        assert!(close(screen.overlay().chromatic_aberration, 2.0));
        for bad in [f32::NAN, f32::INFINITY, -1.0] {
            screen.flash(Color::WHITE, bad);
            screen.distort(bad, bad);
            screen.hit_stop(bad);
            screen.fade_to(Color::WHITE, bad, bad);
        }
        let overlay = screen.update(f32::NAN, None);
        assert!(overlay.color.a.is_finite() && overlay.chromatic_aberration.is_finite());
        let mut time = Time::new(1.0 / 60.0);
        screen.hit_stop(1.0);
        screen.update(0.0, Some(&mut time));
        screen.clear(Some(&mut time));
        assert_eq!(time.time_scale, 1.0);
        assert_eq!(screen, ScreenEffects::default());
    }
}
