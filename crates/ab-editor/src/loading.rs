//! The loading screen: checks that this machine can run the editor (graphics, renderer,
//! audio, Rust toolchain, engine sources, settings folder) while showing progress, and
//! loads the project list.
//!
//! Graphics checks run on the UI thread (they need the GPU device); the others run on a
//! background thread. Results are revealed one by one, at a readable pace.

use crate::projects::{self, RecentProjects};
use crate::theme;
use eframe::egui::{self, Color32, FontId, RichText, Stroke, Vec2};
use eframe::egui_wgpu;
use egui_phosphor::regular as icon;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// Shortest time a step stays on screen, so the list is readable rather than a flash.
const STEP_TIME: Duration = Duration::from_millis(170);
/// Pause on 100 % before moving on when everything passed.
const DONE_PAUSE: Duration = Duration::from_millis(450);

/// What the engine is, in one line: the splash and the About box say the same thing.
pub const TAGLINE: &str = "Lightweight 3D game engine. Performance and memory safety first.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

/// Result of one check.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub level: Level,
    pub detail: String,
}

impl Outcome {
    fn ok(detail: impl Into<String>) -> Self {
        Self {
            level: Level::Ok,
            detail: detail.into(),
        }
    }
    fn warn(detail: impl Into<String>) -> Self {
        Self {
            level: Level::Warn,
            detail: detail.into(),
        }
    }
    fn fail(detail: impl Into<String>) -> Self {
        Self {
            level: Level::Fail,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Check {
    Graphics,
    Renderer,
    Audio,
    Toolchain,
    Engine,
    Settings,
    Projects,
}

impl Check {
    const ALL: [Check; 7] = [
        Check::Graphics,
        Check::Renderer,
        Check::Audio,
        Check::Toolchain,
        Check::Engine,
        Check::Settings,
        Check::Projects,
    ];

    fn title(self) -> &'static str {
        match self {
            Check::Graphics => "Graphics device",
            Check::Renderer => "Renderer & shaders",
            Check::Audio => "Audio output",
            Check::Toolchain => "Rust toolchain",
            Check::Engine => "Engine sources",
            Check::Settings => "Settings folder",
            Check::Projects => "Projects",
        }
    }

    fn running(self) -> &'static str {
        match self {
            Check::Graphics => "Checking the graphics device…",
            Check::Renderer => "Compiling the renderer's shaders…",
            Check::Audio => "Opening the audio output…",
            Check::Toolchain => "Looking for the Rust toolchain…",
            Check::Engine => "Finding the engine sources…",
            Check::Settings => "Checking the settings folder…",
            Check::Projects => "Loading projects…",
        }
    }
}

/// Everything the checks found, kept for the project manager's "System" panel.
#[derive(Debug, Clone)]
pub struct Report {
    pub results: Vec<(&'static str, Outcome)>,
}

impl Report {
    pub fn worst(&self) -> Level {
        self.results
            .iter()
            .map(|(_, o)| o.level)
            .max()
            .unwrap_or(Level::Ok)
    }

    pub fn count(&self, level: Level) -> usize {
        self.results
            .iter()
            .filter(|(_, o)| o.level == level)
            .count()
    }
}

/// What the loading screen hands on when it is done.
#[derive(Debug)]
pub struct Loaded {
    pub report: Report,
    pub recent: RecentProjects,
}

pub enum Next {
    Stay,
    Continue(Loaded),
    Quit,
}

pub struct Loading {
    started: Instant,
    results: Vec<Option<Outcome>>,
    /// Steps shown as finished.
    shown: usize,
    last_reveal: Instant,
    receiver: Receiver<(usize, Outcome)>,
    recent: Receiver<RecentProjects>,
    loaded_recent: Option<RecentProjects>,
    frames: u32,
    finished_at: Option<Instant>,
    /// Keep showing the screen (screenshots of it).
    hold: bool,
}

impl Loading {
    pub fn new(hold: bool) -> Self {
        let (send, receiver) = mpsc::channel();
        let (send_recent, recent) = mpsc::channel();
        std::thread::Builder::new()
            .name("system-check".into())
            .spawn(move || {
                let index = |check| Check::ALL.iter().position(|c| *c == check).unwrap_or(0);
                let _ = send.send((index(Check::Audio), check_audio()));
                let _ = send.send((index(Check::Toolchain), check_toolchain()));
                let _ = send.send((index(Check::Engine), check_engine()));
                let _ = send.send((index(Check::Settings), check_settings()));
                let (outcome, recent) = load_projects();
                let _ = send_recent.send(recent);
                let _ = send.send((index(Check::Projects), outcome));
            })
            .expect("spawning a thread");
        Self {
            started: Instant::now(),
            results: vec![None; Check::ALL.len()],
            shown: 0,
            last_reveal: Instant::now(),
            receiver,
            recent,
            loaded_recent: None,
            frames: 0,
            finished_at: None,
            hold,
        }
    }

