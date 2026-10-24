//! Tool windows: editor tools in native windows of their own, next to the main one (another
//! monitor, say), each with room for its own tool bar and side panels.
//!
//! Every tool window has the editor's frame (title bar with a Window menu, resize edges) or
//! the system's (`--system-titlebar`), and a status bar with the editor's last message and
//! mode. The editor's shortcuts work in them too: the keys pressed in a tool window are
//! handed back to the main window ([`Host::commands`]), which runs them; Ctrl+W closes
//! the tool window. Each window opens where it was last closed ([`Host::rects`]).
//!
//! The windows are immediate viewports: they draw in the main window's frame and borrow
//! the editor's state directly. Where the platform has a single window, they show as
//! windows inside the main one. A minimized or hidden window draws nothing, and views of
//! the scene in a window that is not being used redraw less often ([`Redraw`]).

use crate::chrome;
use crate::commands::{self, Command, Focus};
use crate::i18n::t;
use crate::state::{Editor, Mode};
use crate::theme;
use eframe::egui::{self, KeyboardShortcut, RichText, ViewportCommand, ViewportId};
use egui_phosphor::regular as icon;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Closes the tool window that has the keyboard.
pub const CLOSE: KeyboardShortcut = KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::W);

/// How a tool window looks and starts.
#[derive(Debug, Clone)]
pub struct Spec {
    /// Stable name: the window's identity.
    pub key: String,
    pub icon: &'static str,
    pub title: String,
    pub size: [f32; 2],
    pub min_size: [f32; 2],
    /// Offer to put the tool back into the main window.
    pub dockable: bool,
    /// Open where it was last closed (the place is kept under `key`).
    pub remember: bool,
    /// Esc closes it (a preview), before the editor's own use of Esc.
    pub escape_closes: bool,
}

/// What was asked of a tool window this frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Response {
    /// Closed (its close button, Ctrl+W, the Window menu, or the system).
    pub close: bool,
    /// Window → Dock in Main Window.
    pub dock: bool,
}

/// A window's place on the desktop: outer position and inner size, in points.
pub type Placement = [f32; 4];

/// What every tool window shows in its frame, and what they hand back to the main window.
#[derive(Debug)]
pub struct Host {
    pub custom_frame: bool,
    project: String,
    message: Option<(String, bool)>,
    mode: Mode,
    modified: bool,
    /// Where each window opens: fixed while it is open (a changing builder would move it).
    placements: BTreeMap<String, Placement>,
    /// Shortcuts pressed in the tool windows, for the main window to run.
    pub commands: Vec<(Command, Option<String>)>,
    /// A tool window has the pointer down or a text field focused: an edit made there is
    /// still going on (one undo step per gesture).
    pub busy: bool,
    /// Where the remembered windows are now, to keep.
    pub rects: Vec<(String, Placement)>,
}

/// The viewport of the tool window named `key`.
pub fn viewport_id(key: &str) -> ViewportId {
    ViewportId::from_hash_of(("ab-tool-window", key))
}

/// Raises the tool window named `key` (one already open, asked for again).
pub fn focus(ctx: &egui::Context, key: &str) {
    ctx.send_viewport_cmd_to(viewport_id(key), ViewportCommand::Focus);
}

/// A placement worth restoring: a real size, on a plausible desktop.
pub fn valid(rect: &Placement, min_size: [f32; 2]) -> bool {
    rect.iter().all(|v| v.is_finite())
        && rect[2] >= min_size[0]
        && rect[3] >= min_size[1]
        && rect[2] <= 16384.0
        && rect[3] <= 16384.0
        && rect[0].abs() <= 32768.0
        && rect[1].abs() <= 32768.0
}

impl Host {
    pub fn new(
        editor: &Editor,
        custom_frame: bool,
        placements: BTreeMap<String, Placement>,
    ) -> Self {
        Self {
            custom_frame,
            project: chrome::project_name(&editor.assets.root),
            message: editor.status.as_ref().map(|s| (s.text.clone(), s.error)),
            mode: editor.mode,
            modified: editor.modified,
            placements,
            commands: Vec::new(),
            busy: false,
            rects: Vec::new(),
        }
    }

