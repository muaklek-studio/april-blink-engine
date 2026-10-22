//! Debug tools: time control, debug views, input, damage and object tracing, ragdoll
//! testing and live counters.

use super::{grid, label, section};
use crate::state::{Editor, Mode};
use crate::theme;
use ab_engine::prelude::*;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;

pub fn show(ui: &mut egui::Ui, editor: &mut Editor) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            simulation(ui, editor);
            views(ui, editor);
            input(ui, editor);
            damage_trace(ui, editor);
            object_trace(ui, editor);
            ragdolls(ui, editor);
            counters(ui, editor);
        });
}

fn simulation(ui: &mut egui::Ui, editor: &mut Editor) {
    section(ui, "debug_sim", icon::CLOCK, "Simulation", |ui| {
        let playing = editor.is_playing();
        ui.horizontal(|ui| {
            let paused = matches!(editor.mode, Mode::Play { paused: true });
            if ui
                .add_enabled(
                    playing,
                    egui::Button::new(if paused { icon::PLAY } else { icon::PAUSE }),
                )
                .on_hover_text("Pause / resume (F6)")
                .clicked()
            {
                editor.toggle_pause();
            }
            if ui
                .add_enabled(
                    playing,
                    egui::Button::new(format!("{}  Step", icon::SKIP_FORWARD)),
                )
                .on_hover_text("Advance one frame while paused (F7)")
                .clicked()
            {
                editor.step_frame();
            }
            if !playing {
                ui.label(
                    RichText::new("Play (F5) to use these")
                        .small()
                        .color(theme::TEXT_DIM),
                );
            }
        });
        let world = editor.tree().world();
        if let Some(mut time) = world.resource_mut::<Time>() {
            grid(ui, "sim_grid", |ui| {
                label(ui, "Time scale");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Slider::new(&mut time.time_scale, 0.0..=2.0)
                            .fixed_decimals(2)
                            .suffix("×"),
                    );
                });
                ui.end_row();
                label(ui, "");
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for preset in [0.1, 0.25, 0.5, 1.0, 2.0] {
                        if ui
                            .selectable_label(
                                (time.time_scale - preset).abs() < 1e-4,
                                format!("{preset}×"),
                            )
                            .clicked()
                        {
                            time.time_scale = preset;
                        }
                    }
                });
                ui.end_row();
                label(ui, "Game time");
                ui.label(
                    RichText::new(format!(
                        "{:.2} s · frame {}",
                        time.elapsed, time.frame_count
                    ))
                    .monospace(),
                );
                ui.end_row();
            });
        }
    });
}

fn views(ui: &mut egui::Ui, editor: &mut Editor) {
    section(ui, "debug_views", icon::EYE, "Debug views", |ui| {
        let world = editor.tree().world();
        if let Some(mut settings) = world.resource_mut::<DebugSettings>() {
            grid(ui, "views_grid", |ui| {
                label(ui, "Physics (F4)");
                ui.checkbox(&mut settings.physics, "");
                ui.end_row();
                let on = settings.physics;
                let options = &mut settings.physics_options;
                for (name, value) in [
                    ("  Shapes", &mut options.shapes),
                    ("  Joints", &mut options.joints),
                    ("  Contacts", &mut options.contacts),
                    ("  Bounding boxes", &mut options.aabbs),
                ] {
                    label(ui, name);
                    ui.add_enabled(on, egui::Checkbox::without_text(value));
                    ui.end_row();
                }
                label(ui, "Navigation grid");
                ui.checkbox(&mut settings.navigation, "");
                ui.end_row();
                label(ui, "Mesh bounds");
                ui.checkbox(&mut settings.bounds, "").on_hover_text(
                    "The world bounding box of every visible mesh, as the renderer culls it",
                );
                ui.end_row();
            });
        }
        grid(ui, "views_grid2", |ui| {
            if let Some(mut debug) = world.resource_mut::<DebugDraw>() {
                label(ui, "Game debug lines");
                ui.checkbox(&mut debug.enabled, "")
                    .on_hover_text("Lines drawn by game code through DebugDraw");
                ui.end_row();
            }
            if let Some(mut overlay) = world.resource_mut::<DebugOverlay>() {
                label(ui, "Stats overlay (F3)");
                ui.checkbox(&mut overlay.visible, "");
                ui.end_row();
            }
        });
        ui.label(
            RichText::new("Physics colours: orange simulated · cyan kinematic · green static · violet areas · yellow joints · red contacts. Mesh bounds: green static · yellow skinned (posed)")
                .small()
                .color(theme::TEXT_DIM),
        );
    });
}

