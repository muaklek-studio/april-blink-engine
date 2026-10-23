//! Fonts: loading, shaping (rustybuzz) and multi-channel signed distance field glyphs
//! baked on demand into one atlas.

use crate::UiError;
use ab_rendering::GlyphAtlasImage;
use fdsm::bezier::scanline::FillRule;
use fdsm::bezier::{Point, Segment};
use fdsm::correct_error::{correct_error_mtsdf, ErrorCorrectionConfig};
use fdsm::generate::generate_mtsdf;
use fdsm::render::correct_sign_mtsdf;
use fdsm::shape::{Contour, Shape};
use image::Rgba32FImage;
use rustybuzz::ttf_parser::{self, GlyphId, OutlineBuilder};
use rustybuzz::{Direction, UnicodeBuffer};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tracing::warn;

/// Characters that belong to the character before them (combining marks, joiners,
/// variation selectors) or fit any font (spaces): they never start a new font run.
fn attaches(c: char) -> bool {
    c.is_whitespace()
        || matches!(c,
            '\u{0300}'..='\u{036F}'
            | '\u{0E31}' | '\u{0E34}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}'
            | '\u{200C}' | '\u{200D}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{1AB0}'..='\u{1AFF}' | '\u{20D0}'..='\u{20FF}')
}

/// The built-in font (Prompt Regular, Latin + Thai, SIL Open Font License 1.1).
const DEFAULT_FONT: &[u8] = include_bytes!("../fonts/Prompt-Regular.ttf");

/// Atlas pixels per em: glyphs are baked at this size and scaled freely when drawn.
pub(crate) const BAKE_EM_PX: f32 = 42.0;
/// Width of the distance band stored around each glyph edge, in atlas pixels (±half).
/// Outlines and glows up to about `RANGE / 2` atlas pixels wide are possible.
pub(crate) const DISTANCE_RANGE: f32 = 8.0;
/// Empty texels around each glyph: the distance band plus a guard against bleeding.
const PADDING: f32 = DISTANCE_RANGE / 2.0 + 1.0;
const INITIAL_ATLAS_SIZE: u32 = 1024;
const MAX_ATLAS_SIZE: u32 = 4096;

/// Reference to a font in [`Fonts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct FontId(u32);

impl FontId {
    /// The built-in font, always present.
    pub const DEFAULT: FontId = FontId(0);
}

/// Vertical metrics of a font at some size, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMetrics {
    /// Baseline distance below the top of the line.
    pub ascent: f32,
    /// Distance from the baseline to the bottom of the line (positive).
    pub descent: f32,
    /// Distance between consecutive baselines.
    pub line_height: f32,
}

/// One positioned glyph of a shaped run, in pixels (y down), relative to the run origin on
/// the baseline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    pub glyph: u16,
    /// The font the glyph is from: the run's, or a fallback for characters it lacks.
    pub font: FontId,
    /// Byte offset of the first character of the cluster this glyph belongs to. Marks
    /// (Thai vowels and tone marks) share the cluster of their base consonant.
    pub cluster: usize,
    /// Pen position of the glyph.
    pub x: f32,
    pub y: f32,
    pub advance: f32,
}

/// Result of shaping a single line of text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShapedRun {
    pub glyphs: Vec<ShapedGlyph>,
    pub width: f32,
}

/// Where a baked glyph sits in the atlas and how to place it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GlyphEntry {
    /// Atlas texel rectangle: x, y, width, height.
    pub atlas: [u32; 4],
    /// Quad relative to the pen position, in ems (y down): left, top, right, bottom.
    pub plane: [f32; 4],
}

struct FontData {
    bytes: Box<[u8]>,
    units_per_em: f32,
    ascender: f32,
    descender: f32,
    line_gap: f32,
}

impl FontData {
    fn parse(bytes: Box<[u8]>) -> Result<Self, UiError> {
        let face = ttf_parser::Face::parse(&bytes, 0)
            .map_err(|err| UiError::InvalidFont(err.to_string()))?;
        let units_per_em = f32::from(face.units_per_em());
        let (ascender, descender, line_gap) = (
            f32::from(face.ascender()),
            f32::from(face.descender()),
            f32::from(face.line_gap()),
        );
        Ok(Self {
            bytes,
            units_per_em,
            ascender,
            descender,
            line_gap,
        })
    }

