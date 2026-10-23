//! The build tool: put levels together from pieces, like bricks.
//!
//! Pick a piece (a model or a prefab) in the Build panel and a copy of it follows the
//! pointer in the viewport, resting against what is under it: on top of the ground or
//! another piece, or against a piece's side, lined up with its bottom. Its position snaps
//! to the grid (the snap step; hold Alt for none). R turns it a quarter, a click places it
//! and the next one is ready; Esc or a right-click puts the tool away.

use ab_engine::core::Vec3;
use ab_engine::rendering::Aabb;

/// What the pointer is over: where, the surface's outward axis, and the bounds of the
/// object hit (`None`: the ground).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    pub point: Vec3,
    /// A unit world axis (±X, ±Y, ±Z).
    pub normal: Vec3,
    pub support: Option<Aabb>,
}

/// `value` to the nearest multiple of `step` (no snapping for steps ≤ 0).
fn snap(value: f32, step: f32) -> f32 {
    if step > 0.0 {
        (value / step).round() * step
    } else {
        value
    }
}

/// The world axis closest to `v`.
pub fn dominant_axis(v: Vec3) -> Vec3 {
    let a = v.abs();
    if a.y >= a.x && a.y >= a.z {
        Vec3::Y * v.y.signum()
    } else if a.x >= a.z {
        Vec3::X * v.x.signum()
    } else {
        Vec3::Z * v.z.signum()
    }
}

/// Where a piece goes: its origin, given its bounds around its origin (`piece`, already
/// turned) and the surface under the pointer. The piece's face opposite the surface's
/// normal touches the surface; along the other axes it is centred on the point and
/// snapped to `step`. Against a side, its bottom lines up with the bottom of what it leans on.
pub fn placement(surface: &Surface, piece: &Aabb, step: f32) -> Vec3 {
    let n = surface.normal;
    let centre = piece.center();
    let mut origin = Vec3::ZERO;
    for axis in 0..3 {
        let along = n[axis];
        origin[axis] = if along > 0.5 {
            surface.point[axis] - piece.min[axis]
        } else if along < -0.5 {
            surface.point[axis] - piece.max[axis]
        } else if axis == 1 {
            // Leaning against a side: on the same floor as the support.
            match surface.support {
                Some(support) => support.min.y - piece.min.y,
                None => surface.point.y - piece.min.y,
            }
        } else {
            snap(surface.point[axis] - centre[axis], step)
        };
    }
    origin
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_box() -> Aabb {
        Aabb {
            min: Vec3::new(-0.5, 0.0, -0.5),
            max: Vec3::new(0.5, 1.0, 0.5),
        }
    }

    #[test]
    fn pieces_sit_on_the_ground_on_the_grid() {
        let ground = Surface {
            point: Vec3::new(1.3, 0.0, -0.2),
            normal: Vec3::Y,
            support: None,
        };
        assert_eq!(
            placement(&ground, &unit_box(), 0.5),
            Vec3::new(1.5, 0.0, 0.0)
        );
        // Without snapping: right under the pointer.
        assert_eq!(
            placement(&ground, &unit_box(), 0.0),
            Vec3::new(1.3, 0.0, -0.2)
        );
    }

    #[test]
    fn pieces_stack_and_line_up_against_sides() {
        // On top of a box whose top is at y = 1.
        let top = Surface {
            point: Vec3::new(0.1, 1.0, 0.1),
            normal: Vec3::Y,
            support: Some(unit_box()),
        };
        assert_eq!(placement(&top, &unit_box(), 1.0), Vec3::new(0.0, 1.0, 0.0));
        // Against its +X side, halfway up: next to it, on the same floor.
        let side = Surface {
            point: Vec3::new(0.5, 0.6, 0.2),
            normal: Vec3::X,
            support: Some(unit_box()),
        };
        assert_eq!(placement(&side, &unit_box(), 1.0), Vec3::new(1.0, 0.0, 0.0));
        // Against its -Z side.
        let back = Surface {
            point: Vec3::new(0.0, 0.3, -0.5),
            normal: Vec3::NEG_Z,
            support: Some(unit_box()),
        };
        assert_eq!(
            placement(&back, &unit_box(), 1.0),
            Vec3::new(0.0, 0.0, -1.0)
        );
    }

    #[test]
    fn surfaces_snap_to_axes() {
        assert_eq!(dominant_axis(Vec3::new(0.1, 0.9, 0.2)), Vec3::Y);
        assert_eq!(dominant_axis(Vec3::new(-0.8, 0.1, 0.3)), Vec3::NEG_X);
        assert_eq!(dominant_axis(Vec3::new(0.0, 0.2, -0.5)), Vec3::NEG_Z);
    }
}
