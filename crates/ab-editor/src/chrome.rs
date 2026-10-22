//! The editor's own window frame: a title bar in the editor's theme (menus, document title,
//! window buttons), drag-to-move, resize edges and a hairline border. The system title bar
//! is turned off (`with_decorations(false)`); `--system-titlebar` brings it back.

use crate::theme;
use eframe::egui::{
    self, viewport::ResizeDirection, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Stroke,
    StrokeKind, Ui, Vec2, ViewportCommand,
};

/// Height of the title bar.
pub const HEIGHT: f32 = 34.0;
/// Width of one window button.
const BUTTON_WIDTH: f32 = 46.0;
/// How far inside the window edge a press starts a resize.
const RESIZE_MARGIN: f32 = 5.0;
/// Red of the close button under the pointer (the color every desktop uses for it).
const CLOSE_HOVER: Color32 = Color32::from_rgb(196, 43, 28);

/// What the title bar shows in its middle.
pub struct Title<'a> {
    pub document: &'a str,
    pub modified: bool,
    /// Shown dimmed after the document (the project's name).
    pub project: Option<&'a str>,
    pub playing: bool,
}

/// Whether the window fills the screen (no resize edges or border then).
fn is_maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| {
        let viewport = i.viewport();
        viewport.maximized.unwrap_or(false) || viewport.fullscreen.unwrap_or(false)
    })
}

/// The title bar. `menus` draws the menus on its left; the rest of the bar moves the window
/// (double click maximizes).
pub fn title_bar(ui: &mut Ui, title: &Title<'_>, menus: impl FnOnce(&mut Ui)) {
    let bar = ui.max_rect();
    // The background first, so the widgets drawn on top of it take the clicks.
    let background = ui.interact(bar, ui.id().with("drag"), Sense::click_and_drag());
    if background.double_clicked() {
        let maximized = is_maximized(ui.ctx());
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if background.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }

    let mut menus_end = bar.left();
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        window_buttons(ui);
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            egui::MenuBar::new().ui(ui, |ui| {
                menus(ui);
                menus_end = ui.cursor().left();
            });
        });
    });
    let buttons_start = bar.right() - 3.0 * BUTTON_WIDTH;
    paint_title(ui, bar, menus_end + 16.0..=buttons_start - 16.0, title);
}

/// The document title, centered on the window when it fits between the menus and buttons.
fn paint_title(ui: &Ui, bar: Rect, free: std::ops::RangeInclusive<f32>, title: &Title<'_>) {
    let focused = ui.ctx().input(|i| i.viewport().focused.unwrap_or(true));
    let strong = if focused {
        Color32::from_rgb(226, 230, 238)
    } else {
        theme::TEXT_DIM
    };
    let font = FontId::proportional(13.0);
    let mut job = egui::text::LayoutJob::default();
    let mut add = |text: &str, color: Color32| {
        job.append(text, 0.0, egui::TextFormat::simple(font.clone(), color));
    };
    add(title.document, strong);
    if title.modified {
        add("  ●", theme::ACCENT);
    }
    if let Some(project) = title.project {
        add(&format!("   —   {project}"), theme::TEXT_DIM);
    }
    if title.playing {
        add(
            &format!("   {}  Playing", egui_phosphor::regular::PLAY),
            theme::PLAY,
        );
    }
    let galley = ui.painter().layout_job(job);
    let width = galley.size().x;
    let (left, right) = (*free.start(), *free.end());
    if right - left < width {
        return;
    }
    let x = (bar.center().x - width / 2.0).clamp(left, right - width);
    let pos = Pos2::new(x, bar.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, strong);
}

/// Minimize, maximize/restore and close, right to left.
fn window_buttons(ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    if window_button(ui, WindowButton::Close).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    let maximized = is_maximized(&ctx);
    let (kind, tip) = if maximized {
        (WindowButton::Restore, "Restore")
    } else {
        (WindowButton::Maximize, "Maximize")
    };
    if window_button(ui, kind).on_hover_text(tip).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }
    if window_button(ui, WindowButton::Minimize)
        .on_hover_text("Minimize")
        .clicked()
    {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WindowButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// A flat button the height of the bar with a line-drawn glyph (crisp at any scale).
fn window_button(ui: &mut Ui, kind: WindowButton) -> egui::Response {
    let height = ui.available_height().max(HEIGHT);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(BUTTON_WIDTH, height), Sense::click());
    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();
    let (fill, ink) = match (kind, hovered, pressed) {
        (WindowButton::Close, true, false) => (CLOSE_HOVER, Color32::WHITE),
        (WindowButton::Close, true, true) => (CLOSE_HOVER.gamma_multiply(0.8), Color32::WHITE),
        (_, true, false) => (theme::BG_3, Color32::WHITE),
        (_, true, true) => (theme::BG_4, Color32::WHITE),
        _ => (Color32::TRANSPARENT, Color32::from_rgb(196, 200, 210)),
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, fill);
    let c = rect.center();
    let stroke = Stroke::new(1.0_f32, ink);
    let s = 5.0; // half the glyph size
    match kind {
        WindowButton::Minimize => {
            painter.line_segment([Pos2::new(c.x - s, c.y), Pos2::new(c.x + s, c.y)], stroke);
        }
        WindowButton::Maximize => {
            painter.rect_stroke(
                Rect::from_center_size(c, Vec2::splat(2.0 * s)),
                1.0,
                stroke,
                StrokeKind::Inside,
            );
        }
        WindowButton::Restore => {
            let front = Rect::from_min_size(Pos2::new(c.x - s, c.y - s + 2.0), Vec2::splat(8.0));
            painter.rect_stroke(front, 1.0, stroke, StrokeKind::Inside);
            // The window behind: only the parts not covered by the front one.
            let (l, t, r) = (front.left() + 2.0, front.top() - 2.0, front.right() + 2.0);
            painter.line_segment([Pos2::new(l, front.top()), Pos2::new(l, t)], stroke);
            painter.line_segment([Pos2::new(l, t), Pos2::new(r, t)], stroke);
            painter.line_segment(
                [Pos2::new(r, t), Pos2::new(r, front.bottom() - 2.0)],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(r, front.bottom() - 2.0),
                    Pos2::new(front.right(), front.bottom() - 2.0),
                ],
                stroke,
            );
        }
        WindowButton::Close => {
            painter.line_segment([c + Vec2::new(-s, -s), c + Vec2::new(s, s)], stroke);
            painter.line_segment([c + Vec2::new(-s, s), c + Vec2::new(s, -s)], stroke);
        }
    }
    response
}