    /// Shows the tool window `spec`, `body` filling it below its title bar.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        spec: &Spec,
        body: impl FnOnce(&mut egui::Ui),
    ) -> Response {
        let id = viewport_id(&spec.key);
        let mut builder = egui::ViewportBuilder::default()
            .with_title(format!("{} — {}", spec.title, self.project))
            .with_app_id("april-blink-editor")
            .with_inner_size(spec.size)
            .with_min_inner_size(spec.min_size)
            .with_decorations(!self.custom_frame);
        if let Some(rect) = self
            .placements
            .get(&spec.key)
            .filter(|r| spec.remember && valid(r, spec.min_size))
        {
            builder = builder
                .with_position([rect[0], rect[1]])
                .with_inner_size([rect[2], rect[3]]);
        }
        let mut body = Some(body);
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            let mut response = Response::default();
            let ctx = ui.ctx().clone();
            if class == egui::ViewportClass::EmbeddedWindow {
                // No native windows here: the tool shows as a window inside the main one.
                if let Some(body) = body.take() {
                    body(ui);
                }
                return response;
            }
            let (close, hidden, outer, inner) = ctx.input(|i| {
                let v = i.viewport();
                (
                    v.close_requested(),
                    v.minimized.unwrap_or(false) || v.occluded.unwrap_or(false),
                    v.outer_rect,
                    v.inner_rect,
                )
            });
            response.close = close;
            if spec.remember {
                if let (Some(outer), Some(inner)) = (outer, inner) {
                    self.rects.push((
                        spec.key.clone(),
                        [outer.min.x, outer.min.y, inner.width(), inner.height()],
                    ));
                }
            }
            if hidden {
                // Nobody sees it: no layout, no rendering.
                return response;
            }
            let typing = ctx.egui_wants_keyboard_input();
            if ctx.input_mut(|i| i.consume_shortcut(&CLOSE)) {
                response.close = true;
            }
            if spec.escape_closes
                && !typing
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            {
                response.close = true;
            }
            let focus = Focus {
                game: false,
                text: typing,
            };
            self.commands.extend(commands::pressed(&ctx, focus));
            self.title_bar(ui, spec, &mut response);
            self.status_bar(ui, spec);
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(theme::BG_1))
                .show(ui, |ui| {
                    if let Some(body) = body.take() {
                        body(ui);
                    }
                });
            if self.custom_frame {
                chrome::window_frame(&ctx);
            }
            self.busy |= ctx.input(|i| i.pointer.any_down()) || ctx.egui_wants_keyboard_input();
            response
        })
    }

    fn title_bar(&self, ui: &mut egui::Ui, spec: &Spec, response: &mut Response) {
        let menus = |ui: &mut egui::Ui| {
            ui.label(RichText::new(spec.icon).color(theme::ACCENT));
            ui.menu_button(t("window.menu"), |ui| {
                ui.set_min_width(220.0);
                if spec.dockable
                    && ui
                        .button(format!("{}  {}", icon::ARROW_SQUARE_IN, t("window.dock")))
                        .clicked()
                {
                    response.dock = true;
                    ui.close();
                }
                let on_top_id = egui::Id::new(("ab-tool-window-on-top", &spec.key));
                let mut on_top = ui
                    .ctx()
                    .data(|d| d.get_temp::<bool>(on_top_id))
                    .unwrap_or(false);
                if ui
                    .checkbox(
                        &mut on_top,
                        format!("{}  {}", icon::PUSH_PIN, t("window.on_top")),
                    )
                    .changed()
                {
                    ui.ctx().data_mut(|d| d.insert_temp(on_top_id, on_top));
                    ui.ctx()
                        .send_viewport_cmd(ViewportCommand::WindowLevel(if on_top {
                            egui::WindowLevel::AlwaysOnTop
                        } else {
                            egui::WindowLevel::Normal
                        }));
                }
                ui.separator();
                let keys = ui.ctx().format_shortcut(&CLOSE);
                if ui
                    .add(
                        egui::Button::new(format!("{}  {}", icon::X, t("window.close")))
                            .shortcut_text(keys),
                    )
                    .clicked()
                {
                    response.close = true;
                    ui.close();
                }
            });
        };
        let frame = egui::Frame::NONE.fill(theme::BG_0);
        if self.custom_frame {
            let title = chrome::Title {
                document: &spec.title,
                modified: self.modified,
                project: Some(&self.project),
                playing: self.mode != Mode::Edit,
            };
            egui::Panel::top(egui::Id::new(("ab-tool-window-title", &spec.key)))
                .exact_size(chrome::HEIGHT)
                .frame(frame.inner_margin(egui::Margin {
                    left: 8,
                    ..Default::default()
                }))
                .show(ui, |ui| chrome::title_bar(ui, &title, menus));
        } else {
            egui::Panel::top(egui::Id::new(("ab-tool-window-menu", &spec.key)))
                .frame(frame.inner_margin(egui::Margin::symmetric(8, 2)))
                .show(ui, |ui| egui::MenuBar::new().ui(ui, menus));
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui, spec: &Spec) {
        egui::Panel::bottom(egui::Id::new(("ab-tool-window-status", &spec.key)))
            .exact_size(22.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG_0)
                    .inner_margin(egui::Margin::symmetric(10, 0))
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    match &self.message {
                        Some((text, true)) => {
                            ui.label(
                                RichText::new(format!("{}  {text}", icon::X_CIRCLE))
                                    .color(theme::ERROR),
                            );
                        }
                        Some((text, false)) => {
                            ui.label(
                                RichText::new(format!("{}  {text}", icon::CHECK_CIRCLE))
                                    .color(theme::TEXT_DIM),
                            );
                        }
                        None => {
                            ui.label(RichText::new(t("status.ready")).color(theme::TEXT_DIM));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (mode, color) = match self.mode {
                            Mode::Edit => (t("status.edit"), theme::ACCENT),
                            Mode::Play { paused: false } => (t("status.playing"), theme::PLAY),
                            Mode::Play { paused: true } => (t("status.paused"), theme::WARN),
                        };
                        ui.label(RichText::new(mode).strong().color(color));
                        if self.modified {
                            ui.label(
                                RichText::new(format!("●  {}", t("window.unsaved")))
                                    .color(theme::ACCENT),
                            );
                        }
                    });
                });
            });
    }
}

