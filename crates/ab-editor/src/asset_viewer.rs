//! The asset viewer (double-click in the asset browser): every asset in a native window of
//! its own (several side by side), with the tools that suit it on a tool bar and its
//! details on the right. Its actions reveal it in the asset browser, add a model or scene to
//! the edited scene, open a scene, or read the file again.
//!
//! * Images and textures: zoom (wheel), pan (drag), fit or 1:1, a single channel at a time,
//!   the pixel under the pointer, a checkerboard under transparency. Normal maps also show
//!   lit, with a light you can turn.
//! * Models and scenes: orbit (drag), zoom (wheel), turn the light (right drag); their
//!   nodes, meshes, vertices, triangles, materials, textures, skin and animations.
//! * Materials: on a sphere, a cube or a plane, under a light you can turn; their color,
//!   roughness, metallic, emission and textures.

use crate::preview::{Orbit, Previewer, Shape, Subject, SubjectInfo};
use crate::project::AssetKind;
use crate::theme;
use ab_engine::core::jobs::{self, Task};
use ab_engine::rendering::{GpuContext, OffscreenTarget};
use ab_engine::scene_file::MaterialDesc;
use eframe::egui::{self, Color32, RichText};
use eframe::egui_wgpu;
use egui_phosphor::regular as icon;
use std::path::{Path, PathBuf};

/// A decoded image, full size, for inspection.
struct ImageData {
    rgba: image::RgbaImage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    All,
    Red,
    Green,
    Blue,
    Alpha,
    /// A normal map lit by a light (normal maps only).
    Lit,
}

struct ImageView {
    loading: Option<Task<Result<ImageData, String>>>,
    data: Option<ImageData>,
    texture: Option<egui::TextureHandle>,
    channel: Channel,
    /// The channel `texture` shows.
    shown: Option<(Channel, i32)>,
    zoom: f32,
    pan: egui::Vec2,
    fit: bool,
    normal_map: bool,
    light: f32,
}

struct RenderView {
    subject: Subject,
    orbit: Orbit,
    target: Option<OffscreenTarget>,
    texture: Option<egui::TextureId>,
    info: Option<SubjectInfo>,
    material: Option<MaterialDesc>,
    /// What the image shows: drawn again only when the subject, the view or the size
    /// changes.
    drawn: Option<(Subject, Orbit, (u32, u32))>,
}

// The variants' sizes do not matter: a few viewer windows at most.
#[allow(clippy::large_enum_variant)]
enum Body {
    Image(ImageView),
    Render(RenderView),
}

struct Open {
    path: String,
    kind: AssetKind,
    body: Body,
    error: Option<String>,
    file_size: Option<u64>,
}

/// One asset's window. Models, scenes and materials get a previewer of their own, so every
/// window keeps its subject loaded.
struct ViewerWindow {
    /// Names the window (`asset-viewer-<slot>`): the lowest free one, so the preview
    /// window keeps its place on the desktop from one asset to the next.
    slot: usize,
    open: Open,
    previewer: Option<Previewer>,
    /// Kept open: the next asset opens in another window instead of replacing this one.
    pinned: bool,
}

/// The asset windows (double-click in the asset browser). Like a preview: the next asset
/// replaces the one shown, unless that window is pinned.
#[derive(Default)]
pub struct AssetViewer {
    windows: Vec<ViewerWindow>,
    /// Windows asked for again: raised next frame.
    focus: Vec<usize>,
    /// Images of closed or replaced assets, freed next frame.
    freed: Vec<egui::TextureId>,
}

/// What a viewer window asks of the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewerAction {
    /// Show the asset in the asset browser.
    Reveal(String),
    /// Add the model or scene to the edited scene.
    AddToScene(String),
    /// Open the scene for editing.
    OpenScene(String),
}

/// At most this many viewer windows: opening another closes the oldest.
const MAX_WINDOWS: usize = 6;

/// Whether the viewer can show `kind`.
pub fn can_view(kind: AssetKind) -> bool {
    matches!(
        kind,
        AssetKind::Image | AssetKind::Model | AssetKind::Material | AssetKind::Scene
    )
}

