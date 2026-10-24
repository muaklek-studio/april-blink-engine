//! The Effect editor: every setting of a particle effect, a colour-over-life gradient with
//! draggable stops, the layers of an effect (several emitters, a light flash), live preview
//! controls, and saving the effect as a scene to instance or spawn from scripts
//! (`tree.spawn_effect_scene("effects/blast.abscene", ..)`).
//!
//! It edits the selected `Particles3D`, or the layers of a selected effect (a node whose
//! children are `Particles3D`s).

use super::{grid, label, section};
use crate::state::Editor;
use crate::theme;
use ab_engine::core::{Color, Vec3};
use ab_engine::effects::{ColorRamp, EmissionShape, ParticleEmitter, ParticleSettings};
use ab_engine::scene::NodeHandle;
use ab_engine::scene_file::{particle_emitter, ColorDesc, NodeDesc, ParticlesDesc};
use eframe::egui::{self, Color32, RichText, Sense, Stroke};
use egui_phosphor::regular as icon;

const PARTICLES: &str = "Particles3D";

#[derive(Default)]
pub struct EffectsPanel {
    /// The layer being edited, among the effect's.
    layer: Option<NodeHandle>,
    /// The gradient stop being edited.
    stop: Option<usize>,
    /// "Save as" file name.
    save_name: String,
    /// A save that would replace this existing file, waiting for Replace or Cancel.
    replace: Option<String>,
}

