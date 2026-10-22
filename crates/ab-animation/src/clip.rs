use ab_core::{Quat, Vec3};

/// Which part of a target's local transform a channel animates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Property {
    Translation,
    Rotation,
    Scale,
}

/// How values between keyframes are computed (glTF sampler interpolation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Interpolation {
    /// Hold the previous key's value.
    Step,
    /// Linear for vectors, spherical linear for rotations.
    #[default]
    Linear,
    /// Cubic Hermite spline. Values are stored as `[in_tangent, value, out_tangent]` triples.
    CubicSpline,
}

/// Keyframe values of one channel.
#[derive(Debug, Clone, PartialEq)]
pub enum Keyframes {
    Translation(Vec<Vec3>),
    Rotation(Vec<Quat>),
    Scale(Vec<Vec3>),
}

impl Keyframes {
    pub fn property(&self) -> Property {
        match self {
            Keyframes::Translation(_) => Property::Translation,
            Keyframes::Rotation(_) => Property::Rotation,
            Keyframes::Scale(_) => Property::Scale,
        }
    }

    fn len(&self) -> usize {
        match self {
            Keyframes::Translation(v) | Keyframes::Scale(v) => v.len(),
            Keyframes::Rotation(v) => v.len(),
        }
    }
}

/// A sampled value, ready to write into a transform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sample {
    Translation(Vec3),
    Rotation(Quat),
    Scale(Vec3),
}

/// One animated property of one target.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    /// Index into the playing [`AnimationPlayer`](crate::AnimationPlayer)'s `targets`.
    pub target: usize,
    pub interpolation: Interpolation,
    /// Strictly increasing key times in seconds.
    pub times: Vec<f32>,
    pub keyframes: Keyframes,
}

impl Channel {
    /// Checks that times are sorted and the value count matches the interpolation.
    pub fn is_valid(&self) -> bool {
        let per_key = if self.interpolation == Interpolation::CubicSpline {
            3
        } else {
            1
        };
        !self.times.is_empty()
            && self.keyframes.len() == self.times.len() * per_key
            && self.times.windows(2).all(|w| w[0] <= w[1])
            && self.times.iter().all(|t| t.is_finite())
    }

    /// Value at `time` seconds. Times outside the key range clamp to the first/last key.
    pub fn sample(&self, time: f32) -> Sample {
        let times = &self.times;
        let last = times.len() - 1;
        // Index of the first key strictly after `time`.
        let next = times.partition_point(|&t| t <= time);
        let (k0, k1, t) = if next == 0 {
            (0, 0, 0.0)
        } else if next > last {
            (last, last, 0.0)
        } else {
            let k0 = next - 1;
            let span = times[next] - times[k0];
            let t = if span > 0.0 {
                (time - times[k0]) / span
            } else {
                0.0
            };
            (k0, next, t)
        };
        let span = times[k1] - times[k0];

        match &self.keyframes {
            Keyframes::Translation(v) => Sample::Translation(self.sample_vec3(v, k0, k1, t, span)),
            Keyframes::Scale(v) => Sample::Scale(self.sample_vec3(v, k0, k1, t, span)),
            Keyframes::Rotation(v) => Sample::Rotation(self.sample_quat(v, k0, k1, t, span)),
        }
    }

    fn sample_vec3(&self, values: &[Vec3], k0: usize, k1: usize, t: f32, span: f32) -> Vec3 {
        match self.interpolation {
            Interpolation::Step => values[k0],
            Interpolation::Linear => values[k0].lerp(values[k1], t),
            Interpolation::CubicSpline => {
                let [h00, h10, h01, h11] = hermite(t);
                let p0 = values[k0 * 3 + 1];
                let m0 = values[k0 * 3 + 2] * span;
                let p1 = values[k1 * 3 + 1];
                let m1 = values[k1 * 3] * span;
                p0 * h00 + m0 * h10 + p1 * h01 + m1 * h11
            }
        }
    }

    fn sample_quat(&self, values: &[Quat], k0: usize, k1: usize, t: f32, span: f32) -> Quat {
        match self.interpolation {
            Interpolation::Step => unit(values[k0]),
            Interpolation::Linear => unit(values[k0]).slerp(unit(values[k1]), t),
            Interpolation::CubicSpline => {
                let [h00, h10, h01, h11] = hermite(t);
                let p0 = values[k0 * 3 + 1];
                let m0 = values[k0 * 3 + 2] * span;
                let p1 = values[k1 * 3 + 1];
                let m1 = values[k1 * 3] * span;
                unit(p0 * h00 + m0 * h10 + p1 * h01 + m1 * h11)
            }
        }
    }
}