/// Live input while playing: keys, mouse, gamepad sticks, actions (with buttons to hold
/// them down without touching the keyboard) and the latest presses.
fn input(ui: &mut egui::Ui, editor: &mut Editor) {
    use ab_engine::input::{Actions, GamepadAxis, InputLog, InputState};
    section(ui, "debug_input", icon::GAME_CONTROLLER, "Input", |ui| {
        let world = editor.tree().world();
        let (Some(state), Some(mut actions)) = (
            world.resource::<InputState>(),
            world.resource_mut::<Actions>(),
        ) else {
            return;
        };
        if !editor.is_playing() {
            ui.label(
                RichText::new(
                    "Play (F5) to see the game's input. Held actions stay held while playing.",
                )
                .small()
                .color(theme::TEXT_DIM),
            );
        }
        let keys: Vec<String> = state.keys.pressed().map(|k| format!("{k:?}")).collect();
        let buttons: Vec<String> = state
            .mouse_buttons
            .pressed()
            .map(|b| format!("{b:?}"))
            .collect();
        grid(ui, "input_grid", |ui| {
            label(ui, "Keys");
            ui.label(
                RichText::new(if keys.is_empty() {
                    "—".into()
                } else {
                    keys.join(" ")
                })
                .monospace(),
            );
            ui.end_row();
            label(ui, "Mouse");
            let delta = state.mouse_delta();
            ui.label(
                RichText::new(format!(
                    "Δ {:+.1} {:+.1}  {}",
                    delta.x,
                    delta.y,
                    buttons.join(" ")
                ))
                .monospace(),
            );
            ui.end_row();
            let look = actions.look();
            label(ui, "Look");
            ui.label(RichText::new(format!("{:+.3} {:+.3}", look.x, look.y)).monospace());
            ui.end_row();
        });
        // Gamepads: both sticks as dots in circles, triggers as bars.
        for (id, pad) in state.gamepads() {
            ui.horizontal(|ui| {
                ui.label(format!("Pad {}", id.0));
                for (x, y) in [
                    (GamepadAxis::LeftStickX, GamepadAxis::LeftStickY),
                    (GamepadAxis::RightStickX, GamepadAxis::RightStickY),
                ] {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
                    let painter = ui.painter();
                    painter.circle_stroke(
                        rect.center(),
                        20.0,
                        egui::Stroke::new(1.0_f32, theme::TEXT_DIM),
                    );
                    let dot = rect.center() + egui::vec2(pad.axis(x), -pad.axis(y)) * 20.0;
                    painter.circle_filled(dot, 4.0, theme::ACCENT);
                }
                for trigger in [GamepadAxis::LeftTrigger, GamepadAxis::RightTrigger] {
                    ui.add(
                        egui::ProgressBar::new(pad.axis(trigger))
                            .desired_width(40.0)
                            .text(format!("{trigger:?}").replace("Trigger", "T")),
                    );
                }
            });
        }
        ui.add_space(4.0);
        let names: Vec<String> = actions.names().map(str::to_owned).collect();
        if names.is_empty() {
            ui.label(
                RichText::new("No actions: the project's input.toml defines them.")
                    .small()
                    .color(theme::TEXT_DIM),
            );
        }
        egui::Grid::new("input_actions")
            .num_columns(3)
            .spacing([8.0, 3.0])
            .show(ui, |ui| {
                for name in &names {
                    let pressed = actions.is_pressed(name);
                    let text = RichText::new(name).monospace();
                    ui.label(if pressed {
                        text.color(theme::PLAY)
                    } else {
                        text
                    });
                    ui.add(
                        egui::ProgressBar::new(actions.strength(name))
                            .desired_width(80.0)
                            .text(format!("{:.2}", actions.strength(name))),
                    );
                    let mut held = actions.is_held(name);
                    if ui
                        .toggle_value(&mut held, "Hold")
                        .on_hover_text("Press this action down as if its key were held")
                        .changed()
                    {
                        if held {
                            actions.hold(name, 1.0);
                        } else {
                            actions.release(name);
                        }
                    }
                    ui.end_row();
                }
            });
        drop(actions);
        drop(state);
        if let Some(log) = world.resource::<InputLog>() {
            let entries: Vec<String> = log
                .entries()
                .take(10)
                .map(|(t, text)| format!("{t:7.2}s  {text}"))
                .collect();
            if !entries.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new("Recent").small().color(theme::TEXT_DIM));
                for line in entries {
                    ui.label(RichText::new(line).monospace().small());
                }
            }
        }
        if let Some(mut overlay) = world.resource_mut::<ab_engine::InputOverlay>() {
            ui.checkbox(&mut overlay.visible, "Show in the game too (F8)");
        }
    });
}

