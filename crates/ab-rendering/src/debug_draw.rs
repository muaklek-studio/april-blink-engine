//! Immediate-mode debug drawing: lines and wire shapes from gameplay code, drawn by the
//! renderer over the scene.
//!
//! ```
//! use ab_core::{Color, Vec3};
//! use ab_rendering::DebugDraw;
//!
//! let mut debug = DebugDraw::default();
//! debug.line(Vec3::ZERO, Vec3::Y, Color::rgb(1.0, 0.0, 0.0)); // this frame only
//! debug.sphere(Vec3::new(0.0, 1.0, 0.0), 0.5, Color::WHITE);
//! debug.timed(2.0).arrow(Vec3::ZERO, Vec3::X, Color::rgb(0.0, 1.0, 0.0)); // two seconds
//! debug.on_top().cross(Vec3::ZERO, 0.2, Color::WHITE); // not hidden by geometry
//! assert!(debug.line_count() > 10);
//! ```
//!
//! In scripts: `tree.world().resource_mut::<DebugDraw>()`. Lines without a duration last one
//! frame: draw them every frame you want them (the engine clears them at the start of each
//! frame, see [`debug_draw_system`]).

use ab_core::{Affine3A, Color, Time, Vec3};
use ab_ecs::World;

/// Lines kept at most; further lines are dropped (a runaway loop cannot exhaust memory).
pub const MAX_DEBUG_LINES: usize = 200_000;

const CIRCLE_SEGMENTS: usize = 24;

/// One line segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DebugLine {
    pub a: Vec3,
    pub b: Vec3,
    pub color: Color,
    /// Drawn over everything instead of hidden by closer geometry.
    pub on_top: bool,
}

/// ECS resource: the debug lines to draw.
#[derive(Debug, Clone)]
pub struct DebugDraw {
    /// Draw nothing (lines are still collected).
    pub enabled: bool,
    lines: Vec<DebugLine>,
    /// Lines with seconds left.
    timed: Vec<(DebugLine, f32)>,
}

impl Default for DebugDraw {
    fn default() -> Self {
        Self {
            enabled: true,
            lines: Vec::new(),
            timed: Vec::new(),
        }
    }
}

/// Draws shapes with a style: depth-tested or on top, for one frame or for a duration.
#[derive(Debug)]
pub struct DebugBrush<'a> {
    draw: &'a mut DebugDraw,
    on_top: bool,
    duration: f32,
}

impl DebugDraw {
    /// A brush for this frame, depth-tested.
    pub fn brush(&mut self) -> DebugBrush<'_> {
        DebugBrush {
            draw: self,
            on_top: false,
            duration: 0.0,
        }
    }

    /// A brush drawing over all geometry.
    pub fn on_top(&mut self) -> DebugBrush<'_> {
        self.brush().on_top()
    }

    /// A brush whose shapes stay for `seconds`.
    pub fn timed(&mut self, seconds: f32) -> DebugBrush<'_> {
        self.brush().duration(seconds)
    }

    pub fn line(&mut self, a: Vec3, b: Vec3, color: Color) {
        self.brush().line(a, b, color);
    }

    pub fn ray(&mut self, origin: Vec3, direction: Vec3, color: Color) {
        self.brush().ray(origin, direction, color);
    }

    pub fn arrow(&mut self, from: Vec3, to: Vec3, color: Color) {
        self.brush().arrow(from, to, color);
    }

    pub fn cross(&mut self, at: Vec3, size: f32, color: Color) {
        self.brush().cross(at, size, color);
    }

    pub fn circle(&mut self, center: Vec3, normal: Vec3, radius: f32, color: Color) {
        self.brush().circle(center, normal, radius, color);
    }

    pub fn sphere(&mut self, center: Vec3, radius: f32, color: Color) {
        self.brush().sphere(center, radius, color);
    }

    pub fn aabb(&mut self, min: Vec3, max: Vec3, color: Color) {
        self.brush().aabb(min, max, color);
    }

    pub fn cube(&mut self, transform: &Affine3A, half_extents: Vec3, color: Color) {
        self.brush().cube(transform, half_extents, color);
    }

    pub fn capsule(&mut self, a: Vec3, b: Vec3, radius: f32, color: Color) {
        self.brush().capsule(a, b, radius, color);
    }

    pub fn axes(&mut self, transform: &Affine3A, size: f32) {
        self.brush().axes(transform, size);
    }

    pub fn path(&mut self, points: &[Vec3], color: Color) {
        self.brush().path(points, color);
    }

    /// Lines to draw now (this frame's and timed ones).
    pub fn lines(&self) -> impl Iterator<Item = &DebugLine> {
        self.lines.iter().chain(self.timed.iter().map(|(l, _)| l))
    }

    pub fn line_count(&self) -> usize {
        self.lines.len() + self.timed.len()
    }

    /// Drops this frame's lines and ages timed ones by `delta` seconds.
    pub fn begin_frame(&mut self, delta: f32) {
        self.lines.clear();
        self.timed.retain_mut(|(_, left)| {
            *left -= delta;
            *left > 0.0
        });
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.timed.clear();
    }

    fn push(&mut self, line: DebugLine, duration: f32) {
        if self.line_count() >= MAX_DEBUG_LINES {
            return;
        }
        if duration > 0.0 {
            self.timed.push((line, duration));
        } else {
            self.lines.push(line);
        }
    }
}