impl EffectsPanel {
    pub fn show(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        let Some((root, layers)) = effect_of(editor) else {
            super::empty_hint(
                ui,
                icon::SPARKLE,
                "Select a Particles3D, or an effect (a node with Particles3D layers). \
                 Create → Effect makes one.",
            );
            return;
        };
        if self.layer.is_none_or(|l| !layers.contains(&l)) {
            self.layer = layers.first().copied();
            self.stop = None;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.preview_bar(ui, editor, root, &layers);
                self.layers(ui, editor, root, &layers);
                if let Some(layer) = self.layer {
                    self.settings(ui, editor, layer);
                }
                self.save(ui, editor, root);
            });
    }

    fn preview_bar(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        root: NodeHandle,
        layers: &[NodeHandle],
    ) {
        let alive: usize = layers
            .iter()
            .filter_map(|&l| {
                editor
                    .tree()
                    .component::<ParticleEmitter>(l)
                    .map(|e| e.alive())
            })
            .sum();
        ui.horizontal(|ui| {
            let name = editor
                .tree()
                .name(root)
                .map(|n| n.to_owned())
                .unwrap_or_default();
            ui.label(RichText::new(name).strong());
            ui.label(
                RichText::new(format!("{alive} particles"))
                    .small()
                    .color(theme::TEXT_DIM),
            );
        });
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Restart", icon::ARROW_COUNTER_CLOCKWISE))
                .on_hover_text("Clear the particles and start again (one-shots burst again)")
                .clicked()
            {
                for &layer in layers {
                    if let Some(mut emitter) =
                        editor.tree_mut().component_mut::<ParticleEmitter>(layer)
                    {
                        emitter.restart();
                    }
                }
            }
            let playing = editor.preview_effects;
            let (glyph, tip) = if playing {
                (icon::PAUSE, "Freeze the preview")
            } else {
                (icon::PLAY, "Play the preview")
            };
            if ui.button(glyph).on_hover_text(tip).clicked() {
                editor.preview_effects = !playing;
            }
        });
        ui.add_space(4.0);
    }

    fn layers(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        root: NodeHandle,
        layers: &[NodeHandle],
    ) {
        let is_effect = !layers.contains(&root);
        section(ui, "effect_layers", icon::STACK, "Layers", |ui| {
            for &layer in layers {
                let name = editor
                    .tree()
                    .name(layer)
                    .map(|n| n.to_owned())
                    .unwrap_or_default();
                let selected = self.layer == Some(layer);
                let swatch = editor
                    .tree()
                    .component::<ParticleEmitter>(layer)
                    .map(|e| e.settings.color.sample(0.2));
                ui.horizontal(|ui| {
                    if let Some(color) = swatch {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, color32(color));
                    }
                    if ui.selectable_label(selected, name).clicked() {
                        self.layer = Some(layer);
                        self.stop = None;
                    }
                });
            }
            ui.horizontal(|ui| {
                let editable = editor.is_editable(root) && !editor.is_playing();
                ui.add_enabled_ui(editable, |ui| {
                    ui.menu_button(format!("{}  Add layer", icon::PLUS), |ui| {
                        for preset in ParticleSettings::PRESETS {
                            if ui.button(preset.replace('_', " ")).clicked() {
                                add_layer(editor, root, is_effect, preset);
                                ui.close();
                            }
                        }
                    });
                    if let Some(layer) = self.layer.filter(|_| is_effect) {
                        if ui.button(format!("{}  Duplicate", icon::COPY)).clicked() {
                            editor.duplicate(layer);
                        }
                        if layers.len() > 1
                            && ui.button(format!("{}  Remove", icon::TRASH)).clicked()
                        {
                            editor.delete(layer);
                            editor.select(Some(root));
                            self.layer = None;
                        }
                    }
                });
            });
            if !is_effect {
                ui.label(
                    RichText::new("Adding a layer turns this emitter into an effect with layers.")
                        .small()
                        .color(theme::TEXT_DIM),
                );
            }
        });
    }

    fn settings(&mut self, ui: &mut egui::Ui, editor: &mut Editor, layer: NodeHandle) {
        let Some(old) = editor.describe_node(layer) else {
            return;
        };
        let Some(desc) = old.particles.clone() else {
            return;
        };
        let editable = editor.is_editable(layer) && !editor.is_playing();
        let base = desc
            .preset
            .as_deref()
            .and_then(ParticleSettings::preset)
            .unwrap_or_default();
        let current = particle_emitter(&desc)
            .map(|e| e.settings)
            .unwrap_or_default();
        let mut settings = current.clone();
        let mut flags = Flags::of(&desc);
        let mut preset = desc.preset.clone();
        ui.add_enabled_ui(editable, |ui| {
            section(ui, "effect_preset", icon::SPARKLE, "Start from", |ui| {
                egui::ComboBox::from_id_salt("effect_preset")
                    .selected_text(preset.as_deref().unwrap_or("Plain"))
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for name in ParticleSettings::PRESETS {
                            if ui
                                .selectable_label(preset.as_deref() == Some(name), name)
                                .clicked()
                            {
                                preset = Some(name.to_owned());
                                settings = ParticleSettings::preset(name).unwrap_or_default();
                                self.stop = None;
                            }
                        }
                    });
            });
            emission(ui, &mut settings, &mut flags);
            shape(ui, &mut settings);
            motion(ui, &mut settings);
            self.look(ui, &mut settings);
        });
        // Back into the description: only what differs from the preset is written.
        let preset_base = preset
            .as_deref()
            .and_then(ParticleSettings::preset)
            .unwrap_or_default();
        let base = if preset == desc.preset {
            base
        } else {
            preset_base
        };
        // Only when something was changed here: looking at an effect must not rewrite it.
        let changed = settings != current || flags != Flags::of(&desc) || preset != desc.preset;
        let fresh = describe(&settings, &base, preset, flags);
        if changed && fresh != desc {
            let mut new = old.clone();
            new.particles = Some(fresh);
            editor.apply_desc(layer, &old, new);
        }
    }

    fn look(&mut self, ui: &mut egui::Ui, s: &mut ParticleSettings) {
        section(ui, "effect_look", icon::PALETTE, "Look", |ui| {
            ui.label(
                RichText::new("Color over life")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            gradient(ui, &mut s.color, &mut self.stop);
            grid(ui, "effect_look_grid", |ui| {
                label(ui, "Size start → end");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut s.size[0])
                            .speed(0.005)
                            .range(0.0..=50.0),
                    );
                    ui.add(
                        egui::DragValue::new(&mut s.size[1])
                            .speed(0.005)
                            .range(0.0..=50.0),
                    );
                });
                ui.end_row();
                label(ui, "Size variety");
                ui.add(egui::Slider::new(&mut s.size_random, 0.0..=1.0));
                ui.end_row();
                label(ui, "Glow");
                ui.add(egui::Slider::new(&mut s.emission, 0.0..=20.0).logarithmic(true));
                ui.end_row();
                label(ui, "Hardness");
                ui.add(egui::Slider::new(&mut s.hardness, 0.0..=1.0))
                    .on_hover_text("0 soft like smoke, 1 a hard dot");
                ui.end_row();
                label(ui, "Stretch");
                ui.add(egui::Slider::new(&mut s.stretch, 0.0..=0.5))
                    .on_hover_text("Long along the motion: sparks, rain");
                ui.end_row();
                label(ui, "Spin min → max");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut s.spin[0])
                            .speed(0.05)
                            .suffix(" rad/s"),
                    );
                    ui.add(
                        egui::DragValue::new(&mut s.spin[1])
                            .speed(0.05)
                            .suffix(" rad/s"),
                    );
                });
                ui.end_row();
                label(ui, "Adds light");
                ui.checkbox(&mut s.additive, "")
                    .on_hover_text("Fire, sparks, magic glow; smoke and dust cover instead");
                ui.end_row();
            });
        });
    }

    fn save(&mut self, ui: &mut egui::Ui, editor: &mut Editor, root: NodeHandle) {
        section(
            ui,
            "effect_save",
            icon::FLOPPY_DISK,
            "Save as effect",
            |ui| {
                if self.save_name.is_empty() {
                    self.save_name = editor
                        .tree()
                        .name(root)
                        .map(|n| n.to_lowercase())
                        .unwrap_or_else(|| "effect".into());
                }
                ui.horizontal(|ui| {
                    ui.label("effects/");
                    ui.add(egui::TextEdit::singleline(&mut self.save_name).desired_width(140.0));
                    ui.label(format!(".{}", ab_engine::scene_file::SCENE_EXTENSION));
                });
                let name: String = self
                    .save_name
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '-' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                let relative = format!("effects/{name}.{}", ab_engine::scene_file::SCENE_EXTENSION);
                let confirming = self.replace.as_deref() == Some(relative.as_str());
                let mut write = None;
                if confirming {
                    ui.label(
                        RichText::new(format!("{}  {relative} already exists.", icon::WARNING))
                            .color(theme::WARN),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Replace").clicked() {
                            write = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            self.replace = None;
                        }
                    });
                } else if ui
                    .add_enabled(
                        !name.is_empty(),
                        egui::Button::new(format!("{}  Save", icon::FLOPPY_DISK)),
                    )
                    .clicked()
                {
                    write = Some(false);
                }
                if let Some(overwrite) = write {
                    self.replace = None;
                    match save_effect(editor, root, &relative, overwrite) {
                        Ok(true) => {
                            editor.assets.request_rescan();
                            editor.info(format!(
                                "Saved {relative}: drag it into a scene, or spawn it from a script"
                            ));
                        }
                        Ok(false) => self.replace = Some(relative.clone()),
                        Err(err) => editor.fail(format!("cannot save {relative}: {err}")),
                    }
                }
                ui.label(
                    RichText::new(format!(
                        "From a script: tree.spawn_effect_scene(parent, position, \"{relative}\")"
                    ))
                    .small()
                    .monospace()
                    .color(theme::TEXT_DIM),
                );
            },
        );
    }
}

