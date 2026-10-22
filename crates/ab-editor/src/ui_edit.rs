//! Editing UI controls in the viewport: click one to select it, drag it to move it, drag the
//! handles of the selected one to resize it. Moves change the control's offsets (its anchors
//! stay), in UI units (1920 × 1080 by default), rounded to whole units, or to 8 with snapping.

use crate::state::Editor;
use crate::theme;
use ab_engine::scene::NodeHandle;
use ab_engine::ui::{class, CanvasLayer, Control, UiState};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};

/// Grid of UI units a snapped move or resize lands on.
pub const SNAP: f32 = 8.0;
/// Half the size of a resize handle, in points.
const HANDLE: f32 = 4.5;

/// Edges a drag moves: left, top, right, bottom.
type Edges = [bool; 4];
const MOVE: Edges = [true; 4];

#[derive(Debug, Clone)]
pub struct UiDrag {
    edges: Edges,
    start: Pos2,
    /// Each moved control with its offsets when the drag started.
    nodes: Vec<(NodeHandle, [f32; 4])>,
}

/// A control as drawn: its rect in UI units.
#[derive(Debug, Clone, Copy)]
pub struct Placed {
    pub node: NodeHandle,
    pub rect: ab_engine::ui::Rect,
    /// Placed by a box container: cannot be moved by hand.
    pub in_container: bool,
}

/// Maps UI units to viewport points.
#[derive(Debug, Clone, Copy)]
pub struct UiView {
    origin: Pos2,
    /// Points per UI unit.
    scale: f32,
}

impl UiView {
    /// The mapping of the last frame laid out for a viewport at `rect`.
    pub fn new(editor: &Editor, rect: Rect, pixels_per_point: f32) -> Option<Self> {
        let state = editor.tree().world().resource::<UiState>()?;
        let scale = state.scale() / pixels_per_point.max(0.01);
        (scale > 0.0).then_some(Self {
            origin: rect.min,
            scale,
        })
    }

    pub fn screen(&self, r: &ab_engine::ui::Rect) -> Rect {
        Rect::from_min_size(
            self.origin + Vec2::new(r.position.x, r.position.y) * self.scale,
            Vec2::new(r.size.x, r.size.y) * self.scale,
        )
    }
}

/// Every editable, visible control of the scene in drawing order (later ones on top).
pub fn controls(editor: &Editor) -> Vec<Placed> {
    let tree = editor.tree();
    let mut placed: Vec<(i32, usize, Placed)> = Vec::new();
    for (index, node) in tree.subtree(editor.scene_root).into_iter().enumerate() {
        if !tree.is_visible_in_tree(node) || !editor.is_editable(node) {
            continue;
        }
        let Some(control) = tree.component::<Control>(node) else {
            continue;
        };
        // The layer of the nearest canvas layer above it.
        let layer = std::iter::successors(tree.parent(node), |&n| tree.parent(n))
            .find_map(|n| tree.component::<CanvasLayer>(n).map(|c| c.layer))
            .unwrap_or(0);
        let in_container = tree.parent(node).is_some_and(|p| {
            matches!(
                tree.class_name(p),
                Some(class::VBOX_CONTAINER | class::HBOX_CONTAINER)
            )
        });
        placed.push((
            layer,
            index,
            Placed {
                node,
                rect: control.rect(),
                in_container,
            },
        ));
    }
    placed.sort_by_key(|&(layer, index, _)| (layer, index));
    placed.into_iter().map(|(_, _, p)| p).collect()
}

/// The control a click at `pos` selects: the topmost under it. Plain `Control`s (groups,
/// usually as big as the screen) are skipped so the 3D scene stays clickable through them.
pub fn pick(controls: &[Placed], view: &UiView, pos: Pos2, editor: &Editor) -> Option<NodeHandle> {
    let tree = editor.tree();
    controls
        .iter()
        .rev()
        .filter(|p| tree.class_name(p.node) != Some(class::CONTROL))
        .find(|p| view.screen(&p.rect).contains(pos))
        .map(|p| p.node)
}

/// The eight handles of a rect: which edges each moves, and where it is.
fn handles(r: Rect) -> [(Edges, Pos2); 8] {
    let (l, t, rr, b) = (r.left(), r.top(), r.right(), r.bottom());
    let (cx, cy) = (r.center().x, r.center().y);
    [
        ([true, true, false, false], Pos2::new(l, t)),
        ([false, true, false, false], Pos2::new(cx, t)),
        ([false, true, true, false], Pos2::new(rr, t)),
        ([false, false, true, false], Pos2::new(rr, cy)),
        ([false, false, true, true], Pos2::new(rr, b)),
        ([false, false, false, true], Pos2::new(cx, b)),
        ([true, false, false, true], Pos2::new(l, b)),
        ([true, false, false, false], Pos2::new(l, cy)),
    ]
}

