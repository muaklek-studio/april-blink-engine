//! CPU-side mesh data and built-in primitives.

use ab_core::{Affine3A, Vec2, Vec3};
use bytemuck::{Pod, Zeroable};
use std::f32::consts::{PI, TAU};

/// Vertex layout shared by every mesh (48 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Tangent (xyz) and bitangent sign (w) for normal mapping; see [`Mesh::compute_tangents`].
    pub tangent: [f32; 4],
}

impl Vertex {
    /// Vertex with a zero tangent (fill it with [`Mesh::compute_tangents`]).
    pub fn new(position: Vec3, normal: Vec3, uv: Vec2) -> Self {
        Self {
            position: position.into(),
            normal: normal.into(),
            uv: uv.into(),
            tangent: [0.0; 4],
        }
    }
}

/// Per-vertex skinning data: up to four joints (indices into the skin's joint list) and their
/// weights, which should sum to 1.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct SkinWeights {
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

impl SkinWeights {
    /// Fully bound to a single joint.
    pub fn single(joint: u16) -> Self {
        Self {
            joints: [joint, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        }
    }
}

/// Axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Option<Self> {
        let mut points = points.into_iter();
        let first = points.next()?;
        Some(points.fold(
            Self {
                min: first,
                max: first,
            },
            |aabb, p| Self {
                min: aabb.min.min(p),
                max: aabb.max.max(p),
            },
        ))
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    /// The smallest box holding both.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    pub fn contains_point(&self, point: Vec3) -> bool {
        point.cmpge(self.min).all() && point.cmple(self.max).all()
    }

    /// Whether the boxes overlap (touching counts).
    pub fn intersects(&self, other: &Self) -> bool {
        self.min.cmple(other.max).all() && other.min.cmple(self.max).all()
    }

    /// The eight corners: bit 0 of the index picks x (min, max), bit 1 y, bit 2 z.
    pub fn corners(&self) -> [Vec3; 8] {
        std::array::from_fn(|i| {
            Vec3::new(
                if i & 1 == 0 { self.min.x } else { self.max.x },
                if i & 2 == 0 { self.min.y } else { self.max.y },
                if i & 4 == 0 { self.min.z } else { self.max.z },
            )
        })
    }

    pub fn half_extents(&self) -> Vec3 {
        (self.max - self.min) * 0.5
    }

    /// Bounds of this box after an affine transform (conservative, exact for the corners).
    pub fn transformed(&self, transform: &Affine3A) -> Self {
        let center = transform.transform_point3(self.center());
        let m = transform.matrix3;
        let h = self.half_extents();
        let extent = Vec3::new(
            m.x_axis.x.abs() * h.x + m.y_axis.x.abs() * h.y + m.z_axis.x.abs() * h.z,
            m.x_axis.y.abs() * h.x + m.y_axis.y.abs() * h.y + m.z_axis.y.abs() * h.z,
            m.x_axis.z.abs() * h.x + m.y_axis.z.abs() * h.y + m.z_axis.z.abs() * h.z,
        );
        Self {
            min: center - extent,
            max: center + extent,
        }
    }
}

/// Indexed triangle list (counter-clockwise front faces), optionally skinned.
#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// One entry per vertex for skinned meshes.
    pub skin: Option<Vec<SkinWeights>>,
}

impl Mesh {
    pub fn new(vertices: Vec<Vertex>, indices: Vec<u32>) -> Self {
        Self {
            vertices,
            indices,
            skin: None,
        }
    }

    #[must_use]
    pub fn with_skin(mut self, skin: Vec<SkinWeights>) -> Self {
        self.skin = Some(skin);
        self
    }

    pub fn is_skinned(&self) -> bool {
        self.skin.is_some()
    }

    pub fn aabb(&self) -> Option<Aabb> {
        Aabb::from_points(self.vertices.iter().map(|v| Vec3::from(v.position)))
    }

