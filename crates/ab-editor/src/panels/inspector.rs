//! Properties of the selected node, and the scene's environment.
//!
//! Every field edits the node's scene-file description; [`Editor::apply_desc`] applies the
//! difference (live for transforms, scripts, visibility and particles, by rebuilding the
//! node otherwise).

use super::{grid, label, opt_bool, opt_color, opt_f32, opt_u32, section, vec3};
use crate::project::AssetKind;
use crate::scripts::ScriptDrag;
use crate::state::Editor;
use crate::theme;
use ab_engine::core::{Color, EulerRot, Quat, Transform3D, Vec3};
use ab_engine::effects::ParticleSettings;
use ab_engine::physics::{PhysicsBody3D, PhysicsNodes};
use ab_engine::scene::script_property;
use ab_engine::scene::NodeHandle;
use ab_engine::scene::ScriptPropertyDesc;
use ab_engine::scene_file::{
    relative_path, AudioDesc, BodyDesc, CameraDesc, CharacterDesc, EnvironmentDesc, JointDesc,
    LightDesc, MaterialDesc, MeshDesc, NodeDesc, ParticlesDesc, RagdollDesc, ShapeDesc, TimerDesc,
};
use ab_script_api as ffi;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;

#[derive(Default)]
pub struct InspectorPanel {
    /// Euler angles shown for a rotation, so dragging one does not flip the others.
    euler: Option<(NodeHandle, Quat, [f32; 3])>,
    groups_text: Option<(NodeHandle, String)>,
    /// The Script field's open button was pressed: (file, line).
    open_script: Option<(std::path::PathBuf, usize)>,
    /// Only the sections matching this are shown.
    filter: String,
}

impl InspectorPanel {
    pub fn show(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        let Some(node) = editor.selection.filter(|&n| editor.tree().is_valid(n)) else {
            super::empty_hint(
                ui,
                icon::CURSOR_CLICK,
                &crate::i18n::t("inspector.nothing_selected"),
            );
            return;
        };
        let tree = editor.tree();
        let class = tree.class_name(node).unwrap_or("Node").to_owned();
        let Some(old) = editor.describe_node(node) else {
            return;
        };
        let editable = editor.can_edit_properties(node);
        let override_target = editor.is_override_target(node);
        let is_3d = tree.is_node3d(node);
        let mut desc = old.clone();

        // Header: icon, editable name, class.
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(theme::class_icon(&class))
                    .size(22.0)
                    .color(theme::class_color(&class)),
            );
            ui.vertical(|ui| {
                // Typed into a buffer and applied once, on Enter or leaving the field: a
                // rename per keystroke would trim spaces and number names mid-typing.
                let id = ui.id().with(("inspector_name", node));
                let mut text = ui
                    .data(|d| d.get_temp::<String>(id))
                    .unwrap_or_else(|| old.name.clone());
                let response = ui.add_enabled(
                    editable && !override_target && node != editor.scene_root,
                    egui::TextEdit::singleline(&mut text)
                        .font(egui::TextStyle::Heading)
                        .desired_width(f32::INFINITY),
                );
                if response.has_focus() {
                    ui.data_mut(|d| d.insert_temp(id, text));
                } else {
                    ui.data_mut(|d| d.remove::<String>(id));
                    if response.lost_focus() && !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        desc.name = text;
                    }
                }
                let mut subtitle = class.clone();
                if let Some(instance) = &old.instance {
                    subtitle = crate::i18n::tf("inspector.instance_of", &[("path", instance)]);
                }
                ui.label(RichText::new(subtitle).small().color(theme::TEXT_DIM));
            });
        });
        let others = editor.selected.len().saturating_sub(1);
        if others > 0 {
            ui.label(
                RichText::new(format!(
                    "{}  {}",
                    icon::SELECTION_ALL,
                    crate::i18n::tf("inspector.many_selected", &[("count", &(others + 1))])
                ))
                .small()
                .color(theme::ACCENT),
            );
        }
        if !editable {
            ui.label(
                RichText::new(format!(
                    "{}  {}",
                    icon::INFO,
                    crate::i18n::t("inspector.inside_instance")
                ))
                .color(theme::WARN),
            );
            return;
        }
        if override_target {
            ui.label(
                RichText::new(format!(
                    "{}  {}",
                    icon::LINK_SIMPLE,
                    crate::i18n::t("inspector.prefab_override")
                ))
                .color(theme::ACCENT),
            );
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::FUNNEL_SIMPLE).color(theme::TEXT_DIM));
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text(crate::i18n::t("inspector.filter"))
                    .desired_width(ui.available_width() - 28.0),
            );
            if !self.filter.is_empty() && ui.small_button(icon::X).clicked() {
                self.filter.clear();
            }
        });
        ui.separator();

        let filter = self.filter.clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                super::with_section_filter(ui, &filter, |ui| {
                    if is_3d {
                        self.transform(ui, editor, node);
                    }
                    self.node_section(ui, editor, node, &mut desc);
                    class_sections(ui, editor, node, &class, &mut desc);
                    if is_3d {
                        rig_section(ui, editor, node, &mut desc);
                        lod_section(ui, editor, node, &class, &mut desc);
                    }
                    if let Some(rebuild) = ui.ctx().data_mut(|d| {
                        d.remove_temp::<Option<NodeHandle>>(egui::Id::new(LOD_REBUILD))
                            .flatten()
                    }) {
                        ab_engine::scene_file::lod::build_all(editor.tree_mut(), rebuild);
                        editor.info("Levels of detail rebuilt");
                    }
                    if let Some(bake) = ui.ctx().data_mut(|d| {
                        d.remove_temp::<Option<NodeHandle>>(egui::Id::new(BAKE_REQUEST))
                            .flatten()
                    }) {
                        match editor.bake_generator(bake) {
                            Ok(_) => return,
                            Err(err) => editor.fail(err),
                        }
                    }
                    if let Some(missing) = editor
                        .tree()
                        .component::<ab_engine::scene_file::prefab::MissingInstance>(node)
                        .map(|m| m.error.clone())
                    {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!(
                                "{}  Not loaded: {missing}. The instance and its overrides are \
                             kept and saved as they are.",
                                icon::WARNING
                            ))
                            .color(theme::WARN),
                        );
                    } else if old
                        .instance
                        .as_deref()
                        .is_some_and(ab_engine::scene_file::is_scene_path)
                    {
                        self.prefab_section(ui, editor, node, &old);
                    }
                })
            });

        if desc != old {
            editor.apply_desc(node, &old, desc);
        }
        if let Some((path, line)) = self.open_script.take() {
            crate::panels::assets::open_script(editor, &path, line);
        }
    }

    /// A prefab instance: its file, its overrides, and what can be done with them.
    fn prefab_section(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        node: NodeHandle,
        desc: &NodeDesc,
    ) {
        let path = desc.instance.clone().unwrap_or_default();
        let overrides = desc.overrides.len();
        section(ui, "prefab", icon::LINK_SIMPLE, "Prefab", |ui| {
            ui.label(RichText::new(&path).monospace().color(theme::ACCENT));
            let text = match overrides {
                0 => "No overrides: the instance is the prefab as it is.".to_owned(),
                1 => "1 node changed in this instance.".to_owned(),
                n => format!("{n} nodes changed in this instance."),
            };
            ui.label(RichText::new(text).color(theme::TEXT_DIM));
            for o in &desc.overrides {
                let target = o.get("path").and_then(|p| p.as_str()).unwrap_or("?");
                let keys: Vec<&str> = o
                    .keys()
                    .map(String::as_str)
                    .filter(|k| *k != "path")
                    .collect();
                ui.label(
                    RichText::new(format!(
                        "{}  {target}: {}",
                        icon::DOT_OUTLINE,
                        keys.join(", ")
                    ))
                    .small()
                    .color(theme::TEXT_DIM),
                );
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(format!("{}  Open prefab", icon::ARROW_SQUARE_OUT))
                    .clicked()
                {
                    crate::app::request_open(ui.ctx(), path.clone());
                }
                if ui
                    .add_enabled(
                        overrides > 0,
                        egui::Button::new(format!("{}  Apply to prefab", icon::UPLOAD_SIMPLE)),
                    )
                    .on_hover_text(
                        "Write this instance's changes into the prefab: every instance gets them",
                    )
                    .clicked()
                {
                    if let Err(err) = editor.apply_to_prefab(node) {
                        editor.fail(err);
                    }
                }
                if ui
                    .add_enabled(
                        overrides > 0,
                        egui::Button::new(format!("{}  Revert", icon::ARROW_COUNTER_CLOCKWISE)),
                    )
                    .on_hover_text("Drop this instance's changes: back to the prefab")
                    .clicked()
                {
                    editor.revert_overrides(node);
                }
            });
        });
    }

    fn transform(&mut self, ui: &mut egui::Ui, editor: &mut Editor, node: NodeHandle) {
        let Some(transform) = editor.tree().transform(node) else {
            return;
        };
        let euler = match self.euler {
            Some((n, q, e)) if n == node && q == transform.rotation => e,
            _ => {
                let (y, x, z) = transform.rotation.to_euler(EulerRot::YXZ);
                // `+ 0.0` turns -0.0 into 0.0 for display.
                [x, y, z].map(|r| (r.to_degrees() * 1e3).round() / 1e3 + 0.0)
            }
        };
        let mut position = transform.translation.to_array();
        let mut rotation = euler;
        let mut scale = transform.scale.to_array();
        let mut changed = false;
        section(
            ui,
            "transform",
            icon::ARROWS_OUT_CARDINAL,
            "Transform",
            |ui| {
                grid(ui, "transform_grid", |ui| {
                    label(ui, "Position");
                    changed |= vec3(ui, &mut position, 0.05, "");
                    ui.end_row();
                    label(ui, "Rotation");
                    changed |= vec3(ui, &mut rotation, 0.5, "°");
                    ui.end_row();
                    label(ui, "Scale");
                    changed |= vec3(ui, &mut scale, 0.01, "");
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    if ui
                        .small_button(crate::i18n::field("Reset position"))
                        .clicked()
                    {
                        position = [0.0; 3];
                        changed = true;
                    }
                    if ui
                        .small_button(crate::i18n::field("Reset rotation"))
                        .clicked()
                    {
                        rotation = [0.0; 3];
                        changed = true;
                    }
                    if ui.small_button(crate::i18n::field("Reset scale")).clicked() {
                        scale = [1.0; 3];
                        changed = true;
                    }
                });
            },
        );
        if changed {
            let quat = Quat::from_euler(
                EulerRot::YXZ,
                rotation[1].to_radians(),
                rotation[0].to_radians(),
                rotation[2].to_radians(),
            );
            self.euler = Some((node, quat, rotation));
            editor.set_transform(
                node,
                Transform3D {
                    translation: Vec3::from_array(position),
                    rotation: quat,
                    scale: Vec3::from_array(scale).max(Vec3::splat(1e-4)),
                },
            );
        }
    }

    fn node_section(
        &mut self,
        ui: &mut egui::Ui,
        editor: &Editor,
        node: NodeHandle,
        desc: &mut NodeDesc,
    ) {
        section(ui, "node", icon::CIRCLES_THREE, "Node", |ui| {
            grid(ui, "node_grid", |ui| {
                if editor.tree().is_node3d(node) || ui_kind(editor, node) {
                    label(ui, "Visible");
                    opt_visible(ui, &mut desc.visible);
                    ui.end_row();
                }

                label(ui, "Scripts");
                let mut list = desc.script_list();
                let mut changed = false;
                let area = ui.vertical(|ui| {
                    // One row per script, in the order they run.
                    let count = list.len();
                    let mut action: Option<(usize, i32)> = None; // (index, -1 up / 1 down / 0 remove)
                    for (i, name) in list.iter().enumerate() {
                        let source = editor.scripts.as_ref().and_then(|s| {
                            s.find(name).map(|(file, line)| (file.path.clone(), line))
                        });
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            ui.label(RichText::new(icon::SCROLL).color(theme::ACCENT));
                            ui.label(name);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .small_button(icon::X)
                                        .on_hover_text("Remove this script")
                                        .clicked()
                                    {
                                        action = Some((i, 0));
                                    }
                                    if ui
                                        .add_enabled(
                                            i + 1 < count,
                                            egui::Button::new(icon::CARET_DOWN).small(),
                                        )
                                        .on_hover_text("Run later")
                                        .clicked()
                                    {
                                        action = Some((i, 1));
                                    }
                                    if ui
                                        .add_enabled(
                                            i > 0,
                                            egui::Button::new(icon::CARET_UP).small(),
                                        )
                                        .on_hover_text("Run earlier")
                                        .clicked()
                                    {
                                        action = Some((i, -1));
                                    }
                                    if let Some((path, line)) = &source {
                                        if ui
                                            .small_button(icon::CODE)
                                            .on_hover_text("Open in the code editor")
                                            .clicked()
                                        {
                                            self.open_script = Some((path.clone(), *line));
                                        }
                                    }
                                },
                            );
                        });
                    }
                    match action {
                        Some((i, 0)) => {
                            list.remove(i);
                            changed = true;
                        }
                        Some((i, -1)) => {
                            list.swap(i, i - 1);
                            changed = true;
                        }
                        Some((i, _)) => {
                            list.swap(i, i + 1);
                            changed = true;
                        }
                        None => {}
                    }
                    let available: Vec<String> = editor
                        .script_names()
                        .into_iter()
                        .filter(|n| !list.contains(n))
                        .collect();
                    egui::ComboBox::from_id_salt("add_script")
                        .selected_text(
                            RichText::new(format!(
                                "{}  {}",
                                icon::PLUS,
                                crate::i18n::field("Add script…")
                            ))
                            .color(theme::TEXT_DIM),
                        )
                        .width(ui.available_width() - 16.0)
                        .show_ui(ui, |ui| {
                            if available.is_empty() {
                                ui.label(
                                    RichText::new("No other scripts: Assets → Create → Script")
                                        .color(theme::TEXT_DIM),
                                );
                            }
                            for name in available {
                                if ui.selectable_label(false, &name).clicked() {
                                    list.push(name);
                                    changed = true;
                                }
                            }
                        });
                });
                let area = area.response;
                if area.dnd_hover_payload::<ScriptDrag>().is_some() {
                    ui.painter().rect_stroke(
                        area.rect.expand(2.0),
                        3.0,
                        egui::Stroke::new(2.0_f32, theme::ACCENT),
                        egui::StrokeKind::Inside,
                    );
                }
                if let Some(script) = area.dnd_release_payload::<ScriptDrag>() {
                    if !list.contains(&script.0) {
                        list.push(script.0.clone());
                        changed = true;
                    }
                }
                if changed {
                    desc.set_script_list(list);
                }
                ui.end_row();

                label(ui, "Groups");
                let joined = desc.groups.join(", ");
                let text = match &mut self.groups_text {
                    Some((n, text)) if *n == node => text,
                    slot => &mut slot.insert((node, joined.clone())).1,
                };
                let response = ui.add(
                    egui::TextEdit::singleline(text)
                        .hint_text(crate::i18n::t("inspector.groups_hint"))
                        .desired_width(f32::INFINITY),
                );
                if !response.has_focus() && !response.lost_focus() {
                    // Not being typed in: show the node's groups as they are now (after an
                    // undo too), never write an old text back.
                    self.groups_text = None;
                } else if response.lost_focus() {
                    desc.groups = text
                        .split(',')
                        .map(str::trim)
                        .filter(|g| !g.is_empty())
                        .map(str::to_owned)
                        .collect();
                    self.groups_text = None;
                }
                ui.end_row();
            });
        });

        for script in desc.script_list() {
            let Some(props) = editor.script_metadata(&script) else {
                continue;
            };
            if props.iter().all(ScriptPropertyDesc::hidden) {
                continue;
            }
            let id = format!("script_props_{script}");
            section(ui, &id, icon::CODE, &script, |ui| {
                grid(ui, &format!("{id}_grid"), |ui| {
                    for prop in props.iter().filter(|p| !p.hidden()) {
                        let set = desc
                            .script_properties
                            .get(&script)
                            .and_then(|t| t.get(&prop.name));
                        let response = ui.label(RichText::new(&prop.name).color(theme::TEXT_DIM));
                        if !prop.doc.is_empty() {
                            response.on_hover_text(&prop.doc);
                        }
                        if let Some(value) = script_property(ui, prop, set) {
                            desc.script_properties
                                .entry(script.clone())
                                .or_default()
                                .insert(prop.name.clone(), value);
                        }
                        ui.end_row();
                    }
                });
            });
        }
    }
}

