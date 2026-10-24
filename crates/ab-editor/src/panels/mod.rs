//! Dockable panels and the small widgets they share.

pub mod animation;
pub mod assets;
pub mod build;
pub mod config;
pub mod console;
pub mod debug;
pub mod effects;
pub mod git;
pub mod inspector;
pub mod profiler;
pub mod scene_tree;

use crate::theme;
use ab_engine::core::Color;
use ab_engine::scene_file::ColorDesc;
use eframe::egui::{self, Color32, RichText};

/// A foldable section: a full-width header band (click anywhere on it) with an icon and a
/// title, and its contents below.
pub fn section(
    ui: &mut egui::Ui,
    id: &str,
    icon: &str,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let id = ui.make_persistent_id(id);
    let mut state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
    let header = egui::Frame::NONE
        .fill(theme::BG_2)
        .corner_radius(3)
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let caret = if state.is_open() {
                    egui_phosphor::regular::CARET_DOWN
                } else {
                    egui_phosphor::regular::CARET_RIGHT
                };
                ui.label(RichText::new(caret).size(11.0).color(theme::TEXT_DIM));
                ui.label(RichText::new(icon).color(theme::ACCENT));
                ui.label(RichText::new(title).strong());
            });
        });
    let click = ui.interact(
        header.response.rect,
        id.with("header"),
        egui::Sense::click(),
    );
    if click.clicked() {
        state.toggle(ui);
    }
    if click.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    state.show_body_unindented(ui, |ui| {
        egui::Frame::NONE
            .inner_margin(egui::Margin {
                left: 6,
                right: 2,
                top: 3,
                bottom: 6,
            })
            .show(ui, add_contents);
    });
}

/// Two-column property grid: dim labels on the left, editors on the right.
pub fn grid(ui: &mut egui::Ui, id: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([8.0, 3.0])
        .min_col_width(112.0)
        .striped(false)
        .show(ui, add_contents);
}

pub fn label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::TEXT_DIM));
}

/// An `Option<f32>` field showing `default` while unset.
pub fn opt_f32(
    ui: &mut egui::Ui,
    value: &mut Option<f32>,
    default: f32,
    range: std::ops::RangeInclusive<f32>,
    speed: f64,
) -> bool {
    let mut v = value.unwrap_or(default);
    let changed = ui
        .add(
            egui::DragValue::new(&mut v)
                .custom_parser(crate::panels::parse_finite)
                .range(range)
                .speed(speed)
                .max_decimals(3),
        )
        .changed();
    if changed {
        *value = Some(v);
    }
    changed
}

pub fn opt_u32(
    ui: &mut egui::Ui,
    value: &mut Option<u32>,
    default: u32,
    range: std::ops::RangeInclusive<u32>,
) -> bool {
    let mut v = value.unwrap_or(default);
    let changed = ui
        .add(
            egui::DragValue::new(&mut v)
                .custom_parser(crate::panels::parse_finite)
                .range(range)
                .speed(1.0),
        )
        .changed();
    if changed {
        *value = Some(v);
    }
    changed
}

pub fn opt_bool(ui: &mut egui::Ui, value: &mut Option<bool>, default: bool) -> bool {
    let mut v = value.unwrap_or(default);
    let changed = ui.checkbox(&mut v, "").changed();
    if changed {
        *value = Some(v);
    }
    changed
}

/// Linear engine color → egui (linear) color for the picker.
pub fn to_rgba(color: Color) -> egui::Rgba {
    egui::Rgba::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

/// A color field over a scene-file color (`#rrggbb[aa]`), `default` while unset.
pub fn opt_color(
    ui: &mut egui::Ui,
    value: &mut Option<ColorDesc>,
    default: Color,
    alpha: bool,
) -> bool {
    let color = value
        .as_ref()
        .and_then(|c| c.to_color().ok())
        .unwrap_or(default);
    let mut rgba = to_rgba(color);
    let mode = if alpha {
        egui::color_picker::Alpha::OnlyBlend
    } else {
        egui::color_picker::Alpha::Opaque
    };
    let changed = egui::color_picker::color_edit_button_rgba(ui, &mut rgba, mode).changed();
    if changed {
        let [r, g, b, a] = rgba.to_rgba_unmultiplied();
        *value = Some(ColorDesc::from_color(Color::rgba(r, g, b, a)));
    }
    changed
}

/// Three drag values labelled X/Y/Z in axis colors.
pub fn vec3(ui: &mut egui::Ui, value: &mut [f32; 3], speed: f64, suffix: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let width = ((ui.available_width() - 3.0 * 16.0) / 3.0).max(40.0);
        for (i, (axis, color)) in [
            ("X", theme::AXIS_X),
            ("Y", theme::AXIS_Y),
            ("Z", theme::AXIS_Z),
        ]
        .into_iter()
        .enumerate()
        {
            ui.label(RichText::new(axis).color(color).strong());
            changed |= ui
                .add_sized(
                    [width, ui.spacing().interact_size.y],
                    egui::DragValue::new(&mut value[i])
                        .custom_parser(crate::panels::parse_finite)
                        .speed(speed)
                        .max_decimals(3)
                        .suffix(suffix),
                )
                .changed();
        }
    });
    changed
}

/// A muted, centered hint for empty panels.
pub fn empty_hint(ui: &mut egui::Ui, icon: &str, text: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icon).size(32.0).color(theme::BG_4));
        ui.label(RichText::new(text).color(theme::TEXT_DIM));
    });
}

/// Fill color for a selected row.
pub const ROW_SELECTED: Color32 = theme::ACCENT_DIM;

/// Whether `c` may stay in a name made from user text (node, clip and file names): letters,
/// digits and the marks that combine with them. `char::is_alphanumeric` alone turns Thai
/// tone marks and vowels above or below (ไม้, กล่อง) into `_`.
pub fn is_name_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(c,
            '\u{0300}'..='\u{036F}' // combining diacritics
            | '\u{0E31}' | '\u{0E34}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}' // Thai
            | '\u{0EB1}' | '\u{0EB4}'..='\u{0EBC}' | '\u{0EC8}'..='\u{0ECD}' // Lao
        )
}

/// Number fields' parser: egui's, minus `inf` and `nan` (Rust's float parser takes them, and
/// a non-finite position or size breaks bounds, framing and the saved scene).
pub fn parse_finite(text: &str) -> Option<f64> {
    let text: String = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == '\u{2212}' { '-' } else { c })
        .collect();
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}