    /// Checks that every index refers to a vertex, the list is made of whole triangles and the
    /// skin (if any) covers every vertex.
    pub fn validate(&self) -> Result<(), MeshError> {
        if self.vertices.is_empty() || self.indices.is_empty() {
            return Err(MeshError::Empty);
        }
        if !self.indices.len().is_multiple_of(3) {
            return Err(MeshError::PartialTriangle(self.indices.len()));
        }
        let count = self.vertices.len() as u32;
        if let Some(&bad) = self.indices.iter().find(|&&i| i >= count) {
            return Err(MeshError::IndexOutOfRange {
                index: bad,
                vertex_count: count,
            });
        }
        if let Some(skin) = &self.skin {
            if skin.len() != self.vertices.len() {
                return Err(MeshError::SkinLength {
                    skin: skin.len(),
                    vertices: self.vertices.len(),
                });
            }
        }
        Ok(())
    }

    /// Recomputes smooth, area-weighted per-vertex normals from the triangles.
    pub fn compute_normals(&mut self) {
        let mut normals = vec![Vec3::ZERO; self.vertices.len()];
        for &[i0, i1, i2] in self.indices.as_chunks::<3>().0 {
            let [i0, i1, i2] = [i0 as usize, i1 as usize, i2 as usize];
            if i0.max(i1).max(i2) >= self.vertices.len() {
                continue;
            }
            let p = |i: usize| Vec3::from(self.vertices[i].position);
            let face = (p(i1) - p(i0)).cross(p(i2) - p(i0));
            for i in [i0, i1, i2] {
                normals[i] += face;
            }
        }
        for (vertex, n) in self.vertices.iter_mut().zip(normals) {
            vertex.normal = n.normalize_or(Vec3::Y).into();
        }
    }

    /// Computes per-vertex tangents from positions, normals and UVs (Lengyel's method,
    /// Gram-Schmidt orthogonalised). Vertices without usable UVs get an arbitrary tangent
    /// perpendicular to the normal.
    pub fn compute_tangents(&mut self) {
        let mut tangents = vec![Vec3::ZERO; self.vertices.len()];
        let mut bitangents = vec![Vec3::ZERO; self.vertices.len()];
        for &[i0, i1, i2] in self.indices.as_chunks::<3>().0 {
            let [i0, i1, i2] = [i0 as usize, i1 as usize, i2 as usize];
            let (Some(v0), Some(v1), Some(v2)) = (
                self.vertices.get(i0),
                self.vertices.get(i1),
                self.vertices.get(i2),
            ) else {
                continue;
            };
            let e1 = Vec3::from(v1.position) - Vec3::from(v0.position);
            let e2 = Vec3::from(v2.position) - Vec3::from(v0.position);
            let d1 = Vec2::from(v1.uv) - Vec2::from(v0.uv);
            let d2 = Vec2::from(v2.uv) - Vec2::from(v0.uv);
            let det = d1.x * d2.y - d2.x * d1.y;
            if det.abs() < 1e-12 {
                continue;
            }
            let r = 1.0 / det;
            let t = (e1 * d2.y - e2 * d1.y) * r;
            let b = (e2 * d1.x - e1 * d2.x) * r;
            for i in [i0, i1, i2] {
                tangents[i] += t;
                bitangents[i] += b;
            }
        }
        for (vertex, (t, b)) in self
            .vertices
            .iter_mut()
            .zip(tangents.into_iter().zip(bitangents))
        {
            let n = Vec3::from(vertex.normal).normalize_or(Vec3::Y);
            let ortho = (t - n * n.dot(t))
                .try_normalize()
                .unwrap_or_else(|| n.any_orthonormal_vector());
            let sign = if n.cross(ortho).dot(b) < 0.0 {
                -1.0
            } else {
                1.0
            };
            vertex.tangent = ortho.extend(sign).into();
        }
    }

    fn with_tangents(mut self) -> Self {
        self.compute_tangents();
        self
    }

    /// Axis-aligned box centred on the origin.
    pub fn cuboid(size: Vec3) -> Self {
        let h = size * 0.5;
        // (normal, tangent u, tangent v) per face; u × v = normal keeps CCW winding.
        let faces = [
            (Vec3::X, Vec3::NEG_Z, Vec3::Y),
            (Vec3::NEG_X, Vec3::Z, Vec3::Y),
            (Vec3::Y, Vec3::X, Vec3::NEG_Z),
            (Vec3::NEG_Y, Vec3::X, Vec3::Z),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
        ];
        let mut vertices = Vec::with_capacity(24);
        let mut indices = Vec::with_capacity(36);
        for (normal, u, v) in faces {
            let base = vertices.len() as u32;
            for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let position = (normal + u * su + v * sv) * h;
                let uv = Vec2::new((su + 1.0) * 0.5, (1.0 - sv) * 0.5);
                vertices.push(Vertex::new(position, normal, uv));
            }
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        Self::new(vertices, indices).with_tangents()
    }

