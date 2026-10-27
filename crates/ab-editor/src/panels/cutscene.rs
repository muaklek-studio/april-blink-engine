//! The Cutscene Editor: a timeline for a `Cutscene` node's file (see
//! [`ab_engine::cutscene`]).
//!
//! The Camera row holds the camera's keys: place the workspace's view where the shot should
//! be and press Key Camera to record it at the playhead. The Events row holds what happens
//! (animations, sounds, signals, shown and hidden nodes, fades, subtitles), each a block as
//! long as it lasts. Drag keys and events along the timeline; select one to change it on the
//! right. Play previews the camera move (the events act in Play mode). Save writes the file
//! and gives the node the new version.

use super::section;
use crate::i18n::t;
use crate::state::Editor;
use crate::theme;
use ab_engine::animation_file::{rotation_to_degrees, TrackInterpolation};
use ab_engine::core::{Affine3A, Quat, Vec3};
use ab_engine::cutscene::{
    CameraKey, CutsceneFile, CutscenePlayer, Event, EventKind, CUTSCENE_EXTENSION,
};
use ab_engine::scene::NodeHandle;
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use egui_phosphor::regular as icon;

const ROW: f32 = 34.0;
const NAMES: f32 = 110.0;
const RULER: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selected {
    Camera(usize),
    Event(usize),
}

#[derive(Debug)]
pub struct CutscenePanel {
    node: Option<NodeHandle>,
    file: CutsceneFile,
    /// The file it is saved to (relative to the project).
    path: String,
    dirty: bool,
    pub time: f32,
    playing: bool,
    selected: Option<Selected>,
    /// Show the shot through the cutscene's camera.
    pub look_through: bool,
    pixels_per_second: f32,
    /// Zoom the timeline to the cutscene's length on the next draw.
    fit: bool,
    error: Option<String>,
}

/// The edited cutscene's camera move, for the 3D views to draw (see [`camera_path`]).
#[derive(Debug, Clone)]
pub struct CameraPath {
    /// The path, sampled along the cutscene.
    pub points: Vec<Vec3>,
    /// The keys' positions, and which one is selected.
    pub keys: Vec<Vec3>,
    pub selected: Option<usize>,
    /// The camera at the playhead: position, rotation, field of view (degrees).
    pub at: Option<(Vec3, Quat, f32)>,
    pass: u64,
}

const CAMERA_PATH: &str = "ab-editor.cutscene-path";

/// The camera move of the cutscene being edited, while its editor is open.
pub fn camera_path(ctx: &egui::Context) -> Option<CameraPath> {
    let path: CameraPath = ctx.data(|d| d.get_temp(egui::Id::new(CAMERA_PATH)))?;
    // Published by the editor in the last frames: gone when it closes.
    (ctx.cumulative_pass_nr() <= path.pass + 2).then_some(path)
}

impl Default for CutscenePanel {
    fn default() -> Self {
        Self {
            node: None,
            file: CutsceneFile::default(),
            path: String::new(),
            dirty: false,
            time: 0.0,
            playing: false,
            selected: None,
            look_through: true,
            pixels_per_second: 90.0,
            fit: true,
            error: None,
        }
    }
}

/// The cutscene node `node` is, or is inside.
fn cutscene_of(editor: &Editor, node: NodeHandle) -> Option<NodeHandle> {
    let tree = editor.tree();
    std::iter::successors(Some(node), |&n| tree.parent(n))
        .find(|&n| tree.component::<CutscenePlayer>(n).is_some())
}

fn kind_color(kind: &EventKind) -> Color32 {
    match kind {
        EventKind::Animation { .. } => Color32::from_rgb(120, 190, 255),
        EventKind::Sound { .. } => Color32::from_rgb(240, 130, 180),
        EventKind::Signal { .. } => theme::WARN,
        EventKind::Visible { .. } => theme::PLAY,
        EventKind::Fade { .. } => Color32::from_rgb(160, 160, 170),
        EventKind::Subtitle { .. } => Color32::from_rgb(200, 160, 255),
    }
}

/// The narrowest an event's block is drawn (pixels): instant ones still get a handle.
const MIN_BLOCK: f32 = 18.0;

/// The lane of each event (sorted by time), so that none overlaps another on its lane, and
/// how many lanes there are. `min_seconds`: the shortest a block lasts on screen.
fn lanes(events: &[Event], min_seconds: f32) -> (Vec<usize>, usize) {
    let mut ends: Vec<f32> = Vec::new();
    let lanes = events
        .iter()
        .map(|e| {
            let end = e.time + e.kind.duration().max(min_seconds);
            match ends.iter().position(|&lane_end| lane_end <= e.time + 1e-4) {
                Some(lane) => {
                    ends[lane] = end;
                    lane
                }
                None => {
                    ends.push(end);
                    ends.len() - 1
                }
            }
        })
        .collect();
    (lanes, ends.len())
}

