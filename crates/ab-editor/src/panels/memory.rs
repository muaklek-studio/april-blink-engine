//! The profiler's memory view (see [`ab_engine::memory`]): the heap, the engine's assets
//! and the GPU now, graphs of the last minutes, what keeps growing, and a baseline to
//! compare against.

use crate::i18n::t;
use crate::state::Editor;
use crate::theme;
use ab_engine::core::memory::human_bytes;
use ab_engine::memory::{Category, MemoryProfiler, MemorySample};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Stroke, Vec2};
use egui_phosphor::regular as icon;

const HEAP: Color32 = Color32::from_rgb(90, 150, 255);
const ASSETS: Color32 = Color32::from_rgb(90, 210, 130);
const GPU: Color32 = Color32::from_rgb(190, 130, 255);

fn bytes(v: u64) -> String {
    human_bytes(v)
}

/// A change since the baseline, signed.
fn delta(now: u64, then: u64) -> String {
    match now.cmp(&then) {
        std::cmp::Ordering::Greater => format!("+{}", bytes(now - then)),
        std::cmp::Ordering::Less => format!("−{}", bytes(then - now)),
        std::cmp::Ordering::Equal => "—".into(),
    }
}

pub fn show(ui: &mut egui::Ui, editor: &mut Editor) {
    // Sampled here while editing (the engine samples while playing).
    ab_engine::memory::update(editor.tree());
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(500));
    let world = editor.tree().world();
    let Some(mut profiler) = world.resource_mut::<MemoryProfiler>() else {
        return;
    };
    let Some(now) = profiler.latest().cloned() else {
        return;
    };
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
            .button(format!("{}  {}", icon::FLAG, t("memory.baseline")))
            .on_hover_text(t("memory.baseline_hint"))
            .clicked()
        {
            profiler.take_baseline();
        }
        if profiler.baseline.is_some() && ui.button(t("memory.clear_baseline")).clicked() {
            profiler.baseline = None;
        }
        ui.separator();
        if now.heap.installed {
            ui.label(
                RichText::new(format!(
                    "{}  {}",
                    t("memory.heap"),
                    bytes(now.heap.live as u64)
                ))
                .strong()
                .color(HEAP),
            );
            ui.label(
                RichText::new(format!(
                    "{} {} · {:.0}/s",
                    t("memory.peak"),
                    bytes(now.heap.peak as u64),
                    profiler.allocations_per_second()
                ))
                .monospace()
                .color(theme::TEXT_DIM),
            );
        } else {
            ui.label(RichText::new(t("memory.no_heap")).color(theme::TEXT_DIM));
        }
    });
    let warnings = profiler.warnings();
    for w in &warnings {
        let rate = if w.nodes {
            format!("{:.0} nodes", w.per_minute)
        } else {
            bytes(w.per_minute.max(0.0) as u64)
        };
        ui.label(
            RichText::new(format!(
                "{}  {} {}",
                icon::WARNING,
                w.what,
                crate::i18n::tf("memory.growing", &[("rate", &rate)])
            ))
            .color(theme::WARN),
        );
    }
    ui.add_space(4.0);
    graph(ui, &profiler.history.iter().cloned().collect::<Vec<_>>());
    ui.add_space(4.0);
    let baseline = profiler.baseline.clone();
    drop(profiler);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            table(ui, &now, baseline.as_ref());
        });
}