    fn face(&self) -> rustybuzz::Face<'_> {
        rustybuzz::Face::from_slice(&self.bytes, 0).expect("validated when the font was added")
    }
}

/// ECS resource: every loaded font plus the shared MSDF glyph atlas.
///
/// Glyphs are baked the first time they are drawn, at a fixed size; the distance field keeps
/// them sharp at any on-screen size.
pub struct Fonts {
    fonts: Vec<FontData>,
    /// Tried in order for characters a run's font lacks (see [`Fonts::set_fallbacks`]).
    fallbacks: Vec<FontId>,
    glyphs: HashMap<(FontId, u16), Option<GlyphEntry>>,
    packer: ShelfPacker,
    pixels: Vec<u8>,
    revision: u64,
    snapshot: Option<GlyphAtlasImage>,
}

impl Fonts {
    /// A library holding the built-in font as [`FontId::DEFAULT`].
    pub fn new() -> Self {
        let size = INITIAL_ATLAS_SIZE;
        Self {
            fonts: vec![FontData::parse(DEFAULT_FONT.into()).expect("built-in font is valid")],
            fallbacks: Vec::new(),
            glyphs: HashMap::new(),
            packer: ShelfPacker::new(size),
            pixels: vec![0; (size * size * 4) as usize],
            revision: 1,
            snapshot: None,
        }
    }

    /// Adds a TrueType/OpenType font.
    pub fn add(&mut self, bytes: impl Into<Vec<u8>>) -> Result<FontId, UiError> {
        let data = FontData::parse(bytes.into().into_boxed_slice())?;
        self.fonts.push(data);
        Ok(FontId(self.fonts.len() as u32 - 1))
    }

    /// Fonts for the characters a run's font lacks, tried in order (the built-in font comes
    /// last on its own): a CJK font for Chinese, Japanese and Korean text, say.
    pub fn set_fallbacks(&mut self, fallbacks: Vec<FontId>) {
        self.fallbacks = fallbacks
            .into_iter()
            .filter(|f| (f.0 as usize) < self.fonts.len())
            .collect();
    }

    pub fn fallbacks(&self) -> &[FontId] {
        &self.fallbacks
    }

    /// Whether `font` draws `c`.
    pub fn has_glyph(&self, font: FontId, c: char) -> bool {
        self.font(font).face().glyph_index(c).is_some()
    }

    pub fn len(&self) -> usize {
        self.fonts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }

    fn font(&self, font: FontId) -> &FontData {
        self.fonts
            .get(font.0 as usize)
            .unwrap_or(&self.fonts[FontId::DEFAULT.0 as usize])
    }

    pub fn line_metrics(&self, font: FontId, size: f32) -> LineMetrics {
        let data = self.font(font);
        let scale = size / data.units_per_em;
        let ascent = data.ascender * scale;
        let descent = -data.descender * scale;
        LineMetrics {
            ascent,
            descent,
            line_height: ascent + descent + data.line_gap * scale,
        }
    }

    /// Shapes one line of text at `size` pixels per em: applies kerning, ligatures and mark
    /// positioning (so Thai vowels and tone marks stack correctly above and below their
    /// consonant). Line breaks are the caller's job.
    ///
    /// Characters `font` lacks are drawn with the first fallback that has them
    /// ([`Fonts::set_fallbacks`], then the built-in font); the text is split into runs per
    /// font, each shaped on its own.
    pub fn shape(&self, font: FontId, text: &str, size: f32) -> ShapedRun {
        if text.is_empty() {
            return ShapedRun::default();
        }
        let font = if (font.0 as usize) < self.fonts.len() {
            font
        } else {
            FontId::DEFAULT
        };
        let mut glyphs = Vec::new();
        let mut pen = 0.0;
        for (start, end, run_font) in self.font_runs(font, text) {
            pen = self.shape_run(run_font, &text[start..end], start, size, pen, &mut glyphs);
        }
        ShapedRun { glyphs, width: pen }
    }

