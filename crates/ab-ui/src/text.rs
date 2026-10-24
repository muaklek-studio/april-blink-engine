//! Multi-line text layout on top of shaping: paragraphs, word wrap, alignment.

use crate::font::{FontId, Fonts, ShapedGlyph};
use ab_core::{Color, Vec2};

/// Horizontal alignment of text lines or of content in a box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Vertical alignment of text in its box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VAlign {
    #[default]
    Top,
    Center,
    Bottom,
}

/// A blurred copy of the text drawn behind it: a drop shadow, or a glow with no offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextShadow {
    pub color: Color,
    /// Offset in UI units.
    pub offset: Vec2,
    /// Blur radius in UI units.
    pub softness: f32,
    /// Grows the shadow beyond the glyph edge, in UI units (thicker glows).
    pub spread: f32,
}

/// How text looks. Sizes are in UI units (see [`UiSettings`](crate::UiSettings)).
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    pub font: FontId,
    /// Pixels per em at the reference resolution.
    pub size: f32,
    pub color: Color,
    /// Outline thickness in UI units (0 = none).
    pub outline_width: f32,
    pub outline_color: Color,
    pub shadow: Option<TextShadow>,
}

impl TextStyle {
    pub fn new(size: f32) -> Self {
        Self {
            size,
            ..Self::default()
        }
    }

    pub fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn with_outline(mut self, width: f32, color: Color) -> Self {
        self.outline_width = width;
        self.outline_color = color;
        self
    }

    pub fn with_shadow(mut self, shadow: TextShadow) -> Self {
        self.shadow = Some(shadow);
        self
    }

    pub fn with_font(mut self, font: FontId) -> Self {
        self.font = font;
        self
    }
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font: FontId::DEFAULT,
            size: 24.0,
            color: Color::WHITE,
            outline_width: 0.0,
            outline_color: Color::BLACK,
            shadow: None,
        }
    }
}

/// A laid-out line. Glyph positions are relative to the line start on its baseline.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextLine {
    pub glyphs: Vec<ShapedGlyph>,
    pub width: f32,
    /// Offset from the block's left edge after alignment.
    pub x: f32,
    /// Baseline position from the block's top.
    pub baseline: f32,
}

/// Text broken into lines and aligned, in UI units.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextBlock {
    pub lines: Vec<TextLine>,
    pub size: Vec2,
}

/// Lays out `text`: `\n` starts a new line; with `wrap_width`, lines also break at spaces,
/// and words wider than the line break between grapheme clusters (which never separates a
/// Thai consonant from its vowels and tone marks). Thai has no spaces between words, so
/// Thai sentences wrap at those cluster boundaries.
pub fn layout_text(
    fonts: &Fonts,
    text: &str,
    style: &TextStyle,
    wrap_width: Option<f32>,
    align: HAlign,
) -> TextBlock {
    let metrics = fonts.line_metrics(style.font, style.size);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        // Text written on Windows ends its lines with `\r\n`.
        let paragraph = paragraph.strip_suffix('\r').unwrap_or(paragraph);
        let run = fonts.shape(style.font, paragraph, style.size);
        match wrap_width {
            Some(width) if run.width > width => {
                wrap(paragraph, &run.glyphs, width, &mut lines);
            }
            _ => lines.push(TextLine {
                width: run.width,
                glyphs: run.glyphs,
                ..Default::default()
            }),
        }
    }
    let width = lines.iter().map(|l| l.width).fold(0.0, f32::max);
    let block_width = wrap_width.map_or(width, |w| w.max(width));
    for (i, line) in lines.iter_mut().enumerate() {
        line.baseline = metrics.ascent + i as f32 * metrics.line_height;
        line.x = match align {
            HAlign::Left => 0.0,
            HAlign::Center => (block_width - line.width) * 0.5,
            HAlign::Right => block_width - line.width,
        };
    }
    let height = if lines.is_empty() {
        0.0
    } else {
        metrics.ascent + metrics.descent + (lines.len() - 1) as f32 * metrics.line_height
    };
    TextBlock {
        lines,
        size: Vec2::new(width, height),
    }
}

