//! Dockable panels and the small widgets they share.

pub mod animation;
pub mod assets;
pub mod audio_mixer;
pub mod build;
pub mod config;
pub mod console;
pub mod cutscene;
pub mod debug;
pub mod effects;
pub mod git;
pub mod inspector;
pub mod memory;
pub mod profiler;
pub mod rigging;
pub mod scene_tree;
pub mod sound_event;
pub mod state_graph;

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
    // A panel's filter (see [`with_section_filter`]): only sections whose title, or one of
    // whose fields, matches.
    let filter = current_filter(ui);
    let filtering = !filter.is_empty();
    let fields_id = egui::Id::new((SECTION_FIELDS, id));
    if filtering {
        let translated = crate::i18n::field(title);
        let fields: Vec<String> = ui.data(|d| d.get_temp(fields_id)).unwrap_or_default();
        let matches = [title, translated.as_str(), id]
            .into_iter()
            .chain(fields.iter().map(String::as_str))
            .any(|text| matches_filter(&filter, text));
        if !matches {
            return;
        }
    }
    let id = ui.make_persistent_id(id);
    let mut state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
    if filtering && !state.is_open() {
        // What was searched for is shown, folded or not.
        state.set_open(true);
    }
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
                ui.label(RichText::new(crate::i18n::field(title)).strong());
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
        // The fields drawn inside are remembered, so a later filter finds the section by
        // them (see [`label`]).
        let current = egui::Id::new(CURRENT_SECTION);
        let outer = ui.data(|d| d.get_temp::<egui::Id>(current));
        ui.data_mut(|d| d.insert_temp(current, fields_id));
        egui::Frame::NONE
            .inner_margin(egui::Margin {
                left: 6,
                right: 2,
                top: 3,
                bottom: 6,
            })
            .show(ui, add_contents);
        // A section inside another hands the fields back to the outer one.
        ui.data_mut(|d| match outer {
            Some(outer) => {
                d.insert_temp(current, outer);
            }
            None => {
                d.remove::<egui::Id>(current);
            }
        });
    });
}

const SECTION_FIELDS: &str = "ab-editor.section-fields";
const CURRENT_SECTION: &str = "ab-editor.current-section";

/// What the panel being drawn is filtered by (empty: nothing).
fn current_filter(ui: &egui::Ui) -> String {
    ui.data(|d| d.get_temp::<String>(egui::Id::new(SECTION_FILTER)))
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// Whether a section or field name answers the filter (in English or translated).
fn matches_filter(filter: &str, text: &str) -> bool {
    crate::palette::score(filter, text).is_some()
        || crate::palette::score(filter, &crate::i18n::field(text)).is_some()
}

/// Notes a field's name under the section being drawn.
fn remember_field(ui: &egui::Ui, text: &str) {
    let Some(section) = ui.data(|d| d.get_temp::<egui::Id>(egui::Id::new(CURRENT_SECTION))) else {
        return;
    };
    ui.data_mut(|d| {
        let fields = d.get_temp_mut_or_default::<Vec<String>>(section);
        if !fields.iter().any(|f| f == text) {
            fields.push(text.to_owned());
        }
    });
}

const SECTION_FILTER: &str = "ab-editor.section-filter";

/// Draws `add_contents` showing only the [`section`]s whose title matches `filter`.
pub fn with_section_filter<R>(
    ui: &mut egui::Ui,
    filter: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let id = egui::Id::new(SECTION_FILTER);
    ui.data_mut(|d| d.insert_temp(id, filter.to_owned()));
    let result = add_contents(ui);
    ui.data_mut(|d| d.remove::<String>(id));
    result
}

/// Two-column property grid: dim labels on the left, editors on the right, every other
/// row shaded so a value is easy to follow back to its name.
pub fn grid(ui: &mut egui::Ui, id: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([10.0, 4.0])
        .min_col_width(112.0)
        .striped(true)
        .show(ui, |ui| {
            // Number fields wide enough to read and to grab for dragging.
            ui.spacing_mut().interact_size.x = 64.0;
            add_contents(ui);
        });
}

/// A field's name (translated, see [`crate::i18n::field`]); highlighted when it is what
/// the panel's filter looks for.
pub fn label(ui: &mut egui::Ui, text: &str) {
    remember_field(ui, text);
    let filter = current_filter(ui);
    let shown = RichText::new(crate::i18n::field(text));
    if !filter.is_empty() && matches_filter(&filter, text) {
        ui.label(shown.color(theme::ACCENT).strong());
    } else {
        ui.label(shown.color(theme::TEXT_DIM));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws a panel of two sections (one inside a fold of the other) under `filter`;
    /// returns which bodies were drawn.
    fn draw(ctx: &egui::Context, filter: &str) -> (bool, bool) {
        let mut shown = (false, false);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            with_section_filter(ui, filter, |ui| {
                section(ui, "sound", "", "Sound", |ui| {
                    shown.0 = true;
                    label(ui, "Volume");
                });
                section(ui, "space", "", "3D sound", |ui| {
                    shown.1 = true;
                    label(ui, "Rolloff");
                    label(ui, "Doppler");
                });
            });
        });
        output.textures_delta.clear();
        shown
    }

    #[test]
    fn a_filter_finds_sections_by_their_title_or_their_fields() {
        let ctx = egui::Context::default();
        assert_eq!(draw(&ctx, ""), (true, true), "unfiltered, every section");
        assert_eq!(draw(&ctx, "rolloff"), (false, true), "by a field inside");
        assert_eq!(draw(&ctx, "sound"), (true, true), "by the titles");
        assert_eq!(draw(&ctx, "volu"), (true, false));
        assert_eq!(draw(&ctx, "zzz"), (false, false));
        assert_eq!(draw(&ctx, ""), (true, true), "back when cleared");
    }
}