    /// Splits `text` into (start, end, font) runs: a character stays in the current run
    /// while its font has it (marks, spaces and joiners always stay), else goes to the
    /// first font of the chain that has it.
    fn font_runs(&self, font: FontId, text: &str) -> Vec<(usize, usize, FontId)> {
        let mut chain = vec![font];
        for &fallback in self.fallbacks.iter().chain([&FontId::DEFAULT]) {
            if !chain.contains(&fallback) {
                chain.push(fallback);
            }
        }
        if chain.len() == 1 {
            return vec![(0, text.len(), font)];
        }
        let faces: Vec<rustybuzz::Face<'_>> = chain.iter().map(|&f| self.font(f).face()).collect();
        let mut runs: Vec<(usize, usize, FontId)> = Vec::new();
        for (index, c) in text.char_indices() {
            let end = index + c.len_utf8();
            let current = runs.last().map(|&(_, _, f)| f);
            let sticks = |f: FontId| {
                let face = &faces[chain.iter().position(|&x| x == f).unwrap_or(0)];
                face.glyph_index(c).is_some() || attaches(c)
            };
            let chosen = match current {
                Some(f) if sticks(f) => f,
                _ => chain
                    .iter()
                    .zip(&faces)
                    .find(|(_, face)| face.glyph_index(c).is_some())
                    .map_or(current.unwrap_or(font), |(&f, _)| f),
            };
            match runs.last_mut() {
                Some(run) if run.2 == chosen => run.1 = end,
                _ => runs.push((index, end, chosen)),
            }
        }
        runs
    }

    /// Shapes `text` (bytes `offset..` of the line) with one font, from pen position `pen`;
    /// returns the pen after it.
    fn shape_run(
        &self,
        font: FontId,
        text: &str,
        offset: usize,
        size: f32,
        mut pen: f32,
        out: &mut Vec<ShapedGlyph>,
    ) -> f32 {
        let data = self.font(font);
        let face = data.face();
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        // UI text is laid out left to right; right-to-left scripts are out of scope.
        buffer.set_direction(Direction::LeftToRight);
        let output = rustybuzz::shape(&face, &[], buffer);
        let scale = size / data.units_per_em;
        for (info, position) in output.glyph_infos().iter().zip(output.glyph_positions()) {
            let glyph = ShapedGlyph {
                glyph: info.glyph_id as u16,
                font,
                cluster: offset + info.cluster as usize,
                x: pen + position.x_offset as f32 * scale,
                y: -position.y_offset as f32 * scale,
                advance: position.x_advance as f32 * scale,
            };
            pen += glyph.advance;
            out.push(glyph);
        }
        pen
    }

    /// Atlas placement of a glyph, baking it first if needed. `None` for glyphs without an
    /// outline (spaces) or when the atlas is full.
    pub(crate) fn glyph(&mut self, font: FontId, glyph: u16) -> Option<GlyphEntry> {
        let font = if (font.0 as usize) < self.fonts.len() {
            font
        } else {
            FontId::DEFAULT
        };
        if let Some(entry) = self.glyphs.get(&(font, glyph)) {
            return *entry;
        }
        let entry = self.bake(font, glyph);
        self.glyphs.insert((font, glyph), entry);
        entry
    }

    fn bake(&mut self, font: FontId, glyph: u16) -> Option<GlyphEntry> {
        let data = &self.fonts[font.0 as usize];
        let face = ttf_parser::Face::parse(&data.bytes, 0).ok()?;
        let baked = bake_glyph(&face, GlyphId(glyph), data.units_per_em)?;
        let [width, height] = baked.size;
        let (x, y) = loop {
            if let Some(position) = self.packer.allocate(width, height) {
                break position;
            }
            if !self.grow() {
                warn!("glyph atlas is full; glyph {glyph} not drawn");
                return None;
            }
        };
        let stride = self.packer.size as usize * 4;
        for row in 0..height as usize {
            let dst = (y as usize + row) * stride + x as usize * 4;
            let src = row * width as usize * 4;
            self.pixels[dst..dst + width as usize * 4]
                .copy_from_slice(&baked.pixels[src..src + width as usize * 4]);
        }
        self.revision += 1;
        Some(GlyphEntry {
            atlas: [x, y, width, height],
            plane: baked.plane,
        })
    }

