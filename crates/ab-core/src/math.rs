//! Math types. Re-exports `glam` and adds [`Transform3D`].
//!
//! Conventions follow Godot: right-handed, +Y up, -Z forward, +X right.

pub use glam::{camera, Affine3A, EulerRot, Mat3, Mat3A, Mat4, Quat, Vec2, Vec3, Vec3A, Vec4};

use serde::{Deserialize, Serialize};
use std::ops::Mul;

/// Translation / rotation / scale transform.
///
/// This is the editable, decomposed form used for local transforms. Composed (global)
/// transforms are kept as [`Affine3A`], which can also represent the shear produced by
/// non-uniform scale under rotation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform3D {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform3D {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub const fn identity() -> Self {
        Self::IDENTITY
    }

    pub const fn from_translation(translation: Vec3) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    pub const fn from_rotation(rotation: Quat) -> Self {
        Self {
            rotation,
            ..Self::IDENTITY
        }
    }

    pub const fn from_scale(scale: Vec3) -> Self {
        Self {
            scale,
            ..Self::IDENTITY
        }
    }

    pub const fn from_trs(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Self {
            translation,
            rotation,
            scale,
        }
    }

    /// Decomposes an affine transform. Shear (if any) is lost.
    pub fn from_affine(affine: &Affine3A) -> Self {
        let (scale, rotation, translation) = affine.to_scale_rotation_translation();
        Self {
            translation,
            rotation,
            scale,
        }
    }

    /// Decomposes a matrix. Shear and projection (if any) are lost.
    pub fn from_matrix(matrix: &Mat4) -> Self {
        let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
        Self {
            translation,
            rotation,
            scale,
        }
    }

    /// Transform placed at `eye`, rotated so that its forward axis (-Z) points at `target`.
    pub fn looking_at(eye: Vec3, target: Vec3, up: Vec3) -> Self {
        Self::from_translation(eye).with_looking_at(target, up)
    }

    #[must_use]
    pub fn with_translation(mut self, translation: Vec3) -> Self {
        self.translation = translation;
        self
    }

    #[must_use]
    pub fn with_rotation(mut self, rotation: Quat) -> Self {
        self.rotation = rotation;
        self
    }

    #[must_use]
    pub fn with_scale(mut self, scale: Vec3) -> Self {
        self.scale = scale;
        self
    }

    /// Returns this transform rotated so that -Z points from `translation` towards `target`.
    /// If the direction is degenerate (zero or parallel to `up`) the rotation is unchanged.
    #[must_use]
    pub fn with_looking_at(mut self, target: Vec3, up: Vec3) -> Self {
        if let Some(rotation) = look_rotation(target - self.translation, up) {
            self.rotation = rotation;
        }
        self
    }

    pub fn to_affine(&self) -> Affine3A {
        Affine3A::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    pub fn to_matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    pub fn transform_point(&self, point: Vec3) -> Vec3 {
        self.translation + self.rotation * (self.scale * point)
    }

    pub fn transform_vector(&self, vector: Vec3) -> Vec3 {
        self.rotation * (self.scale * vector)
    }

    /// Local -Z axis in parent space.
    pub fn forward(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Z
    }

    /// Local +X axis in parent space.
    pub fn right(&self) -> Vec3 {
        self.rotation * Vec3::X
    }

    /// Local +Y axis in parent space.
    pub fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }

    /// Moves along parent-space axes.
    pub fn translate(&mut self, offset: Vec3) {
        self.translation += offset;
    }

    /// Moves along this transform's own (rotated) axes.
    pub fn translate_local(&mut self, offset: Vec3) {
        self.translation += self.rotation * offset;
    }

    /// Applies `rotation` in parent space (pre-multiplies).
    pub fn rotate(&mut self, rotation: Quat) {
        self.rotation = (rotation * self.rotation).normalize();
    }

    /// Applies `rotation` around this transform's own axes (post-multiplies).
    pub fn rotate_local(&mut self, rotation: Quat) {
        self.rotation = (self.rotation * rotation).normalize();
    }

    pub fn is_finite(&self) -> bool {
        self.translation.is_finite() && self.rotation.is_finite() && self.scale.is_finite()
    }
}

