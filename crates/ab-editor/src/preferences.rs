//! Edit → Preferences: language, interface font and scale, viewport highlighting and
//! snapping, and the engine sources folder. Changes apply at once and are saved with the
//! editor settings.

use crate::i18n::{t, tf};
use crate::settings::{EditorSettings, UI_SCALES};
use crate::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

/// The fonts, language and zoom in effect, to apply only what changed (kept in the
/// context's memory, so every screen shares it).
#[derive(Debug, Clone, Default, PartialEq)]
struct Applied {
    font: Option<Option<PathBuf>>,
    system_fonts: bool,
    language: String,
    scale: f32,
}

/// Applies the settings' language, fonts and zoom where they differ from `applied`.
/// Returns why the chosen font was not used, if it was not (the built-in one is used).
pub fn apply(ctx: &egui::Context, settings: &EditorSettings) -> Option<String> {
    let id = egui::Id::new("ab-editor.applied-preferences");
    let mut applied: Applied = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let error = apply_changes(ctx, settings, &mut applied);
    ctx.data_mut(|d| d.insert_temp(id, applied));
    error
}

fn apply_changes(
    ctx: &egui::Context,
    settings: &EditorSettings,
    applied: &mut Applied,
) -> Option<String> {
    let mut error = None;
    let font_changed = applied.font.as_ref() != Some(&settings.ui_font)
        || applied.system_fonts != settings.system_fonts;
    if font_changed {
        let custom = settings
            .ui_font
            .as_deref()
            .map(crate::fonts::load_font)
            .transpose()
            .unwrap_or_else(|err| {
                tracing::warn!("{err}");
                error = Some(err);
                None
            });
        let system: &[crate::fonts::LoadedFont] = if settings.system_fonts {
            crate::fonts::system_fallbacks()
        } else {
            &[]
        };
        ctx.set_fonts(crate::fonts::definitions(custom.as_ref(), system));
        applied.font = Some(settings.ui_font.clone());
        applied.system_fonts = settings.system_fonts;
    }
    if applied.language != settings.language {
        if !crate::i18n::set_language(&settings.language) {
            tracing::warn!(language = %settings.language, "unknown editor language");
        }
        applied.language = settings.language.clone();
    }
    if applied.scale != settings.ui_scale {
        ctx.set_zoom_factor(settings.ui_scale);
        applied.scale = settings.ui_scale;
    }
    error
}

#[derive(Default)]
pub struct PreferencesWindow {
    open: bool,
    font_picker: Option<Receiver<Option<PathBuf>>>,
    engine_picker: Option<Receiver<Option<PathBuf>>>,
    /// The last font or engine folder problem.
    pub error: Option<String>,
}

impl PreferencesWindow {
    pub fn show_window(&mut self) {
        self.open = true;
    }

