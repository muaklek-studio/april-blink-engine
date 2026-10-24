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
use crate::i18n::{t as tr, tf};
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
    /// Show the selected track's values as curves (instead of the timeline alone).
    curves: bool,
    /// Which components the curves show (x, y, z).
    curve_axes: [bool; 3],
    /// When the panel was last shown: Delete acts on its selected key only while it is.
    shown_at: Option<std::time::Instant>,
    /// Delete was pressed for the selected key (the editor's shortcuts take the key event).
    delete_key: bool,
    /// The selected key as last seen, and the node selection then: selecting another node
    /// drops the key selection, so Delete goes back to deleting nodes.
    key_seen: (Option<(usize, usize)>, Option<NodeHandle>),
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
            curves: false,
            curve_axes: [true; 3],
            shown_at: None,
            delete_key: false,
            key_seen: (None, None),
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
        self.shown_at = Some(std::time::Instant::now());
        if self.selected_key != self.key_seen.0 {
            self.key_seen = (self.selected_key, editor.selection);
        } else if self.selected_key.is_some() && editor.selection != self.key_seen.1 {
            self.selected_key = None;
            self.key_seen = (None, None);
        }
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
        let editable = self.file_index().is_some() && desc_of(editor, player).is_some();
        if editable {
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut self.curves,
                    false,
                    format!("{}  {}", icon::ROWS, tr("animation.timeline")),
                );
                ui.selectable_value(
                    &mut self.curves,
                    true,
                    format!("{}  {}", icon::WAVE_SINE, tr("animation.curves")),
                );
                if self.curves {
                    ui.separator();
                    for (i, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                        ui.checkbox(&mut self.curve_axes[i], name);
                    }
                    ui.label(
                        RichText::new(tr("animation.curve_hint"))
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                }
            });
        }
        if self.curves && editable {
            let height = (ui.available_height() * 0.35).clamp(60.0, 180.0);
            ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                self.timeline(ui, editor, player);
            });
            self.curve_editor(ui, editor, player);
        } else {
            self.timeline(ui, editor, player);
        }
    }

    /// The selected track (its key's, else the first) as one curve per component: keys are
    /// dragged in value and time.
    fn curve_editor(&mut self, ui: &mut egui::Ui, editor: &mut Editor, player: NodeHandle) {
        let Some(i) = self.file_index() else {
            return;
        };
        let track_index = self.selected_key.map_or(0, |(t, _)| t);
        let Some(track) = self.files[i].1.tracks.get(track_index).cloned() else {
            super::empty_hint(ui, icon::WAVE_SINE, &tr("animation.key_first"));
            return;
        };
        let duration = self.duration(editor, player).max(0.1);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ui.available_height().max(120.0)),
            Sense::hover(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, theme::BG_0);
        let plot = Rect::from_min_max(
            Pos2::new(rect.left() + 48.0, rect.top() + 10.0),
            Pos2::new(rect.right() - 10.0, rect.bottom() - 18.0),
        );
        // Value range: the keys and the curve between them, with a margin.
        let samples = 160;
        let mut low = f32::MAX;
        let mut high = f32::MIN;
        for s in 0..=samples {
            let time = duration * s as f32 / samples as f32;
            if let Some(v) = track.sample(time) {
                for c in (0..3).filter(|&c| self.curve_axes[c]) {
                    low = low.min(v[c]);
                    high = high.max(v[c]);
                }
            }
        }
        if low > high {
            return;
        }
        let pad = ((high - low) * 0.15).max(0.5);
        let (low, high) = (low - pad, high + pad);
        let x_of = |t: f32| plot.left() + t / duration * plot.width();
        let t_of = |x: f32| ((x - plot.left()) / plot.width() * duration).clamp(0.0, duration);
        let y_of = |v: f32| plot.bottom() - (v - low) / (high - low) * plot.height();
        let v_of = |y: f32| low + (plot.bottom() - y) / plot.height() * (high - low);
        // Grid: zero, and a few values.
        for k in 0..=4 {
            let v = low + (high - low) * k as f32 / 4.0;
            let y = y_of(v);
            painter.line_segment(
                [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
                Stroke::new(1.0_f32, theme::BG_3),
            );
            painter.text(
                Pos2::new(rect.left() + 4.0, y),
                egui::Align2::LEFT_CENTER,
                format!("{v:.2}"),
                egui::FontId::monospace(10.0),
                theme::TEXT_DIM,
            );
        }
        if low < 0.0 && high > 0.0 {
            let y = y_of(0.0);
            painter.line_segment(
                [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
                Stroke::new(1.0_f32, theme::TEXT_DIM.gamma_multiply(0.5)),
            );
        }
        let colors = [theme::AXIS_X, theme::AXIS_Y, theme::AXIS_Z];
        for c in (0..3).filter(|&c| self.curve_axes[c]) {
            let points: Vec<Pos2> = (0..=samples)
                .filter_map(|s| {
                    let time = duration * s as f32 / samples as f32;
                    track
                        .sample(time)
                        .map(|v| Pos2::new(x_of(time), y_of(v[c])))
                })
                .collect();
            painter.add(egui::Shape::line(points, Stroke::new(1.5_f32, colors[c])));
        }
        // Playhead.
        let x = x_of(self.time.min(duration));
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            Stroke::new(1.0_f32, theme::WARN),
        );
        painter.text(
            plot.left_bottom() + Vec2::new(0.0, 4.0),
            egui::Align2::LEFT_TOP,
            format!(
                "{} · {} ({})",
                track.target,
                track.property.name(),
                track.interpolation.name()
            ),
            egui::FontId::proportional(10.5),
            theme::TEXT_DIM,
        );
        // Keys, per component.
        // (key, component, new time or `None` to keep it, value).
        let mut edit: Option<(usize, usize, Option<f32>, f32)> = None;
        for (k, &(time, value)) in track.keys.iter().enumerate() {
            for c in (0..3).filter(|&c| self.curve_axes[c]) {
                let center = Pos2::new(x_of(time), y_of(value[c]));
                let selected = self.selected_key == Some((track_index, k));
                painter.circle_filled(center, if selected { 5.0 } else { 4.0 }, colors[c]);
                painter.circle_stroke(
                    center,
                    5.0,
                    Stroke::new(
                        1.0_f32,
                        if selected {
                            Color32::WHITE
                        } else {
                            theme::BG_0
                        },
                    ),
                );
                let hit = ui.interact(
                    Rect::from_center_size(center, Vec2::splat(12.0)),
                    ui.id().with(("curve_key", track_index, k, c)),
                    Sense::click_and_drag(),
                );
                if hit.clicked() || hit.drag_started() {
                    self.selected_key = Some((track_index, k));
                    self.time = time;
                }
                if hit.dragged() && !editor.is_playing() {
                    if let Some(p) = hit.interact_pointer_pos() {
                        // Hold Shift: value only.
                        let keep_time = ui.input(|i| i.modifiers.shift);
                        let t = (!keep_time).then(|| t_of(p.x));
                        // The key being dragged is the selected one: once it passes a
                        // neighbour the keys re-sort, and this widget's index is the other's.
                        let dragged = self
                            .selected_key
                            .filter(|s| s.0 == track_index)
                            .map_or(k, |s| s.1);
                        edit = Some((dragged, c, t, v_of(p.y)));
                    }
                }
                hit.on_hover_text(format!("{:.2}s: {:.3}", time, value[c]));
            }
        }
        if let Some((k, c, time, value)) = edit {
            let t = &mut self.files[i].1.tracks[track_index];
            if k < t.keys.len() {
                let time = time.map_or(t.keys[k].0, |time| (time * 100.0).round() / 100.0);
                t.keys[k].0 = time;
                t.keys[k].1[c] = (value * 1000.0).round() / 1000.0;
                let moved = t.keys[k];
                t.sort();
                let index = t.keys.iter().position(|key| *key == moved).unwrap_or(k);
                self.selected_key = Some((track_index, index));
                self.time = time;
                self.changed(editor, player, i);
                self.pose(editor, player);
            }
        }
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
            ui.label(RichText::new(tr("animation.no_player")).color(theme::TEXT_DIM));
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

    /// Reads the player's clip files (once per player). Unsaved changes to the previous
    /// player's clips are saved first, never dropped.
    fn load_files(&mut self, editor: &mut Editor, player: NodeHandle) {
        if self.files_for == Some(player) {
            return;
        }
        self.save_all(editor);
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
                .selected_text(self.clip.clone().unwrap_or_else(|| tr("animation.no_clip")))
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
            if editable_player
                && ui
                    .button(format!("{}  {}", icon::PLUS, tr("animation.new_clip")))
                    .clicked()
            {
                // The first free name: other players' clip files count too.
                let project = editor.project();
                let free = (clips.len() + 1..)
                    .map(|n| format!("Clip{n}"))
                    .find(|name| !project.join(clip_path(name)).exists())
                    .unwrap_or_default();
                self.new_clip = Some(free);
            }
            ui.separator();
            if ui
                .button(icon::SKIP_BACK)
                .on_hover_text(tr("animation.to_start"))
                .clicked()
            {
                self.time = 0.0;
                self.pose(editor, player);
            }
            let (glyph, tip) = if self.playing {
                (icon::PAUSE, tr("command.pause"))
            } else {
                (icon::PLAY, tr("animation.play_clip"))
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
                .on_hover_text(tr("animation.stop_hint"))
                .clicked()
            {
                self.end_preview(editor);
            }
            ui.label(RichText::new(format!("{:.2} / {duration:.2} s", self.time)).monospace());
            if let Some(i) = self.file_index().filter(|_| editable_player) {
                ui.separator();
                let mut length = duration;
                ui.label(tr("animation.length"));
                if ui
                    .add(
                        egui::DragValue::new(&mut length)
                            .custom_parser(crate::panels::parse_finite)
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
                ui.toggle_value(&mut self.key_position, tr("animation.position"));
                ui.toggle_value(&mut self.key_rotation, tr("animation.rotation"));
                ui.toggle_value(&mut self.key_scale, tr("animation.scale"));
                let can_key = editor
                    .selection
                    .is_some_and(|n| key_path(editor, player, n).is_some());
                if ui
                    .add_enabled(
                        can_key,
                        egui::Button::new(
                            RichText::new(format!("{}  {}", icon::DIAMOND, tr("animation.key")))
                                .strong(),
                        )
                        .fill(theme::ACCENT_DIM),
                    )
                    .on_hover_text(tr("animation.key_hint"))
                    .on_disabled_hover_text(tr("animation.key_disabled"))
                    .clicked()
                {
                    self.key_selected(editor, player, i);
                }
                let dirty = self.dirty.contains(&self.files[i].0);
                if ui
                    .add_enabled(
                        dirty,
                        egui::Button::new(format!("{}  {}", icon::FLOPPY_DISK, tr("command.save"))),
                    )
                    .clicked()
                {
                    self.save(editor, i);
                }
            } else if !clips.is_empty() {
                ui.label(
                    RichText::new(tr("animation.imported"))
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
            ui.label(tr("animation.name"));
            let edit = ui.add(egui::TextEdit::singleline(&mut name).desired_width(140.0));
            edit.request_focus();
            create = ui.button(tr("menu.create")).clicked()
                || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            cancel = ui.button(tr("dialog.cancel")).clicked();
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
            .filter(|&c| super::is_name_char(c) || c == '_')
            .collect();
        if name.is_empty() || self.files.iter().any(|(_, f)| f.name == name) {
            editor.fail(tr("animation.name_taken"));
            return;
        }
        let path = clip_path(&name);
        let full = editor.project().join(&path);
        if full.exists() {
            // Another player's clip, most likely: never written over.
            editor.fail(tf("animation.file_exists", &[("path", &path)]));
            self.new_clip = Some(name);
            return;
        }
        let file = ClipFile {
            name: name.clone(),
            length: Some(2.0),
            tracks: Vec::new(),
        };
        let written = full
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&full, file.to_toml().unwrap_or_default()));
        if let Err(err) = written {
            editor.fail(tf(
                "animation.write_failed",
                &[("path", &path), ("error", &err)],
            ));
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
        editor.info(tf("animation.created", &[("name", &name), ("path", &path)]));
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
    /// Whether a clip is playing in the preview (its views animate).
    pub fn is_playing(&self) -> bool {
        self.playing && self.preview.is_some()
    }

    pub fn end_preview(&mut self, editor: &mut Editor) {
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
        editor.info(tf(
            "animation.keyed",
            &[("path", &path), ("time", &format!("{time:.2}"))],
        ));
    }

    /// Whether Delete is for the panel's selected key rather than the selected nodes: a key
    /// is selected and the panel is on screen.
    pub fn takes_delete(&self) -> bool {
        self.selected_key.is_some()
            && self
                .shown_at
                .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(500))
    }

    /// Deletes the selected key the next time the panel is shown.
    pub fn delete_selected_key(&mut self) {
        self.delete_key = true;
    }

    /// Whether a clip file has changes that are not saved yet.
    pub fn has_unsaved(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Saves every clip file with unsaved changes; whether they all were.
    pub fn save_all(&mut self, editor: &mut Editor) -> bool {
        for i in 0..self.files.len() {
            if self.dirty.contains(&self.files[i].0) {
                self.save(editor, i);
            }
        }
        self.dirty.is_empty()
    }

    /// Forgets unsaved clip changes (the files on disk stay as they are): read again on the
    /// next show.
    pub fn discard_changes(&mut self) {
        self.dirty.clear();
        self.files.clear();
        self.files_for = None;
    }

    fn save(&mut self, editor: &mut Editor, i: usize) {
        let (path, file) = &self.files[i];
        let full = editor.project().join(path);
        match file.to_toml().map_err(|e| e.to_string()).and_then(|text| {
            editor
                .write_project_file(&full, text)
                .map_err(|e| e.to_string())
        }) {
            Ok(()) => {
                editor.info(tf("animation.saved", &[("path", path)]));
                self.dirty.remove(path);
            }
            Err(err) => editor.fail(tf(
                "animation.save_failed",
                &[("path", path), ("error", &err)],
            )),
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
                                    // The selected key is the one dragged (after passing a
                                    // neighbour, this widget's index is the neighbour's).
                                    let dragged = self
                                        .selected_key
                                        .filter(|s| s.0 == track)
                                        .map_or(k, |s| s.1);
                                    row_actions.push(RowAction::Move(track, dragged, t_of(p.x)));
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
                                    for (mode, name) in
                                        TrackInterpolation::ALL.map(|m| (m, m.name()))
                                    {
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
                                    .on_hover_text(tr("animation.delete_track"))
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
                            tr("animation.no_tracks_yet")
                        } else {
                            tr("animation.no_tracks")
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
        // Delete the selected key (Delete, routed here by the editor's shortcuts).
        if std::mem::take(&mut self.delete_key) && editable && self.selected_key.is_some() {
            if let (Some(i), Some((track, key))) = (self.file_index(), self.selected_key.take()) {
                // The selection may be from another clip (the game switched it).
                if let Some(t) = self.files[i].1.tracks.get_mut(track) {
                    if key < t.keys.len() {
                        t.keys.remove(key);
                    }
                }
                self.changed(editor, player, i);
                self.pose(editor, player);
            }
        }
        section(
            ui,
            "anim_help",
            icon::INFO,
            &tr("animation.help_title"),
            |ui| {
                ui.label(
                    RichText::new(tr("animation.help"))
                        .small()
                        .color(theme::TEXT_DIM),
                );
            },
        );
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
/// Where a clip named `name` is kept, relative to the project.
fn clip_path(name: &str) -> String {
    format!("anims/{}.toml", name.to_lowercase())
}

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
        panel.load_files(&mut editor, player);
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