    fn report(&self) -> Report {
        Report {
            results: Check::ALL
                .iter()
                .zip(&self.results)
                .filter_map(|(c, o)| o.clone().map(|o| (c.title(), o)))
                .collect(),
        }
    }

    pub fn update(
        &mut self,
        ui: &mut egui::Ui,
        render_state: Option<&egui_wgpu::RenderState>,
    ) -> Next {
        let ctx = &ui.ctx().clone();
        self.frames += 1;
        // The GPU checks block for a moment: draw the screen once first.
        if self.frames == 3 {
            self.results[0] = Some(check_graphics(render_state));
            self.results[1] = Some(check_renderer(render_state));
        }
        while let Ok((index, outcome)) = self.receiver.try_recv() {
            self.results[index] = Some(outcome);
        }
        if let Ok(recent) = self.recent.try_recv() {
            self.loaded_recent = Some(recent);
        }
        if self.shown < self.results.len()
            && self.results[self.shown].is_some()
            && self.last_reveal.elapsed() >= STEP_TIME
        {
            self.shown += 1;
            self.last_reveal = Instant::now();
        }
        let done = self.shown == self.results.len();
        if done && self.finished_at.is_none() {
            self.finished_at = Some(Instant::now());
        }
        let report = self.report();
        let clean = report.worst() == Level::Ok;

        let mut next = Next::Stay;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::BG_0))
            .show(ui, |ui| {
                // The whole splash moves the window.
                let drag = ui.interact(ui.max_rect(), ui.id().with("drag"), egui::Sense::drag());
                if drag.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if let Some(choice) = self.draw(ui, &report, done) {
                    next = choice;
                }
            });
        ctx.request_repaint();

        let paused = self.finished_at.is_some_and(|t| t.elapsed() >= DONE_PAUSE);
        if matches!(next, Next::Stay) && clean && paused && !self.hold {
            next = Next::Continue(Loaded {
                report,
                recent: self.loaded_recent.take().unwrap_or_default(),
            });
        }
        if let Next::Continue(loaded) = next {
            tracing::info!(
                elapsed_ms = self.started.elapsed().as_millis() as u64,
                warnings = loaded.report.count(Level::Warn),
                failures = loaded.report.count(Level::Fail),
                "system check finished"
            );
            return Next::Continue(loaded);
        }
        next
    }

