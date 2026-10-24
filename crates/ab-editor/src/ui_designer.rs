//! The UI Designer (a tool window): the game's interface on a screen of a chosen size, as
//! the player will see it, with the controls' hierarchy on the left and the selected
//! control's properties on the right.
//!
//! * Pick a screen (Full HD, 4K, ultrawide, phone portrait, …): the interface is laid out
//!   for it, so anchors and scaling can be checked without resizing the window.
//! * Click a control to select it, drag it to move it, drag its handles to resize it (the
//!   same tools as the viewport's UI editing, see [`crate::ui_edit`]). Arrow keys nudge the
//!   selection by one unit, eight with Shift. Snapping (8 units) is on the tool bar; Ctrl
//!   snaps while held.
//! * Wheel zooms around the pointer, middle or right drag pans, Fit shows the whole screen,
//!   F (or a double click in the hierarchy) zooms to the selected control.
//! * The scene is drawn every frame while the window is in use, and otherwise only when it
//!   changes or a few times a second (see [`tool_window::Redraw`]).
//! * Guides show the screen's centre and its title-safe area (90 %).
//! * It edits one UI node (a canvas layer) at a time, the one double-clicked in the scene
//!   tree or picked on the tool bar: its controls are listed, picked and added there, and
//!   only there (the scene tree shows the UI node alone).

use crate::i18n::t;
use crate::panels::inspector::InspectorPanel;
use crate::state::{Editor, Mode};
use crate::theme;
use crate::tool_window;
use crate::ui_edit::{self, UiDrag, UiView};
use crate::viewport::ViewportRenderer;
use ab_engine::rendering::CameraOverride;
use ab_engine::scene::NodeHandle;
use ab_engine::ui::{class, CanvasLayer, Control, UiState};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use eframe::egui_wgpu;
use egui_phosphor::regular as icon;

/// Screens to lay the interface out for: name, width and height in pixels.
pub const SCREENS: &[(&str, u32, u32)] = &[
    ("Full HD", 1920, 1080),
    ("HD", 1280, 720),
    ("QHD", 2560, 1440),
    ("4K UHD", 3840, 2160),
    ("Ultrawide", 3440, 1440),
    ("Steam Deck", 1280, 800),
    ("Classic 4:3", 1024, 768),
    ("Tablet", 2048, 1536),
    ("Phone (landscape)", 2340, 1080),
    ("Phone (portrait)", 1080, 2340),
];

/// Zoom steps of the − and + buttons.
const ZOOMS: &[f32] = &[0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0];
/// Space kept around the screen when it is fitted to the canvas, in points.
const FIT_MARGIN: f32 = 32.0;

pub struct UiDesigner {
    /// Draws the scene and its interface (made on first show).
    renderer: Option<ViewportRenderer>,
    drag: Option<UiDrag>,
    /// Index in [`SCREENS`].
    screen: usize,
    /// Screen pixels per physical pixel; `None`: fit the canvas.
    zoom: Option<f32>,
    /// The screen's offset from the canvas centre, in points.
    pan: Vec2,
    snap: bool,
    guides: bool,
    inspector: InspectorPanel,
    redraw: tool_window::Redraw,
    /// Zoom to the selected control next frame (F, or a double click in the hierarchy).
    frame_request: bool,
    /// The UI node being edited (a canvas layer of the scene).
    root: Option<NodeHandle>,
    /// Show the edited UI node alone (the scene's other UIs hidden while drawing).
    solo: bool,
    /// What the screen was last drawn with (solo, root): a change draws it again.
    drawn_with: Option<(bool, Option<NodeHandle>)>,
}

impl Default for UiDesigner {
    fn default() -> Self {
        Self {
            renderer: None,
            drag: None,
            screen: 0,
            zoom: None,
            pan: Vec2::ZERO,
            snap: true,
            guides: true,
            inspector: InspectorPanel::default(),
            redraw: Default::default(),
            frame_request: false,
            root: None,
            solo: true,
            drawn_with: None,
        }
    }
}

/// Whether `node` is part of an interface (a control or a canvas layer).
fn is_ui(editor: &Editor, node: NodeHandle) -> bool {
    let tree = editor.tree();
    tree.component::<Control>(node).is_some() || tree.component::<CanvasLayer>(node).is_some()
}

