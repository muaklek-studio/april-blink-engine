//! Occlusion culling: meshes hidden behind others are not drawn.
//!
//! Each frame the largest opaque meshes on screen (the occluders, up to a triangle budget)
//! are drawn into a small depth buffer on the CPU; then every mesh in view is tested: its
//! box's nearest point must be behind the buffer on every pixel the box covers to be culled.
//!
//! It errs on the side of drawing. An occluder only fills the pixels whose four corners it
//! covers, at the farthest of its depths there (corners and any vertex inside); triangles
//! crossing the camera plane are left out; the tested box is grown by a pixel; skinned and
//! cut-out (alpha) meshes never occlude. So an object is only culled where something is in
//! front of all of it (to within a pixel of the small buffer), never through a gap.
//!
//! It is off unless the [`OcclusionCulling`] resource turns it on. With `debug`, the depth
//! buffer is kept in [`crate::RenderList::occlusion`] for tools to show.

use crate::assets::RenderAssets;
use crate::camera::CameraView;
use crate::extract::DrawItem;
use crate::lod::screen_size;
use crate::mesh::Aabb;
use ab_core::{Mat4, Vec3, Vec4};

/// Occlusion culling settings (a resource; see the module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OcclusionCulling {
    pub enabled: bool,
    /// The depth buffer's size in pixels.
    pub width: u32,
    pub height: u32,
    /// Triangles drawn into it at most per frame.
    pub max_triangles: usize,
    /// Only meshes at least this big on screen occlude (fraction of the screen's height).
    pub min_occluder_size: f32,
    /// Keep the buffer for tools ([`crate::RenderList::occlusion`]).
    pub debug: bool,
}

impl Default for OcclusionCulling {
    fn default() -> Self {
        Self {
            enabled: false,
            width: 192,
            height: 108,
            max_triangles: 40_000,
            min_occluder_size: 0.15,
            debug: false,
        }
    }
}

/// The last frame's occlusion buffer and counts (with [`OcclusionCulling::debug`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OcclusionDebug {
    pub width: u32,
    pub height: u32,
    /// Depth (0 near .. 1 far) per pixel, rows from the top.
    pub depth: Vec<f32>,
    pub occluders: u32,
    pub triangles: u32,
    pub culled: u32,
}

/// Reused per occluder.
#[derive(Default)]
struct Scratch {
    /// Nearest depth at each pixel corner ((width + 1) × (height + 1)).
    corners: Vec<f32>,
    /// Farthest vertex depth inside each pixel.
    inside: Vec<f32>,
}

struct Buffer {
    width: usize,
    height: usize,
    depth: Vec<f32>,
}

impl Buffer {
    fn new(width: u32, height: u32) -> Self {
        let (width, height) = (
            width.clamp(16, 1024) as usize,
            height.clamp(16, 1024) as usize,
        );
        Self {
            width,
            height,
            depth: vec![1.0; width * height],
        }
    }

    /// Clip space to pixels (x right, y down) and depth.
    fn screen(&self, clip: Vec4) -> Vec3 {
        let ndc = clip.truncate() / clip.w;
        Vec3::new(
            (ndc.x * 0.5 + 0.5) * self.width as f32,
            (0.5 - ndc.y * 0.5) * self.height as f32,
            ndc.z,
        )
    }