    fn draw(&mut self, ui: &mut egui::Ui, report: &Report, done: bool) -> Option<Next> {
        // The splash is dragged to move the window: without this, the drag selects the text.
        let mut style = (**ui.style()).clone();
        style.interaction.selectable_labels = false;
        ui.set_style(style);

        let full = ui.max_rect();
        let painter = ui.painter().clone();
        glow(&painter, full.left_top() + Vec2::new(70.0, 50.0), 260.0);
        painter.rect_stroke(
            full.shrink(0.5),
            0.0,
            Stroke::new(1.0_f32, theme::BORDER),
            egui::StrokeKind::Inside,
        );

        let margin = 28.0;
        let content = full.shrink(margin);
        let mut choice = None;

        // Brand.
        let brand = egui::Rect::from_min_max(
            content.min,
            egui::pos2(content.right(), content.bottom() - 60.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(brand), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(icon::LIGHTNING)
                        .size(30.0)
                        .color(theme::ACCENT),
                );
                ui.label(
                    RichText::new("April Blink")
                        .size(26.0)
                        .strong()
                        .color(Color32::WHITE),
                );
            });
            ui.label(
                RichText::new(format!("Editor  ·  v{}", env!("CARGO_PKG_VERSION")))
                    .size(13.0)
                    .color(theme::TEXT_DIM),
            );
            ui.add_space(10.0);
            ui.label(
                RichText::new(TAGLINE)
                    .size(13.0)
                    .color(Color32::from_rgb(178, 184, 196)),
            );
        });

        // Progress, bottom.
        let bar_top = content.bottom() - 34.0;
        let current = Check::ALL.get(self.shown).copied();
        let worst = report.worst();
        let status = match (done, worst) {
            (false, _) => current.map_or("", |c| c.running()).to_owned(),
            (true, Level::Ok) => "Ready".to_owned(),
            (true, level) => {
                let (title, outcome) = report
                    .results
                    .iter()
                    .find(|(_, o)| o.level == level)
                    .expect("the worst level comes from a result");
                let more = report.count(Level::Warn) + report.count(Level::Fail) - 1;
                let more = if more > 0 {
                    format!(" (+{more} more)")
                } else {
                    String::new()
                };
                format!("{title}: {}{more}", outcome.detail)
            }
        };
        let fraction = self.shown as f32 / Check::ALL.len() as f32;
        let shown = ui
            .ctx()
            .animate_value_with_time(ui.id().with("progress"), fraction, 0.25);
        // One line, cut with "…" so it never runs into the percentage.
        let mut job = egui::text::LayoutJob::simple_singleline(
            status,
            FontId::proportional(13.0),
            match (done, worst) {
                (true, Level::Warn) => theme::WARN,
                (true, Level::Fail) => theme::ERROR,
                _ => Color32::from_rgb(214, 218, 226),
            },
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(content.width() - 52.0);
        let galley = ui.painter().layout_job(job);
        painter.galley(
            egui::pos2(content.left(), bar_top - 10.0 - galley.size().y),
            galley,
            Color32::WHITE,
        );
        painter.text(
            egui::pos2(content.right(), bar_top - 10.0),
            egui::Align2::RIGHT_BOTTOM,
            format!("{:.0}%", shown * 100.0),
            FontId::proportional(12.0),
            theme::TEXT_DIM,
        );
        let track = egui::Rect::from_min_size(
            egui::pos2(content.left(), bar_top),
            Vec2::new(content.width(), 6.0),
        );
        painter.rect_filled(track, 3.0, theme::BG_3);
        let fill_color = match (done, worst) {
            (true, Level::Warn) => theme::WARN,
            (true, Level::Fail) => theme::ERROR,
            _ => theme::ACCENT,
        };
        let mut fill = track;
        fill.set_width(track.width() * shown.clamp(0.0, 1.0));
        if fill.width() > 0.5 {
            painter.rect_filled(fill, 3.0, fill_color);
        }

        // Buttons when something needs the user's attention.
        if done && worst != Level::Ok {
            let row = egui::Rect::from_min_max(
                egui::pos2(content.left(), bar_top + 12.0),
                egui::pos2(content.right(), full.bottom() - 6.0),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if worst == Level::Fail {
                        "Continue anyway"
                    } else {
                        "Continue"
                    };
                    let primary = egui::Button::new(
                        RichText::new(format!("{label}  {}", icon::ARROW_RIGHT))
                            .color(Color32::WHITE),
                    )
                    .fill(theme::ACCENT_DIM)
                    .min_size(Vec2::new(120.0, 26.0));
                    if ui.add(primary).clicked() {
                        choice = Some(Next::Continue(Loaded {
                            report: report.clone(),
                            recent: self.loaded_recent.take().unwrap_or_default(),
                        }));
                    }
                    if ui
                        .add(egui::Button::new("Quit").min_size(Vec2::new(80.0, 26.0)))
                        .clicked()
                    {
                        choice = Some(Next::Quit);
                    }
                });
            });
        }
        choice
    }
}

/// A soft radial accent glow: bright in the middle, fading to nothing at `radius`.
fn glow(painter: &egui::Painter, center: egui::Pos2, radius: f32) {
    let mut mesh = egui::Mesh::default();
    let inner = Color32::from_rgba_premultiplied(18, 40, 76, 60);
    mesh.colored_vertex(center, inner);
    let segments = 64;
    for i in 0..=segments {
        let angle = i as f32 / segments as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(center + radius * Vec2::angled(angle), Color32::TRANSPARENT);
    }
    for i in 1..=segments {
        mesh.add_triangle(0, i, i + 1);
    }
    painter.add(mesh);
}