impl DebugBrush<'_> {
    pub fn on_top(mut self) -> Self {
        self.on_top = true;
        self
    }

    pub fn duration(mut self, seconds: f32) -> Self {
        self.duration = seconds.max(0.0);
        self
    }

    pub fn line(&mut self, a: Vec3, b: Vec3, color: Color) -> &mut Self {
        let line = DebugLine {
            a,
            b,
            color,
            on_top: self.on_top,
        };
        self.draw.push(line, self.duration);
        self
    }

    pub fn ray(&mut self, origin: Vec3, direction: Vec3, color: Color) -> &mut Self {
        self.line(origin, origin + direction, color)
    }

    /// A line with a small head at `to`.
    pub fn arrow(&mut self, from: Vec3, to: Vec3, color: Color) -> &mut Self {
        let direction = to - from;
        let length = direction.length();
        self.line(from, to, color);
        if length > 1e-5 {
            let forward = direction / length;
            let side = forward.any_orthonormal_vector();
            let up = forward.cross(side);
            let head = (length * 0.2).min(0.3);
            let base = to - forward * head;
            for offset in [side, -side, up, -up] {
                self.line(to, base + offset * head * 0.5, color);
            }
        }
        self
    }

    pub fn cross(&mut self, at: Vec3, size: f32, color: Color) -> &mut Self {
        let h = size * 0.5;
        self.line(at - Vec3::X * h, at + Vec3::X * h, color);
        self.line(at - Vec3::Y * h, at + Vec3::Y * h, color);
        self.line(at - Vec3::Z * h, at + Vec3::Z * h, color)
    }

    pub fn circle(&mut self, center: Vec3, normal: Vec3, radius: f32, color: Color) -> &mut Self {
        let normal = normal.normalize_or(Vec3::Y);
        let u = normal.any_orthonormal_vector() * radius;
        let v = normal.cross(u.normalize_or(Vec3::X)) * radius;
        let point = |i: usize| {
            let angle = i as f32 / CIRCLE_SEGMENTS as f32 * std::f32::consts::TAU;
            center + u * angle.cos() + v * angle.sin()
        };
        for i in 0..CIRCLE_SEGMENTS {
            self.line(point(i), point(i + 1), color);
        }
        self
    }

    /// Three great circles.
    pub fn sphere(&mut self, center: Vec3, radius: f32, color: Color) -> &mut Self {
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            self.circle(center, axis, radius, color);
        }
        self
    }

    pub fn aabb(&mut self, min: Vec3, max: Vec3, color: Color) -> &mut Self {
        let center = (min + max) * 0.5;
        self.cube(
            &Affine3A::from_translation(center),
            (max - min) * 0.5,
            color,
        )
    }

    /// A box of `half_extents` in `transform`'s space.
    pub fn cube(&mut self, transform: &Affine3A, half_extents: Vec3, color: Color) -> &mut Self {
        let corner = |i: usize| {
            transform.transform_point3(Vec3::new(
                if i & 1 == 0 {
                    -half_extents.x
                } else {
                    half_extents.x
                },
                if i & 2 == 0 {
                    -half_extents.y
                } else {
                    half_extents.y
                },
                if i & 4 == 0 {
                    -half_extents.z
                } else {
                    half_extents.z
                },
            ))
        };
        for (a, b) in [
            (0, 1),
            (1, 3),
            (3, 2),
            (2, 0),
            (4, 5),
            (5, 7),
            (7, 6),
            (6, 4),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ] {
            self.line(corner(a), corner(b), color);
        }
        self
    }

    /// A capsule between the centres of its end spheres.
    pub fn capsule(&mut self, a: Vec3, b: Vec3, radius: f32, color: Color) -> &mut Self {
        let axis = (b - a).normalize_or(Vec3::Y);
        let side = axis.any_orthonormal_vector();
        let up = axis.cross(side);
        for end in [a, b] {
            self.circle(end, axis, radius, color);
            self.circle(end, side, radius, color);
            self.circle(end, up, radius, color);
        }
        for offset in [side, -side, up, -up] {
            self.line(a + offset * radius, b + offset * radius, color);
        }
        self
    }

    /// X (red), Y (green) and Z (blue) axes of a transform.
    pub fn axes(&mut self, transform: &Affine3A, size: f32) -> &mut Self {
        let origin: Vec3 = transform.translation.into();
        for (axis, color) in [
            (transform.matrix3.x_axis, Color::rgb(1.0, 0.2, 0.2)),
            (transform.matrix3.y_axis, Color::rgb(0.2, 1.0, 0.2)),
            (transform.matrix3.z_axis, Color::rgb(0.3, 0.5, 1.0)),
        ] {
            self.line(
                origin,
                origin + Vec3::from(axis).normalize_or_zero() * size,
                color,
            );
        }
        self
    }

    /// A polyline through `points`.
    pub fn path(&mut self, points: &[Vec3], color: Color) -> &mut Self {
        for pair in points.windows(2) {
            self.line(pair[0], pair[1], color);
        }
        self
    }
}