/// An exported script property's editor: shows the value set on the node (or the script's
/// default) and returns the new value when it is changed.
fn script_property(
    ui: &mut egui::Ui,
    prop: &ScriptPropertyDesc,
    set: Option<&toml::Value>,
) -> Option<toml::Value> {
    let current = set
        .or(prop.default.as_ref())
        .and_then(|v| script_property::to_ffi(Some(prop.kind), v));
    let floats = |v: &[f32]| {
        toml::Value::Array(
            v.iter()
                .map(|&x| toml::Value::Float(f64::from(x)))
                .collect(),
        )
    };
    let drags = |ui: &mut egui::Ui, values: &mut [f32]| {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for v in values.iter_mut() {
                changed |= ui
                    .add(
                        egui::DragValue::new(v)
                            .custom_parser(crate::panels::parse_finite)
                            .speed(0.01),
                    )
                    .changed();
            }
        });
        changed
    };
    match prop.kind {
        ffi::Tag::BOOL => {
            let mut v = current.and_then(|c| c.as_bool()).unwrap_or(false);
            ui.checkbox(&mut v, "")
                .changed()
                .then_some(toml::Value::Boolean(v))
        }
        ffi::Tag::INT => {
            let mut v = current.and_then(|c| c.as_int()).unwrap_or(0);
            ui.add(egui::DragValue::new(&mut v).custom_parser(crate::panels::parse_finite))
                .changed()
                .then_some(toml::Value::Integer(v))
        }
        ffi::Tag::FLOAT => {
            let mut v = current.and_then(|c| c.as_float()).unwrap_or(0.0);
            ui.add(
                egui::DragValue::new(&mut v)
                    .custom_parser(crate::panels::parse_finite)
                    .speed(0.01),
            )
            .changed()
            .then_some(toml::Value::Float(v))
        }
        ffi::Tag::VEC2 => {
            let v = current.and_then(|c| c.as_vec2()).unwrap_or_default();
            let mut a = [v.x, v.y];
            drags(ui, &mut a).then(|| floats(&a))
        }
        ffi::Tag::VEC3 => {
            let v = current.and_then(|c| c.as_vec3()).unwrap_or_default();
            let mut a = [v.x, v.y, v.z];
            vec3(ui, &mut a, 0.01, "").then(|| floats(&a))
        }
        ffi::Tag::QUAT => {
            let q = current.and_then(|c| c.as_quat()).unwrap_or_default();
            let mut a = [q.x, q.y, q.z, q.w];
            drags(ui, &mut a).then(|| floats(&a))
        }
        ffi::Tag::COLOR => {
            let c = current.and_then(|c| c.as_color()).unwrap_or(ffi::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            });
            // Linear, like the colors scripts receive.
            let mut rgba = [c.r, c.g, c.b, c.a];
            ui.color_edit_button_rgba_unmultiplied(&mut rgba)
                .changed()
                .then(|| {
                    let [r, g, b, a] = rgba;
                    toml::Value::String(script_property::color_hex(ffi::Color { r, g, b, a }))
                })
        }
        ffi::Tag::STR => {
            // SAFETY: the string borrows `set` or the default, both alive here.
            let mut text = current
                .and_then(|c| unsafe { c.as_str() })
                .unwrap_or("")
                .to_owned();
            ui.text_edit_singleline(&mut text)
                .changed()
                .then_some(toml::Value::String(text))
        }
        _ => {
            ui.label(RichText::new("(not editable)").color(theme::TEXT_DIM));
            None
        }
    }
}

fn opt_visible(ui: &mut egui::Ui, visible: &mut Option<bool>) {
    let mut v = visible.unwrap_or(true);
    if ui.checkbox(&mut v, "").changed() {
        *visible = (!v).then_some(false);
    }
}

fn class_sections(
    ui: &mut egui::Ui,
    editor: &Editor,
    node: NodeHandle,
    class: &str,
    desc: &mut NodeDesc,
) {
    match class {
        "Joint3D" => joint_section(
            ui,
            editor,
            node,
            desc.joint.get_or_insert_with(Default::default),
        ),
        "Ragdoll" => ragdoll_section(
            ui,
            editor,
            node,
            desc.ragdoll.get_or_insert_with(Default::default),
        ),
        "MeshInstance3D" if desc.mesh.is_some() => {
            mesh_section(ui, desc);
            let material = desc.material.get_or_insert_with(MaterialDesc::default);
            material_section(ui, editor, material);
            section(ui, "shadows", icon::SUN_HORIZON, "Shadows", |ui| {
                grid(ui, "shadows_grid", |ui| {
                    label(ui, "Cast shadows");
                    opt_bool(ui, &mut desc.cast_shadows, true);
                    ui.end_row();
                });
            });
        }
        ab_engine::scene_file::post_volume::POST_VOLUME => {
            let mut v = desc.post_volume.clone().unwrap_or_default();
            post_volume_section(ui, &mut v);
            if desc.post_volume.clone().unwrap_or_default() != v {
                desc.post_volume = Some(v);
            }
        }
        ab_engine::scene_file::generator::GENERATOR => {
            if let Some(mut g) = desc.generator.clone() {
                generator_section(ui, editor, node, &mut g);
                if desc.generator.as_ref() != Some(&g) {
                    desc.generator = Some(g);
                }
            }
        }
        ab_engine::scene_file::path::PATH_3D => {
            let mut path = desc.path.clone().unwrap_or_default();
            path_section(ui, editor, &mut path);
            if desc.path.clone().unwrap_or_default() != path {
                desc.path = Some(path);
            }
            let mut material = desc.material.clone().unwrap_or_default();
            material_section(ui, editor, &mut material);
            if desc.material.clone().unwrap_or_default() != material {
                desc.material = Some(material);
            }
        }
        "Camera3D" => camera_section(ui, desc.camera.get_or_insert_with(Default::default)),
        "DirectionalLight3D" | "OmniLight3D" | "SpotLight3D" => {
            light_section(ui, class, desc.light.get_or_insert_with(Default::default));
        }
        "StaticBody3D" | "RigidBody3D" | "AnimatableBody3D" | "CharacterBody3D" | "Area3D" => {
            body_section(ui, class, desc.body.get_or_insert_with(Default::default));
            if class == "CharacterBody3D" {
                character_section(ui, desc.character.get_or_insert_with(Default::default));
            }
        }
        "CollisionShape3D" => match desc.shape.as_mut() {
            Some(shape) => shape_section(ui, shape),
            // A hull or triangle mesh made by code: shown, never replaced by a box.
            None => {
                section(ui, "shape", icon::SHAPES, "Collision shape", |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{}  A convex hull or triangle mesh made by a script: it cannot \
                             be edited here.",
                            icon::INFO
                        ))
                        .color(theme::TEXT_DIM),
                    );
                });
            }
        },
        "AudioStreamPlayer" | "AudioStreamPlayer3D" => {
            audio_section(
                ui,
                editor,
                class,
                desc.audio.get_or_insert_with(Default::default),
            );
        }
        "AudioZone" => {
            audio_zone_section(
                ui,
                editor,
                desc.audio_zone.get_or_insert_with(Default::default),
            );
        }
        "Timer" => timer_section(
            ui,
            desc.timer.get_or_insert_with(|| TimerDesc {
                wait_time: 1.0,
                ..Default::default()
            }),
        ),
        "Particles3D" => particles_section(ui, desc.particles.get_or_insert_with(Default::default)),
        "CanvasLayer" => {
            let c = desc.canvas.get_or_insert_with(Default::default);
            section(ui, "canvas", icon::STACK, "Canvas layer", |ui| {
                grid(ui, "canvas_grid", |ui| {
                    label(ui, "Layer");
                    let mut layer = c.layer.unwrap_or(0);
                    if ui
                        .add(
                            egui::DragValue::new(&mut layer)
                                .custom_parser(crate::panels::parse_finite)
                                .range(-128..=128),
                        )
                        .on_hover_text("Higher layers draw on top")
                        .changed()
                    {
                        c.layer = (layer != 0).then_some(layer);
                    }
                    ui.end_row();
                });
            });
            if desc.canvas == Some(Default::default()) {
                desc.canvas = None;
            }
        }
        _ if ui_kind(editor, node) => ui_sections(ui, editor, node, class, desc),
        _ => {}
    }
    // Drop tables the UI created but left empty, so saved files stay minimal.
    if desc.camera == Some(CameraDesc::default()) {
        desc.camera = None;
    }
    if desc.light == Some(LightDesc::default()) {
        desc.light = None;
    }
    if desc.body == Some(BodyDesc::default()) {
        desc.body = None;
    }
    if desc.character == Some(CharacterDesc::default()) {
        desc.character = None;
    }
    if desc.material == Some(MaterialDesc::default()) {
        desc.material = None;
    }
}

fn joint_section(ui: &mut egui::Ui, editor: &Editor, node: NodeHandle, j: &mut JointDesc) {
    let tree = editor.tree();
    // Every physics body of the scene, by path from the joint.
    let bodies: Vec<String> = tree
        .subtree(editor.scene_root)
        .into_iter()
        .filter(|&n| tree.component::<PhysicsBody3D>(n).is_some())
        .filter_map(|n| relative_path(tree, node, n))
        .collect();
    section(ui, "joint", icon::LINK, "Joint", |ui| {
        grid(ui, "joint_grid", |ui| {
            label(ui, "Kind");
            let kind = j.kind.clone().unwrap_or_else(|| "pin".into());
            egui::ComboBox::from_id_salt("joint_kind")
                .selected_text(match kind.as_str() {
                    "hinge" => "Hinge",
                    "cone" => "Cone-twist",
                    "fixed" => "Fixed",
                    _ => "Pin",
                })
                .show_ui(ui, |ui| {
                    for (value, text) in [
                        ("pin", "Pin"),
                        ("hinge", "Hinge"),
                        ("cone", "Cone-twist"),
                        ("fixed", "Fixed"),
                    ] {
                        if ui.selectable_label(kind == value, text).clicked() {
                            j.kind = Some(value.into());
                        }
                    }
                });
            ui.end_row();
            for (title, path, id) in [
                ("Body A", &mut j.body_a, "body_a"),
                ("Body B", &mut j.body_b, "body_b"),
            ] {
                label(ui, title);
                egui::ComboBox::from_id_salt(id)
                    .selected_text(if path.is_empty() {
                        crate::i18n::field("None")
                    } else {
                        path.clone()
                    })
                    .width(ui.available_width() - 4.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(path.is_empty(), crate::i18n::field("None"))
                            .clicked()
                        {
                            path.clear();
                        }
                        for body in &bodies {
                            if ui.selectable_label(path == body, body).clicked() {
                                *path = body.clone();
                            }
                        }
                    });
                ui.end_row();
            }
            match kind.as_str() {
                "hinge" => {
                    label(ui, "Limits (°)");
                    let mut limits = j.limits.unwrap_or([-90.0, 90.0]);
                    let mut changed = false;
                    ui.horizontal(|ui| {
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut limits[0])
                                    .custom_parser(crate::panels::parse_finite)
                                    .range(-180.0..=180.0)
                                    .suffix("°"),
                            )
                            .changed();
                        ui.label(RichText::new(icon::ARROW_RIGHT).color(theme::TEXT_DIM));
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut limits[1])
                                    .custom_parser(crate::panels::parse_finite)
                                    .range(-180.0..=180.0)
                                    .suffix("°"),
                            )
                            .changed();
                    });
                    if changed {
                        j.limits = Some(limits);
                    }
                    ui.end_row();
                }
                "cone" => {
                    label(ui, "Swing (°)");
                    super::opt_f32(ui, &mut j.swing, 45.0, 0.0..=180.0, 0.5);
                    ui.end_row();
                    label(ui, "Twist (°)");
                    super::opt_f32(ui, &mut j.twist, 30.0, 0.0..=180.0, 0.5);
                    ui.end_row();
                }
                _ => {}
            }
            label(ui, "Bodies collide");
            opt_bool(ui, &mut j.collide, false);
            ui.end_row();
        });
        ui.label(
            RichText::new(
                "The joint's position is the anchor; its X axis is the hinge / twist axis.",
            )
            .small()
            .color(theme::TEXT_DIM),
        );
    });
}