/// Hermite basis functions at `t`.
fn hermite(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        2.0 * t3 - 3.0 * t2 + 1.0,
        t3 - 2.0 * t2 + t,
        -2.0 * t3 + 3.0 * t2,
        t3 - t2,
    ]
}

/// A named set of channels with a duration.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnimationClip {
    pub name: String,
    /// Length in seconds (the last key time across channels).
    pub duration: f32,
    pub channels: Vec<Channel>,
}

impl AnimationClip {
    /// Builds a clip, dropping invalid channels and computing the duration.
    pub fn new(name: impl Into<String>, channels: Vec<Channel>) -> Self {
        let channels: Vec<Channel> = channels.into_iter().filter(Channel::is_valid).collect();
        let duration = channels
            .iter()
            .filter_map(|c| c.times.last().copied())
            .fold(0.0, f32::max);
        Self {
            name: name.into(),
            duration,
            channels,
        }
    }
}

/// Normalised, or identity for a zero quaternion (a broken key must not produce NaN bones).
fn unit(q: Quat) -> Quat {
    let length = q.length();
    if length > 1e-6 && length.is_finite() {
        q / length
    } else {
        Quat::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translation(interpolation: Interpolation, times: Vec<f32>, values: Vec<Vec3>) -> Channel {
        Channel {
            target: 0,
            interpolation,
            times,
            keyframes: Keyframes::Translation(values),
        }
    }

    fn as_vec3(sample: Sample) -> Vec3 {
        match sample {
            Sample::Translation(v) | Sample::Scale(v) => v,
            Sample::Rotation(_) => panic!("not a vector"),
        }
    }

    #[test]
    fn linear_interpolates_and_clamps() {
        let c = translation(
            Interpolation::Linear,
            vec![1.0, 3.0],
            vec![Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)],
        );
        assert_eq!(as_vec3(c.sample(0.0)), Vec3::ZERO, "before first key");
        assert_eq!(as_vec3(c.sample(2.0)), Vec3::new(2.0, 0.0, 0.0));
        assert_eq!(
            as_vec3(c.sample(9.0)),
            Vec3::new(4.0, 0.0, 0.0),
            "after last key"
        );
    }

    #[test]
    fn step_holds_previous_value() {
        let c = translation(
            Interpolation::Step,
            vec![0.0, 1.0, 2.0],
            vec![Vec3::X, Vec3::Y, Vec3::Z],
        );
        assert_eq!(as_vec3(c.sample(0.99)), Vec3::X);
        assert_eq!(as_vec3(c.sample(1.0)), Vec3::Y);
        assert_eq!(as_vec3(c.sample(1.5)), Vec3::Y);
    }

    #[test]
    fn cubic_spline_hits_keys_and_uses_tangents() {
        // in-tangent, value, out-tangent per key. Zero tangents give a smoothstep.
        let c = translation(
            Interpolation::CubicSpline,
            vec![0.0, 2.0],
            vec![
                Vec3::ZERO,
                Vec3::ZERO,
                Vec3::ZERO,
                Vec3::ZERO,
                Vec3::X,
                Vec3::ZERO,
            ],
        );
        assert_eq!(as_vec3(c.sample(0.0)), Vec3::ZERO);
        assert!(as_vec3(c.sample(2.0)).abs_diff_eq(Vec3::X, 1e-6));
        assert!(as_vec3(c.sample(1.0)).abs_diff_eq(Vec3::X * 0.5, 1e-6));
        assert!(as_vec3(c.sample(0.5)).x < 0.25, "eases in");
    }

    #[test]
    fn rotation_slerps() {
        let c = Channel {
            target: 0,
            interpolation: Interpolation::Linear,
            times: vec![0.0, 1.0],
            keyframes: Keyframes::Rotation(vec![Quat::IDENTITY, Quat::from_rotation_y(1.0)]),
        };
        let Sample::Rotation(q) = c.sample(0.5) else {
            panic!("expected rotation")
        };
        assert!(q.abs_diff_eq(Quat::from_rotation_y(0.5), 1e-5));
    }

    #[test]
    fn invalid_channels_are_dropped_and_duration_computed() {
        let good = translation(Interpolation::Linear, vec![0.0, 2.5], vec![Vec3::ZERO; 2]);
        let mismatched = translation(Interpolation::Linear, vec![0.0, 1.0], vec![Vec3::ZERO]);
        let unsorted = translation(Interpolation::Linear, vec![1.0, 0.0], vec![Vec3::ZERO; 2]);
        let clip = AnimationClip::new("walk", vec![good, mismatched, unsorted]);
        assert_eq!(clip.channels.len(), 1);
        assert_eq!(clip.duration, 2.5);
    }
}
