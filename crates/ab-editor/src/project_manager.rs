//! The project manager: the editor's start screen. It lists recent projects, creates new
//! ones from the game template (in any folder), imports existing games, and opens a project
//! (building its scripts first). The editor itself only opens once a project is chosen.

use crate::chrome;
use crate::loading::{Level, Report};
use crate::projects::{
    self, BuildState, NewProject, Profile, Project, RecentProjects, ScriptBuild,
};
use crate::theme;
use ab_engine::save::SaveStore;
use eframe::egui::{self, Color32, FontId, Key, RichText, Sense, Stroke, Vec2};
use egui_phosphor::regular as icon;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

/// What the manager asks the shell to do.
pub enum Action {
    None,
    /// Open the editor on `project`, with its script library if it was built.
    Open {
        project: Project,
        scripts: Option<PathBuf>,
    },
}

enum Dialog {
    New {
        title: String,
        /// The folder the project goes in (typed or picked).
        location: String,
        /// Make a folder named after the project inside `location`.
        new_folder: bool,
        error: Option<String>,
    },
    Import {
        path: String,
        found: Vec<Project>,
        error: Option<String>,
    },
}

/// A project being opened: its scripts are building.
struct Opening {
    project: Project,
    build: Option<ScriptBuild>,
    started: Instant,
}

pub struct ProjectManager {
    store: SaveStore,
    recent: RecentProjects,
    report: Report,
    search: String,
    selected: Option<PathBuf>,
    dialog: Option<Dialog>,
    opening: Option<Opening>,
    /// A problem to show above the list (open failed, …).
    banner: Option<String>,
    show_system: bool,
    /// The system's folder picker, open on its own thread; sends the chosen folder.
    picker: Option<Receiver<Option<PathBuf>>>,
}

impl ProjectManager {
    pub fn new(recent: RecentProjects, report: Report) -> Self {
        let selected = recent.projects.first().map(|p| p.path.clone());
        Self {
            store: RecentProjects::store(),
            recent,
            report,
            search: String::new(),
            selected,
            dialog: None,
            opening: None,
            banner: None,
            show_system: false,
            picker: None,
        }
    }

    /// The manager again after closing a project (the list is re-read).
    pub fn reopen(report: Report) -> Self {
        let recent = RecentProjects::load(&RecentProjects::store()).unwrap_or_default();
        Self::new(recent, report)
    }

    fn save_recent(&mut self) {
        if let Err(err) = self.recent.save(&self.store) {
            tracing::warn!("cannot save the project list: {err:#}");
        }
    }

