//! The 3D viewport: renders the scene through the engine's renderer into a texture shown by
//! egui, with an editor camera, grid, icons, selection bounds, picking and transform gizmos.

use crate::i18n::{t, tf};
use crate::settings::SnapSettings;
use crate::state::{Editor, Mode};
use crate::theme;
use ab_engine::core::{Affine3A, Color, EulerRot, Mat4, Quat, Transform3D, Vec3, Vec4};
use ab_engine::physics::{CollisionShape3D, PhysicsBody3D, Shape};
use ab_engine::platform::PlatformEvent;
use ab_engine::rendering::{
    Aabb, Bounds, Camera3D, CameraOverride, CameraView, Environment, FrameStats, GpuContext,
    MeshInstance3D, OffscreenTarget, RenderAssets, RendererConfig, SceneRenderer, Skin,
};
use ab_engine::scene::NodeHandle;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use eframe::egui_wgpu;
use egui_phosphor::regular as icon;

/// Payload of an asset dragged from the asset browser.
#[derive(Debug, Clone)]
pub struct AssetDrag(pub String);

/// What a view looks through: the free perspective camera, or an orthographic camera
/// looking straight down an axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewKind {
    Perspective,
    /// Down the Y axis.
    Top,
    /// Along -Z.
    Front,
    /// Along -X (from +X).
    Side,
}

impl ViewKind {
    pub const ALL: [ViewKind; 4] = [
        ViewKind::Perspective,
        ViewKind::Top,
        ViewKind::Front,
        ViewKind::Side,
    ];

    pub fn is_orthographic(self) -> bool {
        self != ViewKind::Perspective
    }

    /// The fixed camera rotation of an orthographic view.
    fn rotation(self) -> Option<Quat> {
        let (yaw, pitch) = match self {
            ViewKind::Perspective => return None,
            ViewKind::Top => (0.0, -std::f32::consts::FRAC_PI_2),
            ViewKind::Front => (0.0, 0.0),
            ViewKind::Side => (std::f32::consts::FRAC_PI_2, 0.0),
        };
        Some(Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0))
    }

    /// The world axes the grid of the view spans.
    fn grid_axes(self) -> (Vec3, Vec3) {
        match self {
            ViewKind::Perspective | ViewKind::Top => (Vec3::X, Vec3::Z),
            ViewKind::Front => (Vec3::X, Vec3::Y),
            ViewKind::Side => (Vec3::Z, Vec3::Y),
        }
    }

    pub fn label(self) -> String {
        crate::i18n::t(match self {
            ViewKind::Perspective => "viewport.perspective",
            ViewKind::Top => "viewport.top",
            ViewKind::Front => "viewport.front",
            ViewKind::Side => "viewport.side",
        })
    }
}

/// How far behind the pivot an orthographic camera sits: far enough that nothing in the
/// scene is behind it.
const ORTHO_BACK: f32 = 1000.0;

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
    pub snap: SnapSettings,
    /// Outline selected objects (selection itself, the gizmo and picking work either way).
    pub highlight_selection: bool,
    /// Outline the object under the pointer.
    pub highlight_hover: bool,
    pub grid: bool,
    pub icons: bool,
    pub collision: bool,
    pub stats: bool,
    /// Look through the editor camera while playing too.
    pub free_camera_in_play: bool,
    /// While editing: brighter ambient light and no fog, so dark levels stay workable.
    pub editor_lighting: bool,
    /// While editing: select, move and resize UI controls in the view (off by default: the
    /// UI Editor is where interfaces are made).
    pub ui: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            gizmo: GizmoMode::Translate,
            snap: SnapSettings::default(),
            highlight_selection: true,
            highlight_hover: true,
            grid: true,
            icons: true,
            collision: false,
            stats: true,
            free_camera_in_play: false,
            editor_lighting: true,
            ui: false,
        }
    }
}

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
    /// An orthographic view's height in world units, and the field of view that maps it to
    /// a perspective distance (for sizes that should look the same in both).
    ortho: Option<(f32, f32)>,
}

impl Projector {
    fn new(view: &CameraView, rect: Rect) -> Self {
        Self {
            view_projection: view.view_projection,
            inverse: view.view_projection.inverse(),
            eye: view.position,
            rect,
            ortho: None,
        }
    }

    fn orthographic(view: &CameraView, rect: Rect, fov_y_radians: f32) -> Self {
        Self {
            ortho: view.ortho_height.map(|h| (h, fov_y_radians)),
            ..Self::new(view, rect)
        }
    }