/// The scene's UI nodes (canvas layers), in tree order.
fn layers(editor: &Editor) -> Vec<NodeHandle> {
    let tree = editor.tree();
    tree.subtree(editor.scene_root)
        .into_iter()
        .filter(|&n| tree.component::<CanvasLayer>(n).is_some() && editor.is_editable(n))
        .collect()
}

/// Runs `f` with the scene's other UI nodes hidden (when `solo`), then shows them again.
fn with_solo<R>(
    editor: &mut Editor,
    root: Option<NodeHandle>,
    solo: bool,
    f: impl FnOnce(&mut Editor) -> R,
) -> R {
    match root {
        Some(root) if solo => with_only_ui(editor, Some(root), f),
        _ => f(editor),
    }
}

/// Runs `f` with every UI node but `keep` hidden (all of them without one), then shows
/// them again: views that are about something else (an effect, an animation) are not
/// covered by the game's interface. Only the flag the drawing reads is touched: no edit,
/// no undo step, no new revision.
pub(crate) fn with_only_ui<R>(
    editor: &mut Editor,
    keep: Option<NodeHandle>,
    f: impl FnOnce(&mut Editor) -> R,
) -> R {
    let hidden: Vec<NodeHandle> = layers(editor)
        .into_iter()
        .filter(|&l| Some(l) != keep)
        .filter(|&l| {
            editor
                .tree()
                .world()
                .get::<ab_engine::scene::NodeInfo>(l.entity())
                .is_some_and(|i| i.visible)
        })
        .collect();
    let set = |editor: &mut Editor, visible: bool| {
        for &layer in &hidden {
            if let Some(mut info) = editor
                .tree_mut()
                .world_mut()
                .get_mut::<ab_engine::scene::NodeInfo>(layer.entity())
            {
                info.visible = visible;
            }
        }
    };
    set(editor, false);
    let result = f(editor);
    set(editor, true);
    result
}

/// Whether `node` is `root` or inside it (everything, without a root).
fn in_scope(editor: &Editor, root: Option<NodeHandle>, node: NodeHandle) -> bool {
    let Some(root) = root else {
        return true;
    };
    let tree = editor.tree();
    std::iter::successors(Some(node), |&n| tree.parent(n)).any(|n| n == root)
}

impl UiDesigner {
    /// Edits the UI node `layer` (it is selected, the screen fitted).
    pub fn edit(&mut self, editor: &mut Editor, layer: NodeHandle) {
        self.root = Some(layer);
        editor.select(Some(layer));
        self.fit();
    }

    /// The UI node edited: the one chosen while it is still there, else the scene's first.
    fn root(&mut self, editor: &Editor) -> Option<NodeHandle> {
        let all = layers(editor);
        if !self.root.is_some_and(|r| all.contains(&r)) {
            self.root = all.first().copied();
        }
        self.root
    }

