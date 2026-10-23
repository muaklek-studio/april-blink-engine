//! Splines: smooth paths through control points, and what is built along them.
//!
//! A `Path3D` node holds points (in its own space) joined by a centripetal Catmull-Rom curve:
//! it passes through every point without loops or overshoot. Along it the node can build:
//!
//! * a **ribbon**: a flat strip `width` wide (roads, rivers, paths), with collision if asked,
//! * a **tube**: `radius` round, `sides` many (pipes, cables; `sag` makes cables hang),
//! * **instances** of a model or prefab every `spacing` metres (fences, sleepers,
//!   streetlights), turned along the path.
//!
//! ```toml
//! [[node]]
//! name = "Road"
//! type = "Path3D"
//! path = { points = [[0, 0, 0], [10, 0, 2], [20, 0, -3]], mode = "ribbon", width = 4.0, collide = true }
//! material = { color = "#3a3a3c", roughness = 0.9 }
//! ```
//!
//! What is built is regenerated from the points when the scene loads (it is not saved).
//! Scripts sample the path by distance (`Tree::path_point`) to move along it.

use ab_core::{Quat, Vec3};
use ab_rendering::{Mesh, Vertex};

/// How dense the generated geometry is: curve samples per segment between two points.
pub const SAMPLES_PER_SEGMENT: usize = 12;

/// A curve through points (centripetal Catmull-Rom), sampled into a polyline measured
/// along its length.
#[derive(Debug, Clone, PartialEq)]
pub struct Spline {
    pub points: Vec<Vec3>,
    pub closed: bool,
    /// The sampled curve: positions and the distance along the curve at each.
    samples: Vec<(Vec3, f32)>,
}

impl Spline {
    pub fn new(points: Vec<Vec3>, closed: bool) -> Self {
        let mut spline = Self {
            points,
            closed,
            samples: Vec::new(),
        };
        spline.resample(SAMPLES_PER_SEGMENT);
        spline
    }

    fn segment_count(&self) -> usize {
        match (self.points.len(), self.closed) {
            (0 | 1, _) => 0,
            (n, true) => n,
            (n, false) => n - 1,
        }
    }

    fn point(&self, i: isize) -> Vec3 {
        let n = self.points.len() as isize;
        if self.closed {
            self.points[i.rem_euclid(n) as usize]
        } else if i < 0 {
            // Mirror the end so the curve leaves the first point straight.
            2.0 * self.points[0] - self.points[1.min(n as usize - 1)]
        } else if i >= n {
            let last = n as usize - 1;
            2.0 * self.points[last] - self.points[last.saturating_sub(1)]
        } else {
            self.points[i as usize]
        }
    }

    /// The curve at `t` (0..1) of segment `segment`.
    pub fn segment_point(&self, segment: usize, t: f32) -> Vec3 {
        let i = segment as isize;
        let (p0, p1, p2, p3) = (self.point(i - 1), self.point(i), self.point(i + 1), self.point(i + 2));
        centripetal(p0, p1, p2, p3, t)
    }

    fn resample(&mut self, per_segment: usize) {
        self.samples.clear();
        match self.points.len() {
            0 => return,
            1 => {
                self.samples.push((self.points[0], 0.0));
                return;
            }
            _ => {}
        }
        let mut length = 0.0;
        let mut last: Option<Vec3> = None;
        for segment in 0..self.segment_count() {
            let first = if segment == 0 { 0 } else { 1 };
            for s in first..=per_segment {
                let p = self.segment_point(segment, s as f32 / per_segment as f32);
                if let Some(prev) = last {
                    length += prev.distance(p);
                }
                self.samples.push((p, length));
                last = Some(p);
            }
        }
    }

    /// Length along the curve.
    pub fn length(&self) -> f32 {
        self.samples.last().map_or(0.0, |s| s.1)
    }

    /// The sampled curve (positions in the path's space).
    pub fn polyline(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.samples.iter().map(|s| s.0)
    }

    /// The position and unit direction at `distance` along the curve (clamped; a closed
    /// path wraps around).
    pub fn at_distance(&self, distance: f32) -> Option<(Vec3, Vec3)> {
        let length = self.length();
        let (first, _) = *self.samples.first()?;
        if self.samples.len() < 2 || length <= 0.0 {
            return Some((first, Vec3::Z));
        }
        let d = if self.closed {
            distance.rem_euclid(length)
        } else {
            distance.clamp(0.0, length)
        };
        let next = self.samples.partition_point(|s| s.1 < d).clamp(1, self.samples.len() - 1);
        let (a, da) = self.samples[next - 1];
        let (b, db) = self.samples[next];
        let t = if db > da { (d - da) / (db - da) } else { 0.0 };
        Some((a.lerp(b, t), (b - a).normalize_or(Vec3::Z)))
    }