/// Writes the effect at `root` to `relative` (in the project). `Ok(false)`: the file exists
/// and `overwrite` is not set, nothing written.
fn save_effect(
    editor: &Editor,
    root: NodeHandle,
    relative: &str,
    overwrite: bool,
) -> std::io::Result<bool> {
    let path = editor.project().join(relative);
    if path.exists() && !overwrite {
        return Ok(false);
    }
    let scene = ab_engine::scene_file::save_scene(editor.tree(), root);
    editor.write_project_file(&path, scene.to_toml())?;
    Ok(true)
}

/// The effect being edited: (its root, its Particles3D layers).
fn effect_of(editor: &Editor) -> Option<(NodeHandle, Vec<NodeHandle>)> {
    let tree = editor.tree();
    let node = editor.selection.filter(|&n| tree.is_valid(n))?;
    let is_particles = |n: NodeHandle| tree.class_name(n) == Some(PARTICLES);
    let layers_of = |root: NodeHandle| -> Vec<NodeHandle> {
        tree.children(root)
            .into_iter()
            .filter(|&c| is_particles(c))
            .collect()
    };
    // A layer of an effect edits the whole effect.
    if is_particles(node) {
        let parent = tree.parent(node)?;
        let siblings = layers_of(parent);
        if parent != editor.scene_root && siblings.len() > 1 {
            return Some((parent, siblings));
        }
        return Some((node, vec![node]));
    }
    let layers = layers_of(node);
    (!layers.is_empty()).then_some((node, layers))
}