// =============================================================================================
// Checks
// =============================================================================================

fn check_graphics(state: Option<&egui_wgpu::RenderState>) -> Outcome {
    let Some(state) = state else {
        return Outcome::fail("No GPU device: the 3D viewport needs wgpu");
    };
    let info = state.adapter.get_info();
    // Picked automatically: the best GPU, on the API that suits this OS (see the log for
    // the others found).
    let summary =
        ab_engine::rendering::AdapterCandidate::of(&state.adapter, None, None).to_string();
    let limits = state.adapter.limits();
    if limits.max_texture_dimension_2d < 4096 {
        return Outcome::warn(format!(
            "{summary}: textures limited to {} px",
            limits.max_texture_dimension_2d
        ));
    }
    match info.device_type {
        wgpu::DeviceType::Cpu => Outcome::warn(format!("{summary}: software rendering is slow")),
        _ => Outcome::ok(summary),
    }
}

/// Builds the engine's renderer on the editor's device: compiles every shader and
/// pipeline the viewport uses.
fn check_renderer(state: Option<&egui_wgpu::RenderState>) -> Outcome {
    let Some(state) = state else {
        return Outcome::fail("Skipped: no GPU device");
    };
    let started = Instant::now();
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(crate::viewport::Viewport::new(Some(state)));
    }));
    match built {
        Ok(()) => Outcome::ok(format!(
            "Pipelines ready in {} ms",
            started.elapsed().as_millis()
        )),
        Err(_) => Outcome::fail("The engine's shaders do not compile on this GPU"),
    }
}

fn check_audio() -> Outcome {
    let audio = ab_engine::audio::Audio::new(ab_engine::audio::AudioOutput::Device);
    if audio.is_silent() {
        Outcome::warn("No sound device: play mode runs silent")
    } else {
        Outcome::ok("Default output device")
    }
}

/// The toolchain game scripts are built with: `cargo`, and a `rustc` at least as new as the
/// engine's `rust-version` (the script SDK inherits it).
fn check_toolchain() -> Outcome {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let Some(cargo_version) = program_version(&cargo) else {
        return Outcome::warn(
            "cargo not found: install Rust (https://rustup.rs) to build game scripts",
        );
    };
    let rustc = program_version(std::ffi::OsStr::new("rustc")).unwrap_or(cargo_version);
    let found = parse_version(&rustc);
    let needed = parse_version(env!("CARGO_PKG_RUST_VERSION"));
    match (found, needed) {
        (Some(found), Some(needed)) if found < needed => Outcome::warn(format!(
            "Rust {found} is too old: game scripts need {needed} or newer ({})",
            update_hint()
        )),
        (Some(found), _) => Outcome::ok(format!("Rust {found}")),
        (None, _) => Outcome::ok(rustc),
    }
}

/// `program --version`, when the program runs.
fn program_version(program: &std::ffi::OsStr) -> Option<String> {
    ab_project::process::command(program)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

/// How to update Rust on this machine: rustup's way, or the system's.
fn update_hint() -> &'static str {
    if program_version(std::ffi::OsStr::new("rustup")).is_some() {
        "run `rustup update`"
    } else {
        "update the Rust installation of your system"
    }
}

/// A version number: `major.minor.patch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// `rustc 1.90.0 (…)` or `1.85` → `1.90.0` or `1.85.0`.
fn parse_version(text: &str) -> Option<Version> {
    let word = text
        .split_whitespace()
        .find(|w| w.starts_with(|c: char| c.is_ascii_digit()))?;
    let mut parts = word.split(['.', '-']);
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = match parts.next() {
        Some(patch) => patch.parse().ok()?,
        None => 0,
    };
    Some(Version {
        major,
        minor,
        patch,
    })
}

/// The script SDK new projects build against: the engine checkout's (development builds),
/// else the copy shipped next to the editor. (The projects folder needs no check: it is
/// created with the first project.)
fn check_engine() -> Outcome {
    engine_outcome(projects::engine_root(), ab_project::sdk_dir())
}