    /// The window's body. `share` makes the renderer (from the main viewport's).
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        render_state: Option<&egui_wgpu::RenderState>,
        share: impl FnOnce() -> Option<ViewportRenderer>,
    ) {
        if self.renderer.is_none() {
            self.renderer = share();
        }
        let root = self.root(editor);
        tool_window::toolbar(ui, "ui-designer", |ui| self.toolbar(ui, editor, root));
        tool_window::side(
            ui,
            "ui-designer-hierarchy",
            true,
            240.0,
            icon::TREE_STRUCTURE,
            &t("ui_designer.hierarchy"),
            |ui| {
                if hierarchy(ui, editor, root) {
                    self.frame_request = true;
                }
            },
        );
        tool_window::side(
            ui,
            "ui-designer-inspector",
            false,
            320.0,
            icon::SLIDERS_HORIZONTAL,
            &t("tab.inspector"),
            |ui| self.inspector.show(ui, editor),
        );
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::BG_0))
            .show(ui, |ui| self.canvas(ui, editor, render_state, root));
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, editor: &mut Editor, root: Option<NodeHandle>) {
        let editing = !editor.is_playing();
        // Which UI node is edited, or a new one.
        let name = |n: NodeHandle| {
            editor
                .tree()
                .name(n)
                .map(|n| n.to_string())
                .unwrap_or_default()
        };
        let mut chosen = None;
        egui::ComboBox::from_id_salt("ui-designer-root")
            .selected_text(format!(
                "{}  {}",
                theme::class_icon(class::CANVAS_LAYER),
                root.map(name).unwrap_or_else(|| t("ui_designer.no_ui"))
            ))
            .width(180.0)
            .show_ui(ui, |ui| {
                for layer in layers(editor) {
                    if ui
                        .selectable_label(root == Some(layer), name(layer))
                        .clicked()
                    {
                        chosen = Some(layer);
                    }
                }
            })
            .response
            .on_hover_text(t("ui_designer.root_hint"));
        if let Some(layer) = chosen {
            self.edit(editor, layer);
        }
        ui.add_enabled_ui(editing, |ui| {
            if ui
                .button(format!("{}  {}", icon::PLUS, t("ui_designer.new_ui")))
                .on_hover_text(t("ui_designer.new_ui_hint"))
                .clicked()
            {
                if let Some(layer) = new_ui(editor) {
                    self.edit(editor, layer);
                }
            }
            ui.add_enabled_ui(root.is_some(), |ui| {
                ui.menu_button(
                    RichText::new(format!("{}  {}", icon::PLUS, t("ui_designer.add"))).strong(),
                    |ui| add_menu(ui, editor, root),
                );
            });
        });
        ui.separator();
        let (name, width, height) = SCREENS[self.screen];
        egui::ComboBox::from_id_salt("ui-designer-screen")
            .selected_text(format!("{}  {name}  {width} × {height}", icon::MONITOR))
            .width(250.0)
            .show_ui(ui, |ui| {
                for (index, (name, width, height)) in SCREENS.iter().enumerate() {
                    let glyph = if height > width {
                        icon::DEVICE_MOBILE
                    } else {
                        icon::MONITOR
                    };
                    if ui
                        .selectable_label(
                            self.screen == index,
                            format!("{glyph}  {name}   {width} × {height}"),
                        )
                        .clicked()
                    {
                        self.screen = index;
                        self.fit();
                    }
                }
            });
        ui.separator();
        if ui
            .button(icon::MAGNIFYING_GLASS_MINUS)
            .on_hover_text(t("ui_designer.zoom_out"))
            .clicked()
        {
            self.step_zoom(ui.ctx(), -1);
        }
        // The zoom in effect, fitted or chosen (the Fit button says which).
        let fitted = ui
            .ctx()
            .data(|d| d.get_temp::<f32>(egui::Id::new("ui-designer-fit")))
            .unwrap_or(1.0);
        let percent = format!("{:.0} %", self.zoom.unwrap_or(fitted) * 100.0);
        ui.add_sized([56.0, 20.0], egui::Label::new(percent));
        if ui
            .button(icon::MAGNIFYING_GLASS_PLUS)
            .on_hover_text(t("ui_designer.zoom_in"))
            .clicked()
        {
            self.step_zoom(ui.ctx(), 1);
        }
        if ui
            .button(format!("{}  {}", icon::CORNERS_OUT, t("ui_designer.fit")))
            .clicked()
        {
            self.fit();
        }
        if ui
            .button("1:1")
            .on_hover_text(t("ui_designer.actual"))
            .clicked()
        {
            self.zoom = Some(1.0);
            self.pan = Vec2::ZERO;
        }
        ui.separator();
        ui.toggle_value(
            &mut self.snap,
            format!("{}  {}", icon::MAGNET, t("ui_designer.snap")),
        )
        .on_hover_text(t("ui_designer.snap_hint"));
        ui.toggle_value(
            &mut self.guides,
            format!("{}  {}", icon::CROSSHAIR, t("ui_designer.guides")),
        )
        .on_hover_text(t("ui_designer.guides_hint"));
        ui.toggle_value(
            &mut self.solo,
            format!("{}  {}", icon::EYE, t("ui_designer.solo")),
        )
        .on_hover_text(t("ui_designer.solo_hint"));
        ui.separator();
        // The UI node itself is deleted from the scene tree, not here.
        let selection = editing
            && editor
                .selection
                .is_some_and(|n| is_ui(editor, n) && editor.is_editable(n) && Some(n) != root);
        if ui
            .add_enabled(selection, egui::Button::new(icon::COPY))
            .on_hover_text(crate::commands::Command::Duplicate.label())
            .clicked()
        {
            editor.duplicate_selected();
        }
        if ui
            .add_enabled(selection, egui::Button::new(icon::TRASH))
            .on_hover_text(crate::commands::Command::Delete.label())
            .clicked()
        {
            editor.delete_selected();
        }
    }

    fn fit(&mut self) {
        self.zoom = None;
        self.pan = Vec2::ZERO;
    }

    fn step_zoom(&mut self, ctx: &egui::Context, direction: i32) {
        let current = self.zoom.unwrap_or_else(|| {
            ctx.data(|d| d.get_temp::<f32>(egui::Id::new("ui-designer-fit")))
                .unwrap_or(0.5)
        });
        let next = if direction > 0 {
            ZOOMS.iter().copied().find(|&z| z > current + 1e-3)
        } else {
            ZOOMS.iter().rev().copied().find(|&z| z < current - 1e-3)
        };
        if let Some(zoom) = next {
            self.zoom = Some(zoom);
        }
    }

    fn canvas(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        render_state: Option<&egui_wgpu::RenderState>,
        root: Option<NodeHandle>,
    ) {
        let (canvas, response) =
            ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, 0.0, theme::BG_0);
        tool_window::tile(&painter, canvas, "ui-designer-dots", 24.0, || {
            let mut dots = egui::ColorImage::filled([12, 12], Color32::TRANSPARENT);
            dots[(6, 6)] = Color32::from_white_alpha(14);
            dots
        });
        let ppp = ui.ctx().pixels_per_point();
        let (_, width, height) = SCREENS[self.screen];
        let pixels = Vec2::new(width as f32, height as f32) / ppp;
        let fit = ((canvas.width() - FIT_MARGIN * 2.0) / pixels.x)
            .min((canvas.height() - FIT_MARGIN * 2.0) / pixels.y)
            .max(0.02);
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("ui-designer-fit"), fit));
        let mut zoom = self.zoom.unwrap_or(fit);

        // Navigation: wheel zooms around the pointer, middle or right drag pans.
        if let Some(pointer) = response.hover_pos() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let next = (zoom * (1.0 + scroll / 400.0)).clamp(0.02, 8.0);
                let center = canvas.center() + self.pan;
                let moved = pointer - (pointer - center) * (next / zoom);
                self.pan = moved - canvas.center();
                zoom = next;
                self.zoom = Some(zoom);
            }
        }
        if response.dragged_by(egui::PointerButton::Middle)
            || response.dragged_by(egui::PointerButton::Secondary)
        {
            self.pan += response.drag_delta();
            self.zoom = Some(zoom);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let screen = Rect::from_center_size(canvas.center() + self.pan, pixels * zoom);

        // The screen: the scene through the game's camera, with its interface.
        painter.rect_filled(
            screen.translate(Vec2::splat(4.0)),
            2.0,
            Color32::from_black_alpha(90),
        );
        match (&mut self.renderer, render_state) {
            (Some(renderer), Some(state)) => {
                let (w, h) = (
                    (screen.width() * ppp).round().max(1.0) as u32,
                    (screen.height() * ppp).round().max(1.0) as u32,
                );
                // Every frame while in use; when the scene changes or now and then otherwise.
                let due = self
                    .redraw
                    .due(tool_window::in_use(ui.ctx()), editor.revision());
                let with = (self.solo, root);
                let changed = self.drawn_with.replace(with) != Some(with);
                let cached = renderer.cached(w, h).filter(|_| !due && !changed);
                let texture = with_solo(editor, root, self.solo, |editor| match cached {
                    Some(texture) => {
                        // Not drawn, but laid out for this screen: editing needs the rects.
                        ab_engine::ui::prepare_ui(editor.tree().world(), w, h);
                        texture
                    }
                    None => {
                        // The views' editor camera is not the player's.
                        let camera = editor
                            .tree_mut()
                            .world_mut()
                            .remove_resource::<CameraOverride>();
                        let texture = renderer.render(state, editor.tree().world(), w, h);
                        if let Some(camera) = camera {
                            editor.tree_mut().world_mut().insert_resource(camera);
                        }
                        texture
                    }
                });
                painter.image(
                    texture,
                    screen,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            _ => {
                painter.rect_filled(screen, 0.0, theme::BG_2);
                painter.text(
                    screen.center(),
                    egui::Align2::CENTER_CENTER,
                    t("viewport.no_gpu"),
                    egui::FontId::proportional(15.0),
                    theme::TEXT_DIM,
                );
            }
        }
        painter.rect_stroke(
            screen,
            0.0,
            Stroke::new(1.0, theme::BG_4),
            egui::StrokeKind::Outside,
        );
        let (name, width, height) = SCREENS[self.screen];
        painter.text(
            screen.left_top() - Vec2::new(0.0, 6.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{name}  ·  {width} × {height}  ·  {:.0} %", zoom * 100.0),
            egui::FontId::proportional(11.5),
            theme::TEXT_DIM,
        );
        if self.guides {
            guides(&painter, screen);
        }

        // Editing (laid out just now, for this screen).
        let view = UiView::new(editor, screen, ppp).map(|v| v.scoped(root));
        if editor.mode == Mode::Edit {
            if let Some(view) = &view {
                let snapping = self.snap || ui.input(|i| i.modifiers.command);
                ui_edit::edit(
                    &mut self.drag,
                    ui,
                    &response,
                    &painter,
                    view,
                    editor,
                    snapping,
                );
                if response.clicked() {
                    select_at(ui, &response, view, editor, root);
                }
                self.nudge(ui, editor);
            }
        }
        let typing = ui.ctx().egui_wants_keyboard_input();
        if !typing && ui.input(|i| i.key_pressed(egui::Key::F) && i.modifiers.is_none()) {
            self.frame_request = true;
        }
        if std::mem::take(&mut self.frame_request) {
            if let Some(view) = &view {
                self.frame_selection(editor, view, canvas, screen, zoom);
            }
        }
        self.readout(ui, &painter, canvas, screen, editor);
        match root {
            None => {
                if let Some(layer) = empty(ui, canvas, editor, None) {
                    self.edit(editor, layer);
                }
            }
            Some(root) if editor.tree().children(root).is_empty() => {
                empty(ui, canvas, editor, Some(root));
            }
            Some(_) => {}
        }
    }

    /// Zooms and pans so the selected control fills about half the canvas.
    fn frame_selection(
        &mut self,
        editor: &Editor,
        view: &UiView,
        canvas: Rect,
        screen: Rect,
        zoom: f32,
    ) {
        let Some(control) = editor
            .selection
            .and_then(|n| editor.tree().component::<Control>(n).map(|c| c.rect()))
        else {
            return;
        };
        let shown = view.screen(&control);
        if shown.width() < 1.0 || shown.height() < 1.0 {
            return;
        }
        let k = (canvas.width() * 0.5 / shown.width())
            .min(canvas.height() * 0.5 / shown.height())
            .clamp(0.02 / zoom, 8.0 / zoom);
        self.zoom = Some(zoom * k);
        self.pan = -(shown.center() - screen.center()) * k;
    }

    /// Arrow keys move the selected controls by a unit (eight with Shift).
    fn nudge(&self, ui: &egui::Ui, editor: &mut Editor) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (dx, dy, big) = ui.input(|i| {
            let mut d = (0.0, 0.0);
            if i.key_pressed(egui::Key::ArrowLeft) {
                d.0 -= 1.0;
            }
            if i.key_pressed(egui::Key::ArrowRight) {
                d.0 += 1.0;
            }
            if i.key_pressed(egui::Key::ArrowUp) {
                d.1 -= 1.0;
            }
            if i.key_pressed(egui::Key::ArrowDown) {
                d.1 += 1.0;
            }
            (d.0, d.1, i.modifiers.shift)
        });
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let step = if big { ui_edit::SNAP } else { 1.0 };
        let nodes = movable(editor);
        if nodes.is_empty() {
            return;
        }
        editor.begin_edit();
        let tree = editor.tree_mut();
        for node in nodes {
            if let Some(mut control) = tree.component_mut::<Control>(node) {
                let o = &mut control.offsets;
                *o = [
                    o[0] + dx * step,
                    o[1] + dy * step,
                    o[2] + dx * step,
                    o[3] + dy * step,
                ];
            }
        }
    }

    /// The pointer and the active control in UI units, along the canvas's bottom edge.
    fn readout(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        canvas: Rect,
        screen: Rect,
        editor: &Editor,
    ) {
        let Some(scale) = editor
            .tree()
            .world()
            .resource::<UiState>()
            .map(|s| s.scale() / ui.ctx().pixels_per_point())
            .filter(|s| *s > 0.0)
        else {
            return;
        };
        let mut parts = Vec::new();
        if let Some(pos) = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| screen.contains(*p))
        {
            let at = (pos - screen.min) / scale;
            parts.push(format!("{}  {:.0}, {:.0}", icon::CURSOR, at.x, at.y));
        }
        if let Some(node) = editor.selection {
            let tree = editor.tree();
            if let Some(control) = tree.component::<Control>(node) {
                let r = control.rect();
                let name = tree.name(node).map(|n| n.to_string()).unwrap_or_default();
                parts.push(format!(
                    "{}  {name}   x {:.0}  y {:.0}   w {:.0}  h {:.0}",
                    icon::SELECTION,
                    r.position.x,
                    r.position.y,
                    r.size.x,
                    r.size.y
                ));
            }
        }
        if parts.is_empty() {
            return;
        }
        let galley = painter.layout_no_wrap(
            parts.join("     "),
            egui::FontId::monospace(11.5),
            theme::TEXT,
        );
        let at = canvas.left_bottom() + Vec2::new(10.0, -galley.size().y - 8.0);
        painter.rect_filled(
            Rect::from_min_size(
                at - Vec2::new(6.0, 4.0),
                galley.size() + Vec2::new(12.0, 8.0),
            ),
            4.0,
            theme::BG_0.gamma_multiply(0.9),
        );
        painter.galley(at, galley, theme::TEXT);
    }
}