/// What an event's block says: its text, clip, sound, signal or target.
fn event_label(kind: &EventKind) -> String {
    match kind {
        EventKind::Animation { target, clip, .. } => format!("{target} · {clip}"),
        EventKind::Sound { stream, .. } => stream.rsplit('/').next().unwrap_or(stream).to_owned(),
        EventKind::Signal { name } => name.clone(),
        EventKind::Visible { target, visible } => {
            format!("{target} {}", if *visible { "on" } else { "off" })
        }
        EventKind::Fade { to, .. } => format!("Fade {:.0}%", to * 100.0),
        EventKind::Subtitle { text, .. } => format!("“{text}”"),
    }
}

fn kind_icon(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::Animation { .. } => icon::FILM_STRIP,
        EventKind::Sound { .. } => icon::SPEAKER_HIGH,
        EventKind::Signal { .. } => icon::BROADCAST,
        EventKind::Visible { .. } => icon::EYE,
        EventKind::Fade { .. } => icon::CIRCLE_HALF,
        EventKind::Subtitle { .. } => icon::CHAT_TEXT,
    }
}

/// A new event of each kind, for the Add menu.
fn new_events() -> Vec<EventKind> {
    vec![
        EventKind::Animation {
            target: String::new(),
            clip: String::new(),
            crossfade: None,
        },
        EventKind::Sound {
            stream: String::new(),
            volume_db: None,
        },
        EventKind::Signal {
            name: "event".into(),
        },
        EventKind::Visible {
            target: String::new(),
            visible: true,
        },
        EventKind::Fade {
            to: 1.0,
            duration: 1.0,
            color: None,
        },
        EventKind::Subtitle {
            text: "…".into(),
            key: None,
            duration: 2.0,
        },
    ]
}

impl CutscenePanel {
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Edits `node`'s cutscene (it or the cutscene it is in); reads its file again when it
    /// is another one.
    pub fn edit(&mut self, editor: &Editor, node: NodeHandle) {
        let Some(cutscene) = cutscene_of(editor, node) else {
            return;
        };
        if self.node == Some(cutscene) {
            return;
        }
        let Some(player) = editor
            .tree()
            .component::<CutscenePlayer>(cutscene)
            .map(|p| p.clone())
        else {
            return;
        };
        let name = editor
            .tree()
            .name(cutscene)
            .map(|n| n.to_lowercase())
            .unwrap_or_default();
        self.node = Some(cutscene);
        self.file = player.file;
        self.path = if player.desc.file.is_empty() {
            format!("cutscenes/{name}.{CUTSCENE_EXTENSION}")
        } else {
            player.desc.file
        };
        self.dirty = false;
        self.time = 0.0;
        self.playing = false;
        self.selected = None;
        self.error = None;
        self.fit = true;
    }