/// Resize edges and the window border. Call after the panels, so the edge cursor wins.
pub fn window_frame(ctx: &egui::Context) {
    if is_maximized(ctx) {
        return;
    }
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("window-border"),
    ));
    let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
    let color = if focused { theme::BG_4 } else { theme::BORDER };
    painter.rect_stroke(
        screen.shrink(0.5),
        0.0,
        Stroke::new(1.0_f32, color),
        StrokeKind::Inside,
    );

    // Not while dragging something else (a splitter, a gizmo) across the edge.
    let (hover, pressed, busy) = ctx.input(|i| {
        (
            i.pointer.hover_pos(),
            i.pointer.primary_pressed(),
            i.pointer.any_down() && !i.pointer.primary_pressed(),
        )
    });
    let Some(direction) = hover.and_then(|p| resize_direction(screen, p, RESIZE_MARGIN)) else {
        return;
    };
    if busy {
        return;
    }
    ctx.set_cursor_icon(cursor_for(direction));
    if pressed {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

/// Which edge or corner of `window` the point is on, if within `margin` of it. Corners
/// get a larger catch area so they are easy to hit.
pub fn resize_direction(window: Rect, pos: Pos2, margin: f32) -> Option<ResizeDirection> {
    if !window.contains(pos) {
        return None;
    }
    let corner = margin * 3.0;
    let near = |distance: f32, reach: f32| distance <= reach;
    let (left, right) = (pos.x - window.left(), window.right() - pos.x);
    let (top, bottom) = (pos.y - window.top(), window.bottom() - pos.y);
    let on_edge =
        near(left, margin) || near(right, margin) || near(top, margin) || near(bottom, margin);
    if !on_edge {
        return None;
    }
    let west = near(left, corner);
    let east = near(right, corner);
    let north = near(top, corner);
    let south = near(bottom, corner);
    Some(match (north, south, west, east) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (true, _, _, true) => ResizeDirection::NorthEast,
        (_, true, true, _) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        _ if near(left, margin) => ResizeDirection::West,
        _ if near(right, margin) => ResizeDirection::East,
        _ if near(top, margin) => ResizeDirection::North,
        _ => ResizeDirection::South,
    })
}

fn cursor_for(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North | ResizeDirection::South => CursorIcon::ResizeVertical,
        ResizeDirection::East | ResizeDirection::West => CursorIcon::ResizeHorizontal,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::ResizeNeSw,
    }
}

/// Name shown for a project: its folder, or the game's folder when that is `assets`.
pub fn project_name(root: &std::path::Path) -> String {
    let name = |p: &std::path::Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    match name(root) {
        Some(n) if n == "assets" => root.parent().and_then(name).unwrap_or(n),
        Some(n) => n,
        None => root.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))
    }

    #[test]
    fn edges_and_corners() {
        let at = |x, y| resize_direction(window(), Pos2::new(x, y), 5.0);
        assert_eq!(at(400.0, 300.0), None);
        assert_eq!(at(2.0, 300.0), Some(ResizeDirection::West));
        assert_eq!(at(798.0, 300.0), Some(ResizeDirection::East));
        assert_eq!(at(400.0, 1.0), Some(ResizeDirection::North));
        assert_eq!(at(400.0, 599.0), Some(ResizeDirection::South));
        assert_eq!(at(1.0, 1.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(799.0, 599.0), Some(ResizeDirection::SouthEast));
        // Corners reach further along the edge than into the window.
        assert_eq!(at(12.0, 2.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(797.0, 12.0), Some(ResizeDirection::NorthEast));
        assert_eq!(at(12.0, 12.0), None);
        assert_eq!(at(-1.0, 300.0), None);
    }

    #[test]
    fn project_names() {
        use std::path::Path;
        assert_eq!(
            project_name(Path::new("/x/games/coin-garden/assets")),
            "coin-garden"
        );
        assert_eq!(project_name(Path::new("/x/my-levels")), "my-levels");
    }
}