/// Selects what a click at the pointer hit: the topmost control, else nothing (Ctrl and
/// Shift add to the selection, as in the viewport).
fn select_at(
    ui: &egui::Ui,
    response: &egui::Response,
    view: &UiView,
    editor: &mut Editor,
    root: Option<NodeHandle>,
) {
    let Some(pos) = response.interact_pointer_pos() else {
        return;
    };
    let modifiers = ui.input(|i| i.modifiers);
    // Only the edited UI node's controls can be picked.
    let mut controls = ui_edit::controls(editor);
    controls.retain(|c| in_scope(editor, root, c.node));
    match ui_edit::pick(&controls, view, pos, editor) {
        Some(node) if modifiers.command => editor.toggle_selected(node),
        Some(node) if modifiers.shift => editor.add_to_selection(&[node]),
        Some(node) => editor.select(Some(node)),
        None if modifiers.command || modifiers.shift => {}
        None => editor.select(None),
    }
}

/// The selected controls a nudge moves: not those a container places, and not one inside
/// another selected one (the parent carries it).
fn movable(editor: &Editor) -> Vec<NodeHandle> {
    let tree = editor.tree();
    editor
        .selected_nodes()
        .into_iter()
        .filter(|&n| editor.is_editable(n) && tree.component::<Control>(n).is_some())
        .filter(|&n| {
            !tree.parent(n).is_some_and(|p| {
                matches!(
                    tree.class_name(p),
                    Some(class::VBOX_CONTAINER | class::HBOX_CONTAINER)
                )
            })
        })
        .filter(|&n| {
            !std::iter::successors(tree.parent(n), |&p| tree.parent(p))
                .any(|a| editor.is_selected(a))
        })
        .collect()
}