    /// Hands the camera move to the 3D views (see [`camera_path`]).
    fn publish_path(&self, ctx: &egui::Context) {
        let duration = self.file.duration();
        let samples = ((duration * 20.0) as usize).clamp(2, 2000);
        let points = (0..=samples)
            .filter_map(|i| self.file.camera_at(duration * i as f32 / samples as f32))
            .map(|(p, _, _)| p)
            .collect();
        let path = CameraPath {
            points,
            keys: self
                .file
                .camera
                .iter()
                .map(|k| Vec3::from(k.position))
                .collect(),
            selected: match self.selected {
                Some(Selected::Camera(i)) => Some(i),
                _ => None,
            },
            at: (!self.look_through)
                .then(|| self.file.camera_at(self.time))
                .flatten(),
            pass: ctx.cumulative_pass_nr(),
        };
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(CAMERA_PATH), path));
    }

    /// Times worth jumping to: keys, events, the start and the end.
    fn marks(&self) -> Vec<f32> {
        let mut marks: Vec<f32> = self
            .file
            .camera
            .iter()
            .map(|k| k.time)
            .chain(self.file.events.iter().map(|e| e.time))
            .chain([0.0, self.file.duration()])
            .collect();
        marks.sort_by(f32::total_cmp);
        marks.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
        marks
    }

    /// Keys of the timeline: Space plays, K keys the camera, Delete removes the selected,
    /// Home goes to the start, ← → jump between keys and events.
    fn keyboard(&mut self, ui: &egui::Ui, view: Option<(Affine3A, f32)>) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let pressed = |key| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
        if pressed(egui::Key::Space) {
            if !self.playing && self.time >= self.file.duration() {
                self.time = 0.0;
            }
            self.playing = !self.playing;
        }
        if pressed(egui::Key::K) && !self.look_through {
            if let Some((global, fov)) = view {
                self.key_camera(global, fov);
            }
        }
        if pressed(egui::Key::Home) {
            self.time = 0.0;
            self.playing = false;
        }
        if pressed(egui::Key::ArrowRight) {
            if let Some(next) = self.marks().into_iter().find(|&m| m > self.time + 1e-3) {
                self.time = next;
            }
            self.playing = false;
        }
        if pressed(egui::Key::ArrowLeft) {
            if let Some(previous) = self
                .marks()
                .into_iter()
                .rev()
                .find(|&m| m < self.time - 1e-3)
            {
                self.time = previous;
            }
            self.playing = false;
        }
        if pressed(egui::Key::Delete) {
            match self.selected.take() {
                Some(Selected::Camera(i)) if i < self.file.camera.len() => {
                    self.file.camera.remove(i);
                    self.dirty = true;
                }
                Some(Selected::Event(i)) if i < self.file.events.len() => {
                    self.file.events.remove(i);
                    self.dirty = true;
                }
                _ => {}
            }
        }
    }

    /// The camera at the playhead, to look through: world transform and field of view.
    pub fn preview_camera(&self, editor: &Editor) -> Option<(Affine3A, f32)> {
        if !self.look_through {
            return None;
        }
        if !self.node.is_some_and(|n| editor.tree().is_valid(n)) {
            return None;
        }
        // Keys are in world space (the engine places the camera there too).
        let (position, rotation, fov) = self.file.camera_at(self.time)?;
        Some((Affine3A::from_rotation_translation(rotation, position), fov))
    }

    /// The toolbar, timeline and details. `view`: the workspace's camera (world transform,
    /// field of view), for Key Camera.
    pub fn show(&mut self, ui: &mut egui::Ui, editor: &mut Editor, view: Option<(Affine3A, f32)>) {
        if let Some(selected) = editor.selection {
            self.edit(editor, selected);
        }
        let Some(node) = self.node.filter(|&n| editor.tree().is_valid(n)) else {
            self.node = None;
            super::empty_hint(ui, icon::FILM_SLATE, &t("cutscene.empty"));
            return;
        };
        if self.playing {
            let dt = ui.input(|i| i.stable_dt.min(0.1));
            self.time += dt;
            if self.time >= self.file.duration() {
                self.time = self.file.duration();
                self.playing = false;
            }
            ui.ctx().request_repaint();
        }
        self.keyboard(ui, view);
        self.publish_path(ui.ctx());
        self.toolbar(ui, editor, node, view);
        ui.separator();
        egui::Panel::right("cutscene-details")
            .resizable(true)
            .default_size(300.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.details(ui, editor));
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.timeline(ui));
    }

    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        node: NodeHandle,
        view: Option<(Affine3A, f32)>,
    ) {
        ui.horizontal_wrapped(|ui| {
            let name = editor
                .tree()
                .name(node)
                .map(|n| n.to_owned())
                .unwrap_or_default();
            ui.label(
                RichText::new(format!("{}  {name}", icon::FILM_SLATE)).font(theme::bold(13.0)),
            );
            ui.add(egui::TextEdit::singleline(&mut self.path).desired_width(220.0))
                .on_hover_text(t("cutscene.file_hint"));
            let save = egui::Button::new(format!(
                "{}  {}{}",
                icon::FLOPPY_DISK,
                t("dialog.save"),
                if self.dirty { " •" } else { "" }
            ));
            if ui.add_enabled(!editor.is_playing(), save).clicked() {
                self.save(editor, node);
            }
            ui.separator();
            let play = if self.playing {
                icon::PAUSE
            } else {
                icon::PLAY
            };
            if ui
                .button(play)
                .on_hover_text(t("cutscene.preview_hint"))
                .clicked()
            {
                if !self.playing && self.time >= self.file.duration() {
                    self.time = 0.0;
                }
                self.playing = !self.playing;
            }
            if ui.button(icon::STOP).clicked() {
                self.playing = false;
                self.time = 0.0;
            }
            ui.label(
                RichText::new(format!("{:.2} / {:.2} s", self.time, self.file.duration()))
                    .monospace(),
            );
            ui.toggle_value(
                &mut self.look_through,
                format!("{}  {}", icon::VIDEO_CAMERA, t("cutscene.look_through")),
            );
            let key = egui::Button::new(format!("{}  {}", icon::DIAMOND, t("cutscene.key_camera")));
            if ui
                .add_enabled(view.is_some() && !self.look_through, key)
                .on_hover_text(t("cutscene.key_camera_hint"))
                .on_disabled_hover_text(t("cutscene.key_camera_hint"))
                .clicked()
            {
                if let Some((global, fov)) = view {
                    self.key_camera(global, fov);
                }
            }
            ui.menu_button(
                format!("{}  {}", icon::PLUS, t("cutscene.add_event")),
                |ui| {
                    for kind in new_events() {
                        if ui
                            .button(format!("{}  {}", kind_icon(&kind), kind.name()))
                            .clicked()
                        {
                            self.file.events.push(Event {
                                time: self.time,
                                kind,
                            });
                            self.file.sort();
                            self.selected = self
                                .file
                                .events
                                .iter()
                                .rposition(|e| e.time == self.time)
                                .map(Selected::Event);
                            self.dirty = true;
                            ui.close();
                        }
                    }
                },
            );
            ui.separator();
            ui.label(t("cutscene.camera_motion"));
            egui::ComboBox::from_id_salt("cutscene_interpolation")
                .selected_text(self.file.camera_interpolation.name())
                .show_ui(ui, |ui| {
                    for mode in TrackInterpolation::ALL {
                        if ui
                            .selectable_label(self.file.camera_interpolation == mode, mode.name())
                            .clicked()
                        {
                            self.file.camera_interpolation = mode;
                            self.dirty = true;
                        }
                    }
                });
        });
        if let Some(error) = &self.error {
            ui.label(RichText::new(format!("{}  {error}", icon::WARNING)).color(theme::WARN));
        }
    }

    /// Records the view as a camera key at the playhead (replacing one within a frame).
    fn key_camera(&mut self, global: Affine3A, fov: f32) {
        let (_, rotation, position) = global.to_scale_rotation_translation();
        let key = CameraKey {
            time: self.time,
            position: (position * 1000.0).round().to_array().map(|v| v / 1000.0),
            rotation: rotation_to_degrees(rotation).map(|d| (d * 100.0).round() / 100.0),
            fov,
        };
        match self
            .file
            .camera
            .iter_mut()
            .find(|k| (k.time - self.time).abs() < 1.0 / 60.0)
        {
            Some(existing) => *existing = key,
            None => self.file.camera.push(key),
        }
        self.file.sort();
        self.selected = self
            .file
            .camera
            .iter()
            .position(|k| (k.time - self.time).abs() < 1e-6)
            .map(Selected::Camera);
        self.dirty = true;
    }

    fn save(&mut self, editor: &mut Editor, node: NodeHandle) {
        let mut path = self.path.trim().replace('\\', "/");
        if !path.ends_with(&format!(".{CUTSCENE_EXTENSION}")) {
            path = format!("{}.{CUTSCENE_EXTENSION}", path.trim_end_matches('.'));
        }
        let full = editor.project().join(&path);
        let written = editor.write_project_file(&full, self.file.to_toml());
        if let Err(err) = written {
            self.error = Some(format!("{}: {err}", full.display()));
            return;
        }
        self.path = path.clone();
        self.dirty = false;
        self.error = None;
        // The node points at the file and plays the new version.
        if let Some(old) = editor.describe_node(node) {
            let mut new = old.clone();
            if let Some(c) = new.cutscene.as_mut() {
                c.file = path.clone();
            }
            if new != old {
                editor.apply_desc(node, &old, new);
                editor.end_gesture();
                if let Some(fresh) = editor
                    .selection
                    .filter(|&n| cutscene_of(editor, n).is_some())
                {
                    self.node = cutscene_of(editor, fresh);
                }
            }
        }
        if let Some(mut player) = self
            .node
            .and_then(|n| editor.tree().component_mut::<CutscenePlayer>(n))
        {
            player.file = self.file.clone();
        }
        editor.assets.request_rescan();
        editor.info(format!("Saved {path}"));
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        let duration = self.file.duration().max(1.0);
        // Events that overlap in time go on lanes of their own.
        let min_seconds = MIN_BLOCK / self.pixels_per_second;
        let (lanes, lane_count) = lanes(&self.file.events, min_seconds);
        let events_height = ROW * lane_count.max(1) as f32;
        let height = RULER + ROW + events_height + 8.0;
        if std::mem::take(&mut self.fit) {
            // The whole cutscene in view, with a little room after it.
            let room = (ui.available_width() - NAMES - 24.0).max(100.0);
            self.pixels_per_second = (room / (duration * 1.1)).clamp(20.0, 600.0);
        }
        egui::ScrollArea::horizontal().show(ui, |ui| {
            let width = NAMES + (duration + 2.0) * self.pixels_per_second;
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(width.max(ui.available_width()), height),
                Sense::click_and_drag(),
            );
            let painter = ui.painter_at(rect);
            let pps = self.pixels_per_second;
            let x_of = |t: f32| rect.left() + NAMES + t * pps;
            let t_of = |x: f32| ((x - rect.left() - NAMES) / pps).max(0.0);
            painter.rect_filled(
                Rect::from_min_size(rect.min, egui::vec2(rect.width(), RULER)),
                0.0,
                theme::BG_2,
            );
            let mut second = 0.0_f32;
            while second <= duration + 2.0 {
                let x = x_of(second);
                painter.line_segment(
                    [Pos2::new(x, rect.top() + 6.0), Pos2::new(x, rect.bottom())],
                    Stroke::new(1.0_f32, theme::BG_3),
                );
                painter.text(
                    Pos2::new(x + 3.0, rect.top() + 2.0),
                    egui::Align2::LEFT_TOP,
                    format!("{second:.0}s"),
                    egui::FontId::proportional(10.0),
                    theme::TEXT_DIM,
                );
                second += 1.0;
            }
            let end = x_of(self.file.duration());
            painter.line_segment(
                [Pos2::new(end, rect.top()), Pos2::new(end, rect.bottom())],
                Stroke::new(1.0_f32, theme::WARN.gamma_multiply(0.6)),
            );
            let rows = [t("cutscene.camera"), t("cutscene.events")];
            for (r, name) in rows.iter().enumerate() {
                let top = rect.top() + RULER + r as f32 * ROW;
                if r % 2 == 1 {
                    painter.rect_filled(
                        Rect::from_min_size(
                            Pos2::new(rect.left(), top),
                            egui::vec2(rect.width(), events_height),
                        ),
                        0.0,
                        theme::BG_1.gamma_multiply(0.6),
                    );
                }
                painter.text(
                    Pos2::new(rect.left() + 6.0, top + ROW / 2.0),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::proportional(12.0),
                    theme::TEXT,
                );
            }
            // Camera keys, joined: each stretch is a move from one shot to the next.
            let mut moves: Vec<(Selected, f32)> = Vec::new();
            let camera_y = rect.top() + RULER + ROW / 2.0;
            for pair in self.file.camera.windows(2) {
                painter.line_segment(
                    [
                        Pos2::new(x_of(pair[0].time), camera_y),
                        Pos2::new(x_of(pair[1].time), camera_y),
                    ],
                    Stroke::new(3.0_f32, theme::ACCENT.gamma_multiply(0.35)),
                );
            }
            for (i, key) in self.file.camera.iter().enumerate() {
                let center = Pos2::new(x_of(key.time), camera_y);
                let selected = self.selected == Some(Selected::Camera(i));
                let s = 6.0;
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        center + Vec2::new(0.0, -s),
                        center + Vec2::new(s, 0.0),
                        center + Vec2::new(0.0, s),
                        center + Vec2::new(-s, 0.0),
                    ],
                    if selected {
                        Color32::WHITE
                    } else {
                        theme::ACCENT
                    },
                    Stroke::new(1.0_f32, theme::BG_0),
                ));
                let hit = ui.interact(
                    Rect::from_center_size(center, Vec2::splat(14.0)),
                    ui.id().with(("cut_key", i)),
                    Sense::click_and_drag(),
                );
                if hit.clicked() || hit.drag_started() {
                    self.selected = Some(Selected::Camera(i));
                    self.time = key.time;
                }
                if hit.dragged() {
                    if let Some(p) = hit.interact_pointer_pos() {
                        moves.push((Selected::Camera(i), t_of(p.x)));
                    }
                }
            }
            // Events: blocks as long as they last (a short tick for instant ones).
            let events_top = rect.top() + RULER + ROW;
            for (i, event) in self.file.events.iter().enumerate() {
                let x0 = x_of(event.time);
                let x1 = x0 + (event.kind.duration() * pps).max(MIN_BLOCK);
                let top = events_top + lanes[i] as f32 * ROW;
                let block =
                    Rect::from_min_max(Pos2::new(x0, top + 5.0), Pos2::new(x1, top + ROW - 5.0));
                let selected = self.selected == Some(Selected::Event(i));
                let color = kind_color(&event.kind);
                painter.rect_filled(
                    block,
                    4.0,
                    color.gamma_multiply(if selected { 0.9 } else { 0.55 }),
                );
                if selected {
                    painter.rect_stroke(
                        block,
                        4.0,
                        Stroke::new(1.5_f32, Color32::WHITE),
                        egui::StrokeKind::Inside,
                    );
                }
                painter
                    .with_clip_rect(block.shrink2(Vec2::new(4.0, 0.0)))
                    .text(
                        block.left_center() + Vec2::new(5.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        format!("{}  {}", kind_icon(&event.kind), event_label(&event.kind)),
                        egui::FontId::proportional(11.5),
                        theme::BG_0,
                    );
                let hit = ui.interact(
                    block,
                    ui.id().with(("cut_event", i)),
                    Sense::click_and_drag(),
                );
                if hit.clicked() || hit.drag_started() {
                    self.selected = Some(Selected::Event(i));
                    self.time = event.time;
                }
                if hit.dragged() {
                    if let Some(p) = hit.interact_pointer_pos() {
                        moves.push((
                            Selected::Event(i),
                            t_of(p.x - (block.width() / 2.0).min(9.0) + 9.0),
                        ));
                    }
                }
            }
            for (what, time) in moves {
                let time = (time * 100.0).round() / 100.0;
                match what {
                    Selected::Camera(i) => self.file.camera[i].time = time,
                    Selected::Event(i) => self.file.events[i].time = time,
                }
                self.time = time;
                self.dirty = true;
                // Keep the same item selected through the re-sort.
                let item = match what {
                    Selected::Camera(i) => Err(self.file.camera[i].clone()),
                    Selected::Event(i) => Ok(self.file.events[i].clone()),
                };
                self.file.sort();
                self.selected = match item {
                    Err(key) => self
                        .file
                        .camera
                        .iter()
                        .position(|k| *k == key)
                        .map(Selected::Camera),
                    Ok(event) => self
                        .file
                        .events
                        .iter()
                        .position(|e| *e == event)
                        .map(Selected::Event),
                };
            }
            // Scrub on the ruler or empty space.
            if response.clicked() || response.dragged() {
                if let Some(p) = response.interact_pointer_pos() {
                    if p.x > rect.left() + NAMES {
                        self.time = t_of(p.x);
                        self.playing = false;
                    }
                }
            }
            let x = x_of(self.time);
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(1.5_f32, theme::WARN),
            );
            // A handle on the ruler with the time, to grab and to read.
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(x - 6.0, rect.top()),
                    Pos2::new(x + 6.0, rect.top()),
                    Pos2::new(x, rect.top() + 9.0),
                ],
                theme::WARN,
                Stroke::NONE,
            ));
            painter.text(
                Pos2::new(x + 8.0, rect.top() + RULER + ROW * 0.5 - 9.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{:.2}s", self.time),
                egui::FontId::monospace(10.5),
                theme::WARN,
            );
            // Zoom with Ctrl+wheel.
            if response.hovered() && ui.input(|i| i.modifiers.command) {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                self.pixels_per_second =
                    (self.pixels_per_second * (1.0 + scroll / 300.0)).clamp(20.0, 600.0);
            }
        });
    }

    fn details(&mut self, ui: &mut egui::Ui, editor: &Editor) {
        let mut changed = false;
        let mut delete = false;
        match self.selected {
            Some(Selected::Camera(i)) if i < self.file.camera.len() => {
                let key = &mut self.file.camera[i];
                section(
                    ui,
                    "cut_camera",
                    icon::VIDEO_CAMERA,
                    &t("cutscene.camera_key"),
                    |ui| {
                        super::grid(ui, "cut_camera_grid", |ui| {
                            super::label(ui, "Time (s)");
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut key.time)
                                        .range(0.0..=3600.0)
                                        .speed(0.01),
                                )
                                .changed();
                            ui.end_row();
                            super::label(ui, "Position");
                            changed |= super::vec3(ui, &mut key.position, 0.05, "");
                            ui.end_row();
                            super::label(ui, "Rotation");
                            changed |= super::vec3(ui, &mut key.rotation, 0.5, "°");
                            ui.end_row();
                            super::label(ui, "Field of view");
                            changed |= ui
                                .add(egui::Slider::new(&mut key.fov, 10.0..=120.0))
                                .changed();
                            ui.end_row();
                        });
                        delete = ui
                            .button(
                                RichText::new(format!("{}  Delete key", icon::TRASH))
                                    .color(theme::ERROR),
                            )
                            .clicked();
                    },
                );
            }
            Some(Selected::Event(i)) if i < self.file.events.len() => {
                let event = &mut self.file.events[i];
                let title = format!("{}  {}", kind_icon(&event.kind), event.kind.name());
                section(ui, "cut_event", icon::LIGHTNING, &title, |ui| {
                    super::grid(ui, "cut_event_grid", |ui| {
                        super::label(ui, "Time (s)");
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut event.time)
                                    .range(0.0..=3600.0)
                                    .speed(0.01),
                            )
                            .changed();
                        ui.end_row();
                        changed |= event_fields(ui, editor, &mut event.kind);
                    });
                    delete = ui
                        .button(
                            RichText::new(format!("{}  Delete event", icon::TRASH))
                                .color(theme::ERROR),
                        )
                        .clicked();
                });
            }
            _ => {
                ui.label(RichText::new(t("cutscene.details_hint")).color(theme::TEXT_DIM));
                ui.add_space(6.0);
                super::grid(ui, "cut_file_grid", |ui| {
                    super::label(ui, "Length (s)");
                    let mut fixed = self.file.length.is_some();
                    if ui
                        .checkbox(&mut fixed, "")
                        .on_hover_text("Off: until the last key or event ends")
                        .changed()
                    {
                        self.file.length = fixed.then(|| self.file.duration());
                        changed = true;
                    }
                    if let Some(length) = self.file.length.as_mut() {
                        changed |= ui
                            .add(egui::DragValue::new(length).range(0.1..=3600.0).speed(0.05))
                            .changed();
                    }
                    ui.end_row();
                });
            }
        }
        if delete {
            match self.selected.take() {
                Some(Selected::Camera(i)) => {
                    self.file.camera.remove(i);
                }
                Some(Selected::Event(i)) => {
                    self.file.events.remove(i);
                }
                None => {}
            }
            changed = true;
        }
        if changed {
            self.dirty = true;
            self.file.sort();
        }
    }
}

