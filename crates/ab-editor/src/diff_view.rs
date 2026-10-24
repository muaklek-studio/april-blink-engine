//! Diffs as a code review tool shows them: old and new line numbers, added lines on green,
//! removed lines on red, and within a changed line the part that changed in a stronger
//! tone. Unified (one column) or side by side (old left, new right). Only the rows in view
//! are laid out, so long diffs scroll smoothly.

use crate::theme;
use eframe::egui::{self, text::LayoutJob, Color32, FontId, Pos2, Rect, RichText, Sense, Vec2};
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Context,
    Added,
    Removed,
    /// `@@ -1,4 +1,5 @@ fn main`
    Hunk,
    /// File headers (`diff --git`, `new file mode`, `Binary files … differ`).
    Meta,
    /// `\ No newline at end of file`, `… 200 more lines`.
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: Kind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    /// Without the leading `+`, `-` or space.
    pub text: String,
    /// The part of the line that changed (byte range of `text`), when it pairs with a
    /// line on the other side.
    pub emphasis: Option<Range<usize>>,
}

/// A parsed diff and its counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    pub lines: Vec<Line>,
    pub added: usize,
    pub removed: usize,
    pub binary: bool,
}

/// `@@ -12,3 +12,4 @@` → (12, 12).
fn hunk_start(header: &str) -> Option<(u32, u32)> {
    let mut parts = header.split_whitespace().skip(1);
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let first = |s: &str| s.split(',').next()?.parse().ok();
    Some((first(old)?, first(new)?))
}

pub fn parse(text: &str) -> Diff {
    let mut diff = Diff::default();
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    for raw in text.lines() {
        let (kind, body) = if raw.starts_with("@@") {
            if let Some((o, n)) = hunk_start(raw) {
                (old, new) = (o, n);
            }
            in_hunk = true;
            (Kind::Hunk, raw)
        } else if raw.starts_with("diff ")
            || (!in_hunk && (raw.starts_with("--- ") || raw.starts_with("+++ ")))
        {
            in_hunk = false;
            (Kind::Meta, raw)
        } else if !in_hunk {
            if raw.starts_with("Binary files") || raw.starts_with("Binary file") {
                diff.binary = true;
            }
            if raw.starts_with("index ") {
                continue;
            }
            (Kind::Meta, raw)
        } else if let Some(rest) = raw.strip_prefix('+') {
            (Kind::Added, rest)
        } else if let Some(rest) = raw.strip_prefix('-') {
            (Kind::Removed, rest)
        } else if let Some(rest) = raw.strip_prefix(' ') {
            (Kind::Context, rest)
        } else if raw.is_empty() {
            (Kind::Context, raw)
        } else {
            (Kind::Note, raw)
        };
        let (o, n) = match kind {
            Kind::Context => (Some(old), Some(new)),
            Kind::Added => (None, Some(new)),
            Kind::Removed => (Some(old), None),
            _ => (None, None),
        };
        match kind {
            Kind::Context => {
                old += 1;
                new += 1;
            }
            Kind::Added => {
                new += 1;
                diff.added += 1;
            }
            Kind::Removed => {
                old += 1;
                diff.removed += 1;
            }
            _ => {}
        }
        diff.lines.push(Line {
            kind,
            old: o,
            new: n,
            text: body.replace('\t', "    "),
            emphasis: None,
        });
    }
    emphasize(&mut diff.lines);
    diff
}

/// Pairs each block of removed lines with the added lines after it, and marks in each pair
/// what changed between the common start and the common end.
fn emphasize(lines: &mut [Line]) {
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind != Kind::Removed {
            i += 1;
            continue;
        }
        let removed_start = i;
        while i < lines.len() && lines[i].kind == Kind::Removed {
            i += 1;
        }
        let added_start = i;
        while i < lines.len() && lines[i].kind == Kind::Added {
            i += 1;
        }
        let pairs = (added_start - removed_start).min(i - added_start);
        for k in 0..pairs {
            let (a, b) = (removed_start + k, added_start + k);
            if let Some((old, new)) = changed_parts(&lines[a].text, &lines[b].text) {
                lines[a].emphasis = Some(old);
                lines[b].emphasis = Some(new);
            }
        }
    }
}

