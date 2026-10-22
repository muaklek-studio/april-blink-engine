//! The 3D viewport: renders the scene through the engine's renderer into a texture shown by
//! egui, with an editor camera, grid, icons, selection bounds, picking and transform gizmos.

use crate::state::{Editor, Mode};
use crate::theme;
use ab_engine::core::{Affine3A, Color, EulerRot, Mat4, Quat, Transform3D, Vec3, Vec4};
use ab_engine::physics::{CollisionShape3D, Shape};
use ab_engine::platform::PlatformEvent;
use ab_engine::rendering::{
    Camera3D, CameraOverride, CameraView, Environment, FrameStats, GpuContext, MeshInstance3D,
    OffscreenTarget, RenderAssets, RendererConfig, SceneRenderer,
};
use ab_engine::scene::NodeHandle;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use eframe::egui_wgpu;
use egui_phosphor::regular as icon;

/// Payload of an asset dragged from the asset browser.
#[derive(Debug, Clone)]
pub struct AssetDrag(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoMode {
    Select,
    Translate,
    Rotate,
    Scale,
}

/// Viewport display options.
#[derive(Debug, Clone)]
pub struct ViewOptions {
    pub gizmo: GizmoMode,
    pub snap: bool,
    pub grid: bool,
    pub icons: bool,
    pub collision: bool,
    pub stats: bool,
    /// Look through the editor camera while playing too.
    pub free_camera_in_play: bool,
    /// While editing: brighter ambient light and no fog, so dark levels stay workable.
    pub editor_lighting: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            gizmo: GizmoMode::Translate,
            snap: false,
            grid: true,
            icons: true,
            collision: false,
            stats: true,
            free_camera_in_play: false,
            editor_lighting: true,
        }
    }
}

const TRANSLATE_SNAP: f32 = 0.5;
const ROTATE_SNAP: f32 = 15.0;
const SCALE_SNAP: f32 = 0.1;

/// Orbit/fly camera of the editor.
#[derive(Debug, Clone, Copy)]
pub struct EditorCamera {
    pub pivot: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov: f32,
    pub fly_speed: f32,
}

impl Default for EditorCamera {
    fn default() -> Self {
        Self {
            pivot: Vec3::new(0.0, 0.5, 0.0),
            yaw: 0.65,
            pitch: -0.42,
            distance: 14.0,
            fov: 60.0,
            fly_speed: 6.0,
        }
    }
}

impl EditorCamera {
    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }

    pub fn position(&self) -> Vec3 {
        self.pivot + self.rotation() * Vec3::Z * self.distance
    }

    pub fn global(&self) -> Affine3A {
        Affine3A::from_rotation_translation(self.rotation(), self.position())
    }

    /// Turns around the camera's own position (fly look).
    fn look(&mut self, dyaw: f32, dpitch: f32) {
        let eye = self.position();
        self.yaw += dyaw;
        self.pitch = (self.pitch + dpitch).clamp(-1.55, 1.55);
        self.pivot = eye - self.rotation() * Vec3::Z * self.distance;
    }

    /// Turns around the pivot.
    fn orbit(&mut self, dyaw: f32, dpitch: f32) {
        self.yaw += dyaw;
        self.pitch = (self.pitch + dpitch).clamp(-1.55, 1.55);
    }

    fn pan(&mut self, dx: f32, dy: f32) {
        let rotation = self.rotation();
        let scale = self.distance * 0.0015;
        self.pivot += (rotation * Vec3::NEG_X * dx + rotation * Vec3::Y * dy) * scale;
    }

    fn zoom(&mut self, steps: f32) {
        self.distance = (self.distance * (1.0 - steps * 0.12)).clamp(0.2, 2000.0);
    }

    /// Looks at a sphere around `center`.
    pub fn frame(&mut self, center: Vec3, radius: f32) {
        self.pivot = center;
        let half_fov = (self.fov.to_radians() * 0.5).max(0.1);
        self.distance = (radius.max(0.5) / half_fov.sin() * 1.4).clamp(2.5, 500.0);
    }
}

/// Projection of the frame's view into the viewport rectangle.
#[derive(Debug, Clone, Copy)]
struct Projector {
    view_projection: Mat4,
    inverse: Mat4,
    eye: Vec3,
    rect: Rect,
}

impl Projector {
    fn new(view: &CameraView, rect: Rect) -> Self {
        Self {
            view_projection: view.view_projection,
            inverse: view.view_projection.inverse(),
            eye: view.position,
            rect,
        }
    }

    fn clip(&self, p: Vec3) -> Vec4 {
        self.view_projection * p.extend(1.0)
    }

    fn screen_of(&self, clip: Vec4) -> Pos2 {
        let ndc = clip.truncate() / clip.w;
        Pos2::new(
            self.rect.min.x + (ndc.x + 1.0) * 0.5 * self.rect.width(),
            self.rect.min.y + (1.0 - ndc.y) * 0.5 * self.rect.height(),
        )
    }

    fn project(&self, p: Vec3) -> Option<Pos2> {
        let clip = self.clip(p);
        (clip.w > 1e-3).then(|| self.screen_of(clip))
    }

    /// A world segment clipped to the near side of the camera.
    fn segment(&self, a: Vec3, b: Vec3) -> Option<[Pos2; 2]> {
        const NEAR: f32 = 1e-3;
        let (mut ca, mut cb) = (self.clip(a), self.clip(b));
        if ca.w < NEAR && cb.w < NEAR {
            return None;
        }
        if ca.w < NEAR {
            ca = ca.lerp(cb, (NEAR - ca.w) / (cb.w - ca.w));
        } else if cb.w < NEAR {
            cb = cb.lerp(ca, (NEAR - cb.w) / (ca.w - cb.w));
        }
        Some([self.screen_of(ca), self.screen_of(cb)])
    }

    /// World ray (origin, unit direction) through a screen point.
    fn ray(&self, pos: Pos2) -> (Vec3, Vec3) {
        let x = (pos.x - self.rect.min.x) / self.rect.width() * 2.0 - 1.0;
        let y = 1.0 - (pos.y - self.rect.min.y) / self.rect.height() * 2.0;
        let far = self.inverse * Vec4::new(x, y, 0.5, 1.0);
        let point = far.truncate() / far.w;
        (self.eye, (point - self.eye).normalize_or(Vec3::NEG_Z))
    }

    /// Screen pixels per world unit at `p`.
    fn pixels_per_unit(&self, p: Vec3, axis: Vec3) -> f32 {
        match (self.project(p), self.project(p + axis)) {
            (Some(a), Some(b)) => (b - a).length().max(1e-3),
            _ => 1.0,
        }
    }
}

/// Renders the engine's scene into a texture egui can draw.
pub struct ViewportRenderer {
    gpu: GpuContext,
    renderer: SceneRenderer,
    target: Option<OffscreenTarget>,
    texture: Option<egui::TextureId>,
    pub stats: FrameStats,
}

