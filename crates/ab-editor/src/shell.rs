//! The editor window's screens, in order: the loading screen (system check), the project
//! manager, then the editor on the chosen project. `--project` skips the project manager.

use crate::app::{EditorApp, ScriptWatch};
use crate::console::LogBuffer;
use crate::loading::{Loading, Next, Report};
use crate::project_manager::{Action, ProjectManager};
use crate::theme;
use crate::Options;
use eframe::egui::{self, Vec2, ViewportCommand};
use eframe::egui_wgpu;
use std::path::PathBuf;

/// Size of the loading screen (the window starts at it).
pub const LOADING_SIZE: [f32; 2] = [520.0, 300.0];
const MANAGER_SIZE: Vec2 = Vec2::new(1120.0, 720.0);
const MANAGER_MIN: Vec2 = Vec2::new(860.0, 560.0);
const EDITOR_SIZE: Vec2 = Vec2::new(1600.0, 960.0);
const EDITOR_MIN: Vec2 = Vec2::new(960.0, 600.0);
/// Frames a screen is shown before `--screenshot` grabs it.
const SCREENSHOT_FRAMES: u32 = 30;

enum Screen {
    Loading(Loading),
    Projects(Box<ProjectManager>),
    Editor(Box<EditorApp>),
}

pub struct Shell {
    options: Options,
    logs: LogBuffer,
    render_state: Option<egui_wgpu::RenderState>,
    screen: Screen,
    report: Option<Report>,
    /// `--screenshot`: frames shown on the current screen, and whether the grab was asked.
    screenshot: Option<(PathBuf, u32, bool)>,
}

impl Shell {
    pub fn new(cc: &eframe::CreationContext<'_>, options: Options, logs: LogBuffer) -> Self {
        theme::apply(&cc.egui_ctx);
        // The user's language, font and scale from the first screen on (screenshots use
        // the defaults, so they look the same on every machine).
        let mut settings = match options.screenshot {
            None => {
                crate::settings::EditorSettings::load(&crate::settings::EditorSettings::store())
            }
            Some(_) => Default::default(),
        };
        if let Some(language) = &options.language {
            settings.language = language.clone();
        }
        if let Some(err) = crate::preferences::apply(&cc.egui_ctx, &settings) {
            tracing::warn!("{err}");
        }
        cc.egui_ctx
            .send_viewport_cmd(ViewportCommand::Title("April Blink Editor".into()));
        Self {
            screen: Screen::Loading(Loading::new(options.hold_loading)),
            screenshot: options.screenshot.clone().map(|path| (path, 0, false)),
            options,
            logs,
            render_state: cc.wgpu_render_state.clone(),
            report: None,
        }
    }

    fn show_projects(&mut self, ctx: &egui::Context, manager: ProjectManager) {
        ctx.send_viewport_cmd(ViewportCommand::Title(
            "Project Manager — April Blink Editor".into(),
        ));
        ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
        resize(ctx, MANAGER_SIZE, MANAGER_MIN);
        self.set_screen(Screen::Projects(Box::new(manager)));
    }

    fn show_editor(
        &mut self,
        ctx: &egui::Context,
        assets: PathBuf,
        options: &Options,
        watch: Option<ScriptWatch>,
    ) {
        let maximize = watch.is_some();
        let editor = EditorApp::new(
            self.render_state.as_ref(),
            assets,
            options,
            self.logs.clone(),
            watch,
        );
        resize(ctx, EDITOR_SIZE, EDITOR_MIN);
        if maximize {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
        }
        self.set_screen(Screen::Editor(Box::new(editor)));
    }

    fn set_screen(&mut self, screen: Screen) {
        self.screen = screen;
        if let Some((_, frames, _)) = &mut self.screenshot {
            *frames = 0;
        }
    }