/// The differing middles of two lines (after their common prefix, before their common
/// suffix), when they are alike enough for that to help (else the whole line changed).
pub fn changed_parts(old: &str, new: &str) -> Option<(Range<usize>, Range<usize>)> {
    let prefix: usize = old
        .char_indices()
        .zip(new.chars())
        .take_while(|((_, a), b)| a == b)
        .last()
        .map_or(0, |((i, c), _)| i + c.len_utf8());
    let suffix: usize = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    let old_mid = prefix..old.len() - suffix;
    let new_mid = prefix..new.len() - suffix;
    let longest = old.len().max(new.len()).max(1);
    let changed = old_mid.len().max(new_mid.len());
    // Mostly the same line: worth pointing at the change.
    (changed * 10 <= longest * 7 && longest > 3).then_some((old_mid, new_mid))
}

/// How a diff is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    Unified,
    SideBySide,
}

const ADDED_BG: Color32 = Color32::from_rgba_premultiplied(30, 70, 40, 90);
const REMOVED_BG: Color32 = Color32::from_rgba_premultiplied(80, 30, 34, 90);
const ADDED_STRONG: Color32 = Color32::from_rgba_premultiplied(46, 130, 66, 170);
const REMOVED_STRONG: Color32 = Color32::from_rgba_premultiplied(150, 46, 52, 170);
const HUNK_BG: Color32 = Color32::from_rgba_premultiplied(30, 50, 80, 110);

fn font() -> FontId {
    FontId::monospace(12.5)
}

/// The counts as a header shows them: `+12 −3`.
pub fn counts(ui: &mut egui::Ui, added: usize, removed: usize) {
    ui.label(
        RichText::new(format!("+{added}"))
            .monospace()
            .color(theme::PLAY),
    );
    ui.label(
        RichText::new(format!("−{removed}"))
            .monospace()
            .color(theme::ERROR),
    );
}

