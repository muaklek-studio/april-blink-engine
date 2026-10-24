//! The Game Config panel: the project's named values (`config.toml`) by group, edited
//! within their rules. While playing, edits are live tweaks (undone on Stop); while
//! editing, they are unsaved changes until Save writes them to the file.

use crate::i18n::{t, tf};
use crate::state::Editor;
use crate::theme;
use ab_engine::game_config::{ConfigEntry, ConfigValue, GameConfig};
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;

#[derive(Default)]
pub struct ConfigPanel {
    filter: String,
    /// The last refused edit.
    error: Option<String>,
}

impl ConfigPanel {
    pub fn show(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        self.show_values(ui, editor);
    }

    /// Writes the edited values to the project's config file; whether it worked.
    pub fn save_to_file(&mut self, editor: &mut Editor) -> bool {
        if let Some(path) = editor.config_path.clone() {
            self.save(editor, &path);
        }
        !editor.config_modified
    }

    fn show_values(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        let Some(path) = editor.config_path.clone() else {
            super::empty_hint(ui, icon::SLIDERS, &t("config.no_file"));
            return;
        };
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text(format!(
                        "{}  {}",
                        icon::MAGNIFYING_GLASS,
                        t("config.filter")
                    ))
                    .desired_width(180.0),
            );
            if ui
                .button(format!("{}  {}", icon::ARROW_CLOCKWISE, t("config.reload")))
                .on_hover_text(tf("config.reload_hint", &[("path", &path.display())]))
                .clicked()
            {
                self.error = editor.load_config();
            }
            let editing = !editor.is_playing();
            let save = ui
                .add_enabled(
                    editing,
                    egui::Button::new(format!("{}  {}", icon::FLOPPY_DISK, t("command.save"))),
                )
                .on_hover_text(t("config.save_hint"))
                .on_disabled_hover_text(t("config.save_playing"));
            if save.clicked() {
                self.save(editor, &path);
            }
        });
        if let Some(error) = &self.error {
            ui.label(RichText::new(format!("{}  {error}", icon::WARNING)).color(theme::WARN));
        }
        ui.separator();

        let Some(mut config) = editor
            .tree()
            .world()
            .resource::<GameConfig>()
            .map(|c| c.clone())
        else {
            ui.label(RichText::new(t("config.unreadable")).color(theme::WARN));
            return;
        };
        let needle = self.filter.trim().to_lowercase();
        let keys: Vec<String> = config
            .entries()
            .filter(|(k, _)| needle.is_empty() || k.to_lowercase().contains(&needle))
            .map(|(k, _)| k.to_owned())
            .collect();
        let mut changed: Option<(String, Result<(), String>)> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut group = "";
                for key in &keys {
                    let (this_group, name) = key.split_once('.').unwrap_or(("", key));
                    if this_group != group {
                        group = this_group;
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(group)
                                .font(theme::bold(13.0))
                                .color(theme::ACCENT),
                        );
                    }
                    let entry = config.entry(key).expect("listed").clone();
                    ui.horizontal(|ui| {
                        let label = ui.add_sized(
                            [150.0, 20.0],
                            egui::Label::new(RichText::new(name).color(theme::TEXT)).truncate(),
                        );
                        if !entry.description.is_empty() {
                            label.on_hover_text(&entry.description);
                        }
                        if let Some(value) = value_editor(ui, key, &entry) {
                            let result = config.set(key, value).map_err(|e| e.to_string());
                            changed = Some((key.clone(), result));
                        }
                        if entry.constant {
                            ui.label(RichText::new(icon::LOCK_SIMPLE).color(theme::TEXT_DIM))
                                .on_hover_text(t("config.constant"));
                        } else if entry.value != entry.default
                            && ui
                                .small_button(icon::ARROW_COUNTER_CLOCKWISE)
                                .on_hover_text(tf("config.reset", &[("value", &entry.default)]))
                                .clicked()
                        {
                            let result = config.reset(key).map_err(|e| e.to_string());
                            changed = Some((key.clone(), result));
                        }
                    });
                }
            });
        if let Some((key, result)) = changed {
            match result {
                Ok(()) => {
                    self.error = None;
                    editor.tree_mut().world_mut().insert_resource(config);
                    if !editor.is_playing() {
                        editor.config_modified = true;
                        editor.info(tf("config.changed", &[("key", &key)]));
                    }
                }
                Err(err) => self.error = Some(err),
            }
        }
    }

    fn save(&mut self, editor: &mut Editor, path: &std::path::Path) {
        let Some(mut config) = editor
            .tree()
            .world()
            .resource::<GameConfig>()
            .map(|c| c.clone())
        else {
            return;
        };
        config.commit();
        match editor.write_project_file(path, config.to_toml()) {
            Ok(()) => {
                editor.tree_mut().world_mut().insert_resource(config);
                editor.config_modified = false;
                editor.info(tf("config.saved", &[("path", &path.display())]));
                self.error = None;
            }
            Err(err) => self.error = Some(format!("{}: {err}", path.display())),
        }
    }
}