fn body(project: &Path, path: &str, kind: AssetKind) -> Body {
    let file = project.join(path);
    let render = |subject| {
        Body::Render(RenderView {
            subject,
            orbit: Orbit::default(),
            target: None,
            texture: None,
            info: None,
            material: None,
            drawn: None,
        })
    };
    match kind {
        AssetKind::Image => {
            let name = file
                .file_stem()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let decode = file.clone();
            Body::Image(ImageView {
                loading: Some(jobs::spawn_background("ab-image-viewer", move || {
                    image::open(&decode)
                        .map(|i| ImageData { rgba: i.to_rgba8() })
                        .map_err(|e| format!("{}: {e}", decode.display()))
                })),
                data: None,
                texture: None,
                channel: Channel::All,
                shown: None,
                zoom: 1.0,
                pan: egui::Vec2::ZERO,
                fit: true,
                normal_map: crate::thumbnails::looks_like_normal_map(&name, &[]),
                light: 0.6,
            })
        }
        AssetKind::Material => {
            let mut body = render(Subject::Material(path.to_owned(), Shape::Sphere));
            if let Body::Render(view) = &mut body {
                view.material = std::fs::read_to_string(&file)
                    .ok()
                    .and_then(|t| MaterialDesc::from_toml(&t).ok());
            }
            body
        }
        AssetKind::Scene => render(Subject::Scene(path.to_owned())),
        _ => render(Subject::Model(path.to_owned())),
    }
}

fn opened(project: &Path, path: &str, kind: AssetKind) -> Open {
    Open {
        path: path.to_owned(),
        kind,
        body: body(project, path, kind),
        error: None,
        file_size: std::fs::metadata(project.join(path)).ok().map(|m| m.len()),
    }
}

fn window_key(slot: usize) -> String {
    format!("asset-viewer-{slot}")
}

/// The image a render view registered with the renderer, if any.
fn image_of(open: &Open) -> Option<egui::TextureId> {
    match &open.body {
        Body::Render(view) => view.texture,
        Body::Image(_) => None,
    }
}

impl AssetViewer {
    /// Shows `path` (a project asset of `kind`): raises its window when it is open, else
    /// shows it in the preview window (the last unpinned one), else in a new window.
    /// Returns false when every window is pinned and no more can open (pinned ones stay).
    pub fn open(&mut self, project: &Path, path: &str, kind: AssetKind) -> bool {
        if let Some(window) = self.windows.iter().find(|w| w.open.path == path) {
            self.focus.push(window.slot);
            return true;
        }
        if let Some(window) = self.windows.iter_mut().rev().find(|w| !w.pinned) {
            self.freed.extend(image_of(&window.open));
            window.open = opened(project, path, kind);
            self.focus.push(window.slot);
            return true;
        }
        if self.windows.len() >= MAX_WINDOWS {
            return false;
        }
        let slot = (0..)
            .find(|slot| self.windows.iter().all(|w| w.slot != *slot))
            .expect("fewer windows than slots");
        self.windows.push(ViewerWindow {
            slot,
            open: opened(project, path, kind),
            previewer: None,
            pinned: false,
        });
        true
    }

    /// Whether any asset window is open.
    pub fn is_open(&self) -> bool {
        !self.windows.is_empty()
    }

    pub fn close_all(&mut self) {
        for window in self.windows.drain(..) {
            self.freed.extend(image_of(&window.open));
        }
    }

    /// Shows every asset window; returns what they asked for.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        host: &mut crate::tool_window::Host,
        project: &Path,
        render_state: Option<&egui_wgpu::RenderState>,
    ) -> Vec<ViewerAction> {
        for slot in self.focus.drain(..) {
            crate::tool_window::focus(ctx, &window_key(slot));
        }
        let mut actions = Vec::new();
        let mut index = 0;
        while index < self.windows.len() {
            let window = &mut self.windows[index];
            let mut title = window.open.path.clone();
            if !window.pinned {
                title.push_str(&format!("  ·  {}", crate::i18n::t("viewer.preview")));
            }
            let spec = crate::tool_window::Spec {
                key: window_key(window.slot),
                icon: window.open.kind.icon(),
                title,
                size: [980.0, 680.0],
                min_size: [520.0, 360.0],
                dockable: false,
                remember: true,
                escape_closes: true,
            };
            let response = host.show(ctx, &spec, |ui| {
                window.show(ui, project, render_state, &mut actions);
            });
            if response.close {
                let window = self.windows.remove(index);
                self.freed.extend(image_of(&window.open));
            } else {
                index += 1;
            }
        }
        // Their images go back to the renderer.
        if let Some(state) = render_state {
            for id in self.freed.drain(..) {
                state.renderer.write().free_texture(&id);
            }
        }
        actions
    }
}