fn handle_at(r: Rect, pos: Pos2) -> Option<Edges> {
    handles(r)
        .into_iter()
        .find(|(_, at)| Rect::from_center_size(*at, Vec2::splat(HANDLE * 2.0 + 4.0)).contains(pos))
        .map(|(edges, _)| edges)
}

fn cursor(edges: Edges) -> egui::CursorIcon {
    use egui::CursorIcon as C;
    match edges {
        [true, true, false, false] | [false, false, true, true] => C::ResizeNwSe,
        [false, true, true, false] | [true, false, false, true] => C::ResizeNeSw,
        [false, true, false, false] | [false, false, false, true] => C::ResizeVertical,
        [true, false, false, false] | [false, false, true, false] => C::ResizeHorizontal,
        _ => C::Move,
    }
}

/// Offsets after dragging `edges` by `delta` UI units, snapped to `step`. A control keeps
/// at least one unit of size.
pub fn dragged(start: [f32; 4], edges: Edges, delta: [f32; 2], step: f32) -> [f32; 4] {
    let snap = |v: f32| (v / step).round() * step;
    let mut out = start;
    if edges == MOVE {
        // Moved as a whole: the top-left corner lands on the grid, the size is kept.
        let dx = snap(start[0] + delta[0]) - start[0];
        let dy = snap(start[1] + delta[1]) - start[1];
        return [start[0] + dx, start[1] + dy, start[2] + dx, start[3] + dy];
    }
    for (i, moves) in edges.into_iter().enumerate() {
        if moves {
            out[i] = snap(start[i] + delta[i % 2]);
        }
    }
    out[0] = out[0].min(out[2] - 1.0);
    out[1] = out[1].min(out[3] - 1.0);
    out
}