/// An editor for one value within its rules; the new value when it was changed.
fn value_editor(ui: &mut egui::Ui, key: &str, entry: &ConfigEntry) -> Option<ConfigValue> {
    ui.add_enabled_ui(!entry.constant, |ui| match &entry.value {
        ConfigValue::Bool(v) => {
            let mut v = *v;
            ui.checkbox(&mut v, "")
                .changed()
                .then_some(ConfigValue::Bool(v))
        }
        ConfigValue::Int(v) => {
            let mut v = *v;
            let range = entry.min.map_or(i64::MIN, |m| m.ceil() as i64)
                ..=entry.max.map_or(i64::MAX, |m| m.floor() as i64);
            ui.add(
                egui::DragValue::new(&mut v)
                    .custom_parser(crate::panels::parse_finite)
                    .range(range)
                    .speed(0.2),
            )
            .changed()
            .then_some(ConfigValue::Int(v))
        }
        ConfigValue::Float(v) => {
            let mut v = *v;
            let range = entry.min.unwrap_or(f64::MIN)..=entry.max.unwrap_or(f64::MAX);
            ui.add(
                egui::DragValue::new(&mut v)
                    .custom_parser(crate::panels::parse_finite)
                    .range(range)
                    .speed(0.01)
                    .max_decimals(4),
            )
            .changed()
            .then_some(ConfigValue::Float(v))
        }
        ConfigValue::Text(v) if !entry.options.is_empty() => {
            let mut chosen = None;
            egui::ComboBox::from_id_salt(("config", key))
                .selected_text(v.as_str())
                .show_ui(ui, |ui| {
                    for option in &entry.options {
                        if ui.selectable_label(option == v, option).clicked() && option != v {
                            chosen = Some(ConfigValue::Text(option.clone()));
                        }
                    }
                });
            chosen
        }
        ConfigValue::Text(v) => {
            let mut v = v.clone();
            ui.add(egui::TextEdit::singleline(&mut v).desired_width(160.0))
                .changed()
                .then_some(ConfigValue::Text(v))
        }
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_shows_the_projects_config_and_play_starts_from_the_file() {
        let dir = std::env::temp_dir().join(format!("ab-config-panel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let project = ab_project::create(&ab_project::NewProject {
            title: "Configured".into(),
            location: dir.clone(),
            new_folder: true,
        })
        .unwrap();
        let mut editor = Editor::new(project.assets.clone(), None, Default::default());
        assert_eq!(editor.config_path, Some(project.root.join("config.toml")));
        let speed = |editor: &Editor| {
            editor
                .tree()
                .world()
                .resource::<GameConfig>()
                .unwrap()
                .f32("Player.MoveSpeed")
                .unwrap()
        };
        assert_eq!(speed(&editor), 5.0);

        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut panel = ConfigPanel::default();
        let mut output = ctx.run_ui(Default::default(), |ui| panel.show(ui, &mut editor));
        output.textures_delta.clear();
        assert!(panel.error.is_none());

        // A tweak, then Play: the file's value again.
        editor
            .tree()
            .world()
            .resource_mut::<GameConfig>()
            .unwrap()
            .set("Player.MoveSpeed", ConfigValue::Float(9.0))
            .unwrap();
        editor.play();
        assert_eq!(speed(&editor), 5.0);
        editor.stop();

        // Saved values survive a reload.
        editor
            .tree()
            .world()
            .resource_mut::<GameConfig>()
            .unwrap()
            .set("Player.MoveSpeed", ConfigValue::Float(7.0))
            .unwrap();
        panel.save(&mut editor, &project.root.join("config.toml"));
        assert!(editor.load_config().is_none());
        assert_eq!(speed(&editor), 7.0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
