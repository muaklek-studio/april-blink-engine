//! File → Export Game: builds shippable copies of the open game for Windows and Linux.
//!
//! Above the platforms, the game's details: title, creator, description, copyright,
//! version, icon, the executable's name, a console on Windows, where exports go
//! (`[export]` in `game.toml`, see [`ab_project::export_settings`]). They are checked as
//! they are typed (errors stop the export) and saved to `game.toml` before it starts; on
//! Windows they go into the executable.
//!
//! The window runs the exporter ([`ab_project::export`]) in a background process of the
//! editor itself (`ab-editor --export …`, so Cancel can stop it and the builds it started),
//! shows its progress, and lists the folders and archives it made. Editing goes on while it
//! builds. Projects export with the player; no engine sources are needed.

use crate::projects::{engine_root, Project};
use crate::theme;
use ab_engine::core::jobs::{self, Task};
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

/// Lines of output kept for the log view.
const MAX_LOG_LINES: usize = 4000;

/// The game the editor has open, as the exporter names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportTarget {
    /// What `--export` takes: `slice`, or the project folder.
    pub spec: String,
    /// Shown next to the title (the executable's name).
    pub name: String,
    pub title: String,
    /// Where exports go by default.
    pub out: PathBuf,
}

/// Which game `assets` (the editor's project folder) belongs to, if it can be exported:
/// a project with scripts, or Night Relics from the engine's sources.
pub fn export_target(assets: &Path) -> Option<ExportTarget> {
    let canonical = |p: &Path| ab_project::path::canonicalize_lossy(p);
    let assets = canonical(assets);
    if let Some(engine) = engine_root().filter(|e| canonical(&e.join("assets")) == assets) {
        let game = ab_project::export::Game::slice(&engine);
        return Some(ExportTarget {
            spec: "slice".into(),
            name: game.name(),
            title: game.title().to_owned(),
            out: engine.join("dist"),
        });
    }
    let project = Project::open(&assets).ok()?;
    Some(ExportTarget {
        spec: project.root.to_string_lossy().into_owned(),
        name: project.package.clone()?,
        title: project.title.clone(),
        out: project.root.join("dist"),
    })
}

/// What the player picked in the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOptions {
    pub linux: bool,
    pub windows: bool,
    /// An app bundle; only made on a Mac.
    pub macos: bool,
    /// Also a `.zip` (Windows) / `.tar.gz` (Linux, macOS).
    pub archive: bool,
    /// Assets inside the executable instead of chunks in `assets/` next to it.
    pub single_file: bool,
    /// README.txt and licenses/ as files next to the executable.
    pub extras: bool,
    pub out: String,
}

impl ExportOptions {
    /// How many platforms are ticked.
    fn platforms(&self) -> usize {
        [self.linux, self.windows, self.macos]
            .into_iter()
            .filter(|&on| on)
            .count()
    }

    fn new(out: &Path) -> Self {
        Self {
            // Both where possible: most players are on Windows. Linux builds need Linux
            // (a Mac has no Linux linker by default).
            linux: cfg!(target_os = "linux"),
            windows: true,
            macos: cfg!(target_os = "macos"),
            archive: true,
            single_file: false,
            extras: false,
            out: out.to_string_lossy().into_owned(),
        }
    }

    /// These options with the output folder made absolute: the export runs in a process
    /// whose working folder is the editor's (often `/` when started from a file manager),
    /// so an empty or relative folder means the game's `dist` or a folder inside the game.
    pub fn for_target(&self, target: &ExportTarget) -> Self {
        let out = self.out.trim();
        let out = if out.is_empty() {
            target.out.clone()
        } else if Path::new(out).is_relative() {
            target.out.parent().unwrap_or(&target.out).join(out)
        } else {
            PathBuf::from(out)
        };
        Self {
            out: out.to_string_lossy().into_owned(),
            ..self.clone()
        }
    }

