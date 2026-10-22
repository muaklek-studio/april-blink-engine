use ab_core::Vec3;
use rapier3d::prelude::SharedShape;
use std::sync::Arc;

/// Collision geometry of a [`CollisionShape3D`](crate::CollisionShape3D), in the node's local
/// space (Godot's `Shape3D` resources). Capsules and cylinders stand along the local Y axis.
///
/// The node's global scale is baked in when the collider is built: exact for boxes, hulls
/// and meshes; spheres use the largest axis, capsules and cylinders the largest horizontal
/// axis for the radius and Y for the height.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// Box with full edge lengths `size`.
    Box {
        size: Vec3,
    },
    Sphere {
        radius: f32,
    },
    /// `height` includes both hemispherical caps, as in Godot.
    Capsule {
        radius: f32,
        height: f32,
    },
    Cylinder {
        radius: f32,
        height: f32,
    },
    /// Convex hull of a point cloud.
    ConvexHull {
        points: Arc<[Vec3]>,
    },
    /// Triangle mesh. Meant for static level geometry: it has no volume, so it is a poor fit
    /// for rigid bodies.
    TriMesh {
        vertices: Arc<[Vec3]>,
        indices: Arc<[[u32; 3]]>,
    },
}

impl Shape {
    pub fn cuboid(size: Vec3) -> Self {
        Self::Box { size }
    }

    pub fn sphere(radius: f32) -> Self {
        Self::Sphere { radius }
    }

    pub fn capsule(radius: f32, height: f32) -> Self {
        Self::Capsule { radius, height }
    }

    pub fn cylinder(radius: f32, height: f32) -> Self {
        Self::Cylinder { radius, height }
    }

    pub fn convex_hull(points: impl Into<Arc<[Vec3]>>) -> Self {
        Self::ConvexHull {
            points: points.into(),
        }
    }

    pub fn trimesh(vertices: impl Into<Arc<[Vec3]>>, indices: impl Into<Arc<[[u32; 3]]>>) -> Self {
        Self::TriMesh {
            vertices: vertices.into(),
            indices: indices.into(),
        }
    }

    /// Builds the Rapier shape with `scale` applied, or `None` if the geometry is degenerate
    /// (e.g. a hull of coplanar points, or an index out of range).
    pub(crate) fn build(&self, scale: Vec3) -> Option<SharedShape> {
        let scale = scale.abs();
        let horizontal = scale.x.max(scale.z);
        match self {
            Self::Box { size } => {
                let half = (*size * scale * 0.5).max(Vec3::splat(MIN_EXTENT));
                Some(SharedShape::cuboid(half.x, half.y, half.z))
            }
            Self::Sphere { radius } => Some(SharedShape::ball(
                (radius * scale.max_element()).max(MIN_EXTENT),
            )),
            Self::Capsule { radius, height } => {
                let radius = (radius * horizontal).max(MIN_EXTENT);
                let half_segment = (height * scale.y * 0.5 - radius).max(0.0);
                Some(SharedShape::capsule_y(half_segment, radius))
            }
            Self::Cylinder { radius, height } => Some(SharedShape::cylinder(
                (height * scale.y * 0.5).max(MIN_EXTENT),
                (radius * horizontal).max(MIN_EXTENT),
            )),
            Self::ConvexHull { points } => {
                let points: Vec<Vec3> = points.iter().map(|&p| p * scale).collect();
                // Parry panics on too few points and builds unusable flat hulls.
                if !spans_volume(&points) {
                    return None;
                }
                SharedShape::convex_hull(&points)
            }
            Self::TriMesh { vertices, indices } => {
                let count = vertices.len() as u64;
                if indices.is_empty()
                    || indices.iter().flatten().any(|&i| u64::from(i) >= count)
                    || vertices.iter().any(|v| !v.is_finite())
                {
                    return None;
                }
                let vertices = vertices.iter().map(|&p| p * scale).collect();
                SharedShape::trimesh(vertices, indices.to_vec()).ok()
            }
        }
    }
}

/// Smallest half-extent a primitive is built with; zero-sized shapes break the solver.
const MIN_EXTENT: f32 = 1e-4;

/// Whether the points are finite and not all on one plane (they enclose some volume).
fn spans_volume(points: &[Vec3]) -> bool {
    let Some(&first) = points.first() else {
        return false;
    };
    if points.iter().any(|p| !p.is_finite()) {
        return false;
    }
    // Grow a tetrahedron from extreme points: farthest point, farthest from that line,
    // farthest from that plane.
    let farthest = |score: &dyn Fn(Vec3) -> f32| {
        points
            .iter()
            .copied()
            .max_by(|a, b| score(*a).total_cmp(&score(*b)))
            .unwrap_or(first)
    };
    let a = farthest(&|p| p.distance_squared(first));
    let b = farthest(&|p| p.distance_squared(a));
    let ab = b - a;
    let c = farthest(&|p| ab.cross(p - a).length_squared());
    let normal = ab.cross(c - a);
    let d = farthest(&|p| normal.dot(p - a).abs());
    let volume = normal.dot(d - a).abs() / 6.0;
    volume > MIN_EXTENT * MIN_EXTENT * MIN_EXTENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_apply_scale() {
        let cube = Shape::cuboid(Vec3::new(2.0, 1.0, 4.0))
            .build(Vec3::new(1.0, 2.0, 0.5))
            .unwrap();
        let half = cube.as_cuboid().unwrap().half_extents;
        assert_eq!((half.x, half.y, half.z), (1.0, 1.0, 1.0));

        let ball = Shape::sphere(0.5).build(Vec3::new(1.0, 3.0, 1.0)).unwrap();
        assert_eq!(ball.as_ball().unwrap().radius, 1.5);

        // 2 m tall capsule of radius 0.5: the inner segment is 1 m long.
        let capsule = Shape::capsule(0.5, 2.0).build(Vec3::ONE).unwrap();
        let capsule = capsule.as_capsule().unwrap();
        assert_eq!(capsule.radius, 0.5);
        assert!((capsule.half_height() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn degenerate_geometry_is_rejected() {
        assert!(Shape::convex_hull(Vec::new()).build(Vec3::ONE).is_none());
        let flat = Shape::convex_hull(vec![Vec3::ZERO, Vec3::X, Vec3::Z, Vec3::new(1.0, 0.0, 1.0)]);
        assert!(flat.build(Vec3::ONE).is_none());
        let line = Shape::convex_hull(vec![Vec3::ZERO, Vec3::X, Vec3::X * 2.0, Vec3::X * 3.0]);
        assert!(line.build(Vec3::ONE).is_none());
        let bad_index = Shape::trimesh(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 3]]);
        assert!(bad_index.build(Vec3::ONE).is_none());
        let tetra = Shape::convex_hull(vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z]);
        assert!(tetra.build(Vec3::ONE).is_some());
    }
}