impl ViewportRenderer {
    pub fn new(state: &egui_wgpu::RenderState) -> Self {
        let gpu = GpuContext::from_shared(
            state.adapter.clone(),
            state.device.clone(),
            state.queue.clone(),
        );
        let config = RendererConfig {
            vsync: false,
            ..Default::default()
        };
        let renderer = SceneRenderer::new(&gpu, OffscreenTarget::FORMAT, &config);
        Self {
            gpu,
            renderer,
            target: None,
            texture: None,
            stats: FrameStats::default(),
        }
    }

    fn render(
        &mut self,
        state: &egui_wgpu::RenderState,
        world: &ab_engine::ecs::World,
        width: u32,
        height: u32,
    ) -> egui::TextureId {
        let (width, height) = (width.clamp(1, 8192), height.clamp(1, 8192));
        if self.target.as_ref().map(OffscreenTarget::size) != Some((width, height)) {
            let target = OffscreenTarget::new(&self.gpu, width, height);
            let mut egui_renderer = state.renderer.write();
            match self.texture {
                Some(id) => egui_renderer.update_egui_texture_from_wgpu_texture(
                    &state.device,
                    target.view(),
                    wgpu::FilterMode::Linear,
                    id,
                ),
                None => {
                    self.texture = Some(egui_renderer.register_native_texture(
                        &state.device,
                        target.view(),
                        wgpu::FilterMode::Linear,
                    ));
                }
            }
            self.target = Some(target);
        }
        let target = self.target.as_ref().expect("created above");
        ab_engine::ui::prepare_ui(world, width, height);
        self.stats = self
            .renderer
            .render(&self.gpu, world, target.view(), width, height);
        self.texture.expect("registered above")
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Handle {
    Axis(usize),
    /// Screen-plane move / uniform scale.
    Center,
}

#[derive(Debug, Clone, Copy)]
struct GizmoDrag {
    node: NodeHandle,
    handle: Handle,
    start_mouse: Pos2,
    start_local: Transform3D,
    start_global: Affine3A,
    /// Unit screen direction of the dragged axis.
    axis_screen: Vec2,
    pixels_per_unit: f32,
    start_angle: f32,
}

/// Per-viewport interaction state.
pub struct Viewport {
    pub camera: EditorCamera,
    pub options: ViewOptions,
    renderer: Option<ViewportRenderer>,
    drag: Option<GizmoDrag>,
    /// Play mode: the game receives keyboard and mouse input.
    pub game_focus: bool,
    cursor_grabbed: bool,
    /// Input went to the game since it last had focus.
    forwarding: bool,
    last_modifiers: egui::Modifiers,
    /// Image size in physical pixels, for the game's cursor coordinates.
    pixels_per_point: f32,
}

impl Viewport {
    pub fn new(state: Option<&egui_wgpu::RenderState>) -> Self {
        Self {
            camera: EditorCamera::default(),
            options: ViewOptions::default(),
            renderer: state.map(ViewportRenderer::new),
            drag: None,
            game_focus: false,
            cursor_grabbed: false,
            forwarding: false,
            last_modifiers: egui::Modifiers::NONE,
            pixels_per_point: 1.0,
        }
    }

    pub fn stats(&self) -> FrameStats {
        self.renderer.as_ref().map(|r| r.stats).unwrap_or_default()
    }

    /// Frames the selection (or the whole scene).
    pub fn frame_selection(&mut self, editor: &Editor) {
        let node = editor.selection.unwrap_or(editor.scene_root);
        let (center, radius) = match subtree_bounds(editor, node) {
            Some((min, max)) => ((min + max) * 0.5, (max - min).length() * 0.5),
            None => (
                editor.tree().global_position(node).unwrap_or(Vec3::ZERO),
                1.0,
            ),
        };
        self.camera.frame(center, radius);
    }

    /// The editor camera while editing, when asked to while playing, and when the game has
    /// no camera of its own (otherwise there would be nothing to see).
    fn uses_editor_camera(&self, editor: &Editor) -> bool {
        let game_has_camera = || {
            editor
                .tree()
                .world()
                .storage::<Camera3D>()
                .is_some_and(|cameras| !cameras.is_empty())
        };
        !editor.is_playing() || self.options.free_camera_in_play || !game_has_camera()
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        render_state: Option<&egui_wgpu::RenderState>,
    ) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        self.pixels_per_point = ui.ctx().pixels_per_point();
        let ppp = self.pixels_per_point;
        let (width, height) = (
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        );

        let editor_camera = self.uses_editor_camera(editor);
        if editor_camera {
            self.navigate(ui, &response, editor);
        }
        let camera = Camera3D {
            fov_y_degrees: self.camera.fov,
            near: 0.05,
            far: 2000.0,
            current: true,
        };
        {
            let world = editor.tree_mut().world_mut();
            if editor_camera {
                world.insert_resource(CameraOverride {
                    camera,
                    global: self.camera.global(),
                });
            } else {
                world.remove_resource::<CameraOverride>();
            }
        }

        // The scene image.
        let painter = ui.painter_at(rect);
        match (&mut self.renderer, render_state) {
            (Some(renderer), Some(state)) => {
                let lit = self.options.editor_lighting && editor.mode == Mode::Edit;
                let saved = lit.then(|| brighten(editor.tree().world())).flatten();
                let start = std::time::Instant::now();
                let texture = renderer.render(state, editor.tree().world(), width, height);
                let world = editor.tree_mut().world_mut();
                if let Some(environment) = saved {
                    world.insert_resource(environment);
                }
                world.insert_resource(ab_engine::RenderStats(renderer.stats));
                if let Some(mut profiler) = world.resource_mut::<ab_engine::Profiler>() {
                    profiler.record("render", start.elapsed());
                }
                painter.image(
                    texture,
                    rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            _ => {
                painter.rect_filled(rect, 0.0, theme::BG_0);
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "No GPU renderer",
                    egui::FontId::proportional(16.0),
                    theme::TEXT_DIM,
                );
            }
        }

        let view = CameraView::new(
            &self.camera.global(),
            camera.fov_y_degrees.to_radians(),
            rect.width() / rect.height().max(1.0),
            camera.near,
            camera.far,
        );
        let projector = Projector::new(&view, rect);

        if editor_camera {
            if self.options.grid {
                draw_grid(&painter, &projector, &self.camera);
            }
            if self.options.collision {
                draw_collision(&painter, &projector, editor);
            }
            if self.options.icons && !editor.is_playing() {
                draw_icons(&painter, &projector, editor);
            }
            if let Some(selected) = editor.selection {
                draw_selection(&painter, &projector, editor, selected);
            }
        }

        let mut gizmo_hovered = false;
        if editor.mode == Mode::Edit {
            gizmo_hovered = self.gizmo(ui, &response, &painter, &projector, editor);
            if response.clicked() && !gizmo_hovered {
                if let Some(pos) = response.interact_pointer_pos() {
                    editor.selection = pick(editor, &projector, pos);
                }
            }
            if let Some(path) = response.dnd_release_payload::<AssetDrag>() {
                let at = response
                    .hover_pos()
                    .map(|p| ground_point(&projector, p))
                    .unwrap_or(self.camera.pivot);
                crate::app::instance_asset(editor, &path.0, Some(at));
            }
            // A script dropped on an object attaches to it.
            if let Some(script) = response.dnd_release_payload::<crate::scripts::ScriptDrag>() {
                let target = response
                    .hover_pos()
                    .and_then(|p| pick(editor, &projector, p))
                    .filter(|&n| editor.is_editable(n));
                match target {
                    Some(node) => editor.attach_script(node, &script.0),
                    None => editor.fail(format!(
                        "Drop {} on an object (or on a node in the Scene tree)",
                        script.0
                    )),
                }
            }
            if let (Some(_), Some(pos)) = (
                response.dnd_hover_payload::<crate::scripts::ScriptDrag>(),
                response.hover_pos(),
            ) {
                if let Some(node) = pick(editor, &projector, pos) {
                    draw_selection(&painter, &projector, editor, node);
                }
            }
        } else {
            self.forward_input(ui, &response, editor, rect);
        }

        self.overlay(ui, rect, editor);
        if response.hovered() && editor.is_playing() && !self.game_focus {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let _ = gizmo_hovered;
        // Keep animating (effects preview, play mode).
        ui.ctx().request_repaint();
    }

    // =========================================================================================
    // Camera navigation
    // =========================================================================================

    fn navigate(&mut self, ui: &egui::Ui, response: &egui::Response, editor: &Editor) {
        let (delta, modifiers, dt) =
            ui.input(|i| (i.pointer.delta(), i.modifiers, i.stable_dt.min(0.1)));
        let flying = response.dragged_by(egui::PointerButton::Secondary);
        let orbiting = response.dragged_by(egui::PointerButton::Middle) && !modifiers.shift
            || response.dragged_by(egui::PointerButton::Primary) && modifiers.alt;
        let panning = response.dragged_by(egui::PointerButton::Middle) && modifiers.shift;

        if flying {
            self.camera.look(-delta.x * 0.005, -delta.y * 0.005);
            let rotation = self.camera.rotation();
            let mut wish = Vec3::ZERO;
            ui.input(|i| {
                let key = |k| i.key_down(k);
                if key(egui::Key::W) {
                    wish += rotation * Vec3::NEG_Z;
                }
                if key(egui::Key::S) {
                    wish += rotation * Vec3::Z;
                }
                if key(egui::Key::A) {
                    wish += rotation * Vec3::NEG_X;
                }
                if key(egui::Key::D) {
                    wish += rotation * Vec3::X;
                }
                if key(egui::Key::E) {
                    wish += Vec3::Y;
                }
                if key(egui::Key::Q) {
                    wish -= Vec3::Y;
                }
            });
            let boost = if modifiers.shift { 3.0 } else { 1.0 };
            self.camera.pivot += wish.normalize_or_zero() * self.camera.fly_speed * boost * dt;
            // Wheel steps, not the smoothed scroll: one notch, one speed change.
            let scroll: f32 = ui.input(|i| {
                i.events
                    .iter()
                    .map(|e| match e {
                        egui::Event::MouseWheel { delta, .. } => delta.y,
                        _ => 0.0,
                    })
                    .sum()
            });
            if scroll != 0.0 {
                self.camera.fly_speed =
                    (self.camera.fly_speed * (1.0 + scroll.signum() * 0.15)).clamp(0.5, 200.0);
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        } else if orbiting {
            self.camera.orbit(-delta.x * 0.006, -delta.y * 0.006);
            ui.ctx().set_cursor_icon(egui::CursorIcon::AllScroll);
        } else if panning {
            self.camera.pan(delta.x, delta.y);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if response.hovered() && !flying {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom(scroll / 50.0);
            }
        }
        if response.hovered() && !flying && editor.mode == Mode::Edit {
            let focus = ui.input(|i| i.key_pressed(egui::Key::F) && i.modifiers.is_none());
            if focus && !ui.ctx().egui_wants_keyboard_input() {
                self.frame_selection(editor);
            }
        }
    }

    // =========================================================================================
    // Gizmo
    // =========================================================================================

    /// Draws and drives the transform gizmo. Returns whether the pointer is on a handle.
    fn gizmo(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        painter: &egui::Painter,
        projector: &Projector,
        editor: &mut Editor,
    ) -> bool {
        let mode = self.options.gizmo;
        let Some(node) = editor.selection.filter(|&n| {
            mode != GizmoMode::Select && n != editor.scene_root && editor.is_editable(n)
        }) else {
            self.drag = None;
            return false;
        };
        let tree = editor.tree();
        let (Some(global), Some(local)) = (tree.global_transform(node), tree.transform(node))
        else {
            return false;
        };
        let origin: Vec3 = global.translation.into();
        let Some(center) = projector.project(origin) else {
            return false;
        };
        let size = projector.eye.distance(origin).max(0.1) * 0.16;
        let axes: [Vec3; 3] = match mode {
            // Scale works along the node's own axes.
            GizmoMode::Scale => [
                Vec3::from(global.matrix3.x_axis).normalize_or(Vec3::X),
                Vec3::from(global.matrix3.y_axis).normalize_or(Vec3::Y),
                Vec3::from(global.matrix3.z_axis).normalize_or(Vec3::Z),
            ],
            _ => [Vec3::X, Vec3::Y, Vec3::Z],
        };
        let colors = [theme::AXIS_X, theme::AXIS_Y, theme::AXIS_Z];
        let pointer = ui.input(|i| i.pointer.hover_pos());

        // Hover test.
        let mut hovered = None;
        if let Some(p) = pointer.filter(|_| self.drag.is_none()) {
            let mut best = 9.0_f32;
            if mode != GizmoMode::Rotate && (p - center).length() < 9.0 {
                hovered = Some(Handle::Center);
                best = 0.0;
            }
            for (i, axis) in axes.iter().enumerate() {
                let distance = match mode {
                    GizmoMode::Rotate => ring(projector, origin, *axis, size)
                        .windows(2)
                        .map(|w| distance_to_segment(p, w[0], w[1]))
                        .fold(f32::MAX, f32::min),
                    _ => projector
                        .segment(origin, origin + *axis * size)
                        .map_or(f32::MAX, |[a, b]| distance_to_segment(p, a, b)),
                };
                if distance < best {
                    best = distance;
                    hovered = Some(Handle::Axis(i));
                }
            }
        }

        // Start a drag.
        if response.drag_started_by(egui::PointerButton::Primary) && !ui.input(|i| i.modifiers.alt)
        {
            if let (Some(handle), Some(start)) = (hovered, response.interact_pointer_pos()) {
                let (axis_screen, pixels_per_unit) = match handle {
                    Handle::Axis(i) => {
                        let tip = projector.project(origin + axes[i]).unwrap_or(center);
                        let dir = tip - center;
                        (dir.normalized(), dir.length().max(1e-3))
                    }
                    Handle::Center => (Vec2::X, projector.pixels_per_unit(origin, Vec3::X)),
                };
                let to_mouse = start - center;
                self.drag = Some(GizmoDrag {
                    node,
                    handle,
                    start_mouse: start,
                    start_local: local,
                    start_global: global,
                    axis_screen,
                    pixels_per_unit,
                    start_angle: to_mouse.y.atan2(to_mouse.x),
                });
            }
        }

        // Continue it.
        let snap = self.options.snap || ui.input(|i| i.modifiers.command);
        if let Some(drag) = self.drag.filter(|d| d.node == node) {
            if response.dragged_by(egui::PointerButton::Primary) {
                if let Some(mouse) = response.interact_pointer_pos() {
                    self.apply_drag(&drag, mouse, center, projector, &axes, snap, editor);
                }
            } else {
                self.drag = None;
            }
        } else {
            self.drag = None;
        }

        // Draw.
        let active = self.drag.map(|d| d.handle).or(hovered);
        let highlight = |handle: Handle, color: Color32| {
            if active == Some(handle) {
                Color32::from_rgb(255, 230, 90)
            } else {
                color
            }
        };
        match mode {
            GizmoMode::Translate | GizmoMode::Scale => {
                for (i, axis) in axes.iter().enumerate() {
                    let Some([a, b]) = projector.segment(origin, origin + *axis * size) else {
                        continue;
                    };
                    let color = highlight(Handle::Axis(i), colors[i]);
                    painter.line_segment([a, b], Stroke::new(3.0_f32, color));
                    let dir = (b - a).normalized();
                    if mode == GizmoMode::Translate {
                        let normal = Vec2::new(-dir.y, dir.x);
                        painter.add(egui::Shape::convex_polygon(
                            vec![b + dir * 12.0, b + normal * 5.5, b - normal * 5.5],
                            color,
                            Stroke::NONE,
                        ));
                    } else {
                        painter.rect_filled(
                            Rect::from_center_size(b, Vec2::splat(9.0)),
                            1.0,
                            color,
                        );
                    }
                }
                painter.rect_filled(
                    Rect::from_center_size(center, Vec2::splat(10.0)),
                    2.0,
                    highlight(Handle::Center, Color32::from_white_alpha(200)),
                );
            }
            GizmoMode::Rotate => {
                for (i, axis) in axes.iter().enumerate() {
                    let points = ring(projector, origin, *axis, size);
                    let color = highlight(Handle::Axis(i), colors[i]);
                    painter.add(egui::Shape::line(points, Stroke::new(2.5_f32, color)));
                }
                painter.circle_stroke(center, 3.0, Stroke::new(1.5_f32, Color32::WHITE));
            }
            GizmoMode::Select => {}
        }
        hovered.is_some() || self.drag.is_some()
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_drag(
        &self,
        drag: &GizmoDrag,
        mouse: Pos2,
        center: Pos2,
        projector: &Projector,
        axes: &[Vec3; 3],
        snap: bool,
        editor: &mut Editor,
    ) {
        let delta = mouse - drag.start_mouse;
        let start_origin: Vec3 = drag.start_global.translation.into();
        match (self.options.gizmo, drag.handle) {
            (GizmoMode::Translate, handle) => {
                let offset = match handle {
                    Handle::Axis(i) => {
                        let mut units = delta.dot(drag.axis_screen) / drag.pixels_per_unit;
                        if snap {
                            units = (units / TRANSLATE_SNAP).round() * TRANSLATE_SNAP;
                        }
                        axes[i] * units
                    }
                    Handle::Center => {
                        let rotation = self.camera.rotation();
                        let scale = 1.0 / drag.pixels_per_unit;
                        let mut offset =
                            (rotation * Vec3::X * delta.x - rotation * Vec3::Y * delta.y) * scale;
                        if snap {
                            offset = (offset / TRANSLATE_SNAP).round() * TRANSLATE_SNAP;
                        }
                        offset
                    }
                };
                let mut global = drag.start_global;
                global.translation = (start_origin + offset).into();
                editor.set_global_transform(drag.node, global);
            }
            (GizmoMode::Rotate, Handle::Axis(i)) => {
                let to_mouse = mouse - center;
                let mut angle = to_mouse.y.atan2(to_mouse.x) - drag.start_angle;
                // Screen y points down: on-screen counter-clockwise is a negative angle here.
                let facing = axes[i].dot(projector.eye - start_origin) > 0.0;
                if facing {
                    angle = -angle;
                }
                if snap {
                    let step = ROTATE_SNAP.to_radians();
                    angle = (angle / step).round() * step;
                }
                let rotation = Affine3A::from_translation(start_origin)
                    * Affine3A::from_axis_angle(axes[i], angle)
                    * Affine3A::from_translation(-start_origin);
                editor.set_global_transform(drag.node, rotation * drag.start_global);
            }
            (GizmoMode::Scale, handle) => {
                let mut transform = drag.start_local;
                match handle {
                    Handle::Axis(i) => {
                        let mut factor =
                            (1.0 + delta.dot(drag.axis_screen) / drag.pixels_per_unit).max(0.01);
                        let mut value = drag.start_local.scale[i] * factor;
                        if snap {
                            value = ((value / SCALE_SNAP).round() * SCALE_SNAP).max(SCALE_SNAP);
                            factor = value / drag.start_local.scale[i];
                        }
                        let _ = factor;
                        transform.scale[i] = value;
                    }
                    Handle::Center => {
                        let mut factor = (1.0 + (delta.x - delta.y) / 120.0).max(0.01);
                        if snap {
                            factor = ((factor / SCALE_SNAP).round() * SCALE_SNAP).max(SCALE_SNAP);
                        }
                        transform.scale = drag.start_local.scale * factor;
                    }
                }
                editor.set_transform(drag.node, transform);
            }
            _ => {}
        }
    }

    // =========================================================================================
    // Overlay
    // =========================================================================================

    fn overlay(&mut self, ui: &mut egui::Ui, rect: Rect, editor: &mut Editor) {
        let bar = Rect::from_min_size(
            rect.min + Vec2::new(8.0, 8.0),
            Vec2::new(rect.width() - 16.0, 30.0),
        );
        // While playing, the game's own HUD owns the corners (options stay in the View menu).
        let show_bar = !editor.is_playing() || self.options.free_camera_in_play;
        let builder = if show_bar {
            egui::UiBuilder::new()
        } else {
            egui::UiBuilder::new().invisible()
        };
        ui.scope_builder(builder, |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
                ui.horizontal(|ui| {
                    if editor.mode == Mode::Edit {
                        egui::Frame::NONE
                            .fill(theme::BG_1.gamma_multiply(0.92))
                            .corner_radius(6)
                            .inner_margin(egui::Margin::symmetric(4, 2))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 2.0;
                                    for (mode, glyph, tip) in [
                                        (GizmoMode::Select, icon::CURSOR, "Select (Q)"),
                                        (
                                            GizmoMode::Translate,
                                            icon::ARROWS_OUT_CARDINAL,
                                            "Move (W)",
                                        ),
                                        (GizmoMode::Rotate, icon::ARROWS_CLOCKWISE, "Rotate (E)"),
                                        (GizmoMode::Scale, icon::ARROWS_OUT, "Scale (R)"),
                                    ] {
                                        ui.selectable_value(&mut self.options.gizmo, mode, glyph)
                                            .on_hover_text(tip);
                                    }
                                    ui.separator();
                                    ui.toggle_value(&mut self.options.snap, icon::MAGNET)
                                        .on_hover_text("Snap (hold Ctrl): 0.5 m, 15°, 0.1");
                                });
                            });
                    }
                    egui::Frame::NONE
                        .fill(theme::BG_1.gamma_multiply(0.92))
                        .corner_radius(6)
                        .inner_margin(egui::Margin::symmetric(4, 2))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                ui.menu_button(format!("{} View", icon::EYE), |ui| {
                                    ui.checkbox(&mut self.options.grid, "Grid");
                                    ui.checkbox(&mut self.options.icons, "Node icons");
                                    ui.checkbox(&mut self.options.collision, "Collision shapes");
                                    ui.checkbox(&mut self.options.stats, "Statistics");
                                    ui.checkbox(
                                        &mut self.options.editor_lighting,
                                        "Editor lighting (bright, no fog)",
                                    );
                                    ui.checkbox(&mut editor.preview_effects, "Preview effects");
                                    ui.checkbox(
                                        &mut self.options.free_camera_in_play,
                                        "Editor camera while playing",
                                    );
                                    ui.separator();
                                    ui.add(
                                        egui::Slider::new(&mut self.camera.fov, 20.0..=110.0)
                                            .text("Field of view"),
                                    );
                                    ui.add(
                                        egui::Slider::new(&mut self.camera.fly_speed, 0.5..=100.0)
                                            .logarithmic(true)
                                            .text("Fly speed"),
                                    );
                                    if ui.button("Reset camera").clicked() {
                                        self.camera = EditorCamera::default();
                                        ui.close();
                                    }
                                });
                            });
                        });
                });
            })
        });

        if self.options.stats {
            let stats = self.stats();
            let fps = ui.ctx().input(|i| 1.0 / i.stable_dt.max(1e-4));
            let text = format!(
                "{fps:.0} FPS\n{} objects · {} draws · {} culled\n{} lights · {} particles",
                stats.objects, stats.draw_calls, stats.culled, stats.lights, stats.particles
            );
            let painter = ui.painter_at(rect);
            let galley = painter.layout(
                text,
                egui::FontId::monospace(11.5),
                Color32::from_gray(210),
                220.0,
            );
            let pos = Pos2::new(rect.max.x - galley.size().x - 16.0, rect.min.y + 12.0);
            painter.rect_filled(
                Rect::from_min_size(pos - Vec2::splat(6.0), galley.size() + Vec2::splat(12.0)),
                6.0,
                theme::BG_1.gamma_multiply(0.85),
            );
            painter.galley(pos, galley, Color32::WHITE);
        }

        if editor.is_playing() {
            let painter = ui.painter_at(rect);
            let color = match editor.mode {
                Mode::Play { paused: true } => theme::WARN,
                _ => theme::PLAY,
            };
            painter.rect_stroke(
                rect.shrink(1.0),
                0.0,
                Stroke::new(2.0_f32, color),
                egui::StrokeKind::Inside,
            );
            if !self.game_focus {
                let galley = painter.layout_no_wrap(
                    format!(
                        "{}  Click to control the game · Esc releases the mouse",
                        icon::MOUSE
                    ),
                    egui::FontId::proportional(12.5),
                    Color32::from_white_alpha(220),
                );
                let pos = rect.right_bottom() - galley.size() - Vec2::new(18.0, 16.0);
                painter.rect_filled(
                    Rect::from_min_size(
                        pos - Vec2::new(8.0, 5.0),
                        galley.size() + Vec2::new(16.0, 10.0),
                    ),
                    12.0,
                    theme::BG_0.gamma_multiply(0.85),
                );
                painter.galley(pos, galley, Color32::WHITE);
            }
        }
    }

    // =========================================================================================
    // Play-mode input
    // =========================================================================================

    fn forward_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        editor: &mut Editor,
        rect: Rect,
    ) {
        if response.clicked() || response.drag_started() {
            self.game_focus = true;
            response.request_focus();
        }
        if ui.input(|i| i.pointer.any_pressed()) && !response.hovered() {
            self.game_focus = false;
        }
        let wants_capture = editor
            .tree()
            .world()
            .resource::<ab_engine::EngineControl>()
            .is_some_and(|c| c.is_mouse_captured());
        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if escape && self.cursor_grabbed {
            self.game_focus = false;
        }
        self.set_cursor_grab(ui.ctx(), wants_capture && self.game_focus);
        if !self.game_focus {
            self.release_keys(editor);
            return;
        }
        self.forwarding = true;

        let ppp = self.pixels_per_point;
        let events = ui.input(|i| i.events.clone());
        let modifiers = ui.input(|i| i.modifiers);
        for (was, now, code) in [
            (
                self.last_modifiers.shift,
                modifiers.shift,
                KeyCode::ShiftLeft,
            ),
            (
                self.last_modifiers.ctrl,
                modifiers.ctrl,
                KeyCode::ControlLeft,
            ),
            (self.last_modifiers.alt, modifiers.alt, KeyCode::AltLeft),
        ] {
            if was != now {
                editor.engine.handle_event(&PlatformEvent::Key {
                    code,
                    pressed: now,
                    repeat: false,
                });
            }
        }
        self.last_modifiers = modifiers;
        for event in events {
            let platform = match event {
                egui::Event::Key {
                    key,
                    physical_key,
                    pressed,
                    repeat,
                    ..
                } => key_code(physical_key.unwrap_or(key)).map(|code| PlatformEvent::Key {
                    code,
                    pressed,
                    repeat,
                }),
                egui::Event::PointerButton {
                    button, pressed, ..
                } => mouse_button(button)
                    .map(|button| PlatformEvent::MouseButton { button, pressed }),
                egui::Event::PointerMoved(pos) => Some(PlatformEvent::CursorMoved {
                    x: ((pos.x - rect.min.x) * ppp) as f64,
                    y: ((pos.y - rect.min.y) * ppp) as f64,
                }),
                egui::Event::MouseMoved(delta) => Some(PlatformEvent::MouseMotion {
                    dx: delta.x as f64,
                    dy: delta.y as f64,
                }),
                egui::Event::MouseWheel { delta, unit, .. } => {
                    let lines = match unit {
                        egui::MouseWheelUnit::Line => delta,
                        egui::MouseWheelUnit::Point => delta / 40.0,
                        egui::MouseWheelUnit::Page => delta * 10.0,
                    };
                    Some(PlatformEvent::MouseWheel {
                        dx: lines.x,
                        dy: lines.y,
                    })
                }
                _ => None,
            };
            if let Some(platform) = platform {
                editor.engine.handle_event(&platform);
            }
        }
    }

    /// Releases held keys when the game loses focus, so nothing stays pressed.
    fn release_keys(&mut self, editor: &mut Editor) {
        if !self.forwarding {
            return;
        }
        self.forwarding = false;
        self.last_modifiers = egui::Modifiers::NONE;
        editor
            .tree_mut()
            .world_mut()
            .insert_resource(ab_engine::input::InputState::new());
    }

    pub fn set_cursor_grab(&mut self, ctx: &egui::Context, grab: bool) {
        if grab == self.cursor_grabbed {
            return;
        }
        self.cursor_grabbed = grab;
        let mode = if grab {
            egui::CursorGrab::Locked
        } else {
            egui::CursorGrab::None
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(mode));
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(!grab));
    }
}

