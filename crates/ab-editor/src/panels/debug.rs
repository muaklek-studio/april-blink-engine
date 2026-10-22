//! Debug tools: time control, debug views, ragdoll testing and live counters.

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
            RichText::new("Physics colours: orange simulated · cyan kinematic · green static · violet areas · yellow joints · red contacts")
                .small()
                .color(theme::TEXT_DIM),
        );
    });
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
