//! The Animation Graph: a state machine file (see [`ab_engine::state_machine`]) as a graph.
//!
//! States are boxes, transitions arrows. Drag a box to move it; right-click the canvas to
//! add a state, a state to start a transition from it (then click where it goes), set it as
//! the entry or delete it. Select a state or a transition to change it on the right; the
//! parameters are on the left. While playing, the state machine of the node it belongs to
//! shows its current state and parameters live (they can be changed there), and the last
//! changes of state.

use super::section;
use crate::i18n::t;
use crate::state::Editor;
use crate::theme;
use ab_engine::animation::AnimationPlayer;
use ab_engine::scene::NodeHandle;
use ab_engine::state_machine::{
    BlendDesc, Compare, Condition, ParameterDesc, ParameterKind, StateDesc, StateMachine,
    StateMachineFile, TransitionDesc, ANY_STATE, STATE_MACHINE_EXTENSION,
};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use egui_phosphor::regular as icon;

const BOX: Vec2 = Vec2::new(180.0, 58.0);
/// Where the "any state" box sits in the graph, and its size.
const ANY_AT: Pos2 = Pos2::new(-240.0, -170.0);
const ANY_BOX: Vec2 = Vec2::new(150.0, 40.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selected {
    State(usize),
    Transition(usize),
}

#[derive(Debug)]
pub struct StateGraphPanel {
    /// The file, relative to the project.
    pub path: String,
    file: StateMachineFile,
    /// The file as read or saved.
    saved: StateMachineFile,
    error: Option<String>,
    selected: Option<Selected>,
    pan: Vec2,
    zoom: f32,
    /// A transition being drawn from this state (`None` inside: from any state).
    connecting: Option<Option<usize>>,
    /// A transition being dragged out of a state's port (`None` inside: any state).
    dragging_link: Option<Option<usize>>,
    /// Frame every state on the next draw.
    fit: bool,
    /// The node it was opened from (clip names, the live state while playing).
    node: Option<NodeHandle>,
}

impl Default for StateGraphPanel {
    fn default() -> Self {
        Self {
            path: format!("anims/new.{STATE_MACHINE_EXTENSION}"),
            file: StateMachineFile::default(),
            saved: StateMachineFile::default(),
            error: None,
            selected: None,
            pan: Vec2::new(300.0, 200.0),
            zoom: 1.0,
            connecting: None,
            dragging_link: None,
            fit: true,
            node: None,
        }
    }
}

/// The node with a state machine that `node` is, or is in, or holds.
fn machine_node(editor: &Editor, node: NodeHandle) -> Option<NodeHandle> {
    let tree = editor.tree();
    std::iter::successors(Some(node), |&n| tree.parent(n))
        .chain(tree.subtree(node))
        .find(|&n| tree.component::<StateMachine>(n).is_some())
}

/// [`starter`] with the clips named as the character's are (`idle`, `Walking`, `Armature|Run`
/// all match); states whose clip it lacks keep the usual name, to set by hand.
pub fn starter_for(clips: &[String]) -> StateMachineFile {
    let mut file = starter();
    let find = |wanted: &str| {
        let wanted = wanted.to_lowercase();
        clips
            .iter()
            .find(|c| c.to_lowercase() == wanted)
            .or_else(|| clips.iter().find(|c| c.to_lowercase().contains(&wanted)))
            .cloned()
    };
    for state in &mut file.states {
        if let Some(clip) = &mut state.clip {
            if let Some(found) = find(clip) {
                *clip = found;
            }
        }
        if let Some(blend) = &mut state.blend {
            for (clip, _) in &mut blend.clips {
                if let Some(found) = find(clip) {
                    *clip = found;
                }
            }
        }
    }
    file
}

/// A first state machine for a model: idle, moving by speed, jumping.
pub fn starter() -> StateMachineFile {
    StateMachineFile::from_toml(
        r#"entry = "Idle"
fade = 0.2

[[parameter]]
name = "speed"
[[parameter]]
name = "jump"
kind = "trigger"

[[state]]
name = "Idle"
clip = "Idle"
position = [0.0, 0.0]
[[state]]
name = "Move"
blend = { parameter = "speed", clips = [["Walk", 1.5], ["Run", 4.2]] }
position = [240.0, 0.0]
[[state]]
name = "Jump"
clip = "Jump"
looping = false
position = [120.0, 140.0]

[[transition]]
from = "Idle"
to = "Move"
when = [{ parameter = "speed", op = ">", value = 0.1 }]
[[transition]]
from = "Move"
to = "Idle"
when = [{ parameter = "speed", op = "<=", value = 0.1 }]
[[transition]]
from = "*"
to = "Jump"
when = [{ parameter = "jump" }]
[[transition]]
from = "Jump"
to = "Idle"
exit_time = 0.9
"#,
    )
    .expect("the starter state machine is valid")
}

impl StateGraphPanel {
    pub fn is_dirty(&self) -> bool {
        self.file != self.saved
    }

    /// Opens the state machine of `node` (it, the node it is in, or one in it), if it has
    /// one and it is another file.
    pub fn follow(&mut self, editor: &Editor, node: NodeHandle) {
        let Some(owner) = machine_node(editor, node) else {
            // A character without one yet: a new file named after it, its clips offered.
            let tree = editor.tree();
            let character = std::iter::successors(Some(node), |&n| tree.parent(n))
                .take_while(|&n| n != editor.scene_root)
                .find(|&n| {
                    tree.subtree(n)
                        .into_iter()
                        .any(|m| tree.component::<AnimationPlayer>(m).is_some())
                });
            if let Some(character) = character {
                if self.node != Some(character) && !self.is_dirty() && self.file.states.is_empty() {
                    let name = tree
                        .name(character)
                        .map(|n| n.to_lowercase().replace(' ', "_"))
                        .unwrap_or_else(|| "new".into());
                    self.path = format!("anims/{name}.{STATE_MACHINE_EXTENSION}");
                    self.node = Some(character);
                }
            }
            return;
        };
        self.node = Some(owner);
        let path = editor
            .tree()
            .component::<StateMachine>(owner)
            .map(|m| m.file.clone())
            .unwrap_or_default();
        if path != self.path && !self.is_dirty() {
            self.open(editor, &path);
        }
    }

    pub fn open(&mut self, editor: &Editor, path: &str) {
        let file = std::fs::read_to_string(editor.project().join(path))
            .map_err(|e| format!("{path}: {e}"))
            .and_then(|text| {
                StateMachineFile::from_toml(&text).map_err(|e| format!("{path}: {e}"))
            });
        match file {
            Ok(mut file) => {
                // Written without positions: laid out (in both copies, so this alone does
                // not count as a change).
                if overlapping(&file) {
                    arrange(&mut file);
                }
                self.saved = file.clone();
                self.file = file;
                self.error = None;
                self.fit = true;
            }
            Err(err) => self.error = Some(err),
        }
        self.path = path.to_owned();
        self.selected = None;
        self.connecting = None;
    }

    /// Writes the file; nodes using it read it again.
    fn save(&mut self, editor: &mut Editor) {
        if let Err(err) = self.file.check() {
            self.error = Some(err);
            return;
        }
        let mut path = self.path.trim().replace('\\', "/");
        if !path.ends_with(&format!(".{STATE_MACHINE_EXTENSION}")) {
            path = format!("{path}.{STATE_MACHINE_EXTENSION}");
        }
        let full = editor.project().join(&path);
        if let Err(err) = editor.write_project_file(&full, self.file.to_toml()) {
            self.error = Some(format!("{}: {err}", full.display()));
            return;
        }
        self.path = path.clone();
        self.saved = self.file.clone();
        self.error = None;
        // Every node using it plays the new version.
        let users: Vec<NodeHandle> = editor
            .tree()
            .world()
            .query::<(ab_engine::ecs::Entity, &StateMachine)>()
            .iter()
            .filter(|(_, m)| m.file == path)
            .map(|(e, _)| NodeHandle::from_entity(e))
            .collect();
        for node in users {
            let fresh = StateMachine::new(path.clone(), self.file.clone());
            let _ = editor.tree_mut().insert_component(node, fresh);
        }
        editor.assets.request_rescan();
        editor.info(format!("Saved {path}"));
    }

    /// Gives the selected node this state machine.
    fn assign(&mut self, editor: &mut Editor) {
        let Some(node) = editor.selection else {
            return;
        };
        let Some(old) = editor.describe_node(node) else {
            return;
        };
        let mut new = old.clone();
        new.state_machine = Some(self.path.clone());
        if new != old {
            editor.apply_desc(node, &old, new);
            editor.end_gesture();
        }
        self.node = editor.selection;
    }

    /// The clips of the player the state machine drives.
    fn clip_names(&self, editor: &Editor) -> Vec<String> {
        let tree = editor.tree();
        let Some(node) = self.node.filter(|&n| tree.is_valid(n)) else {
            return Vec::new();
        };
        tree.subtree(node)
            .into_iter()
            .find_map(|n| {
                tree.component::<AnimationPlayer>(n)
                    .map(|p| p.clips.iter().map(|(name, _)| name.clone()).collect())
            })
            .unwrap_or_default()
    }

    /// The machine running on the node, while playing.
    fn live(&self, editor: &Editor) -> Option<StateMachine> {
        if !editor.is_playing() {
            return None;
        }
        let node = self.node?;
        editor
            .tree()
            .component::<StateMachine>(node)
            .filter(|m| m.file == self.path)
            .map(|m| m.clone())
    }

    pub fn show(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        if let Some(selected) = editor.selection {
            self.follow(editor, selected);
        }
        let live = self.live(editor);
        if live.is_some() {
            ui.ctx().request_repaint();
        }
        self.toolbar(ui, editor);
        ui.separator();
        egui::Panel::left("state-graph-params")
            .resizable(true)
            .default_size(240.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .show(ui, |ui| self.parameters(ui, editor, live.as_ref()));
            });
        egui::Panel::right("state-graph-details")
            .resizable(true)
            .default_size(300.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.details(ui, editor));
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::BG_0))
            .show(ui, |ui| {
                if self.file.states.is_empty() {
                    self.empty_canvas(ui, editor);
                } else {
                    self.canvas(ui, live.as_ref());
                }
            });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(icon::GRAPH).font(theme::bold(14.0)));
            egui::ComboBox::from_id_salt("state_graph_file")
                .selected_text(&self.path)
                .width(220.0)
                .show_ui(ui, |ui| {
                    let files: Vec<String> = editor
                        .assets
                        .of_kind(crate::project::AssetKind::StateMachine)
                        .map(|a| a.path.clone())
                        .collect();
                    for path in files {
                        if ui.selectable_label(path == self.path, &path).clicked() {
                            self.open(editor, &path);
                        }
                    }
                });
            ui.add(egui::TextEdit::singleline(&mut self.path).desired_width(180.0))
                .on_hover_text(t("state_graph.path_hint"));
            if ui
                .button(format!("{}  {}", icon::FILE_PLUS, t("state_graph.new")))
                .on_hover_text(t("state_graph.new_hint"))
                .clicked()
            {
                self.file = starter_for(&self.clip_names(editor));
                self.selected = None;
                self.fit = true;
            }
            let save = format!(
                "{}  {}{}",
                icon::FLOPPY_DISK,
                t("dialog.save"),
                if self.is_dirty() { " •" } else { "" }
            );
            if ui
                .add_enabled(!editor.is_playing(), egui::Button::new(save))
                .clicked()
            {
                self.save(editor);
            }
            let can_assign = editor.selection.is_some() && !editor.is_playing();
            if ui
                .add_enabled(
                    can_assign,
                    egui::Button::new(format!("{}  {}", icon::LINK, t("state_graph.assign"))),
                )
                .on_hover_text(t("state_graph.assign_hint"))
                .clicked()
            {
                self.assign(editor);
            }
            ui.separator();
            if ui
                .button(format!(
                    "{}  {}",
                    icon::TREE_STRUCTURE,
                    t("state_graph.arrange")
                ))
                .on_hover_text(t("state_graph.arrange_hint"))
                .clicked()
            {
                arrange(&mut self.file);
                self.fit = true;
            }
            if ui
                .button(format!("{}  {}", icon::FRAME_CORNERS, t("state_graph.fit")))
                .on_hover_text(t("state_graph.fit_hint"))
                .clicked()
            {
                self.fit = true;
            }
            if self.connecting.is_some() {
                ui.label(RichText::new(t("state_graph.connecting")).color(theme::WARN));
                if ui.button(t("dialog.cancel")).clicked() {
                    self.connecting = None;
                }
            }
        });
        let problem = self.error.clone().or_else(|| self.file.check().err());
        if let Some(error) = problem {
            ui.label(RichText::new(format!("{}  {error}", icon::WARNING)).color(theme::WARN));
        }
    }

    fn parameters(&mut self, ui: &mut egui::Ui, editor: &mut Editor, live: Option<&StateMachine>) {
        section(
            ui,
            "sm_params",
            icon::SLIDERS,
            &t("state_graph.parameters"),
            |ui| {
                let mut remove = None;
                for (i, p) in self.file.parameters.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(90.0));
                        egui::ComboBox::from_id_salt(("sm_kind", i))
                            .selected_text(kind_name(p.kind))
                            .width(64.0)
                            .show_ui(ui, |ui| {
                                for kind in [
                                    ParameterKind::Float,
                                    ParameterKind::Bool,
                                    ParameterKind::Trigger,
                                ] {
                                    ui.selectable_value(&mut p.kind, kind, kind_name(kind));
                                }
                            });
                        if p.kind == ParameterKind::Float {
                            ui.add(egui::DragValue::new(&mut p.default).speed(0.01));
                        } else if p.kind == ParameterKind::Bool {
                            let mut on = p.default > 0.5;
                            if ui.checkbox(&mut on, "").changed() {
                                p.default = on as u32 as f32;
                            }
                        }
                        if ui.small_button(icon::X).clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    self.file.parameters.remove(i);
                }
                if ui
                    .button(format!(
                        "{}  {}",
                        icon::PLUS,
                        t("state_graph.add_parameter")
                    ))
                    .clicked()
                {
                    let name = (1..)
                        .map(|n| format!("param{n}"))
                        .find(|n| self.file.parameter(n).is_none())
                        .expect("a free name");
                    self.file.parameters.push(ParameterDesc {
                        name,
                        kind: ParameterKind::Float,
                        default: 0.0,
                    });
                }
            },
        );
        let Some(live) = live else {
            ui.label(
                RichText::new(t("state_graph.live_hint"))
                    .small()
                    .color(theme::TEXT_DIM),
            );
            return;
        };
        section(ui, "sm_live", icon::PULSE, &t("state_graph.live"), |ui| {
            ui.label(format!(
                "{}  {}  ({:.1} s)",
                icon::PLAY,
                live.state_name().unwrap_or("—"),
                live.state_time
            ));
            let mut changes = Vec::new();
            for (p, value) in live.desc.parameters.iter().zip(&live.values) {
                ui.horizontal(|ui| {
                    ui.label(&p.name);
                    let mut v = *value;
                    let changed = match p.kind {
                        ParameterKind::Float => {
                            ui.add(egui::DragValue::new(&mut v).speed(0.02)).changed()
                        }
                        ParameterKind::Bool => {
                            let mut on = v > 0.5;
                            let c = ui.checkbox(&mut on, "").changed();
                            v = on as u32 as f32;
                            c
                        }
                        ParameterKind::Trigger => {
                            let fired = ui.button(t("state_graph.fire")).clicked();
                            if fired {
                                v = 1.0;
                            }
                            fired
                        }
                    };
                    if changed {
                        changes.push((p.name.clone(), v));
                    }
                });
            }
            if let Some(node) = self.node {
                if let Some(mut machine) = editor.tree_mut().component_mut::<StateMachine>(node) {
                    for (name, v) in changes {
                        machine.set(&name, v);
                    }
                }
            }
            ui.add_space(4.0);
            ui.label(RichText::new(t("state_graph.history")).color(theme::TEXT_DIM));
            for change in live.history.iter().rev() {
                ui.label(
                    RichText::new(format!(
                        "{:>6.2}s  {} → {}",
                        change.at,
                        change.from.as_deref().unwrap_or("·"),
                        change.to
                    ))
                    .monospace()
                    .small(),
                );
            }
        });
    }

    fn details(&mut self, ui: &mut egui::Ui, editor: &Editor) {
        let clips = self.clip_names(editor);
        let states: Vec<String> = self.file.states.iter().map(|s| s.name.clone()).collect();
        let params: Vec<(String, ParameterKind)> = self
            .file
            .parameters
            .iter()
            .map(|p| (p.name.clone(), p.kind))
            .collect();
        match self.selected {
            Some(Selected::State(i)) if i < self.file.states.len() => {
                let mut rename = None;
                let entry = self.file.entry.clone().or_else(|| states.first().cloned());
                let mut make_entry = false;
                let mut delete = false;
                let state = &mut self.file.states[i];
                section(
                    ui,
                    "sm_state",
                    icon::SQUARE,
                    &t("state_graph.state"),
                    |ui| {
                        super::grid(ui, "sm_state_grid", |ui| {
                            super::label(ui, "Name");
                            let old = state.name.clone();
                            if ui.text_edit_singleline(&mut state.name).changed() {
                                rename = Some((old, state.name.clone()));
                            }
                            ui.end_row();
                            super::label(ui, "Plays");
                            let mut blended = state.blend.is_some();
                            ui.horizontal(|ui| {
                                if ui.selectable_label(!blended, "Clip").clicked() {
                                    blended = false;
                                }
                                if ui.selectable_label(blended, "Blend").clicked() {
                                    blended = true;
                                }
                            });
                            if blended && state.blend.is_none() {
                                let parameter =
                                    params.first().map(|p| p.0.clone()).unwrap_or_default();
                                let clip = state.clip.take().unwrap_or_default();
                                state.blend = Some(BlendDesc {
                                    parameter,
                                    clips: vec![(clip, 0.0)],
                                });
                            } else if !blended && state.blend.is_some() {
                                state.clip = state
                                    .blend
                                    .take()
                                    .and_then(|b| b.clips.first().map(|c| c.0.clone()));
                            }
                            ui.end_row();
                            if let Some(clip) = &mut state.clip {
                                super::label(ui, "Clip");
                                clip_choice(ui, ("sm_clip", i), clip, &clips);
                                ui.end_row();
                            } else if state.clip.is_none() && state.blend.is_none() {
                                state.clip = Some(String::new());
                            }
                            if let Some(blend) = &mut state.blend {
                                super::label(ui, "By parameter");
                                egui::ComboBox::from_id_salt(("sm_blend_param", i))
                                    .selected_text(&blend.parameter)
                                    .show_ui(ui, |ui| {
                                        for (p, _) in &params {
                                            ui.selectable_value(&mut blend.parameter, p.clone(), p);
                                        }
                                    });
                                ui.end_row();
                                let mut remove = None;
                                for (k, (clip, at)) in blend.clips.iter_mut().enumerate() {
                                    super::label(ui, if k == 0 { "Clips at" } else { "" });
                                    ui.horizontal(|ui| {
                                        clip_choice(ui, ("sm_blend_clip", i, k), clip, &clips);
                                        ui.add(egui::DragValue::new(at).speed(0.05));
                                        if ui.small_button(icon::X).clicked() {
                                            remove = Some(k);
                                        }
                                    });
                                    ui.end_row();
                                }
                                if let Some(k) = remove.filter(|_| blend.clips.len() > 1) {
                                    blend.clips.remove(k);
                                }
                                super::label(ui, "");
                                if ui.small_button(format!("{}  Clip", icon::PLUS)).clicked() {
                                    let next = blend.clips.last().map_or(0.0, |c| c.1 + 1.0);
                                    blend.clips.push((String::new(), next));
                                }
                                ui.end_row();
                                blend.clips.sort_by(|a, b| a.1.total_cmp(&b.1));
                            }
                            super::label(ui, "Speed");
                            ui.add(
                                egui::DragValue::new(&mut state.speed)
                                    .range(-10.0..=10.0)
                                    .speed(0.01),
                            );
                            ui.end_row();
                            super::label(ui, "Loop");
                            ui.checkbox(&mut state.looping, "");
                            ui.end_row();
                        });
                        ui.horizontal(|ui| {
                            let is_entry = entry.as_deref() == Some(state.name.as_str());
                            if ui
                                .add_enabled(
                                    !is_entry,
                                    egui::Button::new(format!(
                                        "{}  {}",
                                        icon::FLAG,
                                        t("state_graph.make_entry")
                                    )),
                                )
                                .clicked()
                            {
                                make_entry = true;
                            }
                            if ui
                                .button(
                                    RichText::new(format!(
                                        "{}  {}",
                                        icon::TRASH,
                                        t("state_graph.delete")
                                    ))
                                    .color(theme::ERROR),
                                )
                                .clicked()
                            {
                                delete = true;
                            }
                        });
                    },
                );
                if let Some((old, new)) = rename {
                    self.rename_state(&old, &new);
                }
                if make_entry {
                    self.file.entry = Some(self.file.states[i].name.clone());
                }
                if delete {
                    self.delete_state(i);
                }
            }
            Some(Selected::Transition(i)) if i < self.file.transitions.len() => {
                let mut delete = false;
                let tr = &mut self.file.transitions[i];
                section(
                    ui,
                    "sm_transition",
                    icon::ARROW_RIGHT,
                    &t("state_graph.transition"),
                    |ui| {
                        super::grid(ui, "sm_transition_grid", |ui| {
                            super::label(ui, "From");
                            state_choice(ui, ("sm_from", i), &mut tr.from, &states, true);
                            ui.end_row();
                            super::label(ui, "To");
                            state_choice(ui, ("sm_to", i), &mut tr.to, &states, false);
                            ui.end_row();
                            super::label(ui, "Exit time");
                            super::opt_f32(ui, &mut tr.exit_time, 0.9, 0.0..=1.0, 0.01);
                            ui.end_row();
                            super::label(ui, "Fade (s)");
                            super::opt_f32(ui, &mut tr.fade, 0.2, 0.0..=5.0, 0.01);
                            ui.end_row();
                        });
                        ui.add_space(4.0);
                        ui.label(RichText::new(t("state_graph.conditions")).color(theme::TEXT_DIM));
                        let mut remove = None;
                        for (k, c) in tr.when.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt(("sm_cond", i, k))
                                    .selected_text(&c.parameter)
                                    .width(90.0)
                                    .show_ui(ui, |ui| {
                                        for (p, _) in &params {
                                            ui.selectable_value(&mut c.parameter, p.clone(), p);
                                        }
                                    });
                                match params.iter().find(|p| p.0 == c.parameter).map(|p| p.1) {
                                    Some(ParameterKind::Trigger) => {
                                        ui.label(RichText::new("fired").color(theme::TEXT_DIM));
                                        c.op = None;
                                        c.value = None;
                                    }
                                    Some(ParameterKind::Bool) => {
                                        let mut on = c.value.unwrap_or(1.0) > 0.5;
                                        ui.checkbox(&mut on, "is on");
                                        c.op = None;
                                        c.value = (!on).then_some(0.0);
                                    }
                                    _ => {
                                        let mut op = c.op.unwrap_or(Compare::Greater);
                                        egui::ComboBox::from_id_salt(("sm_op", i, k))
                                            .selected_text(op.symbol())
                                            .width(44.0)
                                            .show_ui(ui, |ui| {
                                                for o in Compare::ALL {
                                                    ui.selectable_value(&mut op, o, o.symbol());
                                                }
                                            });
                                        c.op = Some(op);
                                        let mut v = c.value.unwrap_or(0.0);
                                        ui.add(egui::DragValue::new(&mut v).speed(0.01));
                                        c.value = Some(v);
                                    }
                                }
                                if ui.small_button(icon::X).clicked() {
                                    remove = Some(k);
                                }
                            });
                        }
                        if let Some(k) = remove {
                            tr.when.remove(k);
                        }
                        if ui
                            .add_enabled(
                                !params.is_empty(),
                                egui::Button::new(format!(
                                    "{}  {}",
                                    icon::PLUS,
                                    t("state_graph.add_condition")
                                )),
                            )
                            .clicked()
                        {
                            tr.when.push(Condition {
                                parameter: params[0].0.clone(),
                                op: None,
                                value: None,
                            });
                        }
                        ui.add_space(4.0);
                        if ui
                            .button(
                                RichText::new(format!(
                                    "{}  {}",
                                    icon::TRASH,
                                    t("state_graph.delete")
                                ))
                                .color(theme::ERROR),
                            )
                            .clicked()
                        {
                            delete = true;
                        }
                    },
                );
                if delete {
                    self.file.transitions.remove(i);
                    self.selected = None;
                }
            }
            _ => {
                ui.label(RichText::new(t("state_graph.hint")).color(theme::TEXT_DIM));
                ui.add_space(6.0);
                super::grid(ui, "sm_file_grid", |ui| {
                    super::label(ui, "Default fade (s)");
                    ui.add(
                        egui::DragValue::new(&mut self.file.fade)
                            .range(0.0..=5.0)
                            .speed(0.01),
                    );
                    ui.end_row();
                });
            }
        }
    }

    fn rename_state(&mut self, old: &str, new: &str) {
        for tr in &mut self.file.transitions {
            if tr.from == old {
                tr.from = new.to_owned();
            }
            if tr.to == old {
                tr.to = new.to_owned();
            }
        }
        if self.file.entry.as_deref() == Some(old) {
            self.file.entry = Some(new.to_owned());
        }
    }

    fn delete_state(&mut self, i: usize) {
        let name = self.file.states.remove(i).name;
        self.file
            .transitions
            .retain(|t| t.from != name && t.to != name);
        if self.file.entry.as_deref() == Some(name.as_str()) {
            self.file.entry = None;
        }
        self.selected = None;
    }

    fn add_state(&mut self, at: Pos2) -> usize {
        let name = (1..)
            .map(|n| format!("State{n}"))
            .find(|n| self.file.state(n).is_none())
            .expect("a free name");
        self.file.states.push(StateDesc {
            name,
            clip: Some(String::new()),
            blend: None,
            speed: 1.0,
            looping: true,
            position: [at.x.round(), at.y.round()],
        });
        self.file.states.len() - 1
    }

    /// No states yet: what to do, and a start from the character's own clips.
    fn empty_canvas(&mut self, ui: &mut egui::Ui, editor: &Editor) {
        let clips = self.clip_names(editor);
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.label(RichText::new(icon::GRAPH).size(42.0).color(theme::TEXT_DIM));
            ui.label(RichText::new(t("state_graph.empty")).color(theme::TEXT_DIM));
            ui.add_space(8.0);
            if ui
                .button(
                    RichText::new(format!("{}  {}", icon::MAGIC_WAND, t("state_graph.start")))
                        .strong(),
                )
                .on_hover_text(t("state_graph.new_hint"))
                .clicked()
            {
                self.file = starter_for(&clips);
                self.selected = None;
                self.fit = true;
            }
            if ui
                .button(format!("{}  {}", icon::PLUS, t("state_graph.add_state")))
                .clicked()
            {
                let i = self.add_state(Pos2::ZERO);
                if let Some(clip) = clips.first() {
                    self.file.states[i].clip = Some(clip.clone());
                }
                self.selected = Some(Selected::State(i));
            }
            if !clips.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("{}: {}", t("state_graph.clips"), clips.join(", ")))
                        .small()
                        .color(theme::TEXT_DIM),
                );
            }
        });
    }

    fn canvas(&mut self, ui: &mut egui::Ui, live: Option<&StateMachine>) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        if std::mem::take(&mut self.fit) {
            self.fit_view(rect);
        }
        let zoom = self.zoom;
        let origin = rect.min + self.pan;
        let to_screen = |p: Pos2| origin + p.to_vec2() * zoom;
        let to_graph = |p: Pos2| Pos2::ZERO + (p - origin) / zoom;
        // Dot grid: present, not in the way.
        let step = 32.0 * zoom;
        if step > 8.0 {
            let mut x = rect.left() + (origin.x - rect.left()).rem_euclid(step);
            while x < rect.right() {
                let mut y = rect.top() + (origin.y - rect.top()).rem_euclid(step);
                while y < rect.bottom() {
                    painter.circle_filled(Pos2::new(x, y), 1.0, theme::BG_3);
                    y += step;
                }
                x += step;
            }
        }
        let box_at =
            |p: [f32; 2]| Rect::from_min_size(to_screen(Pos2::new(p[0], p[1])), BOX * zoom);
        let any_box = Rect::from_min_size(to_screen(ANY_AT), ANY_BOX * zoom);
        let rect_of = |name: &str, file: &StateMachineFile| -> Option<Rect> {
            if name == ANY_STATE {
                return Some(any_box);
            }
            file.state(name).map(|i| box_at(file.states[i].position))
        };
        let current = live.and_then(|m| m.state_name().map(str::to_owned));
        let pointer = response.hover_pos();
        let font = |size: f32| egui::FontId::proportional((size * zoom).clamp(8.0, 30.0));

        // Transitions: curves from edge to edge; two ways between the same states bend
        // apart, each with its head on the state it goes to and its conditions on a pill.
        let mut hovered_transition = None;
        let mut curves = Vec::with_capacity(self.file.transitions.len());
        for tr in &self.file.transitions {
            let (Some(a), Some(b)) = (rect_of(&tr.from, &self.file), rect_of(&tr.to, &self.file))
            else {
                curves.push(None);
                continue;
            };
            let both_ways = self
                .file
                .transitions
                .iter()
                .any(|o| o.from == tr.to && o.to == tr.from);
            let curve = transition_curve(a, b, if both_ways { 28.0 * zoom } else { 0.0 });
            if let Some(p) = pointer {
                if curve.distance(p) < 7.0 {
                    hovered_transition = Some(curves.len());
                }
            }
            curves.push(Some(curve));
        }
        for (i, (tr, curve)) in self.file.transitions.iter().zip(&curves).enumerate() {
            let Some(curve) = curve else { continue };
            let selected = self.selected == Some(Selected::Transition(i));
            let hovered = hovered_transition == Some(i);
            let from_live = current.as_deref() == Some(tr.from.as_str())
                || tr.from == ANY_STATE && current.is_some();
            let color = if selected {
                Color32::WHITE
            } else if hovered {
                theme::ACCENT
            } else if from_live {
                theme::PLAY.gamma_multiply(0.8)
            } else {
                EDGE
            };
            let width = if selected || hovered { 2.5 } else { 1.75 } * zoom.max(0.6);
            painter.add(egui::Shape::line(
                curve.points(24),
                Stroke::new(width, color),
            ));
            let (tip, direction) = curve.end();
            arrow_head(&painter, tip, direction, 11.0 * zoom.max(0.6), color);
            let label = condition_label(tr);
            if !label.is_empty() && zoom > 0.45 {
                let at = curve.at(0.5);
                let galley = painter.layout_no_wrap(
                    label,
                    font(11.0),
                    if selected {
                        Color32::WHITE
                    } else {
                        theme::TEXT
                    },
                );
                let pill = Rect::from_center_size(
                    at,
                    galley.size() + Vec2::new(12.0, 4.0) * zoom.max(0.6),
                );
                painter.rect(
                    pill,
                    pill.height() * 0.5,
                    theme::BG_3,
                    Stroke::new(1.0_f32, color.gamma_multiply(0.7)),
                    egui::StrokeKind::Inside,
                );
                painter.galley(pill.center() - galley.size() * 0.5, galley, theme::TEXT);
            }
        }

        // The "any state" box: transitions from it can leave whatever state is playing.
        let any_hover = pointer.is_some_and(|p| any_box.contains(p));
        painter.rect(
            any_box,
            8.0 * zoom,
            theme::BG_1,
            Stroke::new(
                1.5_f32,
                if any_hover {
                    theme::ACCENT
                } else {
                    theme::BORDER
                },
            ),
            egui::StrokeKind::Inside,
        );
        painter.text(
            any_box.center(),
            egui::Align2::CENTER_CENTER,
            format!("{}  {}", icon::ASTERISK, t("state_graph.any_state")),
            font(13.0),
            theme::TEXT_DIM,
        );
        let any_port = Pos2::new(any_box.right(), any_box.center().y);
        let any_hit = ui.interact(any_box, ui.id().with("sm_any"), Sense::click_and_drag());

        // States.
        let entry = self
            .file
            .entry
            .clone()
            .or_else(|| self.file.states.first().map(|s| s.name.clone()));
        let mut moved = None;
        let mut clicked_state = None;
        let mut released_on = None;
        let mut menu: Option<(usize, StateMenu)> = None;
        let mut drag_from: Option<Option<usize>> = None;
        for (i, s) in self.file.states.iter().enumerate() {
            let r = box_at(s.position);
            let selected = self.selected == Some(Selected::State(i));
            let active = current.as_deref() == Some(s.name.as_str());
            let is_entry = entry.as_deref() == Some(s.name.as_str());
            let blend = s.blend.is_some();
            let hovered = pointer.is_some_and(|p| r.expand(6.0).contains(p));
            let accent = if blend { BLEND } else { theme::ACCENT };
            // Shadow, body, colored band on the left.
            painter.rect_filled(
                r.translate(Vec2::new(0.0, 3.0 * zoom)),
                8.0 * zoom,
                Color32::from_black_alpha(90),
            );
            let fill = if active {
                theme::PLAY.gamma_multiply(0.30)
            } else if hovered {
                theme::BG_3
            } else {
                theme::BG_2
            };
            let border = if selected {
                Stroke::new(2.0_f32, Color32::WHITE)
            } else if active {
                Stroke::new(2.0_f32, theme::PLAY)
            } else {
                Stroke::new(1.0_f32, theme::BORDER)
            };
            painter.rect(r, 8.0 * zoom, fill, border, egui::StrokeKind::Inside);
            let band = Rect::from_min_size(r.min, Vec2::new(5.0 * zoom, r.height()));
            painter.rect_filled(
                band,
                egui::CornerRadius {
                    nw: (8.0 * zoom) as u8,
                    sw: (8.0 * zoom) as u8,
                    ne: 0,
                    se: 0,
                },
                if is_entry { theme::WARN } else { accent },
            );
            let left = r.left() + 14.0 * zoom;
            let title = if is_entry {
                format!("{}  {}", icon::FLAG, s.name)
            } else {
                s.name.clone()
            };
            painter.text(
                Pos2::new(left, r.top() + 17.0 * zoom),
                egui::Align2::LEFT_CENTER,
                title,
                egui::FontId::new(
                    (14.0 * zoom).clamp(8.0, 32.0),
                    egui::FontFamily::Name(crate::fonts::BOLD.into()),
                ),
                theme::TEXT,
            );
            let sub = match (&s.clip, &s.blend) {
                (_, Some(b)) => format!(
                    "{}  {} · {}",
                    icon::CHART_LINE,
                    b.clips
                        .iter()
                        .map(|c| c.0.as_str())
                        .collect::<Vec<_>>()
                        .join(" / "),
                    b.parameter
                ),
                (Some(clip), None) if !clip.is_empty() => format!("{}  {clip}", icon::FILM_STRIP),
                _ => format!("{}  {}", icon::WARNING, t("state_graph.no_clip")),
            };
            painter.with_clip_rect(r.shrink(2.0)).text(
                Pos2::new(left, r.bottom() - 15.0 * zoom),
                egui::Align2::LEFT_CENTER,
                sub,
                font(11.0),
                theme::TEXT_DIM,
            );
            if active {
                if let Some(m) = live {
                    let progress = (m.state_time % 1.0).clamp(0.0, 1.0);
                    let bar = Rect::from_min_size(
                        Pos2::new(r.left() + 6.0 * zoom, r.bottom() - 4.0 * zoom),
                        Vec2::new((r.width() - 12.0 * zoom) * progress, 2.5 * zoom),
                    );
                    painter.rect_filled(bar, 1.0, theme::PLAY);
                }
            }
            // The port on the right: drag from it to another state for a transition.
            let port = Pos2::new(r.right(), r.center().y);
            let port_hot = pointer.is_some_and(|p| p.distance(port) < 9.0 * zoom.max(0.7));
            if hovered || port_hot || selected {
                painter.circle(
                    port,
                    5.5 * zoom.max(0.7),
                    if port_hot { theme::ACCENT } else { theme::BG_4 },
                    Stroke::new(1.5_f32, theme::BG_0),
                );
            }
            let port_hit = ui.interact(
                Rect::from_center_size(port, Vec2::splat(18.0 * zoom.max(0.7))),
                ui.id().with(("sm_port", i)),
                Sense::drag(),
            );
            if port_hit.drag_started() {
                drag_from = Some(Some(i));
            }
            if port_hit.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            let hit = ui.interact(r, ui.id().with(("sm_state", i)), Sense::click_and_drag());
            if hit.clicked() {
                clicked_state = Some(i);
            }
            if hit.double_clicked() {
                self.selected = Some(Selected::State(i));
            }
            if hit.dragged() && self.dragging_link.is_none() {
                moved = Some((i, hit.drag_delta() / zoom));
            }
            if hovered {
                released_on = Some(i);
            }
            hit.context_menu(|ui| {
                for (action, glyph, key) in [
                    (
                        StateMenu::Connect,
                        icon::ARROW_RIGHT,
                        "state_graph.add_transition",
                    ),
                    (StateMenu::Entry, icon::FLAG, "state_graph.make_entry"),
                    (StateMenu::Delete, icon::TRASH, "state_graph.delete"),
                ] {
                    if ui.button(format!("{glyph}  {}", t(key))).clicked() {
                        menu = Some((i, action));
                        ui.close();
                    }
                }
            });
        }
        if any_hover {
            painter.circle(
                any_port,
                5.5 * zoom.max(0.7),
                theme::BG_4,
                Stroke::new(1.5_f32, theme::BG_0),
            );
        }
        if any_hit.drag_started() {
            drag_from = Some(None);
        }
        if let Some(from) = drag_from {
            self.dragging_link = Some(from);
        }
        match menu {
            Some((i, StateMenu::Connect)) => self.connecting = Some(Some(i)),
            Some((i, StateMenu::Entry)) => self.file.entry = Some(self.file.states[i].name.clone()),
            Some((i, StateMenu::Delete)) => {
                self.delete_state(i);
                return;
            }
            None => {}
        }
        any_hit.context_menu(|ui| {
            if ui
                .button(format!(
                    "{}  {}",
                    icon::ARROW_RIGHT,
                    t("state_graph.add_transition")
                ))
                .clicked()
            {
                self.connecting = Some(None);
                ui.close();
            }
        });
        if let Some((i, d)) = moved {
            let p = &mut self.file.states[i].position;
            p[0] += d.x;
            p[1] += d.y;
        }

        // A link being dragged from a port: a curve to the pointer, made on release.
        if let Some(from) = self.dragging_link {
            let start = from.map_or(any_port, |f| {
                let r = box_at(self.file.states[f].position);
                Pos2::new(r.right(), r.center().y)
            });
            if let Some(p) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                let curve = Curve::new(
                    start,
                    start + Vec2::new(60.0 * zoom, 0.0),
                    p - Vec2::new(60.0 * zoom, 0.0),
                    p,
                );
                painter.add(egui::Shape::line(
                    curve.points(24),
                    Stroke::new(2.0_f32, theme::WARN),
                ));
                arrow_head(&painter, p, curve.end().1, 10.0, theme::WARN);
            }
            ui.ctx().request_repaint();
            if ui.input(|i| i.pointer.any_released()) {
                self.dragging_link = None;
                if let Some(to) = released_on {
                    self.link(from, to);
                }
            }
        }

        // Clicks: finish a transition, or select.
        if let Some(i) = clicked_state {
            match self.connecting.take() {
                Some(from) => self.link(from, i),
                None => self.selected = Some(Selected::State(i)),
            }
        } else if let (true, Some(i)) = (response.clicked(), hovered_transition) {
            self.selected = Some(Selected::Transition(i));
        } else if response.clicked() {
            self.selected = None;
            self.connecting = None;
        }
        // Right-click a transition: delete it.
        if let (true, Some(i)) = (response.secondary_clicked(), hovered_transition) {
            self.selected = Some(Selected::Transition(i));
        }
        // While connecting by click, a line follows the pointer.
        if let (Some(from), Some(p)) = (self.connecting, response.hover_pos()) {
            let start = from.map_or(any_port, |f| box_at(self.file.states[f].position).center());
            painter.line_segment([start, p], Stroke::new(1.5_f32, theme::WARN));
            ui.ctx().request_repaint();
        }
        // Pan (drag the background) and zoom (wheel, around the pointer).
        if response.dragged() && moved.is_none() && self.dragging_link.is_none() {
            self.pan += response.drag_delta();
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let before = response.hover_pos().map(to_graph);
                self.zoom = (self.zoom * (1.0 + scroll / 400.0)).clamp(0.3, 2.5);
                if let (Some(g), Some(p)) = (before, response.hover_pos()) {
                    self.pan = p - rect.min - g.to_vec2() * self.zoom;
                }
            }
        }
        // Double-click empty space: a state there.
        if response.double_clicked() && hovered_transition.is_none() {
            if let Some(p) = response.interact_pointer_pos() {
                let at = to_graph(p) - BOX * 0.5;
                let i = self.add_state(at);
                self.selected = Some(Selected::State(i));
            }
        }
        let mut add_at = None;
        response.context_menu(|ui| {
            if let Some(i) = hovered_transition.or(match self.selected {
                Some(Selected::Transition(i)) => Some(i),
                _ => None,
            }) {
                if ui
                    .button(format!("{}  {}", icon::TRASH, t("state_graph.delete")))
                    .clicked()
                {
                    self.file.transitions.remove(i);
                    self.selected = None;
                    ui.close();
                    return;
                }
                ui.separator();
            }
            if ui
                .button(format!("{}  {}", icon::PLUS, t("state_graph.add_state")))
                .clicked()
            {
                add_at = Some(
                    ui.ctx()
                        .input(|i| i.pointer.interact_pos())
                        .map_or(Pos2::ZERO, to_graph),
                );
                ui.close();
            }
            if ui
                .button(format!(
                    "{}  {}",
                    icon::TREE_STRUCTURE,
                    t("state_graph.arrange")
                ))
                .clicked()
            {
                arrange(&mut self.file);
                self.fit = true;
                ui.close();
            }
        });
        if let Some(at) = add_at {
            let i = self.add_state(at);
            self.selected = Some(Selected::State(i));
        }
        let keys_here = response.hovered() && !ui.ctx().egui_wants_keyboard_input();
        if keys_here && ui.input(|i| i.key_pressed(egui::Key::F)) {
            self.fit = true;
        }
        if keys_here && ui.input(|i| i.key_pressed(egui::Key::Delete)) {
            match self.selected {
                Some(Selected::State(i)) => self.delete_state(i),
                Some(Selected::Transition(i)) => {
                    self.file.transitions.remove(i);
                    self.selected = None;
                }
                None => {}
            }
        }
        // How to use it, along the bottom.
        painter.text(
            rect.left_bottom() + Vec2::new(10.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            t("state_graph.canvas_hint"),
            egui::FontId::proportional(11.0),
            theme::TEXT_DIM,
        );
    }

    /// A transition from `from` (`None`: any state) to state `to`, selected.
    fn link(&mut self, from: Option<usize>, to: usize) {
        let from = from.map_or_else(
            || ANY_STATE.to_owned(),
            |f| self.file.states[f].name.clone(),
        );
        let to = self.file.states[to].name.clone();
        if from == to
            || self
                .file
                .transitions
                .iter()
                .any(|t| t.from == from && t.to == to)
        {
            return;
        }
        self.file.transitions.push(TransitionDesc {
            from,
            to,
            when: Vec::new(),
            exit_time: None,
            fade: None,
        });
        self.selected = Some(Selected::Transition(self.file.transitions.len() - 1));
    }

    /// Zooms and pans so every state (and the any-state box) is in `rect`.
    fn fit_view(&mut self, rect: Rect) {
        let mut bounds = Rect::from_min_size(ANY_AT, ANY_BOX);
        for s in &self.file.states {
            bounds = bounds.union(Rect::from_min_size(
                Pos2::new(s.position[0], s.position[1]),
                BOX,
            ));
        }
        let bounds = bounds.expand(40.0);
        let zoom = (rect.width() / bounds.width())
            .min(rect.height() / bounds.height())
            .clamp(0.3, 1.4);
        self.zoom = zoom;
        self.pan = rect.center() - rect.min - bounds.center().to_vec2() * zoom;
    }
}