fn table(ui: &mut egui::Ui, now: &MemorySample, baseline: Option<&MemorySample>) {
    egui::Grid::new("memory_table")
        .striped(true)
        .num_columns(4)
        .spacing([18.0, 4.0])
        .show(ui, |ui| {
            for head in ["memory.what", "memory.count", "memory.bytes"] {
                ui.label(RichText::new(t(head)).color(theme::TEXT_DIM));
            }
            ui.label(
                RichText::new(if baseline.is_some() {
                    t("memory.since")
                } else {
                    String::new()
                })
                .color(theme::TEXT_DIM),
            );
            ui.end_row();
            let row =
                |ui: &mut egui::Ui, name: String, count: String, value: u64, then: Option<u64>| {
                    ui.label(name);
                    ui.label(RichText::new(count).monospace());
                    ui.label(RichText::new(bytes(value)).monospace());
                    match then {
                        Some(then) => {
                            let color = if value > then {
                                theme::WARN
                            } else {
                                theme::TEXT_DIM
                            };
                            ui.label(RichText::new(delta(value, then)).monospace().color(color));
                        }
                        None => {
                            ui.label("");
                        }
                    }
                    ui.end_row();
                };
            if now.heap.installed {
                row(
                    ui,
                    format!("{}  {}", icon::MEMORY, t("memory.heap")),
                    format!("{} live", now.heap.outstanding()),
                    now.heap.live as u64,
                    baseline.map(|b| b.heap.live as u64),
                );
            }
            for category in Category::ALL {
                let (b, count) = now.category(category);
                row(
                    ui,
                    format!("{}  {}", icon::PACKAGE, category.name()),
                    count.to_string(),
                    b,
                    baseline.map(|s| s.category(category).0),
                );
            }
            row(
                ui,
                format!("{}  {}", icon::STACK, t("memory.assets_total")),
                format!("{} models", now.models),
                now.asset_bytes(),
                baseline.map(MemorySample::asset_bytes),
            );
            row(
                ui,
                format!("{}  GPU textures", icon::IMAGE),
                String::new(),
                now.gpu.textures,
                baseline.map(|b| b.gpu.textures),
            );
            row(
                ui,
                format!("{}  GPU meshes", icon::CUBE),
                String::new(),
                now.gpu.meshes,
                baseline.map(|b| b.gpu.meshes),
            );
            row(
                ui,
                format!("{}  GPU targets and buffers", icon::MONITOR),
                String::new(),
                now.gpu.other,
                baseline.map(|b| b.gpu.other),
            );
            ui.label(format!("{}  {}", icon::TREE_STRUCTURE, t("memory.nodes")));
            ui.label(
                RichText::new(format!("{} nodes · {} entities", now.nodes, now.entities))
                    .monospace(),
            );
            ui.label("");
            match baseline {
                Some(b) => {
                    let d = now.nodes as i64 - b.nodes as i64;
                    ui.label(RichText::new(format!("{d:+}")).monospace().color(if d > 0 {
                        theme::WARN
                    } else {
                        theme::TEXT_DIM
                    }));
                }
                None => {
                    ui.label("");
                }
            }
            ui.end_row();
        });
}

/// Heap, assets and GPU over the samples kept.
fn graph(ui: &mut egui::Ui, samples: &[MemorySample]) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 120.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, theme::BG_1);
    if samples.len() < 2 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            t("memory.collecting"),
            egui::FontId::proportional(12.0),
            theme::TEXT_DIM,
        );
        return;
    }
    let value = |s: &MemorySample, i: usize| -> f32 {
        match i {
            0 => s.heap.live as f32,
            1 => s.asset_bytes() as f32,
            _ => s.gpu.total() as f32,
        }
    };
    let max = samples
        .iter()
        .flat_map(|s| (0..3).map(move |i| value(s, i)))
        .fold(1.0_f32, f32::max)
        * 1.1;
    let n = samples.len();
    for (i, color) in [HEAP, ASSETS, GPU].into_iter().enumerate() {
        let points: Vec<Pos2> = samples
            .iter()
            .enumerate()
            .map(|(k, s)| {
                let x = rect.left() + rect.width() * k as f32 / (n - 1) as f32;
                let y = rect.bottom() - rect.height() * value(s, i) / max;
                Pos2::new(x, y)
            })
            .collect();
        painter.add(egui::Shape::line(points, Stroke::new(1.5_f32, color)));
    }
    let legend = [
        (HEAP, t("memory.heap")),
        (ASSETS, t("memory.assets_total")),
        (GPU, "GPU".to_owned()),
    ];
    let mut x = rect.left() + 8.0;
    for (color, name) in legend {
        painter.circle_filled(Pos2::new(x + 4.0, rect.top() + 10.0), 4.0, color);
        let galley = painter.layout_no_wrap(name, egui::FontId::proportional(11.0), theme::TEXT);
        let w = galley.size().x;
        painter.galley(Pos2::new(x + 12.0, rect.top() + 3.0), galley, theme::TEXT);
        x += w + 26.0;
    }
    painter.text(
        Rect::from_min_max(rect.min, rect.max).right_top() + Vec2::new(-6.0, 4.0),
        egui::Align2::RIGHT_TOP,
        bytes(max as u64),
        egui::FontId::monospace(10.0),
        theme::TEXT_DIM,
    );
}
