//! The Rigging editor: a character's skeleton and its rig file (see [`ab_engine::rig`]).
//!
//! The bones are listed as the skeleton nests them and drawn over the view (the selected
//! one white, humanoid parts in the accent colour). For each bone: which humanoid part it
//! is (found from the names and shape, or set by hand) and how far it may turn (a hinge or
//! a cone), used by IK. Named IK chains are made from a selected tip. Save writes the rig
//! file; Assign gives it to the model.

use super::section;
use crate::i18n::t;
use crate::state::Editor;
use crate::theme;
use crate::viewport::OverlayLine;
use ab_engine::physics::HumanBone;
use ab_engine::rendering::Skin;
use ab_engine::rig::{ChainDesc, HingeDesc, LimitDesc, Rig, RigFile, RIG_EXTENSION};
use ab_engine::scene::NodeHandle;
use eframe::egui::{self, Color32, RichText};
use egui_phosphor::regular as icon;

#[derive(Debug, Default)]
pub struct RiggingPanel {
    /// The character being rigged.
    model: Option<NodeHandle>,
    /// The rig file, relative to the project.
    pub path: String,
    rig: RigFile,
    saved: RigFile,
    pub bone: Option<NodeHandle>,
    filter: String,
    error: Option<String>,
}

/// The character `node` belongs to: the nearest node at or above it with a skinned mesh in
/// it that is a model instance or has a rig.
pub fn model_of(editor: &Editor, node: NodeHandle) -> Option<NodeHandle> {
    let tree = editor.tree();
    std::iter::successors(Some(node), |&n| tree.parent(n))
        .take_while(|&n| n != editor.scene_root)
        .find(|&n| {
            let rigged = editor.is_instance(n) || tree.component::<Rig>(n).is_some();
            rigged
                && tree
                    .subtree(n)
                    .into_iter()
                    .any(|m| tree.component::<Skin>(m).is_some())
        })
}

fn part_name(part: HumanBone) -> String {
    part.key().replace('_', " ")
}

impl RiggingPanel {
    pub fn model(&self) -> Option<NodeHandle> {
        self.model
    }

    pub fn is_dirty(&self) -> bool {
        self.rig != self.saved
    }

    /// Follows the selection to its character; reads its rig when it is another one.
    pub fn follow(&mut self, editor: &Editor) {
        let Some(model) = editor.selection.and_then(|n| model_of(editor, n)) else {
            return;
        };
        if self.model == Some(model) {
            return;
        }
        if self.is_dirty() && self.model.is_some_and(|m| editor.tree().is_valid(m)) {
            return;
        }
        self.model = Some(model);
        self.bone = None;
        self.error = None;
        let rig = editor.tree().component::<Rig>(model).map(|r| r.clone());
        match rig {
            Some(rig) => {
                self.path = rig.file.unwrap_or_default();
                self.rig = rig.desc;
            }
            None => {
                let name = editor
                    .tree()
                    .name(model)
                    .map(|n| n.to_lowercase())
                    .unwrap_or_default();
                self.path = format!("rigs/{name}.{RIG_EXTENSION}");
                self.rig = RigFile::default();
            }
        }
        self.saved = self.rig.clone();
    }

    fn live_rig(&self) -> Rig {
        Rig {
            file: Some(self.path.clone()),
            desc: self.rig.clone(),
        }
    }

    /// The skeleton's bones with their depth, in order.
    fn bones(&self, editor: &Editor) -> Vec<(NodeHandle, usize)> {
        let Some(model) = self.model.filter(|&m| editor.tree().is_valid(m)) else {
            return Vec::new();
        };
        let tree = editor.tree();
        let bones = ab_engine::ragdoll::skeleton_bones(tree, model);
        let mut out = Vec::new();
        // Roots: bones whose parent is not a bone; then depth first.
        let roots: Vec<NodeHandle> = bones
            .iter()
            .copied()
            .filter(|&b| tree.parent(b).is_none_or(|p| !bones.contains(&p)))
            .collect();
        let mut stack: Vec<(NodeHandle, usize)> = roots.into_iter().rev().map(|b| (b, 0)).collect();
        while let Some((bone, depth)) = stack.pop() {
            out.push((bone, depth));
            for &child in tree.children(bone).iter().rev() {
                if bones.contains(&child) {
                    stack.push((child, depth + 1));
                }
            }
        }
        out
    }