/// Greedy line breaking over the glyphs of one shaped paragraph.
fn wrap(text: &str, glyphs: &[ShapedGlyph], max_width: f32, out: &mut Vec<TextLine>) {
    // Clusters: (start glyph index, end glyph index, advance, is whitespace).
    let mut clusters: Vec<(usize, usize, f32, bool)> = Vec::new();
    // First character of each cluster.
    let mut firsts: Vec<char> = Vec::new();
    for (i, glyph) in glyphs.iter().enumerate() {
        match clusters.last_mut() {
            Some(last) if glyphs[last.0].cluster == glyph.cluster => {
                last.1 = i + 1;
                last.2 += glyph.advance;
            }
            _ => {
                let first = text[glyph.cluster..].chars().next().unwrap_or(' ');
                clusters.push((i, i + 1, glyph.advance, first.is_whitespace()));
                firsts.push(first);
            }
        }
    }
    // Thai leading vowels (เ แ โ ใ ไ) belong to the consonant after them and following
    // vowels (ะ า ำ ๅ) to the one before, although each is a grapheme of its own.
    let can_break_before = |j: usize| {
        !matches!(firsts[j], '\u{0E30}' | '\u{0E32}' | '\u{0E33}' | '\u{0E45}')
            && !matches!(firsts[j - 1], '\u{0E40}'..='\u{0E44}')
    };

    let mut line_start = 0; // cluster index
    let mut width = 0.0;
    let mut last_space: Option<usize> = None; // cluster index of the last space on the line
    let mut i = 0;
    while i < clusters.len() {
        let (_, _, advance, space) = clusters[i];
        if space {
            last_space = Some(i);
        }
        if !space && width + advance > max_width && i > line_start {
            // Break after the last space, else before this cluster (or an earlier one that
            // allows a break).
            let (end, next) = match last_space {
                Some(s) if s >= line_start => (s, s + 1),
                _ => {
                    let end = (line_start + 1..=i)
                        .rev()
                        .find(|&j| can_break_before(j))
                        .unwrap_or(i);
                    (end, end)
                }
            };
            out.push(make_line(glyphs, &clusters[line_start..end]));
            line_start = next;
            // Skip spaces at the start of the new line.
            while line_start < clusters.len() && clusters[line_start].3 {
                line_start += 1;
            }
            width = clusters[line_start..i.max(line_start)]
                .iter()
                .map(|c| c.2)
                .sum();
            last_space = None;
            i = i.max(line_start);
            continue;
        }
        width += advance;
        i += 1;
    }
    if line_start < clusters.len() || out.is_empty() {
        out.push(make_line(
            glyphs,
            &clusters[line_start.min(clusters.len())..],
        ));
    }
}

fn make_line(glyphs: &[ShapedGlyph], clusters: &[(usize, usize, f32, bool)]) -> TextLine {
    let (Some(first), Some(_)) = (clusters.first(), clusters.last()) else {
        return TextLine::default();
    };
    // Trailing spaces do not count towards the width.
    let visible = clusters.iter().rposition(|c| !c.3).map_or(0, |i| i + 1);
    let origin = pen_x(glyphs, first.0);
    let end = clusters.last().map_or(first.0, |c| c.1);
    let line_glyphs = glyphs[first.0..end]
        .iter()
        .map(|g| ShapedGlyph {
            x: g.x - origin,
            ..*g
        })
        .collect();
    let width = clusters[..visible].iter().map(|c| c.2).sum();
    TextLine {
        glyphs: line_glyphs,
        width,
        ..Default::default()
    }
}