    pub fn update(&mut self, ui: &mut egui::Ui) -> Action {
        let ctx = &ui.ctx().clone();
        egui::Panel::top("pm-titlebar")
            .exact_size(chrome::HEIGHT)
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG_0)
                    .inner_margin(egui::Margin {
                        left: 12,
                        ..Default::default()
                    }),
            )
            .show(ui, |ui| {
                let title = chrome::Title {
                    document: "Project Manager",
                    modified: false,
                    project: None,
                    playing: false,
                };
                chrome::title_bar(ui, &title, |ui| {
                    ui.label(
                        RichText::new(format!("{} April Blink", icon::LIGHTNING))
                            .strong()
                            .color(theme::ACCENT),
                    );
                });
            });
        self.sidebar(ui);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG_1)
                    .inner_margin(egui::Margin::symmetric(32, 24)),
            )
            .show(ui, |ui| self.main(ui));

        if self.show_system {
            self.system_window(ctx);
        }
        let mut action = Action::None;
        if self.dialog.is_some() {
            modal_backdrop(ctx);
            self.dialog(ctx);
        } else if self.opening.is_some() {
            modal_backdrop(ctx);
            action = self.opening_window(ctx);
        } else {
            self.shortcuts(ctx);
        }
        chrome::window_frame(ctx);
        action
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::N)) {
            self.new_dialog();
        }
        if ctx.input(|i| i.key_pressed(Key::Enter)) {
            if let Some(path) = self.selected.clone() {
                self.open(&path);
            }
        }
        let step = ctx
            .input(|i| i.key_pressed(Key::ArrowDown) as i32 - i.key_pressed(Key::ArrowUp) as i32);
        if step != 0 {
            let list = self.filtered();
            let current = list.iter().position(|p| Some(p) == self.selected.as_ref());
            let next = match current {
                Some(i) => (i as i32 + step).clamp(0, list.len() as i32 - 1) as usize,
                None => 0,
            };
            self.selected = list.get(next).cloned();
        }
    }

    // =========================================================================================
    // Layout
    // =========================================================================================

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("pm-side")
            .exact_size(220.0)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG_0)
                    .inner_margin(egui::Margin::symmetric(12, 16))
                    .stroke(Stroke::new(1.0_f32, theme::BORDER)),
            )
            .show(ui, |ui| {
                nav_item(ui, icon::FOLDERS, "Projects", true);
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.label(
                        RichText::new(format!("April Blink Editor {}", env!("CARGO_PKG_VERSION")))
                            .size(11.0)
                            .color(theme::TEXT_DIM),
                    );
                    ui.add_space(6.0);
                    let (glyph, color, text) = match self.report.worst() {
                        Level::Ok => (icon::CHECK_CIRCLE, theme::PLAY, "System ready".to_owned()),
                        Level::Warn => (
                            icon::WARNING,
                            theme::WARN,
                            plural(self.report.count(Level::Warn), "warning"),
                        ),
                        Level::Fail => (
                            icon::X_CIRCLE,
                            theme::ERROR,
                            plural(self.report.count(Level::Fail), "problem"),
                        ),
                    };
                    let chip =
                        egui::Button::new(RichText::new(format!("{glyph}  {text}")).color(color))
                            .fill(theme::BG_2)
                            .min_size(Vec2::new(ui.available_width(), 28.0));
                    if ui
                        .add(chip)
                        .on_hover_text("Show the start-up system check")
                        .clicked()
                    {
                        self.show_system = !self.show_system;
                    }
                });
            });
    }

    fn main(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("Projects")
                        .size(24.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.label(
                    RichText::new("Create a project, or open one, to start editing.")
                        .color(theme::TEXT_DIM),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(accent_button(&format!("{}  New Project", icon::PLUS)))
                    .on_hover_text("Ctrl+N")
                    .clicked()
                {
                    self.new_dialog();
                }
                if ui
                    .add(
                        egui::Button::new(format!("{}  Import…", icon::FOLDER_OPEN))
                            .min_size(Vec2::new(0.0, 30.0)),
                    )
                    .clicked()
                {
                    self.import_dialog();
                }
            });
        });
        ui.add_space(16.0);

        if let Some(banner) = self.banner.clone() {
            egui::Frame::NONE
                .fill(Color32::from_rgb(58, 30, 32))
                .stroke(Stroke::new(1.0_f32, theme::ERROR))
                .corner_radius(4)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::WARNING_CIRCLE).color(theme::ERROR));
                        ui.add(
                            egui::Label::new(RichText::new(banner).color(Color32::WHITE)).wrap(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(icon::X).clicked() {
                                self.banner = None;
                            }
                        });
                    });
                });
            ui.add_space(12.0);
        }

        if self.recent.projects.is_empty() {
            self.empty_state(ui);
            return;
        }

        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text(format!("{}  Search projects", icon::MAGNIFYING_GLASS))
                .desired_width(320.0)
                .margin(egui::Margin::symmetric(8, 6)),
        );
        ui.add_space(10.0);

        let list = self.filtered();
        if list.is_empty() {
            ui.label(RichText::new("No project matches the search.").color(theme::TEXT_DIM));
            return;
        }
        let now = projects::now();
        let mut open = None;
        let mut remove = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                for path in &list {
                    let Some(entry) = self.recent.projects.iter().find(|p| &p.path == path) else {
                        continue;
                    };
                    let selected = self.selected.as_ref() == Some(path);
                    let row = project_row(ui, entry, selected, now);
                    if row.response.clicked() {
                        self.selected = Some(path.clone());
                    }
                    if row.response.double_clicked() || row.open {
                        open = Some(path.clone());
                    }
                    if row.remove {
                        remove = Some(path.clone());
                    }
                    row.response.context_menu(|ui| {
                        if ui.button(format!("{}  Open", icon::FOLDER_OPEN)).clicked() {
                            open = Some(path.clone());
                            ui.close();
                        }
                        if ui
                            .button(format!("{}  Remove from list", icon::TRASH))
                            .clicked()
                        {
                            remove = Some(path.clone());
                            ui.close();
                        }
                    });
                }
            });
        if let Some(path) = remove {
            self.recent.remove(&path);
            self.save_recent();
            if self.selected.as_ref() == Some(&path) {
                self.selected = None;
            }
        }
        if let Some(path) = open {
            self.open(&path);
        }
    }

    fn filtered(&self) -> Vec<PathBuf> {
        let needle = self.search.trim().to_lowercase();
        self.recent
            .projects
            .iter()
            .filter(|p| {
                needle.is_empty()
                    || p.title.to_lowercase().contains(&needle)
                    || p.path.to_string_lossy().to_lowercase().contains(&needle)
            })
            .map(|p| p.path.clone())
            .collect()
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        ui.add_space((ui.available_height() * 0.18).max(12.0));
        ui.vertical_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(96.0), Sense::hover());
            ui.painter().circle_filled(rect.center(), 48.0, theme::BG_2);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                icon::FOLDER_SIMPLE_PLUS,
                FontId::proportional(44.0),
                theme::ACCENT,
            );
            ui.add_space(14.0);
            ui.label(
                RichText::new("Create your first project")
                    .size(20.0)
                    .strong()
                    .color(Color32::WHITE),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "A project holds your game's scenes, scripts and assets.\n\
                     The editor opens once you have one.",
                )
                .color(theme::TEXT_DIM),
            );
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                // Center the two buttons.
                let width = 160.0 + 120.0 + ui.spacing().item_spacing.x;
                ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                if ui
                    .add(
                        accent_button(&format!("{}  New Project", icon::PLUS))
                            .min_size(Vec2::new(160.0, 34.0)),
                    )
                    .clicked()
                {
                    self.new_dialog();
                }
                if ui
                    .add(
                        egui::Button::new(format!("{}  Import…", icon::FOLDER_OPEN))
                            .min_size(Vec2::new(120.0, 34.0)),
                    )
                    .clicked()
                {
                    self.import_dialog();
                }
            });
        });
    }

    // =========================================================================================
    // Opening
    // =========================================================================================

    /// Opens the project in `path`: builds its scripts, then the editor opens on it.
    pub fn open_project(&mut self, path: &Path) {
        self.open(path);
    }

    fn open(&mut self, path: &Path) {
        let project = match Project::open(path) {
            Ok(project) => project,
            Err(err) => {
                self.banner = Some(format!("Cannot open the project: {err:#}"));
                return;
            }
        };
        self.banner = None;
        self.recent.touch(&project);
        self.save_recent();
        self.selected = Some(project.root.clone());
        let build = if project.package.is_some() {
            match ScriptBuild::start(&project, Profile::Debug) {
                Ok(build) => Some(build),
                Err(err) => {
                    tracing::warn!("cannot build the scripts: {err:#}");
                    None
                }
            }
        } else {
            None
        };
        self.opening = Some(Opening {
            project,
            build,
            started: Instant::now(),
        });
    }

    fn opening_window(&mut self, ctx: &egui::Context) -> Action {
        let Some(opening) = &mut self.opening else {
            return Action::None;
        };
        let state = opening
            .build
            .as_mut()
            .map_or(BuildState::Succeeded, |b| b.poll().clone());
        let library = opening.project.editor_library().filter(|l| l.is_file());
        if state == BuildState::Succeeded {
            let opening = self.opening.take().expect("checked above");
            tracing::info!(
                project = %opening.project.root.display(),
                build_ms = opening.started.elapsed().as_millis() as u64,
                "opening project"
            );
            return Action::Open {
                project: opening.project,
                scripts: library,
            };
        }

        let mut choice = None;
        modal_window(ctx, "opening", 460.0, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(icon::FOLDER_OPEN)
                        .size(22.0)
                        .color(theme::ACCENT),
                );
                ui.label(
                    RichText::new(format!("Opening {}", opening.project.title))
                        .size(17.0)
                        .strong()
                        .color(Color32::WHITE),
                );
            });
            ui.add_space(10.0);
            match &state {
                BuildState::Running => {
                    let build = opening.build.as_ref().expect("running builds exist");
                    ui.label(RichText::new("Building the game's scripts…").color(Color32::WHITE));
                    ui.label(
                        RichText::new(format!(
                            "{} · {} crate{} compiled · {:.0} s",
                            build.progress,
                            build.compiled,
                            if build.compiled == 1 { "" } else { "s" },
                            opening.started.elapsed().as_secs_f32()
                        ))
                        .size(11.5)
                        .color(theme::TEXT_DIM),
                    );
                    ui.add_space(10.0);
                    indeterminate_bar(ui);
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(
                            "The first build of a project compiles the engine and can take a few minutes.",
                        )
                        .size(11.5)
                        .color(theme::TEXT_DIM),
                    );
                }
                BuildState::Failed(errors) => {
                    ui.label(
                        RichText::new(format!("{}  The scripts did not build", icon::X_CIRCLE))
                            .color(theme::ERROR),
                    );
                    ui.add_space(6.0);
                    egui::Frame::NONE
                        .fill(theme::BG_0)
                        .corner_radius(4)
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(180.0)
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(errors).monospace().size(11.0),
                                        )
                                        .wrap(),
                                    );
                                });
                        });
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Fix the errors and open again, or edit the scenes without scripts.",
                        )
                        .size(11.5)
                        .color(theme::TEXT_DIM),
                    );
                }
                BuildState::Succeeded => {}
            }
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let skip = if library.is_some() {
                    "Open with the last build"
                } else {
                    "Open without scripts"
                };
                if ui.add(accent_button(skip)).clicked() {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    choice = Some(false);
                }
            });
        });
        ctx.request_repaint();
        match choice {
            Some(true) => {
                let opening = self.opening.take().expect("checked above");
                Action::Open {
                    project: opening.project,
                    scripts: library,
                }
            }
            Some(false) => {
                self.opening = None;
                Action::None
            }
            None => Action::None,
        }
    }

    // =========================================================================================
    // Dialogs
    // =========================================================================================

    fn new_dialog(&mut self) {
        let location = self
            .recent
            .last_location
            .clone()
            .filter(|dir| dir.is_dir())
            .or_else(projects::projects_dir)
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();
        self.dialog = Some(Dialog::New {
            title: String::new(),
            location,
            new_folder: true,
            error: None,
        });
    }

    /// Opens the system's folder picker (which can also make new folders) without blocking
    /// the UI; the choice arrives in [`Self::dialog`].
    fn pick_folder(&mut self, ctx: &egui::Context, title: &str, start: &str) {
        if self.picker.is_some() {
            return;
        }
        let (send, receive) = mpsc::channel();
        let mut picker = rfd::FileDialog::new().set_title(title);
        let start = Path::new(start);
        if start.is_dir() {
            picker = picker.set_directory(start);
        }
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = send.send(picker.pick_folder());
            ctx.request_repaint();
        });
        self.picker = Some(receive);
    }

    fn import_dialog(&mut self) {
        self.dialog = Some(Dialog::Import {
            path: String::new(),
            found: projects::unlisted_projects(&self.recent),
            error: None,
        });
    }

    fn dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        if let Some(picked) = self.picker.as_ref().and_then(|p| p.try_recv().ok()) {
            self.picker = None;
            if let Some(folder) = picked {
                let folder = folder.display().to_string();
                match &mut dialog {
                    Dialog::New {
                        location, error, ..
                    } => {
                        *location = folder;
                        *error = None;
                    }
                    Dialog::Import { path, error, .. } => {
                        *path = folder;
                        *error = None;
                    }
                }
            }
        }
        let keep = match &mut dialog {
            Dialog::New {
                title,
                location,
                new_folder,
                error,
            } => self.new_project_window(ctx, title, location, new_folder, error),
            Dialog::Import { path, found, error } => self.import_window(ctx, path, found, error),
        };
        if keep && self.dialog.is_none() {
            self.dialog = Some(dialog);
        }
    }

    /// Returns whether the dialog stays open.
    fn new_project_window(
        &mut self,
        ctx: &egui::Context,
        title: &mut String,
        location: &mut String,
        new_folder: &mut bool,
        error: &mut Option<String>,
    ) -> bool {
        let mut keep = true;
        let mut create = false;
        let mut browse = false;
        let new = NewProject {
            title: title.clone(),
            location: PathBuf::from(expand_home(location.trim())),
            new_folder: *new_folder,
        };
        modal_window(ctx, "new-project", 540.0, |ui| {
            ui.label(
                RichText::new("New Project")
                    .size(18.0)
                    .strong()
                    .color(Color32::WHITE),
            );
            ui.add_space(14.0);
            field_label(ui, "Name");
            let edit = ui.add(
                egui::TextEdit::singleline(title)
                    .hint_text("My Game")
                    .desired_width(f32::INFINITY)
                    .margin(egui::Margin::symmetric(8, 6)),
            );
            if ui.memory(|m| m.focused().is_none()) {
                edit.request_focus();
            }

            ui.add_space(10.0);
            field_label(ui, "Location");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(location)
                        .hint_text("/path/to/folder")
                        .desired_width(ui.available_width() - 96.0)
                        .margin(egui::Margin::symmetric(8, 6)),
                );
                let picking = self.picker.is_some();
                if ui
                    .add_enabled(
                        !picking,
                        egui::Button::new(format!("{}  Browse…", icon::FOLDER_OPEN)),
                    )
                    .on_hover_text("Choose a folder, or make a new one, in the system's picker")
                    .clicked()
                {
                    browse = true;
                }
            });
            ui.checkbox(new_folder, "Create a new folder for the project");
            let check = new.validate();
            ui.add_space(2.0);
            let shown = if new.package().is_empty() && *new_folder {
                new.location.join("<name>")
            } else {
                new.root()
            };
            ui.label(
                RichText::new(format!("{}  {}", icon::FOLDER_SIMPLE, shown.display()))
                    .size(11.5)
                    .color(theme::TEXT_DIM),
            );

            ui.add_space(14.0);
            ui.label(
                RichText::new(format!(
                    "{}  Starts with a floor, a light and a camera, and a script crate with \
                     nothing in it yet.",
                    icon::CUBE
                ))
                .color(theme::TEXT_DIM),
            );

            ui.add_space(12.0);
            let message = match (&check, error.as_deref()) {
                (_, Some(failed)) => Some((failed.to_owned(), theme::ERROR)),
                (Err(problem), None) if !title.trim().is_empty() => {
                    Some((problem.clone(), theme::WARN))
                }
                _ => None,
            };
            if let Some((text, color)) = message {
                ui.label(RichText::new(text).color(color));
                ui.add_space(6.0);
            }
            ui.label(
                RichText::new(
                    "The project is a Rust crate with its own folder: the editor builds its \
                     scripts with cargo and loads them; nothing else is needed.",
                )
                .size(11.5)
                .color(theme::TEXT_DIM),
            );
            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ok = check.is_ok();
                if ui
                    .add_enabled(ok, accent_button(&format!("{}  Create", icon::PLUS)))
                    .clicked()
                    || (ok && ui.input(|i| i.key_pressed(Key::Enter)))
                {
                    create = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    keep = false;
                }
            });
        });
        if browse {
            self.pick_folder(ctx, "Where to create the project", location);
        }
        if create {
            match projects::create(&new) {
                Ok(project) => {
                    tracing::info!(project = %project.root.display(), "project created");
                    let root = project.root.clone();
                    self.recent.last_location = Some(new.location.clone());
                    self.recent.touch(&project);
                    self.save_recent();
                    self.open(&root);
                    return false;
                }
                Err(err) => *error = Some(format!("{err:#}")),
            }
        }
        if title.is_empty() {
            *error = None;
        }
        keep
    }

    fn import_window(
        &mut self,
        ctx: &egui::Context,
        path: &mut String,
        found: &mut Vec<Project>,
        error: &mut Option<String>,
    ) -> bool {
        let mut keep = true;
        let mut add: Option<Project> = None;
        let mut browse = false;
        let picking = self.picker.is_some();
        modal_window(ctx, "import-project", 540.0, |ui| {
            ui.label(
                RichText::new("Import Project")
                    .size(18.0)
                    .strong()
                    .color(Color32::WHITE),
            );
            ui.add_space(4.0);
            ui.label(RichText::new("Add an existing game to the list.").color(theme::TEXT_DIM));
            ui.add_space(12.0);
            if !found.is_empty() {
                field_label(ui, "Found in April Blink Projects");
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .show(ui, |ui| {
                        for project in found.iter() {
                            ui.horizontal(|ui| {
                                tile(ui, &project.title, 28.0);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    ui.label(RichText::new(&project.title).color(Color32::WHITE));
                                    ui.label(
                                        RichText::new(project.root.display().to_string())
                                            .size(11.0)
                                            .color(theme::TEXT_DIM),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.button(format!("{}  Add", icon::PLUS)).clicked() {
                                            add = Some(project.clone());
                                        }
                                    },
                                );
                            });
                            ui.add_space(4.0);
                        }
                    });
                ui.add_space(12.0);
            }
            field_label(ui, "Folder");
            ui.horizontal(|ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(path)
                        .hint_text("/path/to/my-game")
                        .desired_width(ui.available_width() - 150.0)
                        .margin(egui::Margin::symmetric(8, 6)),
                );
                let submit = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if ui
                    .add_enabled(
                        !picking,
                        egui::Button::new(format!("{}  Browse…", icon::FOLDER_OPEN)),
                    )
                    .clicked()
                {
                    browse = true;
                }
                if ui
                    .add_enabled(!path.trim().is_empty(), egui::Button::new("Add"))
                    .clicked()
                    || (submit && !path.trim().is_empty())
                {
                    match Project::open(Path::new(expand_home(path.trim()).as_str())) {
                        Ok(project) => add = Some(project),
                        Err(err) => *error = Some(format!("{err:#}")),
                    }
                }
            });
            ui.label(
                RichText::new("A game folder (with game.toml) or its assets folder.")
                    .size(11.5)
                    .color(theme::TEXT_DIM),
            );
            if let Some(error) = error.as_deref() {
                ui.add_space(6.0);
                ui.label(RichText::new(error).color(theme::ERROR));
            }
            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Done").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    keep = false;
                }
            });
        });
        if browse {
            self.pick_folder(ctx, "Choose the game's folder", path);
        }
        if let Some(project) = add {
            self.recent.add(&project);
            self.save_recent();
            self.selected = Some(project.root.clone());
            found.retain(|p| p.root != project.root);
            path.clear();
            *error = None;
        }
        keep
    }

    fn system_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        egui::Window::new("System check")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(egui::Align2::LEFT_BOTTOM, Vec2::new(236.0, -16.0))
            .show(ctx, |ui| {
                for (title, outcome) in &self.report.results {
                    ui.horizontal(|ui| {
                        let (glyph, color) = match outcome.level {
                            Level::Ok => (icon::CHECK_CIRCLE, theme::PLAY),
                            Level::Warn => (icon::WARNING, theme::WARN),
                            Level::Fail => (icon::X_CIRCLE, theme::ERROR),
                        };
                        ui.label(RichText::new(glyph).size(15.0).color(color));
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(RichText::new(*title).color(Color32::WHITE));
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&outcome.detail)
                                        .size(11.0)
                                        .color(theme::TEXT_DIM),
                                )
                                .wrap(),
                            );
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        self.show_system = open;
    }
}