fn ragdoll_section(ui: &mut egui::Ui, editor: &Editor, node: NodeHandle, r: &mut RagdollDesc) {
    use ab_engine::ragdoll::{humanoid_map, skeleton_bones};
    let tree = editor.tree();
    let parts = tree.ragdoll_parts(node).len();
    // Characters to build on: the models in the scene that have a skin.
    let mut characters: Vec<NodeHandle> = tree
        .subtree(editor.scene_root)
        .into_iter()
        .filter(|&n| tree.component::<ab_engine::rendering::Skin>(n).is_some())
        .filter_map(|n| editor.instance_owner(n))
        .collect();
    characters.dedup();
    let model = r
        .skeleton
        .as_deref()
        .and_then(|path| tree.get_node(node, path));
    section(ui, "ragdoll", icon::PERSON_ARMS_SPREAD, "Ragdoll", |ui| {
        grid(ui, "ragdoll_grid", |ui| {
            label(ui, "Limp on start");
            opt_bool(ui, &mut r.active, false);
            ui.end_row();
            label(ui, "Character");
            let shown = model
                .and_then(|m| tree.name(m).map(|n| n.to_owned()))
                .or_else(|| r.skeleton.clone().map(|s| format!("{s} (not found)")))
                .unwrap_or_else(|| "None (bodies built by hand)".into());
            egui::ComboBox::from_id_salt("ragdoll_character")
                .selected_text(shown)
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(r.skeleton.is_none(), "None (bodies built by hand)")
                        .clicked()
                    {
                        r.skeleton = None;
                        r.bones.clear();
                    }
                    for &character in &characters {
                        let name = tree
                            .name(character)
                            .map(|n| n.to_owned())
                            .unwrap_or_default();
                        let path = relative_path(tree, node, character);
                        if ui
                            .selectable_label(model == Some(character), name)
                            .clicked()
                        {
                            r.skeleton = path;
                            r.bones.clear();
                        }
                    }
                });
            ui.end_row();
            if r.skeleton.is_some() {
                label(ui, "Mass (kg)");
                opt_f32(ui, &mut r.mass, 70.0, 1.0..=1000.0, 0.5);
                ui.end_row();
                label(ui, "Thickness");
                opt_f32(ui, &mut r.thickness, 1.0, 0.3..=3.0, 0.01);
                ui.end_row();
            }
            label(ui, "Parts");
            ui.label(format!("{parts} bodies"));
            ui.end_row();
        });
        ui.label(
            RichText::new(if r.skeleton.is_some() {
                "Bodies shaped like a person are made from the character's skeleton when the scene loads. Off: they follow the animation as hit boxes until a script calls go_limp."
            } else {
                "Off: parts stay kinematic (follow their bones, act as hit boxes) until a script calls ragdoll::go_limp. Pick a character to build the bodies from its skeleton."
            })
            .small()
            .color(theme::TEXT_DIM),
        );
        if characters.is_empty() && r.skeleton.is_none() {
            ui.label(
                RichText::new(
                    "Drop a rigged model (glTF with a skin) into the scene to pick it here.",
                )
                .small()
                .color(theme::TEXT_DIM),
            );
        }
    });
    let Some(model) = model else {
        return;
    };
    let bones = skeleton_bones(tree, model);
    let automatic = humanoid_map(tree, model, &Default::default());
    let current = humanoid_map(tree, model, &r.bones);
    let bone_name = |b: Option<NodeHandle>| b.and_then(|b| tree.name(b).map(|n| n.to_owned()));
    section(ui, "ragdoll_bones", icon::BONE, "Bone map", |ui| {
        let missing = current.missing();
        if missing.is_empty() {
            ui.label(
                RichText::new(format!(
                    "{}  Every bone a ragdoll needs is mapped",
                    icon::CHECK_CIRCLE
                ))
                .color(theme::PLAY),
            );
        } else {
            ui.label(
                RichText::new(format!(
                    "{}  Missing: {}. Pick them below.",
                    icon::WARNING,
                    missing
                        .iter()
                        .map(|b| b.label())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .color(theme::WARN),
            );
        }
        grid(ui, "ragdoll_bones_grid", |ui| {
            for slot in ab_engine::physics::HumanBone::ALL {
                let set_by_hand = r.bones.get(slot.key()).cloned();
                let mapped = bone_name(current.get(slot));
                let text = RichText::new(slot.label());
                let text = if slot.is_required() && mapped.is_none() {
                    text.color(theme::WARN)
                } else {
                    text
                };
                ui.label(text);
                let shown = match (&mapped, &set_by_hand) {
                    (Some(name), Some(_)) => name.clone(),
                    (Some(name), None) => format!("{name}  (auto)"),
                    (None, _) => "—".into(),
                };
                egui::ComboBox::from_id_salt(("ragdoll_bone", slot.key()))
                    .selected_text(shown)
                    .width(ui.available_width())
                    .height(320.0)
                    .show_ui(ui, |ui| {
                        let auto =
                            bone_name(automatic.get(slot)).unwrap_or_else(|| "none found".into());
                        if ui
                            .selectable_label(set_by_hand.is_none(), format!("Automatic ({auto})"))
                            .clicked()
                        {
                            r.bones.remove(slot.key());
                        }
                        if ui
                            .selectable_label(
                                set_by_hand.as_deref() == Some(""),
                                crate::i18n::field("None"),
                            )
                            .clicked()
                        {
                            r.bones.insert(slot.key().into(), String::new());
                        }
                        ui.separator();
                        for &bone in &bones {
                            let name = tree.name(bone).map(|n| n.to_owned()).unwrap_or_default();
                            let chosen = set_by_hand.as_deref() == Some(name.as_str());
                            if ui.selectable_label(chosen, &name).clicked() {
                                r.bones.insert(slot.key().into(), name);
                            }
                        }
                    });
                ui.end_row();
            }
        });
        if !r.bones.is_empty()
            && ui
                .button(format!("{}  All automatic", icon::MAGIC_WAND))
                .clicked()
        {
            r.bones.clear();
        }
    });
}

fn mesh_section(ui: &mut egui::Ui, desc: &mut NodeDesc) {
    let Some(mesh) = desc.mesh.as_mut() else {
        return;
    };
    section(ui, "mesh", icon::CUBE, "Mesh", |ui| {
        grid(ui, "mesh_grid", |ui| {
            label(ui, "Primitive");
            let kind = match mesh {
                MeshDesc::Cube { .. } => "Cube",
                MeshDesc::Sphere { .. } => "Sphere",
                MeshDesc::Plane { .. } => "Plane",
            };
            egui::ComboBox::from_id_salt("primitive")
                .selected_text(kind)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(kind == "Cube", "Cube").clicked() && kind != "Cube" {
                        *mesh = MeshDesc::Cube { size: 1.0 };
                    }
                    if ui.selectable_label(kind == "Sphere", "Sphere").clicked() && kind != "Sphere"
                    {
                        *mesh = MeshDesc::Sphere {
                            radius: 0.5,
                            segments: 32,
                            rings: 16,
                        };
                    }
                    if ui.selectable_label(kind == "Plane", "Plane").clicked() && kind != "Plane" {
                        *mesh = MeshDesc::Plane {
                            size: 1.0,
                            subdivisions: 1,
                        };
                    }
                });
            ui.end_row();
            match mesh {
                MeshDesc::Cube { size } => {
                    label(ui, "Size");
                    ui.add(
                        egui::DragValue::new(size)
                            .custom_parser(crate::panels::parse_finite)
                            .range(0.01..=1000.0)
                            .speed(0.01),
                    );
                    ui.end_row();
                }
                MeshDesc::Sphere {
                    radius,
                    segments,
                    rings,
                } => {
                    label(ui, "Radius");
                    ui.add(
                        egui::DragValue::new(radius)
                            .custom_parser(crate::panels::parse_finite)
                            .range(0.01..=1000.0)
                            .speed(0.01),
                    );
                    ui.end_row();
                    label(ui, "Segments");
                    ui.add(
                        egui::DragValue::new(segments)
                            .custom_parser(crate::panels::parse_finite)
                            .range(3..=128),
                    );
                    ui.end_row();
                    label(ui, "Rings");
                    ui.add(
                        egui::DragValue::new(rings)
                            .custom_parser(crate::panels::parse_finite)
                            .range(2..=64),
                    );
                    ui.end_row();
                }
                MeshDesc::Plane { size, subdivisions } => {
                    label(ui, "Size");
                    ui.add(
                        egui::DragValue::new(size)
                            .custom_parser(crate::panels::parse_finite)
                            .range(0.01..=1000.0)
                            .speed(0.01),
                    );
                    ui.end_row();
                    label(ui, "Subdivisions");
                    ui.add(
                        egui::DragValue::new(subdivisions)
                            .custom_parser(crate::panels::parse_finite)
                            .range(1..=256),
                    );
                    ui.end_row();
                }
            }
        });
    });
}

/// A pick of one of the project's files of `kind` (or none).
fn asset_choice(
    ui: &mut egui::Ui,
    editor: &Editor,
    id: &str,
    kind: AssetKind,
    value: &mut Option<String>,
) {
    let shown = value.clone().unwrap_or_else(|| crate::i18n::field("None"));
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(ui.available_width() - 4.0)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(value.is_none(), crate::i18n::field("None"))
                .clicked()
            {
                *value = None;
            }
            for asset in editor.assets.of_kind(kind) {
                let chosen = value.as_deref() == Some(asset.path.as_str());
                if ui.selectable_label(chosen, &asset.path).clicked() {
                    *value = Some(asset.path.clone());
                }
            }
        });
}

fn material_section(ui: &mut egui::Ui, editor: &Editor, m: &mut MaterialDesc) {
    section(ui, "material", icon::PALETTE, "Material", |ui| {
        grid(ui, "material_grid", |ui| {
            label(ui, "Material file");
            asset_choice(
                ui,
                editor,
                "material_file",
                AssetKind::Material,
                &mut m.file,
            );
            ui.end_row();
            if m.file.is_some() {
                label(ui, "");
                ui.label(
                    RichText::new("Values set below override the file's.")
                        .small()
                        .color(theme::TEXT_DIM),
                );
                ui.end_row();
            }
            label(ui, "Albedo map");
            asset_choice(
                ui,
                editor,
                "albedo_texture",
                AssetKind::Image,
                &mut m.albedo_texture,
            );
            ui.end_row();
            label(ui, "Normal map");
            asset_choice(
                ui,
                editor,
                "normal_texture",
                AssetKind::Image,
                &mut m.normal_texture,
            );
            ui.end_row();
            if m.normal_texture.is_some() {
                label(ui, "Normal strength");
                opt_f32(ui, &mut m.normal_scale, 1.0, 0.0..=4.0, 0.01);
                ui.end_row();
            }
            label(ui, "Metal/rough map");
            asset_choice(
                ui,
                editor,
                "metallic_roughness_texture",
                AssetKind::Image,
                &mut m.metallic_roughness_texture,
            );
            ui.end_row();
            label(ui, "Albedo");
            opt_color(ui, &mut m.color, Color::WHITE, true);
            ui.end_row();
            label(ui, "Roughness");
            slider(ui, &mut m.roughness, 0.5, 0.0..=1.0);
            ui.end_row();
            label(ui, "Metallic");
            slider(ui, &mut m.metallic, 0.0, 0.0..=1.0);
            ui.end_row();
            label(ui, "Emission");
            opt_color(ui, &mut m.emission, Color::BLACK, false);
            ui.end_row();
            label(ui, "Emission energy");
            opt_f32(ui, &mut m.emission_energy, 1.0, 0.0..=100.0, 0.05);
            ui.end_row();
            label(ui, "Unshaded");
            opt_bool(ui, &mut m.unshaded, false);
            ui.end_row();
            label(ui, "Double sided");
            opt_bool(ui, &mut m.double_sided, false);
            ui.end_row();
            label(ui, "Shader");
            asset_choice(
                ui,
                editor,
                "material_shader",
                AssetKind::Shader,
                &mut m.shader,
            );
            ui.end_row();
            if let Some(path) = m.shader.clone().filter(|p| !p.is_empty()) {
                shader_params(ui, editor, &path, m);
            }
        });
    });
}

/// What a shader file declares, read again when it changes: its `// param` lines and
/// whether it builds.
#[derive(Clone)]
struct ShaderInfo {
    modified: Option<std::time::SystemTime>,
    params: Vec<ab_engine::rendering::ShaderParam>,
    error: Option<String>,
}

fn shader_info(ui: &egui::Ui, editor: &Editor, path: &str) -> ShaderInfo {
    let file = editor.project().join(path);
    let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
    let id = egui::Id::new(("shader-info", path));
    if let Some(info) = ui
        .data(|d| d.get_temp::<ShaderInfo>(id))
        .filter(|i| i.modified == modified)
    {
        return info;
    }
    let info = match std::fs::read_to_string(&file) {
        Ok(code) => ShaderInfo {
            modified,
            params: ab_engine::rendering::shader::params(&code),
            error: ab_engine::rendering::shader::validate(&code)
                .err()
                .map(|e| e.to_string()),
        },
        Err(err) => ShaderInfo {
            modified,
            params: Vec::new(),
            error: Some(format!("{path}: {err}")),
        },
    };
    ui.data_mut(|d| d.insert_temp(id, info.clone()));
    info
}

/// The shader's parameters (a value left at the default is not written).
fn shader_params(ui: &mut egui::Ui, editor: &Editor, path: &str, m: &mut MaterialDesc) {
    let info = shader_info(ui, editor, path);
    if let Some(error) = &info.error {
        label(ui, "");
        ui.label(
            RichText::new(format!("{}  {error}", icon::WARNING))
                .small()
                .color(theme::ERROR),
        )
        .on_hover_text("Drawn with the standard surface until the shader builds");
        ui.end_row();
    }
    for p in &info.params {
        label(ui, &p.name);
        let current = m.params.as_ref().and_then(|v| v.get(&p.name).copied());
        let mut value = current.unwrap_or(p.default);
        let changed = ui
            .add(egui::DragValue::new(&mut value).speed(0.01))
            .on_hover_text(format!("Default {}", p.default))
            .changed();
        if changed {
            let params = m.params.get_or_insert_with(Default::default);
            params.insert(p.name.clone(), value);
        }
        ui.end_row();
    }
    if m.params.as_ref().is_some_and(|v| v.is_empty()) {
        m.params = None;
    }
}