/// A text filter kept in egui's memory under `id`.
fn filter(ui: &mut egui::Ui, id: &str) -> String {
    let id = egui::Id::new(id);
    let mut text = ui.data_mut(|d| d.get_temp::<String>(id).unwrap_or_default());
    ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text(format!("{}  Filter by name", icon::MAGNIFYING_GLASS))
            .desired_width(ui.available_width() - 4.0),
    );
    ui.data_mut(|d| d.insert_temp(id, text.clone()));
    text.to_lowercase()
}

/// A node name that selects the node when clicked (while it exists).
fn node_link(
    ui: &mut egui::Ui,
    editor: &Editor,
    node: Option<NodeHandle>,
    name: &str,
) -> Option<NodeHandle> {
    let live = node.filter(|&n| editor.tree().is_valid(n));
    match live {
        Some(n) => ui
            .add(
                egui::Label::new(RichText::new(name).monospace().color(theme::ACCENT))
                    .sense(egui::Sense::click()),
            )
            .on_hover_text("Select it")
            .clicked()
            .then_some(n),
        None => {
            ui.label(RichText::new(name).monospace().color(theme::TEXT_DIM));
            None
        }
    }
}

fn number(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round())
    } else {
        format!("{v:.1}")
    }
}

/// Who hit whom, for how much, whether it landed, kills, and totals per object.
fn damage_trace(ui: &mut egui::Ui, editor: &mut Editor) {
    use ab_engine::trace::{DamageKind, Outcome, TraceOverlay};
    let mut select: Option<NodeHandle> = None;
    section(ui, "debug_damage", icon::SWORD, "Damage trace", |ui| {
        let world = editor.tree().world();
        let Some(mut tracer) = world.resource_mut::<Tracer>() else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut tracer.enabled, "Record");
            ui.checkbox(&mut tracer.draw, "Hit lines")
                .on_hover_text("Attacker → target arrows and hit marks in the world");
            ui.checkbox(&mut tracer.popups, "Damage numbers");
            if let Some(mut overlay) = world.resource_mut::<TraceOverlay>() {
                ui.checkbox(&mut overlay.visible, "In the game (F9)");
            }
        });
        ui.horizontal(|ui| {
            if ui.button(format!("{}  Clear", icon::TRASH)).clicked() {
                tracer.clear();
            }
            if ui
                .button(format!("{}  Copy CSV", icon::COPY))
                .on_hover_text("The whole log, for a spreadsheet")
                .clicked()
            {
                ui.ctx().copy_text(tracer.damage_csv());
            }
        });
        if !editor.is_playing() && tracer.damage().len() == 0 {
            ui.label(
                RichText::new(
                    "Play (F5) and hit something. Scripts deal damage with \
                     tree.deal_damage(Some(node), target, amount) (or a `damage` signal); \
                     health scripts answer with `damaged` and `died`.",
                )
                .small()
                .color(theme::TEXT_DIM),
            );
            return;
        }

        let totals = tracer.totals();
        if !totals.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("Totals").small().color(theme::TEXT_DIM));
            egui::Grid::new("damage_totals")
                .striped(true)
                .spacing([10.0, 2.0])
                .show(ui, |ui| {
                    for title in [
                        "Object", "Dealt", "Taken", "Hits", "Ignored", "Kills", "Deaths",
                    ] {
                        ui.label(RichText::new(title).small().color(theme::TEXT_DIM));
                    }
                    ui.end_row();
                    for t in &totals {
                        if let Some(n) = node_link(ui, editor, t.node, &t.name) {
                            select = Some(n);
                        }
                        for value in [
                            number(t.dealt),
                            number(t.taken),
                            t.hits.to_string(),
                            t.ignored.to_string(),
                            t.kills.to_string(),
                            t.deaths.to_string(),
                        ] {
                            ui.label(RichText::new(value).monospace());
                        }
                        ui.end_row();
                    }
                });
        }

        ui.add_space(4.0);
        let wanted = filter(ui, "damage_filter");
        egui::ScrollArea::vertical()
            .id_salt("damage_log")
            .max_height(240.0)
            .show(ui, |ui| {
                egui::Grid::new("damage_log_grid")
                    .striped(true)
                    .spacing([8.0, 2.0])
                    .show(ui, |ui| {
                        for e in tracer.damage().rev().take(300) {
                            if !wanted.is_empty()
                                && !e.source_name.to_lowercase().contains(&wanted)
                                && !e.target_name.to_lowercase().contains(&wanted)
                            {
                                continue;
                            }
                            ui.label(
                                RichText::new(format!("{:.2}", e.time))
                                    .monospace()
                                    .small()
                                    .color(theme::TEXT_DIM),
                            );
                            if let Some(n) = node_link(ui, editor, e.source, &e.source_name) {
                                select = Some(n);
                            }
                            ui.label(
                                RichText::new(if e.kind == DamageKind::Death {
                                    "killed"
                                } else {
                                    "→"
                                })
                                .color(theme::TEXT_DIM),
                            );
                            if let Some(n) = node_link(ui, editor, Some(e.target), &e.target_name) {
                                select = Some(n);
                            }
                            let (text, color) = match (e.kind, e.outcome) {
                                (DamageKind::Death, _) => ("KO".to_owned(), egui::Color32::WHITE),
                                (_, Outcome::Ignored) => {
                                    (format!("{} ignored", number(e.amount)), theme::TEXT_DIM)
                                }
                                (_, Outcome::Pending) => {
                                    (format!("{} …", number(e.amount)), theme::TEXT_DIM)
                                }
                                (DamageKind::Damage, _) => {
                                    (format!("-{}", number(e.applied)), theme::ERROR)
                                }
                                (DamageKind::Heal, _) => {
                                    (format!("+{}", number(e.applied)), theme::PLAY)
                                }
                            };
                            ui.label(RichText::new(text).monospace().color(color));
                            let hp = e
                                .health
                                .map(|(c, m)| format!("hp {}/{}", number(c), number(m)))
                                .unwrap_or_default();
                            ui.label(RichText::new(hp).monospace().small().color(theme::TEXT_DIM));
                            ui.end_row();
                        }
                    });
            });
        ui.label(
            RichText::new(
                "Ignored: nobody answered (invulnerable, already dead, or no health script).",
            )
            .small()
            .color(theme::TEXT_DIM),
        );
    });
    if let Some(node) = select {
        editor.select(Some(node));
    }
}