impl ViewerWindow {
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        project: &Path,
        render_state: Option<&egui_wgpu::RenderState>,
        actions: &mut Vec<ViewerAction>,
    ) {
        let key = window_key(self.slot);
        crate::tool_window::toolbar(ui, &key, |ui| {
            if let Some(action) = toolbar(ui, &mut self.open, &mut self.pinned) {
                match action {
                    Toolbar::Reload => {
                        self.open.error = None;
                        match &mut self.open.body {
                            Body::Render(view) => {
                                view.info = None;
                                view.drawn = None;
                            }
                            body @ Body::Image(_) => {
                                *body = self::body(project, &self.open.path, self.open.kind)
                            }
                        }
                        self.open.file_size = std::fs::metadata(project.join(&self.open.path))
                            .ok()
                            .map(|m| m.len());
                    }
                    Toolbar::Editor(action) => actions.push(action),
                }
            }
        });
        crate::tool_window::side(
            ui,
            (&key, "details"),
            false,
            250.0,
            icon::INFO,
            &crate::i18n::t("viewer.details"),
            |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| details(ui, &mut self.open));
            },
        );
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::BG_0))
            .show(ui, |ui| match &mut self.open.body {
                Body::Image(view) => image_view(ui, view, &mut self.open.error),
                Body::Render(view) => render_view(
                    ui,
                    view,
                    &mut self.previewer,
                    project,
                    render_state,
                    &mut self.open.error,
                ),
            });
    }
}

enum Toolbar {
    Reload,
    Editor(ViewerAction),
}

/// The view's controls on the left, the asset's actions on the right.
fn toolbar(ui: &mut egui::Ui, open: &mut Open, pinned: &mut bool) -> Option<Toolbar> {
    use crate::i18n::t;
    let mut action = None;
    match &mut open.body {
        Body::Image(view) => {
            for (channel, name) in [
                (Channel::All, "RGBA"),
                (Channel::Red, "R"),
                (Channel::Green, "G"),
                (Channel::Blue, "B"),
                (Channel::Alpha, "A"),
            ] {
                ui.selectable_value(&mut view.channel, channel, name);
            }
            if view.normal_map {
                ui.selectable_value(&mut view.channel, Channel::Lit, "Lit")
                    .on_hover_text(t("viewer.lit_hint"));
            }
            if view.channel == Channel::Lit {
                ui.add(
                    egui::Slider::new(
                        &mut view.light,
                        -std::f32::consts::PI..=std::f32::consts::PI,
                    )
                    .show_value(false),
                )
                .on_hover_text(t("viewer.light"));
            }
            ui.separator();
            if ui
                .button(format!("{}  {}", icon::CORNERS_OUT, t("ui_designer.fit")))
                .clicked()
            {
                view.fit = true;
            }
            if ui.button("1:1").clicked() {
                view.fit = false;
                view.zoom = 1.0;
                view.pan = egui::Vec2::ZERO;
            }
            ui.label(RichText::new(format!("{:.0} %", view.zoom * 100.0)).color(theme::TEXT_DIM));
            ui.separator();
            ui.checkbox(&mut view.normal_map, t("viewer.normal_map"))
                .on_hover_text("A tangent-space normal map: shown lit, loaded as linear data");
        }
        Body::Render(view) => {
            if let Subject::Material(_, shape) = &mut view.subject {
                for (s, name) in [
                    (Shape::Sphere, "Sphere"),
                    (Shape::Cube, "Cube"),
                    (Shape::Plane, "Plane"),
                ] {
                    ui.selectable_value(shape, s, name);
                }
                ui.separator();
            }
            if ui
                .button(format!(
                    "{}  {}",
                    icon::ARROW_COUNTER_CLOCKWISE,
                    t("viewer.reset_view")
                ))
                .clicked()
            {
                view.orbit = Orbit::default();
            }
            ui.label(
                RichText::new(t("viewer.render_hint"))
                    .small()
                    .color(theme::TEXT_DIM),
            );
        }
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.toggle_value(pinned, icon::PUSH_PIN)
            .on_hover_text(t("viewer.pin_hint"));
        if ui
            .button(icon::ARROW_CLOCKWISE)
            .on_hover_text(t("viewer.reload"))
            .clicked()
        {
            action = Some(Toolbar::Reload);
        }
        if ui
            .button(format!("{}  {}", icon::FOLDER_OPEN, t("viewer.reveal")))
            .clicked()
        {
            action = Some(Toolbar::Editor(ViewerAction::Reveal(open.path.clone())));
        }
        if matches!(open.kind, AssetKind::Model | AssetKind::Scene)
            && ui
                .button(format!("{}  {}", icon::PLUS, t("viewer.add_to_scene")))
                .clicked()
        {
            action = Some(Toolbar::Editor(ViewerAction::AddToScene(open.path.clone())));
        }
        if open.kind == AssetKind::Scene
            && ui
                .button(format!(
                    "{}  {}",
                    icon::PENCIL_SIMPLE,
                    t("viewer.open_scene")
                ))
                .clicked()
        {
            action = Some(Toolbar::Editor(ViewerAction::OpenScene(open.path.clone())));
        }
    });
    action
}