/// Starts a frame of debug drawing: clears last frame's lines and ages timed ones. The
/// engine runs it first in `PreUpdate`.
pub fn debug_draw_system(world: &mut World) {
    let delta = world.resource::<Time>().map_or(0.0, |t| t.delta);
    if let Some(mut debug) = world.resource_mut::<DebugDraw>() {
        debug.begin_frame(delta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_and_timed_lines() {
        let mut debug = DebugDraw::default();
        debug.line(Vec3::ZERO, Vec3::X, Color::WHITE);
        debug.timed(1.0).line(Vec3::ZERO, Vec3::Y, Color::WHITE);
        debug
            .on_top()
            .cube(&Affine3A::IDENTITY, Vec3::ONE, Color::WHITE);
        assert_eq!(debug.line_count(), 14);
        assert_eq!(debug.lines().filter(|l| l.on_top).count(), 12);
        debug.begin_frame(0.5);
        assert_eq!(debug.line_count(), 1, "only the timed line survives");
        debug.begin_frame(0.6);
        assert_eq!(debug.line_count(), 0, "expired");
    }

    #[test]
    fn shapes_and_limits() {
        let mut debug = DebugDraw::default();
        debug.sphere(Vec3::ZERO, 1.0, Color::WHITE);
        assert_eq!(debug.line_count(), 3 * CIRCLE_SEGMENTS);
        // Circle points stay on the radius.
        assert!(debug
            .lines()
            .all(|l| (l.a.length() - 1.0).abs() < 1e-4 && (l.b.length() - 1.0).abs() < 1e-4));
        debug.clear();
        debug.arrow(Vec3::ZERO, Vec3::new(0.0, 0.0, 2.0), Color::WHITE);
        assert_eq!(debug.line_count(), 5);
        debug.clear();
        for _ in 0..MAX_DEBUG_LINES + 10 {
            debug.line(Vec3::ZERO, Vec3::X, Color::WHITE);
        }
        assert_eq!(debug.line_count(), MAX_DEBUG_LINES);
    }
}
