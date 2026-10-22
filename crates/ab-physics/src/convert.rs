//! The rigid pose the engine hands to Rapier. Rapier works in `glam` types too, so vectors
//! and rotations pass through as they are.

use ab_core::{Quat, Vec3};

/// A rigid transform (no scale): what Rapier simulates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Pose {
    pub position: Vec3,
    pub rotation: Quat,
}

impl Pose {
    pub fn to_rapier(self) -> rapier3d::math::Pose {
        rapier3d::math::Pose::from_parts(self.position, self.rotation)
    }

    pub fn from_rapier(pose: &rapier3d::math::Pose) -> Self {
        Self {
            position: pose.translation,
            rotation: pose.rotation,
        }
    }

    /// Below these differences two poses count as equal, so float noise from re-composing
    /// transforms is never mistaken for a teleport.
    const POSITION_EPSILON: f32 = 1e-5;
    const ROTATION_EPSILON: f32 = 1e-6;

    /// Inverse of `self` applied to `other`: `other` expressed in `self`'s frame.
    pub fn relative(&self, other: Pose) -> Pose {
        let inverse = self.rotation.inverse();
        Pose {
            position: inverse * (other.position - self.position),
            rotation: (inverse * other.rotation).normalize(),
        }
    }

    pub fn approx_eq(&self, other: &Pose) -> bool {
        self.position.distance_squared(other.position)
            <= Self::POSITION_EPSILON * Self::POSITION_EPSILON
            // q and -q are the same rotation.
            && 1.0 - self.rotation.dot(other.rotation).abs() <= Self::ROTATION_EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapier_round_trip() {
        let original = Pose {
            position: Vec3::new(1.0, -2.0, 3.0),
            rotation: Quat::from_rotation_y(0.7) * Quat::from_rotation_x(-0.3),
        };
        let back = Pose::from_rapier(&original.to_rapier());
        assert!(back.approx_eq(&original), "{back:?}");
    }

    #[test]
    fn relative_pose() {
        let body = Pose {
            position: Vec3::new(0.0, 1.0, 0.0),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        };
        let shape = Pose {
            position: Vec3::new(0.0, 1.0, -2.0),
            rotation: body.rotation,
        };
        let rel = body.relative(shape);
        assert!(
            rel.position.abs_diff_eq(Vec3::new(2.0, 0.0, 0.0), 1e-5),
            "{rel:?}"
        );
        assert!(rel.rotation.abs_diff_eq(Quat::IDENTITY, 1e-6));
    }
}