    /// `--screenshot`: grabs the screen the start-up settles on, saves it and quits.
    fn screenshot(&mut self, ctx: &egui::Context) {
        let settled = match &self.screen {
            Screen::Loading(_) => self.options.hold_loading,
            Screen::Projects(_) | Screen::Editor(_) => true,
        };
        let Some((path, frames, requested)) = &mut self.screenshot else {
            return;
        };
        if !settled {
            return;
        }
        if !*requested {
            *frames += 1;
            if *frames >= SCREENSHOT_FRAMES {
                *requested = true;
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
            }
            return;
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(image) = image else {
            return;
        };
        let [width, height] = image.size;
        let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
        let saved = image::RgbaImage::from_raw(width as u32, height as u32, bytes)
            .map(|img| img.save(&*path));
        match saved {
            Some(Ok(())) => tracing::info!(path = %path.display(), "screenshot saved"),
            Some(Err(err)) => tracing::error!("cannot save the screenshot: {err}"),
            None => tracing::error!("screenshot has an unexpected size"),
        }
        self.screenshot = None;
        if let Screen::Editor(editor) = &mut self.screen {
            editor.discard_changes();
        }
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
}

/// Resizes the window and, where the platform allows it, keeps it centered.
fn resize(ctx: &egui::Context, size: Vec2, min: Vec2) {
    ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(min));
    let monitor = ctx.input(|i| i.viewport().monitor_size);
    let size = match monitor {
        // Leave room for panels and docks on small screens.
        Some(monitor) => size.min(monitor * 0.92).max(min),
        None => size,
    };
    ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
    if let Some(monitor) = monitor {
        let corner = ((monitor - size) / 2.0).max(Vec2::ZERO);
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(corner.to_pos2()));
    }
}

impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        // `--screenshot-window`: tool windows drawn inside the main one, so its grab has them.
        if self.screenshot.is_some() && self.options.screenshot_window.is_some() {
            ctx.set_embed_viewports(true);
        }
        self.screenshot(ctx);
        match &mut self.screen {
            Screen::Loading(loading) => match loading.update(ui, self.render_state.as_ref()) {
                Next::Stay => {}
                Next::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
                Next::Continue(loaded) => {
                    self.report = Some(loaded.report.clone());
                    let project_folder = self
                        .options
                        .project
                        .as_deref()
                        .filter(|_| self.options.scripts.is_none())
                        .and_then(|path| crate::projects::Project::open(path).ok());
                    match self.options.project.clone() {
                        // A project: built and opened as from the project manager.
                        Some(_) if project_folder.is_some() => {
                            let mut manager = ProjectManager::new(loaded.recent, loaded.report);
                            manager.open_project(&project_folder.expect("checked").root);
                            self.show_projects(ctx, manager);
                        }
                        // An asset folder, with `--scripts` for its library.
                        Some(project) => {
                            let options = self.options.clone();
                            self.show_editor(ctx, project, &options, None);
                        }
                        None => {
                            let manager = ProjectManager::new(loaded.recent, loaded.report);
                            self.show_projects(ctx, manager);
                        }
                    }
                }
            },
            Screen::Projects(manager) => {
                if let Action::Open { project, scripts } = manager.update(ui) {
                    // `--scene` names the scene the first project opens with; after that (and
                    // without it) a project opens on its main scene.
                    let scene = self
                        .options
                        .scene
                        .take()
                        .or_else(|| project.main_scene.clone());
                    let options = Options {
                        scene,
                        scripts,
                        ..self.options.clone()
                    };
                    let assets = project.assets.clone();
                    let watch = project
                        .package
                        .is_some()
                        .then(|| ScriptWatch::new(*project));
                    self.show_editor(ctx, assets, &options, watch);
                }
            }
            Screen::Editor(editor) => {
                editor.update(ui, frame);
                if editor.wants_close_project() {
                    // The project's log and caches are its own: not the next one's.
                    crate::console::log_to(None);
                    ab_engine::asset::set_decode_cache(None);
                    let report = self.report.clone().unwrap_or(Report {
                        results: Vec::new(),
                    });
                    self.show_projects(ctx, ProjectManager::reopen(report));
                }
            }
        }
    }
}