    /// Frames along the curve every `step` metres (at least at both ends): position,
    /// direction, and a sideways vector level with the ground.
    pub fn frames(&self, step: f32) -> Vec<(Vec3, Vec3, Vec3)> {
        let length = self.length();
        let count = ((length / step.max(0.01)).ceil() as usize).max(1);
        (0..=count)
            .filter_map(|i| self.at_distance(length * i as f32 / count as f32))
            .map(|(p, forward)| (p, forward, side_of(forward)))
            .collect()
    }

    /// The index of the segment nearest `point`, and where on it (for inserting a point).
    pub fn nearest_segment(&self, point: Vec3) -> Option<(usize, f32)> {
        let mut best: Option<(usize, f32, f32)> = None;
        for segment in 0..self.segment_count() {
            for s in 0..=SAMPLES_PER_SEGMENT {
                let t = s as f32 / SAMPLES_PER_SEGMENT as f32;
                let d = self.segment_point(segment, t).distance_squared(point);
                if best.is_none_or(|(_, _, bd)| d < bd) {
                    best = Some((segment, t, d));
                }
            }
        }
        best.map(|(s, t, _)| (s, t))
    }
}

/// A horizontal vector to the right of `forward` (a level road on a slope stays level).
fn side_of(forward: Vec3) -> Vec3 {
    let side = forward.cross(Vec3::Y);
    if side.length_squared() < 1e-6 {
        Vec3::X
    } else {
        side.normalize()
    }
}

/// Centripetal Catmull-Rom between `p1` and `p2`.
fn centripetal(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let knot = |a: Vec3, b: Vec3| a.distance(b).sqrt().max(1e-4);
    let t0 = 0.0;
    let t1 = t0 + knot(p0, p1);
    let t2 = t1 + knot(p1, p2);
    let t3 = t2 + knot(p2, p3);
    let t = t1 + (t2 - t1) * t;
    let a1 = p0 * ((t1 - t) / (t1 - t0)) + p1 * ((t - t0) / (t1 - t0));
    let a2 = p1 * ((t2 - t) / (t2 - t1)) + p2 * ((t - t1) / (t2 - t1));
    let a3 = p2 * ((t3 - t) / (t3 - t2)) + p3 * ((t - t2) / (t3 - t2));
    let b1 = a1 * ((t2 - t) / (t2 - t0)) + a2 * ((t - t0) / (t2 - t0));
    let b2 = a2 * ((t3 - t) / (t3 - t1)) + a3 * ((t - t1) / (t3 - t1));
    b1 * ((t2 - t) / (t2 - t1)) + b2 * ((t - t1) / (t2 - t1))
}

/// A strip `width` wide along the path, facing up, its UVs running along it (u across,
/// v in metres along: textures repeat every metre).
pub fn ribbon(spline: &Spline, width: f32) -> Option<Mesh> {
    if spline.length() < 1e-4 {
        return None;
    }
    let frames = spline.frames(spline.length() / (spline.points.len().max(2) * SAMPLES_PER_SEGMENT) as f32);
    if frames.len() < 2 {
        return None;
    }
    let half = width * 0.5;
    let mut vertices = Vec::with_capacity(frames.len() * 2);
    let mut along = 0.0;
    for (i, &(p, forward, side)) in frames.iter().enumerate() {
        if i > 0 {
            along += frames[i - 1].0.distance(p);
        }
        let up = side.cross(forward).normalize_or(Vec3::Y);
        for (offset, u) in [(-half, 0.0), (half, 1.0)] {
            vertices.push(vertex(p + side * offset, up, [u, along], side));
        }
    }
    let mut indices = Vec::new();
    for i in 0..frames.len() as u32 - 1 {
        let (a, b, c, d) = (i * 2, i * 2 + 1, i * 2 + 2, i * 2 + 3);
        indices.extend([a, c, b, b, c, d]);
    }
    Some(Mesh::new(vertices, indices))
}

/// A tube of `radius` with `sides` around the path. `sag` lowers its middle (a hanging
/// cable): by `sag` metres at the middle of the path.
pub fn tube(spline: &Spline, radius: f32, sides: usize, sag: f32) -> Option<Mesh> {
    if spline.length() < 1e-4 {
        return None;
    }
    let sides = sides.clamp(3, 64);
    let frames = spline.frames(spline.length() / (spline.points.len().max(2) * SAMPLES_PER_SEGMENT) as f32);
    if frames.len() < 2 {
        return None;
    }
    let last = (frames.len() - 1) as f32;
    let mut vertices = Vec::new();
    let mut along = 0.0;
    for (i, &(p, forward, side)) in frames.iter().enumerate() {
        if i > 0 {
            along += frames[i - 1].0.distance(p);
        }
        let k = i as f32 / last;
        let p = p - Vec3::Y * sag * 4.0 * k * (1.0 - k);
        let up = side.cross(forward).normalize_or(Vec3::Y);
        for s in 0..=sides {
            let angle = s as f32 / sides as f32 * std::f32::consts::TAU;
            let normal = side * angle.cos() + up * angle.sin();
            vertices.push(vertex(p + normal * radius, normal, [s as f32 / sides as f32, along], forward));
        }
    }
    let ring = sides as u32 + 1;
    let mut indices = Vec::new();
    for i in 0..frames.len() as u32 - 1 {
        for s in 0..sides as u32 {
            let (a, b) = (i * ring + s, i * ring + s + 1);
            let (c, d) = (a + ring, b + ring);
            indices.extend([a, b, c, b, d, c]);
        }
    }
    Some(Mesh::new(vertices, indices))
}

