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
use ab_engine::scene::NodeHandle;
use ab_engine::scene_file::{
    relative_path, AudioDesc, BodyDesc, CameraDesc, CharacterDesc, EnvironmentDesc, JointDesc,
    LightDesc, MaterialDesc, MeshDesc, NodeDesc, ParticlesDesc, RagdollDesc, ShapeDesc, TimerDesc,
};
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;

#[derive(Default)]
pub struct InspectorPanel {
    /// Euler angles shown for a rotation, so dragging one does not flip the others.
    euler: Option<(NodeHandle, Quat, [f32; 3])>,
    groups_text: Option<(NodeHandle, String)>,
    /// The Script field's open button was pressed: (file, line).
    open_script: Option<(std::path::PathBuf, usize)>,
}

impl InspectorPanel {
    pub fn show(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        let Some(node) = editor.selection.filter(|&n| editor.tree().is_valid(n)) else {
            super::empty_hint(
                ui,
                icon::CURSOR_CLICK,
                "Select a node to see its properties",
            );
            return;
        };
        let tree = editor.tree();
        let class = tree.class_name(node).unwrap_or("Node").to_owned();
        let Some(old) = editor.describe_node(node) else {
            return;
        };
        let editable = editor.is_editable(node);
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
                ui.add_enabled(
                    editable && node != editor.scene_root,
                    egui::TextEdit::singleline(&mut desc.name)
                        .font(egui::TextStyle::Heading)
                        .desired_width(f32::INFINITY),
                );
                let mut subtitle = class.clone();
                if let Some(instance) = &old.instance {
                    subtitle = format!("Instance of {instance}");
                }
                ui.label(RichText::new(subtitle).small().color(theme::TEXT_DIM));
            });
        });
        if !editable {
            ui.label(
                RichText::new(format!(
                    "{}  Part of an instanced model: edit the instance instead.",
                    icon::INFO
                ))
                .color(theme::WARN),
            );
            return;
        }
        ui.add_space(4.0);
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if is_3d {
                    self.transform(ui, editor, node);
                }
                self.node_section(ui, editor, node, &mut desc);
                class_sections(ui, editor, node, &class, &mut desc);
                if old
                    .instance
                    .as_deref()
                    .is_some_and(|p| p.ends_with(".toml"))
                {
                    ui.add_space(6.0);
                    if ui
                        .button(format!("{}  Open instanced scene", icon::ARROW_SQUARE_OUT))
                        .clicked()
                    {
                        let path = old.instance.clone().unwrap_or_default();
                        crate::app::request_open(ui.ctx(), path);
                    }
                }
            });

        if desc != old {
            editor.apply_desc(node, &old, desc);
        }
        if let Some((path, line)) = self.open_script.take() {
            crate::panels::assets::open_script(editor, &path, line);
        }
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
                    if ui.small_button("Reset position").clicked() {
                        position = [0.0; 3];
                        changed = true;
                    }
                    if ui.small_button("Reset rotation").clicked() {
                        rotation = [0.0; 3];
                        changed = true;
                    }
                    if ui.small_button("Reset scale").clicked() {
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
                if editor.tree().is_node3d(node) {
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
                            RichText::new(format!("{}  Add script (or drop one here)", icon::PLUS))
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
                        .hint_text("enemies, pickups")
                        .desired_width(f32::INFINITY),
                );
                if response.lost_focus() || (!response.has_focus() && *text != joined) {
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
            material_section(ui, material);
            section(ui, "shadows", icon::SUN_HORIZON, "Shadows", |ui| {
                grid(ui, "shadows_grid", |ui| {
                    label(ui, "Cast shadows");
                    opt_bool(ui, &mut desc.cast_shadows, true);
                    ui.end_row();
                });
            });
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
        "CollisionShape3D" => {
            shape_section(ui, desc.shape.get_or_insert(ShapeDesc::Box([1.0; 3])));
        }
        "AudioStreamPlayer" | "AudioStreamPlayer3D" => {
            audio_section(
                ui,
                editor,
                class,
                desc.audio.get_or_insert_with(Default::default),
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
                        "None"
                    } else {
                        path.as_str()
                    })
                    .width(ui.available_width() - 4.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(path.is_empty(), "None").clicked() {
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
                                    .range(-180.0..=180.0)
                                    .suffix("°"),
                            )
                            .changed();
                        ui.label(RichText::new(icon::ARROW_RIGHT).color(theme::TEXT_DIM));
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut limits[1])
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
    let parts = editor.tree().ragdoll_parts(node).len();
    section(ui, "ragdoll", icon::PERSON_ARMS_SPREAD, "Ragdoll", |ui| {
        grid(ui, "ragdoll_grid", |ui| {
            label(ui, "Limp on start");
            opt_bool(ui, &mut r.active, false);
            ui.end_row();
            label(ui, "Parts");
            ui.label(format!("{parts} bodies"));
            ui.end_row();
        });
        ui.label(
            RichText::new("Off: parts stay kinematic (follow their bones, act as hit boxes) until a script calls ragdoll::go_limp.")
                .small()
                .color(theme::TEXT_DIM),
        );
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
                    ui.add(egui::DragValue::new(size).range(0.01..=1000.0).speed(0.01));
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
                            .range(0.01..=1000.0)
                            .speed(0.01),
                    );
                    ui.end_row();
                    label(ui, "Segments");
                    ui.add(egui::DragValue::new(segments).range(3..=128));
                    ui.end_row();
                    label(ui, "Rings");
                    ui.add(egui::DragValue::new(rings).range(2..=64));
                    ui.end_row();
                }
                MeshDesc::Plane { size, subdivisions } => {
                    label(ui, "Size");
                    ui.add(egui::DragValue::new(size).range(0.01..=1000.0).speed(0.01));
                    ui.end_row();
                    label(ui, "Subdivisions");
                    ui.add(egui::DragValue::new(subdivisions).range(1..=256));
                    ui.end_row();
                }
            }
        });
    });
}