    /// Arguments of `ab-editor` for this export of `game`.
    pub fn args(&self, game: &str) -> Vec<String> {
        let platforms: Vec<&str> = [
            (self.linux, "linux"),
            (self.windows, "windows"),
            (self.macos, "macos"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        let mut args = vec![
            "--export".to_owned(),
            game.to_owned(),
            "--platform".to_owned(),
            platforms.join(","),
        ];
        if self.archive {
            args.push("--archive".into());
        }
        if self.single_file {
            args.push("--single-file".into());
        }
        if self.extras {
            args.push("--extras".into());
        }
        if !self.out.trim().is_empty() {
            args.push("--out".into());
            args.push(self.out.trim().to_owned());
        }
        args
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Running,
    Succeeded,
    Failed(String),
    Cancelled,
}

/// What the export produced, read from its output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outputs {
    pub folders: Vec<PathBuf>,
    pub archives: Vec<PathBuf>,
}

/// Progress read from the export's output lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// "linux", "windows".
    pub platform: Option<String>,
    /// What it is doing now.
    pub step: String,
    /// Crates compiled for the current platform.
    pub compiled: usize,
    /// Platforms finished.
    pub done: usize,
    pub outputs: Outputs,
}

/// Removes terminal color codes (the game's own log, printed during the check, has them).
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Progress {
    /// Updates from one output line.
    pub fn read(&mut self, line: &str) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("=== ") {
            // "=== My Game 0.1.0 for windows ==="
            let platform = rest
                .trim_end_matches(" ===")
                .rsplit(" for ")
                .next()
                .unwrap_or_default()
                .to_owned();
            if self.platform.is_some() {
                self.done += 1;
            }
            self.step = format!("Building for {platform}");
            self.platform = Some(platform);
            self.compiled = 0;
        } else if let Some(rest) = line.strip_prefix("Compiling ") {
            self.compiled += 1;
            let krate = rest.split_whitespace().next().unwrap_or(rest);
            self.step = format!("Compiling {krate}");
        } else if line.starts_with("Blocking waiting for file lock") {
            self.step = "Waiting for another build to finish".into();
        } else if line.starts_with("packed ") {
            self.step = "Packing assets".into();
        } else if line.starts_with("verify:") {
            self.step = "Checking the build".into();
        } else if let Some(path) = line.strip_prefix("archive: ") {
            self.step = "Making the archive".into();
            self.outputs.archives.push(PathBuf::from(path));
        } else if let Some(dir) = line.strip_prefix("folder: ") {
            self.outputs.folders.push(PathBuf::from(dir));
        }
    }
}

/// A running (or finished) export.
#[derive(Debug)]
pub struct ExportJob {
    child: Child,
    lines: Receiver<String>,
    pub log: Vec<String>,
    pub progress: Progress,
    pub state: JobState,
    pub started: Instant,
    pub finished: Option<Instant>,
    pub platforms: usize,
}

impl ExportJob {
    /// Starts exporting `target` in a background process of this editor.
    pub fn start(target: &ExportTarget, options: &ExportOptions) -> anyhow::Result<Self> {
        let editor = std::env::current_exe()?;
        let mut command = ab_project::process::command(editor);
        command.args(options.for_target(target).args(&target.spec));
        let mut job = Self::spawn(command)?;
        job.platforms = options.platforms();
        Ok(job)
    }

    fn spawn(mut command: Command) -> anyhow::Result<Self> {
        let shown = format!("$ {command:?}").replace('"', "");
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Its own process group, so Cancel stops the builds it started too.
        ab_project::process::own_group(&mut command);
        let mut child = command
            .spawn()
            .map_err(|e| anyhow::anyhow!("cannot start the export: {e}"))?;
        let (send, lines) = mpsc::channel();
        let pipe = |stream: Option<Box<dyn Read + Send>>| {
            if let Some(stream) = stream {
                let send = send.clone();
                std::thread::spawn(move || {
                    for line in BufReader::new(stream).lines().map_while(Result::ok) {
                        if send.send(strip_ansi(&line)).is_err() {
                            break;
                        }
                    }
                });
            }
        };
        pipe(
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
        );
        pipe(
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
        );
        Ok(Self {
            child,
            lines,
            log: vec![shown],
            progress: Progress {
                step: "Starting".into(),
                ..Default::default()
            },
            state: JobState::Running,
            started: Instant::now(),
            finished: None,
            platforms: 1,
        })
    }

    /// Reads new output; true while it runs.
    pub fn poll(&mut self) -> bool {
        if self.state != JobState::Running {
            return false;
        }
        let take = |job: &mut Self, line: String| {
            job.progress.read(&line);
            if job.log.len() < MAX_LOG_LINES {
                job.log.push(line);
            }
        };
        while let Ok(line) = self.lines.try_recv() {
            take(self, line);
        }
        if let Ok(Some(status)) = self.child.try_wait() {
            // Output written just before exiting.
            while let Ok(line) = self
                .lines
                .recv_timeout(std::time::Duration::from_millis(50))
            {
                take(self, line);
            }
            self.finished = Some(Instant::now());
            self.state = if status.success() {
                self.progress.done = self.platforms;
                JobState::Succeeded
            } else {
                let error = self
                    .log
                    .iter()
                    .rev()
                    .find(|l| l.starts_with("Error:") || l.starts_with("error"))
                    .cloned()
                    .unwrap_or_else(|| format!("the export failed ({status})"));
                JobState::Failed(error.trim_start_matches("Error: ").to_owned())
            };
        }
        self.state == JobState::Running
    }