fn details(ui: &mut egui::Ui, open: &mut Open) {
    let row = |ui: &mut egui::Ui, name: &str, value: String| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).color(theme::TEXT_DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(value);
            });
        });
    };
    row(ui, "Type", format!("{:?}", open.kind));
    if let Some(size) = open.file_size {
        row(ui, "File size", human_bytes(size));
    }
    match &mut open.body {
        Body::Image(view) => {
            if let Some(data) = &view.data {
                row(
                    ui,
                    "Size",
                    format!("{} × {} px", data.rgba.width(), data.rgba.height()),
                );
                let transparent = data.rgba.pixels().any(|p| p[3] < 255);
                row(
                    ui,
                    "Alpha",
                    if transparent { "used" } else { "opaque" }.into(),
                );
                row(
                    ui,
                    "Memory (GPU, mips)",
                    human_bytes(
                        u64::from(data.rgba.width()) * u64::from(data.rgba.height()) * 4 * 4 / 3,
                    ),
                );
            }
        }
        Body::Render(view) => {
            if let Some(info) = &view.info {
                row(ui, "Nodes", info.nodes.to_string());
                row(ui, "Meshes", info.meshes.to_string());
                row(ui, "Vertices", info.vertices.to_string());
                row(ui, "Triangles", info.triangles.to_string());
                row(ui, "Materials", info.materials.to_string());
                row(ui, "Textures", info.textures.to_string());
                if let Some(size) = info.size {
                    row(
                        ui,
                        "Size (m)",
                        format!("{:.2} × {:.2} × {:.2}", size.x, size.y, size.z),
                    );
                }
                row(
                    ui,
                    "Skinned",
                    if info.skinned { "yes" } else { "no" }.into(),
                );
                if !info.animations.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Animations").color(theme::TEXT_DIM));
                    for name in &info.animations {
                        ui.label(format!("{}  {name}", icon::FILM_STRIP));
                    }
                }
            }
            if let Some(m) = &view.material {
                ui.add_space(6.0);
                ui.label(RichText::new("Material").font(theme::bold(13.0)));
                let text = |v: &Option<String>| v.clone().unwrap_or_else(|| "—".into());
                let num =
                    |v: Option<f32>| v.map_or_else(|| "default".into(), |v| format!("{v:.2}"));
                row(
                    ui,
                    "Color",
                    m.color
                        .as_ref()
                        .map_or_else(|| "white".into(), |c| format!("{c:?}")),
                );
                row(ui, "Roughness", num(m.roughness));
                row(ui, "Metallic", num(m.metallic));
                row(
                    ui,
                    "Emission",
                    m.emission
                        .as_ref()
                        .map_or_else(|| "none".into(), |c| format!("{c:?}")),
                );
                for (name, value) in [
                    ("Albedo map", &m.albedo_texture),
                    ("Normal map", &m.normal_texture),
                    ("Metal/rough map", &m.metallic_roughness_texture),
                    ("Emission map", &m.emission_texture),
                ] {
                    row(ui, name, text(value));
                }
            }
        }
    }
    if let Some(error) = &open.error {
        ui.add_space(6.0);
        ui.label(RichText::new(format!("{}  {error}", icon::WARNING)).color(theme::WARN));
    }
}