use ab_engine::input::{KeyCode, MouseButton};

fn mouse_button(button: egui::PointerButton) -> Option<MouseButton> {
    Some(match button {
        egui::PointerButton::Primary => MouseButton::Left,
        egui::PointerButton::Secondary => MouseButton::Right,
        egui::PointerButton::Middle => MouseButton::Middle,
        egui::PointerButton::Extra1 => MouseButton::Back,
        egui::PointerButton::Extra2 => MouseButton::Forward,
    })
}

/// Physical key for an egui key (US layout positions).
pub fn key_code(key: egui::Key) -> Option<KeyCode> {
    use egui::Key as K;
    Some(match key {
        K::A => KeyCode::KeyA,
        K::B => KeyCode::KeyB,
        K::C => KeyCode::KeyC,
        K::D => KeyCode::KeyD,
        K::E => KeyCode::KeyE,
        K::F => KeyCode::KeyF,
        K::G => KeyCode::KeyG,
        K::H => KeyCode::KeyH,
        K::I => KeyCode::KeyI,
        K::J => KeyCode::KeyJ,
        K::K => KeyCode::KeyK,
        K::L => KeyCode::KeyL,
        K::M => KeyCode::KeyM,
        K::N => KeyCode::KeyN,
        K::O => KeyCode::KeyO,
        K::P => KeyCode::KeyP,
        K::Q => KeyCode::KeyQ,
        K::R => KeyCode::KeyR,
        K::S => KeyCode::KeyS,
        K::T => KeyCode::KeyT,
        K::U => KeyCode::KeyU,
        K::V => KeyCode::KeyV,
        K::W => KeyCode::KeyW,
        K::X => KeyCode::KeyX,
        K::Y => KeyCode::KeyY,
        K::Z => KeyCode::KeyZ,
        K::Num0 => KeyCode::Digit0,
        K::Num1 => KeyCode::Digit1,
        K::Num2 => KeyCode::Digit2,
        K::Num3 => KeyCode::Digit3,
        K::Num4 => KeyCode::Digit4,
        K::Num5 => KeyCode::Digit5,
        K::Num6 => KeyCode::Digit6,
        K::Num7 => KeyCode::Digit7,
        K::Num8 => KeyCode::Digit8,
        K::Num9 => KeyCode::Digit9,
        K::ArrowUp => KeyCode::ArrowUp,
        K::ArrowDown => KeyCode::ArrowDown,
        K::ArrowLeft => KeyCode::ArrowLeft,
        K::ArrowRight => KeyCode::ArrowRight,
        K::Space => KeyCode::Space,
        K::Enter => KeyCode::Enter,
        K::Escape => KeyCode::Escape,
        K::Tab => KeyCode::Tab,
        K::Backspace => KeyCode::Backspace,
        K::Delete => KeyCode::Delete,
        K::Insert => KeyCode::Insert,
        K::Home => KeyCode::Home,
        K::End => KeyCode::End,
        K::PageUp => KeyCode::PageUp,
        K::PageDown => KeyCode::PageDown,
        K::Minus => KeyCode::Minus,
        K::Equals | K::Plus => KeyCode::Equal,
        K::Comma => KeyCode::Comma,
        K::Period => KeyCode::Period,
        K::Slash => KeyCode::Slash,
        K::Backslash => KeyCode::Backslash,
        K::Semicolon => KeyCode::Semicolon,
        K::Quote => KeyCode::Quote,
        K::OpenBracket => KeyCode::BracketLeft,
        K::CloseBracket => KeyCode::BracketRight,
        K::Backtick => KeyCode::Backquote,
        K::F1 => KeyCode::F1,
        K::F2 => KeyCode::F2,
        K::F3 => KeyCode::F3,
        K::F4 => KeyCode::F4,
        K::F5 => KeyCode::F5,
        K::F6 => KeyCode::F6,
        K::F7 => KeyCode::F7,
        K::F8 => KeyCode::F8,
        K::F9 => KeyCode::F9,
        K::F10 => KeyCode::F10,
        K::F11 => KeyCode::F11,
        K::F12 => KeyCode::F12,
        _ => return None,
    })
}

