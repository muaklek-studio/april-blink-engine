//! Frame-time graph and tables of the engine's sections, systems and scripts.

use crate::i18n::{t, tf};
use crate::state::Editor;
use crate::theme;
use ab_engine::{FrameSample, Profiler};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Stroke, Vec2};
use egui_phosphor::regular as icon;

/// 60 fps budget.
const BUDGET_MS: f32 = 1000.0 / 60.0;

/// (text key, colour) of each part of a frame.
const SERIES: [(&str, Color32); 4] = [
    ("profiler.update", Color32::from_rgb(90, 150, 255)),
    ("profiler.physics", Color32::from_rgb(255, 150, 60)),
    ("profiler.scripts", Color32::from_rgb(90, 210, 130)),
    ("profiler.render", Color32::from_rgb(190, 130, 255)),
];

fn series(sample: &FrameSample, index: usize) -> f32 {
    match index {
        0 => sample.update_ms,
        1 => sample.physics_ms,
        2 => sample.scripts_ms,
        _ => sample.render_ms,
    }
}

#[derive(Default)]
pub struct ProfilerPanel {
    filter: String,
}

impl ProfilerPanel {
    pub fn show(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        let world = editor.tree().world();
        let Some(mut profiler) = world.resource_mut::<Profiler>() else {
            return;
        };
        let stats = profiler.frame_statistics();
        ui.horizontal(|ui| {
            let (glyph, tip) = if profiler.paused {
                (icon::PLAY, t("profiler.resume"))
            } else {
                (icon::PAUSE, t("profiler.freeze"))
            };
            if ui.button(glyph).on_hover_text(tip).clicked() {
                profiler.paused = !profiler.paused;
            }
            if ui
                .button(icon::ARROW_COUNTER_CLOCKWISE)
                .on_hover_text(t("profiler.clear"))
                .clicked()
            {
                profiler.reset();
            }
            ui.separator();
            let fps_color = if stats.fps >= 55.0 {
                theme::PLAY
            } else if stats.fps >= 30.0 {
                theme::WARN
            } else {
                theme::ERROR
            };
            ui.label(
                RichText::new(format!("{:.0} FPS", stats.fps))
                    .strong()
                    .color(fps_color),
            );
            ui.label(
                RichText::new(tf(
                    "profiler.frame_stats",
                    &[
                        ("average", &format!("{:.2}", stats.average_ms)),
                        ("min", &format!("{:.1}", stats.min_ms)),
                        ("max", &format!("{:.1}", stats.max_ms)),
                    ],
                ))
                .monospace()
                .color(theme::TEXT_DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text(format!(
                            "{}  {}",
                            icon::MAGNIFYING_GLASS,
                            t("console.filter")
                        ))
                        .desired_width(150.0),
                );
            });
        });
        if !editor.is_playing() && profiler.history().is_empty() {
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("{}  {}", icon::INFO, t("profiler.idle")))
                    .color(theme::TEXT_DIM),
            );
        }
        ui.add_space(2.0);
        graph(
            ui,
            profiler
                .history()
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .as_slice(),
        );
        ui.add_space(4.0);

        let filter = self.filter.trim().to_lowercase();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // (id, text key, icon).
                let groups = [
                    ("Frame", "profiler.frame", icon::TIMER),
                    ("Scripts", "profiler.scripts", icon::SCROLL),
                    ("Systems", "profiler.systems", icon::GEAR_SIX),
                    ("Custom", "profiler.custom", icon::TAG),
                ];
                for (index, (id, key, glyph)) in groups.into_iter().enumerate() {
                    let mut rows: Vec<_> = profiler
                        .entries()
                        .iter()
                        .filter(|e| group_of(&e.name) == index)
                        .filter(|e| filter.is_empty() || e.name.to_lowercase().contains(&filter))
                        .cloned()
                        .collect();
                    if rows.is_empty() {
                        continue;
                    }
                    rows.sort_by(|a, b| b.average_ms.total_cmp(&a.average_ms));
                    super::section(ui, &format!("profile_{id}"), glyph, &t(key), |ui| {
                        table(ui, id, &rows);
                    });
                }
            });
    }
}

/// Which table an entry goes in: engine sections, scripts, systems, the game's own.
fn group_of(name: &str) -> usize {
    if name.starts_with("script.") {
        1
    } else if name.starts_with("system.") {
        2
    } else if !name.contains('.') || name.starts_with("scripts.") {
        0
    } else {
        3
    }
}