/// The screen's centre lines and title-safe area.
fn guides(painter: &egui::Painter, screen: Rect) {
    let stroke = Stroke::new(1.0, Color32::from_rgba_unmultiplied(90, 170, 255, 70));
    let c = screen.center();
    painter.line_segment(
        [
            Pos2::new(c.x, screen.top()),
            Pos2::new(c.x, screen.bottom()),
        ],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(screen.left(), c.y),
            Pos2::new(screen.right(), c.y),
        ],
        stroke,
    );
    let safe = screen.shrink2(screen.size() * 0.05);
    for (a, b) in [
        (safe.left_top(), safe.right_top()),
        (safe.right_top(), safe.right_bottom()),
        (safe.right_bottom(), safe.left_bottom()),
        (safe.left_bottom(), safe.left_top()),
    ] {
        painter.add(egui::Shape::dashed_line(&[a, b], stroke, 6.0, 4.0));
    }
}

/// A new UI node (canvas layer) at the top of the scene; it is returned (and selected).
fn new_ui(editor: &mut Editor) -> Option<NodeHandle> {
    editor.select(None);
    editor.add_scene(crate::create::UI[0].scene(), None);
    editor
        .selection
        .filter(|&n| editor.tree().component::<CanvasLayer>(n).is_some())
}