// =============================================================================================
// Widgets
// =============================================================================================

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// `~/x` (or `~\x`) → `$HOME/x`.
fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map(|home| format!("{home}/{rest}"))
            .unwrap_or_else(|_| path.to_owned()),
        None => path.to_owned(),
    }
}

fn accent_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(Color32::WHITE))
        .fill(theme::ACCENT_DIM)
        .stroke(Stroke::new(1.0_f32, theme::ACCENT))
        .min_size(Vec2::new(0.0, 30.0))
}

fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .size(11.0)
            .strong()
            .color(theme::TEXT_DIM),
    );
    ui.add_space(2.0);
}

fn nav_item(ui: &mut egui::Ui, glyph: &str, text: &str, active: bool) {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::click());
    let fill = if active {
        theme::BG_2
    } else if response.hovered() {
        theme::BG_1
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    if active {
        let stripe =
            egui::Rect::from_min_size(rect.min + Vec2::new(0.0, 7.0), Vec2::new(3.0, 18.0));
        ui.painter().rect_filled(stripe, 1.5, theme::ACCENT);
    }
    let color = if active {
        Color32::WHITE
    } else {
        theme::TEXT_DIM
    };
    ui.painter().text(
        rect.left_center() + Vec2::new(14.0, 0.0),
        egui::Align2::LEFT_CENTER,
        format!("{glyph}   {text}"),
        FontId::proportional(13.5),
        color,
    );
}

