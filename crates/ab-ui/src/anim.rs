//! Code-driven UI animation: fades and slides.

use crate::Control;
use ab_core::{Time, Vec2};
use ab_ecs::World;

/// Easing curve of a tween.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Easing {
    Linear,
    EaseIn,
    #[default]
    EaseOut,
    EaseInOut,
    /// Overshoots a little, then settles (pop-ins, buttons).
    BackOut,
    /// Springs past the target and wobbles (collectibles, jelly).
    ElasticOut,
    /// Bounces on arrival (dropped objects).
    BounceOut,
    /// Gentle sine curve in and out (floating, breathing).
    SineInOut,
}

impl Easing {
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EaseIn => t * t * t,
            Self::EaseOut => 1.0 - (1.0 - t).powi(3),
            Self::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Self::BackOut => {
                const C1: f32 = 1.70158;
                const C3: f32 = C1 + 1.0;
                1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2)
            }
            Self::ElasticOut => {
                if t <= 0.0 || t >= 1.0 {
                    t
                } else {
                    let c4 = std::f32::consts::TAU / 3.0;
                    2f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * c4).sin() + 1.0
                }
            }
            Self::BounceOut => {
                const N1: f32 = 7.5625;
                const D1: f32 = 2.75;
                if t < 1.0 / D1 {
                    N1 * t * t
                } else if t < 2.0 / D1 {
                    let t = t - 1.5 / D1;
                    N1 * t * t + 0.75
                } else if t < 2.5 / D1 {
                    let t = t - 2.25 / D1;
                    N1 * t * t + 0.9375
                } else {
                    let t = t - 2.625 / D1;
                    N1 * t * t + 0.984375
                }
            }
            Self::SineInOut => -((std::f32::consts::PI * t).cos() - 1.0) / 2.0,
        }
    }
}

/// What a tween animates on the [`Control`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TweenTarget {
    /// `modulate.a`.
    Alpha { from: f32, to: f32 },
    /// `translation` (UI units).
    Translation { from: Vec2, to: Vec2 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiTween {
    pub target: TweenTarget,
    pub duration: f32,
    /// Seconds to wait before starting.
    pub delay: f32,
    pub easing: Easing,
    elapsed: f32,
}

/// Runs tweens on its node's [`Control`]. Add it with
/// [`UiNodes::animate`](crate::UiNodes::animate).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UiAnimator {
    tweens: Vec<UiTween>,
}

impl UiAnimator {
    /// Queues a tween; it starts after `delay` seconds.
    pub fn tween(
        &mut self,
        target: TweenTarget,
        duration: f32,
        delay: f32,
        easing: Easing,
    ) -> &mut Self {
        self.tweens.push(UiTween {
            target,
            duration: duration.max(0.0),
            delay: delay.max(0.0),
            easing,
            elapsed: 0.0,
        });
        self
    }

    /// Fades the alpha from `from` to `to`.
    pub fn fade(&mut self, from: f32, to: f32, duration: f32) -> &mut Self {
        self.tween(
            TweenTarget::Alpha { from, to },
            duration,
            0.0,
            Easing::EaseOut,
        )
    }

    /// Slides in from `offset` to the laid-out position.
    pub fn slide_in(&mut self, offset: Vec2, duration: f32) -> &mut Self {
        self.tween(
            TweenTarget::Translation {
                from: offset,
                to: Vec2::ZERO,
            },
            duration,
            0.0,
            Easing::EaseOut,
        )
    }

    /// Slides from the laid-out position to `offset`.
    pub fn slide_out(&mut self, offset: Vec2, duration: f32) -> &mut Self {
        self.tween(
            TweenTarget::Translation {
                from: Vec2::ZERO,
                to: offset,
            },
            duration,
            0.0,
            Easing::EaseIn,
        )
    }

    pub fn is_animating(&self) -> bool {
        !self.tweens.is_empty()
    }

    pub fn stop(&mut self) {
        self.tweens.clear();
    }

    /// Advances by `delta` seconds and writes the animated values. Finished tweens apply
    /// their end value and are dropped.
    pub fn advance(&mut self, control: &mut Control, delta: f32) {
        self.tweens.retain_mut(|tween| {
            tween.elapsed += delta;
            let active = tween.elapsed - tween.delay;
            if active < 0.0 {
                return true;
            }
            let t = if tween.duration > 0.0 {
                active / tween.duration
            } else {
                1.0
            };
            let k = tween.easing.apply(t);
            match tween.target {
                TweenTarget::Alpha { from, to } => control.modulate.a = from + (to - from) * k,
                TweenTarget::Translation { from, to } => {
                    control.translation = from + (to - from) * k
                }
            }
            t < 1.0
        });
    }
}

/// Advances every [`UiAnimator`] by [`Time::delta`]. The engine runs it in `Update`.
pub fn ui_animation_system(world: &mut World) {
    let delta = world.resource::<Time>().map_or(0.0, |t| t.delta);
    for (animator, control) in world.query::<(&mut UiAnimator, &mut Control)>().iter() {
        animator.advance(control, delta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_endpoints() {
        for easing in [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
        ] {
            assert_eq!(easing.apply(0.0), 0.0);
            assert!((easing.apply(1.0) - 1.0).abs() < 1e-6);
            assert!(easing.apply(0.5) > 0.0 && easing.apply(0.5) < 1.0);
        }
        assert!(Easing::EaseOut.apply(0.5) > 0.5);
        assert!(Easing::EaseIn.apply(0.5) < 0.5);
    }

    #[test]
    fn fade_then_slide_with_delay() {
        let mut control = Control::new();
        let mut animator = UiAnimator::default();
        animator.fade(0.0, 1.0, 1.0).tween(
            TweenTarget::Translation {
                from: Vec2::new(-100.0, 0.0),
                to: Vec2::ZERO,
            },
            1.0,
            0.5,
            Easing::Linear,
        );
        animator.advance(&mut control, 0.25);
        assert!(control.modulate.a > 0.25 && control.modulate.a < 1.0);
        assert_eq!(control.translation, Vec2::ZERO, "slide still waiting");
        animator.advance(&mut control, 0.75);
        assert_eq!(control.modulate.a, 1.0);
        assert!((control.translation.x + 50.0).abs() < 1e-4);
        assert!(animator.is_animating());
        animator.advance(&mut control, 1.0);
        assert_eq!(control.translation, Vec2::ZERO);
        assert!(!animator.is_animating());
    }
}