/// One side of a row: its line number and text, laid out in `rect`.
fn paint_line(ui: &egui::Ui, rect: Rect, line: &Line, number: Option<u32>, gutter: f32) {
    let painter = ui.painter_at(rect);
    let (bg, strong, sign) = match line.kind {
        Kind::Added => (ADDED_BG, ADDED_STRONG, "+"),
        Kind::Removed => (REMOVED_BG, REMOVED_STRONG, "−"),
        Kind::Hunk => (HUNK_BG, Color32::TRANSPARENT, ""),
        _ => (Color32::TRANSPARENT, Color32::TRANSPARENT, " "),
    };
    if bg != Color32::TRANSPARENT {
        painter.rect_filled(rect, 0.0, bg);
    }
    let y = rect.center().y;
    if let Some(n) = number {
        painter.text(
            Pos2::new(rect.left() + gutter - 8.0, y),
            egui::Align2::RIGHT_CENTER,
            n.to_string(),
            FontId::monospace(11.0),
            theme::TEXT_DIM,
        );
    }
    let text_color = match line.kind {
        Kind::Hunk => theme::ACCENT,
        Kind::Meta | Kind::Note => theme::TEXT_DIM,
        _ => theme::TEXT,
    };
    let x = rect.left() + gutter;
    if !sign.is_empty() {
        painter.text(
            Pos2::new(x, y),
            egui::Align2::LEFT_CENTER,
            sign,
            font(),
            match line.kind {
                Kind::Added => theme::PLAY,
                Kind::Removed => theme::ERROR,
                _ => theme::TEXT_DIM,
            },
        );
    }
    let mut job = LayoutJob::default();
    let format = |background: Color32| egui::TextFormat {
        font_id: font(),
        color: text_color,
        background,
        ..Default::default()
    };
    match &line.emphasis {
        Some(range) if range.end <= line.text.len() && !range.is_empty() => {
            job.append(&line.text[..range.start], 0.0, format(Color32::TRANSPARENT));
            job.append(&line.text[range.clone()], 0.0, format(strong));
            job.append(&line.text[range.end..], 0.0, format(Color32::TRANSPARENT));
        }
        _ => job.append(&line.text, 0.0, format(Color32::TRANSPARENT)),
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let at = Pos2::new(x + 14.0, y - galley.size().y / 2.0);
    painter.galley(at, galley, text_color);
}

/// Width of the line-number column for numbers up to `largest`.
fn gutter_width(largest: u32) -> f32 {
    let digits = largest.max(1).ilog10() as f32 + 1.0;
    digits * 7.5 + 18.0
}

/// Shows `diff` in `layout`, filling the space given.
pub fn show(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    diff: &Diff,
    layout: Layout,
) {
    if diff.binary {
        crate::panels::empty_hint(
            ui,
            egui_phosphor::regular::FILE,
            "Binary file: no line changes to show",
        );
        return;
    }
    if diff.lines.iter().all(|l| matches!(l.kind, Kind::Meta)) {
        crate::panels::empty_hint(
            ui,
            egui_phosphor::regular::CHECK,
            "No textual changes (mode or rename only)",
        );
        return;
    }
    let largest = diff
        .lines
        .iter()
        .flat_map(|l| [l.old, l.new])
        .flatten()
        .max()
        .unwrap_or(1);
    let gutter = gutter_width(largest);
    let row_height = ui.fonts_mut(|f| f.row_height(&font())) + 3.0;
    match layout {
        Layout::Unified => {
            let longest = diff
                .lines
                .iter()
                .map(|l| l.text.chars().count())
                .max()
                .unwrap_or(0);
            let width = (longest as f32 * 7.6 + gutter * 2.0 + 40.0).max(ui.available_width());
            egui::ScrollArea::both()
                .id_salt(("diff", &id))
                .auto_shrink([false, false])
                .show_rows(ui, row_height, diff.lines.len(), |ui, range| {
                    ui.set_min_width(width);
                    for line in &diff.lines[range] {
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(width, row_height), Sense::hover());
                        // Old number, then new number, then the text.
                        let old_rect = Rect::from_min_size(rect.min, Vec2::new(gutter, row_height));
                        if let Some(n) = line.old {
                            ui.painter().text(
                                Pos2::new(old_rect.right() - 8.0, old_rect.center().y),
                                egui::Align2::RIGHT_CENTER,
                                n.to_string(),
                                FontId::monospace(11.0),
                                theme::TEXT_DIM,
                            );
                        }
                        let rest = Rect::from_min_max(
                            Pos2::new(rect.left() + gutter, rect.top()),
                            rect.max,
                        );
                        paint_line(ui, rest, line, line.new, gutter);
                    }
                });
        }
        Layout::SideBySide => {
            let rows = side_by_side(&diff.lines);
            egui::ScrollArea::vertical()
                .id_salt(("diff-split", &id))
                .auto_shrink([false, false])
                .show_rows(ui, row_height, rows.len(), |ui, range| {
                    let width = ui.available_width();
                    let half = (width - 2.0) / 2.0;
                    for (left, right) in &rows[range] {
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(width, row_height), Sense::hover());
                        let left_rect = Rect::from_min_size(rect.min, Vec2::new(half, row_height));
                        let right_rect = Rect::from_min_size(
                            Pos2::new(rect.left() + half + 2.0, rect.top()),
                            Vec2::new(half, row_height),
                        );
                        match (left, right) {
                            (Some(l), _)
                                if matches!(l.kind, Kind::Hunk | Kind::Meta | Kind::Note) =>
                            {
                                paint_line(ui, rect, l, None, gutter);
                            }
                            _ => {
                                if let Some(l) = left {
                                    paint_line(ui, left_rect, l, l.old, gutter);
                                } else {
                                    ui.painter().rect_filled(left_rect, 0.0, theme::BG_0);
                                }
                                if let Some(r) = right {
                                    paint_line(ui, right_rect, r, r.new, gutter);
                                } else {
                                    ui.painter().rect_filled(right_rect, 0.0, theme::BG_0);
                                }
                            }
                        }
                        ui.painter().vline(
                            rect.left() + half + 1.0,
                            rect.y_range(),
                            egui::Stroke::new(1.0, theme::BORDER),
                        );
                    }
                });
        }
    }
}

