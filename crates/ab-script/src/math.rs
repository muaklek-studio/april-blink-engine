//! Math: glam's vectors and quaternions, plus [`Transform`].

pub use glam::{EulerRot, Mat3, Mat4, Quat, Vec2, Vec3, Vec4};

/// Position, rotation and scale of a node (local or global).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn from_translation(translation: Vec3) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    pub fn from_rotation(rotation: Quat) -> Self {
        Self {
            rotation,
            ..Self::IDENTITY
        }
    }

    pub fn with_rotation(mut self, rotation: Quat) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_scale(mut self, scale: Vec3) -> Self {
        self.scale = scale;
        self
    }

    /// The direction the node looks at: its -Z axis (Godot's convention).
    pub fn forward(&self) -> Vec3 {
        (self.rotation * Vec3::NEG_Z).normalize_or_zero()
    }

    pub fn right(&self) -> Vec3 {
        (self.rotation * Vec3::X).normalize_or_zero()
    }

    pub fn up(&self) -> Vec3 {
        (self.rotation * Vec3::Y).normalize_or_zero()
    }

    /// A point in this transform's space, in its parent's space.
    pub fn transform_point(&self, point: Vec3) -> Vec3 {
        self.translation + self.rotation * (self.scale * point)
    }

    /// A direction in this transform's space, in its parent's space (no translation).
    pub fn transform_vector(&self, vector: Vec3) -> Vec3 {
        self.rotation * (self.scale * vector)
    }

    pub fn to_mat4(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms_points_and_directions() {
        let t = Transform::from_translation(Vec3::new(1.0, 0.0, 0.0))
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2))
            .with_scale(Vec3::splat(2.0));
        assert!(t
            .transform_point(Vec3::NEG_Z)
            .abs_diff_eq(Vec3::new(-1.0, 0.0, 0.0), 1e-5));
        assert!(t.forward().abs_diff_eq(Vec3::NEG_X, 1e-5));
        assert!(t
            .to_mat4()
            .transform_point3(Vec3::X)
            .abs_diff_eq(t.transform_point(Vec3::X), 1e-5));
    }
}
