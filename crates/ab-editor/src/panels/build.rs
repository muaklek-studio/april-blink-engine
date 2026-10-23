//! The Build panel: the pieces to build with (models and prefabs, by folder, with their
//! thumbnails) and tools to arrange what is selected: align, distribute, drop to the floor,
//! copies in a row. Picking a piece starts the build tool in the viewport ([`crate::build`]).

use crate::i18n::t;
use crate::project::AssetKind;
use crate::state::{Align, BuildPiece, Editor};
use crate::theme;
use eframe::egui::{self, RichText, Sense};
use egui_phosphor::regular as icon;

pub struct BuildPanel {
    /// The folder pieces are taken from ("" = every folder).
    folder: String,
    thumbnails: Option<crate::thumbnails::Thumbnails>,
    row_axis: usize,
    row_count: usize,
    row_gap: f32,
}

impl Default for BuildPanel {
    fn default() -> Self {
        Self {
            folder: String::new(),
            thumbnails: None,
            row_axis: 0,
            row_count: 3,
            row_gap: 0.0,
        }
    }
}

/// Whether a file can be built with.
fn is_piece(kind: AssetKind) -> bool {
    matches!(kind, AssetKind::Model | AssetKind::Scene)
}

impl BuildPanel {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        render_state: Option<&eframe::egui_wgpu::RenderState>,
        grid: &mut f32,
    ) {
        let thumbnails = self.thumbnails.get_or_insert_with(|| {
            let gpu = render_state.map(|state| {
                ab_engine::rendering::GpuContext::from_shared(
                    state.adapter.clone(),
                    state.device.clone(),
                    state.queue.clone(),
                )
            });
            crate::thumbnails::Thumbnails::new(editor.project(), gpu)
        });
        if thumbnails.update(ui.ctx()) {
            ui.ctx().request_repaint();
        }
        let editing = !editor.is_playing();
        let pieces: Vec<(String, AssetKind)> = editor
            .assets
            .entries
            .iter()
            .filter(|e| is_piece(e.kind))
            // The open scene cannot be a piece of itself.
            .filter(|e| editor.scene_path.as_deref() != Some(e.path.as_str()))
            .map(|e| (e.path.clone(), e.kind))
            .collect();
        let mut folders: Vec<String> = pieces
            .iter()
            .map(|(p, _)| p.rsplit_once('/').map_or("", |(f, _)| f).to_owned())
            .collect();
        folders.sort();
        folders.dedup();
        // Start in a folder of pieces when the project has one.
        if self.folder.is_empty() {
            if let Some(f) = folders
                .iter()
                .find(|f| f.contains("piece") || f.contains("prefab"))
            {
                self.folder = f.clone();
            }
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new(t("build.pieces")).font(theme::bold(13.0)));
            egui::ComboBox::from_id_salt("build_folder")
                .selected_text(if self.folder.is_empty() {
                    t("build.all_folders")
                } else {
                    self.folder.clone()
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.folder, String::new(), t("build.all_folders"));
                    for folder in &folders {
                        let label = if folder.is_empty() {
                            "/"
                        } else {
                            folder.as_str()
                        };
                        ui.selectable_value(&mut self.folder, folder.clone(), label);
                    }
                });
            ui.label(RichText::new(t("build.grid")).color(theme::TEXT_DIM));
            ui.add(
                egui::DragValue::new(grid)
                    .range(0.01..=100.0)
                    .speed(0.05)
                    .suffix(" m"),
            )
            .on_hover_text("Where pieces snap to: the viewport's move snap step (Edit → Snapping)");
        });
        let shown: Vec<&(String, AssetKind)> = pieces
            .iter()
            .filter(|(p, _)| {
                self.folder.is_empty() || p.rsplit_once('/').map_or("", |(f, _)| f) == self.folder
            })
            .collect();
        if shown.is_empty() {
            super::empty_hint(ui, icon::CUBE, &t("build.no_pieces"));
        }
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - 64.0).max(100.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                    for (path, kind) in &shown {
                        let active = editor.build.as_ref().is_some_and(|b| &b.path == path);
                        let size = egui::vec2(78.0, 92.0);
                        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                        let fill = if active {
                            theme::ACCENT_DIM
                        } else if response.hovered() {
                            theme::BG_3
                        } else {
                            theme::BG_2
                        };
                        ui.painter().rect_filled(rect, 6.0, fill);
                        let picture = egui::Rect::from_center_size(
                            rect.center_top() + egui::vec2(0.0, 36.0),
                            egui::Vec2::splat(62.0),
                        );
                        match thumbnails
                            .get(ui.ctx(), path, *kind)
                            .and_then(|t| t.texture.clone())
                        {
                            Some(texture) => {
                                ui.painter().image(
                                    texture.id(),
                                    picture,
                                    egui::Rect::from_min_max(
                                        egui::Pos2::ZERO,
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::WHITE,
                                );
                            }
                            None => {
                                ui.painter().text(
                                    picture.center(),
                                    egui::Align2::CENTER_CENTER,
                                    kind.icon(),
                                    egui::FontId::proportional(24.0),
                                    theme::ACCENT,
                                );
                            }
                        }
                        let name = path.rsplit('/').next().unwrap_or(path);
                        let galley = ui.painter().layout(
                            name.to_owned(),
                            egui::FontId::proportional(11.0),
                            theme::TEXT,
                            size.x - 6.0,
                        );
                        ui.painter().with_clip_rect(rect.shrink(2.0)).galley(
                            egui::pos2(
                                rect.center().x - galley.size().x / 2.0,
                                picture.bottom() + 3.0,
                            ),
                            galley,
                            theme::TEXT,
                        );
                        if response.on_hover_text(path.as_str()).clicked() && editing {
                            chosen = Some(path.clone());
                        }
                    }
                });
            });
        if let Some(path) = chosen {
            editor.build = match &editor.build {
                Some(b) if b.path == path => None,
                _ => Some(BuildPiece { path, turns: 0 }),
            };
        }
        ui.horizontal(|ui| {
            let building = editor.build.is_some();
            if ui
                .add_enabled(
                    building,
                    egui::Button::new(format!("{}  {}", icon::ARROW_CLOCKWISE, t("build.turn"))),
                )
                .clicked()
            {
                if let Some(b) = &mut editor.build {
                    b.turns = (b.turns + 1) % 4;
                }
            }
            if ui
                .add_enabled(
                    building,
                    egui::Button::new(format!("{}  {}", icon::CHECK, t("build.stop"))),
                )
                .clicked()
            {
                editor.build = None;
            }
        });

        egui::CollapsingHeader::new(RichText::new(t("build.arrange")).font(theme::bold(13.0)))
            .id_salt("arrange")
            .show(ui, |ui| self.arrange(ui, editor));
    }

    fn arrange(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        let can = editor.selected.len() > 1 && !editor.is_playing();
        egui::Grid::new("arrange_grid")
            .num_columns(4)
            .spacing([6.0, 4.0])
            .show(ui, |ui| {
                for (axis, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                    ui.label(format!("{} {name}", t("build.align")));
                    for (align, glyph, tip) in [
                        (Align::Min, icon::ALIGN_LEFT, "low sides"),
                        (Align::Center, icon::ALIGN_CENTER_HORIZONTAL, "centres"),
                        (Align::Max, icon::ALIGN_RIGHT, "high sides"),
                    ] {
                        if ui
                            .add_enabled(can, egui::Button::new(glyph))
                            .on_hover_text(format!(
                                "Line up the {tip} on {name} with the active object"
                            ))
                            .clicked()
                        {
                            editor.align_selected(axis, align);
                        }
                    }
                    ui.end_row();
                }
            });
        ui.horizontal(|ui| {
            for (axis, name) in [(0, "X"), (2, "Z")] {
                if ui
                    .add_enabled(
                        editor.selected.len() > 2,
                        egui::Button::new(format!("{} {name}", t("build.distribute"))),
                    )
                    .clicked()
                {
                    editor.distribute_selected(axis);
                }
            }
            if ui
                .add_enabled(
                    !editor.selected.is_empty(),
                    egui::Button::new(format!("{}  {}", icon::ARROW_LINE_DOWN, t("build.drop"))),
                )
                .clicked()
            {
                editor.drop_selected();
            }
        });
        ui.horizontal(|ui| {
            ui.label(t("build.row"));
            egui::ComboBox::from_id_salt("row_axis")
                .width(40.0)
                .selected_text(["X", "Y", "Z"][self.row_axis])
                .show_ui(ui, |ui| {
                    for (axis, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                        ui.selectable_value(&mut self.row_axis, axis, name);
                    }
                });
            ui.add(
                egui::DragValue::new(&mut self.row_count)
                    .range(1..=100)
                    .prefix("× "),
            );
            ui.add(
                egui::DragValue::new(&mut self.row_gap)
                    .range(-10.0..=100.0)
                    .speed(0.05)
                    .prefix(format!("{} ", t("build.gap")))
                    .suffix(" m"),
            );
            if ui
                .add_enabled(
                    !editor.selected.is_empty() && !editor.is_playing(),
                    egui::Button::new(t("build.make_row")),
                )
                .clicked()
            {
                editor.duplicate_in_row(self.row_axis, self.row_count, self.row_gap);
            }
        });
    }
}
