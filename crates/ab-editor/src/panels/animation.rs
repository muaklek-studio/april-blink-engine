//! The Animation editor: a timeline for an `AnimationPlayer`.
//!
//! Pick a player (the selected one, or one inside the selected model) and a clip; play,
//! pause, scrub. Clips made in the editor (`anims/*.toml`) are edited here: select a node,
//! move it into place with the gizmo, and press Key to record its position, rotation or scale
//! at the playhead; drag keys along the timeline, change a track's interpolation, delete keys
//! and tracks, then Save. Clips imported with a model (glTF) can be previewed, not edited.
//!
//! Previewing poses the scene without changing it: the animated nodes return to where they
//! were when the preview stops, the player changes, or the game is played.

use super::section;
use crate::state::Editor;
use crate::theme;
use ab_engine::animation::{apply_pose, AnimationLibrary, AnimationPlayer};
use ab_engine::animation_file::{rotation_to_degrees, ClipFile, TrackInterpolation, TrackProperty};
use ab_engine::core::Transform3D;
use ab_engine::scene::NodeHandle;
use ab_engine::scene_file::{install_clips, relative_path, AnimationDesc, NodeDesc, SceneOrigin};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use egui_phosphor::regular as icon;
use std::collections::BTreeSet;

const ROW: f32 = 22.0;
const NAMES_WIDTH: f32 = 230.0;
const RULER: f32 = 20.0;

/// A preview in progress: where the animated nodes were before it.
struct Preview {
    player: NodeHandle,
    rest: Vec<(NodeHandle, Transform3D)>,
}

pub struct AnimationPanel {
    player: Option<NodeHandle>,
    clip: Option<String>,
    /// Clip files of the player being edited, by path, and which have unsaved changes.
    files: Vec<(String, ClipFile)>,
    dirty: BTreeSet<String>,
    /// The player the files were read for.
    files_for: Option<NodeHandle>,
    time: f32,
    playing: bool,
    preview: Option<Preview>,
    selected_key: Option<(usize, usize)>,
    /// A key is being dragged (the playhead does not follow the pointer then).
    dragging_key: bool,
    /// Timeline zoom.
    pixels_per_second: f32,
    /// What Key records.
    key_position: bool,
    key_rotation: bool,
    key_scale: bool,
    new_clip: Option<String>,
}

impl Default for AnimationPanel {
    fn default() -> Self {
        Self {
            player: None,
            clip: None,
            files: Vec::new(),
            dirty: BTreeSet::new(),
            files_for: None,
            time: 0.0,
            playing: false,
            preview: None,
            selected_key: None,
            dragging_key: false,
            pixels_per_second: 160.0,
            key_position: true,
            key_rotation: true,
            key_scale: false,
            new_clip: None,
        }
    }
}

/// Every AnimationPlayer in the scene, with a label.
fn players(editor: &Editor) -> Vec<(NodeHandle, String)> {
    let tree = editor.tree();
    tree.subtree(editor.scene_root)
        .into_iter()
        .filter(|&n| tree.component::<AnimationPlayer>(n).is_some())
        .map(|n| {
            let owner = editor.instance_owner(n).filter(|&o| o != n);
            let name = owner
                .or(tree
                    .parent(n)
                    .filter(|_| tree.name(n).is_some_and(|x| &*x == "AnimationPlayer")))
                .unwrap_or(n);
            let label = editor.relative_path(name).unwrap_or_default();
            (n, label)
        })
        .collect()
}

impl AnimationPanel {
    /// The player for the selection: the selected node's, one below it, or one above it.
    fn player_for_selection(&self, editor: &Editor) -> Option<NodeHandle> {
        let tree = editor.tree();
        let node = editor.selection?;
        let has = |n: NodeHandle| tree.component::<AnimationPlayer>(n).is_some();
        tree.subtree(node)
            .into_iter()
            .find(|&n| has(n))
            .or_else(|| {
                std::iter::successors(tree.parent(node), |&n| tree.parent(n))
                    .find(|&n| tree.children(n).into_iter().any(has))
                    .and_then(|n| tree.children(n).into_iter().find(|&c| has(c)))
            })
    }