/// No UI node yet: offer to make one (returned when made). With `root`, a UI node without
/// controls: offer to add some.
fn empty(
    ui: &mut egui::Ui,
    canvas: Rect,
    editor: &mut Editor,
    root: Option<NodeHandle>,
) -> Option<NodeHandle> {
    let mut made = None;
    let area = Rect::from_center_size(canvas.center(), Vec2::new(360.0, 150.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
        egui::Frame::NONE
            .fill(theme::BG_1.gamma_multiply(0.95))
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(6)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(icon::BROWSERS)
                            .size(30.0)
                            .color(theme::ACCENT),
                    );
                    let hint = if root.is_some() {
                        t("ui_designer.empty_ui")
                    } else {
                        t("ui_designer.empty")
                    };
                    ui.label(RichText::new(hint).color(theme::TEXT_DIM));
                    ui.add_space(6.0);
                    ui.add_enabled_ui(!editor.is_playing(), |ui| {
                        if root.is_some() {
                            ui.menu_button(
                                RichText::new(format!("{}  {}", icon::PLUS, t("ui_designer.add")))
                                    .strong(),
                                |ui| add_menu(ui, editor, root),
                            );
                        } else if ui
                            .button(
                                RichText::new(format!(
                                    "{}  {}",
                                    icon::PLUS,
                                    t("ui_designer.new_ui")
                                ))
                                .strong(),
                            )
                            .clicked()
                        {
                            made = new_ui(editor);
                        }
                    });
                });
            });
    });
    made
}

