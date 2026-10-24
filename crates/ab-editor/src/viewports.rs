//! The viewport area: one view, or four (perspective, top, front, side) side by side.
//!
//! Every view has its own camera and display options and navigates on its own; they share
//! the scene, the selection, the scene renderer and the editing tools (gizmo mode,
//! snapping, highlighting), so a change in one shows in all. The first view is the main
//! one: while playing it shows the game's camera and takes the game's input.

use crate::state::Editor;
use crate::viewport::{GizmoMode, ViewKind, Viewport};
use eframe::egui::{self, Rect};
use eframe::egui_wgpu;
use std::ops::{Deref, DerefMut};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Single,
    /// Two by two: perspective, top / front, side.
    Quad,
}

/// The editing tools every view shares.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shared {
    gizmo: GizmoMode,
    snap: crate::settings::SnapSettings,
    highlight_selection: bool,
    highlight_hover: bool,
    ui: bool,
}

impl Shared {
    fn of(view: &Viewport) -> Self {
        let o = &view.options;
        Self {
            gizmo: o.gizmo,
            snap: o.snap,
            highlight_selection: o.highlight_selection,
            highlight_hover: o.highlight_hover,
            ui: o.ui,
        }
    }

    fn apply(self, view: &mut Viewport) {
        let o = &mut view.options;
        o.gizmo = self.gizmo;
        o.snap = self.snap;
        o.highlight_selection = self.highlight_selection;
        o.highlight_hover = self.highlight_hover;
        o.ui = self.ui;
    }
}

/// Gap between the views of the four-view layout, in points.
const GAP: f32 = 2.0;

pub struct ViewportArea {
    /// The main view first; the others are made when four views are first shown.
    views: Vec<Viewport>,
    pub layout: Layout,
    /// The view last clicked (four views).
    active: usize,
    shared: Shared,
}

impl ViewportArea {
    /// Frames the selection in every view (in the four-view layout, each keeps its angle).
    pub fn frame_selection_everywhere(&mut self, editor: &Editor) {
        for view in &mut self.views {
            view.frame_selection(editor);
        }
    }

    /// Where new things go: the pivot of the view last clicked (the main view's in the
    /// single layout).
    pub fn placement(&self) -> ab_engine::core::Vec3 {
        let index = match self.layout {
            Layout::Single => 0,
            Layout::Quad => self.active.min(self.views.len() - 1),
        };
        self.views[index].camera.pivot
    }

    pub fn new(render_state: Option<&egui_wgpu::RenderState>) -> Self {
        let main = Viewport::new(render_state);
        Self {
            shared: Shared::of(&main),
            views: vec![main],
            layout: Layout::Single,
            active: 0,
        }
    }

    /// The views on screen, the main one first.
    #[cfg(test)]
    pub fn views(&self) -> &[Viewport] {
        match self.layout {
            Layout::Single => &self.views[..1],
            Layout::Quad => &self.views,
        }
    }

    #[cfg(test)]
    pub fn active(&self) -> usize {
        self.active
    }

    pub fn set_layout(&mut self, layout: Layout) {
        self.layout = layout;
        if layout == Layout::Quad && self.views.len() < 4 {
            for kind in [ViewKind::Top, ViewKind::Front, ViewKind::Side] {
                let view = self.views[0].sharing(kind);
                self.views.push(view);
            }
        }
        if layout == Layout::Single {
            self.active = 0;
        }
    }

    pub fn toggle_layout(&mut self) {
        self.set_layout(match self.layout {
            Layout::Single => Layout::Quad,
            Layout::Quad => Layout::Single,
        });
    }

