//! Levels of detail.
//!
//! * [`MeshLod`] on a mesh instance: simpler meshes for when it is small on screen, and a
//!   size below which it is not drawn at all. Sizes are fractions of the screen's height
//!   that the mesh's bounding sphere covers (1 = as tall as the screen), so they hold for
//!   any resolution and field of view.
//! * [`HlodGroup`] on a node: far away, its meshes are replaced by a few merged, simpler
//!   ones ([`HlodProxy`] children): one draw per material instead of one per object.
//!
//! [`simplify`] makes the simpler meshes (vertex clustering: nearby vertices with the same
//! facing merge), [`merge`] the merged ones. The renderer picks the level of every mesh
//! each frame ([`crate::FrameStats`] counts them).

use crate::assets::MeshHandle;
use crate::camera::CameraView;
use crate::mesh::{Aabb, Mesh, SkinWeights, Vertex};
use ab_core::{Affine3A, Vec3};
use std::collections::HashMap;

/// A simpler mesh for below a screen size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LodLevel {
    /// Used below this screen size (fraction of the screen's height).
    pub screen_size: f32,
    pub mesh: MeshHandle,
}

/// A mesh instance's levels of detail (see the module docs).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MeshLod {
    /// Largest screen size first.
    pub levels: Vec<LodLevel>,
    /// Not drawn below this screen size (0: always drawn).
    pub cull_below: f32,
}

impl MeshLod {
    /// Sorts the levels, largest screen size first.
    pub fn sorted(mut self) -> Self {
        self.levels
            .sort_by(|a, b| b.screen_size.total_cmp(&a.screen_size));
        self
    }

    /// The level for `screen_size`: 0 is the mesh itself, `i + 1` is `levels[i]`; `None`
    /// when culled.
    pub fn level(&self, screen_size: f32) -> Option<usize> {
        if screen_size < self.cull_below {
            return None;
        }
        Some(
            self.levels
                .iter()
                .take_while(|l| screen_size < l.screen_size)
                .count(),
        )
    }
}

/// Far from the camera, a node's meshes give way to its [`HlodProxy`] children.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HlodGroup {
    /// Metres from the camera to the group's centre where the proxies take over.
    pub distance: f32,
    /// The group's centre (world space), set when the proxies are built.
    pub center: Vec3,
}

/// A merged mesh standing in for its [`HlodGroup`]'s meshes far away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HlodProxy;

/// The most levels a draw keeps (beyond its own mesh).
pub const MAX_LOD_LEVELS: usize = 4;

/// A draw's levels, copied for the frame (no allocation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LodPick {
    pub levels: [Option<LodLevel>; MAX_LOD_LEVELS],
    pub cull_below: f32,
}

impl LodPick {
    pub fn of(lod: &MeshLod) -> Self {
        let mut levels = [None; MAX_LOD_LEVELS];
        for (slot, level) in levels.iter_mut().zip(&lod.levels) {
            *slot = Some(*level);
        }
        Self {
            levels,
            cull_below: lod.cull_below,
        }
    }

    /// The mesh and level for `screen_size` (`base` at level 0); `None` when culled.
    pub fn pick(&self, base: MeshHandle, screen_size: f32) -> Option<(MeshHandle, u8)> {
        if screen_size < self.cull_below {
            return None;
        }
        let mut chosen = (base, 0);
        for (i, level) in self.levels.iter().flatten().enumerate() {
            if screen_size < level.screen_size {
                chosen = (level.mesh, i as u8 + 1);
            }
        }
        Some(chosen)
    }
}

/// A draw inside an [`HlodGroup`]: drawn near (its own meshes) or far (the proxies).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HlodSwitch {
    pub center: Vec3,
    pub distance: f32,
    /// One of the group's proxies (drawn far away only).
    pub proxy: bool,
}