/// Controls and whole screens to add to the edited UI node `root`: under the selected
/// control when it is inside, else under `root` itself.
fn add_menu(ui: &mut egui::Ui, editor: &mut Editor, root: Option<NodeHandle>) {
    ui.set_min_width(230.0);
    let mut chosen = None;
    // The UI node itself is made with New UI, not added inside another.
    for template in crate::create::UI
        .iter()
        .filter(|t| t.class != class::CANVAS_LAYER)
    {
        let text = format!("{}  {}", theme::class_icon(template.class), template.label);
        if ui.button(text).clicked() {
            chosen = Some(template.scene());
            ui.close();
        }
    }
    let picture = crate::create::image(editor);
    if ui
        .add_enabled(
            picture.is_some(),
            egui::Button::new(format!(
                "{}  {}",
                theme::class_icon("TextureRect"),
                t("create.image")
            )),
        )
        .on_disabled_hover_text(t("create.image_hint"))
        .clicked()
    {
        chosen = picture;
        ui.close();
    }
    ui.separator();
    ui.label(
        RichText::new(t("ui_designer.screens"))
            .small()
            .color(theme::TEXT_DIM),
    );
    for template in crate::create::UI_SCREENS {
        let text = format!("{}  {}", theme::class_icon(template.class), template.label);
        if ui.button(text).clicked() {
            chosen = Some(template.scene());
            ui.close();
        }
    }
    if let Some(scene) = chosen {
        let inside = editor
            .selection
            .is_some_and(|n| root.is_some() && in_scope(editor, root, n) && is_ui(editor, n));
        if !inside {
            editor.select(root);
        }
        let scene = crate::create::place_ui(editor, scene);
        editor.add_scene(scene, None);
    }
}