/// A colored square with the project's initials.
fn tile(ui: &mut egui::Ui, title: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let hash = title.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    });
    let palette = [
        Color32::from_rgb(64, 138, 255),
        Color32::from_rgb(76, 194, 120),
        Color32::from_rgb(232, 178, 64),
        Color32::from_rgb(200, 120, 255),
        Color32::from_rgb(240, 110, 150),
        Color32::from_rgb(64, 196, 210),
    ];
    let color = palette[hash as usize % palette.len()];
    ui.painter()
        .rect_filled(rect, size * 0.22, color.gamma_multiply(0.28));
    ui.painter().rect_stroke(
        rect.shrink(0.5),
        size * 0.22,
        Stroke::new(1.0_f32, color.gamma_multiply(0.6)),
        egui::StrokeKind::Inside,
    );
    let initials: String = title
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        if initials.is_empty() {
            "?".into()
        } else {
            initials
        },
        FontId::proportional(size * 0.4),
        color,
    );
}

struct Row {
    response: egui::Response,
    open: bool,
    remove: bool,
}

fn project_row(
    ui: &mut egui::Ui,
    entry: &projects::RecentProject,
    selected: bool,
    now: u64,
) -> Row {
    let missing = !entry.path.is_dir();
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 62.0), Sense::click());
    let hovered = response.hovered();
    let fill = if selected {
        Color32::from_rgb(33, 45, 66)
    } else if hovered {
        theme::BG_2
    } else {
        theme::BG_1
    };
    let stroke = if selected {
        Stroke::new(1.0_f32, theme::ACCENT_DIM)
    } else {
        Stroke::new(1.0_f32, theme::BORDER)
    };
    ui.painter()
        .rect(rect, 6.0, fill, stroke, egui::StrokeKind::Inside);

    let mut open = false;
    let mut remove = false;
    let inner = rect.shrink2(Vec2::new(14.0, 10.0));
    let mut content = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    {
        let ui = &mut content;
        ui.horizontal_centered(|ui| {
            tile(ui, &entry.title, 40.0);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.add_space(2.0);
                let title_color = if missing {
                    theme::TEXT_DIM
                } else {
                    Color32::WHITE
                };
                ui.label(
                    RichText::new(&entry.title)
                        .size(14.5)
                        .strong()
                        .color(title_color),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(entry.path.display().to_string())
                            .size(11.0)
                            .color(theme::TEXT_DIM),
                    )
                    .truncate(),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if hovered || selected {
                    if ui
                        .small_button(icon::TRASH)
                        .on_hover_text("Remove from list")
                        .clicked()
                    {
                        remove = true;
                    }
                    if !missing && ui.add(accent_button("Open")).clicked() {
                        open = true;
                    }
                    ui.add_space(8.0);
                }
                if missing {
                    ui.label(
                        RichText::new(format!("{}  Folder not found", icon::WARNING))
                            .size(11.5)
                            .color(theme::ERROR),
                    );
                } else {
                    ui.label(
                        RichText::new(projects::ago(entry.last_opened, now))
                            .size(11.5)
                            .color(theme::TEXT_DIM),
                    );
                }
            });
        });
    }
    Row {
        response,
        open,
        remove,
    }
}