/// Spawns and frees, watched objects (position, speed, health, trail) and their signals.
fn object_trace(ui: &mut egui::Ui, editor: &mut Editor) {
    use ab_engine::trace::{ObjectEventKind, Tracing};
    let mut select: Option<NodeHandle> = None;
    let mut watch: Option<(NodeHandle, bool)> = None;
    section(ui, "debug_objects", icon::PATH, "Object trace", |ui| {
        let world = editor.tree().world();
        let Some(mut tracer) = world.resource_mut::<Tracer>() else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut tracer.lifecycle, "Spawns and frees");
            ui.checkbox(&mut tracer.all_signals, "Every signal")
                .on_hover_text("Every node's signals, not only the watched ones' (busy)");
        });
        let selected = editor.selection.filter(|_| editor.is_playing());
        ui.horizontal(|ui| {
            let can_watch = selected.is_some_and(|n| !tracer.is_watched(n));
            if ui
                .add_enabled(
                    can_watch,
                    egui::Button::new(format!("{}  Watch selected", icon::CROSSHAIR)),
                )
                .on_hover_text("Its trail, speed, health and every signal it emits")
                .on_disabled_hover_text("Select an object while playing")
                .clicked()
            {
                watch = selected.map(|n| (n, true));
            }
        });
        if !tracer.watches().is_empty() {
            egui::Grid::new("watches")
                .striped(true)
                .spacing([10.0, 2.0])
                .show(ui, |ui| {
                    for w in tracer.watches() {
                        if let Some(n) = node_link(ui, editor, Some(w.node), &w.name) {
                            select = Some(n);
                        }
                        let position = w.position.map_or("-".to_owned(), |p| {
                            format!("{:.1} {:.1} {:.1}", p.x, p.y, p.z)
                        });
                        ui.label(RichText::new(position).monospace().small());
                        ui.label(
                            RichText::new(format!("{:.1} m/s", w.velocity.length()))
                                .monospace()
                                .small(),
                        );
                        let hp = tracer
                            .health(w.node)
                            .map(|(c, m)| format!("hp {}/{}", number(c), number(m)));
                        let state = if w.alive {
                            hp.unwrap_or_default()
                        } else {
                            "freed".to_owned()
                        };
                        ui.label(
                            RichText::new(state)
                                .monospace()
                                .small()
                                .color(theme::TEXT_DIM),
                        );
                        if ui
                            .small_button(icon::X)
                            .on_hover_text("Stop watching")
                            .clicked()
                        {
                            watch = Some((w.node, false));
                        }
                        ui.end_row();
                    }
                });
        }
        ui.add_space(4.0);
        let wanted = filter(ui, "object_filter");
        egui::ScrollArea::vertical()
            .id_salt("object_log")
            .max_height(220.0)
            .show(ui, |ui| {
                for e in tracer.objects().rev().take(300) {
                    if !wanted.is_empty() && !e.name.to_lowercase().contains(&wanted) {
                        continue;
                    }
                    let color = match e.kind {
                        ObjectEventKind::Spawned => theme::PLAY,
                        ObjectEventKind::Freed => theme::ERROR,
                        ObjectEventKind::Signal { .. } => ui.visuals().text_color(),
                        ObjectEventKind::Note(_) => theme::WARN,
                    };
                    let response = ui.add(
                        egui::Label::new(
                            RichText::new(e.describe()).monospace().small().color(color),
                        )
                        .sense(egui::Sense::click()),
                    );
                    if response.clicked() && editor.tree().is_valid(e.node) {
                        select = Some(e.node);
                    }
                }
            });
    });
    if let Some((node, on)) = watch {
        editor.tree_mut().trace_watch(node, on);
    }
    if let Some(node) = select {
        editor.select(Some(node));
    }
}