/// A character's rig, animation state machine and IK chains: shown for models with a
/// skeleton and nodes that have any of them.
fn rig_section(ui: &mut egui::Ui, editor: &Editor, node: NodeHandle, desc: &mut NodeDesc) {
    let tree = editor.tree();
    let skinned = tree
        .subtree(node)
        .into_iter()
        .any(|n| tree.component::<ab_engine::rendering::Skin>(n).is_some());
    let has_any = desc.rig.is_some() || desc.state_machine.is_some() || !desc.ik.is_empty();
    if !skinned && !has_any {
        return;
    }
    section(
        ui,
        "rig",
        icon::PERSON_SIMPLE_WALK,
        "Animation and IK",
        |ui| {
            grid(ui, "rig_grid", |ui| {
                label(ui, "State machine");
                asset_choice(
                    ui,
                    editor,
                    "state_machine",
                    AssetKind::StateMachine,
                    &mut desc.state_machine,
                );
                ui.end_row();
                label(ui, "Rig");
                asset_choice(ui, editor, "rig_file", AssetKind::Rig, &mut desc.rig);
                ui.end_row();
            });
            if let Some(machine) = tree.component::<ab_engine::state_machine::StateMachine>(node) {
                if editor.is_playing() {
                    ui.label(
                        RichText::new(format!(
                            "{}  {}",
                            icon::PLAY,
                            machine.state_name().unwrap_or("—")
                        ))
                        .color(theme::PLAY),
                    );
                }
            }
            ui.add_space(4.0);
            ui.label(RichText::new("IK chains").color(theme::TEXT_DIM));
            let mut remove = None;
            for (i, ik) in desc.ik.iter_mut().enumerate() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    grid(ui, &format!("ik_grid_{i}"), |ui| {
                        label(ui, "Tip bone");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut ik.tip)
                                    .hint_text("left_foot / bone name")
                                    .desired_width(120.0),
                            );
                            egui::ComboBox::from_id_salt(("ik_part", i))
                                .selected_text(icon::PERSON)
                                .width(24.0)
                                .show_ui(ui, |ui| {
                                    for part in ab_engine::physics::HumanBone::ALL {
                                        if ui
                                            .selectable_label(ik.tip == part.key(), part.key())
                                            .clicked()
                                        {
                                            ik.tip = part.key().to_owned();
                                            ik.chain = None;
                                        }
                                    }
                                });
                        });
                        ui.end_row();
                        label(ui, "Bones");
                        ui.add(egui::DragValue::new(&mut ik.bones).range(1..=32));
                        ui.end_row();
                        let path = |ui: &mut egui::Ui,
                                    name: &str,
                                    value: &mut Option<String>,
                                    hint: &str| {
                            label(ui, name);
                            let mut text = value.clone().unwrap_or_default();
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut text)
                                        .hint_text(hint)
                                        .desired_width(150.0),
                                )
                                .changed()
                            {
                                *value = (!text.trim().is_empty()).then(|| text.trim().to_owned());
                            }
                            ui.end_row();
                        };
                        path(ui, "Target", &mut ik.target, "../HandTarget");
                        path(ui, "Pole", &mut ik.pole, "../ElbowHint");
                        path(ui, "Rig chain", &mut ik.chain, "left_leg");
                        label(ui, "On the ground");
                        ui.checkbox(&mut ik.ground, "").on_hover_text(
                            "Foot placement: the tip stands on the colliders below it",
                        );
                        ui.end_row();
                        label(ui, "Turn to target");
                        ui.checkbox(&mut ik.align, "");
                        ui.end_row();
                        label(ui, "Solver");
                        let mut solver = ik.solver.unwrap_or_default();
                        ui.horizontal(|ui| {
                            ui.selectable_value(
                                &mut solver,
                                ab_engine::ik::Solver::TwoBone,
                                "Two bone",
                            );
                            ui.selectable_value(
                                &mut solver,
                                ab_engine::ik::Solver::Fabrik,
                                "FABRIK",
                            );
                        });
                        ik.solver = (solver != ab_engine::ik::Solver::default()
                            || ik.solver.is_some())
                        .then_some(solver);
                        ui.end_row();
                        label(ui, "Weight");
                        ui.add(egui::Slider::new(&mut ik.weight, 0.0..=1.0));
                        ui.end_row();
                    });
                    if ui
                        .button(
                            RichText::new(format!("{}  Remove", icon::TRASH)).color(theme::ERROR),
                        )
                        .clicked()
                    {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                desc.ik.remove(i);
            }
            ui.horizontal(|ui| {
                if ui.button(format!("{}  IK chain", icon::PLUS)).clicked() {
                    desc.ik.push(ab_engine::ik::IkDesc {
                        tip: ab_engine::physics::HumanBone::RightHand.key().into(),
                        ..Default::default()
                    });
                }
                if ui
                    .button(format!("{}  Feet on the ground", icon::FOOTPRINTS))
                    .on_hover_text("Two chains that keep the feet on slopes and steps")
                    .clicked()
                {
                    for part in [
                        ab_engine::physics::HumanBone::LeftFoot,
                        ab_engine::physics::HumanBone::RightFoot,
                    ] {
                        if !desc.ik.iter().any(|c| c.tip == part.key()) {
                            desc.ik.push(ab_engine::ik::IkDesc {
                                tip: part.key().into(),
                                ground: true,
                                ..Default::default()
                            });
                        }
                    }
                }
            });
            if let Some(chains) = tree.component::<ab_engine::ik::IkChains>(node) {
                for error in &chains.errors {
                    ui.label(
                        RichText::new(format!("{}  {error}", icon::WARNING))
                            .small()
                            .color(theme::WARN),
                    );
                }
            }
        },
    );
}

/// Asks the inspector to build a node's levels of detail again.
const LOD_REBUILD: &str = "ab-editor.lod-rebuild";

/// Levels of detail of the meshes in a node, and an HLOD group's settings, with their
/// triangle counts and what is wrong with them.
fn lod_section(
    ui: &mut egui::Ui,
    editor: &Editor,
    node: NodeHandle,
    class: &str,
    desc: &mut NodeDesc,
) {
    use ab_engine::rendering::{HlodProxy, MeshInstance3D, MeshLod};
    use ab_engine::scene_file::lod::{HlodDesc, LodDesc, HLOD};
    let tree = editor.tree();
    let meshes: Vec<NodeHandle> = tree
        .subtree(node)
        .into_iter()
        .filter(|&n| {
            tree.component::<MeshInstance3D>(n).is_some()
                && tree.component::<HlodProxy>(n).is_none()
        })
        .collect();
    if meshes.is_empty() && class != HLOD {
        return;
    }
    let Some(assets) = tree
        .world()
        .resource::<ab_engine::rendering::RenderAssets>()
    else {
        return;
    };
    let triangles = |mesh| assets.mesh(mesh).map_or(0, |m| m.indices.len() / 3);
    section(ui, "lod", icon::STACK, "Level of detail", |ui| {
        if class == HLOD {
            let h = desc.hlod.get_or_insert_with(HlodDesc::default);
            grid(ui, "hlod_grid", |ui| {
                label(ui, "Switch distance (m)");
                ui.add(
                    egui::DragValue::new(&mut h.distance)
                        .range(1.0..=10_000.0)
                        .speed(0.5),
                );
                ui.end_row();
                label(ui, "Merged detail");
                ui.add(egui::Slider::new(&mut h.detail, 0.01..=1.0));
                ui.end_row();
            });
            let proxies: Vec<usize> = tree
                .children(node)
                .into_iter()
                .filter(|&c| tree.component::<HlodProxy>(c).is_some())
                .filter_map(|c| {
                    tree.component::<MeshInstance3D>(c)
                        .map(|m| triangles(m.mesh))
                })
                .collect();
            let original: usize = meshes
                .iter()
                .filter_map(|&n| {
                    tree.component::<MeshInstance3D>(n)
                        .map(|m| triangles(m.mesh))
                })
                .sum();
            ui.label(
                RichText::new(format!(
                    "{} meshes, {original} triangles → {} merged meshes, {} triangles far away",
                    meshes.len(),
                    proxies.len(),
                    proxies.iter().sum::<usize>()
                ))
                .small()
                .color(theme::TEXT_DIM),
            );
        }
        let mut on = desc.lod.is_some();
        if ui
            .checkbox(&mut on, "Simpler meshes when small on screen")
            .changed()
        {
            desc.lod = on.then(|| LodDesc {
                levels: vec![[0.3, 0.5], [0.1, 0.2]],
                cull: Some(0.01),
            });
        }
        if let Some(lod) = &mut desc.lod {
            let mut remove = None;
            grid(ui, "lod_grid", |ui| {
                label(ui, "Below screen size");
                ui.label(RichText::new("Detail").color(theme::TEXT_DIM));
                ui.end_row();
                for (i, level) in lod.levels.iter_mut().enumerate() {
                    ui.add(
                        egui::DragValue::new(&mut level[0])
                            .range(0.001..=2.0)
                            .speed(0.005),
                    );
                    ui.horizontal(|ui| {
                        ui.add(egui::Slider::new(&mut level[1], 0.01..=1.0));
                        if ui.small_button(icon::X).clicked() {
                            remove = Some(i);
                        }
                    });
                    ui.end_row();
                }
                label(ui, "Hide below");
                super::opt_f32(ui, &mut lod.cull, 0.01, 0.0..=1.0, 0.001);
                ui.end_row();
            });
            if let Some(i) = remove {
                lod.levels.remove(i);
            }
            if lod.levels.len() < ab_engine::rendering::MAX_LOD_LEVELS
                && ui.small_button(format!("{}  Level", icon::PLUS)).clicked()
            {
                let last = lod.levels.last().copied().unwrap_or([0.5, 1.0]);
                lod.levels.push([last[0] * 0.5, last[1] * 0.5]);
            }
            match lod.check() {
                Err(err) => {
                    ui.label(RichText::new(format!("{}  {err}", icon::WARNING)).color(theme::WARN));
                }
                Ok(()) => {
                    // What the levels made, for the first mesh.
                    let made = meshes.iter().find_map(|&n| {
                        Some((
                            tree.component::<MeshInstance3D>(n)?.mesh,
                            tree.component::<MeshLod>(n)?.clone(),
                        ))
                    });
                    if let Some((base, made)) = made {
                        let mut counts = vec![triangles(base)];
                        counts.extend(made.levels.iter().map(|l| triangles(l.mesh)));
                        ui.label(
                            RichText::new(format!(
                                "Triangles per level: {}",
                                counts
                                    .iter()
                                    .map(|c| c.to_string())
                                    .collect::<Vec<_>>()
                                    .join(" → ")
                            ))
                            .small()
                            .color(theme::TEXT_DIM),
                        );
                    }
                }
            }
            ui.label(
                RichText::new("Screen size: the part of the view's height the mesh covers (1 = all of it). View → LOD levels shows the level each mesh is drawn at.")
                    .small()
                    .color(theme::TEXT_DIM),
            );
        }
        if ui
            .button(format!("{}  Rebuild", icon::ARROWS_CLOCKWISE))
            .on_hover_text(
                "Makes the simpler and merged meshes again (after moving the group's meshes)",
            )
            .clicked()
        {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new(LOD_REBUILD), Some(node)));
        }
    });
}