fn table(ui: &mut egui::Ui, id: &str, rows: &[ab_engine::ProfileEntry]) {
    egui::Grid::new(id)
        .num_columns(4)
        .spacing([10.0, 2.0])
        .striped(true)
        .show(ui, |ui| {
            for (label, align) in [
                ("profiler.name", egui::Align::Min),
                ("profiler.average", egui::Align::Max),
                ("profiler.peak", egui::Align::Max),
                ("profiler.share", egui::Align::Min),
            ] {
                ui.with_layout(
                    egui::Layout::left_to_right(egui::Align::Center).with_main_align(align),
                    |ui| {
                        ui.label(RichText::new(t(label)).small().color(theme::TEXT_DIM));
                    },
                );
            }
            ui.end_row();
            for row in rows {
                let name = row
                    .name
                    .trim_start_matches("system.")
                    .trim_start_matches("script.");
                ui.label(name);
                ui.label(RichText::new(format!("{:.3}", row.average_ms)).monospace());
                let peak_color = if row.peak_ms > BUDGET_MS {
                    theme::ERROR
                } else if row.peak_ms > BUDGET_MS * 0.5 {
                    theme::WARN
                } else {
                    theme::TEXT_DIM
                };
                ui.label(
                    RichText::new(format!("{:.3}", row.peak_ms))
                        .monospace()
                        .color(peak_color),
                );
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(120.0, 10.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 2.0, theme::BG_3);
                let share = (row.average_ms / BUDGET_MS).clamp(0.0, 1.0);
                let color = if share > 0.5 {
                    theme::WARN
                } else {
                    theme::ACCENT
                };
                ui.painter().rect_filled(
                    Rect::from_min_size(rect.min, Vec2::new(rect.width() * share, rect.height())),
                    2.0,
                    color,
                );
                ui.end_row();
            }
        });
}

/// Frame-time lines with 60/30 fps guides and a hover read-out.
fn graph(ui: &mut egui::Ui, samples: &[FrameSample]) {
    let height = 96.0;
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, theme::BG_0);
    let max_ms = samples
        .iter()
        .map(|s| s.delta_ms.max(s.update_ms + s.render_ms))
        .fold(BUDGET_MS * 2.2, f32::max)
        .min(250.0);
    let y = |ms: f32| rect.bottom() - (ms / max_ms).clamp(0.0, 1.0) * (rect.height() - 6.0);
    for (ms, label) in [(BUDGET_MS, "60 fps"), (BUDGET_MS * 2.0, "30 fps")] {
        let line_y = y(ms);
        painter.hline(rect.x_range(), line_y, Stroke::new(1.0_f32, theme::BORDER));
        painter.text(
            Pos2::new(rect.right() - 4.0, line_y - 1.0),
            egui::Align2::RIGHT_BOTTOM,
            label,
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
    }
    if samples.len() < 2 {
        return;
    }
    let step = rect.width() / (ab_engine::PROFILE_HISTORY as f32 - 1.0);
    let x = |i: usize| rect.right() - (samples.len() - 1 - i) as f32 * step;
    // Wall-clock frame time as a filled area, the parts as lines.
    let mut area: Vec<Pos2> = samples
        .iter()
        .enumerate()
        .map(|(i, s)| Pos2::new(x(i), y(s.delta_ms)))
        .collect();
    area.push(Pos2::new(x(samples.len() - 1), rect.bottom()));
    area.push(Pos2::new(x(0), rect.bottom()));
    for pair in area[..samples.len()].windows(2) {
        painter.add(egui::Shape::convex_polygon(
            vec![
                pair[0],
                pair[1],
                Pos2::new(pair[1].x, rect.bottom()),
                Pos2::new(pair[0].x, rect.bottom()),
            ],
            Color32::from_rgba_unmultiplied(255, 255, 255, 14),
            Stroke::NONE,
        ));
    }
    painter.add(egui::Shape::line(
        area[..samples.len()].to_vec(),
        Stroke::new(1.0_f32, Color32::from_gray(150)),
    ));
    for (index, (_, color)) in SERIES.iter().enumerate() {
        let points: Vec<Pos2> = samples
            .iter()
            .enumerate()
            .map(|(i, s)| Pos2::new(x(i), y(series(s, index))))
            .collect();
        painter.add(egui::Shape::line(points, Stroke::new(1.5_f32, *color)));
    }

    // Legend.
    let mut at = rect.left_top() + Vec2::new(8.0, 6.0);
    for (key, color) in std::iter::once(("profiler.frame", Color32::from_gray(150))).chain(SERIES) {
        painter.rect_filled(
            Rect::from_min_size(at + Vec2::new(0.0, 3.0), Vec2::splat(7.0)),
            1.0,
            color,
        );
        let galley =
            painter.layout_no_wrap(t(key), egui::FontId::proportional(10.5), theme::TEXT_DIM);
        let width = galley.size().x;
        painter.galley(at + Vec2::new(10.0, 0.0), galley, Color32::WHITE);
        at.x += width + 22.0;
    }

    // Hover read-out.
    if let Some(pointer) = response.hover_pos() {
        let index = ((pointer.x - x(0)) / step).round();
        if index >= 0.0 && (index as usize) < samples.len() {
            let i = index as usize;
            let sample = samples[i];
            painter.vline(
                x(i),
                rect.y_range(),
                Stroke::new(1.0_f32, Color32::from_white_alpha(60)),
            );
            let ms = |v: f32| format!("{v:.2}");
            let text = tf(
                "profiler.readout",
                &[
                    ("frame", &ms(sample.delta_ms)),
                    ("update", &ms(sample.update_ms)),
                    ("physics", &ms(sample.physics_ms)),
                    ("scripts", &ms(sample.scripts_ms)),
                    ("render", &ms(sample.render_ms)),
                ],
            );
            response.on_hover_text_at_pointer(RichText::new(text).monospace());
        }
    }
}