    /// How far `p` looks from the camera: its distance, or for an orthographic view the
    /// distance a perspective camera would need to show the same size.
    fn view_distance(&self, p: Vec3) -> f32 {
        match self.ortho {
            Some((height, fov)) => height * 0.5 / (fov * 0.5).tan().max(1e-3),
            None => self.eye.distance(p),
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

    /// World ray (origin, unit direction) through a screen point. Orthographic rays are
    /// parallel and start on the near plane under the point.
    fn ray(&self, pos: Pos2) -> (Vec3, Vec3) {
        let x = (pos.x - self.rect.min.x) / self.rect.width() * 2.0 - 1.0;
        let y = 1.0 - (pos.y - self.rect.min.y) / self.rect.height() * 2.0;
        let unproject = |z: f32| {
            let p = self.inverse * Vec4::new(x, y, z, 1.0);
            p.truncate() / p.w
        };
        if self.ortho.is_some() {
            let (near, far) = (unproject(0.0), unproject(1.0));
            return (near, (far - near).normalize_or(Vec3::NEG_Z));
        }
        let point = unproject(0.5);
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
    /// Shared by every view of the editor: one set of pipelines and shadow maps.
    renderer: std::rc::Rc<std::cell::RefCell<SceneRenderer>>,
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
            renderer: std::rc::Rc::new(std::cell::RefCell::new(renderer)),
            target: None,
            texture: None,
            stats: FrameStats::default(),
        }
    }

    /// The last image, when it was drawn at `width` × `height` (a view that need not redraw
    /// this frame shows it again).
    pub fn cached(&self, width: u32, height: u32) -> Option<egui::TextureId> {
        let size = (width.clamp(1, 8192), height.clamp(1, 8192));
        self.target
            .as_ref()
            .filter(|t| t.size() == size)
            .and(self.texture)
    }

    /// Another view drawing with the same scene renderer, into its own image.
    fn sharing(&self) -> Self {
        Self {
            gpu: self.gpu.clone(),
            renderer: self.renderer.clone(),
            target: None,
            texture: None,
            stats: FrameStats::default(),
        }
    }

    pub fn render(
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
        self.stats =
            self.renderer
                .borrow_mut()
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

/// What a gizmo drag does to each dragged node, relative to where it started.
#[derive(Debug, Clone, Copy)]
enum Change {
    /// World offset.
    Move(Vec3),
    /// World rotation about the active node's origin.
    Turn(Affine3A),
    /// Scale factors along each node's own axes.
    Scale(Vec3),
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
    pub kind: ViewKind,
    /// The view that shows the game's camera and takes the game's input while playing
    /// (the first one); the others keep their editor camera.
    pub main: bool,
    /// The layout button was pressed (the viewport area acts on it).
    pub toggle_layout: bool,
    /// Draw the scene this frame; when off, the last image is shown again (a view in a
    /// background window, see [`crate::tool_window::Redraw`]).
    pub redraw: bool,
    renderer: Option<ViewportRenderer>,
    drag: Option<GizmoDrag>,
    /// What the gizmo drag moves: each selected root with its transforms at the start.
    drag_group: Vec<(NodeHandle, Affine3A, Transform3D)>,
    /// A box selection in progress: where the drag started (screen).
    marquee: Option<Pos2>,
    /// A UI control being moved or resized.
    ui_drag: Option<crate::ui_edit::UiDrag>,
    /// Play mode: the game receives keyboard and mouse input.
    pub game_focus: bool,
    cursor_grabbed: bool,
    /// Input went to the game since it last had focus.
    forwarding: bool,
    last_modifiers: egui::Modifiers,
    /// Image size in physical pixels, for the game's cursor coordinates.
    pixels_per_point: f32,
    /// What was under the pointer when the context menu opened, and the ground point there.
    menu_target: Option<(Option<NodeHandle>, Vec3)>,
    /// A right-click elsewhere closed the open menu: open the new one next frame.
    reopen_menu: bool,
    /// A path point being dragged: the path, the point, and the points before the drag.
    path_drag: Option<(NodeHandle, usize, Vec<[f32; 3]>)>,
    /// A right-click deleted a path point: no context menu for it.
    suppress_menu: bool,
}

impl Viewport {
    pub fn new(state: Option<&egui_wgpu::RenderState>) -> Self {
        Self::with_renderer(state.map(ViewportRenderer::new))
    }

    /// Another view sharing this one's scene renderer.
    pub fn sharing(&self, kind: ViewKind) -> Self {
        let mut view = Self::with_renderer(self.renderer.as_ref().map(ViewportRenderer::sharing));
        view.kind = kind;
        view.main = false;
        view.options = self.options.clone();
        view.camera = self.camera;
        view
    }

    /// A renderer drawing with this view's scene renderer into an image of its own (a
    /// tool window's view of the scene).
    pub fn share_renderer(&self) -> Option<ViewportRenderer> {
        self.renderer.as_ref().map(ViewportRenderer::sharing)
    }

    fn with_renderer(renderer: Option<ViewportRenderer>) -> Self {
        Self {
            camera: EditorCamera::default(),
            options: ViewOptions::default(),
            kind: ViewKind::Perspective,
            main: true,
            toggle_layout: false,
            redraw: true,
            renderer,
            drag: None,
            drag_group: Vec::new(),
            marquee: None,
            ui_drag: None,
            game_focus: false,
            cursor_grabbed: false,
            forwarding: false,
            last_modifiers: egui::Modifiers::NONE,
            pixels_per_point: 1.0,
            menu_target: None,
            reopen_menu: false,
            path_drag: None,
            suppress_menu: false,
        }
    }

    pub fn stats(&self) -> FrameStats {
        self.renderer.as_ref().map(|r| r.stats).unwrap_or_default()
    }

    /// Frames the selection (or the whole scene).
    pub fn frame_selection(&mut self, editor: &Editor) {
        let mut nodes = editor.selected_nodes();
        if nodes.is_empty() {
            nodes.push(editor.scene_root);
        }
        let tree = editor.tree();
        // Meshes by their bounds, then collision shapes, other nodes (lights, cameras) by
        // their position.
        let bounds = nodes
            .iter()
            .filter_map(|&n| {
                tree.subtree_bounds(n)
                    .or_else(|| subtree_shape_bounds(editor, n))
                    .or_else(|| {
                        tree.global_position(n).map(|p| Aabb {
                            min: p - Vec3::splat(0.5),
                            max: p + Vec3::splat(0.5),
                        })
                    })
            })
            .reduce(|a, b| a.union(&b));
        let (center, radius) = match bounds {
            Some(bounds) => (bounds.center(), bounds.size().length() * 0.5),
            None => (Vec3::ZERO, 1.0),
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
        !self.main || !editor.is_playing() || self.options.free_camera_in_play || !game_has_camera()
    }

    /// The view camera's global transform: the orbit camera, or for an orthographic view
    /// its fixed direction from far behind the pivot.
    fn view_global(&self) -> Affine3A {
        match self.kind.rotation() {
            Some(rotation) => Affine3A::from_rotation_translation(
                rotation,
                self.camera.pivot + rotation * Vec3::Z * ORTHO_BACK,
            ),
            None => self.camera.global(),
        }
    }

    /// The view camera's rotation: the orbit camera's, or an orthographic view's own.
    fn view_rotation(&self) -> Quat {
        self.kind
            .rotation()
            .unwrap_or_else(|| self.camera.rotation())
    }

    /// An orthographic view's height in world units: what a perspective camera at the
    /// same distance sees, so switching views keeps the scale.
    fn ortho_height(&self) -> Option<f32> {
        self.kind
            .is_orthographic()
            .then(|| 2.0 * self.camera.distance * (self.camera.fov.to_radians() * 0.5).tan())
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
        let ortho_height = self.ortho_height();
        let camera = Camera3D {
            fov_y_degrees: self.camera.fov,
            near: 0.05,
            far: if ortho_height.is_some() {
                ORTHO_BACK * 2.0 + 2000.0
            } else {
                2000.0
            },
            current: true,
        };
        {
            let world = editor.tree_mut().world_mut();
            if editor_camera {
                world.insert_resource(CameraOverride {
                    camera,
                    global: self.view_global(),
                    ortho_height,
                });
            } else {
                world.remove_resource::<CameraOverride>();
            }
        }

        // The scene image.
        let painter = ui.painter_at(rect);
        match (&mut self.renderer, render_state) {
            (Some(renderer), Some(_))
                if !self.redraw && renderer.cached(width, height).is_some() =>
            {
                let texture = renderer.cached(width, height).expect("checked above");
                painter.image(
                    texture,
                    rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
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
                    t("viewport.no_gpu"),
                    egui::FontId::proportional(16.0),
                    theme::TEXT_DIM,
                );
            }
        }

        let aspect = rect.width() / rect.height().max(1.0);
        let fov = camera.fov_y_degrees.to_radians();
        let projector = match ortho_height {
            Some(height) => {
                let view = CameraView::orthographic(
                    &self.view_global(),
                    height,
                    aspect,
                    camera.near,
                    camera.far,
                );
                Projector::orthographic(&view, rect, fov)
            }
            None => Projector::new(
                &CameraView::new(&self.view_global(), fov, aspect, camera.near, camera.far),
                rect,
            ),
        };

        if editor_camera {
            if self.options.grid {
                draw_grid(&painter, &projector, &self.camera, self.kind);
            }
            if self.options.collision {
                draw_collision(&painter, &projector, editor);
            }
            if self.options.icons && !editor.is_playing() {
                draw_icons(&painter, &projector, editor);
            }
            // While playing, the engine draws them in 3D itself.
            let bounds_view = editor
                .tree()
                .world()
                .resource::<ab_engine::DebugSettings>()
                .is_some_and(|s| s.bounds);
            if bounds_view && !editor.is_playing() {
                draw_mesh_bounds(&painter, &projector, editor);
            }
            if self.options.highlight_selection {
                for node in editor.selected_nodes() {
                    let active = editor.selection == Some(node);
                    draw_selection(&painter, &projector, editor, node, active);
                }
            }
        }

        let mut gizmo_hovered = false;
        if editor.mode == Mode::Edit && editor.build.is_some() {
            self.build_tool(ui, &response, &painter, &projector, editor);
        } else if editor.mode == Mode::Edit {
            // A selected path's points come before the gizmo.
            let path_busy = self.path_tool(ui, &response, &painter, &projector, editor);
            gizmo_hovered = path_busy;
            // The gizmo first (it is drawn on top), then the UI controls over the scene.
            if self.ui_drag.is_none() && !path_busy {
                gizmo_hovered = self.gizmo(ui, &response, &painter, &projector, editor);
            }
            let ui_view = self
                .options
                .ui
                .then(|| crate::ui_edit::UiView::new(editor, rect, ppp))
                .flatten();
            let mut ui_busy = false;
            if let Some(view) = ui_view
                .as_ref()
                .filter(|_| self.drag.is_none() && !gizmo_hovered)
            {
                let snapping = self.options.snap.enabled || ui.input(|i| i.modifiers.command);
                ui_busy = crate::ui_edit::edit(
                    &mut self.ui_drag,
                    ui,
                    &response,
                    &painter,
                    view,
                    editor,
                    snapping,
                );
            }
            if response.clicked() && !gizmo_hovered {
                if let Some(pos) = response.interact_pointer_pos() {
                    let modifiers = ui.input(|i| i.modifiers);
                    let control = ui_view.as_ref().and_then(|view| {
                        let controls = crate::ui_edit::controls(editor);
                        crate::ui_edit::pick(&controls, view, pos, editor)
                    });
                    let hit = control
                        .map(|node| Hit {
                            owner: node,
                            part: node,
                        })
                        .or_else(|| hit(editor, &projector, pos));
                    match hit {
                        Some(hit) if modifiers.command => editor.toggle_selected(hit.owner),
                        Some(hit) if modifiers.shift => editor.add_to_selection(&[hit.owner]),
                        // A second click on a selected body picks the part under the
                        // pointer (its mesh or collision shape).
                        Some(hit)
                            if editor.selection == Some(hit.owner)
                                && editor.selected.len() == 1
                                && hit.part != hit.owner
                                && editor.can_edit_properties(hit.part) =>
                        {
                            editor.select(Some(hit.part));
                        }
                        Some(hit) => editor.select(Some(hit.owner)),
                        // A click on nothing with a modifier keeps the selection.
                        None if modifiers.command || modifiers.shift => {}
                        None => editor.select(None),
                    }
                }
            }
            self.marquee(
                ui,
                &response,
                &painter,
                &projector,
                editor,
                gizmo_hovered || ui_busy,
            );
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
            let dragging_script = response
                .dnd_hover_payload::<crate::scripts::ScriptDrag>()
                .is_some();
            let idle = self.drag.is_none()
                && self.marquee.is_none()
                && self.ui_drag.is_none()
                && !gizmo_hovered
                && !ui.input(|i| i.pointer.any_down());
            // The object under the pointer: a script's drop target, or a hover outline.
            if let Some(pos) = response
                .hover_pos()
                .filter(|_| dragging_script || (idle && self.options.highlight_hover))
            {
                if let Some(node) = pick(editor, &projector, pos) {
                    if dragging_script || !editor.is_selected(node) {
                        draw_hover(&painter, &projector, editor, node);
                    }
                }
            }
            self.context_menu(ui, &response, &projector, editor);
        } else if self.main {
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
    // Path points
    // =========================================================================================

    /// The selected path's curve and points: drag a point to move it (Shift: up and
    /// down), Ctrl+click to add one (on the curve, or after the last), right-click a point
    /// to delete it. Returns whether the pointer is busy with the points.
    fn path_tool(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        painter: &egui::Painter,
        projector: &Projector,
        editor: &mut Editor,
    ) -> bool {
        use ab_engine::scene_file::path::Path3D;
        let Some(node) = editor
            .selection
            .filter(|&n| editor.can_edit_properties(n))
            .filter(|&n| editor.tree().component::<Path3D>(n).is_some())
        else {
            self.path_drag = None;
            return false;
        };
        let tree = editor.tree();
        let Some(global) = tree.global_transform(node) else {
            return false;
        };
        let Some(path) = tree.component::<Path3D>(node).map(|p| p.clone()) else {
            return false;
        };
        // The curve.
        let curve: Vec<Pos2> = path
            .spline
            .polyline()
            .filter_map(|p| projector.project(global.transform_point3(p)))
            .collect();
        painter.add(egui::Shape::line(
            curve,
            Stroke::new(2.0_f32, theme::ACCENT),
        ));
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let pressed_at = ui.input(|i| i.pointer.press_origin());
        let screen: Vec<Option<Pos2>> = path
            .desc
            .points
            .iter()
            .map(|&p| projector.project(global.transform_point3(Vec3::from(p))))
            .collect();
        let point_at = |p: Pos2| {
            screen
                .iter()
                .enumerate()
                .filter_map(|(i, s)| Some((i, ((*s)? - p).length())))
                .filter(|(_, d)| *d < 9.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        };
        let hovered = pointer.and_then(point_at);
        for (i, at) in screen.iter().enumerate() {
            let Some(at) = at else { continue };
            let active = self.path_drag.as_ref().is_some_and(|d| d.1 == i) || hovered == Some(i);
            let rect = Rect::from_center_size(*at, Vec2::splat(if active { 11.0 } else { 9.0 }));
            painter.rect_filled(
                rect,
                2.0,
                if active {
                    Color32::WHITE
                } else {
                    theme::ACCENT
                },
            );
            painter.rect_stroke(
                rect,
                2.0,
                Stroke::new(1.0_f32, theme::BG_0),
                egui::StrokeKind::Outside,
            );
        }

        let apply = |editor: &mut Editor, points: Vec<[f32; 3]>, before: Vec<[f32; 3]>| {
            // Back to the points before the change, so undo returns to them.
            if let Some(mut p) = editor.tree().component_mut::<Path3D>(node) {
                let mut desc = p.desc.clone();
                desc.points = before.clone();
                *p = Path3D::new(desc);
            }
            let Some(mut old) = editor.describe_node(node) else {
                return;
            };
            let mut new = old.clone();
            if let Some(p) = new.path.as_mut() {
                p.points = points;
            }
            if let Some(p) = old.path.as_mut() {
                p.points = before;
            }
            editor.apply_desc(node, &old, new);
            editor.end_gesture();
        };

        // Delete with a right-click.
        if response.secondary_clicked() {
            if let Some(i) = hovered {
                let before = path.desc.points.clone();
                let mut points = before.clone();
                points.remove(i);
                apply(editor, points, before);
                self.suppress_menu = true;
                return true;
            }
        }
        // Start, continue or end a drag.
        if response.drag_started_by(egui::PointerButton::Primary) {
            // Where the button went down: the drag starts once the pointer has moved.
            if let Some(i) = pressed_at.and_then(point_at) {
                self.path_drag = Some((node, i, path.desc.points.clone()));
            }
        }
        if let Some((dragged, i, before)) = self.path_drag.clone() {
            if dragged != node {
                self.path_drag = None;
            } else if response.dragged_by(egui::PointerButton::Primary) {
                if let (Some(pos), Some(&local)) = (pointer, path.desc.points.get(i)) {
                    let world = global.transform_point3(Vec3::from(local));
                    let (origin, dir) = projector.ray(pos);
                    let vertical = ui.input(|i| i.modifiers.shift);
                    // Level moves on the point's height; Shift moves it up and down. Front
                    // and side views look along the ground: the point moves in the plane
                    // facing the view (a level plane is edge-on there).
                    let facing = projector.ortho.is_some() && dir.y.abs() < 0.5;
                    let normal = if facing {
                        -dir
                    } else if vertical {
                        let toward = projector.eye - world;
                        Vec3::new(toward.x, 0.0, toward.z).normalize_or(Vec3::Z)
                    } else {
                        Vec3::Y
                    };
                    let denom = dir.dot(normal);
                    if denom.abs() > 1e-4 {
                        let t = (world - origin).dot(normal) / denom;
                        let mut hit = origin + dir * t;
                        if vertical {
                            hit = Vec3::new(world.x, hit.y, world.z);
                        }
                        if self.options.snap.enabled || ui.input(|i| i.modifiers.command) {
                            hit = snap_point(hit, self.options.snap.translate);
                        }
                        let local = global.inverse().transform_point3(hit);
                        let mut points = path.desc.points.clone();
                        points[i] = local.to_array();
                        // Live: the curve follows; what it builds is rebuilt on release.
                        let mut moved = path.clone();
                        moved.desc.points = points;
                        let moved = Path3D::new(moved.desc);
                        if let Some(mut p) = editor.tree().component_mut::<Path3D>(node) {
                            *p = moved;
                        }
                    }
                }
                return true;
            } else {
                self.path_drag = None;
                let points = path.desc.points.clone();
                if points != before {
                    apply(editor, points, before);
                }
                return true;
            }
        }
        // Add with Ctrl+click.
        if response.clicked() && ui.input(|i| i.modifiers.command) {
            if let Some(pos) = response.interact_pointer_pos() {
                let at = global
                    .inverse()
                    .transform_point3(ground_point(projector, pos));
                let before = path.desc.points.clone();
                let mut points = before.clone();
                let near_curve = path.spline.nearest_segment(at).filter(|&(segment, t)| {
                    let p = global.transform_point3(path.spline.segment_point(segment, t));
                    projector
                        .project(p)
                        .is_some_and(|s| (s - pos).length() < 16.0)
                });
                match near_curve {
                    Some((segment, _)) => points.insert(segment + 1, at.to_array()),
                    None => points.push(at.to_array()),
                }
                apply(editor, points, before);
                return true;
            }
        }
        hovered.is_some() || pressed_at.and_then(point_at).is_some()
    }

    // =========================================================================================
    // Build tool
    // =========================================================================================

    /// The build tool's piece follows the pointer, resting on what is under it; a click
    /// places it (see [`crate::build`]).
    fn build_tool(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        painter: &egui::Painter,
        projector: &Projector,
        editor: &mut Editor,
    ) {
        if response.secondary_clicked() {
            editor.build = None;
            return;
        }
        let Some(pos) = response.hover_pos() else {
            return;
        };
        let Some((ghost, bounds)) = editor.build_ghost() else {
            return;
        };
        let surface = surface_at(editor, projector, pos);
        let step = if ui.input(|i| i.modifiers.alt) {
            0.0
        } else {
            self.options.snap.translate
        };
        let origin = crate::build::placement(&surface, &bounds, step);
        {
            let tree = editor.tree_mut();
            let _ = tree.set_global_position(ghost, origin);
            tree.update_transforms();
        }
        let placed = Aabb {
            min: bounds.min + origin,
            max: bounds.max + origin,
        };
        draw_box(
            painter,
            projector,
            &placed.corners(),
            Stroke::new(1.5_f32, theme::ACCENT),
        );
        // The footprint on the surface it rests on.
        if let Some(at) = projector.project(surface.point) {
            painter.circle_stroke(at, 4.0, Stroke::new(1.5_f32, theme::ACCENT));
        }
        let hint = format!("{}  {}", icon::CUBE, t("build.hint"));
        let galley = painter.layout_no_wrap(hint, egui::FontId::proportional(12.0), Color32::WHITE);
        let at = pos + Vec2::new(16.0, 18.0);
        painter.rect_filled(
            Rect::from_min_size(
                at - Vec2::new(6.0, 3.0),
                galley.size() + Vec2::new(12.0, 6.0),
            ),
            6.0,
            theme::BG_0.gamma_multiply(0.85),
        );
        painter.galley(at, galley, Color32::WHITE);
        if response.clicked() {
            if let Some(piece) = editor.build.clone() {
                editor.place_piece(&piece, origin);
            }
        }
    }

    // =========================================================================================
    // Context menu
    // =========================================================================================

    /// Right-click (without dragging, which flies the camera): actions for the object under
    /// the pointer, or for the scene when there is none. The object is selected first,
    /// unless it is part of the selection already.
    fn context_menu(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        projector: &Projector,
        editor: &mut Editor,
    ) {
        let clicked = response.secondary_clicked() && !std::mem::take(&mut self.suppress_menu);
        if clicked {
            self.menu_target = response.interact_pointer_pos().map(|pos| {
                let node = pick(editor, projector, pos);
                if let Some(node) = node.filter(|&n| !editor.is_selected(n)) {
                    editor.select(Some(node));
                }
                (node, ground_point(projector, pos))
            });
        }
        let Some((target, at)) = self.menu_target else {
            return;
        };
        let mut popup = egui::Popup::context_menu(response);
        // egui closes an open menu on any click, this one included.
        let was_open = egui::Popup::is_id_open(ui.ctx(), popup.get_id());
        if std::mem::take(&mut self.reopen_menu) {
            popup = popup.open_memory(Some(true.into()));
        }
        let shown = popup.show(|ui| {
            ui.set_min_width(210.0);
            let target = target.filter(|&n| editor.tree().is_valid(n));
            match target {
                Some(node) => self.object_menu(ui, editor, node, at),
                None => self.space_menu(ui, editor, at),
            }
        });
        if clicked && was_open {
            // The click that closed the previous menu opens this one, next frame.
            self.reopen_menu = true;
        } else if shown.is_none() {
            self.menu_target = None;
        }
    }

    fn object_menu(&mut self, ui: &mut egui::Ui, editor: &mut Editor, node: NodeHandle, at: Vec3) {
        use crate::commands::{menu_item, Command};
        let tree = editor.tree();
        let class = tree.class_name(node).unwrap_or("Node").to_owned();
        let name = tree.name(node).map(|n| n.to_owned()).unwrap_or_default();
        let count = editor.selected.len();
        let title = if count > 1 {
            tf("menu.objects", &[("count", &count)])
        } else {
            name
        };
        ui.label(
            egui::RichText::new(format!("{}  {title}", theme::class_icon(&class)))
                .strong()
                .color(theme::class_color(&class)),
        );
        ui.separator();
        let editable = editor.is_editable(node) && !editor.is_playing();
        menu_item(ui, Command::Cut, editable);
        menu_item(ui, Command::Copy, editable);
        menu_item(ui, Command::Paste, editor.can_paste(None));
        let into = editable && !editor.is_instance(node);
        if ui
            .add_enabled(
                into && editor.can_paste(None),
                egui::Button::new(format!(
                    "{}  {}",
                    icon::CLIPBOARD_TEXT,
                    t("menu.paste_as_child")
                )),
            )
            .clicked()
        {
            editor.paste(None, Some(node), None);
            ui.close();
        }
        menu_item(ui, Command::Duplicate, editable);
        menu_item(ui, Command::Rename, editable && count == 1);
        if editable
            && !editor.is_instance(node)
            && count == 1
            && ui
                .button(format!(
                    "{}  {}",
                    icon::LINK_SIMPLE,
                    t("menu.create_prefab")
                ))
                .clicked()
        {
            let path = crate::panels::scene_tree::prefab_path(editor, node);
            match editor.create_prefab(node, &path) {
                Ok(instance) => editor.select(Some(instance)),
                Err(err) => editor.fail(err),
            }
            ui.close();
        }
        ui.separator();
        menu_item(ui, Command::FrameSelection, true);
        if let Some(parent) = editor
            .tree()
            .parent(node)
            .filter(|&p| p != editor.scene_root)
            .and_then(|p| editor.instance_owner(p))
        {
            if ui
                .button(format!(
                    "{}  {}",
                    icon::ARROW_ELBOW_LEFT_UP,
                    t("menu.select_parent")
                ))
                .clicked()
            {
                editor.select(Some(parent));
                ui.close();
            }
        }
        let children: Vec<NodeHandle> = editor
            .tree()
            .children(node)
            .into_iter()
            .filter(|&c| editor.is_editable(c))
            .collect();
        if !children.is_empty()
            && ui
                .button(format!(
                    "{}  {}",
                    icon::TREE_STRUCTURE,
                    t("menu.select_children")
                ))
                .clicked()
        {
            editor.set_selection(children, None);
            ui.close();
        }
        menu_item(ui, Command::SelectNone, true);
        ui.separator();
        ui.menu_button(
            format!("{}  {}", icon::ALIGN_CENTER_HORIZONTAL, t("build.arrange")),
            |ui| {
                use crate::state::Align;
                let many = count > 1;
                for (axis, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                    ui.add_enabled_ui(many, |ui| {
                        ui.menu_button(format!("{} {name}", t("build.align")), |ui| {
                            for (align, label) in [
                                (Align::Min, "Min"),
                                (Align::Center, "Center"),
                                (Align::Max, "Max"),
                            ] {
                                if ui.button(label).clicked() {
                                    editor.align_selected(axis, align);
                                    ui.close();
                                }
                            }
                        });
                    });
                }
                for (axis, name) in [(0, "X"), (2, "Z")] {
                    if ui
                        .add_enabled(
                            count > 2,
                            egui::Button::new(format!("{} {name}", t("build.distribute"))),
                        )
                        .clicked()
                    {
                        editor.distribute_selected(axis);
                        ui.close();
                    }
                }
                if ui
                    .add_enabled(editable, egui::Button::new(t("build.drop")))
                    .clicked()
                {
                    editor.drop_selected();
                    ui.close();
                }
            },
        );
        ui.add_enabled_ui(into, |ui| {
            ui.menu_button(format!("{}  {}", icon::PLUS, t("menu.add_child")), |ui| {
                crate::create::menu(ui, editor, at);
            });
        });
        ui.separator();
        if ui
            .add_enabled(
                editable,
                egui::Button::new(
                    egui::RichText::new(format!("{}  {}", icon::TRASH, Command::Delete.label()))
                        .color(theme::ERROR),
                )
                .shortcut_text(Command::Delete.shortcut_text(ui.ctx())),
            )
            .clicked()
        {
            crate::commands::request(ui.ctx(), Command::Delete);
            ui.close();
        }
    }

    fn space_menu(&mut self, ui: &mut egui::Ui, editor: &mut Editor, at: Vec3) {
        use crate::commands::{menu_item, Command};
        let editing = !editor.is_playing();
        ui.add_enabled_ui(editing, |ui| {
            ui.menu_button(format!("{}  {}", icon::PLUS, t("menu.create")), |ui| {
                editor.select(None);
                crate::create::menu(ui, editor, at);
            });
        });
        if ui
            .add_enabled(
                editor.can_paste(None),
                egui::Button::new(format!("{}  {}", icon::CLIPBOARD, t("menu.paste_here")))
                    .shortcut_text(Command::Paste.shortcut_text(ui.ctx())),
            )
            .clicked()
        {
            editor.paste(None, Some(editor.scene_root), Some(at));
            ui.close();
        }
        ui.separator();
        menu_item(ui, Command::SelectAll, editing);
        menu_item(ui, Command::SelectNone, !editor.selected.is_empty());
        if ui
            .button(format!("{}  {}", icon::CROSSHAIR, t("menu.frame_scene")))
            .clicked()
        {
            editor.select(None);
            self.frame_selection(editor);
            ui.close();
        }
        ui.separator();
        ui.checkbox(&mut self.options.grid, t("menu.grid"));
        ui.checkbox(&mut self.options.icons, t("menu.node_icons"));
        ui.checkbox(&mut self.options.collision, t("menu.collision"));
        ui.checkbox(
            &mut self.options.highlight_selection,
            t("menu.highlight_selection"),
        );
        ui.checkbox(&mut self.options.highlight_hover, t("menu.highlight_hover"));
        ui.checkbox(&mut self.options.snap.enabled, t("menu.snap_to_grid"));
    }

    // =========================================================================================
    // Box selection
    // =========================================================================================

    /// Dragging with the left button on empty space selects what is inside the box: every
    /// object whose centre is in it. Shift or Ctrl adds to the selection.
    fn marquee(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        painter: &egui::Painter,
        projector: &Projector,
        editor: &mut Editor,
        gizmo_hovered: bool,
    ) {
        let alt = ui.input(|i| i.modifiers.alt);
        if response.drag_started_by(egui::PointerButton::Primary)
            && !gizmo_hovered
            && self.drag.is_none()
            && !alt
        {
            self.marquee = ui.input(|i| i.pointer.press_origin());
        }
        let Some(start) = self.marquee else {
            return;
        };
        let Some(now) = ui.input(|i| i.pointer.interact_pos()) else {
            return;
        };
        let rect = Rect::from_two_pos(start, now).intersect(projector.rect);
        painter.rect(
            rect,
            2.0,
            theme::SELECTION.gamma_multiply(0.12),
            Stroke::new(1.0_f32, theme::SELECTION),
            egui::StrokeKind::Inside,
        );
        if response.dragged_by(egui::PointerButton::Primary) {
            return;
        }
        // Released.
        self.marquee = None;
        let found = marquee_nodes(editor, projector, rect);
        let modifiers = ui.input(|i| i.modifiers);
        if modifiers.shift || modifiers.command {
            editor.add_to_selection(&found);
        } else {
            editor.set_selection(found, None);
        }
        let count = editor.selected.len();
        if count > 1 {
            editor.info(format!("{count} nodes selected"));
        }
    }

    // =========================================================================================
    // Camera navigation
    // =========================================================================================

    fn navigate(&mut self, ui: &egui::Ui, response: &egui::Response, editor: &Editor) {
        let (delta, modifiers, dt) =
            ui.input(|i| (i.pointer.delta(), i.modifiers, i.stable_dt.min(0.1)));
        if let (Some(rotation), Some(height)) = (self.kind.rotation(), self.ortho_height()) {
            // Orthographic: every drag pans (the view keeps its axis), the wheel zooms.
            let dragging = response.dragged_by(egui::PointerButton::Secondary)
                || response.dragged_by(egui::PointerButton::Middle)
                || response.dragged_by(egui::PointerButton::Primary) && modifiers.alt;
            if dragging {
                let per_pixel = height / response.rect.height().max(1.0);
                self.camera.pivot +=
                    (rotation * Vec3::NEG_X * delta.x + rotation * Vec3::Y * delta.y) * per_pixel;
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    self.camera.zoom(scroll / 50.0);
                }
                let focus = ui.input(|i| i.key_pressed(egui::Key::F) && i.modifiers.is_none());
                if focus && editor.mode == Mode::Edit && !ui.ctx().egui_wants_keyboard_input() {
                    self.frame_selection(editor);
                }
            }
            return;
        }
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
            mode != GizmoMode::Select && n != editor.scene_root && editor.can_edit_properties(n)
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
        let size = projector.view_distance(origin).max(0.1) * 0.16;
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
        // A drag starts once the pointer has moved a few pixels: what it grabbed is what
        // was under the pointer where the button went down (the centre handle is smaller
        // than that move).
        let starting = response.drag_started_by(egui::PointerButton::Primary)
            && !ui.input(|i| i.modifiers.alt);
        let pointer = if starting {
            ui.input(|i| i.pointer.press_origin())
        } else {
            ui.input(|i| i.pointer.hover_pos())
        };

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
        if starting {
            if let (Some(handle), Some(start)) = (hovered, pointer) {
                let (axis_screen, pixels_per_unit) = match handle {
                    Handle::Axis(i) => {
                        let tip = projector.project(origin + axes[i]).unwrap_or(center);
                        let dir = tip - center;
                        (dir.normalized(), dir.length().max(1e-3))
                    }
                    // Along the view's right: world X can point into the screen.
                    Handle::Center => (
                        Vec2::X,
                        projector.pixels_per_unit(origin, self.view_rotation() * Vec3::X),
                    ),
                };
                let to_mouse = start - center;
                // Every selected root moves; children follow their parent.
                let tree = editor.tree();
                self.drag_group = editor
                    .selected_roots()
                    .into_iter()
                    .filter(|&n| n != editor.scene_root && editor.can_edit_properties(n))
                    .filter_map(|n| Some((n, tree.global_transform(n)?, tree.transform(n)?)))
                    .collect();
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
        let snap = (self.options.snap.enabled || ui.input(|i| i.modifiers.command))
            .then_some(self.options.snap);
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
        snap: Option<SnapSettings>,
        editor: &mut Editor,
    ) {
        let delta = mouse - drag.start_mouse;
        let start_origin: Vec3 = drag.start_global.translation.into();
        match (self.options.gizmo, drag.handle) {
            (GizmoMode::Translate, handle) => {
                let offset = match handle {
                    Handle::Axis(i) => {
                        let mut units = delta.dot(drag.axis_screen) / drag.pixels_per_unit;
                        if let Some(snap) = snap {
                            units = snap.translate(units);
                        }
                        axes[i] * units
                    }
                    Handle::Center => {
                        let rotation = self.view_rotation();
                        let scale = 1.0 / drag.pixels_per_unit;
                        let mut offset =
                            (rotation * Vec3::X * delta.x - rotation * Vec3::Y * delta.y) * scale;
                        if let Some(snap) = snap {
                            offset = Vec3::new(
                                snap.translate(offset.x),
                                snap.translate(offset.y),
                                snap.translate(offset.z),
                            );
                        }
                        offset
                    }
                };
                self.apply_change(editor, Change::Move(offset));
            }
            (GizmoMode::Rotate, Handle::Axis(i)) => {
                let to_mouse = mouse - center;
                let mut angle = to_mouse.y.atan2(to_mouse.x) - drag.start_angle;
                // Screen y points down: on-screen counter-clockwise is a negative angle here.
                let facing = axes[i].dot(projector.eye - start_origin) > 0.0;
                if facing {
                    angle = -angle;
                }
                if let Some(snap) = snap {
                    angle = snap.rotate(angle.to_degrees()).to_radians();
                }
                let rotation = Affine3A::from_translation(start_origin)
                    * Affine3A::from_axis_angle(axes[i], angle)
                    * Affine3A::from_translation(-start_origin);
                self.apply_change(editor, Change::Turn(rotation));
            }
            (GizmoMode::Scale, handle) => {
                // Factors along the nodes' own axes, worked out on the active node.
                let factors = match handle {
                    Handle::Axis(i) => {
                        let mut factor =
                            (1.0 + delta.dot(drag.axis_screen) / drag.pixels_per_unit).max(0.01);
                        if let Some(snap) = snap {
                            factor = snap_scale_factor(&snap, drag.start_local.scale[i], factor);
                        }
                        let mut factors = Vec3::ONE;
                        factors[i] = factor;
                        factors
                    }
                    Handle::Center => {
                        let mut factor = (1.0 + (delta.x - delta.y) / 120.0).max(0.01);
                        if let Some(snap) = snap {
                            factor = snap.scale(factor);
                        }
                        Vec3::splat(factor)
                    }
                };
                self.apply_change(editor, Change::Scale(factors));
            }
            _ => {}
        }
    }

    /// Applies a gizmo change to every node of the drag, from where each one started.
    fn apply_change(&self, editor: &mut Editor, change: Change) {
        for &(node, global, local) in &self.drag_group {
            match change {
                Change::Move(offset) => {
                    let mut moved = global;
                    moved.translation = (Vec3::from(global.translation) + offset).into();
                    editor.set_global_transform(node, moved);
                }
                Change::Turn(rotation) => editor.set_global_transform(node, rotation * global),
                Change::Scale(factors) => {
                    let mut scaled = local;
                    scaled.scale = local.scale * factors;
                    editor.set_transform(node, scaled);
                }
            }
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
                                        (GizmoMode::Select, icon::CURSOR, t("viewport.select")),
                                        (
                                            GizmoMode::Translate,
                                            icon::ARROWS_OUT_CARDINAL,
                                            t("viewport.move"),
                                        ),
                                        (
                                            GizmoMode::Rotate,
                                            icon::ARROWS_CLOCKWISE,
                                            t("viewport.rotate"),
                                        ),
                                        (GizmoMode::Scale, icon::ARROWS_OUT, t("viewport.scale")),
                                    ] {
                                        ui.selectable_value(&mut self.options.gizmo, mode, glyph)
                                            .on_hover_text(tip);
                                    }
                                    ui.separator();
                                    let snap = self.options.snap;
                                    ui.toggle_value(&mut self.options.snap.enabled, icon::MAGNET)
                                        .on_hover_text(tf(
                                            "snap.tooltip",
                                            &[
                                                ("move", &snap.translate),
                                                ("rotate", &snap.rotate),
                                                ("scale", &snap.scale),
                                            ],
                                        ));
                                    ui.menu_button(icon::CARET_DOWN, |ui| {
                                        snap_settings(ui, &mut self.options.snap);
                                    })
                                    .response
                                    .on_hover_text(t("snap.steps"));
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
                                let kind_icon = if self.kind.is_orthographic() {
                                    icon::SQUARE_HALF
                                } else {
                                    icon::CUBE
                                };
                                ui.menu_button(
                                    format!("{kind_icon} {}", self.kind.label()),
                                    |ui| {
                                        for kind in ViewKind::ALL {
                                            if ui
                                                .selectable_label(self.kind == kind, kind.label())
                                                .clicked()
                                            {
                                                self.kind = kind;
                                                ui.close();
                                            }
                                        }
                                    },
                                )
                                .response
                                .on_hover_text(t("viewport.projection"));
                                if ui
                                    .button(icon::SQUARES_FOUR)
                                    .on_hover_text(t("viewport.layout"))
                                    .clicked()
                                {
                                    self.toggle_layout = true;
                                }
                                ui.menu_button(
                                    format!("{} {}", icon::EYE, t("viewport.view")),
                                    |ui| {
                                        let o = &mut self.options;
                                        ui.checkbox(&mut o.grid, t("menu.grid"));
                                        ui.checkbox(&mut o.icons, t("menu.node_icons"));
                                        ui.checkbox(&mut o.collision, t("menu.collision"));
                                        ui.checkbox(
                                            &mut o.highlight_selection,
                                            format!("{} (H)", t("menu.highlight_selection")),
                                        );
                                        ui.checkbox(
                                            &mut o.highlight_hover,
                                            t("menu.highlight_hover"),
                                        );
                                        ui.checkbox(&mut o.ui, t("menu.edit_ui"));
                                        ui.checkbox(&mut o.stats, t("menu.statistics"));
                                        ui.checkbox(
                                            &mut o.editor_lighting,
                                            t("menu.editor_lighting"),
                                        );
                                        ui.checkbox(
                                            &mut editor.preview_effects,
                                            t("menu.preview_effects"),
                                        );
                                        ui.checkbox(
                                            &mut o.free_camera_in_play,
                                            t("viewport.camera_in_play"),
                                        );
                                        ui.separator();
                                        ui.add(
                                            egui::Slider::new(&mut self.camera.fov, 20.0..=110.0)
                                                .text(t("viewport.field_of_view")),
                                        );
                                        ui.add(
                                            egui::Slider::new(
                                                &mut self.camera.fly_speed,
                                                0.5..=100.0,
                                            )
                                            .logarithmic(true)
                                            .text(t("viewport.fly_speed")),
                                        );
                                        if ui.button(t("viewport.reset_camera")).clicked() {
                                            self.camera = EditorCamera::default();
                                            ui.close();
                                        }
                                    },
                                );
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
            let galley = painter.layout_no_wrap(
                text,
                egui::FontId::monospace(11.5),
                Color32::from_gray(210),
            );
            // Narrow views (four at once): the bottom corner, clear of the tool bar.
            let narrow = rect.width() < 1000.0;
            let y = if narrow {
                rect.max.y - galley.size().y - 12.0
            } else {
                rect.min.y + 12.0
            };
            let pos = Pos2::new(rect.max.x - galley.size().x - 16.0, y);
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
                    format!("{}  {}", icon::MOUSE, t("viewport.click_to_play")),
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

/// `p` on the grid of `step` (unchanged for a zero step, which has no grid).
fn snap_point(p: Vec3, step: f32) -> Vec3 {
    Vec3::new(
        crate::settings::snap(p.x, step),
        crate::settings::snap(p.y, step),
        crate::settings::snap(p.z, step),
    )
}

/// The scale factor that puts a scale of `start` on the snap steps (by its size, keeping a
/// mirrored axis mirrored). A zero scale has no steps to snap to: the factor stays.
fn snap_scale_factor(snap: &SnapSettings, start: f32, factor: f32) -> f32 {
    if start.abs() < 1e-6 {
        return factor;
    }
    snap.scale((start * factor).abs()) / start.abs()
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
            | "Path3D"
    )
}

/// What a click hits: the node under the pointer (`part`) and the object it selects
/// (`owner`): a physics body for its mesh or collision shape, an instance for its contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hit {
    owner: NodeHandle,
    part: NodeHandle,
}

/// The object a click on `node` selects (see [`Hit`]).
fn selectable_owner(editor: &Editor, node: NodeHandle) -> Option<NodeHandle> {
    let tree = editor.tree();
    let owner = editor.instance_owner(node)?;
    match tree.parent(owner) {
        Some(parent)
            if parent != editor.scene_root
                && tree.component::<PhysicsBody3D>(parent).is_some()
                && editor.is_editable(parent)
                && tree.component::<PhysicsBody3D>(owner).is_none() =>
        {
            Some(parent)
        }
        _ => Some(owner),
    }
}

/// The object under a screen point (see [`hit`]).
fn pick(editor: &Editor, projector: &Projector, pos: Pos2) -> Option<NodeHandle> {
    hit(editor, projector, pos).map(|hit| hit.owner)
}

/// What is under a screen point: icons first, then the nearest mesh, then the nearest
/// collision shape (so a trigger volume does not hide the objects inside it).
fn hit(editor: &Editor, projector: &Projector, pos: Pos2) -> Option<Hit> {
    let tree = editor.tree();
    let nodes = tree.subtree(editor.scene_root);
    let found = |part: NodeHandle| selectable_owner(editor, part).map(|owner| Hit { owner, part });
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
        return found(node);
    }

    let (origin, dir) = projector.ray(pos);
    let assets = tree.world().resource::<RenderAssets>()?;
    let nearest = |best: &mut Option<(f32, NodeHandle)>, t: f32, node: NodeHandle| {
        if best.is_none_or(|(d, _)| t < d) {
            *best = Some((t, node));
        }
    };
    // A ray tested in the space of `global`: tight for rotated boxes, same ray parameter.
    let local_hit = |global: &Affine3A, min: Vec3, max: Vec3| {
        let inverse = global.inverse();
        ray_aabb(
            inverse.transform_point3(origin),
            inverse.transform_vector3(dir),
            min,
            max,
        )
    };
    let mut best_mesh: Option<(f32, NodeHandle)> = None;
    let mut best_shape: Option<(f32, NodeHandle)> = None;
    for node in nodes {
        if !tree.is_visible_in_tree(node) {
            continue;
        }
        if let Some(mesh) = tree.component::<MeshInstance3D>(node).map(|m| *m) {
            // A skinned mesh is wherever its joints put it: test its posed world box.
            let t = if tree.component::<Skin>(node).is_some() {
                tree.mesh_bounds(node)
                    .and_then(|b| ray_aabb(origin, dir, b.min, b.max))
            } else {
                assets
                    .mesh_aabb(mesh.mesh)
                    .zip(tree.global_transform(node))
                    .and_then(|(aabb, global)| local_hit(&global, aabb.min, aabb.max))
            };
            if let Some(t) = t {
                nearest(&mut best_mesh, t, node);
            }
        }
        if let Some(bounds) = tree
            .component::<CollisionShape3D>(node)
            .and_then(|c| shape_bounds(c.shape()))
        {
            if let Some(t) = tree
                .global_transform(node)
                .and_then(|global| local_hit(&global, bounds.min, bounds.max))
            {
                nearest(&mut best_shape, t, node);
            }
        }
    }
    best_mesh.or(best_shape).and_then(|(_, node)| found(node))
}

/// Ray / box slab test with the side the ray enters by: (distance, outward unit normal).
fn ray_aabb_normal(origin: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<(f32, Vec3)> {
    let inv = dir.recip();
    let t1 = (min - origin) * inv;
    let t2 = (max - origin) * inv;
    let near = t1.min(t2);
    let far = t1.max(t2).min_element();
    let enter = near.max_element();
    if far < enter.max(0.0) {
        return None;
    }
    let axis = (0..3)
        .max_by(|&a, &b| near[a].total_cmp(&near[b]))
        .unwrap_or(1);
    let mut normal = Vec3::ZERO;
    normal[axis] = -dir[axis].signum();
    Some((enter.max(0.0), normal))
}

/// What the build tool's piece would rest on under a screen point: the nearest mesh's side
/// (as a world axis), else the ground.
fn surface_at(editor: &Editor, projector: &Projector, pos: Pos2) -> crate::build::Surface {
    let tree = editor.tree();
    let (origin, dir) = projector.ray(pos);
    let mut best: Option<(f32, Vec3, NodeHandle)> = None;
    if let Some(assets) = tree.world().resource::<RenderAssets>() {
        for node in tree.subtree(editor.scene_root) {
            if !tree.is_visible_in_tree(node) {
                continue;
            }
            let Some(mesh) = tree.component::<MeshInstance3D>(node).map(|m| *m) else {
                continue;
            };
            let (Some(aabb), Some(global)) =
                (assets.mesh_aabb(mesh.mesh), tree.global_transform(node))
            else {
                continue;
            };
            let inverse = global.inverse();
            let hit = ray_aabb_normal(
                inverse.transform_point3(origin),
                inverse.transform_vector3(dir),
                aabb.min,
                aabb.max,
            );
            if let Some((t, local)) = hit {
                if best.is_none_or(|(d, _, _)| t < d) {
                    let world = Vec3::from(global.matrix3 * ab_engine::core::Vec3A::from(local));
                    best = Some((t, crate::build::dominant_axis(world), node));
                }
            }
        }
    }
    match best {
        Some((t, normal, node)) => crate::build::Surface {
            point: origin + dir * t,
            normal,
            support: selectable_owner(editor, node)
                .and_then(|owner| editor.world_bounds(owner))
                .or_else(|| tree.mesh_bounds(node)),
        },
        None => crate::build::Surface {
            point: ground_point(projector, pos),
            normal: Vec3::Y,
            support: None,
        },
    }
}

/// The box around a collision shape, in the shape node's space.
fn shape_bounds(shape: &Shape) -> Option<Aabb> {
    let extents = match shape {
        Shape::Box { size } => *size * 0.5,
        Shape::Sphere { radius } => Vec3::splat(*radius),
        Shape::Capsule { radius, height } => {
            Vec3::new(*radius, (height * 0.5).max(*radius), *radius)
        }
        Shape::Cylinder { radius, height } => Vec3::new(*radius, height * 0.5, *radius),
        Shape::ConvexHull { points } => return Aabb::from_points(points.iter().copied()),
        Shape::TriMesh { vertices, .. } => return Aabb::from_points(vertices.iter().copied()),
    };
    let extents = extents.abs();
    Some(Aabb {
        min: -extents,
        max: extents,
    })
}

/// The world box around the collision shapes under `node`.
fn subtree_shape_bounds(editor: &Editor, node: NodeHandle) -> Option<Aabb> {
    let tree = editor.tree();
    tree.subtree(node)
        .into_iter()
        .filter_map(|n| {
            let bounds = tree
                .component::<CollisionShape3D>(n)
                .and_then(|c| shape_bounds(c.shape()))?;
            let global = tree.global_transform(n)?;
            Aabb::from_points(box_corners(&global, bounds.min, bounds.max))
        })
        .reduce(|a, b| a.union(&b))
}

/// Where a screen point's ray meets the ground plane (or 10 m ahead).
fn ground_point(projector: &Projector, pos: Pos2) -> Vec3 {
    let (origin, dir) = projector.ray(pos);
    // Front and side views look along the ground: the point on the plane through the
    // world origin facing the view.
    if projector.ortho.is_some() && dir.y.abs() < 0.5 {
        return origin + dir * dir.dot(-origin);
    }
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

/// The grid of the view's plane (the ground, or the XY / ZY plane of the front and side
/// views), around the camera's pivot, with the two world axes in it highlighted.
fn draw_grid(
    painter: &egui::Painter,
    projector: &Projector,
    camera: &EditorCamera,
    kind: ViewKind,
) {
    let (u, v) = kind.grid_axes();
    let distance = projector.view_distance(camera.pivot).max(camera.distance);
    let step = if distance > 60.0 { 10.0 } else { 1.0 };
    let half = 40;
    let cu = (camera.pivot.dot(u) / step).round() * step;
    let cv = (camera.pivot.dot(v) / step).round() * step;
    let extent = half as f32 * step;
    let at = |a: f32, b: f32| u * a + v * b;
    for i in -half..=half {
        let i: i32 = i;
        let offset = i as f32 * step;
        let fade = 1.0 - (i.unsigned_abs() as f32 / half as f32);
        let alpha = (fade * fade * 22.0) as u8;
        if alpha == 0 {
            continue;
        }
        let a = cu + offset;
        let major = (a / step).round() as i64 % 10 == 0;
        let color = Color32::from_white_alpha(if major {
            alpha.saturating_mul(2)
        } else {
            alpha
        });
        if a.abs() > 1e-4 {
            line(
                painter,
                projector,
                at(a, cv - extent),
                at(a, cv + extent),
                Stroke::new(1.0_f32, color),
            );
        }
        let b = cv + offset;
        if b.abs() > 1e-4 {
            line(
                painter,
                projector,
                at(cu - extent, b),
                at(cu + extent, b),
                Stroke::new(1.0_f32, color),
            );
        }
    }
    let axis_color = |axis: Vec3| {
        if axis == Vec3::X {
            theme::AXIS_X
        } else if axis == Vec3::Y {
            theme::AXIS_Y
        } else {
            theme::AXIS_Z
        }
    };
    let axis = |color: Color32| Stroke::new(1.5_f32, color.gamma_multiply(0.75));
    line(
        painter,
        projector,
        at(cu - extent, 0.0),
        at(cu + extent, 0.0),
        axis(axis_color(u)),
    );
    line(
        painter,
        projector,
        at(0.0, cv - extent),
        at(0.0, cv + extent),
        axis(axis_color(v)),
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

/// The objects whose screen centre is inside `rect`, in tree order: meshes by the centre of
/// their bounds, lights, cameras and other icon nodes by their position. A mesh or collision
/// shape of a physics body stands for the body, so a box around crates selects the crates.
fn marquee_nodes(editor: &Editor, projector: &Projector, rect: Rect) -> Vec<NodeHandle> {
    let tree = editor.tree();
    let mut found = Vec::new();
    for node in tree.subtree(editor.scene_root) {
        if node == editor.scene_root || !tree.is_visible_in_tree(node) {
            continue;
        }
        let shape = tree
            .component::<CollisionShape3D>(node)
            .and_then(|c| shape_bounds(c.shape()));
        let center = if tree.component::<MeshInstance3D>(node).is_some() {
            tree.mesh_bounds(node).map(|b| b.center())
        } else if let Some(shape) = shape {
            tree.global_transform(node)
                .map(|global| global.transform_point3(shape.center()))
        } else if tree
            .class_name(node)
            .is_some_and(|c| icon_shown(editor, node, c))
        {
            tree.global_position(node)
        } else {
            None
        };
        let Some(at) = center.and_then(|c| projector.project(c)) else {
            continue;
        };
        if !rect.contains(at) {
            continue;
        }
        let Some(owner) = selectable_owner(editor, node) else {
            continue;
        };
        if !found.contains(&owner) {
            found.push(owner);
        }
    }
    found
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

/// Outlines `node`'s meshes; the active node of a selection is drawn brighter. Its
/// collision shapes show too, fainter and under the outline, so a shape the size of its
/// mesh does not hide it.
fn draw_selection(
    painter: &egui::Painter,
    projector: &Projector,
    editor: &Editor,
    node: NodeHandle,
    active: bool,
) {
    let tree = editor.tree();
    let world = tree.world();
    let Some(assets) = world.resource::<RenderAssets>() else {
        return;
    };
    let stroke = if active {
        Stroke::new(1.5_f32, theme::SELECTION)
    } else {
        Stroke::new(1.0_f32, theme::SELECTION.gamma_multiply(0.6))
    };
    let shape_stroke = Stroke::new(
        1.0_f32,
        theme::PLAY.gamma_multiply(if active { 0.7 } else { 0.4 }),
    );
    let mut any = false;
    let nodes = tree.subtree(node);
    for &n in &nodes {
        if let (Some(shape), Some(global)) = (
            tree.component::<CollisionShape3D>(n),
            tree.global_transform(n),
        ) {
            draw_shape(painter, projector, &global, shape.shape(), shape_stroke);
            any = true;
        }
    }
    for &n in &nodes {
        let Some(mesh) = tree.component::<MeshInstance3D>(n).map(|m| *m) else {
            continue;
        };
        // Skinned meshes: their posed world box. Others: their own box, turned with them.
        let corners = if tree.component::<Skin>(n).is_some() {
            tree.mesh_bounds(n).map(|b| b.corners())
        } else {
            assets
                .mesh_aabb(mesh.mesh)
                .zip(tree.global_transform(n))
                .map(|(aabb, global)| box_corners(&global, aabb.min, aabb.max))
        };
        let Some(corners) = corners else {
            continue;
        };
        draw_box(painter, projector, &corners, stroke);
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

/// A thin outline around the object under the pointer.
fn draw_hover(painter: &egui::Painter, projector: &Projector, editor: &Editor, node: NodeHandle) {
    let tree = editor.tree();
    let Some(assets) = tree.world().resource::<RenderAssets>() else {
        return;
    };
    let stroke = Stroke::new(1.0_f32, Color32::from_white_alpha(110));
    let mut any = false;
    for n in tree.subtree(node) {
        let corners = match tree.component::<MeshInstance3D>(n).map(|m| *m) {
            Some(_) if tree.component::<Skin>(n).is_some() => {
                tree.mesh_bounds(n).map(|b| b.corners())
            }
            Some(mesh) => assets
                .mesh_aabb(mesh.mesh)
                .zip(tree.global_transform(n))
                .map(|(aabb, global)| box_corners(&global, aabb.min, aabb.max)),
            None => None,
        };
        if let Some(corners) = corners {
            draw_box(painter, projector, &corners, stroke);
            any = true;
        }
    }
    if !any {
        if let Some(bounds) = subtree_shape_bounds(editor, node) {
            draw_box(painter, projector, &bounds.corners(), stroke);
        } else if let Some(at) = tree
            .global_position(node)
            .and_then(|p| projector.project(p))
        {
            painter.circle_stroke(at, 14.0, stroke);
        }
    }
}

/// The snap steps, editable.
pub fn snap_settings(ui: &mut egui::Ui, snap: &mut SnapSettings) {
    ui.set_min_width(220.0);
    ui.checkbox(&mut snap.enabled, t("snap.title"));
    egui::Grid::new("snap_steps").num_columns(2).show(ui, |ui| {
        ui.label(t("snap.move"));
        ui.add(
            egui::DragValue::new(&mut snap.translate)
                .custom_parser(crate::panels::parse_finite)
                .range(0.001..=1000.0)
                .speed(0.01)
                .suffix(" m"),
        );
        ui.end_row();
        ui.label(t("snap.rotate"));
        ui.add(
            egui::DragValue::new(&mut snap.rotate)
                .custom_parser(crate::panels::parse_finite)
                .range(0.1..=180.0)
                .speed(0.5)
                .suffix("°"),
        );
        ui.end_row();
        ui.label(t("snap.scale"));
        ui.add(
            egui::DragValue::new(&mut snap.scale)
                .custom_parser(crate::panels::parse_finite)
                .range(0.001..=100.0)
                .speed(0.005),
        );
        ui.end_row();
    });
    ui.horizontal(|ui| {
        for (label, step) in [
            ("0.1 m", 0.1),
            ("0.25 m", 0.25),
            ("0.5 m", 0.5),
            ("1 m", 1.0),
        ] {
            if ui.small_button(label).clicked() {
                snap.translate = step;
            }
        }
    });
    if ui.button(t("snap.reset")).clicked() {
        *snap = SnapSettings {
            enabled: snap.enabled,
            ..SnapSettings::default()
        };
    }
}

/// The Debug panel's mesh bounds view while editing: every visible mesh's world box.
fn draw_mesh_bounds(painter: &egui::Painter, projector: &Projector, editor: &Editor) {
    let color32 = |c: Color| {
        Color32::from_rgb(
            (c.r * 255.0) as u8,
            (c.g * 255.0) as u8,
            (c.b * 255.0) as u8,
        )
    };
    ab_engine::rendering::for_each_mesh_bounds(editor.tree().world(), |mesh| {
        let color = if mesh.skinned {
            ab_engine::SKINNED_BOUNDS_COLOR
        } else {
            ab_engine::BOUNDS_COLOR
        };
        let stroke = Stroke::new(1.0_f32, color32(color).gamma_multiply(0.8));
        draw_box(painter, projector, &mesh.bounds.corners(), stroke);
    });
}

fn draw_collision(painter: &egui::Painter, projector: &Projector, editor: &Editor) {
    let tree = editor.tree();
    let stroke = Stroke::new(1.0_f32, theme::PLAY.gamma_multiply(0.85));
    for node in tree.subtree(editor.scene_root) {
        if let (Some(shape), Some(global)) = (
            tree.component::<CollisionShape3D>(node),
            tree.global_transform(node),
        ) {
            draw_shape(painter, projector, &global, shape.shape(), stroke);
        }
    }
}

/// A closed polyline through `points` (in the space of `global`).
fn local_polyline(
    painter: &egui::Painter,
    projector: &Projector,
    global: &Affine3A,
    points: impl IntoIterator<Item = Vec3>,
    stroke: Stroke,
) {
    let points: Vec<Vec3> = points
        .into_iter()
        .map(|p| global.transform_point3(p))
        .collect();
    for pair in points.windows(2) {
        line(painter, projector, pair[0], pair[1], stroke);
    }
}

/// Points of an arc in the shape's space: `center + (u cos a + v sin a) * radius`.
fn arc(center: Vec3, u: Vec3, v: Vec3, radius: f32, from: f32, to: f32) -> Vec<Vec3> {
    const STEPS: usize = 32;
    (0..=STEPS)
        .map(|i| {
            let angle = from + (to - from) * i as f32 / STEPS as f32;
            center + (u * angle.cos() + v * angle.sin()) * radius
        })
        .collect()
}

/// Wireframe of a collision shape placed at `global` (its scale applies, as in physics).
fn draw_shape(
    painter: &egui::Painter,
    projector: &Projector,
    global: &Affine3A,
    shape: &Shape,
    stroke: Stroke,
) {
    use std::f32::consts::{PI, TAU};
    let poly = |points: Vec<Vec3>| local_polyline(painter, projector, global, points, stroke);
    match shape {
        Shape::Box { size } => {
            draw_box(
                painter,
                projector,
                &box_corners(global, -*size * 0.5, *size * 0.5),
                stroke,
            );
        }
        Shape::Sphere { radius } => {
            for (u, v) in [(Vec3::X, Vec3::Y), (Vec3::Y, Vec3::Z), (Vec3::Z, Vec3::X)] {
                poly(arc(Vec3::ZERO, u, v, *radius, 0.0, TAU));
            }
        }
        Shape::Capsule { radius, height } => {
            // `height` includes both caps: the straight part is what is left.
            let half = (height * 0.5 - radius).max(0.0);
            let (top, bottom) = (Vec3::Y * half, Vec3::NEG_Y * half);
            for center in [top, bottom] {
                poly(arc(center, Vec3::X, Vec3::Z, *radius, 0.0, TAU));
            }
            for side in [Vec3::X, Vec3::Z] {
                poly(arc(top, side, Vec3::Y, *radius, 0.0, PI));
                poly(arc(bottom, side, Vec3::NEG_Y, *radius, 0.0, PI));
                for sign in [1.0, -1.0] {
                    poly(vec![
                        top + side * radius * sign,
                        bottom + side * radius * sign,
                    ]);
                }
            }
        }
        Shape::Cylinder { radius, height } => {
            let half = height * 0.5;
            for y in [-half, half] {
                poly(arc(Vec3::Y * y, Vec3::X, Vec3::Z, *radius, 0.0, TAU));
            }
            for side in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
                poly(vec![
                    side * *radius - Vec3::Y * half,
                    side * *radius + Vec3::Y * half,
                ]);
            }
        }
        Shape::TriMesh { vertices, indices } if indices.len() <= 4096 => {
            for triangle in indices.iter() {
                let corner = |i: u32| vertices.get(i as usize).copied();
                if let (Some(a), Some(b), Some(c)) = (
                    corner(triangle[0]),
                    corner(triangle[1]),
                    corner(triangle[2]),
                ) {
                    poly(vec![a, b, c, a]);
                }
            }
        }
        // Hulls and big meshes: their bounds.
        Shape::ConvexHull { .. } | Shape::TriMesh { .. } => {
            if let Some(bounds) = shape_bounds(shape) {
                draw_box(
                    painter,
                    projector,
                    &box_corners(global, bounds.min, bounds.max),
                    stroke,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapping_with_a_zero_step_or_a_zero_scale_stays_finite() {
        let p = Vec3::new(1.26, 0.4, -2.74);
        assert_eq!(snap_point(p, 0.0), p, "no grid");
        assert_eq!(snap_point(p, 0.5), Vec3::new(1.5, 0.5, -2.5));
        let snap = SnapSettings::default();
        assert_eq!(snap_scale_factor(&snap, 0.0, 1.7), 1.7, "a zero scale");
        assert!((snap_scale_factor(&snap, 1.0, 1.26) - 1.3).abs() < 1e-5);
        // A mirrored axis (negative scale) stays mirrored: factor > 0.
        assert!((snap_scale_factor(&snap, -1.0, 1.26) - 1.3).abs() < 1e-5);
    }

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
    fn box_selection_takes_bodies_inside_the_box() {
        let project =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let mut editor = Editor::new(project, None, ab_engine::audio::AudioOutput::Silent);
        editor.open_scene("slice/level.abscene");
        let mut camera = EditorCamera::default();
        let scene = editor.tree().subtree_bounds(editor.scene_root).unwrap();
        camera.frame(scene.center(), scene.size().length() * 0.5);
        let view = CameraView::new(&camera.global(), 60f32.to_radians(), 1.6, 0.05, 2000.0);
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0));
        let projector = Projector::new(&view, rect);

        let everything = marquee_nodes(&editor, &projector, rect);
        let pillar = editor.node_at("Geometry/Pillar0").unwrap();
        assert!(everything.contains(&pillar), "a body, not its Mesh child");
        assert!(!everything.contains(&editor.node_at("Geometry/Pillar0/Mesh").unwrap()));

        // A small box around the pillar's centre takes the pillar only.
        let center = projector
            .project(editor.tree().subtree_bounds(pillar).unwrap().center())
            .unwrap();
        let small = Rect::from_center_size(center, Vec2::splat(6.0));
        assert_eq!(marquee_nodes(&editor, &projector, small), [pillar]);
        let empty = Rect::from_min_size(Pos2::new(-50.0, -50.0), Vec2::splat(10.0));
        assert!(marquee_nodes(&editor, &projector, empty).is_empty());
    }

    /// The default camera looking at the origin, and a fresh empty scene.
    fn shapes_scene() -> (Editor, Projector) {
        let project =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let mut editor = Editor::new(project, None, ab_engine::audio::AudioOutput::Silent);
        editor.new_scene();
        for child in editor.tree().children(editor.scene_root) {
            editor.delete(child);
        }
        let mut camera = EditorCamera::default();
        camera.frame(Vec3::ZERO, 1.0);
        let view = CameraView::new(&camera.global(), 60f32.to_radians(), 1.6, 0.05, 2000.0);
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0));
        (editor, Projector::new(&view, rect))
    }

    fn template(label: &str) -> SceneFile {
        crate::create::PHYSICS
            .iter()
            .find(|t| t.label == label)
            .unwrap()
            .scene()
    }

    use ab_engine::scene_file::SceneFile;

    #[test]
    fn bodies_without_meshes_are_picked_by_their_shape() {
        let (mut editor, projector) = shapes_scene();
        editor.add_scene(template("Character"), Some(Vec3::ZERO));
        let character = editor.selection.unwrap();
        // Placed at the origin: the capsule spans -0.9..0.9.
        let center = projector.project(Vec3::new(0.0, 0.3, 0.0)).unwrap();
        editor.select(None);
        assert_eq!(pick(&editor, &projector, center), Some(character));
        let shape = editor.tree().get_node(character, "Shape").unwrap();
        assert_eq!(
            hit(&editor, &projector, center),
            Some(Hit {
                owner: character,
                part: shape
            })
        );
        // Framing and box selection find it too.
        assert!(subtree_shape_bounds(&editor, character).is_some());
        let middle = projector.project(Vec3::ZERO).unwrap();
        let around = Rect::from_center_size(middle, Vec2::splat(20.0));
        assert_eq!(marquee_nodes(&editor, &projector, around), [character]);
    }

    #[test]
    fn a_click_on_a_bodys_mesh_selects_the_body_so_mesh_and_shape_move_together() {
        let (mut editor, projector) = shapes_scene();
        editor.add_scene(template("Static Box"), Some(Vec3::ZERO));
        let wall = editor.selection.unwrap();
        let mesh = editor.tree().get_node(wall, "Mesh").unwrap();
        let at = projector.project(Vec3::ZERO).unwrap();
        assert_eq!(
            hit(&editor, &projector, at),
            Some(Hit {
                owner: wall,
                part: mesh
            })
        );
    }

    #[test]
    fn trigger_volumes_do_not_hide_what_is_inside() {
        let (mut editor, projector) = shapes_scene();
        editor.add_scene(template("Trigger Area"), Some(Vec3::ZERO));
        let area = editor.selection.unwrap();
        editor.select(None);
        editor.add_scene(crate::create::MESHES[0].scene(), Some(Vec3::ZERO));
        let cube = editor.selection.unwrap();
        // Off-centre, away from the area's icon: the cube inside wins over the volume.
        let at = projector.project(Vec3::new(0.3, 0.3, 0.3)).unwrap();
        assert_eq!(pick(&editor, &projector, at), Some(cube));
        // Outside the cube but inside the volume: the area.
        let at = projector.project(Vec3::new(0.0, 0.9, 0.0)).unwrap();
        assert_eq!(pick(&editor, &projector, at), Some(area));
    }

    #[test]
    fn shape_bounds_follow_the_shape() {
        let capsule = shape_bounds(&Shape::capsule(0.4, 1.8)).unwrap();
        assert_eq!(capsule.max, Vec3::new(0.4, 0.9, 0.4));
        let sphere = shape_bounds(&Shape::sphere(0.5)).unwrap();
        assert_eq!(sphere.min, Vec3::splat(-0.5));
        let hull = shape_bounds(&Shape::convex_hull(vec![Vec3::ZERO, Vec3::ONE])).unwrap();
        assert_eq!((hull.min, hull.max), (Vec3::ZERO, Vec3::ONE));
    }

    /// Runs the viewport for one frame with `events`, on a 1200 × 800 window.
    fn frame(
        ctx: &egui::Context,
        viewport: &mut Viewport,
        editor: &mut Editor,
        events: Vec<egui::Event>,
    ) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| viewport.show(ui, editor, None));
        output.textures_delta.clear();
    }

    fn right_click(at: Pos2) -> [Vec<egui::Event>; 3] {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        [
            vec![egui::Event::PointerMoved(at)],
            vec![button(true)],
            vec![button(false)],
        ]
    }

    #[test]
    fn right_click_opens_a_menu_for_the_object_or_the_scene() {
        let (mut editor, _) = shapes_scene();
        // At the camera's pivot: the middle of the view.
        let pivot = EditorCamera::default().pivot;
        editor.add_scene(crate::create::MESHES[0].scene(), Some(pivot));
        let cube = editor.selection.unwrap();
        editor.select(None);
        let ctx = egui::Context::default();
        let mut viewport = Viewport::new(None);
        frame(&ctx, &mut viewport, &mut editor, Vec::new());

        for events in right_click(Pos2::new(600.0, 400.0)) {
            frame(&ctx, &mut viewport, &mut editor, events);
        }
        frame(&ctx, &mut viewport, &mut editor, Vec::new());
        assert_eq!(
            editor.selection,
            Some(cube),
            "the object is selected for its menu"
        );
        assert!(matches!(viewport.menu_target, Some((Some(n), _)) if n == cube));
        assert!(egui::Popup::is_any_open(&ctx), "the menu is open");

        // Empty space: the scene menu, the selection left alone.
        for events in right_click(Pos2::new(40.0, 760.0)) {
            frame(&ctx, &mut viewport, &mut editor, events);
        }
        frame(&ctx, &mut viewport, &mut editor, Vec::new());
        assert!(matches!(viewport.menu_target, Some((None, _))));
        assert_eq!(editor.selection, Some(cube));
        assert!(egui::Popup::is_any_open(&ctx));
    }

    #[test]
    fn the_build_tool_places_pieces_on_the_grid_and_stacks_them() {
        let (mut editor, _) = shapes_scene();
        let ctx = egui::Context::default();
        let mut viewport = Viewport::new(None);
        viewport.options.snap.translate = 1.0;
        editor.build = Some(crate::state::BuildPiece {
            path: "pieces/floor_tile.abscene".into(),
            turns: 0,
        });
        let before = editor.tree().children(editor.scene_root).len();
        let click = |at: Pos2| {
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            [
                vec![egui::Event::PointerMoved(at)],
                vec![button(true)],
                vec![button(false)],
            ]
        };
        let middle = Pos2::new(600.0, 400.0);
        for events in click(middle) {
            frame(&ctx, &mut viewport, &mut editor, events);
        }
        let placed: Vec<_> = editor.tree().children(editor.scene_root);
        assert_eq!(placed.len(), before + 1, "one piece placed");
        let tile = *placed.last().unwrap();
        let p = editor.tree().global_position(tile).unwrap();
        assert_eq!(p.y, 0.0, "on the ground");
        assert_eq!((p.x.fract(), p.z.fract()), (0.0, 0.0), "on the grid: {p}");
        // The ghost is not part of the scene.
        assert!(editor
            .tree()
            .subtree(editor.scene_root)
            .iter()
            .all(|&n| editor.tree().name(n).as_deref() != Some("BuildGhost")));

        // A second click on the tile: stacked on top of it.
        for events in click(middle) {
            frame(&ctx, &mut viewport, &mut editor, events);
        }
        let top = *editor.tree().children(editor.scene_root).last().unwrap();
        let q = editor.tree().global_position(top).unwrap();
        assert!((q.y - 0.2).abs() < 1e-3, "on the tile's top: {q}");

        // Done: the ghost goes away.
        editor.build = None;
        editor.tick(0.0);
        assert!(editor.tree().children(editor.tree().root()).len() == 1);
    }

    #[test]
    fn path_points_drag_add_and_delete_with_undo() {
        use ab_engine::scene_file::path::Path3D;
        let (mut editor, _) = shapes_scene();
        let ctx = egui::Context::default();
        let mut viewport = Viewport::new(None);
        editor.select(None);
        editor.add_scene(crate::create::PATHS[0].scene(), Some(Vec3::ZERO));
        let path = editor.selection.unwrap();
        editor.end_gesture();
        frame(&ctx, &mut viewport, &mut editor, Vec::new());
        let points = |editor: &Editor| {
            let node = editor.selection.unwrap();
            editor
                .tree()
                .component::<Path3D>(node)
                .unwrap()
                .desc
                .points
                .clone()
        };
        let before = points(&editor);
        assert_eq!(before.len(), 3);
        // Where the middle point is on screen (the view of `frame`: 1200 × 800).
        let view = CameraView::new(
            &viewport.camera.global(),
            60f32.to_radians(),
            1.5,
            0.05,
            2000.0,
        );
        let projector = Projector::new(
            &view,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0)),
        );
        let middle = projector
            .project(
                editor
                    .tree()
                    .global_transform(path)
                    .unwrap()
                    .transform_point3(Vec3::from(before[1])),
            )
            .unwrap();
        let primary = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            &ctx,
            &mut viewport,
            &mut editor,
            vec![egui::Event::PointerMoved(middle)],
        );
        frame(
            &ctx,
            &mut viewport,
            &mut editor,
            vec![primary(middle, true)],
        );
        for step in 1..=8 {
            let pos = middle + Vec2::new(step as f32 * 6.0, 0.0);
            frame(
                &ctx,
                &mut viewport,
                &mut editor,
                vec![egui::Event::PointerMoved(pos)],
            );
        }
        let end = middle + Vec2::new(48.0, 0.0);
        frame(&ctx, &mut viewport, &mut editor, vec![primary(end, false)]);
        frame(&ctx, &mut viewport, &mut editor, Vec::new());
        let moved = points(&editor);
        assert!(moved[1][0] > before[1][0] + 0.2, "{moved:?}");
        assert_eq!(moved[1][1], before[1][1], "level");
        editor.undo();
        assert_eq!(points(&editor), before, "one undo step back");

        // Right-click a point: gone.
        let first = projector
            .project(
                editor
                    .tree()
                    .global_transform(editor.selection.unwrap())
                    .unwrap()
                    .transform_point3(Vec3::from(before[0])),
            )
            .unwrap();
        let secondary = |pressed| egui::Event::PointerButton {
            pos: first,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [
            vec![egui::Event::PointerMoved(first)],
            vec![secondary(true)],
            vec![secondary(false)],
            vec![],
        ] {
            frame(&ctx, &mut viewport, &mut editor, events);
        }
        assert_eq!(points(&editor).len(), 2);
        assert!(!egui::Popup::is_any_open(&ctx), "no context menu for it");
    }

    #[test]
    fn dragging_with_the_right_button_flies_without_a_menu() {
        let (mut editor, _) = shapes_scene();
        let ctx = egui::Context::default();
        let mut viewport = Viewport::new(None);
        frame(&ctx, &mut viewport, &mut editor, Vec::new());
        let start = Pos2::new(600.0, 400.0);
        let yaw = viewport.camera.yaw;
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            &ctx,
            &mut viewport,
            &mut editor,
            vec![egui::Event::PointerMoved(start)],
        );
        frame(&ctx, &mut viewport, &mut editor, vec![button(start, true)]);
        for step in 1..=10 {
            let pos = start + Vec2::new(step as f32 * 12.0, 0.0);
            frame(
                &ctx,
                &mut viewport,
                &mut editor,
                vec![egui::Event::PointerMoved(pos)],
            );
        }
        let end = start + Vec2::new(120.0, 0.0);
        frame(&ctx, &mut viewport, &mut editor, vec![button(end, false)]);
        frame(&ctx, &mut viewport, &mut editor, Vec::new());
        assert!(viewport.camera.yaw != yaw, "the camera turned");
        assert!(viewport.menu_target.is_none());
        assert!(!egui::Popup::is_any_open(&ctx));
    }

    /// The gizmo's centre handle moves in the plane of the view: in the side view (which
    /// looks along -X) it used to scale by world X's on-screen length (nothing) and move
    /// along the perspective camera's axes, throwing the object far away.
    #[test]
    fn the_centre_handle_moves_in_the_plane_of_an_orthographic_view() {
        let (mut editor, _) = shapes_scene();
        let pivot = EditorCamera::default().pivot;
        editor.add_scene(crate::create::MESHES[0].scene(), Some(pivot));
        let cube = editor.selection.unwrap();
        editor.end_gesture();
        let ctx = egui::Context::default();
        let mut viewport = Viewport::new(None);
        viewport.kind = ViewKind::Side;
        viewport.options.gizmo = GizmoMode::Translate;
        frame(&ctx, &mut viewport, &mut editor, Vec::new());
        // The pivot is in the middle of the view.
        let start = Pos2::new(600.0, 400.0);
        let primary = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            &ctx,
            &mut viewport,
            &mut editor,
            vec![egui::Event::PointerMoved(start)],
        );
        frame(&ctx, &mut viewport, &mut editor, vec![primary(start, true)]);
        for step in 1..=10 {
            let pos = start + Vec2::new(step as f32 * 3.0, 0.0);
            frame(
                &ctx,
                &mut viewport,
                &mut editor,
                vec![egui::Event::PointerMoved(pos)],
            );
        }
        let end = start + Vec2::new(30.0, 0.0);
        frame(&ctx, &mut viewport, &mut editor, vec![primary(end, false)]);
        let moved = editor.tree().global_position(cube).unwrap() - pivot;
        // Screen right in the side view is world -Z.
        assert!(moved.z < -0.1 && moved.z > -2.0, "{moved}");
        assert!(moved.x.abs() < 1e-3 && moved.y.abs() < 1e-3, "{moved}");
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