impl Default for Transform3D {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl From<Transform3D> for Affine3A {
    fn from(t: Transform3D) -> Self {
        t.to_affine()
    }
}

impl From<Transform3D> for Mat4 {
    fn from(t: Transform3D) -> Self {
        t.to_matrix()
    }
}

/// Composition: `(a * b).transform_point(p) == a.transform_point(b.transform_point(p))`.
///
/// Exact only when the result has no shear (uniform scale in `a`, or no rotation in `b`);
/// otherwise compose as [`Affine3A`].
impl Mul for Transform3D {
    type Output = Transform3D;

    fn mul(self, rhs: Transform3D) -> Transform3D {
        Transform3D {
            translation: self.transform_point(rhs.translation),
            rotation: (self.rotation * rhs.rotation).normalize(),
            scale: self.scale * rhs.scale,
        }
    }
}

/// Rotation whose -Z axis points along `direction`, keeping +Y as close as possible to `up`.
pub fn look_rotation(direction: Vec3, up: Vec3) -> Option<Quat> {
    let forward = direction.try_normalize()?;
    let right = forward.cross(up).try_normalize()?;
    let up = right.cross(forward);
    Some(Quat::from_mat3(&Mat3::from_cols(right, up, -forward)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    fn approx(a: Vec3, b: Vec3) -> bool {
        a.abs_diff_eq(b, 1e-5)
    }

    #[test]
    fn identity_is_noop() {
        let p = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(Transform3D::IDENTITY.transform_point(p), p);
        assert_eq!(Transform3D::IDENTITY.to_matrix(), Mat4::IDENTITY);
    }

    #[test]
    fn transform_point_applies_scale_then_rotation_then_translation() {
        let t = Transform3D::from_trs(
            Vec3::new(10.0, 0.0, 0.0),
            Quat::from_rotation_y(FRAC_PI_2),
            Vec3::splat(2.0),
        );
        // (1,0,0) -> scale (2,0,0) -> rotate 90° about Y (0,0,-2) -> translate (10,0,-2)
        assert!(approx(
            t.transform_point(Vec3::X),
            Vec3::new(10.0, 0.0, -2.0)
        ));
        assert!(approx(
            t.to_affine().transform_point3(Vec3::X),
            Vec3::new(10.0, 0.0, -2.0)
        ));
    }

    #[test]
    fn composition_matches_matrix_product() {
        let parent = Transform3D::from_trs(
            Vec3::new(1.0, 2.0, 3.0),
            Quat::from_rotation_z(0.3),
            Vec3::splat(1.5),
        );
        let child = Transform3D::from_trs(
            Vec3::new(-1.0, 0.5, 2.0),
            Quat::from_rotation_x(1.1),
            Vec3::new(1.0, 2.0, 0.5),
        );
        let p = Vec3::new(0.3, -0.7, 1.2);
        let composed = parent * child;
        let matrix = parent.to_matrix() * child.to_matrix();
        assert!(approx(
            composed.transform_point(p),
            matrix.transform_point3(p)
        ));
    }

    #[test]
    fn affine_round_trip() {
        let t = Transform3D::from_trs(
            Vec3::new(4.0, -2.0, 1.0),
            Quat::from_euler(EulerRot::YXZ, 0.4, 0.2, -0.9),
            Vec3::new(1.0, 2.0, 3.0),
        );
        let back = Transform3D::from_affine(&t.to_affine());
        assert!(approx(back.translation, t.translation));
        assert!(approx(back.scale, t.scale));
        assert!(back.rotation.abs_diff_eq(t.rotation, 1e-5));
    }

    #[test]
    fn looking_at_points_forward_at_target() {
        let t = Transform3D::looking_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y);
        assert!(approx(t.forward(), Vec3::NEG_Z));

        let t = Transform3D::looking_at(Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0), Vec3::Y);
        assert!(approx(t.forward(), Vec3::X));
        assert!(approx(t.up(), Vec3::Y));
    }

    #[test]
    fn looking_at_degenerate_direction_keeps_rotation() {
        let t = Transform3D::looking_at(Vec3::ZERO, Vec3::new(0.0, 5.0, 0.0), Vec3::Y);
        assert_eq!(t.rotation, Quat::IDENTITY);
    }

    #[test]
    fn translate_local_follows_rotation() {
        let mut t = Transform3D::from_rotation(Quat::from_rotation_y(FRAC_PI_2));
        t.translate_local(Vec3::NEG_Z);
        assert!(approx(t.translation, Vec3::NEG_X));
    }
}