    pub fn cube(size: f32) -> Self {
        Self::cuboid(Vec3::splat(size))
    }

    /// Flat plane on XZ facing +Y, centred on the origin, split into `subdivisions`² quads
    /// (more vertices give smoother per-vertex fog and lighting on large floors).
    pub fn plane(size: f32, subdivisions: u32) -> Self {
        let n = subdivisions.max(1);
        let mut vertices = Vec::with_capacity(((n + 1) * (n + 1)) as usize);
        for z in 0..=n {
            for x in 0..=n {
                let (fx, fz) = (x as f32 / n as f32, z as f32 / n as f32);
                let position = Vec3::new((fx - 0.5) * size, 0.0, (fz - 0.5) * size);
                vertices.push(Vertex::new(position, Vec3::Y, Vec2::new(fx, fz)));
            }
        }
        let mut indices = Vec::with_capacity((n * n * 6) as usize);
        let row = n + 1;
        for z in 0..n {
            for x in 0..n {
                let i = z * row + x;
                indices.extend([i, i + row, i + 1, i + 1, i + row, i + row + 1]);
            }
        }
        Self::new(vertices, indices).with_tangents()
    }

    /// UV sphere centred on the origin.
    pub fn sphere(radius: f32, segments: u32, rings: u32) -> Self {
        let segments = segments.max(3);
        let rings = rings.max(2);
        let mut vertices = Vec::with_capacity(((segments + 1) * (rings + 1)) as usize);
        for r in 0..=rings {
            let v = r as f32 / rings as f32;
            let phi = v * PI;
            for s in 0..=segments {
                let u = s as f32 / segments as f32;
                let theta = u * TAU;
                let normal =
                    Vec3::new(phi.sin() * theta.cos(), phi.cos(), -phi.sin() * theta.sin());
                vertices.push(Vertex::new(normal * radius, normal, Vec2::new(u, v)));
            }
        }
        let mut indices = Vec::with_capacity((segments * rings * 6) as usize);
        let row = segments + 1;
        for r in 0..rings {
            for s in 0..segments {
                let i = r * row + s;
                indices.extend([i, i + row, i + 1, i + 1, i + row, i + row + 1]);
            }
        }
        Self::new(vertices, indices).with_tangents()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MeshError {
    #[error("mesh has no vertices or no indices")]
    Empty,
    #[error("index count {0} is not a multiple of 3")]
    PartialTriangle(usize),
    #[error("index {index} is out of range for {vertex_count} vertices")]
    IndexOutOfRange { index: u32, vertex_count: u32 },
    #[error("skin has {skin} entries but the mesh has {vertices} vertices")]
    SkinLength { skin: usize, vertices: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_core::Quat;

    /// Every triangle's winding must agree with its vertex normals (front faces are CCW).
    fn assert_outward_ccw(mesh: &Mesh) {
        for tri in mesh.indices.chunks(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| mesh.vertices[i as usize]);
            let face = (Vec3::from(b.position) - Vec3::from(a.position))
                .cross(Vec3::from(c.position) - Vec3::from(a.position));
            if face.length_squared() < 1e-12 {
                continue; // degenerate triangles at sphere poles
            }
            let normal = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
            assert!(
                face.dot(normal) > 0.0,
                "triangle {tri:?} winds against its normals"
            );
        }
    }

    #[test]
    fn cube_is_valid_and_outward() {
        let cube = Mesh::cube(2.0);
        cube.validate().unwrap();
        assert_eq!((cube.vertices.len(), cube.indices.len()), (24, 36));
        assert_outward_ccw(&cube);
        let aabb = cube.aabb().unwrap();
        assert_eq!((aabb.min, aabb.max), (Vec3::splat(-1.0), Vec3::splat(1.0)));
    }

    #[test]
    fn plane_and_sphere_are_valid_and_outward() {
        let plane = Mesh::plane(10.0, 4);
        plane.validate().unwrap();
        assert_outward_ccw(&plane);
        let sphere = Mesh::sphere(1.0, 16, 8);
        sphere.validate().unwrap();
        assert_outward_ccw(&sphere);
        for v in &sphere.vertices {
            assert!((Vec3::from(v.normal).length() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn primitives_have_orthonormal_tangents() {
        for mesh in [
            Mesh::cube(1.0),
            Mesh::plane(4.0, 2),
            Mesh::sphere(1.0, 12, 6),
        ] {
            for v in &mesh.vertices {
                let t = Vec3::from_slice(&v.tangent[..3]);
                let n = Vec3::from(v.normal);
                assert!((t.length() - 1.0).abs() < 1e-4, "{v:?}");
                assert!(t.dot(n).abs() < 1e-4, "{v:?}");
                assert_eq!(v.tangent[3].abs(), 1.0);
            }
        }
        // The plane's U runs along +X, so its tangent does too.
        let plane = Mesh::plane(2.0, 1);
        assert!(Vec3::from_slice(&plane.vertices[0].tangent[..3]).abs_diff_eq(Vec3::X, 1e-5));
    }

    #[test]
    fn compute_normals_recovers_flat_normals() {
        let mut plane = Mesh::plane(2.0, 2);
        for v in &mut plane.vertices {
            v.normal = [0.0; 3];
        }
        plane.compute_normals();
        assert!(plane
            .vertices
            .iter()
            .all(|v| Vec3::from(v.normal).abs_diff_eq(Vec3::Y, 1e-5)));
    }

    #[test]
    fn validate_catches_bad_indices_and_skins() {
        let mut mesh = Mesh::cube(1.0);
        mesh.indices.push(0);
        assert_eq!(mesh.validate(), Err(MeshError::PartialTriangle(37)));
        mesh.indices.extend([0, 99]);
        assert!(matches!(
            mesh.validate(),
            Err(MeshError::IndexOutOfRange { index: 99, .. })
        ));
        let skinned = Mesh::cube(1.0).with_skin(vec![SkinWeights::single(0); 3]);
        assert_eq!(
            skinned.validate(),
            Err(MeshError::SkinLength {
                skin: 3,
                vertices: 24
            })
        );
    }

    #[test]
    fn aabb_set_operations() {
        let a = Aabb {
            min: Vec3::ZERO,
            max: Vec3::ONE,
        };
        let b = Aabb {
            min: Vec3::splat(0.5),
            max: Vec3::splat(2.0),
        };
        let far = Aabb {
            min: Vec3::splat(3.0),
            max: Vec3::splat(4.0),
        };
        assert_eq!(a.union(&b).min, Vec3::ZERO);
        assert_eq!(a.union(&b).max, Vec3::splat(2.0));
        assert!(a.intersects(&b) && b.intersects(&a));
        assert!(!a.intersects(&far));
        assert!(a.contains_point(Vec3::splat(0.5)) && a.contains_point(Vec3::ONE));
        assert!(!a.contains_point(Vec3::new(0.5, 1.5, 0.5)));
        assert_eq!(a.size(), Vec3::ONE);
        let corners = a.corners();
        assert_eq!(corners[0], Vec3::ZERO);
        assert_eq!(corners[7], Vec3::ONE);
        assert_eq!(corners[5], Vec3::new(1.0, 0.0, 1.0));
    }

    #[test]
    fn transformed_aabb_contains_transformed_corners() {
        let aabb = Aabb {
            min: Vec3::new(-1.0, -2.0, -0.5),
            max: Vec3::new(1.0, 2.0, 0.5),
        };
        let t = Affine3A::from_scale_rotation_translation(
            Vec3::new(2.0, 1.0, 1.0),
            Quat::from_rotation_y(0.7),
            Vec3::new(5.0, 0.0, 0.0),
        );
        let out = aabb.transformed(&t);
        for x in [aabb.min.x, aabb.max.x] {
            for y in [aabb.min.y, aabb.max.y] {
                for z in [aabb.min.z, aabb.max.z] {
                    let p = t.transform_point3(Vec3::new(x, y, z));
                    assert!(p.cmpge(out.min - 1e-5).all() && p.cmple(out.max + 1e-5).all());
                }
            }
        }
    }
}