/// A post-processing volume: its box, how it blends, and the values it sets (unset values
/// keep the scene's).
fn post_volume_section(
    ui: &mut egui::Ui,
    v: &mut ab_engine::scene_file::post_volume::PostVolumeDesc,
) {
    let post = ab_engine::rendering::PostProcess::default();
    section(
        ui,
        "post_volume",
        icon::APERTURE,
        "Post-Process Volume",
        |ui| {
            grid(ui, "post_volume_grid", |ui| {
                label(ui, "Everywhere");
                ui.checkbox(&mut v.global, "")
                    .on_hover_text("Counts wherever the camera is, not only in the box");
                ui.end_row();
                if !v.global {
                    label(ui, "Size (m)");
                    vec3(ui, &mut v.size, 0.05, "");
                    for s in &mut v.size {
                        *s = s.max(0.0);
                    }
                    ui.end_row();
                    label(ui, "Blend distance (m)");
                    ui.add(
                        egui::DragValue::new(&mut v.blend)
                            .range(0.0..=100.0)
                            .speed(0.05),
                    )
                    .on_hover_text("Outside the box it fades out over this distance");
                    ui.end_row();
                }
                label(ui, "Priority");
                ui.add(egui::DragValue::new(&mut v.priority).range(-100..=100))
                    .on_hover_text("Higher applies over lower where volumes overlap");
                ui.end_row();
                label(ui, "Weight");
                ui.add(egui::Slider::new(&mut v.weight, 0.0..=1.0));
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new("Values (unset: the scene's)")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            grid(ui, "post_volume_values", |ui| {
                let dof = post.depth_of_field;
                let rows: [(&str, &mut Option<f32>, f32, f32, f64); 12] = [
                    ("Exposure", &mut v.exposure, post.exposure, 8.0, 0.01),
                    ("Contrast", &mut v.contrast, post.contrast, 4.0, 0.01),
                    ("Saturation", &mut v.saturation, post.saturation, 4.0, 0.01),
                    (
                        "Vignette",
                        &mut v.vignette,
                        post.vignette_intensity,
                        1.0,
                        0.01,
                    ),
                    ("Bloom", &mut v.bloom, post.bloom.intensity, 2.0, 0.005),
                    (
                        "Bloom threshold",
                        &mut v.bloom_threshold,
                        post.bloom.threshold,
                        16.0,
                        0.01,
                    ),
                    (
                        "Chromatic aberration",
                        &mut v.chromatic_aberration,
                        0.0,
                        4.0,
                        0.01,
                    ),
                    ("Film grain", &mut v.grain, 0.0, 4.0, 0.01),
                    ("Depth of field (px)", &mut v.depth_of_field, 0.0, 32.0, 0.1),
                    (
                        "Focus distance (m)",
                        &mut v.focus_distance,
                        dof.focus_distance,
                        500.0,
                        0.05,
                    ),
                    (
                        "Focus range (m)",
                        &mut v.focus_range,
                        dof.focus_range,
                        200.0,
                        0.05,
                    ),
                    ("Motion blur", &mut v.motion_blur, 0.0, 4.0, 0.01),
                ];
                for (name, value, default, max, speed) in rows {
                    label(ui, name);
                    opt_f32(ui, value, default, 0.0..=max, speed);
                    ui.end_row();
                }
                label(ui, "Tint");
                opt_color(ui, &mut v.tint, post.tint, false);
                ui.end_row();
            });
        },
    );
}

/// Asks the inspector to bake a generator (the sections only read the editor).
const BAKE_REQUEST: &str = "ab-editor.bake-generator";

/// A generator: its kind, seed, and its parameters as the generator describes them.
fn generator_section(
    ui: &mut egui::Ui,
    editor: &Editor,
    node: NodeHandle,
    g: &mut ab_engine::scene_file::GeneratorDesc,
) {
    use ab_engine::procgen;
    let Some(generator) = procgen::generator(&g.kind) else {
        ui.label(RichText::new(format!("Unknown generator `{}`", g.kind)).color(theme::ERROR));
        return;
    };
    section(ui, "generator", icon::DICE_FIVE, generator.title(), |ui| {
        grid(ui, "generator_grid", |ui| {
            label(ui, "Generator");
            egui::ComboBox::from_id_salt("generator_kind")
                .selected_text(generator.title())
                .show_ui(ui, |ui| {
                    for other in procgen::GENERATORS {
                        if ui
                            .selectable_label(other.kind() == g.kind, other.title())
                            .clicked()
                            && other.kind() != g.kind
                        {
                            // Another generator: its own parameters, from their defaults.
                            g.kind = other.kind().to_owned();
                            g.params.clear();
                        }
                    }
                });
            ui.end_row();
            label(ui, "Seed");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut g.seed).speed(1.0));
                if ui
                    .button(icon::DICE_FIVE)
                    .on_hover_text("Another seed: a new result from the same settings")
                    .clicked()
                {
                    let time = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.subsec_nanos() as u64);
                    g.seed =
                        (g.seed.wrapping_mul(6364136223846793005).wrapping_add(time)) % 1_000_000;
                }
            });
            ui.end_row();
            for param in generator.params() {
                let key = param.key;
                let current = g
                    .params
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| param.default.to_toml());
                let mut value = current.clone();
                label(ui, param.label);
                let response = param_field(ui, editor, key, param.kind, &mut value);
                response.on_hover_text(param.help);
                ui.end_row();
                if value != current {
                    // Back at the default: not written.
                    if value == param.default.to_toml() {
                        g.params.remove(key);
                    } else {
                        g.params.insert(key.to_owned(), value);
                    }
                }
            }
        });
        let error = editor
            .tree()
            .component::<ab_engine::scene_file::generator::GeneratorNode>(node)
            .and_then(|state| state.error.clone());
        if let Some(error) = error {
            ui.label(RichText::new(format!("{}  {error}", icon::WARNING)).color(theme::WARN));
        }
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Bake", icon::CUBE))
                .on_hover_text(
                    "Turn what it made into ordinary nodes, to change them by hand (the \
                     generator is replaced; Undo brings it back)",
                )
                .clicked()
            {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new(BAKE_REQUEST), Some(node)));
            }
            ui.label(
                RichText::new("Rebuilt on every change; saved as its settings only.")
                    .small()
                    .color(theme::TEXT_DIM),
            );
        });
    });
}

/// An editor for one generator parameter's value.
fn param_field(
    ui: &mut egui::Ui,
    editor: &Editor,
    key: &str,
    kind: ab_engine::procgen::ParamKind,
    value: &mut toml::Value,
) -> egui::Response {
    use ab_engine::procgen::ParamKind;
    let pieces = || {
        editor
            .assets
            .entries
            .iter()
            .filter(|e| matches!(e.kind, AssetKind::Model | AssetKind::Scene))
            .map(|e| e.path.clone())
            .collect::<Vec<_>>()
    };
    match kind {
        ParamKind::Int { min, max } => {
            let mut v = value.as_integer().unwrap_or(min);
            let r = ui.add(egui::DragValue::new(&mut v).range(min..=max).speed(0.2));
            *value = toml::Value::Integer(v);
            r
        }
        ParamKind::Float { min, max } => {
            let mut v = value
                .as_float()
                .or_else(|| value.as_integer().map(|i| i as f64))
                .unwrap_or(f64::from(min)) as f32;
            let speed = ((max - min) / 500.0).clamp(0.001, 1.0);
            let r = ui.add(
                egui::DragValue::new(&mut v)
                    .range(min..=max)
                    .speed(speed)
                    .max_decimals(3),
            );
            *value = toml::Value::Float(f64::from(v));
            r
        }
        ParamKind::Bool => {
            let mut v = value.as_bool().unwrap_or(false);
            let r = ui.checkbox(&mut v, "");
            *value = toml::Value::Boolean(v);
            r
        }
        ParamKind::Text | ParamKind::Node => {
            let mut v = value.as_str().unwrap_or_default().to_owned();
            let hint = if kind == ParamKind::Node {
                "../Road"
            } else {
                ""
            };
            let r = ui.add(
                egui::TextEdit::singleline(&mut v)
                    .hint_text(hint)
                    .desired_width(ui.available_width() - 4.0),
            );
            *value = toml::Value::String(v);
            r
        }
        ParamKind::Color => {
            let text = value.as_str().unwrap_or("#ffffff").to_owned();
            let mut color = ab_engine::scene_file::ColorDesc::Hex(text.clone())
                .to_color()
                .map(super::to_rgba)
                .unwrap_or(egui::Rgba::WHITE);
            let r = egui::color_picker::color_edit_button_rgba(
                ui,
                &mut color,
                egui::color_picker::Alpha::Opaque,
            );
            if r.changed() {
                let c: egui::Color32 = color.into();
                *value = toml::Value::String(format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b()));
            }
            r
        }
        ParamKind::Choice(options) => {
            let mut v = value.as_str().unwrap_or_default().to_owned();
            let r = egui::ComboBox::from_id_salt(("gen_choice", key))
                .selected_text(v.clone())
                .show_ui(ui, |ui| {
                    for option in options {
                        ui.selectable_value(&mut v, (*option).to_owned(), *option);
                    }
                })
                .response;
            *value = toml::Value::String(v);
            r
        }
        ParamKind::Piece => {
            let mut v = value.as_str().unwrap_or_default().to_owned();
            let r = egui::ComboBox::from_id_salt(("gen_piece", key))
                .selected_text(if v.is_empty() {
                    "None (blocks)".to_owned()
                } else {
                    v.clone()
                })
                .width(ui.available_width() - 4.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut v, String::new(), "None (blocks)");
                    for path in pieces() {
                        ui.selectable_value(&mut v, path.clone(), path);
                    }
                })
                .response;
            *value = toml::Value::String(v);
            r
        }
        ParamKind::Pieces => {
            let mut list: Vec<String> = value
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let r = ui
                .vertical(|ui| {
                    let mut remove = None;
                    for (i, item) in list.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(item).small());
                            if ui.small_button(icon::X).clicked() {
                                remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove {
                        list.remove(i);
                    }
                    ui.menu_button(format!("{}  Add piece", icon::PLUS), |ui| {
                        for path in pieces() {
                            if ui.button(&path).clicked() {
                                list.push(path);
                                ui.close();
                            }
                        }
                    });
                })
                .response;
            *value = toml::Value::Array(list.into_iter().map(toml::Value::String).collect());
            r
        }
    }
}

/// A path: what it builds, its settings, and its points.
fn path_section(ui: &mut egui::Ui, editor: &Editor, p: &mut ab_engine::scene_file::PathDesc) {
    use ab_engine::scene_file::path::PathMode;
    section(ui, "path", icon::PATH, "Path", |ui| {
        grid(ui, "path_grid", |ui| {
            label(ui, "Builds");
            let mode = PathMode::of(p).unwrap_or(PathMode::None);
            egui::ComboBox::from_id_salt("path_mode")
                .selected_text(mode.name())
                .show_ui(ui, |ui| {
                    for m in PathMode::ALL {
                        if ui.selectable_label(mode == m, m.name()).clicked() {
                            p.mode = (m != PathMode::None).then(|| m.name().to_owned());
                        }
                    }
                });
            ui.end_row();
            label(ui, "Closed loop");
            opt_bool(ui, &mut p.closed, false);
            ui.end_row();
            match mode {
                PathMode::Ribbon => {
                    label(ui, "Width (m)");
                    opt_f32(ui, &mut p.width, 3.0, 0.01..=200.0, 0.05);
                    ui.end_row();
                }
                PathMode::Tube => {
                    label(ui, "Radius (m)");
                    opt_f32(ui, &mut p.radius, 0.15, 0.001..=50.0, 0.005);
                    ui.end_row();
                    label(ui, "Sides");
                    opt_u32(ui, &mut p.sides, 10, 3..=64);
                    ui.end_row();
                    label(ui, "Sag (m)");
                    opt_f32(ui, &mut p.sag, 0.0, -50.0..=50.0, 0.02);
                    ui.end_row();
                }
                PathMode::Instances => {
                    label(ui, "Piece");
                    let shown = p
                        .piece
                        .clone()
                        .unwrap_or_else(|| crate::i18n::field("None"));
                    egui::ComboBox::from_id_salt("path_piece")
                        .selected_text(shown)
                        .width(ui.available_width() - 4.0)
                        .show_ui(ui, |ui| {
                            for asset in
                                editor.assets.entries.iter().filter(|e| {
                                    matches!(e.kind, AssetKind::Model | AssetKind::Scene)
                                })
                            {
                                if ui
                                    .selectable_label(
                                        p.piece.as_deref() == Some(&asset.path),
                                        &asset.path,
                                    )
                                    .clicked()
                                {
                                    p.piece = Some(asset.path.clone());
                                }
                            }
                        });
                    ui.end_row();
                    label(ui, "Spacing (m)");
                    opt_f32(ui, &mut p.spacing, 2.0, 0.05..=500.0, 0.05);
                    ui.end_row();
                    label(ui, "Face along");
                    opt_bool(ui, &mut p.align, true);
                    ui.end_row();
                }
                PathMode::None => {}
            }
            if matches!(mode, PathMode::Ribbon | PathMode::Tube) {
                label(ui, "Collision");
                opt_bool(ui, &mut p.collide, false);
                ui.end_row();
            }
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new("Points: drag them in the viewport · Ctrl+click adds one · right-click one to delete it")
                .small()
                .color(theme::TEXT_DIM),
        );
        let mut remove = None;
        for (i, point) in p.points.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{i}"))
                        .monospace()
                        .color(theme::TEXT_DIM),
                );
                vec3(ui, point, 0.05, "");
                if ui
                    .small_button(icon::X)
                    .on_hover_text("Remove this point")
                    .clicked()
                {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            p.points.remove(i);
        }
        if ui.button(format!("{}  Add point", icon::PLUS)).clicked() {
            let next = match p.points.as_slice() {
                [] => [0.0; 3],
                [only] => [only[0] + 4.0, only[1], only[2]],
                [.., a, b] => std::array::from_fn(|c| b[c] + (b[c] - a[c])),
            };
            p.points.push(next);
        }
    });
}

fn slider(
    ui: &mut egui::Ui,
    value: &mut Option<f32>,
    default: f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    let mut v = value.unwrap_or(default);
    let changed = ui.add(egui::Slider::new(&mut v, range)).changed();
    if changed {
        *value = Some(v);
    }
    changed
}

fn camera_section(ui: &mut egui::Ui, c: &mut CameraDesc) {
    let defaults = ab_engine::rendering::Camera3D::default();
    section(ui, "camera", icon::VIDEO_CAMERA, "Camera", |ui| {
        grid(ui, "camera_grid", |ui| {
            label(ui, "Field of view");
            slider(ui, &mut c.fov, defaults.fov_y_degrees, 10.0..=150.0);
            ui.end_row();
            label(ui, "Near");
            opt_f32(ui, &mut c.near, defaults.near, 0.001..=100.0, 0.005);
            ui.end_row();
            label(ui, "Far");
            opt_f32(ui, &mut c.far, defaults.far, 1.0..=100_000.0, 1.0);
            ui.end_row();
            label(ui, "Current");
            opt_bool(ui, &mut c.current, false);
            ui.end_row();
        });
    });
}

fn light_section(ui: &mut egui::Ui, class: &str, l: &mut LightDesc) {
    section(ui, "light", icon::LIGHTBULB, "Light", |ui| {
        grid(ui, "light_grid", |ui| {
            label(ui, "Color");
            opt_color(ui, &mut l.color, Color::WHITE, false);
            ui.end_row();
            label(ui, "Energy");
            opt_f32(ui, &mut l.energy, 1.0, 0.0..=1000.0, 0.05);
            ui.end_row();
            if class != "DirectionalLight3D" {
                label(ui, "Range");
                opt_f32(ui, &mut l.range, 5.0, 0.01..=1000.0, 0.05);
                ui.end_row();
            }
            if class == "SpotLight3D" {
                label(ui, "Angle");
                slider(ui, &mut l.angle, 45.0, 1.0..=89.0);
                ui.end_row();
                label(ui, "Softness");
                slider(ui, &mut l.softness, 0.2, 0.0..=1.0);
                ui.end_row();
            }
            label(ui, "Shadows");
            opt_bool(ui, &mut l.shadows, false);
            ui.end_row();
        });
    });
}

fn body_section(ui: &mut egui::Ui, class: &str, b: &mut BodyDesc) {
    section(ui, "body", icon::ATOM, "Physics body", |ui| {
        grid(ui, "body_grid", |ui| {
            if class == "RigidBody3D" {
                label(ui, "Mass (kg)");
                opt_f32(ui, &mut b.mass, 1.0, 0.001..=100_000.0, 0.05);
                ui.end_row();
                label(ui, "Gravity scale");
                opt_f32(ui, &mut b.gravity_scale, 1.0, -10.0..=10.0, 0.05);
                ui.end_row();
                label(ui, "Linear damping");
                opt_f32(ui, &mut b.linear_damping, 0.0, 0.0..=100.0, 0.01);
                ui.end_row();
                label(ui, "Angular damping");
                opt_f32(ui, &mut b.angular_damping, 0.0, 0.0..=100.0, 0.01);
                ui.end_row();
                label(ui, "Lock rotation");
                opt_bool(ui, &mut b.lock_rotation, false);
                ui.end_row();
                label(ui, "Continuous CD");
                opt_bool(ui, &mut b.continuous_cd, false);
                ui.end_row();
                label(ui, "Can sleep");
                opt_bool(ui, &mut b.can_sleep, true);
                ui.end_row();
            }
            if class != "Area3D" {
                label(ui, "Friction");
                slider(ui, &mut b.friction, 0.5, 0.0..=2.0);
                ui.end_row();
                label(ui, "Bounce");
                slider(ui, &mut b.bounce, 0.0, 0.0..=1.0);
                ui.end_row();
            }
            label(ui, "Layer");
            layer_bits(ui, "layer", &mut b.layer, 1);
            ui.end_row();
            label(ui, "Mask");
            layer_bits(ui, "mask", &mut b.mask, u32::MAX);
            ui.end_row();
        });
    });
}

