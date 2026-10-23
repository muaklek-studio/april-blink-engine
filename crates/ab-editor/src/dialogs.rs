//! Shared layout of the editor's small modal dialogs: an icon, a title, a wrapped message
//! and a row of buttons, at a fixed comfortable width that shrinks on narrow windows.

use crate::theme;
use eframe::egui::{self, Key, RichText, Vec2};

/// Width of a confirmation dialog's contents, when the window has room for it.
pub const CONFIRM_WIDTH: f32 = 400.0;
/// Room kept between a dialog and the window edges.
const WINDOW_MARGIN: f32 = 48.0;
const BUTTON_SIZE: Vec2 = Vec2::new(92.0, 28.0);

/// The width a dialog's contents get in this window.
pub fn dialog_width(ctx: &egui::Context, wanted: f32) -> f32 {
    let room = ctx.content_rect().width() - WINDOW_MARGIN;
    wanted.min(room).max(200.0_f32.min(room)).max(0.0)
}

/// Fixes the width of a dialog's contents (both ways: never wider, never narrower).
pub fn set_dialog_width(ui: &mut egui::Ui, wanted: f32) {
    let width = dialog_width(ui.ctx(), wanted);
    ui.set_min_width(width);
    ui.set_max_width(width);
}

/// What the user chose in the unsaved-changes dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsavedChoice {
    Save,
    DontSave,
    Cancel,
}

/// The unsaved-changes dialog's contents. `document` is the scene's name; `action` says
/// what waits ("before quitting"). Enter saves; Escape (handled by the modal) cancels.
pub fn unsaved_changes(ui: &mut egui::Ui, document: &str, action: &str) -> Option<UnsavedChoice> {
    use egui_phosphor::regular as icon;
    set_dialog_width(ui, CONFIRM_WIDTH);
    let mut choice = None;
    ui.horizontal_top(|ui| {
        ui.label(RichText::new(icon::WARNING).size(28.0).color(theme::WARN));
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.label(
                RichText::new(crate::i18n::tf(
                    "dialog.unsaved_title",
                    &[("action", &action)],
                ))
                .font(theme::bold(15.0))
                .color(egui::Color32::WHITE),
            );
            ui.add_space(2.0);
            ui.add(
                egui::Label::new(
                    RichText::new(crate::i18n::tf(
                        "dialog.unsaved_body",
                        &[("document", &document)],
                    ))
                    .color(theme::TEXT_DIM),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        });
    });
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let button = |text: RichText| egui::Button::new(text).min_size(BUTTON_SIZE);
        if ui
            .add(button(
                RichText::new(crate::i18n::t("dialog.dont_save")).color(theme::ERROR),
            ))
            .clicked()
        {
            choice = Some(UnsavedChoice::DontSave);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let save = ui.add(
                button(RichText::new(crate::i18n::t("dialog.save")).strong())
                    .fill(theme::ACCENT_DIM),
            );
            if save.clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                choice = Some(UnsavedChoice::Save);
            }
            if ui
                .add(button(RichText::new(crate::i18n::t("dialog.cancel"))))
                .clicked()
            {
                choice = Some(UnsavedChoice::Cancel);
            }
        });
    });
    choice
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the dialog in a modal on a `width` × 800 window; returns the contents' size.
    fn measure(width: f32, document: &str) -> Vec2 {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut size = Vec2::ZERO;
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(width, 800.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                egui::Modal::new(egui::Id::new("test")).show(ui.ctx(), |ui| {
                    unsaved_changes(ui, document, "before quitting");
                    size = ui.min_rect().size();
                });
            });
            output.textures_delta.clear();
        }
        size
    }

    #[test]
    fn the_unsaved_dialog_keeps_its_width() {
        let normal = measure(1600.0, "scenes/main.abscene");
        assert!(
            (CONFIRM_WIDTH - 1.0..=CONFIRM_WIDTH + 1.0).contains(&normal.x),
            "{normal:?}"
        );
        // A long path without spaces wraps instead of widening the dialog.
        let long = format!("levels/{}/boss.abscene", "very_long_folder_name".repeat(12));
        let wrapped = measure(1600.0, &long);
        assert!(wrapped.x <= CONFIRM_WIDTH + 1.0, "{wrapped:?}");
        assert!(wrapped.y > normal.y, "the message wraps onto more lines");
        // A narrow window: the dialog fits inside it.
        let narrow = measure(360.0, "scenes/main.abscene");
        assert!(narrow.x <= 360.0 - WINDOW_MARGIN + 1.0, "{narrow:?}");
    }
}