    pub fn cancel(&mut self) {
        if self.state != JobState::Running {
            return;
        }
        ab_project::process::kill_tree(&mut self.child);
        self.state = JobState::Cancelled;
        self.finished = Some(Instant::now());
    }

    pub fn elapsed(&self) -> std::time::Duration {
        self.finished.unwrap_or_else(Instant::now) - self.started
    }
}

impl Drop for ExportJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Toolchain checks for a Windows build from Linux, made once when the window opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchain {
    pub windows_target: bool,
    pub xwin: bool,
}

fn check_toolchain() -> Toolchain {
    let ok = |program: &str, args: &[&str]| {
        ab_project::process::command(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
    };
    let windows_target = cfg!(windows)
        || ok("rustup", &["target", "list", "--installed"]).is_some_and(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|l| l.trim() == "x86_64-pc-windows-msvc")
        });
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let xwin = cfg!(windows) || ok(&cargo, &["xwin", "--version"]).is_some();
    Toolchain {
        windows_target,
        xwin,
    }
}

/// Opens a folder (or the folder holding a file) in the system's file manager.
fn reveal(path: &Path) {
    let folder = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match Command::new(program).arg(folder).spawn() {
        Ok(child) => crate::scripts::reap(child),
        Err(err) => tracing::warn!("cannot open {}: {err}", folder.display()),
    }
}

/// The Export Game window and the job it runs.
#[derive(Debug, Default)]
pub struct ExportWindow {
    pub open: bool,
    options: Option<ExportOptions>,
    toolchain: Option<Toolchain>,
    check: Option<Task<Toolchain>>,
    pub job: Option<ExportJob>,
    show_log: bool,
    start_error: Option<String>,
    details: Option<GameDetails>,
}

/// The game's details as edited in the window.
struct GameDetails {
    root: PathBuf,
    title: String,
    settings: ab_project::export_settings::ExportSettings,
    /// As in `game.toml`.
    saved: (String, ab_project::export_settings::ExportSettings),
    /// Shown in the field's hint.
    cargo_version: String,
    package: String,
    /// The icon's preview, for its path.
    preview: Option<(String, Option<egui::TextureHandle>)>,
    picking: Option<Receiver<Option<PathBuf>>>,
    error: Option<String>,
}

