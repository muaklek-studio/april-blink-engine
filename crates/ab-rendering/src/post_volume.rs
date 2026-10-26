//! Post-processing volumes: boxes in the scene that change the look while the camera is in
//! them (a cave darker and bluer, a dream blurred, a fight with motion blur).
//!
//! Each [`PostVolume`] overrides some [`PostProcess`] values. Inside its box it counts fully
//! (times its `weight`); outside, it fades out over `blend_distance` metres, so walking in
//! is a smooth change, not a cut. A `global` volume counts everywhere. Overlapping volumes
//! apply from the lowest `priority` up, each blended over the result of the ones before:
//! the highest priority wins where both count fully.

use crate::environment::PostProcess;
use ab_core::{Affine3A, Color, Vec3};

/// The values a volume sets; `None` leaves the value as it is.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PostOverrides {
    pub exposure: Option<f32>,
    pub contrast: Option<f32>,
    pub saturation: Option<f32>,
    pub tint: Option<Color>,
    pub vignette_intensity: Option<f32>,
    pub bloom_intensity: Option<f32>,
    pub bloom_threshold: Option<f32>,
    pub chromatic_aberration: Option<f32>,
    pub grain: Option<f32>,
    pub focus_distance: Option<f32>,
    pub focus_range: Option<f32>,
    pub max_blur: Option<f32>,
    pub motion_blur: Option<f32>,
}

/// A box (its node's transform, `half_extents` on each side) that changes the
/// post-processing while the camera is near or in it (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PostVolume {
    pub half_extents: Vec3,
    /// Metres outside the box over which it fades out (0: a hard edge).
    pub blend_distance: f32,
    /// Higher applies later (over lower ones).
    pub priority: i32,
    /// 0..1: how much it counts at most.
    pub weight: f32,
    /// Counts everywhere, whatever the box.
    pub global: bool,
    pub overrides: PostOverrides,
}

impl Default for PostVolume {
    fn default() -> Self {
        Self {
            half_extents: Vec3::splat(5.0),
            blend_distance: 2.0,
            priority: 0,
            weight: 1.0,
            global: false,
            overrides: PostOverrides::default(),
        }
    }
}

impl PostVolume {
    /// How much the volume at `global` counts with the camera at `eye` (0..1).
    pub fn weight_at(&self, global: &Affine3A, eye: Vec3) -> f32 {
        let weight = self.weight.clamp(0.0, 1.0);
        if self.global {
            return weight;
        }
        // Distance outside the box, in the box's own space (scale included).
        let local = global.inverse().transform_point3(eye);
        let outside = (local.abs() - self.half_extents.abs()).max(Vec3::ZERO);
        // Back to world units along the axes (a scaled box blends in metres).
        let (scale, _, _) = global.to_scale_rotation_translation();
        let distance = (outside * scale.abs()).length();
        if distance <= 0.0 {
            return weight;
        }
        if self.blend_distance <= 0.0 {
            return 0.0;
        }
        weight * (1.0 - distance / self.blend_distance).clamp(0.0, 1.0)
    }
}

fn mix(a: f32, b: Option<f32>, t: f32) -> f32 {
    b.map_or(a, |b| a + (b - a) * t)
}