/// How much of the screen's height `bounds` covers seen from `view`.
pub fn screen_size(bounds: &Aabb, view: &CameraView) -> f32 {
    let diameter = bounds.size().length();
    if let Some(height) = view.ortho_height {
        return diameter / height.max(1e-4);
    }
    let distance = bounds.center().distance(view.position).max(1e-3);
    diameter / (2.0 * distance * (view.fov_y_radians * 0.5).tan().max(1e-4))
}

/// `mesh` with about `ratio` (0..1) of its triangles: vertices in the same cell of a grid
/// that face the same way merge into one. The grid is searched for the size that comes
/// closest to the ratio without going over. Small meshes may not reach it.
pub fn simplify(mesh: &Mesh, ratio: f32) -> Mesh {
    let triangles = mesh.indices.len() / 3;
    let target = ((triangles as f32) * ratio.clamp(0.0, 1.0)).round() as usize;
    if ratio >= 1.0 || triangles < 8 {
        return mesh.clone();
    }
    let Some(bounds) = mesh.aabb() else {
        return mesh.clone();
    };
    // Cells along the longest side: fewer cells, fewer triangles.
    let (mut low, mut high) = (1u32, 1024u32);
    let mut best: Option<Mesh> = None;
    while low <= high {
        let cells = (low + high) / 2;
        let candidate = cluster(mesh, &bounds, cells);
        if candidate.indices.len() / 3 <= target.max(1) {
            best = Some(candidate);
            low = cells + 1;
        } else {
            high = cells - 1;
        }
    }
    best.unwrap_or_else(|| cluster(mesh, &bounds, 1))
}

/// Which of the six axis directions a normal faces most.
fn facing(normal: [f32; 3]) -> u8 {
    let [x, y, z] = normal;
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    if ax >= ay && ax >= az {
        u8::from(x < 0.0)
    } else if ay >= az {
        2 + u8::from(y < 0.0)
    } else {
        4 + u8::from(z < 0.0)
    }
}

/// Sums of one cluster's vertices: position, normal, uv, count; the first vertex's skin.
type ClusterSum = (Vec3, Vec3, [f32; 2], f32, Option<SkinWeights>);

fn cluster(mesh: &Mesh, bounds: &Aabb, cells: u32) -> Mesh {
    let size = bounds.size();
    let longest = size.max_element().max(1e-6);
    let cell = longest / cells as f32;
    let mut index_of: HashMap<(i32, i32, i32, u8), u32> = HashMap::new();
    let mut sums: Vec<ClusterSum> = Vec::new();
    let mut remap = Vec::with_capacity(mesh.vertices.len());
    for (i, v) in mesh.vertices.iter().enumerate() {
        let p = (Vec3::from(v.position) - bounds.min) / cell;
        let key = (
            p.x.floor() as i32,
            p.y.floor() as i32,
            p.z.floor() as i32,
            facing(v.normal),
        );
        let next = sums.len() as u32;
        let at = *index_of.entry(key).or_insert(next);
        if at == next {
            sums.push((
                Vec3::ZERO,
                Vec3::ZERO,
                [0.0; 2],
                0.0,
                mesh.skin.as_ref().map(|s| s[i]),
            ));
        }
        let s = &mut sums[at as usize];
        s.0 += Vec3::from(v.position);
        s.1 += Vec3::from(v.normal);
        s.2[0] += v.uv[0];
        s.2[1] += v.uv[1];
        s.3 += 1.0;
        remap.push(at);
    }
    let vertices: Vec<Vertex> = sums
        .iter()
        .map(|(p, n, uv, count, _)| Vertex {
            position: (*p / *count).to_array(),
            normal: n.normalize_or(Vec3::Y).to_array(),
            uv: [uv[0] / count, uv[1] / count],
            tangent: [0.0; 4],
        })
        .collect();
    let mut indices = Vec::with_capacity(mesh.indices.len());
    for t in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| remap[i as usize]);
        if a != b && b != c && a != c {
            indices.extend([a, b, c]);
        }
    }
    let mut out = Mesh::new(vertices, indices);
    if mesh.skin.is_some() {
        out.skin = Some(
            sums.iter()
                .map(|s| s.4.unwrap_or_else(|| SkinWeights::single(0)))
                .collect(),
        );
    }
    if mesh.vertices.iter().any(|v| v.tangent != [0.0; 4]) {
        out.compute_tangents();
    }
    out
}