/// Adds a layer from `preset` to the effect at `root` (or makes a lone emitter an effect).
fn add_layer(editor: &mut Editor, root: NodeHandle, is_effect: bool, preset: &str) {
    let layer = NodeDesc {
        name: preset.replace('_', " ").replace(' ', ""),
        kind: Some(PARTICLES.into()),
        particles: Some(ParticlesDesc {
            preset: Some(preset.to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let parent = if is_effect {
        root
    } else {
        // A lone emitter: the new layer goes beside it, under a new effect node.
        editor.tree().parent(root).unwrap_or(editor.scene_root)
    };
    editor.select(Some(parent));
    editor.add_node(layer, None);
}

/// Which emitter flags the description sets.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Flags {
    one_shot: bool,
    local: bool,
    emitting: bool,
}

impl Flags {
    fn of(desc: &ParticlesDesc) -> Self {
        Self {
            one_shot: desc.one_shot.unwrap_or(false),
            local: desc.local.unwrap_or(false),
            emitting: desc.emitting.unwrap_or(true),
        }
    }
}

fn emission(ui: &mut egui::Ui, s: &mut ParticleSettings, flags: &mut Flags) {
    section(ui, "effect_emission", icon::FLOW_ARROW, "Emission", |ui| {
        grid(ui, "effect_emission_grid", |ui| {
            label(ui, "Amount");
            ui.add(
                egui::Slider::new(&mut s.amount, 1..=ab_engine::effects::MAX_PARTICLES)
                    .logarithmic(true),
            )
            .on_hover_text("Alive at once, or emitted at once for a one-shot");
            ui.end_row();
            label(ui, "Lifetime (s)");
            ui.add(egui::Slider::new(&mut s.lifetime, 0.02..=30.0).logarithmic(true));
            ui.end_row();
            label(ui, "Lifetime variety");
            ui.add(egui::Slider::new(&mut s.lifetime_random, 0.0..=1.0));
            ui.end_row();
            label(ui, "One shot");
            ui.checkbox(&mut flags.one_shot, "")
                .on_hover_text("Emit everything at once (impacts, explosions)");
            ui.end_row();
            label(ui, "Emitting");
            ui.checkbox(&mut flags.emitting, "");
            ui.end_row();
            label(ui, "Follow the emitter");
            ui.checkbox(&mut flags.local, "")
                .on_hover_text("Particles move with the node (trails attached to it)");
            ui.end_row();
        });
    });
}

fn shape(ui: &mut egui::Ui, s: &mut ParticleSettings) {
    section(
        ui,
        "effect_shape",
        icon::SHAPES,
        "Shape and direction",
        |ui| {
            grid(ui, "effect_shape_grid", |ui| {
                label(ui, "Born in");
                let kinds = ["Point", "Sphere", "Box", "Ring"];
                let mut kind = match s.shape {
                    EmissionShape::Point => 0,
                    EmissionShape::Sphere { .. } => 1,
                    EmissionShape::Box { .. } => 2,
                    EmissionShape::Ring { .. } => 3,
                };
                let before = kind;
                egui::ComboBox::from_id_salt("effect_shape_kind")
                    .selected_text(kinds[kind])
                    .show_ui(ui, |ui| {
                        for (i, name) in kinds.iter().enumerate() {
                            ui.selectable_value(&mut kind, i, *name);
                        }
                    });
                if kind != before {
                    s.shape = match kind {
                        1 => EmissionShape::Sphere { radius: 0.3 },
                        2 => EmissionShape::Box {
                            size: Vec3::new(1.0, 0.2, 1.0),
                        },
                        3 => EmissionShape::Ring { radius: 0.5 },
                        _ => EmissionShape::Point,
                    };
                }
                ui.end_row();
                match &mut s.shape {
                    EmissionShape::Sphere { radius } | EmissionShape::Ring { radius } => {
                        label(ui, "Radius");
                        ui.add(
                            egui::DragValue::new(radius)
                                .speed(0.01)
                                .range(0.0..=100.0)
                                .suffix(" m"),
                        );
                        ui.end_row();
                    }
                    EmissionShape::Box { size } => {
                        label(ui, "Size");
                        let mut v = size.to_array();
                        if super::vec3(ui, &mut v, 0.01, " m") {
                            *size = Vec3::from(v).max(Vec3::ZERO);
                        }
                        ui.end_row();
                    }
                    EmissionShape::Point => {}
                }
                label(ui, "Direction");
                let mut d = s.direction.to_array();
                if super::vec3(ui, &mut d, 0.01, "") {
                    s.direction = Vec3::from(d);
                }
                ui.end_row();
                label(ui, "Spread");
                ui.add(egui::Slider::new(&mut s.spread, 0.0..=180.0).suffix("°"))
                    .on_hover_text("Cone around the direction; 180 sends them every way");
                ui.end_row();
            });
        },
    );
}

fn motion(ui: &mut egui::Ui, s: &mut ParticleSettings) {
    section(ui, "effect_motion", icon::WIND, "Motion", |ui| {
        grid(ui, "effect_motion_grid", |ui| {
            label(ui, "Speed min → max");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut s.speed[0])
                        .speed(0.02)
                        .suffix(" m/s"),
                );
                ui.add(
                    egui::DragValue::new(&mut s.speed[1])
                        .speed(0.02)
                        .suffix(" m/s"),
                );
            });
            ui.end_row();
            label(ui, "Gravity");
            let mut g = s.gravity.to_array();
            if super::vec3(ui, &mut g, 0.05, "") {
                s.gravity = Vec3::from(g);
            }
            ui.end_row();
            label(ui, "");
            ui.horizontal(|ui| {
                for (name, value) in [("Falls", -9.81), ("Floats", 0.0), ("Rises", 1.0)] {
                    if ui.small_button(name).clicked() {
                        s.gravity = Vec3::new(0.0, value, 0.0);
                    }
                }
            });
            ui.end_row();
            label(ui, "Air drag");
            ui.add(egui::Slider::new(&mut s.damping, 0.0..=10.0));
            ui.end_row();
            label(ui, "Turbulence");
            ui.add(egui::Slider::new(&mut s.turbulence, 0.0..=5.0))
                .on_hover_text("Random swirling");
            ui.end_row();
        });
    });
}