/// Eight toggles for the first collision layers.
fn layer_bits(ui: &mut egui::Ui, id: &str, value: &mut Option<u32>, default: u32) {
    let mut bits = value.unwrap_or(default);
    let before = bits;
    ui.push_id(id, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for i in 0..8 {
                let on = bits & (1 << i) != 0;
                let text = RichText::new(format!("{}", i + 1)).small();
                if ui
                    .add(
                        egui::Button::new(text)
                            .selected(on)
                            .min_size(egui::vec2(18.0, 18.0)),
                    )
                    .clicked()
                {
                    bits ^= 1 << i;
                }
            }
        });
    });
    if bits != before {
        *value = Some(bits);
    }
}

fn character_section(ui: &mut egui::Ui, c: &mut CharacterDesc) {
    section(
        ui,
        "character",
        icon::PERSON_SIMPLE_WALK,
        "Character",
        |ui| {
            grid(ui, "character_grid", |ui| {
                label(ui, "Max floor angle");
                slider(ui, &mut c.floor_max_angle, 45.0, 0.0..=89.0);
                ui.end_row();
                label(ui, "Floor snap");
                opt_f32(ui, &mut c.floor_snap_length, 0.1, 0.0..=2.0, 0.01);
                ui.end_row();
                label(ui, "Max step height");
                opt_f32(ui, &mut c.max_step_height, 0.3, 0.0..=2.0, 0.01);
                ui.end_row();
                label(ui, "Push rigid bodies");
                opt_bool(ui, &mut c.push_rigid_bodies, true);
                ui.end_row();
            });
        },
    );
}

fn shape_section(ui: &mut egui::Ui, shape: &mut ShapeDesc) {
    section(ui, "shape", icon::SHAPES, "Collision shape", |ui| {
        grid(ui, "shape_grid", |ui| {
            label(ui, "Shape");
            let kind = match shape {
                ShapeDesc::Box(_) => "Box",
                ShapeDesc::Sphere(_) => "Sphere",
                ShapeDesc::Capsule(_) => "Capsule",
                ShapeDesc::Cylinder(_) => "Cylinder",
            };
            egui::ComboBox::from_id_salt("shape_kind")
                .selected_text(kind)
                .show_ui(ui, |ui| {
                    for (name, make) in [
                        ("Box", ShapeDesc::Box([1.0; 3])),
                        ("Sphere", ShapeDesc::Sphere(0.5)),
                        ("Capsule", ShapeDesc::Capsule([0.4, 1.8])),
                        ("Cylinder", ShapeDesc::Cylinder([0.5, 1.0])),
                    ] {
                        if ui.selectable_label(kind == name, name).clicked() && kind != name {
                            *shape = make;
                        }
                    }
                });
            ui.end_row();
            let drag = |ui: &mut egui::Ui, v: &mut f32| {
                ui.add(
                    egui::DragValue::new(v)
                        .custom_parser(crate::panels::parse_finite)
                        .range(0.001..=10_000.0)
                        .speed(0.01),
                );
            };
            match shape {
                ShapeDesc::Box(size) => {
                    label(ui, "Size");
                    vec3(ui, size, 0.01, "");
                    ui.end_row();
                }
                ShapeDesc::Sphere(radius) => {
                    label(ui, "Radius");
                    drag(ui, radius);
                    ui.end_row();
                }
                ShapeDesc::Capsule([radius, height]) | ShapeDesc::Cylinder([radius, height]) => {
                    label(ui, "Radius");
                    drag(ui, radius);
                    ui.end_row();
                    label(ui, "Height");
                    drag(ui, height);
                    ui.end_row();
                }
            }
        });
    });
}

fn audio_section(ui: &mut egui::Ui, editor: &Editor, class: &str, a: &mut AudioDesc) {
    use ab_engine::audio::{Attenuation, MASTER, SOUND_EVENT_EXTENSION};
    let is_event = a.stream.ends_with(&format!(".{SOUND_EVENT_EXTENSION}"));
    section(ui, "audio", icon::SPEAKER_HIGH, "Sound", |ui| {
        grid(ui, "audio_grid", |ui| {
            label(ui, "Sound");
            egui::ComboBox::from_id_salt("stream")
                .selected_text(if a.stream.is_empty() {
                    crate::i18n::field("None")
                } else {
                    a.stream.clone()
                })
                .width(ui.available_width() - 4.0)
                .show_ui(ui, |ui| {
                    let events = editor.assets.of_kind(AssetKind::SoundEvent);
                    let files = editor.assets.of_kind(AssetKind::Audio);
                    for asset in events.chain(files) {
                        let text = format!("{}  {}", asset.kind.icon(), asset.path);
                        if ui.selectable_label(a.stream == asset.path, text).clicked() {
                            a.stream = asset.path.clone();
                        }
                    }
                });
            ui.end_row();
            if is_event {
                ui.label("");
                if ui
                    .button(format!(
                        "{}  {}",
                        icon::PENCIL_SIMPLE,
                        crate::i18n::t("audio.edit_event")
                    ))
                    .clicked()
                {
                    crate::app::request_sound_event(ui.ctx(), a.stream.clone());
                }
                ui.end_row();
            }
            label(ui, "Bus");
            let current = a.bus.clone().unwrap_or_default();
            let shown = if current.is_empty() {
                crate::i18n::t(if is_event {
                    "audio.bus_of_event"
                } else {
                    "audio.bus_master"
                })
            } else {
                current.clone()
            };
            egui::ComboBox::from_id_salt("audio_bus")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(
                            current.is_empty(),
                            crate::i18n::t(if is_event {
                                "audio.bus_of_event"
                            } else {
                                "audio.bus_master"
                            }),
                        )
                        .clicked()
                    {
                        a.bus = None;
                    }
                    let returns: Vec<&str> = editor
                        .mixer
                        .returns
                        .iter()
                        .map(|r| r.name.as_str())
                        .collect();
                    for bus in editor
                        .mixer
                        .bus_names()
                        .filter(|b| *b != MASTER && !returns.contains(b))
                    {
                        if ui.selectable_label(current == bus, bus).clicked() {
                            a.bus = Some(bus.to_owned());
                        }
                    }
                });
            ui.end_row();
            label(ui, "Volume (dB)");
            opt_f32(ui, &mut a.volume_db, 0.0, -80.0..=24.0, 0.1);
            ui.end_row();
            label(ui, "Pitch");
            opt_f32(ui, &mut a.pitch, 1.0, 0.01..=4.0, 0.01);
            ui.end_row();
            label(ui, "Looping");
            opt_bool(ui, &mut a.looping, false);
            ui.end_row();
            label(ui, "Autoplay");
            opt_bool(ui, &mut a.autoplay, false);
            ui.end_row();
        });
    });
    if class == "AudioStreamPlayer3D" {
        section(ui, "audio_3d", icon::CUBE, "3D sound", |ui| {
            let base = Attenuation::default();
            let mut att = Attenuation {
                min_distance: a.min_distance.unwrap_or(base.min_distance),
                max_distance: a.max_distance.unwrap_or(base.max_distance),
                rolloff: a.rolloff.unwrap_or(base.rolloff),
                curve: a.curve.clone().unwrap_or_default(),
                spread: a.spread.unwrap_or(base.spread),
                doppler: a.doppler.unwrap_or(base.doppler),
                occlusion: a.occlusion.unwrap_or(base.occlusion),
            };
            if is_event {
                ui.label(
                    RichText::new(crate::i18n::t("audio.event_3d_hint"))
                        .small()
                        .color(theme::TEXT_DIM),
                );
            }
            if crate::panels::sound_event::attenuation_editor(ui, &mut att, "player_3d") {
                a.min_distance = Some(att.min_distance);
                a.max_distance = Some(att.max_distance);
                a.rolloff = Some(att.rolloff);
                a.curve = (!att.curve.is_empty()).then_some(att.curve);
                a.spread = Some(att.spread);
                a.doppler = Some(att.doppler);
                a.occlusion = Some(att.occlusion);
            }
        });
    }
}

fn audio_zone_section(
    ui: &mut egui::Ui,
    editor: &Editor,
    z: &mut ab_engine::scene_file::AudioZoneDesc,
) {
    section(
        ui,
        "audio_zone",
        icon::SPEAKER_SIMPLE_HIGH,
        "Audio zone",
        |ui| {
            grid(ui, "audio_zone_grid", |ui| {
                label(ui, "Snapshot");
                egui::ComboBox::from_id_salt("zone_snapshot")
                    .selected_text(if z.snapshot.is_empty() {
                        crate::i18n::field("None")
                    } else {
                        z.snapshot.clone()
                    })
                    .show_ui(ui, |ui| {
                        for s in &editor.mixer.snapshots {
                            ui.selectable_value(&mut z.snapshot, s.name.clone(), &s.name);
                        }
                    });
                ui.end_row();
                label(ui, "Shape");
                let mut sphere = z.radius.is_some();
                ui.horizontal(|ui| {
                    if ui
                        .radio(!sphere, crate::i18n::t("audio.zone_box"))
                        .clicked()
                    {
                        sphere = false;
                    }
                    if ui
                        .radio(sphere, crate::i18n::t("audio.zone_sphere"))
                        .clicked()
                    {
                        sphere = true;
                    }
                });
                if sphere && z.radius.is_none() {
                    z.radius = Some(5.0);
                    z.size = None;
                } else if !sphere && z.radius.is_some() {
                    z.radius = None;
                    z.size = Some([10.0, 6.0, 10.0]);
                }
                ui.end_row();
                if let Some(radius) = &mut z.radius {
                    label(ui, "Radius");
                    ui.add(
                        egui::DragValue::new(radius)
                            .range(0.01..=10_000.0)
                            .speed(0.05)
                            .suffix(" m"),
                    );
                } else {
                    label(ui, "Size");
                    let mut size = z.size.unwrap_or([10.0, 6.0, 10.0]);
                    if vec3(ui, &mut size, 0.05, " m") {
                        z.size = Some(size.map(|s| s.max(0.01)));
                    }
                }
                ui.end_row();
                label(ui, "Fade distance");
                opt_f32(ui, &mut z.fade_distance, 2.0, 0.0..=1000.0, 0.05);
                ui.end_row();
            });
            if editor.mixer.snapshots.is_empty() {
                ui.label(
                    RichText::new(crate::i18n::t("audio.zone_no_snapshots"))
                        .small()
                        .color(theme::WARN),
                );
            }
        },
    );
}

fn timer_section(ui: &mut egui::Ui, t: &mut TimerDesc) {
    section(ui, "timer", icon::TIMER, "Timer", |ui| {
        grid(ui, "timer_grid", |ui| {
            label(ui, "Wait time (s)");
            ui.add(
                egui::DragValue::new(&mut t.wait_time)
                    .custom_parser(crate::panels::parse_finite)
                    .range(0.0..=3600.0)
                    .speed(0.01),
            );
            ui.end_row();
            label(ui, "One shot");
            opt_bool(ui, &mut t.one_shot, false);
            ui.end_row();
            label(ui, "Autostart");
            opt_bool(ui, &mut t.autostart, false);
            ui.end_row();
        });
    });
}

fn particles_section(ui: &mut egui::Ui, p: &mut ParticlesDesc) {
    let base = p
        .preset
        .as_deref()
        .and_then(ParticleSettings::preset)
        .unwrap_or_default();
    section(ui, "particles", icon::SPARKLE, "Particles", |ui| {
        grid(ui, "particles_grid", |ui| {
            label(ui, "Preset");
            let current = p.preset.clone().unwrap_or_default();
            egui::ComboBox::from_id_salt("preset")
                .selected_text(if current.is_empty() {
                    "Custom"
                } else {
                    &current
                })
                .show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_empty(), "Custom").clicked() {
                        p.preset = None;
                    }
                    for preset in ParticleSettings::PRESETS {
                        if ui.selectable_label(current == preset, preset).clicked() {
                            // A new look: start from the preset's own values.
                            *p = ParticlesDesc {
                                preset: Some(preset.to_owned()),
                                ..Default::default()
                            };
                        }
                    }
                });
            ui.end_row();
            label(ui, "Amount");
            opt_u32(
                ui,
                &mut p.amount,
                base.amount,
                1..=ab_engine::effects::MAX_PARTICLES,
            );
            ui.end_row();
            label(ui, "Lifetime (s)");
            opt_f32(ui, &mut p.lifetime, base.lifetime, 0.01..=60.0, 0.01);
            ui.end_row();
            label(ui, "Size start → end");
            pair(ui, &mut p.size, base.size, 0.005);
            ui.end_row();
            label(ui, "Speed min → max");
            pair(ui, &mut p.speed, base.speed, 0.02);
            ui.end_row();
            label(ui, "Color start");
            let start = base.color.sample(0.0);
            opt_color(ui, &mut p.color_start, start, true);
            ui.end_row();
            label(ui, "Color end");
            let end = base.color.sample(1.0);
            opt_color(ui, &mut p.color_end, end, true);
            ui.end_row();
            label(ui, "Glow");
            opt_f32(ui, &mut p.emission, base.emission, 0.0..=50.0, 0.05);
            ui.end_row();
            label(ui, "Additive");
            opt_bool(ui, &mut p.additive, base.additive);
            ui.end_row();
            label(ui, "One shot");
            opt_bool(ui, &mut p.one_shot, false);
            ui.end_row();
            label(ui, "Local space");
            opt_bool(ui, &mut p.local, false);
            ui.end_row();
            label(ui, "Emitting");
            opt_bool(ui, &mut p.emitting, true);
            ui.end_row();
        });
        if p.color_start.is_some() || p.color_end.is_some() {
            ui.label(
                RichText::new("Custom colors replace the preset's color ramp.")
                    .small()
                    .color(theme::TEXT_DIM),
            );
        }
    });
}