/// The interface's nodes as a tree: canvas layers and the controls under them. Only the
/// rows in view are drawn. Returns whether a row was double-clicked (frame it).
fn hierarchy(ui: &mut egui::Ui, editor: &mut Editor, root: Option<NodeHandle>) -> bool {
    let Some(root) = root else {
        crate::panels::empty_hint(ui, icon::BROWSERS, &t("ui_designer.no_controls"));
        return false;
    };
    let rows: Vec<(NodeHandle, usize)> = {
        let tree = editor.tree();
        tree.subtree(root)
            .into_iter()
            .filter(|&n| is_ui(editor, n))
            .map(|n| {
                let depth = std::iter::successors(tree.parent(n), |&p| tree.parent(p))
                    .take_while(|&p| p != tree.parent(root).unwrap_or(editor.scene_root))
                    .filter(|&p| is_ui(editor, p))
                    .count();
                (n, depth)
            })
            .collect()
    };
    if rows.is_empty() {
        crate::panels::empty_hint(ui, icon::BROWSERS, &t("ui_designer.no_controls"));
        return false;
    }
    let mut clicked = None;
    let mut frame = false;
    let height = 22.0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, height, rows.len(), |ui, range| {
            for &(node, depth) in &rows[range] {
                let tree = editor.tree();
                let class = tree.class_name(node).unwrap_or("Control");
                let name = tree.name(node).map(|n| n.to_string()).unwrap_or_default();
                let visible = tree.is_visible_in_tree(node);
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
                if editor.is_selected(node) {
                    ui.painter()
                        .rect_filled(rect, 3.0, crate::panels::ROW_SELECTED);
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, 3.0, theme::BG_2);
                }
                let x = rect.left() + 6.0 + depth as f32 * 14.0;
                let (glyph, text) = if visible {
                    (theme::ACCENT, theme::TEXT)
                } else {
                    (theme::TEXT_DIM, theme::TEXT_DIM)
                };
                let font = egui::FontId::proportional(13.0);
                ui.painter().text(
                    Pos2::new(x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    theme::class_icon(class),
                    font.clone(),
                    glyph,
                );
                ui.painter().text(
                    Pos2::new(x + 20.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &name,
                    font,
                    text,
                );
                let class = class.to_owned();
                let response = response.on_hover_text(class);
                if response.clicked() {
                    clicked = Some(node);
                }
                if response.double_clicked() {
                    frame = true;
                }
            }
        });
    if let Some(node) = clicked {
        let modifiers = ui.input(|i| i.modifiers);
        if modifiers.command {
            editor.toggle_selected(node);
        } else if modifiers.shift {
            editor.add_to_selection(&[node]);
        } else {
            editor.select(Some(node));
        }
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nudges_move_only_free_controls() {
        let assets = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let mut editor = Editor::new(assets, None, ab_engine::audio::AudioOutput::Silent);
        editor.select(None);
        let menu = crate::create::place_ui(&mut editor, crate::create::UI_SCREENS[1].scene());
        editor.add_scene(menu, None);
        assert_eq!(layers(&editor).len(), 1, "one UI node, made for the menu");
        let tree = editor.tree();
        let find = |name: &str| {
            tree.subtree(editor.scene_root)
                .into_iter()
                .find(|&n| tree.name(n).map(|s| s.to_string()).as_deref() == Some(name))
                .unwrap()
        };
        let (window, quit) = (find("Window"), find("Quit"));
        editor.set_selection(vec![window, quit], None);
        // The button sits in the window's column: the window carries it.
        assert_eq!(movable(&editor), vec![window]);
    }

    #[test]
    fn each_ui_node_is_edited_on_its_own() {
        let assets = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let mut editor = Editor::new(assets, None, ab_engine::audio::AudioOutput::Silent);
        let mut designer = UiDesigner::default();
        let hud = new_ui(&mut editor).expect("a UI node");
        let menu = new_ui(&mut editor).expect("another");
        assert_eq!(layers(&editor), vec![hud, menu]);

        // Controls are added to the UI node being edited.
        designer.edit(&mut editor, menu);
        editor.add_scene(crate::create::UI[2].scene(), None);
        let control = editor.selection.unwrap();
        assert_eq!(editor.tree().parent(control), Some(menu));
        assert!(in_scope(&editor, Some(menu), control));
        assert!(!in_scope(&editor, Some(hud), control));

        // Only the edited UI is drawn, and nothing changes in the scene for it.
        let revision = editor.revision();
        let shown = with_solo(&mut editor, Some(menu), true, |editor| {
            (
                editor.tree().is_visible_in_tree(hud),
                editor.tree().is_visible_in_tree(menu),
            )
        });
        assert_eq!(shown, (false, true));
        assert!(editor.tree().is_visible_in_tree(hud), "shown again");
        assert_eq!(editor.revision(), revision);

        // A UI node that goes away: the next one is edited.
        editor.delete(menu);
        assert_eq!(designer.root(&editor), Some(hud));
    }
}