    /// Takes a change of the shared tools from whichever view made it, then gives every
    /// view the same.
    fn sync(&mut self) {
        if let Some(changed) = self
            .views
            .iter()
            .map(Shared::of)
            .find(|shared| *shared != self.shared)
        {
            self.shared = changed;
        }
        for view in &mut self.views {
            self.shared.apply(view);
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        render_state: Option<&egui_wgpu::RenderState>,
    ) {
        self.sync();
        let area = ui.available_rect_before_wrap();
        let rects: Vec<Rect> = match self.layout {
            Layout::Single => vec![area],
            Layout::Quad => quad(area),
        };
        let pressed = ui
            .input(|i| i.pointer.any_pressed().then(|| i.pointer.press_origin()))
            .flatten();
        for (index, rect) in rects.into_iter().enumerate() {
            let view = &mut self.views[index];
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                if index == 0 {
                    view.show(ui, editor, render_state);
                } else {
                    // Only the main view's camera stays for the game (its damage numbers
                    // and other screen-space effects project with it).
                    crate::app::keeping_camera(editor, |editor| {
                        view.show(ui, editor, render_state);
                    });
                }
            });
            if self.layout == Layout::Quad {
                if pressed.is_some_and(|p| rect.contains(p)) {
                    self.active = index;
                }
                if index == self.active {
                    ui.painter().rect_stroke(
                        rect,
                        0.0,
                        egui::Stroke::new(1.0_f32, crate::theme::ACCENT_DIM),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
        ui.allocate_rect(area, egui::Sense::hover());
        self.sync();
        if self.views.iter().any(|v| v.toggle_layout) {
            for view in &mut self.views {
                view.toggle_layout = false;
            }
            self.toggle_layout();
        }
    }
}

/// The four views' rectangles: perspective, top on the first row; front, side below.
fn quad(area: Rect) -> Vec<Rect> {
    let half = (area.size() - egui::Vec2::splat(GAP)) / 2.0;
    let at = |col: f32, row: f32| {
        Rect::from_min_size(
            area.min + egui::vec2(col * (half.x + GAP), row * (half.y + GAP)),
            half,
        )
    };
    vec![at(0.0, 0.0), at(1.0, 0.0), at(0.0, 1.0), at(1.0, 1.0)]
}

/// The main view: what menus, commands and play mode act on.
impl Deref for ViewportArea {
    type Target = Viewport;

    fn deref(&self) -> &Viewport {
        &self.views[0]
    }
}

impl DerefMut for ViewportArea {
    fn deref_mut(&mut self) -> &mut Viewport {
        &mut self.views[0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_views_share_the_tools_and_keep_their_own_cameras() {
        let mut area = ViewportArea::new(None);
        assert_eq!(area.views().len(), 1);
        area.set_layout(Layout::Quad);
        let kinds: Vec<_> = area.views().iter().map(|v| v.kind).collect();
        assert_eq!(
            kinds,
            [
                ViewKind::Perspective,
                ViewKind::Top,
                ViewKind::Front,
                ViewKind::Side
            ]
        );
        assert!(area.views()[0].main && !area.views()[1].main);

        // A tool picked through the main view (a shortcut) reaches every view…
        area.options.gizmo = GizmoMode::Rotate;
        area.sync();
        assert!(area
            .views()
            .iter()
            .all(|v| v.options.gizmo == GizmoMode::Rotate));
        // …and so does one picked in another view.
        area.views[2].options.snap.enabled = true;
        area.sync();
        assert!(area.views().iter().all(|v| v.options.snap.enabled));
        // Per-view options and cameras stay apart.
        area.views[1].options.grid = false;
        area.views[1].camera.distance = 3.0;
        area.sync();
        assert!(area.views[0].options.grid);
        assert!(area.views[0].camera.distance != 3.0);

        area.toggle_layout();
        assert_eq!(area.views().len(), 1);
        assert_eq!(area.active(), 0);
    }

    #[test]
    fn the_quad_fills_the_area_without_overlap() {
        let area = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(802.0, 602.0));
        let rects = quad(area);
        assert_eq!(rects[0].min, area.min);
        assert_eq!(rects[3].max, area.max);
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                assert!(!a.intersects(*b) || a.intersect(*b).area() == 0.0);
            }
        }
    }
}