/// Edge color of a transition at rest.
const EDGE: Color32 = Color32::from_rgb(150, 160, 180);
/// The band of a blend state.
const BLEND: Color32 = Color32::from_rgb(170, 120, 240);

/// What a transition waits for, short: `speed > 0.1 & grounded`, `90%`.
fn condition_label(tr: &TransitionDesc) -> String {
    let mut parts: Vec<String> = tr
        .when
        .iter()
        .map(|c| match (c.op, c.value) {
            (Some(op), Some(v)) => format!("{} {} {}", c.parameter, op.symbol(), trim_number(v)),
            (Some(op), None) => format!("{} {} 0", c.parameter, op.symbol()),
            _ => c.parameter.clone(),
        })
        .collect();
    if let Some(exit) = tr.exit_time {
        parts.push(format!("{}%", (exit * 100.0).round()));
    }
    parts.join(" & ")
}

fn trim_number(v: f32) -> String {
    let text = format!("{v:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A cubic Bézier curve on screen.
#[derive(Debug, Clone, Copy)]
struct Curve([Pos2; 4]);

impl Curve {
    fn new(a: Pos2, b: Pos2, c: Pos2, d: Pos2) -> Self {
        Self([a, b, c, d])
    }

    fn at(&self, t: f32) -> Pos2 {
        let [a, b, c, d] = self.0;
        let u = 1.0 - t;
        let v = a.to_vec2() * (u * u * u)
            + b.to_vec2() * (3.0 * u * u * t)
            + c.to_vec2() * (3.0 * u * t * t)
            + d.to_vec2() * (t * t * t);
        Pos2::ZERO + v
    }

    fn points(&self, n: usize) -> Vec<Pos2> {
        (0..=n).map(|i| self.at(i as f32 / n as f32)).collect()
    }

    /// The end point and the direction it arrives from.
    fn end(&self) -> (Pos2, Vec2) {
        let d = self.0[3] - self.at(0.95);
        (
            self.0[3],
            if d.length_sq() > 1e-6 {
                d.normalized()
            } else {
                Vec2::X
            },
        )
    }

    fn distance(&self, p: Pos2) -> f32 {
        self.points(24)
            .windows(2)
            .map(|w| distance_to_segment(p, w[0], w[1]))
            .fold(f32::INFINITY, f32::min)
    }
}

/// The curve of a transition from box `a` to box `b`. With `lane` (the other way taken
/// too), it runs on its own side, `lane` to its left, so the two never cross.
fn transition_curve(a: Rect, b: Rect, lane: f32) -> Curve {
    let dir = b.center() - a.center();
    let normal = if dir.length_sq() > 1e-6 {
        Vec2::new(dir.y, -dir.x).normalized() * lane
    } else {
        Vec2::ZERO
    };
    // Leaving and arriving shifted sideways (still on the boxes' edges), slightly bowed.
    let shift = normal * 0.5;
    let from = a.clamp(edge_point(a.translate(shift), b.center() + shift));
    let to = b.clamp(edge_point(b.translate(shift), a.center() + shift));
    let span = to - from;
    let bow = normal * 0.4;
    Curve::new(from, from + span * 0.3 + bow, to - span * 0.3 + bow, to)
}

/// A filled arrowhead with its tip at `tip`, pointing along `direction`.
fn arrow_head(painter: &egui::Painter, tip: Pos2, direction: Vec2, size: f32, color: Color32) {
    let back = -direction * size;
    let side = Vec2::new(-direction.y, direction.x) * size * 0.5;
    painter.add(egui::Shape::convex_polygon(
        vec![tip, tip + back + side, tip + back - side],
        color,
        Stroke::NONE,
    ));
}

/// Lays the states out in columns by how many transitions they are from the entry state
/// (the ones it cannot reach last), rows within a column in file order.
pub fn arrange(file: &mut StateMachineFile) {
    let n = file.states.len();
    if n == 0 {
        return;
    }
    let entry = file
        .entry
        .as_deref()
        .and_then(|e| file.state(e))
        .unwrap_or(0);
    let mut depth = vec![usize::MAX; n];
    depth[entry] = 0;
    let mut queue = std::collections::VecDeque::from([entry]);
    // What any state leads to sits one step out, beside what the entry leads to.
    for tr in file.transitions.iter().filter(|t| t.from == ANY_STATE) {
        if let Some(j) = file.state(&tr.to) {
            if depth[j] == usize::MAX {
                depth[j] = 1;
            }
        }
    }
    while let Some(i) = queue.pop_front() {
        let name = file.states[i].name.clone();
        for tr in file.transitions.iter().filter(|t| t.from == name) {
            if let Some(j) = file.state(&tr.to) {
                if depth[j] == usize::MAX {
                    depth[j] = depth[i] + 1;
                    queue.push_back(j);
                }
            }
        }
        if i == entry {
            // Any state's targets, placed above, go on from there too.
            let more: Vec<usize> = (0..n)
                .filter(|&j| depth[j] == 1 && !queue.contains(&j))
                .collect();
            queue.extend(more);
        }
    }
    let deepest = depth
        .iter()
        .copied()
        .filter(|&d| d != usize::MAX)
        .max()
        .unwrap_or(0);
    for d in &mut depth {
        if *d == usize::MAX {
            *d = deepest + 1;
        }
    }
    let mut rows = vec![0usize; deepest + 2];
    let counts: Vec<usize> = (0..deepest + 2)
        .map(|c| depth.iter().filter(|&&d| d == c).count())
        .collect();
    for (i, state) in file.states.iter_mut().enumerate() {
        let column = depth[i];
        let row = rows[column];
        rows[column] += 1;
        let height = counts[column] as f32 * GAP.y;
        state.position = [
            column as f32 * GAP.x,
            (row as f32 * GAP.y - height * 0.5 + GAP.y * 0.5).round(),
        ];
    }
}

/// Spacing of [`arrange`]: columns, rows.
const GAP: Vec2 = Vec2::new(340.0, 120.0);

/// Whether states sit on top of each other (a file written by hand, without positions).
fn overlapping(file: &StateMachineFile) -> bool {
    file.states.iter().enumerate().any(|(i, a)| {
        file.states[i + 1..].iter().any(|b| {
            (a.position[0] - b.position[0]).abs() < BOX.x
                && (a.position[1] - b.position[1]).abs() < BOX.y
        })
    })
}

#[derive(Debug, Clone, Copy)]
enum StateMenu {
    Connect,
    Entry,
    Delete,
}

fn kind_name(kind: ParameterKind) -> &'static str {
    match kind {
        ParameterKind::Float => "Number",
        ParameterKind::Bool => "Bool",
        ParameterKind::Trigger => "Trigger",
    }
}

fn clip_choice(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    clip: &mut String,
    clips: &[String],
) {
    if clips.is_empty() {
        ui.add(
            egui::TextEdit::singleline(clip)
                .hint_text("Clip name")
                .desired_width(110.0),
        );
        return;
    }
    egui::ComboBox::from_id_salt(id)
        .selected_text(if clip.is_empty() {
            "—"
        } else {
            clip.as_str()
        })
        .width(110.0)
        .show_ui(ui, |ui| {
            for c in clips {
                ui.selectable_value(clip, c.clone(), c);
            }
        });
}

fn state_choice(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut String,
    states: &[String],
    any: bool,
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(if value == ANY_STATE {
            t("state_graph.any_state")
        } else {
            value.clone()
        })
        .show_ui(ui, |ui| {
            if any {
                ui.selectable_value(value, ANY_STATE.to_owned(), t("state_graph.any_state"));
            }
            for s in states {
                ui.selectable_value(value, s.clone(), s);
            }
        });
}

/// Where the line from `rect`'s centre towards `towards` leaves it.
fn edge_point(rect: Rect, towards: Pos2) -> Pos2 {
    let c = rect.center();
    let d = towards - c;
    if d.length_sq() < 1e-6 {
        return c;
    }
    let sx = if d.x.abs() > 1e-6 {
        rect.width() * 0.5 / d.x.abs()
    } else {
        f32::INFINITY
    };
    let sy = if d.y.abs() > 1e-6 {
        rect.height() * 0.5 / d.y.abs()
    } else {
        f32::INFINITY
    };
    c + d * sx.min(sy)
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_starter_is_valid_and_renames_and_deletes_keep_it_so() {
        let mut panel = StateGraphPanel {
            file: starter(),
            ..Default::default()
        };
        panel.file.check().unwrap();
        panel.file.states[0].name = "Rest".into();
        panel.rename_state("Idle", "Rest");
        panel.file.check().unwrap();
        assert_eq!(panel.file.entry.as_deref(), Some("Rest"));
        let jump = panel.file.state("Jump").unwrap();
        panel.delete_state(jump);
        panel.file.check().unwrap();
        assert!(panel
            .file
            .transitions
            .iter()
            .all(|t| t.to != "Jump" && t.from != "Jump"));
        let i = panel.add_state(Pos2::new(10.4, 20.6));
        assert_eq!(panel.file.states[i].position, [10.0, 21.0]);
        assert!(panel.is_dirty());
    }

    #[test]
    fn saved_machines_drive_the_node_they_are_given_to() {
        let project = std::env::temp_dir().join(format!("ab-state-graph-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(project.join("models")).unwrap();
        let assets =
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/models"));
        for file in std::fs::read_dir(assets).unwrap().flatten() {
            std::fs::copy(file.path(), project.join("models").join(file.file_name())).unwrap();
        }
        let mut editor = Editor::new(project.clone(), None, ab_engine::audio::AudioOutput::Silent);
        editor.select(None);
        editor.add_scene(
            ab_engine::scene_file::SceneFile {
                environment: None,
                nodes: vec![ab_engine::scene_file::NodeDesc {
                    name: "Hero".into(),
                    instance: Some("models/mannequin.gltf".into()),
                    ..Default::default()
                }],
            },
            None,
        );
        let mut panel = StateGraphPanel {
            path: "anims/hero.abstate".into(),
            file: starter(),
            ..Default::default()
        };
        panel.save(&mut editor);
        assert_eq!(panel.error, None);
        assert!(!panel.is_dirty());
        panel.assign(&mut editor);
        let hero = editor.selection.unwrap();
        let machine = editor
            .tree()
            .component::<StateMachine>(hero)
            .unwrap()
            .clone();
        assert_eq!(machine.file, "anims/hero.abstate");
        assert_eq!(
            crate::features::Feature::of(&editor, hero),
            Some(crate::features::Feature::AnimationGraph)
        );
        // The model's clips are offered for its states.
        let clips = panel.clip_names(&editor);
        assert!(!clips.is_empty(), "the mannequin has animations");
        // The rigging editor finds the character and its humanoid parts.
        let mut rigging = super::super::rigging::RiggingPanel::default();
        rigging.follow(&editor);
        assert_eq!(rigging.model(), Some(hero));
        assert!(!rigging.overlay(&editor).is_empty(), "its bones are drawn");
        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn the_starter_takes_the_characters_clip_names() {
        let clips = [
            "Armature|idle".to_owned(),
            "Walking".to_owned(),
            "RUN".to_owned(),
        ];
        let file = starter_for(&clips);
        assert_eq!(file.states[0].clip.as_deref(), Some("Armature|idle"));
        let blend = file.states[1].blend.as_ref().unwrap();
        assert_eq!(
            (blend.clips[0].0.as_str(), blend.clips[1].0.as_str()),
            ("Walking", "RUN")
        );
        // No jump clip: the usual name stays, to be set by hand.
        assert_eq!(file.states[2].clip.as_deref(), Some("Jump"));
    }

    #[test]
    fn arranging_spreads_states_out_by_steps_from_the_entry() {
        let mut file = starter_for(&[]);
        for state in &mut file.states {
            state.position = [0.0, 0.0];
        }
        assert!(overlapping(&file));
        arrange(&mut file);
        assert!(!overlapping(&file));
        let x = |name: &str| file.states[file.state(name).unwrap()].position[0];
        assert_eq!(x("Idle"), 0.0, "the entry first");
        assert!(x("Move") > x("Idle"));
        assert_eq!(
            x("Jump"),
            x("Move"),
            "any state's target beside the entry's"
        );
        file.check().unwrap();
    }

    #[test]
    fn conditions_read_as_short_labels() {
        let tr = TransitionDesc {
            from: "Idle".into(),
            to: "Move".into(),
            when: vec![
                ab_engine::state_machine::Condition {
                    parameter: "speed".into(),
                    op: Some(ab_engine::state_machine::Compare::Greater),
                    value: Some(0.1),
                },
                ab_engine::state_machine::Condition {
                    parameter: "grounded".into(),
                    op: None,
                    value: None,
                },
            ],
            exit_time: Some(0.9),
            fade: None,
        };
        assert_eq!(condition_label(&tr), "speed > 0.1 & grounded & 90%");
    }

    #[test]
    fn curves_start_and_end_on_the_boxes() {
        let a = Rect::from_min_size(Pos2::ZERO, BOX);
        let b = Rect::from_min_size(Pos2::new(400.0, 0.0), BOX);
        let c = transition_curve(a, b, 0.0);
        assert!((c.at(0.0).x - a.right()).abs() < 1e-3);
        let (tip, dir) = c.end();
        assert!((tip.x - b.left()).abs() < 1e-3 && dir.x > 0.9);
        let bent = transition_curve(a, b, 30.0);
        assert!(bent.at(0.5).y < a.center().y - 10.0, "bowed to one side");
        let back = transition_curve(b, a, 30.0);
        assert!(
            back.at(0.5).y > a.center().y + 10.0,
            "the other way on the other side"
        );
        for p in [bent.at(0.0), bent.end().0, back.at(0.0), back.end().0] {
            let on_a = a.expand(0.01).contains(p);
            let on_b = b.expand(0.01).contains(p);
            assert!(on_a || on_b, "{p:?} ends on a box");
        }
    }

    #[test]
    fn geometry_helpers() {
        let r = Rect::from_center_size(Pos2::ZERO, Vec2::new(100.0, 40.0));
        assert_eq!(edge_point(r, Pos2::new(200.0, 0.0)), Pos2::new(50.0, 0.0));
        assert_eq!(edge_point(r, Pos2::new(0.0, -100.0)), Pos2::new(0.0, -20.0));
        assert!(
            (distance_to_segment(Pos2::new(5.0, 3.0), Pos2::ZERO, Pos2::new(10.0, 0.0)) - 3.0)
                .abs()
                < 1e-5
        );
    }
}