/// Where instances go: every `spacing` metres from the start (and at the end of an open
/// path), each turned to face along the path when `align`.
pub fn instance_places(spline: &Spline, spacing: f32, align: bool) -> Vec<(Vec3, Quat)> {
    let length = spline.length();
    if spline.points.is_empty() {
        return Vec::new();
    }
    let spacing = spacing.max(0.05);
    let mut count = (length / spacing).floor() as usize;
    if !spline.closed {
        count += 1;
    }
    (0..count.max(1))
        .filter_map(|i| spline.at_distance(i as f32 * spacing))
        .map(|(p, forward)| {
            let rotation = if align {
                let flat = Vec3::new(forward.x, 0.0, forward.z).normalize_or(Vec3::Z);
                Quat::from_rotation_y(flat.x.atan2(flat.z))
            } else {
                Quat::IDENTITY
            };
            (p, rotation)
        })
        .collect()
}

fn vertex(position: Vec3, normal: Vec3, uv: [f32; 2], tangent: Vec3) -> Vertex {
    Vertex {
        position: position.to_array(),
        normal: normal.to_array(),
        uv,
        tangent: [tangent.x, tangent.y, tangent.z, 1.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn straight() -> Spline {
        Spline::new(vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)], false)
    }

    #[test]
    fn the_curve_passes_through_its_points() {
        let points = vec![Vec3::ZERO, Vec3::new(5.0, 0.0, 5.0), Vec3::new(10.0, 2.0, 0.0)];
        let spline = Spline::new(points.clone(), false);
        for (i, p) in points.iter().enumerate().take(2) {
            assert!(spline.segment_point(i, 0.0).distance(*p) < 1e-4);
        }
        assert!(spline.segment_point(1, 1.0).distance(points[2]) < 1e-4);
        assert!((straight().length() - 10.0).abs() < 1e-3);
        let (mid, dir) = straight().at_distance(5.0).unwrap();
        assert!(mid.distance(Vec3::new(5.0, 0.0, 0.0)) < 1e-3);
        assert!(dir.distance(Vec3::X) < 1e-3);
        // Closed: back to the start, and wraps.
        let ring = Spline::new(vec![Vec3::ZERO, Vec3::X * 4.0, Vec3::new(4.0, 0.0, 4.0)], true);
        let (end, _) = ring.at_distance(ring.length()).unwrap();
        assert!(end.distance(Vec3::ZERO) < 1e-3);
    }

    #[test]
    fn meshes_and_instances_along_it() {
        let road = ribbon(&straight(), 4.0).unwrap();
        let (min_z, max_z) = road
            .vertices
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v.position[2]), b.max(v.position[2])));
        assert!((max_z - min_z - 4.0).abs() < 1e-3, "4 m wide");
        assert!(road.vertices.iter().all(|v| v.normal[1] > 0.99), "facing up");
        let pipe = tube(&straight(), 0.5, 8, 0.0).unwrap();
        assert!(pipe.vertices.iter().all(|v| (Vec3::new(0.0, v.position[1], v.position[2]).length() - 0.5).abs() < 1e-3));
        let cable = tube(&straight(), 0.05, 6, 1.0).unwrap();
        let lowest = cable.vertices.iter().map(|v| v.position[1]).fold(f32::MAX, f32::min);
        assert!(lowest < -0.9, "hangs down in the middle");
        // A fence post every 2.5 m, both ends included, facing along +X.
        let posts = instance_places(&straight(), 2.5, true);
        assert_eq!(posts.len(), 5);
        assert!(posts[4].0.distance(Vec3::new(10.0, 0.0, 0.0)) < 1e-3);
        assert!((posts[0].1 * Vec3::Z).distance(Vec3::X) < 1e-3);
        assert!(ribbon(&Spline::new(vec![Vec3::ZERO], false), 1.0).is_none());
    }

    #[test]
    fn points_are_inserted_where_the_curve_is_nearest() {
        let spline = Spline::new(vec![Vec3::ZERO, Vec3::X * 10.0, Vec3::new(10.0, 0.0, 10.0)], false);
        let (segment, t) = spline.nearest_segment(Vec3::new(10.5, 0.0, 6.0)).unwrap();
        assert_eq!(segment, 1);
        assert!(t > 0.4 && t < 0.8, "{t}");
    }
}