    /// Doubles the atlas size, keeping baked glyphs where they are.
    fn grow(&mut self) -> bool {
        let old = self.packer.size;
        if old >= MAX_ATLAS_SIZE {
            return false;
        }
        let new = old * 2;
        let mut pixels = vec![0; (new * new * 4) as usize];
        for row in 0..old as usize {
            let src = row * old as usize * 4;
            pixels[row * new as usize * 4..][..old as usize * 4]
                .copy_from_slice(&self.pixels[src..src + old as usize * 4]);
        }
        self.pixels = pixels;
        self.packer.grow(new);
        self.revision += 1;
        true
    }

    /// Side length of the (square) atlas in texels.
    pub fn atlas_size(&self) -> u32 {
        self.packer.size
    }

    /// The atlas as RGBA8 (MSDF in RGB, true SDF in A), shared with the renderer. Cheap
    /// unless glyphs were baked since the last call.
    pub fn atlas_image(&mut self) -> GlyphAtlasImage {
        if self
            .snapshot
            .as_ref()
            .is_none_or(|s| s.revision != self.revision)
        {
            self.snapshot = Some(GlyphAtlasImage {
                size: self.packer.size,
                pixels: Arc::from(self.pixels.as_slice()),
                revision: self.revision,
                distance_range: DISTANCE_RANGE,
            });
        }
        self.snapshot.clone().expect("set above")
    }
}

impl Default for Fonts {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Fonts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fonts")
            .field("fonts", &self.fonts.len())
            .field("glyphs", &self.glyphs.len())
            .field("atlas_size", &self.packer.size)
            .finish()
    }
}

struct BakedGlyph {
    size: [u32; 2],
    /// RGBA8, rows top to bottom.
    pixels: Vec<u8>,
    plane: [f32; 4],
}

/// Renders one glyph's multi-channel + true signed distance field.
fn bake_glyph(
    face: &ttf_parser::Face<'_>,
    glyph: GlyphId,
    units_per_em: f32,
) -> Option<BakedGlyph> {
    let mut outline = OutlineCollector::default();
    let bbox = face.outline_glyph(glyph, &mut outline)?;
    outline.close();
    if outline.shape.contours.is_empty() {
        return None;
    }
    let scale = f64::from(BAKE_EM_PX / units_per_em);
    let pad = f64::from(PADDING);
    let (x_min, y_min) = (f64::from(bbox.x_min), f64::from(bbox.y_min));
    let width = ((f64::from(bbox.x_max) - x_min) * scale + 2.0 * pad).ceil() as u32;
    let height = ((f64::from(bbox.y_max) - y_min) * scale + 2.0 * pad).ceil() as u32;

    // Font units (y up) → texels with the bounding box's corner at (pad, pad).
    let mut shape = outline.shape;
    for contour in &mut shape.contours {
        for segment in &mut contour.segments {
            *segment = map_segment(segment, |p| {
                Point::new((p.x - x_min) * scale + pad, (p.y - y_min) * scale + pad)
            });
        }
    }
    let colored = Shape::edge_coloring_simple(shape, 0.03, 0x5eed);
    let prepared = colored.prepare();
    let range = f64::from(DISTANCE_RANGE);
    let mut field = Rgba32FImage::new(width, height);
    generate_mtsdf(&prepared, range, &mut field);
    correct_error_mtsdf(
        &mut field,
        &colored,
        &prepared,
        range,
        &ErrorCorrectionConfig::default(),
    );
    correct_sign_mtsdf(&mut field, &prepared, FillRule::Nonzero);

    // The field's row 0 is at the bottom (font y up); store rows top-down.
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in (0..height).rev() {
        for col in 0..width {
            let texel = field.get_pixel(col, row).0;
            pixels.extend(texel.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
        }
    }
    let em = |texels: f64| (texels / f64::from(BAKE_EM_PX)) as f32;
    let left = (x_min * scale - pad) / f64::from(BAKE_EM_PX);
    let top = -(f64::from(bbox.y_max) * scale + pad) / f64::from(BAKE_EM_PX);
    Some(BakedGlyph {
        size: [width, height],
        pixels,
        plane: [
            left as f32,
            top as f32,
            left as f32 + em(f64::from(width)),
            top as f32 + em(f64::from(height)),
        ],
    })
}

fn map_segment(segment: &Segment, f: impl Fn(Point) -> Point) -> Segment {
    match segment.order() {
        fdsm::bezier::Order::Linear => Segment::line(f(segment.start()), f(segment.end())),
        fdsm::bezier::Order::Quadratic => Segment::quad(
            f(segment.start()),
            f(segment.control_point(1)),
            f(segment.end()),
        ),
        fdsm::bezier::Order::Cubic => Segment::cubic(
            f(segment.start()),
            f(segment.control_point(1)),
            f(segment.control_point(2)),
            f(segment.end()),
        ),
    }
}

/// Converts a TrueType/CFF outline into fdsm contours (in font units).
#[derive(Default)]
struct OutlineCollector {
    shape: Shape<Contour>,
    contour: Contour,
    start: Option<Point>,
    pen: Point,
}

impl OutlineCollector {
    fn push(&mut self, segment: Segment) {
        self.pen = segment.end();
        self.contour.segments.push(segment);
    }

    fn close(&mut self) {
        if let Some(start) = self.start.take() {
            if (self.pen - start).norm() > 1e-9 {
                self.push(Segment::line(self.pen, start));
            }
        }
        let contour = std::mem::take(&mut self.contour);
        if !contour.segments.is_empty() {
            self.shape.contours.push(contour);
        }
    }
}

impl OutlineBuilder for OutlineCollector {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close();
        self.pen = Point::new(x.into(), y.into());
        self.start = Some(self.pen);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let to = Point::new(x.into(), y.into());
        if (to - self.pen).norm() > 1e-9 {
            self.push(Segment::line(self.pen, to));
        }
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let control = Point::new(x1.into(), y1.into());
        self.push(Segment::quad(
            self.pen,
            control,
            Point::new(x.into(), y.into()),
        ));
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.push(Segment::cubic(
            self.pen,
            Point::new(x1.into(), y1.into()),
            Point::new(x2.into(), y2.into()),
            Point::new(x.into(), y.into()),
        ));
    }

    fn close(&mut self) {
        OutlineCollector::close(self);
    }
}