fn engine_outcome(engine: Option<PathBuf>, sdk: Option<PathBuf>) -> Outcome {
    match (engine, sdk) {
        (Some(engine), Some(_)) => Outcome::ok(engine.display().to_string()),
        (None, Some(sdk)) => Outcome::ok(format!("SDK shipped with the editor: {}", sdk.display())),
        (_, None) => Outcome::warn(
            "Not found: new projects use the published ab-script crate (downloaded when \
             their scripts first build). Set AB_ENGINE_ROOT to an engine checkout to use it.",
        ),
    }
}

/// The settings folder exists and is writable.
fn check_settings() -> Outcome {
    let store = RecentProjects::store();
    let dir = store.dir().to_path_buf();
    let probe = dir.join(".write-test");
    let writable = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&probe, b"ok"))
        .and_then(|()| std::fs::remove_file(&probe));
    match writable {
        Ok(()) => Outcome::ok(dir.display().to_string()),
        Err(err) => Outcome::warn(format!(
            "{}: {err} (recent projects are not kept)",
            dir.display()
        )),
    }
}

fn load_projects() -> (Outcome, RecentProjects) {
    match RecentProjects::load(&RecentProjects::store()) {
        Ok(recent) => {
            let count = recent.projects.len();
            let missing = recent.projects.iter().filter(|p| !p.path.is_dir()).count();
            let mut detail = match count {
                0 => "No projects yet".to_owned(),
                1 => "1 project".to_owned(),
                n => format!("{n} projects"),
            };
            if missing > 0 {
                detail.push_str(&format!(", {missing} missing"));
            }
            (Outcome::ok(detail), recent)
        }
        Err(err) => (
            Outcome::warn(format!(
                "The project list is unreadable ({err}); starting empty"
            )),
            RecentProjects::default(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_check_looks_for_the_sdk_not_the_projects_folder() {
        // This checkout: found, whether or not ~/April Blink Projects exists yet.
        let here = check_engine();
        assert_eq!(here.level, Level::Ok, "{}", here.detail);
        let engine = PathBuf::from("/engine");
        let sdk = PathBuf::from("/editor/sdk/ab-script");
        assert_eq!(
            engine_outcome(Some(engine.clone()), Some(sdk.clone())).detail,
            engine.display().to_string()
        );
        let shipped = engine_outcome(None, Some(sdk));
        assert_eq!(shipped.level, Level::Ok);
        assert!(shipped.detail.starts_with("SDK shipped with the editor"));
        assert_eq!(engine_outcome(Some(engine), None).level, Level::Warn);
    }

    #[test]
    fn versions() {
        let version = |major, minor, patch| Version {
            major,
            minor,
            patch,
        };
        assert_eq!(
            parse_version("rustc 1.90.0 (1159e78c4 2025-09-14)"),
            Some(version(1, 90, 0))
        );
        assert_eq!(parse_version("1.85"), Some(version(1, 85, 0)));
        assert_eq!(
            parse_version("cargo 1.86.0-nightly"),
            Some(version(1, 86, 0))
        );
        assert_eq!(
            parse_version("rustc 1.99.1 (58cf8f9c3 2026-10-13)"),
            Some(version(1, 99, 1))
        );
        assert_eq!(parse_version("nothing"), None);
        // The patch decides: 1.99.0 is older than 1.99.1.
        assert!(version(1, 99, 0) < version(1, 99, 1));
    }

    #[test]
    fn worst_level() {
        let report = Report {
            results: vec![
                ("a", Outcome::ok("")),
                ("b", Outcome::warn("")),
                ("c", Outcome::ok("")),
            ],
        };
        assert_eq!(report.worst(), Level::Warn);
        assert_eq!(report.count(Level::Ok), 2);
    }

    /// The tagline must fit on one line on the splash and in the About box.
    #[test]
    fn tagline_fits_on_one_line() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        ctx.begin_pass(egui::RawInput::default());
        let width = ctx.fonts_mut(|f| {
            let job = egui::text::LayoutJob::simple_singleline(
                TAGLINE.to_owned(),
                egui::FontId::proportional(13.0),
                egui::Color32::WHITE,
            );
            f.layout_job(job).size().x
        });
        // The splash leaves 28 px of margin on each side; the About box is 440 px wide and
        // its frame eats 4 px on each side.
        assert!(width <= 464.0, "the tagline is {width:.1} px wide");
        assert!(width <= 432.0, "the tagline is {width:.1} px wide");
    }
}