    /// The window, editing `settings` and the viewport's `options` in place.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        settings: &mut EditorSettings,
        options: &mut crate::viewport::ViewOptions,
    ) {
        if let Some(picked) = self.font_picker.as_ref().and_then(|p| p.try_recv().ok()) {
            self.font_picker = None;
            if let Some(path) = picked {
                match crate::fonts::load_font(&path) {
                    Ok(_) => {
                        settings.ui_font = Some(path);
                        self.error = None;
                    }
                    Err(err) => self.error = Some(tf("preferences.font_error", &[("error", &err)])),
                }
            }
        }
        if let Some(picked) = self.engine_picker.as_ref().and_then(|p| p.try_recv().ok()) {
            self.engine_picker = None;
            if let Some(dir) = picked {
                let outcome = crate::loading::choose_engine(&dir);
                match ab_project::engine_root() {
                    Some(root) if outcome.level == crate::loading::Level::Ok => {
                        settings.engine_root = Some(root);
                        self.error = None;
                    }
                    _ => self.error = Some(outcome.detail),
                }
            }
        }
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new(format!("{}  {}", icon::GEAR_SIX, t("preferences.title")))
            .id(egui::Id::new("preferences"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(crate::dialogs::dialog_width(ctx, 460.0))
            .show(ctx, |ui| {
                crate::dialogs::set_dialog_width(ui, 460.0);
                self.contents(ui, settings, options);
            });
        self.open = open;
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut EditorSettings,
        options: &mut crate::viewport::ViewOptions,
    ) {
        let heading = |ui: &mut egui::Ui, text: String| {
            ui.add_space(4.0);
            ui.label(
                RichText::new(text)
                    .font(theme::bold(13.5))
                    .color(egui::Color32::WHITE),
            );
        };
        heading(ui, t("preferences.appearance"));
        egui::Grid::new("preferences_appearance")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(t("preferences.language"));
                let languages = crate::i18n::languages();
                let current = languages
                    .iter()
                    .find(|(code, _)| *code == settings.language)
                    .map_or(settings.language.clone(), |(_, name)| name.clone());
                egui::ComboBox::from_id_salt("language")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (code, name) in &languages {
                            ui.selectable_value(&mut settings.language, code.clone(), name);
                        }
                    })
                    .response
                    .on_hover_text(t("preferences.language_hint"));
                ui.end_row();

                ui.label(t("preferences.ui_font"));
                ui.vertical(|ui| {
                    let name = settings.ui_font.as_ref().map_or_else(
                        || t("preferences.font_default"),
                        |p| {
                            p.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        },
                    );
                    ui.label(RichText::new(name).color(theme::ACCENT));
                    ui.horizontal(|ui| {
                        let choose = egui::Button::new(format!(
                            "{}  {}",
                            icon::TEXT_AA,
                            t("preferences.font_choose")
                        ));
                        if ui.add_enabled(self.font_picker.is_none(), choose).clicked() {
                            self.font_picker = Some(pick_font(ui.ctx()));
                        }
                        if settings.ui_font.is_some()
                            && ui.button(t("preferences.font_reset")).clicked()
                        {
                            settings.ui_font = None;
                            self.error = None;
                        }
                    });
                    ui.add(
                        egui::Label::new(
                            RichText::new(t("preferences.font_hint"))
                                .small()
                                .color(theme::TEXT_DIM),
                        )
                        .wrap(),
                    );
                });
                ui.end_row();

                ui.label("");
                ui.checkbox(&mut settings.system_fonts, t("preferences.system_fonts"));
                ui.end_row();

                ui.label(t("preferences.scale"));
                ui.horizontal(|ui| {
                    // Applied when the slider is let go: zooming while dragging moves it.
                    let mut percent = (settings.ui_scale * 100.0).round();
                    let slider = ui.add(
                        egui::Slider::new(
                            &mut percent,
                            (UI_SCALES.start() * 100.0)..=(UI_SCALES.end() * 100.0),
                        )
                        .step_by(5.0)
                        .suffix(" %"),
                    );
                    if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
                        settings.ui_scale = percent / 100.0;
                    }
                    if ui.small_button("100 %").clicked() {
                        settings.ui_scale = 1.0;
                    }
                });
                ui.end_row();
            });

        heading(ui, t("preferences.viewport"));
        ui.checkbox(
            &mut options.highlight_selection,
            t("menu.highlight_selection"),
        );
        ui.checkbox(&mut options.highlight_hover, t("menu.highlight_hover"));
        ui.collapsing(t("menu.snapping"), |ui| {
            crate::viewport::snap_settings(ui, &mut options.snap);
        });

        heading(ui, t("preferences.engine"));
        ui.horizontal(|ui| {
            let root = ab_project::engine_root();
            let text = root
                .as_ref()
                .map_or_else(|| t("preferences.engine_none"), |r| r.display().to_string());
            ui.add(
                egui::Label::new(RichText::new(text).monospace().color(theme::TEXT_DIM)).truncate(),
            );
            let locate = egui::Button::new(format!(
                "{}  {}",
                icon::FOLDER_OPEN,
                t("preferences.engine_locate")
            ));
            if ui
                .add_enabled(self.engine_picker.is_none(), locate)
                .clicked()
            {
                self.engine_picker = Some(crate::loading::pick_folder(
                    ui.ctx(),
                    "Locate the engine sources",
                ));
            }
        });

        if let Some(error) = &self.error {
            ui.add_space(6.0);
            ui.label(RichText::new(format!("{}  {error}", icon::WARNING)).color(theme::WARN));
        }
    }
}

fn pick_font(ctx: &egui::Context) -> Receiver<Option<PathBuf>> {
    let (send, receive) = std::sync::mpsc::channel();
    let picker = rfd::FileDialog::new()
        .set_title("Choose an interface font")
        .add_filter("Fonts", &["ttf", "otf", "ttc"]);
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = send.send(picker.pick_file());
        ctx.request_repaint();
    });
    receive
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_font_falls_back_to_the_built_in_one_with_a_reason() {
        let ctx = egui::Context::default();
        let settings = EditorSettings {
            ui_font: Some("/nonexistent/font.ttf".into()),
            system_fonts: false,
            ui_scale: 1.25,
            ..Default::default()
        };
        let error = apply(&ctx, &settings);
        assert!(error.is_some_and(|e| e.contains("font.ttf")));
        // The zoom takes effect with the next frame.
        let mut output = ctx.run_ui(Default::default(), |_| {});
        output.textures_delta.clear();
        assert_eq!(ctx.zoom_factor(), 1.25);
        // Applied again with nothing changed: no font reload, no error.
        assert!(apply(&ctx, &settings).is_none());
    }
}