/// Row-based ("shelf") rectangle packer.
struct ShelfPacker {
    size: u32,
    /// (y, height, next free x) of each shelf.
    shelves: Vec<(u32, u32, u32)>,
    next_y: u32,
}

impl ShelfPacker {
    fn new(size: u32) -> Self {
        Self {
            size,
            shelves: Vec::new(),
            next_y: 0,
        }
    }

    fn grow(&mut self, size: u32) {
        self.size = size;
    }

    fn allocate(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        const GAP: u32 = 1;
        let (w, h) = (width + GAP, height + GAP);
        if w > self.size {
            return None;
        }
        // Best-fitting shelf that is not much taller than the glyph.
        let best = self
            .shelves
            .iter_mut()
            .filter(|(_, shelf_h, x)| {
                *shelf_h >= h && *shelf_h <= h + h / 2 + 4 && x + w <= self.size
            })
            .min_by_key(|(_, shelf_h, _)| *shelf_h);
        if let Some((y, _, x)) = best {
            let position = (*x, *y);
            *x += w;
            return Some(position);
        }
        if self.next_y + h > self.size {
            return None;
        }
        let y = self.next_y;
        self.shelves.push((y, h, w));
        self.next_y += h;
        Some((0, y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelf_packer_fills_rows_then_runs_out() {
        let mut packer = ShelfPacker::new(64);
        assert_eq!(packer.allocate(30, 10), Some((0, 0)));
        assert_eq!(packer.allocate(30, 10), Some((31, 0)));
        // Row full: next shelf.
        assert_eq!(packer.allocate(30, 10), Some((0, 11)));
        assert_eq!(packer.allocate(70, 10), None);
        assert_eq!(packer.allocate(10, 60), None);
        packer.grow(128);
        assert!(packer.allocate(10, 60).is_some());
    }

    #[test]
    fn latin_shaping_applies_advances_and_kerning() {
        let fonts = Fonts::new();
        let run = fonts.shape(FontId::DEFAULT, "AV Hello", 40.0);
        assert_eq!(run.glyphs.len(), 8);
        let sum: f32 = run.glyphs.iter().map(|g| g.advance).sum();
        assert!((sum - run.width).abs() < 1e-3);
        assert!(run.width > 100.0 && run.width < 200.0, "{}", run.width);
        // Clusters are byte offsets.
        let clusters: Vec<_> = run.glyphs.iter().map(|g| g.cluster).collect();
        assert_eq!(clusters, [0, 1, 2, 3, 4, 5, 6, 7]);
        // Shaping is linear in size.
        let double = fonts.shape(FontId::DEFAULT, "AV Hello", 80.0);
        assert!((double.width - run.width * 2.0).abs() < 1e-2);
    }

    /// Vertical ink extent (top, bottom) of a shaped glyph in pixels, y down.
    fn ink(fonts: &Fonts, glyph: &ShapedGlyph, size: f32) -> (f32, f32) {
        let data = fonts.font(FontId::DEFAULT);
        let face = ttf_parser::Face::parse(&data.bytes, 0).unwrap();
        let bbox = face.glyph_bounding_box(GlyphId(glyph.glyph)).unwrap();
        let scale = size / data.units_per_em;
        (
            glyph.y - f32::from(bbox.y_max) * scale,
            glyph.y - f32::from(bbox.y_min) * scale,
        )
    }

    #[test]
    fn thai_marks_stack_on_their_consonant() {
        let fonts = Fonts::new();
        // ที่: tho thahan + sara ii (upper vowel) + mai ek (tone mark).
        let run = fonts.shape(FontId::DEFAULT, "ที่", 100.0);
        assert_eq!(run.glyphs.len(), 3);
        let [base, vowel, tone] = [run.glyphs[0], run.glyphs[1], run.glyphs[2]];
        // One cluster; marks take no horizontal space.
        assert!(run.glyphs.iter().all(|g| g.cluster == 0));
        assert_eq!(vowel.advance, 0.0);
        assert_eq!(tone.advance, 0.0);
        assert!((run.width - base.advance).abs() < 1e-3);

        // Stacked: consonant, then the vowel above it, then the tone mark above the vowel.
        let (base_top, _) = ink(&fonts, &base, 100.0);
        let (vowel_top, vowel_bottom) = ink(&fonts, &vowel, 100.0);
        let (tone_top, tone_bottom) = ink(&fonts, &tone, 100.0);
        assert!(vowel_bottom <= base_top + 2.0, "vowel above the consonant");
        assert!(tone_bottom <= vowel_top + 2.0, "tone mark above the vowel");
        assert!(tone_top < vowel_top);

        // Without an upper vowel the tone mark sits lower, right above the consonant: ท่.
        let alone = fonts.shape(FontId::DEFAULT, "ท่", 100.0);
        let (low_top, low_bottom) = ink(&fonts, &alone.glyphs[1], 100.0);
        assert!(low_bottom <= base_top + 2.0);
        assert!(
            low_top > tone_top + 10.0,
            "lowered: {low_top} vs {tone_top}"
        );
    }

    #[test]
    fn sara_am_and_clusters() {
        let fonts = Fonts::new();
        // น้ำ: the tone mark goes between nikhahit and sara aa (ccmp decomposes sara am).
        let run = fonts.shape(FontId::DEFAULT, "น้ำ", 100.0);
        assert!(run.glyphs.len() >= 3, "{run:?}");
        assert!(run.glyphs.iter().all(|g| g.cluster == 0));
        // Two words: the second cluster starts at its byte offset.
        let words = fonts.shape(FontId::DEFAULT, "ไก่ ไข่", 40.0);
        let space = "ไก่".len();
        assert!(words.glyphs.iter().any(|g| g.cluster == space));
    }

    #[test]
    fn glyphs_bake_once_into_the_atlas() {
        let mut fonts = Fonts::new();
        let run = fonts.shape(FontId::DEFAULT, "A ก", 40.0);
        let a = fonts.glyph(FontId::DEFAULT, run.glyphs[0].glyph).unwrap();
        assert!(
            fonts.glyph(FontId::DEFAULT, run.glyphs[1].glyph).is_none(),
            "space"
        );
        let ko = fonts.glyph(FontId::DEFAULT, run.glyphs[2].glyph).unwrap();
        assert_ne!(a.atlas, ko.atlas);
        let revision = fonts.atlas_image().revision;
        assert_eq!(fonts.glyph(FontId::DEFAULT, run.glyphs[0].glyph), Some(a));
        assert_eq!(
            fonts.atlas_image().revision,
            revision,
            "cached, not re-baked"
        );

        // "A": the quad spans the glyph, roughly 0.7 em tall above the baseline.
        let [left, top, right, bottom] = a.plane;
        assert!(left < 0.05 && right > 0.5, "{:?}", a.plane);
        assert!(top < -0.6 && bottom > 0.0, "{:?}", a.plane);

        // Inside the glyph the distance is positive (> 0.5), outside negative.
        let image = fonts.atlas_image();
        let texel = |x: u32, y: u32| {
            let i = ((y * image.size + x) * 4) as usize;
            [
                image.pixels[i],
                image.pixels[i + 1],
                image.pixels[i + 2],
                image.pixels[i + 3],
            ]
        };
        let [x, y, w, h] = a.atlas;
        let corner = texel(x, y);
        assert!(corner[3] < 64, "outside: {corner:?}");
        // Left stem of the "A", a quarter up from its base.
        let median = |t: [u8; 4]| {
            let [r, g, b, _] = t;
            r.max(g).min(r.min(g).max(b))
        };
        let inside = (0..w)
            .map(|dx| texel(x + dx, y + h * 3 / 4))
            .filter(|&t| median(t) > 160)
            .count();
        assert!(inside >= 2, "the stems are inside the shape");
    }

    #[test]
    fn invalid_font_is_rejected() {
        let mut fonts = Fonts::new();
        assert!(matches!(
            fonts.add(vec![0, 1, 2]),
            Err(UiError::InvalidFont(_))
        ));
        let id = fonts.add(DEFAULT_FONT.to_vec()).unwrap();
        assert_eq!(id, FontId(1));
        assert_eq!(fonts.len(), 2);
    }

    #[test]
    fn one_font_one_run_and_glyphs_name_their_font() {
        let mut fonts = Fonts::new();
        let second = fonts.add(DEFAULT_FONT.to_vec()).unwrap();
        // The second font has everything: no fallback needed, one run in it.
        fonts.set_fallbacks(vec![FontId::DEFAULT]);
        let run = fonts.shape(second, "Score คะแนน", 30.0);
        assert!(run.glyphs.iter().all(|g| g.font == second));
        assert_eq!(fonts.font_runs(second, "Score คะแนน"), [(0, 21, second)]);
        // Unknown fallbacks are dropped.
        fonts.set_fallbacks(vec![FontId(9)]);
        assert!(fonts.fallbacks().is_empty());
    }

    /// With a CJK font of the system (when there is one), Chinese inside Thai and Latin text
    /// comes from it while the rest stays in Prompt, positioned one after the other.
    #[test]
    fn missing_characters_come_from_the_fallback_font() {
        let candidates = [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "C:\\Windows\\Fonts\\msyh.ttc",
        ];
        let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) else {
            eprintln!("no CJK font on this machine; skipped");
            return;
        };
        let mut fonts = Fonts::new();
        let cjk = fonts.add(bytes).unwrap();
        assert!(!fonts.has_glyph(FontId::DEFAULT, '保'));
        let text = "A 保存 ก";
        let before = fonts.shape(FontId::DEFAULT, text, 40.0);
        assert!(
            before.glyphs.iter().any(|g| g.glyph == 0),
            "boxes without a fallback"
        );
        fonts.set_fallbacks(vec![cjk]);
        let run = fonts.shape(FontId::DEFAULT, text, 40.0);
        assert!(run.glyphs.iter().all(|g| g.glyph != 0), "{:?}", run.glyphs);
        let font_of = |byte: usize| run.glyphs.iter().find(|g| g.cluster == byte).unwrap().font;
        assert_eq!(font_of(0), FontId::DEFAULT);
        assert_eq!(font_of(2), cjk);
        assert_eq!(font_of("A 保存 ".len()), FontId::DEFAULT);
        // Pen positions keep increasing across the runs.
        assert!(run.glyphs.windows(2).all(|w| w[1].x >= w[0].x));
        let sum: f32 = run.glyphs.iter().map(|g| g.advance).sum();
        assert!((sum - run.width).abs() < 1e-3);
    }
}