/// Brightens the scene's environment for editing; returns the original to restore.
fn brighten(world: &ab_engine::ecs::World) -> Option<Environment> {
    let mut environment = world.resource_mut::<Environment>()?;
    let original = environment.clone();
    environment.ambient_color = Color::rgb(0.85, 0.88, 0.95);
    environment.ambient_energy = environment.ambient_energy.max(0.7);
    environment.fog.enabled = false;
    environment.post.exposure = environment.post.exposure.max(1.0);
    Some(original)
}

// =============================================================================================
// Scene queries
// =============================================================================================

/// Union of the world bounds of the meshes under `node`.
pub fn subtree_bounds(editor: &Editor, node: NodeHandle) -> Option<(Vec3, Vec3)> {
    let tree = editor.tree();
    let world = tree.world();
    let assets = world.resource::<RenderAssets>()?;
    let mut bounds: Option<(Vec3, Vec3)> = None;
    for n in tree.subtree(node) {
        let Some(mesh) = tree.component::<MeshInstance3D>(n).map(|m| *m) else {
            continue;
        };
        let (Some(aabb), Some(global)) = (assets.mesh_aabb(mesh.mesh), tree.global_transform(n))
        else {
            continue;
        };
        let aabb = aabb.transformed(&global);
        bounds = Some(match bounds {
            Some((min, max)) => (min.min(aabb.min), max.max(aabb.max)),
            None => (aabb.min, aabb.max),
        });
    }
    bounds
}