    /// The skeleton drawn over a view.
    pub fn overlay(&self, editor: &Editor) -> Vec<OverlayLine> {
        let Some(model) = self.model.filter(|&m| editor.tree().is_valid(m)) else {
            return Vec::new();
        };
        let tree = editor.tree();
        let humanoid = self.live_rig().humanoid(tree, model);
        let mapped: Vec<NodeHandle> = humanoid.iter().map(|(_, n)| n).collect();
        let bones = ab_engine::ragdoll::skeleton_bones(tree, model);
        let mut lines = Vec::new();
        for &bone in &bones {
            let Some(at) = tree.global_position(bone) else {
                continue;
            };
            let parent = tree.parent(bone).filter(|p| bones.contains(p));
            let from = parent.and_then(|p| tree.global_position(p)).unwrap_or(at);
            let (color, width) = if self.bone == Some(bone) {
                (Color32::WHITE, 3.0)
            } else if mapped.contains(&bone) {
                (theme::ACCENT, 2.0)
            } else {
                (theme::TEXT_DIM, 1.5)
            };
            lines.push(OverlayLine {
                from,
                to: at,
                color,
                width,
                joint: true,
            });
            if self.bone == Some(bone) {
                lines.push(OverlayLine {
                    from: at,
                    to: at,
                    color,
                    width: 4.0,
                    joint: true,
                });
            }
        }
        lines
    }

    pub fn toolbar(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        ui.horizontal_wrapped(|ui| {
            let name = self
                .model
                .and_then(|m| editor.tree().name(m).map(|n| n.to_owned()))
                .unwrap_or_else(|| "—".into());
            ui.label(
                RichText::new(format!("{}  {name}", icon::PERSON_SIMPLE)).font(theme::bold(13.0)),
            );
            ui.add(egui::TextEdit::singleline(&mut self.path).desired_width(220.0))
                .on_hover_text(t("rigging.path_hint"));
            let save = format!(
                "{}  {}{}",
                icon::FLOPPY_DISK,
                t("dialog.save"),
                if self.is_dirty() { " •" } else { "" }
            );
            let can = self.model.is_some() && !editor.is_playing();
            if ui
                .add_enabled(can, egui::Button::new(save))
                .on_hover_text(t("rigging.save_hint"))
                .clicked()
            {
                self.save(editor);
            }
        });
        if let Some(error) = self.error.clone().or_else(|| self.rig.check().err()) {
            ui.label(RichText::new(format!("{}  {error}", icon::WARNING)).color(theme::WARN));
        }
    }

    /// Writes the rig file and gives it to the model (its IK reads it again).
    fn save(&mut self, editor: &mut Editor) {
        let Some(model) = self.model else {
            return;
        };
        if let Err(err) = self.rig.check() {
            self.error = Some(err);
            return;
        }
        let mut path = self.path.trim().replace('\\', "/");
        if !path.ends_with(&format!(".{RIG_EXTENSION}")) {
            path = format!("{path}.{RIG_EXTENSION}");
        }
        let full = editor.project().join(&path);
        if let Err(err) = editor.write_project_file(&full, self.rig.to_toml()) {
            self.error = Some(format!("{}: {err}", full.display()));
            return;
        }
        self.path = path.clone();
        self.saved = self.rig.clone();
        self.error = None;
        if let Some(old) = editor.describe_node(model) {
            let mut new = old.clone();
            new.rig = Some(path.clone());
            if new != old {
                editor.apply_desc(model, &old, new);
                editor.end_gesture();
                // The node was made again.
                self.model = editor.selection.and_then(|n| model_of(editor, n));
            } else {
                let _ = editor.tree_mut().insert_component(
                    model,
                    Rig {
                        file: Some(path.clone()),
                        desc: self.rig.clone(),
                    },
                );
            }
        }
        editor.assets.request_rescan();
        editor.info(format!("Saved {path}"));
    }