    /// Draws one occluder's triangles (screen space): each pixel corner takes the nearest
    /// depth the mesh has there; pixels with all four corners covered are filled at the
    /// farthest of them (and of the mesh's vertices inside the pixel).
    fn mesh(&mut self, triangles: &[[Vec3; 3]], scratch: &mut Scratch) {
        let (w, h) = (self.width, self.height);
        scratch.corners.clear();
        scratch.corners.resize((w + 1) * (h + 1), f32::INFINITY);
        scratch.inside.clear();
        scratch.inside.resize(w * h, 0.0);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
        for [a, b, c] in triangles {
            let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
            if area.abs() < 1e-9 {
                continue;
            }
            let min_x = a.x.min(b.x).min(c.x).ceil().max(0.0) as usize;
            let min_y = a.y.min(b.y).min(c.y).ceil().max(0.0) as usize;
            let max_x = (a.x.max(b.x).max(c.x).floor().max(-1.0) as isize).min(w as isize);
            let max_y = (a.y.max(b.y).max(c.y).floor().max(-1.0) as isize).min(h as isize);
            if max_x < 0 || max_y < 0 {
                continue;
            }
            let (max_x, max_y) = (max_x as usize, max_y as usize);
            for cy in min_y..=max_y {
                for cx in min_x..=max_x {
                    let (x, y) = (cx as f32, cy as f32);
                    let w0 = ((b.x - x) * (c.y - y) - (b.y - y) * (c.x - x)) / area;
                    let w1 = ((c.x - x) * (a.y - y) - (c.y - y) * (a.x - x)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-6 || w1 < -1e-6 || w2 < -1e-6 {
                        continue;
                    }
                    let z = w0 * a.z + w1 * b.z + w2 * c.z;
                    let corner = &mut scratch.corners[cy * (w + 1) + cx];
                    *corner = corner.min(z);
                }
            }
            for v in [a, b, c] {
                if v.x >= 0.0 && v.y >= 0.0 && (v.x as usize) < w && (v.y as usize) < h {
                    let at = &mut scratch.inside[v.y as usize * w + v.x as usize];
                    *at = at.max(v.z);
                }
            }
            x0 = x0.min(min_x.saturating_sub(1));
            y0 = y0.min(min_y.saturating_sub(1));
            x1 = x1.max((max_x + 1).min(w));
            y1 = y1.max((max_y + 1).min(h));
        }
        for py in y0..y1 {
            for px in x0..x1 {
                let corner = |x: usize, y: usize| scratch.corners[y * (w + 1) + x];
                let far = corner(px, py)
                    .max(corner(px + 1, py))
                    .max(corner(px, py + 1))
                    .max(corner(px + 1, py + 1));
                if far.is_finite() {
                    let d = &mut self.depth[py * w + px];
                    *d = d.min(far.max(scratch.inside[py * w + px]));
                }
            }
        }
    }

    /// Whether everything of `bounds` is behind the buffer.
    fn hides(&self, bounds: &Aabb, view_projection: &Mat4) -> bool {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for corner in bounds.corners() {
            let clip = *view_projection * corner.extend(1.0);
            if clip.w <= 1e-4 {
                // Reaches behind the camera: it surrounds the view.
                return false;
            }
            let p = self.screen(clip);
            min = min.min(p);
            max = max.max(p);
        }
        let nearest = min.z.max(0.0);
        let x0 = (min.x.floor() - 1.0).max(0.0) as usize;
        let y0 = (min.y.floor() - 1.0).max(0.0) as usize;
        let x1 = ((max.x.ceil() + 1.0).max(0.0) as usize).min(self.width);
        let y1 = ((max.y.ceil() + 1.0).max(0.0) as usize).min(self.height);
        if x0 >= x1 || y0 >= y1 {
            return false;
        }
        for y in y0..y1 {
            for x in x0..x1 {
                if self.depth[y * self.width + x] >= nearest - 1e-5 {
                    return false;
                }
            }
        }
        true
    }
}

/// Culls the draws in view hidden behind the occluders (see the module docs). `bounds`:
/// each draw's world box, as culled against the frustum. Returns the buffer when asked.
pub(crate) fn cull(
    settings: &OcclusionCulling,
    view: &CameraView,
    assets: &RenderAssets,
    draws: &mut [DrawItem],
    bounds: &[Option<Aabb>],
) -> Option<OcclusionDebug> {
    let mut buffer = Buffer::new(settings.width, settings.height);
    // Occluders: the biggest on screen first.
    let mut occluders: Vec<(usize, f32)> = draws
        .iter()
        .enumerate()
        .filter(|(_, d)| d.in_view && d.joints.is_none())
        // Simplified meshes can stand out of the real surface: only full detail occludes.
        .filter(|(_, d)| d.lod_level == 0 && !d.hlod.is_some_and(|h| h.proxy))
        .filter(|(_, d)| {
            assets
                .material(d.material)
                .is_some_and(|m| m.alpha_cutoff.is_none())
        })
        .filter_map(|(i, _)| {
            let size = screen_size(bounds[i].as_ref()?, view);
            (size >= settings.min_occluder_size).then_some((i, size))
        })
        .collect();
    occluders.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut triangles = 0usize;
    let mut used = 0u32;
    let mut scratch = Scratch::default();
    for (i, _) in occluders {
        let draw = &draws[i];
        let Some(mesh) = assets.mesh(draw.mesh) else {
            continue;
        };
        let count = mesh.indices.len() / 3;
        if triangles + count > settings.max_triangles {
            continue;
        }
        triangles += count;
        used += 1;
        let to_clip = view.view_projection * Mat4::from(draw.model);
        let points: Vec<Vec4> = mesh
            .vertices
            .iter()
            .map(|v| to_clip * Vec3::from(v.position).extend(1.0))
            .collect();
        let screen: Vec<[Vec3; 3]> = mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| t.map(|i| points[i as usize]))
            // Crossing the camera plane: left out (no clipping; only less culled).
            .filter(|t| t.iter().all(|p| p.w > 1e-3))
            .map(|t| t.map(|p| buffer.screen(p)))
            .collect();
        buffer.mesh(&screen, &mut scratch);
    }
    let mut culled = 0;
    if used > 0 {
        for (draw, b) in draws.iter_mut().zip(bounds) {
            if !draw.in_view {
                continue;
            }
            if let Some(b) = b {
                if buffer.hides(b, &view.view_projection) {
                    draw.in_view = false;
                    draw.occluded = true;
                    culled += 1;
                }
            }
        }
    }
    settings.debug.then_some(OcclusionDebug {
        width: buffer.width as u32,
        height: buffer.height as u32,
        depth: buffer.depth,
        occluders: used,
        triangles: triangles as u32,
        culled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_core::Affine3A;

    fn view() -> CameraView {
        CameraView::new(
            &Affine3A::IDENTITY,
            60f32.to_radians(),
            16.0 / 9.0,
            0.1,
            100.0,
        )
    }

    /// A wall from `left` to `right`, 4 m tall, `z` metres ahead.
    fn wall(buffer: &mut Buffer, view: &CameraView, z: f32, left: f32, right: f32) {
        let p =
            |b: &Buffer, x: f32, y: f32| b.screen(view.view_projection * Vec4::new(x, y, z, 1.0));
        let (a, b, c, d) = (
            p(buffer, left, -2.0),
            p(buffer, right, -2.0),
            p(buffer, right, 2.0),
            p(buffer, left, 2.0),
        );
        buffer.mesh(&[[a, b, c], [a, c, d]], &mut Scratch::default());
    }

    #[test]
    fn boxes_behind_a_wall_are_hidden_and_others_are_not() {
        let view = view();
        let mut buffer = Buffer::new(192, 108);
        // A wall 4 m wide, 5 m away.
        wall(&mut buffer, &view, -5.0, -2.0, 2.0);
        let behind = Aabb {
            min: Vec3::new(-0.5, -0.5, -12.0),
            max: Vec3::new(0.5, 0.5, -11.0),
        };
        let front = Aabb {
            min: Vec3::new(-0.5, -0.5, -3.0),
            max: Vec3::new(0.5, 0.5, -2.0),
        };
        let beside = Aabb {
            min: Vec3::new(6.0, -0.5, -12.0),
            max: Vec3::new(7.0, 0.5, -11.0),
        };
        // Wider than the wall: it sticks out on both sides.
        let wide = Aabb {
            min: Vec3::new(-10.0, -0.5, -12.0),
            max: Vec3::new(10.0, 0.5, -11.0),
        };
        assert!(buffer.hides(&behind, &view.view_projection));
        assert!(!buffer.hides(&front, &view.view_projection));
        assert!(!buffer.hides(&beside, &view.view_projection));
        assert!(!buffer.hides(&wide, &view.view_projection));
        // Around the camera: never hidden.
        let around = Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        };
        assert!(!buffer.hides(&around, &view.view_projection));
    }

    #[test]
    fn a_gap_between_occluders_lets_things_show() {
        let view = view();
        let mut buffer = Buffer::new(192, 108);
        // Two walls with a 0.2 m gap at x = 0.
        wall(&mut buffer, &view, -5.0, -3.0, -0.1);
        wall(&mut buffer, &view, -5.0, 0.1, 3.0);
        let thin = Aabb {
            min: Vec3::new(-0.05, -0.5, -20.0),
            max: Vec3::new(0.05, 0.5, -19.0),
        };
        assert!(
            !buffer.hides(&thin, &view.view_projection),
            "seen through the gap"
        );
    }
}