/// Ray / box slab test; distance along the ray.
fn ray_aabb(origin: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let inv = dir.recip();
    let t1 = (min - origin) * inv;
    let t2 = (max - origin) * inv;
    let near = t1.min(t2).max_element();
    let far = t1.max(t2).min_element();
    (far >= near.max(0.0)).then_some(near.max(0.0))
}

/// Classes drawn as clickable icons (they have no mesh of their own).
fn has_icon(class: &str) -> bool {
    matches!(
        class,
        "Camera3D"
            | "DirectionalLight3D"
            | "OmniLight3D"
            | "SpotLight3D"
            | "Particles3D"
            | "AudioStreamPlayer3D"
            | "AudioListener3D"
            | "Area3D"
            | "CameraShake"
            | "Joint3D"
    )
}

/// The scene node under a screen point: icons first, then the nearest mesh.
fn pick(editor: &Editor, projector: &Projector, pos: Pos2) -> Option<NodeHandle> {
    let tree = editor.tree();
    let nodes = tree.subtree(editor.scene_root);
    let mut best_icon: Option<(f32, NodeHandle)> = None;
    for &node in &nodes {
        if !tree
            .class_name(node)
            .is_some_and(|c| icon_shown(editor, node, c))
        {
            continue;
        }
        let Some(at) = tree
            .global_position(node)
            .and_then(|p| projector.project(p))
        else {
            continue;
        };
        let distance = (at - pos).length();
        if distance < 14.0 && best_icon.is_none_or(|(d, _)| distance < d) {
            best_icon = Some((distance, node));
        }
    }
    if let Some((_, node)) = best_icon {
        return editor.instance_owner(node);
    }

    let (origin, dir) = projector.ray(pos);
    let assets = tree.world().resource::<RenderAssets>()?;
    let mut best: Option<(f32, NodeHandle)> = None;
    for node in nodes {
        let Some(mesh) = tree.component::<MeshInstance3D>(node).map(|m| *m) else {
            continue;
        };
        if !tree.is_visible_in_tree(node) {
            continue;
        }
        let (Some(aabb), Some(global)) = (assets.mesh_aabb(mesh.mesh), tree.global_transform(node))
        else {
            continue;
        };
        // Test in the mesh's space: tight for rotated meshes, same ray parameter.
        let inverse = global.inverse();
        let local_origin = inverse.transform_point3(origin);
        let local_dir = inverse.transform_vector3(dir);
        if let Some(t) = ray_aabb(local_origin, local_dir, aabb.min, aabb.max) {
            if best.is_none_or(|(d, _)| t < d) {
                best = Some((t, node));
            }
        }
    }
    best.and_then(|(_, node)| editor.instance_owner(node))
}