/// Rows of the side-by-side layout: context on both sides, a removed block beside the
/// added block after it, headers across.
pub fn side_by_side(lines: &[Line]) -> Vec<(Option<&Line>, Option<&Line>)> {
    let mut rows = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        match lines[i].kind {
            Kind::Context => {
                rows.push((Some(&lines[i]), Some(&lines[i])));
                i += 1;
            }
            Kind::Removed | Kind::Added => {
                let start = i;
                while i < lines.len() && lines[i].kind == Kind::Removed {
                    i += 1;
                }
                let middle = i;
                while i < lines.len() && lines[i].kind == Kind::Added {
                    i += 1;
                }
                let (removed, added) = (&lines[start..middle], &lines[middle..i]);
                for k in 0..removed.len().max(added.len()) {
                    rows.push((removed.get(k), added.get(k)));
                }
            }
            _ => {
                rows.push((Some(&lines[i]), None));
                i += 1;
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = concat!(
        "diff --git a/level.toml b/level.toml\n",
        "index 1111111..2222222 100644\n",
        "--- a/level.toml\n",
        "+++ b/level.toml\n",
        "@@ -3,4 +3,5 @@ [player]\n",
        " name = \"hero\"\n",
        "-speed = 4.0\n",
        "+speed = 6.5\n",
        "+jump = 2.0\n",
        " health = 100\n",
    );

    #[test]
    fn numbers_lines_and_counts_changes() {
        let diff = parse(SAMPLE);
        assert_eq!((diff.added, diff.removed), (2, 1));
        assert!(!diff.binary);
        let context = &diff.lines[4];
        assert_eq!(
            (context.kind, context.old, context.new),
            (Kind::Context, Some(3), Some(3))
        );
        let removed = diff.lines.iter().find(|l| l.kind == Kind::Removed).unwrap();
        assert_eq!(removed.old, Some(4));
        let last = diff.lines.last().unwrap();
        assert_eq!((last.old, last.new), (Some(5), Some(6)));
        assert!(
            diff.lines.iter().all(|l| !l.text.starts_with("index ")),
            "index lines are noise"
        );
    }

    #[test]
    fn a_changed_line_points_at_what_changed() {
        let diff = parse(SAMPLE);
        let removed = diff.lines.iter().find(|l| l.kind == Kind::Removed).unwrap();
        let added = diff.lines.iter().find(|l| l.kind == Kind::Added).unwrap();
        assert_eq!(&removed.text[removed.emphasis.clone().unwrap()], "4.0");
        assert_eq!(&added.text[added.emphasis.clone().unwrap()], "6.5");
        // The second added line has no partner: whole-line change.
        assert_eq!(
            diff.lines.iter().filter(|l| l.emphasis.is_some()).count(),
            2
        );
        // Lines with nothing in common are not "partly" changed.
        assert_eq!(changed_parts("abc def", "xyz uvw"), None);
        // Non-ASCII text stays on character boundaries.
        let (old, new) = changed_parts("ชื่อ = แมว", "ชื่อ = หมา").unwrap();
        assert_eq!(&"ชื่อ = แมว"[old], "แมว");
        assert_eq!(&"ชื่อ = หมา"[new], "หมา");
    }

    #[test]
    fn side_by_side_pairs_removed_with_added() {
        let diff = parse(SAMPLE);
        let rows = side_by_side(&diff.lines);
        let changed: Vec<_> = rows
            .iter()
            .filter(|(l, r)| {
                l.is_some_and(|l| l.kind == Kind::Removed)
                    || r.is_some_and(|r| r.kind == Kind::Added)
            })
            .collect();
        assert_eq!(changed.len(), 2);
        assert_eq!(changed[0].0.unwrap().text, "speed = 4.0");
        assert_eq!(changed[0].1.unwrap().text, "speed = 6.5");
        assert!(
            changed[1].0.is_none(),
            "an added line with no removed partner"
        );
    }

    #[test]
    fn binary_and_untracked_diffs() {
        assert!(parse("Binary files a/x.png and b/x.png differ\n").binary);
        let new = parse("new file notes.md\n@@ -0,0 +1,2 @@\n+one\n+two\n");
        assert_eq!(new.added, 2);
        assert_eq!(new.lines.last().unwrap().new, Some(2));
    }
}