fn image_view(ui: &mut egui::Ui, view: &mut ImageView, error: &mut Option<String>) {
    if let Some(task) = &mut view.loading {
        match task.try_take() {
            Some(Ok(data)) => {
                let name_hint = view.normal_map;
                let sample: Vec<u8> = data
                    .rgba
                    .pixels()
                    .step_by((data.rgba.len() / 4 / 4096).max(1))
                    .flat_map(|p| p.0)
                    .collect();
                view.normal_map =
                    name_hint || crate::thumbnails::looks_like_normal_map("", &sample);
                view.data = Some(data);
                view.loading = None;
            }
            Some(Err(err)) => {
                *error = Some(err);
                view.loading = None;
            }
            None => {
                ui.spinner();
                ui.ctx().request_repaint();
                return;
            }
        }
    }
    let Some(data) = &view.data else {
        return;
    };
    let light_key = (view.light * 100.0) as i32;
    if view.shown != Some((view.channel, light_key)) {
        let image = channel_image(&data.rgba, view.channel, view.light);
        view.texture = Some(ui.ctx().load_texture(
            "asset-viewer-image",
            image,
            egui::TextureOptions::NEAREST,
        ));
        view.shown = Some((view.channel, light_key));
    }
    let Some(texture) = &view.texture else {
        return;
    };
    let (rect, response) =
        ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    crate::tool_window::tile(&painter, rect, "checker", 24.0, || {
        let (dark, light) = (Color32::from_gray(40), Color32::from_gray(52));
        egui::ColorImage::new([2, 2], vec![light, dark, dark, light])
    });
    let size = egui::vec2(data.rgba.width() as f32, data.rgba.height() as f32);
    if view.fit {
        view.zoom = (rect.width() / size.x).min(rect.height() / size.y).min(8.0);
        view.pan = egui::Vec2::ZERO;
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            view.fit = false;
            view.zoom = (view.zoom * (1.0 + scroll / 300.0)).clamp(0.02, 64.0);
        }
    }
    if response.dragged() {
        view.fit = false;
        view.pan += response.drag_delta();
    }
    let shown = egui::Rect::from_center_size(rect.center() + view.pan, size * view.zoom);
    painter.image(
        texture.id(),
        shown,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    // The pixel under the pointer.
    if let Some(pos) = response.hover_pos().filter(|p| shown.contains(*p)) {
        let uv = (pos - shown.min) / shown.size();
        let (x, y) = (
            ((uv.x * size.x) as u32).min(data.rgba.width() - 1),
            ((uv.y * size.y) as u32).min(data.rgba.height() - 1),
        );
        let p = data.rgba.get_pixel(x, y).0;
        let text = format!("{x}, {y}   R {} G {} B {} A {}", p[0], p[1], p[2], p[3]);
        let galley = painter.layout_no_wrap(text, egui::FontId::monospace(11.5), Color32::WHITE);
        let at = rect.left_bottom() + egui::vec2(8.0, -galley.size().y - 8.0);
        painter.rect_filled(
            egui::Rect::from_min_size(
                at - egui::vec2(4.0, 3.0),
                galley.size() + egui::vec2(8.0, 6.0),
            ),
            4.0,
            theme::BG_0.gamma_multiply(0.85),
        );
        painter.galley(at, galley, Color32::WHITE);
    }
}