/// A colour-over-life editor: the ramp drawn over a checkerboard (for transparency), its
/// stops below as handles to drag; click the bar to add a stop at that point, pick a stop to
/// edit its colour.
fn gradient(ui: &mut egui::Ui, ramp: &mut ColorRamp, selected: &mut Option<usize>) {
    let width = ui.available_width().max(120.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 22.0), Sense::click());
    let painter = ui.painter_at(rect.expand(8.0));
    // Checkerboard.
    let cell = 6.0;
    let mut x = rect.left();
    let mut column = 0;
    while x < rect.right() {
        for row in 0..4 {
            let dark = (column + row) % 2 == 0;
            let tile = egui::Rect::from_min_size(
                egui::pos2(x, rect.top() + row as f32 * cell),
                egui::vec2(cell, cell),
            )
            .intersect(rect);
            painter.rect_filled(
                tile,
                0.0,
                if dark {
                    Color32::from_gray(70)
                } else {
                    Color32::from_gray(110)
                },
            );
        }
        x += cell;
        column += 1;
    }
    // The ramp, in thin slices.
    let slices = 48;
    for i in 0..slices {
        let t0 = i as f32 / slices as f32;
        let t1 = (i + 1) as f32 / slices as f32;
        let color = color32(ramp.sample((t0 + t1) * 0.5));
        let slice = egui::Rect::from_min_max(
            egui::pos2(rect.left() + t0 * rect.width(), rect.top()),
            egui::pos2(rect.left() + t1 * rect.width() + 0.5, rect.bottom()),
        );
        painter.rect_filled(slice, 0.0, color);
    }
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0_f32, theme::TEXT_DIM),
        egui::StrokeKind::Outside,
    );
    // Click on the bar: a new stop with the colour there.
    if response.clicked() {
        if let Some(p) = response.interact_pointer_pos() {
            let t = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let color = ramp.sample(t);
            ramp.0.push((t, color));
            ramp.0.sort_by(|a, b| a.0.total_cmp(&b.0));
            *selected = ramp.0.iter().position(|s| s.0 == t);
        }
    }
    // Handles.
    let (handles, _) = ui.allocate_exact_size(egui::vec2(width, 14.0), Sense::hover());
    let mut moved = None;
    for (i, &(t, color)) in ramp.0.iter().enumerate() {
        let x = handles.left() + t * handles.width();
        let id = ui.id().with(("gradient_stop", i));
        let hit =
            egui::Rect::from_center_size(egui::pos2(x, handles.center().y), egui::vec2(12.0, 14.0));
        let handle = ui.interact(hit, id, Sense::click_and_drag());
        if handle.clicked() || handle.drag_started() {
            *selected = Some(i);
        }
        if handle.dragged() {
            let dx = handle.drag_delta().x / handles.width();
            moved = Some((i, (t + dx).clamp(0.0, 1.0)));
        }
        let is_selected = *selected == Some(i);
        let tip = egui::pos2(x, handles.top());
        let points = vec![
            tip,
            egui::pos2(x + 6.0, handles.bottom()),
            egui::pos2(x - 6.0, handles.bottom()),
        ];
        ui.painter().add(egui::Shape::convex_polygon(
            points,
            color32(Color::rgba(color.r, color.g, color.b, 1.0)),
            Stroke::new(
                if is_selected { 2.0 } else { 1.0 },
                if is_selected {
                    Color32::WHITE
                } else {
                    theme::TEXT_DIM
                },
            ),
        ));
    }
    if let Some((i, t)) = moved {
        ramp.0[i].0 = t;
        let stop = ramp.0[i];
        ramp.0.sort_by(|a, b| a.0.total_cmp(&b.0));
        *selected = ramp.0.iter().position(|s| *s == stop);
    }
    // The selected stop's colour.
    if let Some(i) = selected.filter(|&i| i < ramp.0.len()) {
        ui.horizontal(|ui| {
            let (t, color) = ramp.0[i];
            ui.label(RichText::new(format!("{:.0}% of life", t * 100.0)).small());
            let mut rgba = super::to_rgba(color);
            if egui::color_picker::color_edit_button_rgba(
                ui,
                &mut rgba,
                egui::color_picker::Alpha::OnlyBlend,
            )
            .changed()
            {
                let [r, g, b, a] = rgba.to_rgba_unmultiplied();
                ramp.0[i].1 = Color::rgba(r, g, b, a);
            }
            if ramp.0.len() > 2
                && ui
                    .small_button(icon::TRASH)
                    .on_hover_text("Remove this stop")
                    .clicked()
            {
                ramp.0.remove(i);
                *selected = None;
            }
        });
    } else {
        ui.label(
            RichText::new("Click the bar to add a stop, a handle to edit or drag it.")
                .small()
                .color(theme::TEXT_DIM),
        );
    }
}

