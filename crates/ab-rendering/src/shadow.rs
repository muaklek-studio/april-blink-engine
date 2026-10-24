//! Light-space matrices for shadow maps.

use crate::camera::CameraView;
use ab_core::camera::rh::{proj::directx, view};
use ab_core::{Mat4, Vec3};

/// Extra depth (metres) kept towards the light so off-screen casters still cast into view.
const CASTER_EXTENSION: f32 = 100.0;
const SPOT_NEAR: f32 = 0.05;

fn stable_up(direction: Vec3) -> Vec3 {
    if direction.normalize_or_zero().dot(Vec3::Y).abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    }
}

/// Orthographic light matrix covering the camera frustum up to `distance` metres.
///
/// The covered sphere's radius depends only on the camera's projection, and its centre is
/// snapped to shadow-map texels, so shadows do not shimmer as the camera moves or turns.
pub fn directional_shadow_matrix(
    light_direction: Vec3,
    camera: &CameraView,
    distance: f32,
    map_size: u32,
) -> Mat4 {
    let direction = light_direction.normalize_or(Vec3::NEG_Y);
    // `max`/`min` rather than `clamp`: a camera's far plane may sit closer than near + 0.01.
    let far = distance.min(camera.far).max(camera.near + 0.01);
    let corners = camera.slice_corners(camera.near, far);

    // Centre on the view axis so the radius is rotation invariant.
    let centre_distance = (camera.near + far) * 0.5;
    let world_centre = camera.position + camera.forward * centre_distance;
    let radius = corners
        .iter()
        .map(|c| c.distance(world_centre))
        .fold(0.0f32, f32::max)
        .ceil();

    let light_view = view::look_to_mat4(Vec3::ZERO, direction, stable_up(direction));
    let mut centre = light_view.transform_point3(world_centre);
    let texel = 2.0 * radius / map_size.max(1) as f32;
    centre.x = (centre.x / texel).floor() * texel;
    centre.y = (centre.y / texel).floor() * texel;

    let projection = directx::orthographic(
        centre.x - radius,
        centre.x + radius,
        centre.y - radius,
        centre.y + radius,
        -centre.z - radius - CASTER_EXTENSION,
        -centre.z + radius,
    );
    projection * light_view
}

/// Perspective light matrix for a spot light's cone.
pub fn spot_shadow_matrix(position: Vec3, direction: Vec3, angle_radians: f32, range: f32) -> Mat4 {
    let direction = direction.normalize_or(Vec3::NEG_Z);
    // Slight margin so PCF taps at the cone edge stay inside the map.
    let fov = (angle_radians * 2.0 * 1.1).clamp(0.1, 3.0);
    let projection = directx::perspective(fov, 1.0, SPOT_NEAR, range.max(SPOT_NEAR + 0.1));
    projection * view::look_to_mat4(position, direction, stable_up(direction))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_core::{Quat, Transform3D};

    fn in_clip_volume(m: &Mat4, p: Vec3) -> bool {
        let clip = *m * p.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        ndc.x.abs() <= 1.0 + 1e-4 && ndc.y.abs() <= 1.0 + 1e-4 && (-1e-4..=1.0001).contains(&ndc.z)
    }

    #[test]
    fn directional_covers_the_camera_slice() {
        let t = Transform3D::looking_at(Vec3::new(3.0, 2.0, 8.0), Vec3::ZERO, Vec3::Y);
        let cam = CameraView::new(&t.to_affine(), 1.2, 16.0 / 9.0, 0.1, 500.0);
        let light = Vec3::new(-0.3, -1.0, -0.4);
        let m = directional_shadow_matrix(light, &cam, 30.0, 2048);
        for corner in cam.slice_corners(cam.near, 30.0) {
            assert!(in_clip_volume(&m, corner), "{corner}");
        }
        // A caster well above the slice (towards the light) is still inside the depth range.
        let above = cam.position + Vec3::Y * 40.0;
        let clip = m * above.extend(1.0);
        assert!((0.0..=1.0).contains(&clip.z));
    }

    #[test]
    fn directional_extent_is_rotation_invariant() {
        let light = Vec3::new(0.2, -1.0, 0.1);
        let extent = |yaw: f32| {
            let t = Transform3D::from_rotation(Quat::from_rotation_y(yaw));
            let cam = CameraView::new(&t.to_affine(), 1.0, 1.5, 0.1, 100.0);
            let m = directional_shadow_matrix(light, &cam, 25.0, 1024);
            m.x_axis.truncate().length()
        };
        assert!((extent(0.0) - extent(1.3)).abs() < 1e-6);
    }

    #[test]
    fn straight_down_light_is_not_degenerate() {
        let cam = CameraView::new(&Transform3D::IDENTITY.to_affine(), 1.0, 1.0, 0.1, 100.0);
        let m = directional_shadow_matrix(Vec3::NEG_Y, &cam, 20.0, 1024);
        assert!(m.is_finite());
    }

    #[test]
    fn spot_matrix_covers_cone() {
        let position = Vec3::new(0.0, 4.0, 0.0);
        let m = spot_shadow_matrix(position, Vec3::NEG_Y, 30f32.to_radians(), 10.0);
        assert!(in_clip_volume(&m, Vec3::ZERO));
        let edge = position + Quat::from_rotation_z(29f32.to_radians()) * Vec3::NEG_Y * 5.0;
        assert!(in_clip_volume(&m, edge));
        assert!(!in_clip_volume(&m, position + Vec3::Y));
    }
}