/// The image as `channel` shows it; at most 2048 px on a side (inspection, not export).
fn channel_image(rgba: &image::RgbaImage, channel: Channel, light: f32) -> egui::ColorImage {
    let rgba = if rgba.width().max(rgba.height()) > 2048 {
        image::DynamicImage::ImageRgba8(rgba.clone())
            .thumbnail(2048, 2048)
            .to_rgba8()
    } else {
        rgba.clone()
    };
    let (sx, sy) = (light.cos() * 0.7, light.sin() * 0.7);
    let light_dir = [sx, sy, 0.5_f32];
    let length = (light_dir[0].powi(2) + light_dir[1].powi(2) + light_dir[2].powi(2)).sqrt();
    let pixels: Vec<u8> = rgba
        .pixels()
        .flat_map(|p| {
            let [r, g, b, a] = p.0;
            match channel {
                Channel::All => [r, g, b, a],
                Channel::Red => [r, r, r, 255],
                Channel::Green => [g, g, g, 255],
                Channel::Blue => [b, b, b, 255],
                Channel::Alpha => [a, a, a, 255],
                Channel::Lit => {
                    let n = [r, g, b].map(|c| f32::from(c) / 127.5 - 1.0);
                    let lit =
                        (n[0] * light_dir[0] + n[1] * light_dir[1] + n[2] * light_dir[2]) / length;
                    let v = (lit.max(0.0) * 220.0 + 25.0) as u8;
                    [v, v, v, 255]
                }
            }
        })
        .collect();
    egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        &pixels,
    )
}

fn render_view(
    ui: &mut egui::Ui,
    view: &mut RenderView,
    previewer: &mut Option<Previewer>,
    project: &Path,
    render_state: Option<&egui_wgpu::RenderState>,
    error: &mut Option<String>,
) {
    let Some(state) = render_state else {
        crate::panels::empty_hint(ui, icon::CUBE, "No GPU: previews need one");
        return;
    };
    let previewer = previewer.get_or_insert_with(|| {
        let gpu = GpuContext::from_shared(
            state.adapter.clone(),
            state.device.clone(),
            state.queue.clone(),
        );
        Previewer::new(gpu, &PathBuf::from(project))
    });
    if let Err(err) = previewer.load(&view.subject, view.info.is_none()) {
        *error = Some(err);
        return;
    }
    view.info = Some(previewer.info.clone());

    let (rect, response) =
        ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
    if response.dragged_by(egui::PointerButton::Primary) {
        let d = response.drag_delta();
        view.orbit.yaw -= d.x * 0.01;
        view.orbit.pitch = (view.orbit.pitch - d.y * 0.01).clamp(-1.5, 1.5);
    }
    if response.dragged_by(egui::PointerButton::Secondary) {
        view.orbit.light_yaw -= response.drag_delta().x * 0.01;
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            view.orbit.zoom = (view.orbit.zoom * (1.0 - scroll / 400.0)).clamp(0.15, 6.0);
        }
    }
    let ppp = ui.ctx().pixels_per_point();
    let (width, height) = (
        ((rect.width() * ppp) as u32).clamp(16, 4096),
        ((rect.height() * ppp) as u32).clamp(16, 4096),
    );
    if view.target.as_ref().map(OffscreenTarget::size) != Some((width, height)) {
        let target = OffscreenTarget::new(previewer.gpu(), width, height);
        let mut renderer = state.renderer.write();
        match view.texture {
            Some(id) => renderer.update_egui_texture_from_wgpu_texture(
                &state.device,
                target.view(),
                wgpu::FilterMode::Linear,
                id,
            ),
            None => {
                view.texture = Some(renderer.register_native_texture(
                    &state.device,
                    target.view(),
                    wgpu::FilterMode::Linear,
                ));
            }
        }
        view.target = Some(target);
    }
    if let (Some(target), Some(texture)) = (&view.target, view.texture) {
        let now = (view.subject.clone(), view.orbit, (width, height));
        if view.drawn.as_ref() != Some(&now) {
            previewer.render(target, &view.orbit);
            view.drawn = Some(now);
        }
        ui.painter().image(
            texture,
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}

fn human_bytes(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / f64::from(1 << 20)),
        b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_and_lit_normal_maps() {
        let mut image = image::RgbaImage::new(2, 1);
        image.put_pixel(0, 0, image::Rgba([200, 100, 50, 10]));
        // A normal facing the viewer, then one tilted away from the light.
        image.put_pixel(1, 0, image::Rgba([128, 128, 255, 255]));
        let red = channel_image(&image, Channel::Red, 0.0);
        assert_eq!(red.pixels[0], Color32::from_rgb(200, 200, 200));
        let alpha = channel_image(&image, Channel::Alpha, 0.0);
        assert_eq!(alpha.pixels[0], Color32::from_rgb(10, 10, 10));
        let lit = channel_image(&image, Channel::Lit, 0.0);
        assert!(lit.pixels[1].r() > 60, "a facing normal is lit");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(3 << 20), "3.0 MB");
    }
}
