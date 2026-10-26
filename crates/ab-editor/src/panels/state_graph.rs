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

const BOX: Vec2 = Vec2::new(150.0, 44.0);
/// Where the "any state" box sits in the graph.
const ANY_AT: Pos2 = Pos2::new(-220.0, -140.0);

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
            Ok(file) => {
                self.saved = file.clone();
                self.file = file;
                self.error = None;
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
        let zoom = self.zoom;
        let origin = rect.min + self.pan;
        let to_screen = |p: Pos2| origin + p.to_vec2() * zoom;
        let to_graph = |p: Pos2| Pos2::ZERO + (p - origin) / zoom;
        // Grid.
        let step = 40.0 * zoom;
        let mut x = rect.left() + (origin.x - rect.left()).rem_euclid(step);
        while x < rect.right() {
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(1.0_f32, theme::BG_2),
            );
            x += step;
        }
        let mut y = rect.top() + (origin.y - rect.top()).rem_euclid(step);
        while y < rect.bottom() {
            painter.line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                Stroke::new(1.0_f32, theme::BG_2),
            );
            y += step;
        }
        let box_at =
            |p: [f32; 2]| Rect::from_min_size(to_screen(Pos2::new(p[0], p[1])), BOX * zoom);
        let any_box = Rect::from_min_size(to_screen(ANY_AT), BOX * zoom);
        let rect_of = |name: &str, file: &StateMachineFile| -> Option<Rect> {
            if name == ANY_STATE {
                return Some(any_box);
            }
            file.state(name).map(|i| box_at(file.states[i].position))
        };
        let current = live.and_then(|m| m.state_name().map(str::to_owned));
        // Transitions: arrows between box edges, side by side when both ways.
        let mut clicked_transition = None;
        let pointer = response.interact_pointer_pos().or(response.hover_pos());
        for (i, tr) in self.file.transitions.iter().enumerate() {
            let (Some(a), Some(b)) = (rect_of(&tr.from, &self.file), rect_of(&tr.to, &self.file))
            else {
                continue;
            };
            let dir = (b.center() - a.center()).normalized();
            let side = Vec2::new(-dir.y, dir.x) * 6.0 * zoom;
            let from = edge_point(a, b.center()) + side;
            let to = edge_point(b, a.center()) + side;
            let selected = self.selected == Some(Selected::Transition(i));
            let color = if selected {
                Color32::WHITE
            } else {
                theme::TEXT_DIM
            };
            painter.arrow(
                from,
                to - from,
                Stroke::new(if selected { 2.5 } else { 1.5 }, color),
            );
            if !tr.when.is_empty() || tr.exit_time.is_some() {
                let mid = from + (to - from) * 0.5 + side * 1.5;
                let label = if tr.when.is_empty() {
                    format!("{:.0}%", tr.exit_time.unwrap_or(0.0) * 100.0)
                } else {
                    tr.when
                        .iter()
                        .map(|c| c.parameter.as_str())
                        .collect::<Vec<_>>()
                        .join(" & ")
                };
                painter.text(
                    mid,
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(10.0 * zoom.max(0.7)),
                    theme::TEXT_DIM,
                );
            }
            if response.clicked() {
                if let Some(p) = pointer {
                    if distance_to_segment(p, from, to) < 6.0 {
                        clicked_transition = Some(i);
                    }
                }
            }
        }
        // The "any state" box.
        painter.rect(
            any_box,
            6.0,
            theme::BG_2,
            Stroke::new(1.0_f32, theme::BORDER),
            egui::StrokeKind::Inside,
        );
        painter.text(
            any_box.center(),
            egui::Align2::CENTER_CENTER,
            t("state_graph.any_state"),
            egui::FontId::proportional(13.0 * zoom),
            theme::TEXT_DIM,
        );
        let any_hit = ui.interact(any_box, ui.id().with("sm_any"), Sense::click());
        // States.
        let entry = self
            .file
            .entry
            .clone()
            .or_else(|| self.file.states.first().map(|s| s.name.clone()));
        let mut moved = None;
        let mut clicked_state = None;
        // What a state's menu asked for.
        let mut menu: Option<(usize, StateMenu)> = None;
        for (i, s) in self.file.states.iter().enumerate() {
            let r = box_at(s.position);
            let selected = self.selected == Some(Selected::State(i));
            let active = current.as_deref() == Some(s.name.as_str());
            let fill = if active {
                theme::PLAY.gamma_multiply(0.45)
            } else {
                theme::BG_2
            };
            let stroke = if selected {
                Stroke::new(2.0_f32, Color32::WHITE)
            } else if entry.as_deref() == Some(s.name.as_str()) {
                Stroke::new(2.0_f32, theme::WARN)
            } else {
                Stroke::new(1.0_f32, theme::BORDER)
            };
            painter.rect(r, 6.0, fill, stroke, egui::StrokeKind::Inside);
            let kind = if s.blend.is_some() {
                icon::CHART_LINE
            } else {
                icon::FILM_STRIP
            };
            painter.text(
                r.center() - Vec2::new(0.0, 7.0 * zoom),
                egui::Align2::CENTER_CENTER,
                &s.name,
                egui::FontId::proportional(13.0 * zoom),
                theme::TEXT,
            );
            let sub = s.clip.clone().unwrap_or_else(|| {
                s.blend
                    .as_ref()
                    .map(|b| {
                        format!(
                            "{} ({})",
                            b.clips
                                .iter()
                                .map(|c| c.0.as_str())
                                .collect::<Vec<_>>()
                                .join("/"),
                            b.parameter
                        )
                    })
                    .unwrap_or_default()
            });
            painter.text(
                r.center() + Vec2::new(0.0, 9.0 * zoom),
                egui::Align2::CENTER_CENTER,
                format!("{kind} {sub}"),
                egui::FontId::proportional(10.0 * zoom),
                theme::TEXT_DIM,
            );
            if active {
                if let Some(m) = live {
                    let progress = (m.state_time % 1.0).clamp(0.0, 1.0);
                    let bar = Rect::from_min_size(
                        r.left_bottom() - Vec2::new(0.0, 3.0),
                        Vec2::new(r.width() * progress, 3.0),
                    );
                    painter.rect_filled(bar, 0.0, theme::PLAY);
                }
            }
            let hit = ui.interact(r, ui.id().with(("sm_state", i)), Sense::click_and_drag());
            if hit.clicked() {
                clicked_state = Some(i);
            }
            if hit.dragged() {
                moved = Some((i, hit.drag_delta() / zoom));
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
        // Clicks: finish a transition, or select.
        if let Some(i) = clicked_state {
            match self.connecting.take() {
                Some(from) => {
                    let from = from.map_or_else(
                        || ANY_STATE.to_owned(),
                        |f| self.file.states[f].name.clone(),
                    );
                    let to = self.file.states[i].name.clone();
                    if from != to {
                        self.file.transitions.push(TransitionDesc {
                            from,
                            to,
                            when: Vec::new(),
                            exit_time: None,
                            fade: None,
                        });
                        self.selected = Some(Selected::Transition(self.file.transitions.len() - 1));
                    }
                }
                None => self.selected = Some(Selected::State(i)),
            }
        } else if let Some(i) = clicked_transition {
            self.selected = Some(Selected::Transition(i));
        } else if response.clicked() {
            self.selected = None;
            self.connecting = None;
        }
        // While connecting, a line follows the pointer.
        if let (Some(from), Some(p)) = (self.connecting, response.hover_pos()) {
            let start = from.map_or(any_box.center(), |f| {
                box_at(self.file.states[f].position).center()
            });
            painter.line_segment([start, p], Stroke::new(1.5_f32, theme::WARN));
            ui.ctx().request_repaint();
        }
        // Pan (drag the background, or the middle button) and zoom (wheel).
        if response.dragged() && moved.is_none() {
            self.pan += response.drag_delta();
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let before = response.hover_pos().map(to_graph);
                self.zoom = (self.zoom * (1.0 + scroll / 400.0)).clamp(0.4, 2.5);
                if let (Some(g), Some(p)) = (before, response.hover_pos()) {
                    // Keep the point under the pointer still.
                    self.pan = p - rect.min - g.to_vec2() * self.zoom;
                }
            }
        }
        let mut add_at = None;
        response.context_menu(|ui| {
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
        });
        if let Some(at) = add_at {
            let i = self.add_state(at);
            self.selected = Some(Selected::State(i));
        }
        let delete = ui.input(|i| i.key_pressed(egui::Key::Delete)) && response.hovered();
        if delete {
            match self.selected {
                Some(Selected::State(i)) => self.delete_state(i),
                Some(Selected::Transition(i)) => {
                    self.file.transitions.remove(i);
                    self.selected = None;
                }
                None => {}
            }
        }
    }
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