    pub fn show(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        if editor.is_playing() {
            // The game plays the animations; previews end.
            self.end_preview(editor);
        }
        let wanted = self
            .player_for_selection(editor)
            .or(self.player)
            .filter(|&p| editor.tree().component::<AnimationPlayer>(p).is_some());
        if wanted != self.player {
            self.end_preview(editor);
            self.player = wanted;
            self.clip = None;
            self.time = 0.0;
            self.selected_key = None;
        }
        let Some(player) = self.player else {
            self.empty(ui, editor);
            return;
        };
        self.load_files(editor, player);
        if self.clip.is_none() {
            self.clip = editor
                .tree()
                .component::<AnimationPlayer>(player)
                .and_then(|p| {
                    p.current_name()
                        .map(str::to_owned)
                        .or_else(|| p.clips.first().map(|c| c.0.clone()))
                });
        }
        self.toolbar(ui, editor, player);
        ui.separator();
        self.advance(ui, editor, player);
        self.timeline(ui, editor, player);
    }

    fn empty(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        let all = players(editor);
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(icon::FILM_STRIP)
                    .size(28.0)
                    .color(theme::TEXT_DIM),
            );
            ui.label(
                RichText::new(
                    "Select an AnimationPlayer, or a model with animations. \
                     Create → Node → AnimationPlayer animates the nodes beside it.",
                )
                .color(theme::TEXT_DIM),
            );
            if !all.is_empty() {
                ui.add_space(4.0);
                for (node, label) in all {
                    if ui
                        .button(format!("{}  {label}", icon::FILM_STRIP))
                        .clicked()
                    {
                        editor.select(Some(node));
                    }
                }
            }
        });
    }

    /// Reads the player's clip files (once per player).
    fn load_files(&mut self, editor: &Editor, player: NodeHandle) {
        if self.files_for == Some(player) {
            return;
        }
        self.files_for = Some(player);
        self.files.clear();
        self.dirty.clear();
        for path in desc_of(editor, player).map(|d| d.clips).unwrap_or_default() {
            let text = std::fs::read_to_string(editor.project().join(&path)).unwrap_or_default();
            if let Ok(file) = ClipFile::from_toml(&text) {
                self.files.push((path, file));
            }
        }
    }

    /// The editable file of the current clip.
    fn file_index(&self) -> Option<usize> {
        let clip = self.clip.as_deref()?;
        self.files.iter().position(|(_, f)| f.name == clip)
    }

    fn duration(&self, editor: &Editor, player: NodeHandle) -> f32 {
        if let Some(i) = self.file_index() {
            return self.files[i].1.duration().max(0.1);
        }
        let tree = editor.tree();
        let library = tree.world().resource::<AnimationLibrary>();
        tree.component::<AnimationPlayer>(player)
            .and_then(|p| {
                let clip = self.clip.as_deref()?;
                let handle = p.clip_named(clip)?;
                library.as_ref()?.get(handle).map(|c| c.duration)
            })
            .unwrap_or(1.0)
            .max(0.1)
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, editor: &mut Editor, player: NodeHandle) {
        let clips: Vec<String> = editor
            .tree()
            .component::<AnimationPlayer>(player)
            .map(|p| p.clip_names().map(str::to_owned).collect())
            .unwrap_or_default();
        let editable_player = desc_of(editor, player).is_some() && !editor.is_playing();
        let duration = self.duration(editor, player);
        ui.horizontal_wrapped(|ui| {
            let label = editor
                .tree()
                .name(player)
                .map(|n| n.to_owned())
                .unwrap_or_default();
            ui.label(RichText::new(format!("{}  {label}", icon::FILM_STRIP)).strong());
            egui::ComboBox::from_id_salt("anim_clip")
                .selected_text(self.clip.clone().unwrap_or_else(|| "No clip".into()))
                .show_ui(ui, |ui| {
                    for clip in &clips {
                        if ui
                            .selectable_label(self.clip.as_ref() == Some(clip), clip)
                            .clicked()
                        {
                            self.clip = Some(clip.clone());
                            self.time = 0.0;
                            self.selected_key = None;
                        }
                    }
                });
            if editable_player && ui.button(format!("{}  New clip", icon::PLUS)).clicked() {
                self.new_clip = Some(format!("Clip{}", clips.len() + 1));
            }
            ui.separator();
            if ui
                .button(icon::SKIP_BACK)
                .on_hover_text("To the start")
                .clicked()
            {
                self.time = 0.0;
                self.pose(editor, player);
            }
            let (glyph, tip) = if self.playing {
                (icon::PAUSE, "Pause")
            } else {
                (icon::PLAY, "Play the clip")
            };
            if ui
                .add_enabled(!editor.is_playing(), egui::Button::new(glyph))
                .on_hover_text(tip)
                .clicked()
            {
                self.playing = !self.playing;
                if self.playing {
                    self.begin_preview(editor, player);
                }
            }
            if ui
                .add_enabled(self.preview.is_some(), egui::Button::new(icon::STOP))
                .on_hover_text("Stop: the nodes return to their places")
                .clicked()
            {
                self.end_preview(editor);
            }
            ui.label(RichText::new(format!("{:.2} / {duration:.2} s", self.time)).monospace());
            if let Some(i) = self.file_index().filter(|_| editable_player) {
                ui.separator();
                let mut length = duration;
                ui.label("Length");
                if ui
                    .add(
                        egui::DragValue::new(&mut length)
                            .speed(0.02)
                            .range(0.1..=600.0)
                            .suffix(" s"),
                    )
                    .changed()
                {
                    self.files[i].1.length = Some(length);
                    self.changed(editor, player, i);
                }
                ui.separator();
                ui.toggle_value(&mut self.key_position, "Position");
                ui.toggle_value(&mut self.key_rotation, "Rotation");
                ui.toggle_value(&mut self.key_scale, "Scale");
                let can_key = editor
                    .selection
                    .is_some_and(|n| key_path(editor, player, n).is_some());
                if ui
                    .add_enabled(
                        can_key,
                        egui::Button::new(
                            RichText::new(format!("{}  Key", icon::DIAMOND)).strong(),
                        )
                        .fill(theme::ACCENT_DIM),
                    )
                    .on_hover_text("Record the selected node's transform at the playhead")
                    .on_disabled_hover_text(
                        "Select a node beside the AnimationPlayer (or below them) to key it",
                    )
                    .clicked()
                {
                    self.key_selected(editor, player, i);
                }
                let dirty = self.dirty.contains(&self.files[i].0);
                if ui
                    .add_enabled(
                        dirty,
                        egui::Button::new(format!("{}  Save", icon::FLOPPY_DISK)),
                    )
                    .clicked()
                {
                    self.save(editor, i);
                }
            } else if !clips.is_empty() {
                ui.label(
                    RichText::new("Imported clip: preview only")
                        .small()
                        .color(theme::TEXT_DIM),
                );
            }
        });
        self.new_clip_row(ui, editor, player);
    }

    fn new_clip_row(&mut self, ui: &mut egui::Ui, editor: &mut Editor, player: NodeHandle) {
        let Some(mut name) = self.new_clip.take() else {
            return;
        };
        let mut create = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.label("Name");
            let edit = ui.add(egui::TextEdit::singleline(&mut name).desired_width(140.0));
            edit.request_focus();
            create = ui.button("Create").clicked()
                || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            cancel = ui.button("Cancel").clicked();
        });
        if !create {
            if !cancel {
                self.new_clip = Some(name);
            }
            return;
        }
        let name: String = name
            .trim()
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() || self.files.iter().any(|(_, f)| f.name == name) {
            editor.fail("Choose another name for the clip");
            return;
        }
        let path = format!("anims/{}.toml", name.to_lowercase());
        let file = ClipFile {
            name: name.clone(),
            length: Some(2.0),
            tracks: Vec::new(),
        };
        let full = editor.project().join(&path);
        let written = full
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&full, file.to_toml().unwrap_or_default()));
        if let Err(err) = written {
            editor.fail(format!("cannot write {path}: {err}"));
            return;
        }
        editor.assets.request_rescan();
        // Into the player's list (the scene changes: undoable).
        if let Some(old) = editor.describe_node(player) {
            let mut new = old.clone();
            new.animation
                .get_or_insert_with(AnimationDesc::default)
                .clips
                .push(path.clone());
            self.end_preview(editor);
            editor.apply_desc(player, &old, new);
        }
        self.files_for = None;
        self.player = None;
        self.clip = Some(name.clone());
        editor.info(format!(
            "Created clip {name} ({path}): select a node and press Key"
        ));
    }

    /// Advances playback, and poses the scene while previewing.
    fn advance(&mut self, ui: &egui::Ui, editor: &mut Editor, player: NodeHandle) {
        if editor.is_playing() {
            // Show where the game's player is.
            if let Some(p) = editor.tree().component::<AnimationPlayer>(player) {
                self.time = p.time;
                self.clip = p.current_name().map(str::to_owned).or(self.clip.take());
            }
            return;
        }
        if self.playing {
            let duration = self.duration(editor, player);
            let dt = ui.input(|i| i.stable_dt).min(0.1);
            self.time = (self.time + dt) % duration;
            ui.ctx().request_repaint();
            self.pose(editor, player);
        }
    }

    fn begin_preview(&mut self, editor: &Editor, player: NodeHandle) {
        if self.preview.as_ref().is_some_and(|p| p.player == player) {
            return;
        }
        let tree = editor.tree();
        let rest = tree
            .component::<AnimationPlayer>(player)
            .map(|p| p.targets.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|n| Some((n, tree.transform(n)?)))
            .collect();
        self.preview = Some(Preview { player, rest });
    }

    /// Puts the animated nodes back where they were before the preview.
    fn end_preview(&mut self, editor: &mut Editor) {
        self.playing = false;
        let Some(preview) = self.preview.take() else {
            return;
        };
        if editor.is_playing() {
            return;
        }
        let tree = editor.tree_mut();
        for (node, transform) in preview.rest {
            if tree.is_valid(node) {
                let _ = tree.set_transform(node, transform);
            }
        }
        tree.update_transforms();
        if let Some(mut p) = tree.component_mut::<AnimationPlayer>(preview.player) {
            p.playing = false;
        }
    }

    /// Shows the clip at the playhead.
    fn pose(&mut self, editor: &mut Editor, player: NodeHandle) {
        if editor.is_playing() {
            return;
        }
        self.begin_preview(editor, player);
        let Some(clip) = self.clip.clone() else {
            return;
        };
        let tree = editor.tree_mut();
        if let Some(mut p) = tree.component_mut::<AnimationPlayer>(player) {
            if let Some(handle) = p.clip_named(&clip) {
                p.current = Some(handle);
                p.time = self.time;
                p.blend = None;
                p.fade = None;
            }
        }
        apply_pose(tree.world(), player.entity());
        tree.update_transforms();
    }

    /// Clip file `i` changed: rebuild the player's clips from the files being edited.
    fn changed(&mut self, editor: &mut Editor, player: NodeHandle, i: usize) {
        self.dirty.insert(self.files[i].0.clone());
        let desc = desc_of(editor, player).unwrap_or_default();
        let files: Vec<ClipFile> = self.files.iter().map(|(_, f)| f.clone()).collect();
        // The preview's rest poses stay valid: the targets are the same nodes, or new ones.
        let rest_targets: Vec<NodeHandle> = self
            .preview
            .as_ref()
            .map(|p| p.rest.iter().map(|r| r.0).collect())
            .unwrap_or_default();
        let tree = editor.tree_mut();
        let current = tree
            .component::<AnimationPlayer>(player)
            .map(|p| (p.playing, p.time));
        if let Err(err) = install_clips(
            tree,
            player,
            &files,
            &AnimationDesc {
                autoplay: None,
                ..desc
            },
        ) {
            editor.fail(err);
            return;
        }
        if let Some((playing, time)) = current {
            if let Some(mut p) = editor.tree_mut().component_mut::<AnimationPlayer>(player) {
                p.playing = playing;
                p.time = time;
            }
        }
        // Nodes keyed for the first time join the preview with their current pose.
        if let Some(preview) = &mut self.preview {
            let tree = editor.tree();
            let targets = tree
                .component::<AnimationPlayer>(player)
                .map(|p| p.targets.clone())
                .unwrap_or_default();
            for node in targets {
                if !rest_targets.contains(&node) {
                    if let Some(t) = tree.transform(node) {
                        preview.rest.push((node, t));
                    }
                }
            }
        }
    }

    fn key_selected(&mut self, editor: &mut Editor, player: NodeHandle, i: usize) {
        let Some(node) = editor.selection else {
            return;
        };
        let Some(path) = key_path(editor, player, node) else {
            return;
        };
        let Some(transform) = editor.tree().transform(node) else {
            return;
        };
        let time = (self.time * 1000.0).round() / 1000.0;
        let file = &mut self.files[i].1;
        if self.key_position {
            file.track_mut(&path, TrackProperty::Position)
                .set_key(time, transform.translation.to_array());
        }
        if self.key_rotation {
            file.track_mut(&path, TrackProperty::Rotation)
                .set_key(time, rotation_to_degrees(transform.rotation));
        }
        if self.key_scale {
            file.track_mut(&path, TrackProperty::Scale)
                .set_key(time, transform.scale.to_array());
        }
        self.begin_preview(editor, player);
        self.changed(editor, player, i);
        editor.info(format!("Keyed {path} at {time:.2} s"));
    }

    fn save(&mut self, editor: &mut Editor, i: usize) {
        let (path, file) = &self.files[i];
        let full = editor.project().join(path);
        match file
            .to_toml()
            .map_err(|e| e.to_string())
            .and_then(|text| std::fs::write(&full, text).map_err(|e| e.to_string()))
        {
            Ok(()) => {
                editor.info(format!("Saved {path}"));
                self.dirty.remove(path);
            }
            Err(err) => editor.fail(format!("cannot save {path}: {err}")),
        }
    }

    fn timeline(&mut self, ui: &mut egui::Ui, editor: &mut Editor, player: NodeHandle) {
        let duration = self.duration(editor, player);
        let editable = self.file_index().is_some()
            && desc_of(editor, player).is_some()
            && !editor.is_playing();
        // Rows: (label, key times, track index in the file when editable).
        let rows = self.rows(editor, player);
        let height = RULER + ROW * rows.len().max(1) as f32 + 8.0;
        egui::ScrollArea::both()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let width = NAMES_WIDTH + duration * self.pixels_per_second + 40.0;
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(width.max(ui.available_width()), height),
                    Sense::click_and_drag(),
                );
                let painter = ui.painter_at(rect);
                let pps = self.pixels_per_second;
                let x_of = |t: f32| rect.left() + NAMES_WIDTH + t * pps;
                let t_of = |x: f32| ((x - rect.left() - NAMES_WIDTH) / pps).clamp(0.0, duration);
                // Ruler.
                painter.rect_filled(
                    Rect::from_min_size(rect.min, egui::vec2(rect.width(), RULER)),
                    0.0,
                    theme::BG_2,
                );
                let step = if pps > 120.0 { 0.25 } else { 1.0 };
                let mut t = 0.0;
                while t <= duration + 1e-4 {
                    let x = x_of(t);
                    let whole = (t - t.round()).abs() < 1e-4;
                    painter.line_segment(
                        [
                            Pos2::new(x, rect.top() + if whole { 4.0 } else { 12.0 }),
                            Pos2::new(x, rect.top() + RULER),
                        ],
                        Stroke::new(1.0_f32, theme::TEXT_DIM),
                    );
                    if whole {
                        painter.text(
                            Pos2::new(x + 3.0, rect.top() + 2.0),
                            egui::Align2::LEFT_TOP,
                            format!("{t:.0}s"),
                            egui::FontId::proportional(10.0),
                            theme::TEXT_DIM,
                        );
                    }
                    t += step;
                }
                // End of the clip.
                painter.line_segment(
                    [
                        Pos2::new(x_of(duration), rect.top()),
                        Pos2::new(x_of(duration), rect.bottom()),
                    ],
                    Stroke::new(1.0_f32, theme::WARN.gamma_multiply(0.6)),
                );
                // Rows.
                let mut row_actions: Vec<RowAction> = Vec::new();
                for (r, row) in rows.iter().enumerate() {
                    let top = rect.top() + RULER + r as f32 * ROW;
                    let row_rect = Rect::from_min_size(
                        Pos2::new(rect.left(), top),
                        egui::vec2(rect.width(), ROW),
                    );
                    if r % 2 == 1 {
                        painter.rect_filled(row_rect, 0.0, theme::BG_1.gamma_multiply(0.6));
                    }
                    painter.text(
                        Pos2::new(rect.left() + 6.0, top + ROW / 2.0),
                        egui::Align2::LEFT_CENTER,
                        &row.label,
                        egui::FontId::proportional(12.0),
                        ui.visuals().text_color(),
                    );
                    for (k, &time) in row.times.iter().enumerate() {
                        let center = Pos2::new(x_of(time), top + ROW / 2.0);
                        let selected = row.track.is_some_and(|t| self.selected_key == Some((t, k)));
                        let color = if selected {
                            Color32::WHITE
                        } else {
                            theme::ACCENT
                        };
                        let s = 5.0;
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                center + Vec2::new(0.0, -s),
                                center + Vec2::new(s, 0.0),
                                center + Vec2::new(0.0, s),
                                center + Vec2::new(-s, 0.0),
                            ],
                            color,
                            Stroke::new(1.0_f32, theme::BG_0),
                        ));
                        if let Some(track) = row.track.filter(|_| editable) {
                            let id = ui.id().with(("anim_key", track, k));
                            let hit = ui.interact(
                                Rect::from_center_size(center, Vec2::splat(12.0)),
                                id,
                                Sense::click_and_drag(),
                            );
                            if hit.clicked() || hit.drag_started() {
                                row_actions.push(RowAction::Select(track, k));
                                self.dragging_key = true;
                            }
                            if hit.dragged() {
                                if let Some(p) = hit.interact_pointer_pos() {
                                    row_actions.push(RowAction::Move(track, k, t_of(p.x)));
                                }
                            }
                            if hit.drag_stopped() {
                                self.dragging_key = false;
                            }
                        }
                    }
                    if let Some(track) = row.track.filter(|_| editable) {
                        // Interpolation and delete, at the end of the name column.
                        let x = rect.left() + NAMES_WIDTH - 46.0;
                        let menu = Rect::from_min_size(
                            Pos2::new(x, top + 2.0),
                            egui::vec2(40.0, ROW - 4.0),
                        );
                        ui.scope_builder(egui::UiBuilder::new().max_rect(menu), |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                ui.menu_button(RichText::new(icon::WAVE_SINE).small(), |ui| {
                                    for (mode, name) in [
                                        (TrackInterpolation::Linear, "Linear"),
                                        (TrackInterpolation::Smooth, "Smooth"),
                                        (TrackInterpolation::Step, "Step"),
                                    ] {
                                        if ui
                                            .selectable_label(row.interpolation == Some(mode), name)
                                            .clicked()
                                        {
                                            row_actions.push(RowAction::Interpolation(track, mode));
                                            ui.close();
                                        }
                                    }
                                });
                                if ui
                                    .small_button(icon::X)
                                    .on_hover_text("Delete this track")
                                    .clicked()
                                {
                                    row_actions.push(RowAction::DeleteTrack(track));
                                }
                            });
                        });
                    }
                }
                if rows.is_empty() {
                    painter.text(
                        Pos2::new(rect.left() + 8.0, rect.top() + RULER + ROW / 2.0),
                        egui::Align2::LEFT_CENTER,
                        if editable {
                            "No tracks yet: select a node, move it, press Key."
                        } else {
                            "No tracks"
                        },
                        egui::FontId::proportional(12.0),
                        theme::TEXT_DIM,
                    );
                }
                // Playhead: click or drag on the ruler or empty timeline to scrub.
                if (response.clicked() || response.dragged()) && !self.dragging_key {
                    if let Some(p) = response
                        .interact_pointer_pos()
                        .filter(|p| p.x > rect.left() + NAMES_WIDTH)
                    {
                        self.time = t_of(p.x);
                        self.playing = false;
                        self.selected_key = None;
                        self.pose(editor, player);
                    }
                }
                let x = x_of(self.time);
                painter.line_segment(
                    [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                    Stroke::new(2.0_f32, theme::PLAY),
                );
                // Zoom with Ctrl + wheel.
                if response.hovered() {
                    let (zoom, ctrl) = ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.command));
                    if ctrl && zoom != 0.0 {
                        self.pixels_per_second =
                            (self.pixels_per_second * (1.0 + zoom * 0.002)).clamp(20.0, 1200.0);
                    }
                }
                self.apply(editor, player, row_actions);
            });
        // Delete the selected key.
        if editable
            && self.selected_key.is_some()
            && ui.input(|i| i.key_pressed(egui::Key::Delete))
            && !ui.ctx().egui_wants_keyboard_input()
        {
            if let (Some(i), Some((track, key))) = (self.file_index(), self.selected_key.take()) {
                let keys = &mut self.files[i].1.tracks[track].keys;
                if key < keys.len() {
                    keys.remove(key);
                }
                self.changed(editor, player, i);
                self.pose(editor, player);
            }
        }
        section(ui, "anim_help", icon::INFO, "How to animate", |ui| {
            ui.label(
                RichText::new(
                    "Create → Node → AnimationPlayer next to the nodes to animate, New clip, then: \
                     move the playhead, select a node, move it with the gizmo, Key. Drag keys to retime them, \
                     Delete removes the selected one, Ctrl + wheel zooms. The nodes go back to their places \
                     when you press Stop. Scripts play clips with tree.animation(player).play(\"Clip\").",
                )
                .small()
                .color(theme::TEXT_DIM),
            );
        });
    }

    fn apply(&mut self, editor: &mut Editor, player: NodeHandle, actions: Vec<RowAction>) {
        let Some(i) = self.file_index() else {
            return;
        };
        let mut changed = false;
        for action in actions {
            let tracks = &mut self.files[i].1.tracks;
            match action {
                RowAction::Select(track, key) => {
                    self.selected_key = Some((track, key));
                    if let Some(k) = tracks.get(track).and_then(|t| t.keys.get(key)) {
                        self.time = k.0;
                    }
                }
                RowAction::Move(track, key, time) => {
                    if let Some(t) = tracks.get_mut(track) {
                        if key < t.keys.len() {
                            let time = (time * 100.0).round() / 100.0;
                            t.keys[key].0 = time;
                            let value = t.keys[key];
                            t.sort();
                            let moved = t.keys.iter().position(|k| *k == value).unwrap_or(key);
                            self.selected_key = Some((track, moved));
                            self.time = time;
                            changed = true;
                        }
                    }
                }
                RowAction::Interpolation(track, mode) => {
                    if let Some(t) = tracks.get_mut(track) {
                        t.interpolation = mode;
                        changed = true;
                    }
                }
                RowAction::DeleteTrack(track) => {
                    if track < tracks.len() {
                        tracks.remove(track);
                        self.selected_key = None;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.changed(editor, player, i);
            self.pose(editor, player);
        }
    }

    /// The timeline's rows for the current clip.
    fn rows(&self, editor: &Editor, player: NodeHandle) -> Vec<Row> {
        if let Some(i) = self.file_index() {
            return self.files[i]
                .1
                .tracks
                .iter()
                .enumerate()
                .map(|(t, track)| Row {
                    label: format!("{} · {}", track.target, track.property.name()),
                    times: track.keys.iter().map(|k| k.0).collect(),
                    track: Some(t),
                    interpolation: Some(track.interpolation),
                })
                .collect();
        }
        // An imported clip: its channels.
        let tree = editor.tree();
        let Some(p) = tree.component::<AnimationPlayer>(player) else {
            return Vec::new();
        };
        let library = tree.world().resource::<AnimationLibrary>();
        let Some(clip) = self
            .clip
            .as_deref()
            .and_then(|c| p.clip_named(c))
            .and_then(|h| library.as_ref()?.get(h).cloned())
        else {
            return Vec::new();
        };
        clip.channels
            .iter()
            .map(|c| {
                let target = p
                    .targets
                    .get(c.target)
                    .and_then(|&n| tree.name(n).map(|x| x.to_owned()))
                    .unwrap_or_default();
                let property = match c.keyframes.property() {
                    ab_engine::animation::Property::Translation => "position",
                    ab_engine::animation::Property::Rotation => "rotation",
                    ab_engine::animation::Property::Scale => "scale",
                };
                Row {
                    label: format!("{target} · {property}"),
                    times: c.times.clone(),
                    track: None,
                    interpolation: None,
                }
            })
            .collect()
    }
}

struct Row {
    label: String,
    times: Vec<f32>,
    /// Index in the clip file (editable clips).
    track: Option<usize>,
    interpolation: Option<TrackInterpolation>,
}

enum RowAction {
    Select(usize, usize),
    Move(usize, usize, f32),
    Interpolation(usize, TrackInterpolation),
    DeleteTrack(usize),
}

/// The player's scene-file description (players made in the editor; `None` for imported).
fn desc_of(editor: &Editor, player: NodeHandle) -> Option<AnimationDesc> {
    if !editor.is_editable(player) {
        return None;
    }
    editor
        .tree()
        .component::<SceneOrigin>(player)
        .and_then(|o| o.animation.clone())
}

/// `node`'s path for a track: relative to the player's parent, which it must be or be below.
fn key_path(editor: &Editor, player: NodeHandle, node: NodeHandle) -> Option<String> {
    let tree = editor.tree();
    let base = tree.parent(player)?;
    if node == player || !tree.is_node3d(node) {
        return None;
    }
    if node == base {
        return Some(".".into());
    }
    tree.is_ancestor_of(base, node)
        .then(|| relative_path(tree, base, node))
        .flatten()
}

/// A new AnimationPlayer node (Create menu).
pub fn new_player() -> NodeDesc {
    NodeDesc {
        name: "AnimationPlayer".into(),
        kind: Some("AnimationPlayer".into()),
        animation: Some(AnimationDesc::default()),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_engine::core::Vec3;

    #[test]
    fn key_scrub_stop_and_save_a_clip() {
        let project = std::env::temp_dir().join(format!("ab-anim-panel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(project.join("anims")).unwrap();
        std::fs::write(
            project.join("anims/open.toml"),
            "name = \"Open\"\nlength = 2.0\n",
        )
        .unwrap();
        let mut editor = Editor::new(project.clone(), None, ab_engine::audio::AudioOutput::Silent);
        editor.select(None);
        editor.add_node(
            NodeDesc {
                name: "Door".into(),
                kind: Some("Node3D".into()),
                ..Default::default()
            },
            Some(Vec3::ZERO),
        );
        let door = editor.selection.unwrap();
        editor.select(None);
        let mut player_desc = new_player();
        player_desc.animation.as_mut().unwrap().clips = vec!["anims/open.toml".into()];
        editor.add_node(player_desc, None);
        let player = editor.selection.unwrap();
        assert!(editor.tree().component::<AnimationPlayer>(player).is_some());

        let mut panel = AnimationPanel {
            player: Some(player),
            ..Default::default()
        };
        panel.load_files(&editor, player);
        panel.clip = Some("Open".into());
        let i = panel.file_index().expect("the clip file is editable");
        editor.select(Some(door));
        panel.key_position = true;
        panel.key_rotation = false;
        // Key at 0, move the door, key at 2.
        panel.time = 0.0;
        panel.key_selected(&mut editor, player, i);
        let moved = Transform3D::from_translation(Vec3::new(3.0, 0.0, 0.0));
        editor.set_transform(door, moved);
        panel.time = 2.0;
        panel.key_selected(&mut editor, player, i);
        assert_eq!(panel.files[i].1.tracks[0].target, "Door");
        assert_eq!(panel.files[i].1.tracks[0].keys.len(), 2);

        // Scrub to the middle: the door is half way.
        panel.time = 1.0;
        panel.pose(&mut editor, player);
        let x = editor.tree().position(door).unwrap().x;
        assert!((x - 1.5).abs() < 1e-3, "{x}");
        // Stop: back to where it was before the preview.
        panel.end_preview(&mut editor);
        assert_eq!(editor.tree().position(door).unwrap(), Vec3::ZERO);

        panel.save(&mut editor, i);
        let saved =
            ClipFile::from_toml(&std::fs::read_to_string(project.join("anims/open.toml")).unwrap())
                .unwrap();
        assert_eq!(
            saved.tracks[0].keys,
            vec![(0.0, [0.0; 3]), (2.0, [3.0, 0.0, 0.0])]
        );
        assert!(panel.dirty.is_empty());
        let _ = std::fs::remove_dir_all(&project);
    }
}