fn pair(ui: &mut egui::Ui, value: &mut Option<[f32; 2]>, default: [f32; 2], speed: f64) {
    let mut v = value.unwrap_or(default);
    let mut changed = false;
    ui.horizontal(|ui| {
        changed |= ui
            .add(
                egui::DragValue::new(&mut v[0])
                    .custom_parser(crate::panels::parse_finite)
                    .range(0.0..=1000.0)
                    .speed(speed)
                    .max_decimals(3),
            )
            .changed();
        ui.label(RichText::new(icon::ARROW_RIGHT).color(theme::TEXT_DIM));
        changed |= ui
            .add(
                egui::DragValue::new(&mut v[1])
                    .custom_parser(crate::panels::parse_finite)
                    .range(0.0..=1000.0)
                    .speed(speed)
                    .max_decimals(3),
            )
            .changed();
    });
    if changed {
        *value = Some(v);
    }
}

/// The `[environment]` of the scene.
pub fn environment(ui: &mut egui::Ui, editor: &mut Editor) {
    let defaults = ab_engine::rendering::Environment::default();
    let mut desc: EnvironmentDesc = editor.environment.clone();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            section(ui, "sky", icon::CLOUD_SUN, "Background & ambient", |ui| {
                grid(ui, "sky_grid", |ui| {
                    label(ui, "Background");
                    opt_color(ui, &mut desc.background, defaults.background, false);
                    ui.end_row();
                    label(ui, "Ambient color");
                    opt_color(ui, &mut desc.ambient_color, defaults.ambient_color, false);
                    ui.end_row();
                    label(ui, "Ambient energy");
                    opt_f32(
                        ui,
                        &mut desc.ambient_energy,
                        defaults.ambient_energy,
                        0.0..=16.0,
                        0.01,
                    );
                    ui.end_row();
                    label(ui, "Exposure");
                    opt_f32(
                        ui,
                        &mut desc.exposure,
                        defaults.post.exposure,
                        0.0..=16.0,
                        0.01,
                    );
                    ui.end_row();
                });
            });
            section(ui, "fog", icon::CLOUD_FOG, "Fog", |ui| {
                grid(ui, "fog_grid", |ui| {
                    label(ui, "Density");
                    let mut density = desc.fog_density.unwrap_or(if defaults.fog.enabled {
                        defaults.fog.density
                    } else {
                        0.0
                    });
                    if ui
                        .add(egui::Slider::new(&mut density, 0.0..=0.5).logarithmic(true))
                        .changed()
                    {
                        desc.fog_density = Some(density);
                    }
                    ui.end_row();
                    label(ui, "Color");
                    opt_color(ui, &mut desc.fog_color, defaults.fog.color, false);
                    ui.end_row();
                    label(ui, "Height");
                    opt_f32(
                        ui,
                        &mut desc.fog_height,
                        defaults.fog.height,
                        -1000.0..=1000.0,
                        0.05,
                    );
                    ui.end_row();
                    label(ui, "Height falloff");
                    opt_f32(
                        ui,
                        &mut desc.fog_height_falloff,
                        defaults.fog.height_falloff,
                        0.0..=10.0,
                        0.01,
                    );
                    ui.end_row();
                    label(ui, "Max opacity");
                    opt_f32(
                        ui,
                        &mut desc.fog_max_opacity,
                        defaults.fog.max_opacity,
                        0.0..=1.0,
                        0.005,
                    );
                    ui.end_row();
                    label(ui, "Sun glow");
                    opt_f32(
                        ui,
                        &mut desc.fog_sun_scattering,
                        defaults.fog.sun_scattering,
                        0.0..=4.0,
                        0.01,
                    );
                    ui.end_row();
                    label(ui, "Sun halo");
                    opt_f32(
                        ui,
                        &mut desc.fog_sun_anisotropy,
                        defaults.fog.sun_scattering_anisotropy,
                        0.0..=0.95,
                        0.005,
                    );
                    ui.end_row();
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Patches blown by the wind")
                        .small()
                        .color(theme::TEXT_DIM),
                );
                grid(ui, "fog_wind_grid", |ui| {
                    label(ui, "Patchiness");
                    let mut patchiness = desc.fog_patchiness.unwrap_or(defaults.fog.patchiness);
                    if ui
                        .add(egui::Slider::new(&mut patchiness, 0.0..=1.0))
                        .on_hover_text("0: even fog · 1: clear gaps between thick banks")
                        .changed()
                    {
                        desc.fog_patchiness = Some(patchiness);
                    }
                    ui.end_row();
                    label(ui, "Patch size");
                    opt_f32(
                        ui,
                        &mut desc.fog_patch_size,
                        defaults.fog.patch_size,
                        0.5..=500.0,
                        0.1,
                    );
                    ui.end_row();
                    label(ui, "Wind (m/s)");
                    let mut wind = desc.fog_wind.unwrap_or(defaults.fog.wind.into());
                    if super::vec3(ui, &mut wind, 0.05, "") {
                        desc.fog_wind = Some(wind);
                    }
                    ui.end_row();
                });
            });
            section(ui, "light_shafts", icon::SUN_DIM, "Light shafts", |ui| {
                let shafts = defaults.light_shafts;
                grid(ui, "shafts_grid", |ui| {
                    label(ui, "Intensity");
                    let mut intensity = desc.light_shafts.unwrap_or(if shafts.enabled {
                        shafts.intensity
                    } else {
                        0.0
                    });
                    if ui
                        .add(egui::Slider::new(&mut intensity, 0.0..=8.0))
                        .on_hover_text("0 turns them off")
                        .changed()
                    {
                        desc.light_shafts = Some(intensity);
                    }
                    ui.end_row();
                    label(ui, "Air density");
                    opt_f32(ui, &mut desc.light_shafts_density, shafts.density, 0.0..=2.0, 0.001);
                    ui.end_row();
                    label(ui, "Towards the light");
                    opt_f32(ui, &mut desc.light_shafts_anisotropy, shafts.anisotropy, 0.0..=0.95, 0.005);
                    ui.end_row();
                    label(ui, "Distance (m)");
                    opt_f32(ui, &mut desc.light_shafts_distance, shafts.max_distance, 1.0..=500.0, 0.5);
                    ui.end_row();
                });
                ui.label(
                    RichText::new("Beams where the sun and spot lights shine through openings: windows, gaps in roofs and trees, a flashlight in dusty air. Lights need shadows on for the beams to be cut out.")
                        .small()
                        .color(theme::TEXT_DIM),
                );
            });
            section(ui, "post", icon::APERTURE, "Post-processing", |ui| {
                let post = defaults.post;
                grid(ui, "post_grid", |ui| {
                    let rows: [(&str, &mut Option<f32>, f32, f32, f64); 12] = [
                        ("Bloom", &mut desc.bloom, post.bloom.intensity, 2.0, 0.005),
                        (
                            "Bloom threshold",
                            &mut desc.bloom_threshold,
                            post.bloom.threshold,
                            16.0,
                            0.01,
                        ),
                        ("Contrast", &mut desc.contrast, post.contrast, 4.0, 0.01),
                        (
                            "Saturation",
                            &mut desc.saturation,
                            post.saturation,
                            4.0,
                            0.01,
                        ),
                        (
                            "Vignette",
                            &mut desc.vignette,
                            post.vignette_intensity,
                            1.0,
                            0.01,
                        ),
                        (
                            "Vignette smoothness",
                            &mut desc.vignette_smoothness,
                            post.vignette_smoothness,
                            1.0,
                            0.01,
                        ),
                        (
                            "Chromatic aberration",
                            &mut desc.chromatic_aberration,
                            post.chromatic_aberration,
                            4.0,
                            0.01,
                        ),
                        ("Film grain", &mut desc.grain, post.grain, 4.0, 0.01),
                        (
                            "Depth of field (px)",
                            &mut desc.depth_of_field,
                            post.depth_of_field.max_blur,
                            32.0,
                            0.1,
                        ),
                        (
                            "Focus distance (m)",
                            &mut desc.focus_distance,
                            post.depth_of_field.focus_distance,
                            500.0,
                            0.05,
                        ),
                        (
                            "Focus range (m)",
                            &mut desc.focus_range,
                            post.depth_of_field.focus_range,
                            200.0,
                            0.05,
                        ),
                        ("Motion blur", &mut desc.motion_blur, post.motion_blur, 4.0, 0.01),
                    ];
                    label(ui, "Occlusion culling");
                    opt_bool(ui, &mut desc.occlusion_culling, false);
                    ui.end_row();
                    for (name, value, default, max, speed) in rows {
                        label(ui, name);
                        opt_f32(ui, value, default, 0.0..=max, speed);
                        ui.end_row();
                    }
                    label(ui, "Tint");
                    opt_color(ui, &mut desc.tint, post.tint, false);
                    ui.end_row();
                });
            });
            ui.add_space(6.0);
            if ui
                .button(format!(
                    "{}  Reset to defaults",
                    icon::ARROW_COUNTER_CLOCKWISE
                ))
                .clicked()
            {
                desc = EnvironmentDesc::default();
            }
        });
    editor.set_environment(desc);
}

// =============================================================================================
// UI
// =============================================================================================

fn ui_kind(editor: &Editor, node: NodeHandle) -> bool {
    let tree = editor.tree();
    tree.component::<ab_engine::ui::Control>(node).is_some()
        || tree.class_name(node) == Some(ab_engine::ui::class::CANVAS_LAYER)
}

/// A combo box over an optional name, showing `default` while unset.
fn choice(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut Option<String>,
    default: &str,
    options: &[(&str, &str)],
) {
    let current = value.as_deref().unwrap_or(default).to_owned();
    let current = current.as_str();
    let shown = options
        .iter()
        .find(|(name, _)| *name == current)
        .map_or(current, |(_, label)| *label);
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for (name, text) in options {
                if ui.selectable_label(current == *name, *text).clicked() {
                    *value = (*name != default).then(|| (*name).to_owned());
                }
            }
        });
}

/// A label's or button's translation key, with its text in the game's language.
fn key_field(ui: &mut egui::Ui, editor: &Editor, key: &mut Option<String>) {
    label(ui, "Translation key");
    let mut text = key.clone().unwrap_or_default();
    let edit = ui
        .add(
            egui::TextEdit::singleline(&mut text)
                .hint_text("hud.score")
                .desired_width(ui.available_width() - 4.0),
        )
        .on_hover_text(
            "Shows this key's text in the game's language (locales/*.toml, listed under \
             [localization] in game.toml). Text is what shows without a translation.",
        );
    if edit.changed() {
        let trimmed = text.trim();
        *key = (!trimmed.is_empty()).then(|| trimmed.to_owned());
    }
    ui.end_row();
    if let Some(key) = key.as_deref() {
        let localization = editor
            .tree()
            .world()
            .resource::<ab_engine::localization::Localization>()
            .map(|l| {
                (
                    l.language().to_owned(),
                    l.catalog.get(key).map(str::to_owned),
                )
            });
        let (note, color) = match localization {
            Some((language, Some(text))) => (format!("{language}: {text}"), theme::TEXT_DIM),
            Some((language, None)) => (format!("No text for this key ({language})"), theme::WARN),
            None => ("The project has no languages".to_owned(), theme::TEXT_DIM),
        };
        label(ui, "");
        ui.label(RichText::new(note).small().color(color));
        ui.end_row();
    }
}

fn text_field(ui: &mut egui::Ui, text: &mut String) {
    ui.add(
        egui::TextEdit::multiline(text)
            .desired_rows(1)
            .desired_width(ui.available_width() - 4.0),
    );
}

fn edges(
    ui: &mut egui::Ui,
    values: &mut [f32; 4],
    range: std::ops::RangeInclusive<f32>,
    speed: f64,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (v, hint) in values.iter_mut().zip(["Left", "Top", "Right", "Bottom"]) {
            changed |= ui
                .add(
                    egui::DragValue::new(v)
                        .custom_parser(crate::panels::parse_finite)
                        .range(range.clone())
                        .speed(speed)
                        .max_decimals(3),
                )
                .on_hover_text(hint)
                .changed();
        }
    });
    changed
}

/// The 16 layout presets as a 4 × 4 grid of little pictures of where the control goes.
fn preset_picker(ui: &mut egui::Ui) -> Option<ab_engine::ui::LayoutPreset> {
    use ab_engine::scene_file::ui::PRESETS;
    let mut picked = None;
    egui::Grid::new("ui_presets")
        .spacing([3.0, 3.0])
        .show(ui, |ui| {
            for (i, (name, preset)) in PRESETS.iter().enumerate() {
                let size = egui::vec2(26.0, 18.0);
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
                let painter = ui.painter_at(rect);
                let hovered = response.hovered();
                painter.rect_filled(rect, 2.0, if hovered { theme::BG_2 } else { theme::BG_1 });
                painter.rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(1.0, theme::TEXT_DIM.gamma_multiply(0.5)),
                    egui::StrokeKind::Inside,
                );
                // Where the control would sit, small, in the parent box.
                let c = ab_engine::ui::Control::new().with_preset(
                    *preset,
                    ab_engine::core::Vec2::new(0.3, 0.3),
                    0.08,
                );
                let at = |fx: f32, fy: f32, ox: f32, oy: f32| {
                    let inner = rect.shrink(2.0);
                    egui::pos2(
                        inner.left() + (fx + ox) * inner.width(),
                        inner.top() + (fy + oy) * inner.height(),
                    )
                };
                let min = at(c.anchors[0], c.anchors[1], c.offsets[0], c.offsets[1]);
                let max = at(c.anchors[2], c.anchors[3], c.offsets[2], c.offsets[3]);
                painter.rect_filled(
                    egui::Rect::from_min_max(min, max),
                    1.0,
                    if hovered {
                        theme::SELECTION
                    } else {
                        theme::ACCENT
                    },
                );
                if response.on_hover_text(name.replace('_', " ")).clicked() {
                    picked = Some(*preset);
                }
                if i % 4 == 3 {
                    ui.end_row();
                }
            }
        });
    picked
}