/// Where a screen point's ray meets the ground plane (or 10 m ahead).
fn ground_point(projector: &Projector, pos: Pos2) -> Vec3 {
    let (origin, dir) = projector.ray(pos);
    if dir.y < -1e-3 {
        let t = -origin.y / dir.y;
        if t < 500.0 {
            return origin + dir * t;
        }
    }
    origin + dir * 10.0
}

// =============================================================================================
// Drawing
// =============================================================================================

fn line(painter: &egui::Painter, projector: &Projector, a: Vec3, b: Vec3, stroke: Stroke) {
    if let Some(points) = projector.segment(a, b) {
        painter.line_segment(points, stroke);
    }
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// Screen polyline of a circle around `axis`.
fn ring(projector: &Projector, center: Vec3, axis: Vec3, radius: f32) -> Vec<Pos2> {
    let u = axis.any_orthonormal_vector();
    let v = axis.cross(u);
    (0..=48)
        .filter_map(|i| {
            let angle = i as f32 / 48.0 * std::f32::consts::TAU;
            projector.project(center + (u * angle.cos() + v * angle.sin()) * radius)
        })
        .collect()
}

fn draw_grid(painter: &egui::Painter, projector: &Projector, camera: &EditorCamera) {
    let step = if camera.distance > 60.0 { 10.0 } else { 1.0 };
    let half = 40;
    let cx = (camera.pivot.x / step).round() * step;
    let cz = (camera.pivot.z / step).round() * step;
    let extent = half as f32 * step;
    for i in -half..=half {
        let i: i32 = i;
        let offset = i as f32 * step;
        let fade = 1.0 - (i.unsigned_abs() as f32 / half as f32);
        let alpha = (fade * fade * 22.0) as u8;
        let major = ((cx + offset) / step).round() as i64 % 10 == 0;
        let color = Color32::from_white_alpha(if major {
            alpha.saturating_mul(2)
        } else {
            alpha
        });
        if alpha == 0 {
            continue;
        }
        let x = cx + offset;
        if x.abs() > 1e-4 {
            line(
                painter,
                projector,
                Vec3::new(x, 0.0, cz - extent),
                Vec3::new(x, 0.0, cz + extent),
                Stroke::new(1.0_f32, color),
            );
        }
        let z = cz + offset;
        if z.abs() > 1e-4 {
            line(
                painter,
                projector,
                Vec3::new(cx - extent, 0.0, z),
                Vec3::new(cx + extent, 0.0, z),
                Stroke::new(1.0_f32, color),
            );
        }
    }
    let axis = |color: Color32| Stroke::new(1.5_f32, color.gamma_multiply(0.75));
    line(
        painter,
        projector,
        Vec3::new(cx - extent, 0.0, 0.0),
        Vec3::new(cx + extent, 0.0, 0.0),
        axis(theme::AXIS_X),
    );
    line(
        painter,
        projector,
        Vec3::new(0.0, 0.0, cz - extent),
        Vec3::new(0.0, 0.0, cz + extent),
        axis(theme::AXIS_Z),
    );
}

/// Whether a node's icon is shown: always, except joints (a ragdoll has many), which show
/// when they or one of their ancestors are selected.
fn icon_shown(editor: &Editor, node: NodeHandle, class: &str) -> bool {
    let tree = editor.tree();
    has_icon(class)
        && tree.is_visible_in_tree(node)
        && (class != "Joint3D"
            || editor
                .selection
                .is_some_and(|s| s == node || tree.is_ancestor_of(s, node)))
}

fn draw_icons(painter: &egui::Painter, projector: &Projector, editor: &Editor) {
    let tree = editor.tree();
    for node in tree.subtree(editor.scene_root) {
        let Some(class) = tree
            .class_name(node)
            .filter(|c| icon_shown(editor, node, c))
        else {
            continue;
        };
        let Some(global) = tree.global_transform(node) else {
            continue;
        };
        let origin: Vec3 = global.translation.into();
        let Some(at) = projector.project(origin) else {
            continue;
        };
        let selected = editor.selection == Some(node);
        let color = if selected {
            theme::SELECTION
        } else {
            theme::class_color(class)
        };
        painter.circle_filled(at, 11.0, theme::BG_0.gamma_multiply(0.75));
        painter.text(
            at,
            egui::Align2::CENTER_CENTER,
            theme::class_icon(class),
            egui::FontId::proportional(14.0),
            color,
        );
        // Direction hints for directional things.
        if matches!(class, "DirectionalLight3D" | "SpotLight3D" | "Camera3D") {
            let forward =
                Vec3::from(global.matrix3 * ab_engine::core::Vec3A::NEG_Z).normalize_or_zero();
            line(
                painter,
                projector,
                origin,
                origin + forward * 1.2,
                Stroke::new(1.5_f32, color.gamma_multiply(0.8)),
            );
        }
        if class == "Camera3D" && selected {
            draw_camera_frustum(painter, projector, &global, editor, node);
        }
    }
}

fn draw_camera_frustum(
    painter: &egui::Painter,
    projector: &Projector,
    global: &Affine3A,
    editor: &Editor,
    node: NodeHandle,
) {
    let Some(camera) = editor.tree().component::<Camera3D>(node).map(|c| *c) else {
        return;
    };
    let depth = 2.0;
    let half_h = (camera.fov_y_degrees.to_radians() * 0.5).tan() * depth;
    let half_w = half_h * 16.0 / 9.0;
    let origin: Vec3 = global.translation.into();
    let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .map(|(x, y)| global.transform_point3(Vec3::new(x * half_w, y * half_h, -depth)));
    let stroke = Stroke::new(1.0_f32, theme::SELECTION.gamma_multiply(0.8));
    for i in 0..4 {
        line(painter, projector, origin, corners[i], stroke);
        line(painter, projector, corners[i], corners[(i + 1) % 4], stroke);
    }
}

fn draw_box(painter: &egui::Painter, projector: &Projector, corners: &[Vec3; 8], stroke: Stroke) {
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (1, 3),
        (3, 2),
        (2, 0),
        (4, 5),
        (5, 7),
        (7, 6),
        (6, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    for (a, b) in EDGES {
        line(painter, projector, corners[a], corners[b], stroke);
    }
}

fn box_corners(transform: &Affine3A, min: Vec3, max: Vec3) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        transform.transform_point3(Vec3::new(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        ))
    })
}