/// Fields of an event's kind; whether one changed.
fn event_fields(ui: &mut egui::Ui, editor: &Editor, kind: &mut EventKind) -> bool {
    let mut changed = false;
    let text = |ui: &mut egui::Ui, name: &str, value: &mut String, hint: &str| {
        super::label(ui, name);
        let edited = ui
            .add(
                egui::TextEdit::singleline(value)
                    .hint_text(hint)
                    .desired_width(ui.available_width() - 4.0),
            )
            .changed();
        ui.end_row();
        edited
    };
    match kind {
        EventKind::Animation {
            target,
            clip,
            crossfade,
        } => {
            changed |= text(ui, "Player", target, "Door/Animator");
            changed |= text(ui, "Clip", clip, "Open");
            super::label(ui, "Crossfade (s)");
            let mut c = crossfade.unwrap_or(0.0);
            if ui
                .add(egui::DragValue::new(&mut c).range(0.0..=10.0).speed(0.01))
                .changed()
            {
                *crossfade = (c > 0.0).then_some(c);
                changed = true;
            }
            ui.end_row();
        }
        EventKind::Sound { stream, volume_db } => {
            super::label(ui, "Sound");
            egui::ComboBox::from_id_salt("cut_sound")
                .selected_text(if stream.is_empty() {
                    "None"
                } else {
                    stream.as_str()
                })
                .width(ui.available_width() - 4.0)
                .show_ui(ui, |ui| {
                    for asset in editor.assets.of_kind(crate::project::AssetKind::Audio) {
                        if ui
                            .selectable_label(*stream == asset.path, &asset.path)
                            .clicked()
                        {
                            *stream = asset.path.clone();
                            changed = true;
                        }
                    }
                });
            ui.end_row();
            super::label(ui, "Volume (dB)");
            changed |= super::opt_f32(ui, volume_db, 0.0, -80.0..=24.0, 0.1);
            ui.end_row();
        }
        EventKind::Signal { name } => changed |= text(ui, "Name", name, "explode"),
        EventKind::Visible { target, visible } => {
            changed |= text(ui, "Node", target, "Boss");
            super::label(ui, "Visible");
            changed |= ui.checkbox(visible, "").changed();
            ui.end_row();
        }
        EventKind::Fade {
            to,
            duration,
            color,
        } => {
            super::label(ui, "To (covered)");
            changed |= ui.add(egui::Slider::new(to, 0.0..=1.0)).changed();
            ui.end_row();
            super::label(ui, "Duration (s)");
            changed |= ui
                .add(egui::DragValue::new(duration).range(0.0..=60.0).speed(0.01))
                .changed();
            ui.end_row();
            let mut c = color.clone().unwrap_or_else(|| "#000000".into());
            changed |= text(ui, "Color", &mut c, "#000000");
            *color = (c != "#000000" && !c.is_empty()).then_some(c);
        }
        EventKind::Subtitle {
            text: words,
            key,
            duration,
        } => {
            changed |= text(ui, "Text", words, "What they say");
            let mut k = key.clone().unwrap_or_default();
            changed |= text(ui, "Translation key", &mut k, "dialogue.intro1");
            *key = (!k.trim().is_empty()).then(|| k.trim().to_owned());
            super::label(ui, "Duration (s)");
            changed |= ui
                .add(egui::DragValue::new(duration).range(0.1..=60.0).speed(0.01))
                .changed();
            ui.end_row();
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_engine::core::Vec3;

    #[test]
    fn overlapping_events_take_lanes_of_their_own() {
        let at = |time: f32, duration: f32| Event {
            time,
            kind: EventKind::Subtitle {
                text: "x".into(),
                key: None,
                duration,
            },
        };
        // 0–2 and 1–3 overlap; 2.5 fits after the first; an instant one at 5.
        let events = [
            at(0.0, 2.0),
            at(1.0, 2.0),
            at(2.5, 1.0),
            Event {
                time: 5.0,
                kind: EventKind::Signal { name: "go".into() },
            },
        ];
        let (lanes, count) = lanes(&events, 0.2);
        assert_eq!(lanes, [0, 1, 0, 0]);
        assert_eq!(count, 2);
    }

    #[test]
    fn camera_keys_are_recorded_and_events_sorted() {
        let mut panel = CutscenePanel::default();
        let at = |x: f32| Affine3A::from_translation(ab_engine::core::Vec3::new(x, 2.0, 0.0));
        panel.key_camera(at(0.0), 60.0);
        panel.time = 2.0;
        panel.key_camera(at(10.0), 40.0);
        // Again at the same time: replaced, not doubled.
        panel.key_camera(at(8.0), 40.0);
        assert_eq!(panel.file.camera.len(), 2);
        assert_eq!(panel.file.camera[1].position[0], 8.0);
        assert_eq!(panel.selected, Some(Selected::Camera(1)));
        panel.time = 1.0;
        let (p, _, fov) = panel.file.camera_at(1.0).unwrap();
        assert!(p.x > 0.0 && p.x < 8.0 && fov < 60.0 && fov > 40.0);
        assert!(panel.dirty);
    }

    #[test]
    fn a_new_cutscene_is_saved_and_its_node_points_at_the_file() {
        let project = std::env::temp_dir().join(format!("ab-cut-panel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(&project).unwrap();
        let mut editor = Editor::new(project.clone(), None, ab_engine::audio::AudioOutput::Silent);
        editor.select(None);
        editor.add_scene(
            ab_engine::scene_file::SceneFile {
                environment: None,
                nodes: vec![ab_engine::scene_file::NodeDesc {
                    name: "Intro".into(),
                    kind: Some(ab_engine::cutscene::CUTSCENE.into()),
                    cutscene: Some(Default::default()),
                    ..Default::default()
                }],
            },
            None,
        );
        let node = editor.selection.unwrap();
        assert_eq!(
            crate::features::Feature::of(&editor, node),
            Some(crate::features::Feature::Cutscene)
        );
        let mut panel = CutscenePanel::default();
        panel.edit(&editor, node);
        assert_eq!(panel.path, "cutscenes/intro.abcut");
        panel.key_camera(Affine3A::from_translation(Vec3::new(1.0, 2.0, 3.0)), 50.0);
        panel.file.events.push(Event {
            time: 0.5,
            kind: EventKind::Signal {
                name: "boom".into(),
            },
        });
        let node = panel.node.unwrap();
        panel.save(&mut editor, node);
        assert_eq!(panel.error, None);
        assert!(!panel.dirty);
        let written = std::fs::read_to_string(project.join("cutscenes/intro.abcut")).unwrap();
        let file = CutsceneFile::from_toml(&written).unwrap();
        assert_eq!(file, panel.file);
        let node = panel.node.unwrap();
        let player = editor
            .tree()
            .component::<CutscenePlayer>(node)
            .unwrap()
            .clone();
        assert_eq!(player.desc.file, "cutscenes/intro.abcut");
        assert_eq!(player.file.camera.len(), 1);
        // Looking through it shows the keyed shot.
        let (global, fov) = panel.preview_camera(&editor).unwrap();
        assert_eq!(fov, 50.0);
        assert!((Vec3::from(global.translation) - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-4);
        let _ = std::fs::remove_dir_all(&project);
    }
}
