//! Camera matrices and frustum culling.

use crate::mesh::Aabb;
use ab_core::camera::rh::{proj::directx, view};
use ab_core::{Affine3A, Mat4, Vec3, Vec4};

/// Everything the renderer needs to know about the active camera for one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraView {
    pub view: Mat4,
    pub projection: Mat4,
    pub view_projection: Mat4,
    pub position: Vec3,
    /// Unit vector the camera looks along (its -Z axis).
    pub forward: Vec3,
    pub fov_y_radians: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

impl CameraView {
    /// Builds the view from a camera's global transform. Scale on the camera node is ignored.
    pub fn new(global: &Affine3A, fov_y_radians: f32, aspect: f32, near: f32, far: f32) -> Self {
        let (_, rotation, position) = global.to_scale_rotation_translation();
        let forward = rotation * Vec3::NEG_Z;
        let view = view::look_to_mat4(position, forward, rotation * Vec3::Y);
        let near = near.max(1e-4);
        let far = far.max(near + 1e-3);
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        let projection = directx::perspective(fov_y_radians, aspect, near, far);
        Self {
            view,
            projection,
            view_projection: projection * view,
            position,
            forward,
            fov_y_radians,
            aspect,
            near,
            far,
        }
    }

    /// World-space corners of the frustum slice between `near` and `far` distances.
    pub fn slice_corners(&self, near: f32, far: f32) -> [Vec3; 8] {
        let inverse_view = self.view.inverse();
        let tan_y = (self.fov_y_radians * 0.5).tan();
        let tan_x = tan_y * self.aspect;
        let mut corners = [Vec3::ZERO; 8];
        for (i, d) in [near, far].into_iter().enumerate() {
            for (j, (sx, sy)) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                .into_iter()
                .enumerate()
            {
                let local = Vec3::new(sx * tan_x * d, sy * tan_y * d, -d);
                corners[i * 4 + j] = inverse_view.transform_point3(local);
            }
        }
        corners
    }
}

/// Six inward-facing planes (`xyz` = normal, `w` = distance) extracted from a view-projection
/// matrix with wgpu's 0..1 depth range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    pub fn from_view_projection(m: &Mat4) -> Self {
        let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
        let planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2].map(|p| {
            let length = p.truncate().length();
            if length > 0.0 {
                p / length
            } else {
                p
            }
        });
        Self { planes }
    }

    /// Conservative test: `false` only if the box is entirely outside one plane.
    pub fn intersects_aabb(&self, aabb: &Aabb) -> bool {
        let center = aabb.center();
        let half = aabb.half_extents();
        self.planes.iter().all(|plane| {
            let normal = plane.truncate();
            normal.dot(center) + plane.w + normal.abs().dot(half) >= 0.0
        })
    }

    pub fn contains_point(&self, point: Vec3) -> bool {
        self.planes
            .iter()
            .all(|p| p.truncate().dot(point) + p.w >= 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_core::{Quat, Transform3D};

    fn camera_at(position: Vec3, target: Vec3) -> CameraView {
        let t = Transform3D::looking_at(position, target, Vec3::Y);
        CameraView::new(&t.to_affine(), 60f32.to_radians(), 16.0 / 9.0, 0.1, 100.0)
    }

    #[test]
    fn view_projection_maps_target_to_screen_centre() {
        let cam = camera_at(Vec3::new(0.0, 2.0, 10.0), Vec3::new(0.0, 2.0, 0.0));
        let clip = cam.view_projection * Vec3::new(0.0, 2.0, 0.0).extend(1.0);
        let ndc = clip.truncate() / clip.w;
        assert!(ndc.x.abs() < 1e-5 && ndc.y.abs() < 1e-5);
        assert!((0.0..1.0).contains(&ndc.z));
        assert!(cam.forward.abs_diff_eq(Vec3::NEG_Z, 1e-5));
    }

    #[test]
    fn camera_scale_is_ignored() {
        let mut t = Transform3D::looking_at(Vec3::new(1.0, 2.0, 3.0), Vec3::ZERO, Vec3::Y);
        let a = CameraView::new(&t.to_affine(), 1.0, 1.0, 0.1, 10.0);
        t.scale = Vec3::new(3.0, 0.5, 2.0);
        let b = CameraView::new(&t.to_affine(), 1.0, 1.0, 0.1, 10.0);
        assert!(a.view.abs_diff_eq(b.view, 1e-4));
    }

    #[test]
    fn frustum_culls_boxes() {
        let cam = camera_at(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO);
        let frustum = Frustum::from_view_projection(&cam.view_projection);
        let unit = |c: Vec3| Aabb {
            min: c - 0.5,
            max: c + 0.5,
        };
        assert!(frustum.intersects_aabb(&unit(Vec3::ZERO)));
        assert!(
            !frustum.intersects_aabb(&unit(Vec3::new(0.0, 0.0, 20.0))),
            "behind"
        );
        assert!(
            !frustum.intersects_aabb(&unit(Vec3::new(0.0, 0.0, -200.0))),
            "beyond far"
        );
        assert!(
            !frustum.intersects_aabb(&unit(Vec3::new(50.0, 0.0, 0.0))),
            "off to the side"
        );
        // Straddling the left edge still counts as visible.
        let edge_x = (cam.fov_y_radians * 0.5).tan() * cam.aspect * 10.0;
        assert!(frustum.intersects_aabb(&unit(Vec3::new(-edge_x - 0.4, 0.0, 0.0))));
    }

    #[test]
    fn slice_corners_lie_on_frustum_boundary() {
        let t = Transform3D::from_rotation(Quat::from_rotation_y(0.8)).with_translation(Vec3::ONE);
        let cam = CameraView::new(&t.to_affine(), 1.2, 1.5, 0.1, 50.0);
        for corner in cam.slice_corners(1.0, 20.0) {
            let clip = cam.view_projection * corner.extend(1.0);
            let ndc = clip.truncate() / clip.w;
            assert!((ndc.x.abs() - 1.0).abs() < 1e-3 && (ndc.y.abs() - 1.0).abs() < 1e-3);
        }
    }
}