    pub fn bones_ui(&mut self, ui: &mut egui::Ui, editor: &Editor) {
        if self.model.is_none() {
            super::empty_hint(ui, icon::PERSON_SIMPLE, &t("rigging.empty"));
            return;
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text(format!(
                    "{}  {}",
                    icon::MAGNIFYING_GLASS,
                    t("rigging.search")
                ))
                .desired_width(f32::INFINITY),
        );
        let bones = self.bones(editor);
        ui.label(
            RichText::new(format!("{} bones", bones.len()))
                .small()
                .color(theme::TEXT_DIM),
        );
        let filter = self.filter.to_lowercase();
        let tree = editor.tree();
        let humanoid = self
            .model
            .map(|m| self.live_rig().humanoid(tree, m))
            .unwrap_or_default();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (bone, depth) in bones {
                    let name = tree.name(bone).map(|n| n.to_owned()).unwrap_or_default();
                    if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
                        continue;
                    }
                    let part = humanoid.iter().find(|(_, n)| *n == bone).map(|(p, _)| p);
                    let limited = self.rig.limits.iter().any(|l| l.bone == name);
                    let mut text = format!("{}  {name}", icon::BONE);
                    if limited {
                        text.push_str(&format!("  {}", icon::LOCK_SIMPLE));
                    }
                    let mut label = RichText::new(text);
                    if part.is_some() {
                        label = label.color(theme::ACCENT);
                    }
                    // Nested by depth on one line; a long name is cut, not wrapped.
                    let indent = if filter.is_empty() {
                        depth as f32 * 12.0
                    } else {
                        0.0
                    };
                    let response = ui
                        .horizontal(|ui| {
                            ui.add_space(indent);
                            ui.add(
                                egui::Button::selectable(self.bone == Some(bone), label).truncate(),
                            )
                        })
                        .inner
                        .on_hover_text(&name);
                    let response = match part {
                        Some(p) => response.on_hover_text(part_name(p)),
                        None => response,
                    };
                    if response.clicked() {
                        self.bone = Some(bone);
                    }
                }
            });
    }

    pub fn details_ui(&mut self, ui: &mut egui::Ui, editor: &Editor) {
        let Some(model) = self.model.filter(|&m| editor.tree().is_valid(m)) else {
            return;
        };
        let tree = editor.tree();
        let names: Vec<String> = ab_engine::ragdoll::skeleton_bones(tree, model)
            .into_iter()
            .filter_map(|b| tree.name(b).map(|n| n.to_owned()))
            .collect();
        egui::ScrollArea::vertical().show(ui, |ui| {
            let selected = self
                .bone
                .filter(|&b| tree.is_valid(b))
                .and_then(|b| tree.name(b).map(|n| n.to_owned()));
            match &selected {
                Some(name) => self.bone_ui(ui, name),
                None => {
                    ui.label(RichText::new(t("rigging.pick_bone")).color(theme::TEXT_DIM));
                }
            }
            self.humanoid_ui(ui, editor, model, &names);
            self.chains_ui(ui, &names, selected);
        });
    }

    fn bone_ui(&mut self, ui: &mut egui::Ui, name: &str) {
        section(ui, "rig_bone", icon::BONE, name, |ui| {
            let index = self.rig.limits.iter().position(|l| l.bone == name);
            #[derive(PartialEq, Clone, Copy)]
            enum Kind {
                Free,
                Hinge,
                Cone,
            }
            let kind = match index.map(|i| &self.rig.limits[i]) {
                Some(l) if l.hinge.is_some() => Kind::Hinge,
                Some(_) => Kind::Cone,
                None => Kind::Free,
            };
            let mut wanted = kind;
            ui.horizontal(|ui| {
                ui.label(t("rigging.limit"));
                ui.selectable_value(&mut wanted, Kind::Free, t("rigging.free"));
                ui.selectable_value(&mut wanted, Kind::Hinge, t("rigging.hinge"));
                ui.selectable_value(&mut wanted, Kind::Cone, t("rigging.cone"));
            });
            if wanted != kind {
                if let Some(i) = index {
                    self.rig.limits.remove(i);
                }
                match wanted {
                    Kind::Free => {}
                    Kind::Hinge => self.rig.limits.push(LimitDesc {
                        bone: name.to_owned(),
                        hinge: Some(HingeDesc {
                            axis: [1.0, 0.0, 0.0],
                            min: 0.0,
                            max: 140.0,
                        }),
                        cone: None,
                    }),
                    Kind::Cone => self.rig.limits.push(LimitDesc {
                        bone: name.to_owned(),
                        hinge: None,
                        cone: Some(60.0),
                    }),
                }
            }
            let Some(limit) = self.rig.limits.iter_mut().find(|l| l.bone == name) else {
                return;
            };
            super::grid(ui, "rig_limit_grid", |ui| {
                if let Some(h) = &mut limit.hinge {
                    super::label(ui, "Axis");
                    ui.horizontal(|ui| {
                        for (axis, label) in [
                            ([1.0, 0.0, 0.0], "X"),
                            ([0.0, 1.0, 0.0], "Y"),
                            ([0.0, 0.0, 1.0], "Z"),
                        ] {
                            if ui.selectable_label(h.axis == axis, label).clicked() {
                                h.axis = axis;
                            }
                        }
                    });
                    ui.end_row();
                    super::label(ui, "");
                    super::vec3(ui, &mut h.axis, 0.01, "");
                    ui.end_row();
                    super::label(ui, "Min (°)");
                    ui.add(
                        egui::DragValue::new(&mut h.min)
                            .range(-180.0..=180.0)
                            .speed(0.5),
                    );
                    ui.end_row();
                    super::label(ui, "Max (°)");
                    ui.add(
                        egui::DragValue::new(&mut h.max)
                            .range(-180.0..=180.0)
                            .speed(0.5),
                    );
                    ui.end_row();
                    h.max = h.max.max(h.min);
                }
                if let Some(c) = &mut limit.cone {
                    super::label(ui, "Cone (°)");
                    ui.add(egui::Slider::new(c, 0.0..=180.0));
                    ui.end_row();
                }
            });
            ui.label(
                RichText::new(t("rigging.limit_hint"))
                    .small()
                    .color(theme::TEXT_DIM),
            );
        });
    }

    fn humanoid_ui(
        &mut self,
        ui: &mut egui::Ui,
        editor: &Editor,
        model: NodeHandle,
        names: &[String],
    ) {
        let tree = editor.tree();
        let found = self.live_rig().humanoid(tree, model);
        let missing = found.missing();
        let title = if missing.is_empty() {
            t("rigging.humanoid")
        } else {
            format!("{}  ({} missing)", t("rigging.humanoid"), missing.len())
        };
        section(ui, "rig_humanoid", icon::PERSON, &title, |ui| {
            super::grid(ui, "rig_humanoid_grid", |ui| {
                for part in HumanBone::ALL {
                    super::label(ui, &part_name(part));
                    let set = self.rig.bones.get(part.key()).cloned();
                    let shown = found
                        .get(part)
                        .and_then(|b| tree.name(b).map(|n| n.to_owned()))
                        .unwrap_or_else(|| "—".into());
                    let text = match &set {
                        Some(_) => shown.clone(),
                        None => format!("{shown}  (auto)"),
                    };
                    egui::ComboBox::from_id_salt(("rig_part", part.key()))
                        .selected_text(RichText::new(text).color(if found.get(part).is_none() {
                            theme::WARN
                        } else {
                            theme::TEXT
                        }))
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(set.is_none(), t("rigging.auto"))
                                .clicked()
                            {
                                self.rig.bones.remove(part.key());
                            }
                            if ui
                                .selectable_label(set.as_deref() == Some(""), t("rigging.none"))
                                .clicked()
                            {
                                self.rig.bones.insert(part.key().to_owned(), String::new());
                            }
                            for name in names {
                                if ui
                                    .selectable_label(set.as_deref() == Some(name.as_str()), name)
                                    .clicked()
                                {
                                    self.rig.bones.insert(part.key().to_owned(), name.clone());
                                }
                            }
                        });
                    ui.end_row();
                }
            });
        });
    }

    /// `selected`: the selected bone, the tip of a chain added.
    fn chains_ui(&mut self, ui: &mut egui::Ui, names: &[String], selected: Option<String>) {
        section(
            ui,
            "rig_chains",
            icon::LINK_SIMPLE,
            &t("rigging.chains"),
            |ui| {
                let mut remove = None;
                for (i, chain) in self.rig.chains.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut chain.name).desired_width(80.0));
                        egui::ComboBox::from_id_salt(("rig_chain_tip", i))
                            .selected_text(&chain.tip)
                            .width(110.0)
                            .show_ui(ui, |ui| {
                                for part in HumanBone::ALL {
                                    ui.selectable_value(
                                        &mut chain.tip,
                                        part.key().to_owned(),
                                        part_name(part),
                                    );
                                }
                                ui.separator();
                                for name in names {
                                    ui.selectable_value(&mut chain.tip, name.clone(), name);
                                }
                            });
                        ui.add(
                            egui::DragValue::new(&mut chain.bones)
                                .range(1..=32)
                                .suffix(" bones"),
                        );
                        if ui.small_button(icon::X).clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    self.rig.chains.remove(i);
                }
                ui.horizontal(|ui| {
                    if ui
                        .button(format!("{}  {}", icon::PLUS, t("rigging.add_chain")))
                        .clicked()
                    {
                        let tip = selected
                            .clone()
                            .unwrap_or_else(|| HumanBone::LeftHand.key().to_owned());
                        let name = (1..)
                            .map(|n| format!("chain{n}"))
                            .find(|n| !self.rig.chains.iter().any(|c| c.name == *n))
                            .expect("a free name");
                        self.rig.chains.push(ChainDesc {
                            name,
                            tip,
                            bones: 2,
                        });
                    }
                    if ui
                        .button(format!(
                            "{}  {}",
                            icon::PERSON_SIMPLE_WALK,
                            t("rigging.limbs")
                        ))
                        .on_hover_text(t("rigging.limbs_hint"))
                        .clicked()
                    {
                        for (name, part) in [
                            ("left_arm", HumanBone::LeftHand),
                            ("right_arm", HumanBone::RightHand),
                            ("left_leg", HumanBone::LeftFoot),
                            ("right_leg", HumanBone::RightFoot),
                        ] {
                            if !self.rig.chains.iter().any(|c| c.name == name) {
                                self.rig.chains.push(ChainDesc {
                                    name: name.into(),
                                    tip: part.key().into(),
                                    bones: 2,
                                });
                            }
                        }
                    }
                });
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limbs_and_limits_make_a_valid_rig() {
        let mut panel = RiggingPanel::default();
        panel.rig.limits.push(LimitDesc {
            bone: "calf_l".into(),
            hinge: Some(HingeDesc {
                axis: [1.0, 0.0, 0.0],
                min: 0.0,
                max: 140.0,
            }),
            cone: None,
        });
        panel.rig.chains.push(ChainDesc {
            name: "left_leg".into(),
            tip: HumanBone::LeftFoot.key().into(),
            bones: 2,
        });
        panel.rig.check().unwrap();
        assert!(panel.is_dirty());
        assert_eq!(RigFile::from_toml(&panel.rig.to_toml()).unwrap(), panel.rig);
    }
}