/// A tool bar along the top of a tool window's body.
pub fn toolbar(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Panel::top(egui::Id::new(("ab-tool-window-toolbar", id)))
        .exact_size(34.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_1)
                .inner_margin(egui::Margin::symmetric(8, 3))
                .stroke(egui::Stroke::new(1.0_f32, theme::BORDER)),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                add_contents(ui);
            });
        });
}

/// A titled side panel of a tool window (`left` or right), resizable.
pub fn side(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    left: bool,
    width: f32,
    glyph: &str,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let id = egui::Id::new(("ab-tool-window-side", id));
    let panel = if left {
        egui::Panel::left(id)
    } else {
        egui::Panel::right(id)
    };
    panel
        .resizable(true)
        .default_size(width)
        .size_range(160.0..=640.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_1)
                .inner_margin(egui::Margin::same(6))
                .stroke(egui::Stroke::new(1.0_f32, theme::BORDER)),
        )
        .show(ui, |ui| {
            heading(ui, glyph, title);
            add_contents(ui);
        });
}

/// A panel's heading inside a tool window.
pub fn heading(ui: &mut egui::Ui, glyph: &str, title: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(glyph).color(theme::ACCENT));
        ui.label(RichText::new(title).strong());
    });
    ui.add_space(2.0);
    ui.separator();
}

/// Whether the window being drawn is in use: it has the keyboard or the pointer.
pub fn in_use(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().focused.unwrap_or(true) || i.pointer.has_pointer())
}

/// Paces a view of the scene in a tool window: every frame while the window is in use or
/// the scene changes, a few times a second otherwise (it still follows a game being played
/// or effects animating, at less cost).
#[derive(Debug, Default)]
pub struct Redraw {
    last: Option<Instant>,
    revision: Option<u64>,
}

/// How often a view in a window that is not in use redraws.
const BACKGROUND: Duration = Duration::from_millis(125);

impl Redraw {
    /// Whether to draw this frame: `active` (in use, or animating for someone to see),
    /// `revision` the editor's (see [`Editor::revision`]).
    pub fn due(&mut self, active: bool, revision: u64) -> bool {
        let now = Instant::now();
        let due = active
            || self.revision != Some(revision)
            || self.last.is_none_or(|last| now - last >= BACKGROUND);
        if due {
            self.last = Some(now);
            self.revision = Some(revision);
        }
        due
    }
}

/// A small image repeated over `rect` (a checkerboard, a dot grid): one quad, whatever the
/// size. `cell` is the image's size on screen, in points.
pub fn tile(
    painter: &egui::Painter,
    rect: egui::Rect,
    name: &str,
    cell: f32,
    make: impl FnOnce() -> egui::ColorImage,
) {
    let id = egui::Id::new(("ab-tile", name));
    let ctx = painter.ctx();
    let texture = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id));
    let texture = texture.unwrap_or_else(|| {
        let options = egui::TextureOptions {
            wrap_mode: egui::TextureWrapMode::Repeat,
            ..egui::TextureOptions::NEAREST
        };
        let handle = ctx.load_texture(name, make(), options);
        ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
        handle
    });
    let uv = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(rect.width() / cell, rect.height() / cell),
    );
    painter.image(texture.id(), rect, uv, egui::Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_views_redraw_on_change_or_now_and_then() {
        let mut redraw = Redraw::default();
        assert!(redraw.due(false, 1), "the first frame draws");
        assert!(
            !redraw.due(false, 1),
            "nothing changed: the image is reused"
        );
        assert!(redraw.due(false, 2), "an edit draws at once");
        assert!(redraw.due(true, 2), "in use: every frame");
        redraw.last = Some(Instant::now() - BACKGROUND);
        assert!(redraw.due(false, 2), "now and then in the background");
    }

    #[test]
    fn only_sensible_placements_are_restored() {
        assert!(valid(&[100.0, 50.0, 800.0, 600.0], [320.0, 240.0]));
        assert!(
            !valid(&[100.0, 50.0, 100.0, 600.0], [320.0, 240.0]),
            "too small"
        );
        assert!(!valid(&[f32::NAN, 50.0, 800.0, 600.0], [320.0, 240.0]));
        assert!(
            !valid(&[1e9, 50.0, 800.0, 600.0], [320.0, 240.0]),
            "off any desktop"
        );
    }
}