fn ragdolls(ui: &mut egui::Ui, editor: &mut Editor) {
    let tree = editor.tree();
    let dolls: Vec<NodeHandle> = tree
        .subtree(tree.root())
        .into_iter()
        .filter(|&n| tree.component::<Ragdoll>(n).is_some())
        .collect();
    section(
        ui,
        "debug_ragdolls",
        icon::PERSON_ARMS_SPREAD,
        "Ragdolls",
        |ui| {
            if dolls.is_empty() {
                ui.label(
                    RichText::new(
                        "No ragdolls. Create → Physics → Ragdoll adds a jointed mannequin.",
                    )
                    .color(theme::TEXT_DIM),
                );
                return;
            }
            let playing = editor.is_playing();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        playing,
                        egui::Button::new(format!("{}  Go limp", icon::ARROW_FAT_DOWN)),
                    )
                    .on_hover_text("Activate every ragdoll")
                    .clicked()
                {
                    for &doll in &dolls {
                        let _ = ab_engine::ragdoll::go_limp(editor.tree_mut(), doll, None);
                    }
                }
                if ui
                    .add_enabled(
                        playing,
                        egui::Button::new(format!("{}  Kick", icon::LIGHTNING)),
                    )
                    .on_hover_text("Push every ragdoll upward with a random spin")
                    .clicked()
                {
                    for (i, &doll) in dolls.iter().enumerate() {
                        let tree = editor.tree_mut();
                        let _ = ab_engine::ragdoll::go_limp(tree, doll, None);
                        if let Some(at) = tree.global_position(doll) {
                            let side = if i % 2 == 0 { 1.0 } else { -1.0 };
                            tree.apply_ragdoll_impulse(
                                doll,
                                Vec3::new(18.0 * side, 55.0, -12.0),
                                at + Vec3::new(0.0, 1.3, 0.0),
                            );
                        }
                    }
                }
            });
            if !playing {
                ui.label(
                    RichText::new("Play (F5) to test them")
                        .small()
                        .color(theme::TEXT_DIM),
                );
            }
            for &doll in &dolls {
                let tree = editor.tree();
                let name = tree.name(doll).map(|n| n.to_owned()).unwrap_or_default();
                let active = tree.is_ragdoll_active(doll);
                let parts = tree.ragdoll_parts(doll).len();
                ui.label(format!(
                    "{}  {name} · {parts} parts · {}",
                    icon::PERSON_ARMS_SPREAD,
                    if active { "limp" } else { "kinematic" }
                ));
            }
        },
    );
}

