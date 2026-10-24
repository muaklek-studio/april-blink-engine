//! Engine and game log, filterable by level and text.

use crate::console::{LogBuffer, LogLine};
use crate::i18n::t;
use crate::theme;
use eframe::egui::{self, Color32, RichText};
use egui_phosphor::regular as icon;
use tracing::Level;

pub struct ConsolePanel {
    logs: LogBuffer,
    filter: String,
    errors: bool,
    warnings: bool,
    info: bool,
    debug: bool,
    follow: bool,
}

impl ConsolePanel {
    pub fn new(logs: LogBuffer) -> Self {
        Self {
            logs,
            filter: String::new(),
            errors: true,
            warnings: true,
            info: true,
            debug: false,
            follow: true,
        }
    }

    /// Errors and warnings logged so far.
    pub fn counts(&self) -> (usize, usize) {
        let lines = self.logs.lock();
        let errors = lines.iter().filter(|l| l.level == Level::ERROR).count();
        let warnings = lines.iter().filter(|l| l.level == Level::WARN).count();
        (errors, warnings)
    }

    fn shown(&self, line: &LogLine, filter: &str) -> bool {
        let level = match line.level {
            Level::ERROR => self.errors,
            Level::WARN => self.warnings,
            Level::INFO => self.info,
            _ => self.debug,
        };
        level
            && (filter.is_empty()
                || line.message.to_lowercase().contains(filter)
                || line.target.to_lowercase().contains(filter))
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        let (errors, warnings) = self.counts();
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  {}", icon::TRASH, t("console.clear")))
                .on_hover_text(t("console.clear_hint"))
                .clicked()
            {
                self.logs.clear();
            }
            ui.separator();
            ui.toggle_value(
                &mut self.errors,
                RichText::new(format!("{}  {errors}", icon::X_CIRCLE)).color(theme::ERROR),
            )
            .on_hover_text(t("console.errors"));
            ui.toggle_value(
                &mut self.warnings,
                RichText::new(format!("{}  {warnings}", icon::WARNING)).color(theme::WARN),
            )
            .on_hover_text(t("console.warnings"));
            ui.toggle_value(
                &mut self.info,
                format!("{}  {}", icon::INFO, t("console.info")),
            );
            ui.toggle_value(
                &mut self.debug,
                format!("{}  {}", icon::BUG, t("console.debug")),
            );
            ui.separator();
            ui.toggle_value(
                &mut self.follow,
                format!("{}  {}", icon::ARROW_LINE_DOWN, t("console.follow")),
            )
            .on_hover_text(t("console.follow_hint"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text(format!(
                            "{}  {}",
                            icon::MAGNIFYING_GLASS,
                            t("console.filter")
                        ))
                        .desired_width(240.0),
                );
            });
        });
        ui.separator();

        let filter = self.filter.trim().to_lowercase();
        let lines: Vec<LogLine> = self
            .logs
            .lock()
            .iter()
            .filter(|l| self.shown(l, &filter))
            .cloned()
            .collect();
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .stick_to_bottom(self.follow)
            .show_rows(ui, row_height, lines.len(), |ui, range| {
                for line in &lines[range] {
                    let (glyph, color) = match line.level {
                        Level::ERROR => (icon::X_CIRCLE, theme::ERROR),
                        Level::WARN => (icon::WARNING, theme::WARN),
                        Level::INFO => (icon::INFO, theme::ACCENT),
                        _ => (icon::BUG, theme::TEXT_DIM),
                    };
                    let text_color = match line.level {
                        Level::ERROR => theme::ERROR,
                        Level::WARN => theme::WARN,
                        _ => Color32::from_gray(205),
                    };
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.label(
                            RichText::new(format!("{:>8.2}", line.time))
                                .monospace()
                                .color(theme::TEXT_DIM),
                        );
                        ui.label(RichText::new(glyph).color(color));
                        let response = ui.add(
                            egui::Label::new(
                                RichText::new(&line.message).monospace().color(text_color),
                            )
                            .extend()
                            .sense(egui::Sense::click()),
                        );
                        response.on_hover_text(&line.target).context_menu(|ui| {
                            if ui
                                .button(format!("{}  {}", icon::COPY, t("command.copy")))
                                .clicked()
                            {
                                ui.ctx().copy_text(line.message.clone());
                                ui.close();
                            }
                        });
                    });
                }
            });
    }
}