impl std::fmt::Debug for GameDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GameDetails")
            .field("root", &self.root)
            .field("title", &self.title)
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl GameDetails {
    fn open(root: &Path) -> Option<Self> {
        let project = ab_project::Project::open(root).ok()?;
        let cargo_version = {
            let mut p = project.clone();
            p.export.version.clear();
            p.version()
        };
        Some(Self {
            root: project.root.clone(),
            title: project.title.clone(),
            saved: (project.title.clone(), project.export.clone()),
            settings: project.export.clone(),
            cargo_version,
            package: project.package.clone().unwrap_or_default(),
            preview: None,
            picking: None,
            error: None,
        })
    }

    fn dirty(&self) -> bool {
        (self.title.clone(), self.settings.clone()) != self.saved
    }

    fn save(&mut self) -> Result<(), String> {
        self.settings
            .save(&self.root, &self.title)
            .map_err(|e| format!("{e:#}"))?;
        self.saved = (self.title.clone(), self.settings.clone());
        Ok(())
    }

    /// The icon, decoded for the preview (PNG only; again when the path changes).
    fn preview(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        let path = self.settings.icon.clone();
        if self.preview.as_ref().map(|p| &p.0) != Some(&path) {
            let texture = self.settings.icon_path(&self.root).and_then(|file| {
                let image = image::open(file).ok()?.to_rgba8();
                let size = [image.width() as usize, image.height() as usize];
                let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
                Some(ctx.load_texture("export-icon", pixels, egui::TextureOptions::LINEAR))
            });
            self.preview = Some((path, texture));
        }
        self.preview.as_ref().and_then(|p| p.1.clone())
    }

    /// The fields and what is wrong with them; whether an error stops the export.
    fn show(&mut self, ui: &mut egui::Ui) -> bool {
        if let Some(picked) = self.picking.as_ref().and_then(|r| r.try_recv().ok()) {
            self.picking = None;
            if let Some(file) = picked {
                match file.strip_prefix(&self.root) {
                    Ok(relative) => {
                        self.settings.icon = relative.to_string_lossy().replace('\\', "/")
                    }
                    Err(_) => self.error = Some("Choose an icon inside the project folder".into()),
                }
            }
        }
        let issues = self.settings.check(&self.root, &self.title);
        let issue = |field: &str| issues.iter().find(|i| i.field == field);
        let field =
            |ui: &mut egui::Ui, name: &str, key: &'static str, value: &mut String, hint: &str| {
                ui.label(name);
                let edit = egui::TextEdit::singleline(value)
                    .hint_text(hint)
                    .desired_width(f32::INFINITY);
                let response = ui.add(edit);
                if let Some(i) = issue(key) {
                    let color = if i.error { theme::ERROR } else { theme::WARN };
                    response.on_hover_text(RichText::new(&i.message).color(color));
                }
                ui.end_row();
            };
        egui::Grid::new("export_details")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                field(ui, "Title", "title", &mut self.title, "The game's name");
                field(
                    ui,
                    "Creator",
                    "creator",
                    &mut self.settings.creator,
                    "You or your studio",
                );
                field(
                    ui,
                    "Description",
                    "description",
                    &mut self.settings.description,
                    "One line about the game",
                );
                field(
                    ui,
                    "Copyright",
                    "copyright",
                    &mut self.settings.copyright,
                    "© 2026 …",
                );
                let cargo = self.cargo_version.clone();
                field(ui, "Version", "version", &mut self.settings.version, &cargo);
                let package = self.package.clone();
                field(
                    ui,
                    "File name",
                    "file_name",
                    &mut self.settings.file_name,
                    &package,
                );
                ui.label("Icon");
                ui.horizontal(|ui| {
                    if let Some(texture) = self.preview(ui.ctx()) {
                        ui.add(
                            egui::Image::new(&texture).fit_to_exact_size(egui::vec2(32.0, 32.0)),
                        );
                    } else {
                        ui.label(RichText::new(icon::IMAGE).size(24.0).color(theme::TEXT_DIM));
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings.icon)
                            .hint_text("assets/icon.png (256 × 256)")
                            .desired_width(220.0),
                    );
                    if ui
                        .add_enabled(
                            self.picking.is_none(),
                            egui::Button::new(format!("{}  Choose…", icon::FOLDER_OPEN)),
                        )
                        .clicked()
                    {
                        let (send, receive) = mpsc::channel();
                        let picker = rfd::FileDialog::new()
                            .set_title("Choose the game's icon")
                            .add_filter("Icons", &["png", "ico"])
                            .set_directory(&self.root);
                        let ctx = ui.ctx().clone();
                        std::thread::spawn(move || {
                            let _ = send.send(picker.pick_file());
                            ctx.request_repaint();
                        });
                        self.picking = Some(receive);
                    }
                });
                ui.end_row();
                ui.label("Windows");
                ui.checkbox(
                    &mut self.settings.console,
                    "Console window (shows the game's log)",
                );
                ui.end_row();
            });
        for i in &issues {
            let (glyph, color) = if i.error {
                (icon::X_CIRCLE, theme::ERROR)
            } else {
                (icon::WARNING, theme::WARN)
            };
            ui.label(
                RichText::new(format!(
                    "{glyph}  {}: {}",
                    i.field.replace('_', " "),
                    i.message
                ))
                .small()
                .color(color),
            );
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.dirty(),
                    egui::Button::new(format!("{}  Save to game.toml", icon::FLOPPY_DISK)),
                )
                .clicked()
            {
                if let Err(err) = self.save() {
                    self.error = Some(err);
                }
            }
            if self.dirty() {
                ui.label(
                    RichText::new("Saved when the export starts")
                        .small()
                        .color(theme::TEXT_DIM),
                );
            }
        });
        if let Some(err) = &self.error {
            ui.label(RichText::new(err).color(theme::ERROR));
        }
        issues.iter().any(|i| i.error)
    }
}

impl ExportWindow {
    pub fn show_window(&mut self) {
        self.open = true;
    }

    /// Polls the job (call every frame); true while something runs in the background.
    pub fn poll(&mut self) -> bool {
        if let Some(task) = &mut self.check {
            if let Some(toolchain) = task.try_take() {
                self.toolchain = Some(toolchain);
                self.check = None;
            }
        }
        let running = self.job.as_mut().is_some_and(ExportJob::poll);
        running || self.check.is_some()
    }