fn material_section(ui: &mut egui::Ui, m: &mut MaterialDesc) {
    section(ui, "material", icon::PALETTE, "Material", |ui| {
        grid(ui, "material_grid", |ui| {
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
        });
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
            layer_bits(ui, "layer", &mut b.layer);
            ui.end_row();
            label(ui, "Mask");
            layer_bits(ui, "mask", &mut b.mask);
            ui.end_row();
        });
    });
}

/// Eight toggles for the first collision layers.
fn layer_bits(ui: &mut egui::Ui, id: &str, value: &mut Option<u32>) {
    let mut bits = value.unwrap_or(1);
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
                ui.add(egui::DragValue::new(v).range(0.001..=10_000.0).speed(0.01));
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
    section(ui, "audio", icon::SPEAKER_HIGH, "Sound", |ui| {
        grid(ui, "audio_grid", |ui| {
            label(ui, "Stream");
            egui::ComboBox::from_id_salt("stream")
                .selected_text(if a.stream.is_empty() {
                    "None"
                } else {
                    &a.stream
                })
                .width(ui.available_width() - 4.0)
                .show_ui(ui, |ui| {
                    for asset in editor.assets.of_kind(AssetKind::Audio) {
                        if ui
                            .selectable_label(a.stream == asset.path, &asset.path)
                            .clicked()
                        {
                            a.stream = asset.path.clone();
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
            if class == "AudioStreamPlayer3D" {
                label(ui, "Full volume within");
                opt_f32(ui, &mut a.min_distance, 1.0, 0.0..=1000.0, 0.05);
                ui.end_row();
                label(ui, "Silent beyond");
                opt_f32(ui, &mut a.max_distance, 40.0, 0.0..=10_000.0, 0.1);
                ui.end_row();
            }
        });
    });
}

fn timer_section(ui: &mut egui::Ui, t: &mut TimerDesc) {
    section(ui, "timer", icon::TIMER, "Timer", |ui| {
        grid(ui, "timer_grid", |ui| {
            label(ui, "Wait time (s)");
            ui.add(
                egui::DragValue::new(&mut t.wait_time)
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
                    .range(0.0..=1000.0)
                    .speed(speed)
                    .max_decimals(3),
            )
            .changed();
        ui.label(RichText::new(icon::ARROW_RIGHT).color(theme::TEXT_DIM));
        changed |= ui
            .add(
                egui::DragValue::new(&mut v[1])
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
                });
            });
            section(ui, "post", icon::APERTURE, "Post-processing", |ui| {
                let post = defaults.post;
                grid(ui, "post_grid", |ui| {
                    let rows: [(&str, &mut Option<f32>, f32, f32, f64); 8] = [
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
                    ];
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Showing a node in the Inspector must not change it.
    #[test]
    fn looking_at_a_node_changes_nothing() {
        let engine = crate::projects::engine_root().unwrap();
        for (assets, scene) in [
            ("crates/ab-project/template/assets", "scenes/main.toml"),
            ("assets", "slice/level.toml"),
            ("assets", "slice/main.toml"),
        ] {
            check_scene(&engine.join(assets), scene);
        }
    }

    fn check_scene(assets: &std::path::Path, scene: &str) {
        let mut editor = Editor::new(
            assets.to_path_buf(),
            None,
            ab_engine::audio::AudioOutput::Silent,
        );
        editor.open_scene(scene);
        assert!(editor.scene_path.is_some(), "{scene} opened");
        let ctx = egui::Context::default();
        let mut panel = InspectorPanel::default();
        let nodes = editor.tree().subtree(editor.scene_root);
        for node in nodes {
            if !editor.is_editable(node) {
                continue;
            }
            let before = editor.describe_node(node);
            let path = editor.relative_path(node);
            editor.selection = Some(node);
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