/// Draws the controls and handles the pointer. Returns whether the pointer is busy with UI
/// (so the 3D tools leave it alone).
#[allow(clippy::too_many_arguments)]
pub fn edit(
    drag: &mut Option<UiDrag>,
    ui: &egui::Ui,
    response: &egui::Response,
    painter: &egui::Painter,
    view: &UiView,
    editor: &mut Editor,
    snapping: bool,
) -> bool {
    let controls = controls(editor);
    let pointer = response.hover_pos();
    let active = editor.selection;
    let selected = |node: NodeHandle| editor.is_selected(node);

    // Outlines: faint for every control, bright for the selected ones.
    for p in &controls {
        let r = view.screen(&p.rect);
        if selected(p.node) {
            let strong = active == Some(p.node);
            let color = if strong {
                theme::SELECTION
            } else {
                theme::SELECTION.gamma_multiply(0.6)
            };
            painter.rect_stroke(r, 0.0, Stroke::new(1.5, color), egui::StrokeKind::Outside);
        } else {
            painter.rect_stroke(
                r,
                0.0,
                Stroke::new(1.0, Color32::from_white_alpha(28)),
                egui::StrokeKind::Inside,
            );
        }
    }
    // The active control's handles (not for controls a container places).
    let active_placed = controls.iter().find(|p| Some(p.node) == active).copied();
    let resizable = active_placed.filter(|p| !p.in_container);
    if let Some(p) = resizable {
        for (_, at) in handles(view.screen(&p.rect)) {
            let square = Rect::from_center_size(at, Vec2::splat(HANDLE * 2.0));
            painter.rect_filled(square, 1.0, theme::BG_0);
            painter.rect_stroke(
                square,
                1.0,
                Stroke::new(1.5, theme::SELECTION),
                egui::StrokeKind::Inside,
            );
        }
    }

    // What the pointer would grab.
    let grab = pointer.and_then(|pos| {
        if let Some(edges) = resizable.and_then(|p| handle_at(view.screen(&p.rect), pos)) {
            return Some((edges, None));
        }
        let over_selected = controls
            .iter()
            .rev()
            .find(|p| selected(p.node) && view.screen(&p.rect).contains(pos));
        match over_selected {
            Some(_) => Some((MOVE, None)),
            None => pick(&controls, view, pos, editor).map(|n| (MOVE, Some(n))),
        }
    });
    if drag.is_none() {
        if let Some((edges, None)) = grab {
            ui.ctx().set_cursor_icon(cursor(edges));
        }
    }

    // Start a drag.
    if response.drag_started_by(egui::PointerButton::Primary) && drag.is_none() {
        let start = ui.input(|i| i.pointer.press_origin()).or(pointer);
        if let (Some((edges, pick)), Some(start)) = (grab, start) {
            if let Some(node) = pick {
                editor.select(Some(node));
            }
            let tree = editor.tree();
            let nodes: Vec<(NodeHandle, [f32; 4])> = if edges == MOVE {
                controls
                    .iter()
                    .filter(|p| editor.is_selected(p.node) && !p.in_container)
                    // Not both a control and one inside it: the parent carries the child.
                    .filter(|p| {
                        !std::iter::successors(tree.parent(p.node), |&n| tree.parent(n))
                            .any(|a| editor.is_selected(a))
                    })
                    .filter_map(|p| {
                        tree.component::<Control>(p.node)
                            .map(|c| (p.node, c.offsets))
                    })
                    .collect()
            } else {
                resizable
                    .and_then(|p| {
                        tree.component::<Control>(p.node)
                            .map(|c| (p.node, c.offsets))
                    })
                    .into_iter()
                    .collect()
            };
            if nodes.is_empty() {
                if editor
                    .selection
                    .is_some_and(|n| controls.iter().any(|p| p.node == n && p.in_container))
                {
                    editor.info("Its container places this control: move the container");
                }
            } else {
                *drag = Some(UiDrag {
                    edges,
                    start,
                    nodes,
                });
            }
        }
    }

    // Continue it.
    let busy = drag.is_some() || grab.is_some();
    if let Some(d) = drag.as_ref() {
        ui.ctx().set_cursor_icon(cursor(d.edges));
        if let Some(now) = ui.input(|i| i.pointer.interact_pos()) {
            let delta = (now - d.start) / view.scale;
            let step = if snapping { SNAP } else { 1.0 };
            if delta != Vec2::ZERO {
                editor.begin_edit();
                let tree = editor.tree_mut();
                for &(node, start) in &d.nodes {
                    if let Some(mut c) = tree.component_mut::<Control>(node) {
                        c.offsets = dragged(start, d.edges, [delta.x, delta.y], step);
                    }
                }
            }
        }
        if !response.dragged_by(egui::PointerButton::Primary) {
            *drag = None;
        }
    }
    busy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_keep_the_size_and_snap_the_corner() {
        let start = [10.0, 20.0, 110.0, 70.0];
        assert_eq!(
            dragged(start, MOVE, [5.4, -3.0], 1.0),
            [15.0, 17.0, 115.0, 67.0]
        );
        assert_eq!(
            dragged(start, MOVE, [3.0, 0.0], SNAP),
            [16.0, 24.0, 116.0, 74.0]
        );
    }

    #[test]
    fn resizing_moves_only_the_dragged_edges_and_never_flips() {
        let start = [0.0, 0.0, 100.0, 50.0];
        let right_bottom = [false, false, true, true];
        assert_eq!(
            dragged(start, right_bottom, [20.0, 10.0], 1.0),
            [0.0, 0.0, 120.0, 60.0]
        );
        let left = [true, false, false, false];
        assert_eq!(
            dragged(start, left, [500.0, 0.0], 1.0),
            [99.0, 0.0, 100.0, 50.0]
        );
    }

    #[test]
    fn clicks_pick_the_topmost_widget_and_skip_groups() {
        let assets = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let mut editor = Editor::new(assets, None, ab_engine::audio::AudioOutput::Silent);
        editor.select(None);
        let menu = crate::create::place_ui(&mut editor, crate::create::UI_SCREENS[1].scene());
        editor.add_scene(menu, None);
        ab_engine::ui::prepare_ui(editor.tree().world(), 1920, 1080);
        let view = UiView {
            origin: Pos2::ZERO,
            scale: 0.5,
        };
        let placed = controls(&editor);
        let name =
            |n: Option<NodeHandle>| n.and_then(|n| editor.tree().name(n).map(|n| n.to_owned()));
        // The window's centre column holds the buttons; its corner is the window itself.
        let window = placed
            .iter()
            .find(|p| editor.tree().name(p.node).as_deref() == Some("Window"))
            .unwrap();
        let corner = view.screen(&window.rect).min + Vec2::splat(4.0);
        assert_eq!(
            name(pick(&placed, &view, corner, &editor)).as_deref(),
            Some("Window")
        );
        // Outside the window: the dimming panel, not the full-screen group under it.
        assert_eq!(
            name(pick(&placed, &view, Pos2::new(5.0, 5.0), &editor)).as_deref(),
            Some("Dim")
        );
        let quit = placed
            .iter()
            .find(|p| editor.tree().name(p.node).as_deref() == Some("Quit"))
            .unwrap();
        assert!(quit.in_container);
        let centre = view.screen(&quit.rect).center();
        assert_eq!(
            name(pick(&placed, &view, centre, &editor)).as_deref(),
            Some("Quit")
        );
    }
}