fn draw_selection(
    painter: &egui::Painter,
    projector: &Projector,
    editor: &Editor,
    node: NodeHandle,
) {
    let tree = editor.tree();
    let world = tree.world();
    let Some(assets) = world.resource::<RenderAssets>() else {
        return;
    };
    let stroke = Stroke::new(1.5_f32, theme::SELECTION);
    let mut any = false;
    for n in tree.subtree(node) {
        let Some(mesh) = tree.component::<MeshInstance3D>(n).map(|m| *m) else {
            continue;
        };
        let (Some(aabb), Some(global)) = (assets.mesh_aabb(mesh.mesh), tree.global_transform(n))
        else {
            continue;
        };
        draw_box(
            painter,
            projector,
            &box_corners(&global, aabb.min, aabb.max),
            stroke,
        );
        any = true;
    }
    if !any {
        if let Some(at) = tree
            .global_position(node)
            .and_then(|p| projector.project(p))
        {
            painter.circle_stroke(at, 14.0, stroke);
        }
    }
}

fn draw_collision(painter: &egui::Painter, projector: &Projector, editor: &Editor) {
    let tree = editor.tree();
    let stroke = Stroke::new(1.0_f32, theme::PLAY.gamma_multiply(0.85));
    for node in tree.subtree(editor.scene_root) {
        let Some(shape) = tree
            .component::<CollisionShape3D>(node)
            .map(|c| c.shape().clone())
        else {
            continue;
        };
        let Some(global) = tree.global_transform(node) else {
            continue;
        };
        match shape {
            Shape::Box { size } => {
                draw_box(
                    painter,
                    projector,
                    &box_corners(&global, -size * 0.5, size * 0.5),
                    stroke,
                );
            }
            Shape::Sphere { radius } => {
                let center: Vec3 = global.translation.into();
                let scale = Vec3::from(global.matrix3.x_axis).length();
                for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                    let points = ring(projector, center, axis, radius * scale);
                    painter.add(egui::Shape::line(points, stroke));
                }
            }
            Shape::Capsule { radius, height } | Shape::Cylinder { radius, height } => {
                let half = (height * 0.5).max(radius);
                for y in [-half, half] {
                    let center = global.transform_point3(Vec3::new(0.0, y, 0.0));
                    let axis = Vec3::from(global.matrix3.y_axis).normalize_or(Vec3::Y);
                    painter.add(egui::Shape::line(
                        ring(projector, center, axis, radius),
                        stroke,
                    ));
                }
                for (x, z) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                    line(
                        painter,
                        projector,
                        global.transform_point3(Vec3::new(x * radius, -half, z * radius)),
                        global.transform_point3(Vec3::new(x * radius, half, z * radius)),
                        stroke,
                    );
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_box_hits_and_misses() {
        let (min, max) = (Vec3::splat(-1.0), Vec3::splat(1.0));
        assert_eq!(
            ray_aabb(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z, min, max),
            Some(4.0)
        );
        assert_eq!(
            ray_aabb(Vec3::new(3.0, 0.0, 5.0), Vec3::NEG_Z, min, max),
            None
        );
        assert_eq!(ray_aabb(Vec3::new(0.0, 0.0, 5.0), Vec3::Z, min, max), None);
        assert_eq!(ray_aabb(Vec3::ZERO, Vec3::X, min, max), Some(0.0), "inside");
    }

    #[test]
    fn projection_round_trips_through_rays() {
        let camera = EditorCamera::default();
        let view = CameraView::new(
            &camera.global(),
            60f32.to_radians(),
            16.0 / 9.0,
            0.05,
            2000.0,
        );
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(1600.0, 900.0));
        let projector = Projector::new(&view, rect);
        let target = Vec3::new(1.0, 0.5, -2.0);
        let screen = projector.project(target).unwrap();
        let (origin, dir) = projector.ray(screen);
        let closest = origin + dir * dir.dot(target - origin);
        assert!(closest.distance(target) < 1e-3, "{closest} vs {target}");
        // The pivot sits in the middle of the view.
        let center = projector.project(camera.pivot).unwrap();
        assert!((center - rect.center()).length() < 0.5);
        // Segments behind the camera are clipped, not mirrored.
        let behind = camera.position() + camera.rotation() * Vec3::Z * 5.0;
        assert!(projector.segment(behind, behind + Vec3::X).is_none());
        assert!(projector.segment(behind, camera.pivot).is_some());
    }

    #[test]
    fn framing_fits_the_sphere() {
        let mut camera = EditorCamera::default();
        camera.frame(Vec3::new(5.0, 1.0, 0.0), 2.0);
        assert_eq!(camera.pivot, Vec3::new(5.0, 1.0, 0.0));
        assert!(camera.distance > 4.0);
        // Flying keeps the eye still.
        let eye = camera.position();
        camera.look(0.3, 0.1);
        assert!(camera.position().distance(eye) < 1e-4);
    }

    #[test]
    fn keys_map_to_physical_codes() {
        assert_eq!(key_code(egui::Key::W), Some(KeyCode::KeyW));
        assert_eq!(key_code(egui::Key::Space), Some(KeyCode::Space));
        assert_eq!(key_code(egui::Key::F5), Some(KeyCode::F5));
    }
}
