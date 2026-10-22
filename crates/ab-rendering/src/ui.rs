//! Screen-space UI draw data. A UI layer (`ab-ui`) fills the [`UiDrawList`] resource each
//! frame; the renderer draws it over the post-processed scene.

use crate::assets::TextureHandle;
use ab_core::Color;
use std::fmt;
use std::sync::Arc;

/// What a [`UiQuad`] samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiFill {
    /// Flat color, optionally with rounded corners and a border.
    Solid,
    /// A texture from [`RenderAssets`](crate::RenderAssets), tinted by the quad color.
    Image(TextureHandle),
    /// A glyph from the [`GlyphAtlasImage`] (multi-channel signed distance field).
    Glyph,
}

/// One screen-space rectangle, in physical pixels with the origin at the top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiQuad {
    /// x, y, width, height.
    pub rect: [f32; 4],
    /// Texture coordinates of the rect corners: u0, v0, u1, v1 (texels for glyphs,
    /// normalized for images).
    pub uv: [f32; 4],
    /// Linear color with straight alpha.
    pub color: Color,
    pub fill: UiFill,
    /// Solid: corner radius in pixels.
    pub corner_radius: f32,
    /// Solid: border width; glyph: outline width (pixels).
    pub stroke_width: f32,
    pub stroke_color: Color,
    /// Edge blur in pixels on top of anti-aliasing (glyph glows and soft shadows).
    pub softness: f32,
    /// Glyph: distance-field range in screen pixels at this quad's scale.
    pub screen_px_range: f32,
    /// Clip rectangle: x0, y0, x1, y1.
    pub clip: [f32; 4],
}

impl UiQuad {
    /// No clipping.
    pub const NO_CLIP: [f32; 4] = [f32::MIN, f32::MIN, f32::MAX, f32::MAX];

    /// A flat-colored rectangle.
    pub fn solid(rect: [f32; 4], color: Color) -> Self {
        Self {
            rect,
            uv: [0.0, 0.0, 1.0, 1.0],
            color,
            fill: UiFill::Solid,
            corner_radius: 0.0,
            stroke_width: 0.0,
            stroke_color: Color::TRANSPARENT,
            softness: 0.0,
            screen_px_range: 1.0,
            clip: Self::NO_CLIP,
        }
    }
}

/// Snapshot of the MSDF glyph atlas: RGB holds the multi-channel distance, A the true
/// distance; 0.5 is the glyph edge and `distance_range` texels span 0 → 1.
#[derive(Clone)]
pub struct GlyphAtlasImage {
    /// Side length of the square atlas in texels.
    pub size: u32,
    /// RGBA8, row-major, top row first.
    pub pixels: Arc<[u8]>,
    /// Changes whenever the pixels do; the renderer re-uploads on change.
    pub revision: u64,
    pub distance_range: f32,
}

impl fmt::Debug for GlyphAtlasImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlyphAtlasImage")
            .field("size", &self.size)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

/// ECS resource: the UI to draw this frame, back to front.
#[derive(Debug, Clone, Default)]
pub struct UiDrawList {
    pub quads: Vec<UiQuad>,
    pub atlas: Option<GlyphAtlasImage>,
    /// Target size in pixels the quads were laid out for.
    pub size: [u32; 2],
}

impl UiDrawList {
    pub fn clear(&mut self) {
        self.quads.clear();
    }
}