/// Meshes placed by their transforms, made into one (skins dropped).
pub fn merge(parts: &[(&Mesh, Affine3A)]) -> Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for (mesh, transform) in parts {
        let base = vertices.len() as u32;
        let normals = transform.matrix3.inverse().transpose();
        for v in &mesh.vertices {
            let tangent = transform
                .transform_vector3(Vec3::new(v.tangent[0], v.tangent[1], v.tangent[2]))
                .normalize_or_zero();
            vertices.push(Vertex {
                position: transform
                    .transform_point3(Vec3::from(v.position))
                    .to_array(),
                normal: (normals * Vec3::from(v.normal))
                    .normalize_or(Vec3::Y)
                    .to_array(),
                uv: v.uv,
                tangent: [tangent.x, tangent.y, tangent.z, v.tangent[3]],
            });
        }
        indices.extend(mesh.indices.iter().map(|i| i + base));
    }
    Mesh::new(vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplifying_keeps_the_shape_with_fewer_triangles() {
        let sphere = Mesh::sphere(1.0, 48, 24);
        let before = sphere.indices.len() / 3;
        let half = simplify(&sphere, 0.5);
        let tenth = simplify(&sphere, 0.1);
        let (h, t) = (half.indices.len() / 3, tenth.indices.len() / 3);
        assert!(h <= before / 2 && h > before / 5, "{before} → {h}");
        assert!(t <= before / 10 && t > 0, "{before} → {t}");
        half.validate().unwrap();
        tenth.validate().unwrap();
        // Still about a unit sphere.
        let b = tenth.aabb().unwrap();
        assert!((b.size().x - 2.0).abs() < 0.4, "{:?}", b.size());
        assert_eq!(simplify(&sphere, 1.0).indices.len(), sphere.indices.len());
    }

    #[test]
    fn levels_are_picked_by_screen_size() {
        let lod = MeshLod {
            levels: vec![
                LodLevel {
                    screen_size: 0.1,
                    mesh: MeshHandle(2),
                },
                LodLevel {
                    screen_size: 0.4,
                    mesh: MeshHandle(1),
                },
            ],
            cull_below: 0.02,
        }
        .sorted();
        assert_eq!(lod.level(0.8), Some(0));
        assert_eq!(lod.level(0.3), Some(1));
        assert_eq!(lod.level(0.05), Some(2));
        assert_eq!(lod.level(0.01), None);
        let view = CameraView::new(&Affine3A::IDENTITY, 60f32.to_radians(), 1.0, 0.1, 100.0);
        let near = Aabb {
            min: Vec3::new(-0.5, -0.5, -2.5),
            max: Vec3::new(0.5, 0.5, -1.5),
        };
        let far = near.transformed(&Affine3A::from_translation(Vec3::new(0.0, 0.0, -20.0)));
        assert!(screen_size(&near, &view) > screen_size(&far, &view) * 5.0);
    }

    #[test]
    fn merged_meshes_keep_every_triangle_in_place() {
        let cube = Mesh::cube(1.0);
        let merged = merge(&[
            (&cube, Affine3A::IDENTITY),
            (&cube, Affine3A::from_translation(Vec3::new(5.0, 0.0, 0.0))),
        ]);
        assert_eq!(merged.indices.len(), cube.indices.len() * 2);
        merged.validate().unwrap();
        let b = merged.aabb().unwrap();
        assert!((b.max.x - 5.5).abs() < 1e-5 && (b.min.x + 0.5).abs() < 1e-5);
    }
}