/// A bar whose highlight sweeps back and forth (progress of unknown length).
fn indeterminate_bar(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), Sense::hover());
    ui.painter().rect_filled(rect, 3.0, theme::BG_3);
    let t = ui.input(|i| i.time) as f32;
    let phase = (t * 0.8).fract();
    let width = rect.width() * 0.3;
    let x = rect.left() - width + (rect.width() + width) * phase;
    let bar = egui::Rect::from_min_max(
        egui::pos2(x.max(rect.left()), rect.top()),
        egui::pos2((x + width).min(rect.right()), rect.bottom()),
    );
    if bar.width() > 0.0 {
        ui.painter().rect_filled(bar, 3.0, theme::ACCENT);
    }
}

/// Darkens everything behind a modal window.
fn modal_backdrop(ctx: &egui::Context) {
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("modal-backdrop"))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
            ui.painter()
                .rect_filled(rect, 0.0, Color32::from_black_alpha(150));
        });
}

/// A centered window without a title bar, above the backdrop.
fn modal_window(ctx: &egui::Context, id: &str, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Window::new(id)
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .fixed_size(Vec2::new(width, 0.0))
        .frame(
            egui::Frame::window(&ctx.global_style())
                .fill(theme::BG_2)
                .inner_margin(egui::Margin::same(22))
                .corner_radius(8),
        )
        .show(ctx, add);
}