fn color32(c: Color) -> Color32 {
    let [r, g, b, a] = super::to_rgba(c).to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// The description of `settings`: the preset, plus every field that differs from it.
fn describe(
    s: &ParticleSettings,
    base: &ParticleSettings,
    preset: Option<String>,
    flags: Flags,
) -> ParticlesDesc {
    let differs = |a: f32, b: f32| (a - b).abs() > 1e-5;
    let pair = |a: [f32; 2], b: [f32; 2]| (differs(a[0], b[0]) || differs(a[1], b[1])).then_some(a);
    let vec = |a: Vec3, b: Vec3| (a.distance(b) > 1e-5).then_some(a.to_array());
    let one = |a: f32, b: f32| differs(a, b).then_some(a);
    ParticlesDesc {
        preset,
        amount: (s.amount != base.amount).then_some(s.amount),
        lifetime: one(s.lifetime, base.lifetime),
        lifetime_random: one(s.lifetime_random, base.lifetime_random),
        shape: (s.shape != base.shape).then_some(s.shape),
        direction: vec(s.direction, base.direction),
        spread: one(s.spread, base.spread),
        gravity: vec(s.gravity, base.gravity),
        damping: one(s.damping, base.damping),
        turbulence: one(s.turbulence, base.turbulence),
        size_random: one(s.size_random, base.size_random),
        colors: if s.color != base.color {
            s.color
                .0
                .iter()
                .map(|&(t, c)| ((t * 1000.0).round() / 1000.0, ColorDesc::from_color(c)))
                .collect()
        } else {
            Vec::new()
        },
        spin: pair(s.spin, base.spin),
        hardness: one(s.hardness, base.hardness),
        stretch: one(s.stretch, base.stretch),
        size: pair(s.size, base.size),
        speed: pair(s.speed, base.speed),
        color_start: None,
        color_end: None,
        emission: one(s.emission, base.emission),
        additive: (s.additive != base.additive).then_some(s.additive),
        one_shot: flags.one_shot.then_some(true),
        local: flags.local.then_some(true),
        emitting: (!flags.emitting).then_some(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_an_effect_does_not_replace_a_file_unasked() {
        let dir = std::env::temp_dir().join(format!("ab-effect-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("effects")).unwrap();
        std::fs::write(dir.join("effects/fire.abscene"), "mine").unwrap();
        let mut editor = Editor::new(dir.clone(), None, ab_engine::audio::AudioOutput::Silent);
        editor.select(None);
        editor.add_node(
            NodeDesc {
                name: "Fire".into(),
                kind: Some("Node3D".into()),
                ..Default::default()
            },
            None,
        );
        let root = editor.selection.unwrap();
        assert!(!save_effect(&editor, root, "effects/fire.abscene", false).unwrap());
        assert_eq!(std::fs::read_to_string(dir.join("effects/fire.abscene")).unwrap(), "mine");
        assert!(save_effect(&editor, root, "effects/fire.abscene", true).unwrap());
        assert_ne!(std::fs::read_to_string(dir.join("effects/fire.abscene")).unwrap(), "mine");
        assert!(save_effect(&editor, root, "effects/new.abscene", false).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn descriptions_round_trip_through_the_settings() {
        let base = ParticleSettings::fire();
        let mut settings = base.clone();
        settings.amount = 99;
        settings.gravity = Vec3::new(0.0, 3.0, 0.0);
        settings.shape = EmissionShape::Ring { radius: 0.8 };
        settings.color = ColorRamp(vec![
            (0.0, Color::rgba(1.0, 0.5, 0.0, 1.0)),
            (1.0, Color::rgba(0.0, 0.0, 1.0, 0.0)),
        ]);
        let flags = Flags {
            one_shot: true,
            local: false,
            emitting: true,
        };
        let desc = describe(&settings, &base, Some("fire".into()), flags);
        assert_eq!(desc.amount, Some(99));
        assert_eq!(desc.lifetime, None, "unchanged fields stay out of the file");
        assert_eq!(desc.one_shot, Some(true));
        let back = particle_emitter(&desc).unwrap();
        assert!(back.one_shot);
        let s = back.settings;
        assert_eq!(
            (s.amount, s.gravity, s.shape),
            (99, settings.gravity, settings.shape)
        );
        assert!((s.color.sample(0.0).r - 1.0).abs() < 0.01);
        assert_eq!(s.lifetime, base.lifetime);
        // Through TOML too.
        let node = NodeDesc {
            name: "Fx".into(),
            kind: Some(PARTICLES.into()),
            particles: Some(desc.clone()),
            ..Default::default()
        };
        let file = ab_engine::scene_file::SceneFile {
            environment: None,
            nodes: vec![node],
        };
        let text = file.to_toml();
        let parsed = ab_engine::scene_file::SceneFile::from_toml(&text).unwrap();
        assert_eq!(parsed.nodes[0].particles.as_ref(), Some(&desc), "{text}");
    }
}