fn ui_sections(
    ui: &mut egui::Ui,
    editor: &Editor,
    node: NodeHandle,
    class: &str,
    desc: &mut NodeDesc,
) {
    use ab_engine::scene_file::{
        BoxDesc, ButtonDesc, ImageDesc, LabelDesc, PanelDesc, ProgressDesc,
    };
    use ab_engine::ui::class as ui_class;
    let tree = editor.tree();
    let in_box = tree.parent(node).is_some_and(|p| {
        matches!(
            tree.class_name(p),
            Some(ui_class::VBOX_CONTAINER | ui_class::HBOX_CONTAINER)
        )
    });
    let current = tree
        .component::<ab_engine::ui::Control>(node)
        .map(|c| c.rect());
    let c = desc.control.get_or_insert_with(Default::default);
    // Presets and sizes become anchors and offsets: what files store once saved.
    let fresh = ab_engine::scene_file::ui::control(Some(c)).unwrap_or_default();
    c.preset = None;
    c.size = None;
    c.margin = None;
    let mut anchors = fresh.anchors;
    let mut offsets = fresh.offsets;
    section(ui, "layout", icon::LAYOUT, "Layout", |ui| {
        if in_box {
            ui.label(
                RichText::new(
                    "The container places this control: set its minimum size and expand instead.",
                )
                .color(theme::TEXT_DIM),
            );
        } else if let Some(preset) = preset_picker(ui) {
            let size = current.map_or(ab_engine::core::Vec2::new(200.0, 100.0), |r| r.size);
            let placed = ab_engine::ui::Control::new().with_preset(preset, size, 0.0);
            anchors = placed.anchors;
            offsets = placed.offsets;
        }
        grid(ui, "layout_grid", |ui| {
            if !in_box {
                label(ui, "Anchors");
                edges(ui, &mut anchors, 0.0..=1.0, 0.005);
                ui.end_row();
                label(ui, "Offsets");
                edges(ui, &mut offsets, -100_000.0..=100_000.0, 1.0);
                ui.end_row();
                // Position and size, for controls that are not stretched along an axis.
                label(ui, "Position");
                ui.horizontal(|ui| {
                    for axis in 0..2 {
                        let mut v = offsets[axis];
                        if ui
                            .add(
                                egui::DragValue::new(&mut v)
                                    .custom_parser(crate::panels::parse_finite)
                                    .speed(1.0)
                                    .max_decimals(1),
                            )
                            .changed()
                        {
                            offsets[axis + 2] += v - offsets[axis];
                            offsets[axis] = v;
                        }
                    }
                });
                ui.end_row();
                label(ui, "Size");
                ui.horizontal(|ui| {
                    for axis in 0..2 {
                        // Stretched along this axis: the anchors and offsets size it.
                        if anchors[axis] != anchors[axis + 2] {
                            ui.label(RichText::new("stretched").color(theme::TEXT_DIM));
                            continue;
                        }
                        let mut v = offsets[axis + 2] - offsets[axis];
                        let response = ui.add(
                            // No range: it would clamp (and so change) a flipped rect.
                            egui::DragValue::new(&mut v)
                                .custom_parser(crate::panels::parse_finite)
                                .speed(1.0)
                                .max_decimals(1),
                        );
                        if response.changed() {
                            offsets[axis + 2] = offsets[axis] + v.max(0.0);
                        }
                    }
                });
                ui.end_row();
            }
            label(ui, "Minimum size");
            pair(ui, &mut c.min_size, [0.0, 0.0], 1.0);
            ui.end_row();
            if in_box {
                label(ui, "Expand");
                opt_bool(ui, &mut c.expand, false);
                ui.end_row();
            }
            label(ui, "Takes the mouse");
            let passive = ab_engine::scene_file::ui::passive_by_default(class);
            let mut stops = c.mouse.as_deref().map_or(!passive, |m| m == "stop");
            if ui
                .checkbox(&mut stops, "")
                .on_hover_text("Off: clicks go through to what is behind")
                .changed()
            {
                c.mouse =
                    (stops == passive).then(|| if stops { "stop" } else { "ignore" }.to_owned());
            }
            ui.end_row();
            label(ui, "Modulate");
            opt_color(ui, &mut c.modulate, Color::WHITE, true);
            ui.end_row();
            label(ui, "Clip children");
            opt_bool(ui, &mut c.clip, false);
            ui.end_row();
        });
    });
    c.anchors = (anchors != [0.0; 4]).then_some(anchors);
    c.offsets = (offsets != [0.0; 4]).then_some(offsets);
    if c.expand == Some(false) {
        c.expand = None;
    }
    if c.clip == Some(false) {
        c.clip = None;
    }
    if desc.control == Some(Default::default()) {
        desc.control = None;
    }

    let colors = |ui: &mut egui::Ui,
                  name: &str,
                  value: &mut Option<ab_engine::scene_file::ColorDesc>,
                  default: Color| {
        label(ui, name);
        opt_color(ui, value, default, true);
        ui.end_row();
    };
    match class {
        ui_class::LABEL => {
            let l = desc.label.get_or_insert_with(LabelDesc::default);
            section(ui, "label", icon::TEXT_T, "Label", |ui| {
                grid(ui, "label_grid", |ui| {
                    label(ui, "Text");
                    text_field(ui, &mut l.text);
                    ui.end_row();
                    key_field(ui, editor, &mut l.key);
                    label(ui, "Font size");
                    opt_f32(ui, &mut l.size, 24.0, 4.0..=400.0, 0.2);
                    ui.end_row();
                    colors(ui, "Color", &mut l.color, Color::WHITE);
                    label(ui, "Align");
                    choice(
                        ui,
                        "label_align",
                        &mut l.align,
                        "left",
                        &[("left", "Left"), ("center", "Center"), ("right", "Right")],
                    );
                    ui.end_row();
                    label(ui, "Vertical align");
                    choice(
                        ui,
                        "label_valign",
                        &mut l.valign,
                        "top",
                        &[("top", "Top"), ("center", "Center"), ("bottom", "Bottom")],
                    );
                    ui.end_row();
                    label(ui, "Outline");
                    opt_f32(ui, &mut l.outline, 0.0, 0.0..=32.0, 0.05);
                    ui.end_row();
                    if l.outline.is_some_and(|o| o > 0.0) {
                        colors(ui, "Outline color", &mut l.outline_color, Color::BLACK);
                    }
                    label(ui, "Wrap lines");
                    opt_bool(ui, &mut l.autowrap, false);
                    ui.end_row();
                });
            });
        }
        ui_class::BUTTON => {
            let b = desc.button.get_or_insert_with(ButtonDesc::default);
            let default = ab_engine::ui::Button::new("");
            section(ui, "button", icon::CURSOR_CLICK, "Button", |ui| {
                grid(ui, "button_grid", |ui| {
                    label(ui, "Text");
                    text_field(ui, &mut b.text);
                    ui.end_row();
                    key_field(ui, editor, &mut b.key);
                    label(ui, "Font size");
                    opt_f32(ui, &mut b.size, default.text_style.size, 4.0..=400.0, 0.2);
                    ui.end_row();
                    colors(ui, "Color", &mut b.color, default.normal.color);
                    colors(ui, "Text color", &mut b.text_color, Color::WHITE);
                    label(ui, "Disabled");
                    opt_bool(ui, &mut b.disabled, false);
                    ui.end_row();
                });
                ui.label(
                    RichText::new("Scripts hear it through its `pressed` signal.")
                        .color(theme::TEXT_DIM),
                );
            });
        }
        ui_class::PANEL => {
            let p = desc.panel.get_or_insert_with(PanelDesc::default);
            let default = ab_engine::ui::StyleBox::default();
            section(ui, "panel", icon::RECTANGLE, "Panel", |ui| {
                grid(ui, "panel_grid", |ui| {
                    colors(ui, "Color", &mut p.color, default.color);
                    label(ui, "Corner radius");
                    opt_f32(
                        ui,
                        &mut p.corner_radius,
                        default.corner_radius,
                        0.0..=500.0,
                        0.2,
                    );
                    ui.end_row();
                    label(ui, "Border");
                    opt_f32(
                        ui,
                        &mut p.border_width,
                        default.border_width,
                        0.0..=64.0,
                        0.1,
                    );
                    ui.end_row();
                    if p.border_width.is_some_and(|w| w > 0.0) {
                        colors(ui, "Border color", &mut p.border_color, Color::WHITE);
                    }
                });
            });
        }
        ui_class::PROGRESS_BAR => {
            let p = desc.progress.get_or_insert_with(ProgressDesc::default);
            let default = ab_engine::ui::ProgressBar::default();
            section(ui, "progress", icon::BATTERY_HIGH, "Progress bar", |ui| {
                grid(ui, "progress_grid", |ui| {
                    label(ui, "Min");
                    opt_f32(ui, &mut p.min, default.min, -1e6..=1e6, 0.1);
                    ui.end_row();
                    label(ui, "Max");
                    opt_f32(ui, &mut p.max, default.max, -1e6..=1e6, 0.1);
                    ui.end_row();
                    let (min, max) = (p.min.unwrap_or(default.min), p.max.unwrap_or(default.max));
                    label(ui, "Value");
                    slider(ui, &mut p.value, default.value, min.min(max)..=max.max(min));
                    ui.end_row();
                    colors(ui, "Fill", &mut p.fill_color, default.fill.color);
                    colors(
                        ui,
                        "Background",
                        &mut p.background_color,
                        default.background.color,
                    );
                    label(ui, "Show percentage");
                    opt_bool(ui, &mut p.show_percentage, false);
                    ui.end_row();
                });
            });
        }
        ui_class::VBOX_CONTAINER | ui_class::HBOX_CONTAINER => {
            let b = desc.box_container.get_or_insert_with(BoxDesc::default);
            section(ui, "box", icon::ROWS, "Container", |ui| {
                grid(ui, "box_grid", |ui| {
                    label(ui, "Separation");
                    opt_f32(ui, &mut b.separation, 6.0, 0.0..=500.0, 0.2);
                    ui.end_row();
                    label(ui, "Alignment");
                    choice(
                        ui,
                        "box_align",
                        &mut b.alignment,
                        "begin",
                        &[("begin", "Begin"), ("center", "Center"), ("end", "End")],
                    );
                    ui.end_row();
                });
            });
        }
        ui_class::TEXTURE_RECT => {
            let image = desc.image.get_or_insert_with(ImageDesc::default);
            section(ui, "image", icon::IMAGE, "Image", |ui| {
                grid(ui, "image_grid", |ui| {
                    label(ui, "Texture");
                    egui::ComboBox::from_id_salt("image_texture")
                        .selected_text(image.texture.as_str())
                        .width(ui.available_width() - 4.0)
                        .show_ui(ui, |ui| {
                            for asset in editor.assets.of_kind(AssetKind::Image) {
                                if ui
                                    .selectable_label(image.texture == asset.path, &asset.path)
                                    .clicked()
                                {
                                    image.texture = asset.path.clone();
                                }
                            }
                        });
                    ui.end_row();
                    label(ui, "Stretch");
                    choice(
                        ui,
                        "image_stretch",
                        &mut image.stretch,
                        "scale",
                        &[
                            ("scale", "Fill"),
                            ("keep_aspect", "Keep aspect"),
                            ("cover", "Cover"),
                        ],
                    );
                    ui.end_row();
                    colors(ui, "Tint", &mut image.tint, Color::WHITE);
                });
            });
        }
        _ => {}
    }
    if desc.panel == Some(PanelDesc::default()) {
        desc.panel = None;
    }
    if desc.progress == Some(ProgressDesc::default()) {
        desc.progress = None;
    }
    if desc.box_container == Some(BoxDesc::default()) {
        desc.box_container = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Showing a node in the Inspector must not change it.
    #[test]
    fn looking_at_a_node_changes_nothing() {
        let engine = crate::projects::engine_root().unwrap();
        for (assets, scene) in [
            ("crates/ab-project/template/assets", "scenes/main.abscene"),
            ("assets", "slice/level.abscene"),
            ("assets", "slice/main.abscene"),
        ] {
            check_scene(&engine.join(assets), scene);
        }
    }

    /// Every UI control and screen of the Create menu.
    #[test]
    fn looking_at_ui_nodes_changes_nothing() {
        let assets = crate::projects::engine_root().unwrap().join("assets");
        let mut editor = Editor::new(assets, None, ab_engine::audio::AudioOutput::Silent);
        for template in crate::create::UI.iter().chain(crate::create::UI_SCREENS) {
            let scene = crate::create::place_ui(&mut editor, template.scene());
            editor.add_scene(scene, None);
        }
        let scene = crate::create::image(&editor).unwrap();
        let scene = crate::create::place_ui(&mut editor, scene);
        editor.add_scene(scene, None);
        editor.modified = false;
        check_editor(editor, "UI");
    }

    fn check_scene(assets: &std::path::Path, scene: &str) {
        let mut editor = Editor::new(
            assets.to_path_buf(),
            None,
            ab_engine::audio::AudioOutput::Silent,
        );
        editor.open_scene(scene);
        assert!(editor.scene_path.is_some(), "{scene} opened");
        check_editor(editor, scene);
    }

    fn check_editor(mut editor: Editor, scene: &str) {
        let ctx = egui::Context::default();
        let mut panel = InspectorPanel::default();
        let nodes = editor.tree().subtree(editor.scene_root);
        for node in nodes {
            if !editor.is_editable(node) {
                continue;
            }
            let before = editor.describe_node(node);
            let path = editor.relative_path(node);
            editor.select(Some(node));
            for _ in 0..3 {
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    egui::CentralPanel::default().show(ui, |ui| panel.show(ui, &mut editor));
                });
                // Nothing renders here: drop the font atlas uploads.
                output.textures_delta.clear();
            }
            let after = path
                .as_deref()
                .and_then(|p| editor.node_at(p))
                .and_then(|n| editor.describe_node(n));
            let lines = |d: &Option<NodeDesc>| -> Vec<String> {
                format!("{d:#?}").lines().map(str::to_owned).collect()
            };
            let (b, a) = (lines(&before), lines(&after));
            let changed: Vec<String> = a.iter().filter(|l| !b.contains(l)).cloned().collect();
            assert!(
                !editor.modified,
                "{scene}: showing {path:?} changed: {changed:#?}"
            );
        }
    }
}