fn counters(ui: &mut egui::Ui, editor: &mut Editor) {
    section(ui, "debug_counters", icon::LIST_NUMBERS, "Counters", |ui| {
        let tree = editor.tree();
        let world = tree.world();
        let (bodies, colliders, joints) = world.resource::<PhysicsWorld>().map_or((0, 0, 0), |p| {
            (p.body_count(), p.collider_count(), p.joint_count())
        });
        let (meshes, materials) = world
            .resource::<RenderAssets>()
            .map_or((0, 0), |a| (a.mesh_count(), a.material_count()));
        let lines = world.resource::<DebugDraw>().map_or(0, |d| d.line_count());
        let render = world
            .resource::<RenderStats>()
            .map(|r| r.0)
            .unwrap_or_default();
        grid(ui, "counters_grid", |ui| {
            for (name, value) in [
                ("Nodes", tree.node_count()),
                ("Entities", world.entity_count()),
                ("Physics bodies", bodies),
                ("Colliders", colliders),
                ("Joints", joints),
                ("Meshes", meshes),
                ("Materials", materials),
                ("Debug lines", lines),
                ("Particles drawn", render.particles as usize),
            ] {
                label(ui, name);
                ui.label(RichText::new(value.to_string()).monospace());
                ui.end_row();
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_engine::trace::Tracing;

    fn draw(editor: &mut Editor) {
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| show(ui, editor));
            });
            output.textures_delta.clear();
        }
    }

    #[test]
    fn traces_show_while_playing_and_restart_with_each_play() {
        let assets = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let mut editor = Editor::new(assets, None, ab_engine::audio::AudioOutput::Silent);
        editor.select(None);
        editor.add_scene(crate::create::PHYSICS[0].scene(), None);
        editor.select(None);
        editor.add_scene(crate::create::PHYSICS[1].scene(), None);
        draw(&mut editor);
        editor.play();
        editor.tick(1.0 / 60.0);
        let nodes = editor.tree().children(editor.scene_root);
        let (wall, crate_) = (nodes[nodes.len() - 2], nodes[nodes.len() - 1]);
        let tree = editor.tree_mut();
        tree.deal_damage(Some(wall), crate_, 5.0);
        tree.trace_watch(crate_, true);
        for _ in 0..20 {
            editor.tick(1.0 / 60.0);
        }
        draw(&mut editor);
        {
            let tracer = editor.tree().world().resource::<Tracer>().unwrap();
            let hit = tracer.damage().last().unwrap();
            // No health script on a crate: nobody answered.
            assert_eq!(hit.outcome, ab_engine::trace::Outcome::Ignored);
            assert_eq!(
                (hit.source_name.as_str(), hit.target_name.as_str()),
                ("Wall", "Crate")
            );
            assert!(tracer.watches()[0].trail.len() > 1, "the crate falls");
        }
        editor.stop();
        editor.play();
        let tracer = editor.tree().world().resource::<Tracer>().unwrap();
        assert_eq!(tracer.damage().len(), 0);
        assert!(tracer.watches().is_empty());
    }
}