/// Pen position before glyph `index` (sum of the advances before it).
fn pen_x(glyphs: &[ShapedGlyph], index: usize) -> f32 {
    glyphs[..index].iter().map(|g| g.advance).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The source text of each line (from its first cluster to the next line's).
    fn lines(block: &TextBlock, text: &str) -> Vec<String> {
        let starts: Vec<usize> = block
            .lines
            .iter()
            .map(|l| l.glyphs.first().map_or(text.len(), |g| g.cluster))
            .collect();
        starts
            .iter()
            .enumerate()
            .map(|(i, &start)| {
                let end = starts.get(i + 1).copied().unwrap_or(text.len());
                text[start..end].trim_end().to_owned()
            })
            .collect()
    }

    #[test]
    fn newlines_and_metrics() {
        let fonts = Fonts::new();
        let style = TextStyle::new(20.0);
        let block = layout_text(&fonts, "one\ntwo\n\nfour", &style, None, HAlign::Left);
        assert_eq!(block.lines.len(), 4);
        let metrics = fonts.line_metrics(FontId::DEFAULT, 20.0);
        assert!(
            (block.lines[1].baseline - block.lines[0].baseline - metrics.line_height).abs() < 1e-4
        );
        assert!(block.lines[2].glyphs.is_empty());
        let expected = metrics.ascent + metrics.descent + 3.0 * metrics.line_height;
        assert!((block.size.y - expected).abs() < 1e-3);
        // Windows line endings lay out the same.
        let crlf = layout_text(&fonts, "one\r\ntwo", &style, None, HAlign::Left);
        let lf = layout_text(&fonts, "one\ntwo", &style, None, HAlign::Left);
        assert_eq!(crlf.size, lf.size);
        assert_eq!(crlf.lines[0].glyphs.len(), lf.lines[0].glyphs.len());
    }

    #[test]
    fn wraps_at_spaces() {
        let fonts = Fonts::new();
        let style = TextStyle::new(20.0);
        let text = "the quick brown fox jumps over the lazy dog";
        let one_line = layout_text(&fonts, text, &style, None, HAlign::Left);
        let width = one_line.size.x * 0.45;
        let block = layout_text(&fonts, text, &style, Some(width), HAlign::Left);
        assert!(block.lines.len() >= 3, "{:?}", lines(&block, text));
        for line in &block.lines {
            assert!(line.width <= width + 1e-3, "{} > {width}", line.width);
            assert_eq!(line.glyphs.first().map(|g| g.x), Some(0.0));
        }
        // No word was split, no line starts with a space.
        let words: Vec<String> = lines(&block, text)
            .iter()
            .flat_map(|l| l.split(' ').map(str::to_owned).collect::<Vec<_>>())
            .collect();
        assert_eq!(words.join(" "), text);
        assert!(lines(&block, text).iter().all(|l| !l.starts_with(' ')));
    }

    #[test]
    fn thai_wraps_between_clusters() {
        let fonts = Fonts::new();
        let style = TextStyle::new(20.0);
        let text = "สวัสดีครับยินดีต้อนรับสู่เกมของเรา";
        let full = layout_text(&fonts, text, &style, None, HAlign::Left);
        let block = layout_text(&fonts, text, &style, Some(full.size.x / 3.0), HAlign::Left);
        assert!(block.lines.len() >= 3);
        // Every line starts on a cluster boundary with a base character, never a mark.
        for line in &block.lines {
            let first = line.glyphs[0];
            let c = text[first.cluster..].chars().next().unwrap();
            assert!(
                !matches!(c, '\u{0E31}' | '\u{0E34}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}'),
                "line starts with a mark: {c:?}"
            );
        }
        assert_eq!(lines(&block, text).concat(), text);
    }

    #[test]
    fn thai_leading_and_following_vowels_stay_with_their_consonant() {
        let fonts = Fonts::new();
        let style = TextStyle::new(20.0);
        // Leading vowels (เ แ โ ใ ไ) are written before their consonant and following ones
        // (ะ า ำ) after it, each as a grapheme of its own.
        let text = "เกมแมวโลกใจไฟเพลงเสียงแสงเงาไกลมากำลังจะมาถึงแล้วนะ";
        let full = layout_text(&fonts, text, &style, None, HAlign::Left);
        for parts in 2..12 {
            let width = full.size.x / parts as f32;
            let block = layout_text(&fonts, text, &style, Some(width), HAlign::Left);
            let lines = lines(&block, text);
            assert_eq!(lines.concat(), text);
            for line in &lines {
                let first = line.chars().next().unwrap();
                let last = line.chars().last().unwrap();
                assert!(
                    !matches!(first, '\u{0E30}' | '\u{0E32}' | '\u{0E33}' | '\u{0E45}'),
                    "line starts with a following vowel: {lines:?}"
                );
                assert!(
                    !matches!(last, '\u{0E40}'..='\u{0E44}'),
                    "line ends with a leading vowel: {lines:?}"
                );
            }
        }
    }

    #[test]
    fn emoji_sequences_are_not_split() {
        let fonts = Fonts::new();
        let style = TextStyle::new(20.0);
        // Family (ZWJ sequence), thumbs up with skin tone, keycap, flag pair: rustybuzz
        // shapes each as one cluster, so wrapping never splits them.
        let units = [
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
            "\u{1F44D}\u{1F3FD}",
            "1\u{FE0F}\u{20E3}",
            "\u{1F1F9}\u{1F1ED}",
        ];
        let text = units.concat().repeat(4);
        let full = layout_text(&fonts, &text, &style, None, HAlign::Left);
        for parts in 2..20 {
            let width = full.size.x / parts as f32;
            let block = layout_text(&fonts, &text, &style, Some(width), HAlign::Left);
            let lines = lines(&block, &text);
            assert_eq!(lines.concat(), text);
            for line in &lines {
                let mut rest = line.as_str();
                while !rest.is_empty() {
                    let unit = units.iter().find(|u| rest.starts_with(*u));
                    let unit = unit.unwrap_or_else(|| panic!("split sequence: {lines:?}"));
                    rest = &rest[unit.len()..];
                }
            }
        }
    }

    #[test]
    fn alignment() {
        let fonts = Fonts::new();
        let style = TextStyle::new(20.0);
        let block = layout_text(&fonts, "a\nwide line", &style, Some(300.0), HAlign::Center);
        let [short, long] = [&block.lines[0], &block.lines[1]];
        assert!((short.x - (300.0 - short.width) / 2.0).abs() < 1e-3);
        assert!((long.x - (300.0 - long.width) / 2.0).abs() < 1e-3);
        let right = layout_text(&fonts, "abc", &style, Some(100.0), HAlign::Right);
        assert!((right.lines[0].x + right.lines[0].width - 100.0).abs() < 1e-3);
    }

    #[test]
    fn empty_text() {
        let fonts = Fonts::new();
        let block = layout_text(&fonts, "", &TextStyle::default(), Some(50.0), HAlign::Left);
        assert_eq!(block.lines.len(), 1);
        assert_eq!(block.size.x, 0.0);
    }
}