impl PostOverrides {
    /// `base` moved towards these values by `t` (0..1).
    pub fn apply(&self, base: &PostProcess, t: f32) -> PostProcess {
        let mut post = *base;
        post.exposure = mix(post.exposure, self.exposure, t);
        post.contrast = mix(post.contrast, self.contrast, t);
        post.saturation = mix(post.saturation, self.saturation, t);
        if let Some(tint) = self.tint {
            let c = post.tint;
            post.tint = Color::rgba(
                c.r + (tint.r - c.r) * t,
                c.g + (tint.g - c.g) * t,
                c.b + (tint.b - c.b) * t,
                c.a + (tint.a - c.a) * t,
            );
        }
        post.vignette_intensity = mix(post.vignette_intensity, self.vignette_intensity, t);
        post.bloom.intensity = mix(post.bloom.intensity, self.bloom_intensity, t);
        post.bloom.threshold = mix(post.bloom.threshold, self.bloom_threshold, t);
        post.chromatic_aberration = mix(post.chromatic_aberration, self.chromatic_aberration, t);
        post.grain = mix(post.grain, self.grain, t);
        let dof = &mut post.depth_of_field;
        // A volume turning depth of field on starts from its own focus, not from 0.
        let fresh = dof.max_blur <= 0.0;
        dof.focus_distance = if fresh {
            self.focus_distance.unwrap_or(dof.focus_distance)
        } else {
            mix(dof.focus_distance, self.focus_distance, t)
        };
        dof.focus_range = if fresh {
            self.focus_range.unwrap_or(dof.focus_range)
        } else {
            mix(dof.focus_range, self.focus_range, t)
        };
        dof.max_blur = mix(dof.max_blur, self.max_blur, t);
        post.motion_blur = mix(post.motion_blur, self.motion_blur, t);
        post
    }
}

/// `base` with every volume that counts at `eye` applied (see the module docs), and how
/// many counted.
pub fn blend_post(
    base: &PostProcess,
    volumes: &[(Affine3A, PostVolume)],
    eye: Vec3,
) -> (PostProcess, usize) {
    let mut order: Vec<&(Affine3A, PostVolume)> = volumes.iter().collect();
    order.sort_by_key(|(_, v)| v.priority);
    let mut post = *base;
    let mut counted = 0;
    for (global, volume) in order {
        let t = volume.weight_at(global, eye);
        // A degenerate box (zero scale) counts for nothing rather than poisoning the image.
        if t.is_finite() && t > 0.0 {
            post = volume.overrides.apply(&post, t);
            counted += 1;
        }
    }
    (post, counted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn darker(priority: i32, exposure: f32) -> PostVolume {
        PostVolume {
            priority,
            overrides: PostOverrides {
                exposure: Some(exposure),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn volumes_count_inside_fade_outside_and_stack_by_priority() {
        let at = |x: f32| Affine3A::from_translation(Vec3::new(x, 0.0, 0.0));
        let base = PostProcess::default();
        let volume = darker(0, 0.5);
        // Inside, at the edge of the blend, and halfway through it.
        assert_eq!(volume.weight_at(&at(0.0), Vec3::new(4.9, 0.0, 0.0)), 1.0);
        assert_eq!(volume.weight_at(&at(0.0), Vec3::new(7.5, 0.0, 0.0)), 0.0);
        assert!((volume.weight_at(&at(0.0), Vec3::new(6.0, 0.0, 0.0)) - 0.5).abs() < 1e-5);
        let (post, counted) = blend_post(&base, &[(at(0.0), volume)], Vec3::new(6.0, 0.0, 0.0));
        assert_eq!(counted, 1);
        assert!((post.exposure - 0.75).abs() < 1e-5);
        // Two volumes at the camera: the higher priority wins, whatever their order.
        let volumes = [(at(0.0), darker(5, 2.0)), (at(0.0), darker(1, 0.5))];
        let (post, _) = blend_post(&base, &volumes, Vec3::ZERO);
        assert_eq!(post.exposure, 2.0);
        // Far away: unchanged, unless global.
        let (post, counted) = blend_post(&base, &volumes, Vec3::splat(100.0));
        assert_eq!((post, counted), (base, 0));
        let global = PostVolume {
            global: true,
            weight: 0.5,
            ..darker(0, 0.0)
        };
        let (post, _) = blend_post(&base, &[(at(0.0), global)], Vec3::splat(100.0));
        assert!((post.exposure - 0.5).abs() < 1e-5);
    }

    #[test]
    fn a_volume_turns_depth_of_field_on_at_its_own_focus() {
        let volume = PostOverrides {
            focus_distance: Some(12.0),
            max_blur: Some(8.0),
            ..Default::default()
        };
        let post = volume.apply(&PostProcess::default(), 0.5);
        assert_eq!(post.depth_of_field.focus_distance, 12.0);
        assert_eq!(post.depth_of_field.max_blur, 4.0);
    }
}