    /// Short text for the status bar.
    pub fn status(&self) -> Option<(String, egui::Color32)> {
        let job = self.job.as_ref()?;
        Some(match &job.state {
            JobState::Running => (
                format!("{} Exporting: {}", icon::PACKAGE, job.progress.step),
                theme::ACCENT,
            ),
            JobState::Succeeded => (format!("{} Export finished", icon::PACKAGE), theme::PLAY),
            JobState::Failed(_) => (format!("{} Export failed", icon::PACKAGE), theme::ERROR),
            JobState::Cancelled => return None,
        })
    }

    /// `unsaved`: the open scene has changes not on disk (the export reads the files).
    /// Returns true when the user asked to save first.
    pub fn show(&mut self, ctx: &egui::Context, assets: &Path, unsaved: bool) -> bool {
        if !self.open {
            return false;
        }
        let target = export_target(assets);
        if let (None, None) = (&self.toolchain, &self.check) {
            self.check = Some(jobs::spawn_background("ab-export-check", check_toolchain));
        }
        let mut save = false;
        let mut open = self.open;
        egui::Window::new(format!("{}  Export Game", icon::PACKAGE))
            .id(egui::Id::new("export_window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(520.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                let Some(target) = &target else {
                    ui.label(
                        RichText::new(format!(
                            "{}  Only projects with scripts (a Cargo.toml) can be exported.",
                            icon::WARNING
                        ))
                        .color(theme::WARN),
                    );
                    return;
                };
                save = self.contents(ui, target, unsaved);
            });
        self.open = open;
        save
    }

    fn contents(&mut self, ui: &mut egui::Ui, target: &ExportTarget, unsaved: bool) -> bool {
        let options = self
            .options
            .get_or_insert_with(|| ExportOptions::new(&target.out));
        let running = self
            .job
            .as_ref()
            .is_some_and(|j| j.state == JobState::Running);
        let mut save = false;

        ui.horizontal(|ui| {
            ui.label(RichText::new(&target.title).heading().strong());
            ui.label(RichText::new(format!("({})", target.name)).color(theme::TEXT_DIM));
        });
        ui.add_space(4.0);
        let root = Path::new(&target.spec);
        if self
            .details
            .as_ref()
            .is_none_or(|d| d.root != ab_project::path::canonicalize_lossy(root))
        {
            self.details = GameDetails::open(root);
            // The output folder the game names.
            if let Some(out) = self
                .details
                .as_ref()
                .and_then(|d| d.settings.out_dir(&d.root))
            {
                options.out = out.to_string_lossy().into_owned();
            }
        }
        let mut details_error = false;
        if let Some(details) = &mut self.details {
            ui.add_enabled_ui(!running, |ui| {
                egui::CollapsingHeader::new(
                    RichText::new(format!("{}  Game details", icon::IDENTIFICATION_CARD)).strong(),
                )
                .default_open(true)
                .show(ui, |ui| details_error = details.show(ui));
            });
            ui.add_space(4.0);
        }

        ui.add_enabled_ui(!running, |ui| {
            egui::Grid::new("export_options")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Platforms");
                    ui.horizontal(|ui| {
                        ui.checkbox(
                            &mut options.windows,
                            format!("{} Windows (.exe)", icon::WINDOWS_LOGO),
                        );
                        ui.checkbox(&mut options.linux, format!("{} Linux", icon::LINUX_LOGO));
                        // Apple's linker and SDK exist only on a Mac.
                        if cfg!(target_os = "macos") {
                            ui.checkbox(
                                &mut options.macos,
                                format!("{} macOS (.app)", icon::APPLE_LOGO),
                            );
                        }
                    });
                    ui.end_row();
                    ui.label("Package");
                    ui.vertical(|ui| {
                        ui.checkbox(
                            &mut options.archive,
                            "Archive to upload (.zip for Windows, .tar.gz for Linux and macOS)",
                        );
                        ui.checkbox(
                            &mut options.single_file,
                            "Single file (assets inside the executable)",
                        );
                        ui.checkbox(&mut options.extras, "README and license files")
                            .on_hover_text(
                                "Off: the folder holds only the game and its assets. The \
                                 licenses ship inside the assets either way (the game shows them \
                                 with --licenses).",
                            );
                    });
                    ui.end_row();
                    ui.label("Output folder");
                    ui.add(
                        egui::TextEdit::singleline(&mut options.out).desired_width(f32::INFINITY),
                    );
                    ui.end_row();
                });
        });

        // What could stop it before it starts.
        let mut blocked = None;
        if details_error {
            blocked = Some("Fix the game details marked in red first.".to_owned());
        }
        if options.platforms() == 0 {
            blocked = Some("Pick at least one platform.".to_owned());
        }
        if options.linux && !cfg!(target_os = "linux") {
            blocked =
                Some("Linux builds need a Linux PC (or WSL on Windows): untick Linux here.".into());
        }
        if options.windows && !cfg!(windows) {
            match &self.toolchain {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new("Checking the Windows toolchain…").color(theme::TEXT_DIM),
                        );
                    });
                }
                Some(t) if t.windows_target && t.xwin => {
                    ui.label(
                        RichText::new(format!(
                            "{}  Windows builds are made here with cargo-xwin.",
                            icon::CHECK_CIRCLE
                        ))
                        .color(theme::TEXT_DIM),
                    );
                }
                Some(t) => {
                    let mut steps = Vec::new();
                    if !t.windows_target {
                        steps.push("rustup target add x86_64-pc-windows-msvc");
                    }
                    if !t.xwin {
                        steps.push("cargo install --locked cargo-xwin");
                    }
                    ui.label(
                        RichText::new(format!(
                            "{}  Windows builds from Linux need, once:",
                            icon::WARNING
                        ))
                        .color(theme::WARN),
                    );
                    for step in &steps {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(*step).monospace());
                            if ui.small_button(icon::COPY).on_hover_text("Copy").clicked() {
                                ui.ctx().copy_text((*step).to_owned());
                            }
                        });
                    }
                    if ui
                        .small_button(format!("{} Check again", icon::ARROWS_CLOCKWISE))
                        .clicked()
                    {
                        self.toolchain = None;
                    }
                    blocked =
                        Some("Install the Windows toolchain first (or untick Windows).".into());
                }
            }
        }
        if unsaved {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{}  The scene has unsaved changes; the export uses the files on disk.",
                        icon::WARNING
                    ))
                    .color(theme::WARN),
                );
                if ui.small_button("Save").clicked() {
                    save = true;
                }
            });
        }

        ui.separator();
        ui.horizontal(|ui| {
            if running {
                if ui.button(format!("{}  Cancel", icon::STOP)).clicked() {
                    if let Some(job) = &mut self.job {
                        job.cancel();
                    }
                }
            } else {
                // A Windows build from Linux waits for the toolchain check.
                let checked = !options.windows || cfg!(windows) || self.toolchain.is_some();
                let export = ui
                    .add_enabled(
                        blocked.is_none() && checked,
                        egui::Button::new(
                            RichText::new(format!("{}  Export", icon::PACKAGE)).strong(),
                        )
                        .fill(theme::ACCENT_DIM),
                    )
                    .on_disabled_hover_text(blocked.clone().unwrap_or_else(|| "Checking…".into()));
                if export.clicked() {
                    self.start_error = None;
                    // The exporter reads game.toml: the details go there first.
                    let saved = match self.details.as_mut().filter(|d| d.dirty()) {
                        Some(details) => details.save(),
                        None => Ok(()),
                    };
                    let started = saved
                        .map_err(anyhow::Error::msg)
                        .and_then(|()| ExportJob::start(target, options));
                    match started {
                        Ok(job) => self.job = Some(job),
                        Err(err) => self.start_error = Some(format!("{err:#}")),
                    }
                }
                if let Some(reason) = &blocked {
                    ui.label(RichText::new(reason).color(theme::TEXT_DIM));
                }
            }
        });
        if let Some(err) = &self.start_error {
            ui.label(RichText::new(err).color(theme::ERROR));
        }

        if let Some(job) = &self.job {
            ui.add_space(6.0);
            Self::job_view(ui, job, &mut self.show_log);
        }
        save
    }

    fn job_view(ui: &mut egui::Ui, job: &ExportJob, show_log: &mut bool) {
        let seconds = job.elapsed().as_secs();
        let time = format!("{}:{:02}", seconds / 60, seconds % 60);
        match &job.state {
            JobState::Running => {
                let per_platform = 1.0 / job.platforms.max(1) as f32;
                // Builds compile a few hundred crates the first time; the bar eases towards
                // the end of each platform's share instead of pretending to know.
                let within = 1.0 - (-(job.progress.compiled as f32) / 120.0).exp();
                let fraction = (job.progress.done as f32 + within * 0.9) * per_platform;
                ui.horizontal(|ui| {
                    ui.spinner();
                    let platform = job.progress.platform.as_deref().unwrap_or("…");
                    ui.label(RichText::new(format!("{platform}: {}", job.progress.step)).strong());
                    ui.label(RichText::new(time).color(theme::TEXT_DIM));
                });
                ui.add(egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).show_percentage());
                ui.label(
                    RichText::new("The first build of each platform takes a few minutes; later ones are faster.")
                        .small()
                        .color(theme::TEXT_DIM),
                );
            }
            JobState::Succeeded => {
                ui.label(
                    RichText::new(format!("{}  Exported in {time}", icon::CHECK_CIRCLE))
                        .strong()
                        .color(theme::PLAY),
                );
                let outputs = &job.progress.outputs;
                for (glyph, path) in outputs
                    .folders
                    .iter()
                    .map(|p| (icon::FOLDER, p))
                    .chain(outputs.archives.iter().map(|p| (icon::FILE_ZIP, p)))
                {
                    ui.horizontal(|ui| {
                        if ui.small_button(format!("{glyph} Open")).clicked() {
                            reveal(path);
                        }
                        ui.label(RichText::new(path.display().to_string()).monospace());
                    });
                }
                if job.progress.outputs.folders.iter().any(|f| {
                    f.file_name()
                        .is_some_and(|n| n.to_string_lossy().ends_with("-windows"))
                }) && !cfg!(windows)
                {
                    ui.label(
                        RichText::new(format!(
                            "{}  The Windows build was checked but not run here: try it once on a Windows PC.",
                            icon::INFO
                        ))
                        .color(theme::TEXT_DIM),
                    );
                }
            }
            JobState::Failed(error) => {
                ui.label(
                    RichText::new(format!("{}  Export failed after {time}", icon::X_CIRCLE))
                        .strong()
                        .color(theme::ERROR),
                );
                ui.label(RichText::new(error).color(theme::ERROR));
                *show_log = true;
            }
            JobState::Cancelled => {
                ui.label(
                    RichText::new(format!("{}  Cancelled", icon::STOP)).color(theme::TEXT_DIM),
                );
            }
        }
        let header = egui::CollapsingHeader::new(format!("Output ({} lines)", job.log.len()))
            .id_salt("export_log")
            .open(Some(*show_log))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .stick_to_bottom(true)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for line in &job.log {
                            let color = if line.starts_with("error") || line.starts_with("Error") {
                                theme::ERROR
                            } else if line.starts_with("warning") {
                                theme::WARN
                            } else {
                                theme::TEXT_DIM
                            };
                            ui.label(RichText::new(line).monospace().size(11.0).color(color));
                        }
                    });
            });
        if header.header_response.clicked() {
            *show_log = !*show_log;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_details_are_read_checked_and_saved_to_game_toml() {
        let root = std::env::temp_dir().join(format!("ab-export-details-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("game.toml"), "title = \"Relics\"\n").unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"relics\"\nversion = \"0.3.0\"\n",
        )
        .unwrap();
        let mut details = GameDetails::open(&root).unwrap();
        assert_eq!(
            (details.cargo_version.as_str(), details.package.as_str()),
            ("0.3.0", "relics")
        );
        assert!(!details.dirty());
        details.settings.creator = "Moonlit".into();
        details.settings.version = "1.0.0".into();
        details.settings.file_name = "con".into();
        assert!(details.dirty());
        let issues = details.settings.check(&details.root, &details.title);
        assert!(issues.iter().any(|i| i.field == "file_name" && i.error));
        details.settings.file_name = "NightRelics".into();
        details.title = "Night Relics".into();
        details.save().unwrap();
        assert!(!details.dirty());
        let project = ab_project::Project::open(&root).unwrap();
        assert_eq!(project.title, "Night Relics");
        assert_eq!(project.export.creator, "Moonlit");
        assert_eq!(
            project.version(),
            "1.0.0",
            "the export version wins over Cargo's"
        );
        assert_eq!(
            ab_project::export::Game::Project(Box::new(project)).name(),
            "NightRelics"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn finds_the_game_of_a_project() {
        let engine = engine_root().expect("tests run in the engine tree");
        let slice = export_target(&engine.join("assets")).unwrap();
        assert_eq!(
            (slice.spec.as_str(), slice.name.as_str()),
            ("slice", "night-relics")
        );
        assert!(export_target(&std::env::temp_dir()).is_none());

        // A project anywhere: named by its package, exported into its own dist/.
        let root = std::env::temp_dir().join(format!("ab-export-target-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let project = ab_project::create(&ab_project::NewProject {
            title: "My Game".into(),
            location: root.clone(),
            new_folder: true,
        })
        .unwrap();
        let target = export_target(&project.assets).unwrap();
        assert_eq!(
            (target.name.as_str(), target.title.as_str()),
            ("my-game", "My Game")
        );
        assert_eq!(target.spec, project.root.to_string_lossy());
        assert_eq!(target.out, project.root.join("dist"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_output_folder_is_inside_the_game_not_the_editors_working_folder() {
        let root = std::env::temp_dir().join("ab-export-out").join("game");
        let target = ExportTarget {
            spec: root.to_string_lossy().into_owned(),
            name: "game".into(),
            title: "Game".into(),
            out: root.join("dist"),
        };
        let mut options = ExportOptions::new(&target.out);
        options.out = "builds/v1".into();
        assert_eq!(
            Path::new(&options.for_target(&target).out),
            root.join("builds/v1")
        );
        options.out = "  ".into();
        assert_eq!(
            Path::new(&options.for_target(&target).out),
            root.join("dist")
        );
        let elsewhere = std::env::temp_dir().join("elsewhere");
        options.out = elsewhere.to_string_lossy().into_owned();
        assert_eq!(Path::new(&options.for_target(&target).out), elsewhere);
    }

    #[test]
    fn options_become_export_arguments() {
        let mut options = ExportOptions {
            linux: true,
            windows: true,
            macos: false,
            archive: true,
            single_file: false,
            extras: false,
            out: " /tmp/out ".into(),
        };
        assert_eq!(
            options.args("my-game"),
            [
                "--export",
                "my-game",
                "--platform",
                "linux,windows",
                "--archive",
                "--out",
                "/tmp/out"
            ]
        );
        options.linux = false;
        options.archive = false;
        options.single_file = true;
        options.extras = true;
        options.out.clear();
        assert_eq!(
            options.args("slice"),
            [
                "--export",
                "slice",
                "--platform",
                "windows",
                "--single-file",
                "--extras"
            ]
        );
    }

    /// The exporter the Export button runs, with real builds of Night Relics for both
    /// platforms (minutes): `cargo test -p ab-editor export_job -- --ignored`. (The button
    /// runs it in an `ab-editor --export` process; this calls it directly.)
    #[test]
    #[ignore]
    fn export_job_builds_both_platforms() {
        let engine = engine_root().unwrap();
        let out = std::env::temp_dir().join(format!("ab-editor-export-{}", std::process::id()));
        let options = ab_project::export::Options::new(&out)
            .with_platforms(&[ab_project::Platform::Linux, ab_project::Platform::Windows]);
        let mut progress = Progress::default();
        let exported = ab_project::export::export(
            &ab_project::export::Game::slice(&engine),
            &ab_project::export::Options {
                archive: true,
                ..options
            },
            &mut |line: &str| progress.read(line),
        )
        .unwrap();
        assert_eq!(exported.len(), 2);
        assert_eq!(progress.outputs.folders.len(), 2);
        assert_eq!(progress.outputs.archives.len(), 2);
        assert!(out.join("night-relics-windows/night-relics.exe").is_file());
        assert!(out.join("night-relics-linux/night-relics").is_file());
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    #[cfg(unix)]
    fn cancel_stops_the_export() {
        let mut command = Command::new("sleep");
        command.arg("30");
        let mut job = ExportJob::spawn(command).unwrap();
        job.poll();
        job.cancel();
        assert_eq!(job.state, JobState::Cancelled);
        assert!(!job.poll());
        assert!(job.elapsed() < std::time::Duration::from_secs(10));
    }

    #[test]
    fn progress_follows_the_export_output() {
        let mut p = Progress::default();
        for line in [
            "=== My Game 0.1.0 for linux ===",
            "   Compiling wgpu v30.0.1",
            "   Compiling ab-engine v0.1.0 (/x)",
            "packed 3 files (3 compressed): 0.0 MB of assets → 0.0 MB",
            "exported Back to School to /Back to School/dist/my-game-linux",
            "folder: /Back to School/dist/my-game-linux",
            "archive: /dist/my-game-0.1.0-linux.tar.gz",
            "=== My Game 0.1.0 for windows ===",
        ] {
            p.read(line);
        }
        assert_eq!(p.platform.as_deref(), Some("windows"));
        assert_eq!((p.done, p.compiled), (1, 0));
        p.read("Compiling winit v0.30.13");
        assert_eq!(p.step, "Compiling winit");
        assert_eq!(
            p.outputs.folders,
            [PathBuf::from("/Back to School/dist/my-game-linux")]
        );
        assert_eq!(
            p.outputs.archives,
            [PathBuf::from("/dist/my-game-0.1.0-linux.tar.gz")]
        );
        assert_eq!(
            strip_ansi("\u{1b}[2m2026\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m done"),
            "2026  INFO done"
        );
    }
}
