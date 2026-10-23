//! The editor window: menu bar, toolbar, dockable panels, status bar, dialogs and
//! keyboard shortcuts.

use crate::chrome;
use crate::commands::{self, Command, Focus};
use crate::console::LogBuffer;
use crate::dialogs::UnsavedChoice;
use crate::git::{GitClient, RepoState};
use crate::i18n::{t, tf};
use crate::panels::{
    assets::AssetsPanel, console::ConsolePanel, git::GitPanel, inspector::InspectorPanel,
    profiler::ProfilerPanel, scene_tree::SceneTreePanel,
};
use crate::project::AssetKind;
use crate::projects::{self, BuildState, Project, ScriptBuild};
use crate::settings::EditorSettings;
use crate::state::{Editor, Mode};
use crate::theme;
use crate::viewport::GizmoMode;
use crate::viewports::{Layout, ViewportArea};
use crate::Options;
use ab_engine::core::jobs::{self, Task};
use ab_engine::core::Vec3;
use ab_engine::scene_file::NodeDesc;
use eframe::egui::{self, Key, RichText};
use eframe::egui_wgpu;
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use egui_phosphor::regular as icon;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Tab {
    Scene,
    Viewport,
    Inspector,
    Environment,
    Assets,
    Console,
    Profiler,
    Debug,
    SourceControl,
    Effects,
    Animation,
    Config,
    Build,
}

impl Tab {
    const ALL: [Tab; 13] = [
        Tab::Scene,
        Tab::SourceControl,
        Tab::Viewport,
        Tab::Inspector,
        Tab::Environment,
        Tab::Debug,
        Tab::Effects,
        Tab::Assets,
        Tab::Animation,
        Tab::Console,
        Tab::Profiler,
        Tab::Config,
        Tab::Build,
    ];

    fn title(self) -> String {
        format!("{}  {}", self.icon(), t(&format!("tab.{}", self.name())))
    }

    fn icon(self) -> &'static str {
        match self {
            Tab::Scene => icon::TREE_STRUCTURE,
            Tab::Viewport => icon::CUBE_FOCUS,
            Tab::Inspector => icon::SLIDERS_HORIZONTAL,
            Tab::Environment => icon::CLOUD_SUN,
            Tab::Assets => icon::FOLDERS,
            Tab::Console => icon::TERMINAL_WINDOW,
            Tab::Profiler => icon::CHART_LINE,
            Tab::Debug => icon::BUG_BEETLE,
            Tab::SourceControl => icon::GIT_BRANCH,
            Tab::Effects => icon::SPARKLE,
            Tab::Animation => icon::FILM_STRIP,
            Tab::Config => icon::SLIDERS,
            Tab::Build => icon::CUBE,
        }
    }

    /// The tab's name in the language files and for `--show` (`source_control`).
    fn name(self) -> &'static str {
        match self {
            Tab::Scene => "scene",
            Tab::Viewport => "viewport",
            Tab::Inspector => "inspector",
            Tab::Environment => "environment",
            Tab::Assets => "assets",
            Tab::Console => "console",
            Tab::Profiler => "profiler",
            Tab::Debug => "debug",
            Tab::SourceControl => "source_control",
            Tab::Effects => "effects",
            Tab::Animation => "animation",
            Tab::Config => "config",
            Tab::Build => "build",
        }
    }
}

fn default_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Viewport]);
    let surface = dock.main_surface_mut();
    let [center, _right] = surface.split_right(
        NodeIndex::root(),
        0.77,
        vec![
            Tab::Inspector,
            Tab::Environment,
            Tab::Effects,
            Tab::Config,
            Tab::Debug,
        ],
    );
    let [center, _left] = surface.split_left(center, 0.22, vec![Tab::Scene, Tab::SourceControl]);
    let [_center, _bottom] = surface.split_below(
        center,
        0.70,
        vec![
            Tab::Assets,
            Tab::Build,
            Tab::Animation,
            Tab::Console,
            Tab::Profiler,
        ],
    );
    dock
}

/// Something that would discard unsaved changes.
#[derive(Debug, Clone)]
enum Pending {
    New,
    Open(String),
    Quit,
    CloseProject,
}

enum Dialog {
    Open { filter: String },
    SaveAs { path: String },
    Unsaved(Pending),
    Shortcuts,
    About,
}

const OPEN_REQUEST: &str = "ab-editor.open-request";

/// Where Save As suggests saving a scene that has no file yet.
const NEW_SCENE_PATH: &str = "scenes/new_scene.abscene";

/// Asks the app to open a scene (from a panel), checking for unsaved changes first.
pub fn request_open(ctx: &egui::Context, path: String) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(OPEN_REQUEST), path));
}

/// Adds a model or scene file to the scene as an instance.
pub fn instance_asset(editor: &mut Editor, path: &str, at: Option<Vec3>) {
    let stem = std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Instance".into());
    // Node names cannot contain path separators or dots.
    let name: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if editor.scene_path.as_deref() == Some(path) {
        editor.fail("A scene cannot contain itself");
        return;
    }
    editor.add_node(
        NodeDesc {
            name,
            instance: Some(path.to_owned()),
            ..Default::default()
        },
        at,
    );
}

pub struct EditorApp {
    editor: Editor,
    viewport: ViewportArea,
    dock: DockState<Tab>,
    scene_tree: SceneTreePanel,
    inspector: InspectorPanel,
    effects: crate::panels::effects::EffectsPanel,
    animation: crate::panels::animation::AnimationPanel,
    config: crate::panels::config::ConfigPanel,
    build: crate::panels::build::BuildPanel,
    assets: AssetsPanel,
    console: ConsolePanel,
    profiler: ProfilerPanel,
    git_panel: GitPanel,
    /// The project's git worker, started with the first frame (it needs the UI context to
    /// wake it).
    git: Option<GitClient>,
    dialog: Option<Dialog>,
    last_frame: Instant,
    title: String,
    allow_close: bool,
    /// The editor draws its own title bar (no `--system-titlebar`).
    custom_frame: bool,
    /// Rebuilds the game's scripts when their sources change (projects opened from the
    /// project manager; `cargo xtask editor` watches them itself).
    watch: Option<ScriptWatch>,
    /// File → Close Project was chosen: the shell goes back to the project manager.
    close_project: bool,
    /// File → Export Game.
    export: crate::export::ExportWindow,
    /// The editor preferences in effect (the viewport's options are copied in).
    settings: EditorSettings,
    /// The preferences as last saved, to save again only on a change.
    saved_settings: EditorSettings,
    /// Where the settings are kept (`None`: not kept, for screenshots).
    settings_store: Option<ab_engine::save::SaveStore>,
    /// Edit → Preferences.
    preferences: crate::preferences::PreferencesWindow,
    /// Controllers, opened on first use (`Some(None)`: none can be read here).
    gamepads: Option<Option<ab_engine::platform::Gamepads>>,
}

/// Watches a project's `src/` and rebuilds its script library on change; the engine's hot
/// reload then swaps the new scripts in.
pub struct ScriptWatch {
    project: Project,
    seen: Option<SystemTime>,
    last_check: Instant,
    /// Walks `src/` for the newest change, off the UI thread.
    check: Option<Task<Option<SystemTime>>>,
    build: Option<ScriptBuild>,
}

impl ScriptWatch {
    pub fn new(project: Project) -> Self {
        Self {
            seen: projects::newest_change(&project.sources()),
            project,
            last_check: Instant::now(),
            check: None,
            build: None,
        }
    }

    fn poll(&mut self, editor: &mut Editor) {
        if let Some(build) = &mut self.build {
            match build.poll().clone() {
                BuildState::Running => return,
                BuildState::Succeeded => editor.info("Scripts rebuilt"),
                BuildState::Failed(errors) => {
                    tracing::error!("the scripts did not build:\n{errors}");
                    editor.fail("The scripts did not build (see Console)");
                }
            }
            self.build = None;
        }
        let Some(check) = &mut self.check else {
            if self.last_check.elapsed() >= Duration::from_secs(1) {
                let sources = self.project.sources();
                self.check = Some(jobs::spawn_background("ab-script-watch", move || {
                    projects::newest_change(&sources)
                }));
            }
            return;
        };
        let Some(newest) = check.try_take() else {
            return;
        };
        self.check = None;
        self.last_check = Instant::now();
        if newest != self.seen {
            self.seen = newest;
            match ScriptBuild::start(&self.project, projects::Profile::Debug) {
                Ok(build) => {
                    editor.info("Building scripts…");
                    self.build = Some(build);
                }
                Err(err) => editor.fail(format!("Cannot build the scripts: {err:#}")),
            }
        }
    }

    fn building(&self) -> Option<&str> {
        self.build.as_ref().map(|b| b.progress.as_str())
    }
}

impl EditorApp {
    /// The editor on `project` (asset directory) with `options`' scene, scripts, ….
    pub fn new(
        render_state: Option<&egui_wgpu::RenderState>,
        project: PathBuf,
        options: &Options,
        logs: LogBuffer,
        watch: Option<ScriptWatch>,
    ) -> Self {
        let options = options.clone();
        let project = ab_project::path::canonicalize_lossy(&project);
        let project_dir = project.clone();
        let mut editor = Editor::new(project, options.scripts, Default::default());
        if let Some(scene) = &options.scene {
            editor.open_scene(scene);
        }
        if let Some(paths) = &options.select {
            let nodes = paths
                .split(',')
                .filter_map(|path| editor.node_at(path.trim()))
                .collect();
            editor.set_selection(nodes, None);
        }
        let mut viewport = ViewportArea::new(render_state);
        // Screenshots and tests start from the defaults, not this machine's preferences.
        let settings_store = options.screenshot.is_none().then(EditorSettings::store);
        let mut settings = settings_store
            .as_ref()
            .map(EditorSettings::load)
            .unwrap_or_default();
        if let Some(language) = &options.language {
            settings.language = language.clone();
        }
        viewport.options.snap = settings.snap;
        viewport.options.highlight_selection = settings.highlight_selection;
        viewport.options.highlight_hover = settings.highlight_hover;
        if options.scene.is_some() {
            viewport.frame_selection(&editor);
        }
        if options.physics_view {
            if let Some(mut settings) = editor
                .tree()
                .world()
                .resource_mut::<ab_engine::DebugSettings>()
            {
                settings.physics = true;
            }
        }
        if options.play {
            editor.play();
        }
        let mut dock = default_layout();
        for name in &options.show {
            let tab = Tab::ALL
                .into_iter()
                .find(|t| t.name().replace('_', " ").ends_with(name.as_str()));
            if let Some(found) = tab.and_then(|t| dock.find_tab(&t)) {
                // Found just now, so the index is valid.
                _ = dock.set_active_tab(found);
            }
        }
        // `--show quad`: the four views (screenshots of them).
        if options.show.iter().any(|name| name == "quad") {
            viewport.set_layout(Layout::Quad);
        }
        let mut export = crate::export::ExportWindow::default();
        if options.show.iter().any(|name| name == "export") {
            export.show_window();
        }
        // `--show unsaved`: the Unsaved Changes dialog (screenshots of it).
        let dialog = options
            .show
            .iter()
            .any(|name| name == "unsaved")
            .then_some(Dialog::Unsaved(Pending::New));
        Self {
            editor,
            viewport,
            dock,
            scene_tree: SceneTreePanel::default(),
            inspector: InspectorPanel::default(),
            effects: Default::default(),
            animation: Default::default(),
            config: Default::default(),
            build: Default::default(),
            assets: AssetsPanel::starting_at(
                options.browse.as_deref(),
                options.view.as_deref(),
                &project_dir,
            ),
            console: ConsolePanel::new(logs),
            profiler: ProfilerPanel::default(),
            git_panel: GitPanel::default(),
            git: None,
            dialog,
            last_frame: Instant::now(),
            title: String::new(),
            allow_close: false,
            custom_frame: !options.system_titlebar,
            watch,
            close_project: false,
            export,
            saved_settings: settings.clone(),
            settings,
            settings_store,
            preferences: Default::default(),
            gamepads: None,
        }
    }

    /// Feeds controller input to the game while it plays; otherwise drops it, so a press
    /// made while editing does not reach the next Play.
    fn poll_gamepads(&mut self) {
        let gamepads = self
            .gamepads
            .get_or_insert_with(ab_engine::platform::Gamepads::new);
        let Some(gamepads) = gamepads else {
            return;
        };
        let playing = self.editor.is_playing();
        let engine = &mut self.editor.engine;
        gamepads.poll(|event| {
            if playing {
                engine.handle_event(&event);
            }
        });
    }

    /// Copies the viewport's options into the preferences and saves them when they changed.
    fn save_settings(&mut self) {
        let options = &self.viewport.options;
        self.settings.snap = options.snap.sanitized();
        self.settings.highlight_selection = options.highlight_selection;
        self.settings.highlight_hover = options.highlight_hover;
        if self.settings == self.saved_settings {
            return;
        }
        if let Some(store) = &self.settings_store {
            // The engine folder may have been found since start-up: keep the newest.
            if self.settings.engine_root == self.saved_settings.engine_root {
                self.settings.engine_root = EditorSettings::load(store).engine_root;
            }
            self.settings.save(store);
        }
        self.saved_settings = self.settings.clone();
    }

    /// File → Close Project was chosen (and unsaved changes were dealt with).
    pub fn wants_close_project(&self) -> bool {
        self.close_project
    }

    /// Forget unsaved changes (closing for a screenshot).
    pub fn discard_changes(&mut self) {
        self.editor.modified = false;
    }

    /// Runs `pending` now, or asks first when there are unsaved changes.
    fn guard(&mut self, pending: Pending) {
        if self.editor.modified && !self.editor.is_playing() {
            self.dialog = Some(Dialog::Unsaved(pending));
        } else {
            self.run(pending);
        }
    }

    fn run(&mut self, pending: Pending) {
        if self.editor.is_playing() {
            self.editor.stop();
        }
        match pending {
            Pending::New => self.editor.new_scene(),
            Pending::Open(path) => {
                self.editor.open_scene(&path);
                self.viewport.frame_selection(&self.editor);
            }
            Pending::Quit => self.allow_close = true,
            Pending::CloseProject => self.close_project = true,
        }
    }

    fn save_or_ask(&mut self) {
        if self.editor.is_playing() {
            return;
        }
        if self.editor.scene_path.is_some() {
            self.editor.save();
        } else {
            self.dialog = Some(Dialog::SaveAs {
                path: NEW_SCENE_PATH.into(),
            });
        }
    }

    fn toggle_play(&mut self) {
        if self.editor.is_playing() {
            self.editor.stop();
            self.viewport.game_focus = false;
        } else {
            self.editor.play();
            self.viewport.game_focus = true;
        }
    }

    // =========================================================================================
    // Shortcuts
    // =========================================================================================

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let focus = Focus {
            game: self.viewport.game_focus && self.editor.is_playing(),
            text: ctx.egui_wants_keyboard_input(),
        };
        // Tool keys share WASD/QE with flying the camera (right mouse held).
        let flying = ctx.input(|i| i.pointer.secondary_down());
        for (command, text) in commands::pressed(ctx, focus) {
            let tool = matches!(
                command,
                Command::ToolSelect | Command::ToolMove | Command::ToolRotate | Command::ToolScale
            );
            if !(tool && flying) {
                self.execute(ctx, command, text);
            }
        }
    }

    /// Runs `command`. `text`: the system clipboard's text, for a paste.
    fn execute(&mut self, ctx: &egui::Context, command: Command, text: Option<String>) {
        let editing = !self.editor.is_playing();
        // While building, R turns the piece and Esc puts the tool away.
        if let Some(piece) = &mut self.editor.build {
            match command {
                Command::ToolScale => {
                    piece.turns = (piece.turns + 1) % 4;
                    return;
                }
                Command::SelectNone => {
                    self.editor.build = None;
                    return;
                }
                _ => {}
            }
        }
        match command {
            Command::NewScene if editing => self.guard(Pending::New),
            Command::OpenScene if editing => {
                self.dialog = Some(Dialog::Open {
                    filter: String::new(),
                });
            }
            Command::Save => self.save_or_ask(),
            Command::SaveAs if editing => {
                self.dialog = Some(Dialog::SaveAs {
                    path: self
                        .editor
                        .scene_path
                        .clone()
                        .unwrap_or_else(|| NEW_SCENE_PATH.into()),
                });
            }
            Command::ExportGame => self.export.show_window(),
            Command::Undo => self.editor.undo(),
            Command::Redo => self.editor.redo(),
            Command::Copy => {
                if let Some(text) = self.editor.copy_selected() {
                    ctx.copy_text(text);
                }
            }
            Command::Cut => {
                if let Some(text) = self.editor.cut_selected() {
                    ctx.copy_text(text);
                }
            }
            Command::Paste => self.editor.paste(text.as_deref(), None, None),
            Command::Duplicate => self.editor.duplicate_selected(),
            Command::Delete => self.editor.delete_selected(),
            Command::Rename => self.scene_tree.start_rename(&self.editor),
            Command::SelectAll => {
                let scene = self.editor.scene_root;
                let all: Vec<_> = self
                    .editor
                    .tree()
                    .children(scene)
                    .into_iter()
                    .filter(|&n| self.editor.is_editable(n))
                    .collect();
                self.editor.set_selection(all, None);
            }
            Command::SelectNone if self.editor.mode == Mode::Edit => self.editor.select(None),
            Command::FrameSelection => self.viewport.frame_selection(&self.editor),
            Command::Play => self.toggle_play(),
            Command::Stop => {
                self.editor.stop();
                self.viewport.game_focus = false;
            }
            Command::Pause => self.editor.toggle_pause(),
            Command::StepFrame => self.editor.step_frame(),
            Command::ToolSelect => self.viewport.options.gizmo = GizmoMode::Select,
            Command::ToolMove => self.viewport.options.gizmo = GizmoMode::Translate,
            Command::ToolRotate => self.viewport.options.gizmo = GizmoMode::Rotate,
            Command::ToolScale => self.viewport.options.gizmo = GizmoMode::Scale,
            Command::ToggleSnap => {
                let snap = &mut self.viewport.options.snap;
                snap.enabled = !snap.enabled;
                self.editor.info(t(if snap.enabled {
                    "status.snap_on"
                } else {
                    "status.snap_off"
                }));
            }
            Command::ToggleHighlight => {
                let options = &mut self.viewport.options;
                options.highlight_selection = !options.highlight_selection;
                self.editor.info(t(if options.highlight_selection {
                    "status.highlight_on"
                } else {
                    "status.highlight_off"
                }));
            }
            Command::Shortcuts => self.dialog = Some(Dialog::Shortcuts),
            Command::Preferences => self.preferences.show_window(),
            // Not while playing.
            Command::NewScene | Command::OpenScene | Command::SaveAs | Command::SelectNone => {}
        }
    }

    // =========================================================================================
    // Chrome
    // =========================================================================================

    /// The menus (File, Edit, …), inside the title bar or the system-framed menu bar.
    fn menus(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new(format!("{} April Blink", icon::LIGHTNING))
                .strong()
                .color(theme::ACCENT),
        );
        ui.add_space(8.0);
        let editing = !self.editor.is_playing();
        ui.menu_button(t("menu.file"), |ui| {
            ui.set_min_width(220.0);
            commands::menu_item(ui, Command::NewScene, editing);
            commands::menu_item(ui, Command::OpenScene, editing);
            ui.menu_button(
                format!("{}  Open Recent Scene", icon::CLOCK_COUNTER_CLOCKWISE),
                |ui| {
                    let scenes: Vec<String> = self
                        .editor
                        .assets
                        .of_kind(AssetKind::Scene)
                        .map(|a| a.path.clone())
                        .take(12)
                        .collect();
                    for scene in scenes {
                        if ui.button(&scene).clicked() {
                            self.guard(Pending::Open(scene));
                            ui.close();
                        }
                    }
                },
            );
            ui.separator();
            commands::menu_item(ui, Command::Save, editing);
            commands::menu_item(ui, Command::SaveAs, editing);
            ui.separator();
            commands::menu_item(ui, Command::ExportGame, true);
            ui.separator();
            if self.watch.is_some()
                && menu_item(ui, icon::FOLDERS, &t("menu.close_project"), "", editing)
            {
                self.guard(Pending::CloseProject);
            }
            if menu_item(ui, icon::SIGN_OUT, &t("menu.quit"), "", true) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        ui.menu_button(t("menu.edit"), |ui| {
            ui.set_min_width(230.0);
            let selection = self.editor.selection.is_some() && editing;
            commands::menu_item(ui, Command::Undo, editing && self.editor.history.can_undo());
            commands::menu_item(ui, Command::Redo, editing && self.editor.history.can_redo());
            ui.separator();
            commands::menu_item(ui, Command::Cut, selection);
            commands::menu_item(ui, Command::Copy, selection);
            commands::menu_item(ui, Command::Paste, self.editor.can_paste(None));
            commands::menu_item(ui, Command::Duplicate, selection);
            commands::menu_item(ui, Command::Delete, selection);
            commands::menu_item(ui, Command::Rename, selection);
            ui.separator();
            commands::menu_item(ui, Command::SelectAll, editing);
            commands::menu_item(ui, Command::SelectNone, !self.editor.selected.is_empty());
            commands::menu_item(ui, Command::FrameSelection, true);
            ui.separator();
            ui.menu_button(format!("{}  {}", icon::MAGNET, t("menu.snapping")), |ui| {
                crate::viewport::snap_settings(ui, &mut self.viewport.options.snap);
            });
            let options = &mut self.viewport.options;
            ui.checkbox(
                &mut options.highlight_selection,
                format!("{} (H)", t("menu.highlight_selection")),
            );
            ui.checkbox(&mut options.highlight_hover, t("menu.highlight_hover"));
            ui.separator();
            commands::menu_item(ui, Command::Preferences, true);
        });
        ui.add_enabled_ui(editing, |ui| {
            ui.menu_button(t("menu.create"), |ui| {
                let at = self.viewport.camera.pivot;
                crate::create::menu(ui, &mut self.editor, at);
            });
        });
        ui.menu_button(t("menu.view"), |ui| {
            ui.set_min_width(220.0);
            for tab in Tab::ALL {
                let open = self.dock.find_tab(&tab).is_some();
                let mut checked = open;
                if ui.checkbox(&mut checked, tab.title()).clicked() {
                    match self.dock.find_tab(&tab) {
                        Some(found) if !checked && tab != Tab::Viewport => {
                            self.dock.remove_tab(found);
                        }
                        None if checked => self.dock.push_to_focused_leaf(tab),
                        _ => {}
                    }
                }
            }
            ui.separator();
            let mut quad = self.viewport.layout == Layout::Quad;
            if ui
                .checkbox(&mut quad, t("menu.four_views"))
                .on_hover_text(t("viewport.layout"))
                .changed()
            {
                self.viewport
                    .set_layout(if quad { Layout::Quad } else { Layout::Single });
            }
            let options = &mut self.viewport.options;
            ui.checkbox(&mut options.grid, t("menu.grid"));
            ui.checkbox(&mut options.icons, t("menu.node_icons"));
            ui.checkbox(&mut options.collision, t("menu.collision"));
            ui.checkbox(
                &mut options.highlight_selection,
                t("menu.highlight_selection"),
            );
            ui.checkbox(&mut options.ui, t("menu.edit_ui"));
            ui.checkbox(&mut options.stats, t("menu.statistics"));
            ui.checkbox(&mut options.editor_lighting, t("menu.editor_lighting"));
            ui.checkbox(&mut self.editor.preview_effects, t("menu.preview_effects"));
            ui.separator();
            if ui
                .button(format!("{}  {}", icon::LAYOUT, t("menu.reset_layout")))
                .clicked()
            {
                self.dock = default_layout();
                ui.close();
            }
        });
        ui.menu_button(t("menu.play"), |ui| {
            ui.set_min_width(220.0);
            let playing = self.editor.is_playing();
            commands::menu_item(ui, Command::Play, !playing);
            let paused = matches!(self.editor.mode, Mode::Play { paused: true });
            if menu_item(
                ui,
                icon::PAUSE,
                &if paused {
                    t("menu.resume")
                } else {
                    Command::Pause.label()
                },
                "F6",
                playing,
            ) {
                self.editor.toggle_pause();
            }
            commands::menu_item(ui, Command::Stop, playing);
            let paused = matches!(self.editor.mode, Mode::Play { paused: true });
            commands::menu_item(ui, Command::StepFrame, paused);
        });
        ui.menu_button(t("menu.help"), |ui| {
            ui.set_min_width(200.0);
            commands::menu_item(ui, Command::Shortcuts, true);
            if menu_item(ui, icon::INFO, &t("menu.about"), "", true) {
                self.dialog = Some(Dialog::About);
            }
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let editing = !self.editor.is_playing();
            if tool(
                ui,
                icon::FILE_PLUS,
                &Command::NewScene.tooltip(ui.ctx()),
                editing,
            ) {
                self.guard(Pending::New);
            }
            if tool(
                ui,
                icon::FOLDER_OPEN,
                &Command::OpenScene.tooltip(ui.ctx()),
                editing,
            ) {
                self.dialog = Some(Dialog::Open {
                    filter: String::new(),
                });
            }
            if tool(
                ui,
                icon::FLOPPY_DISK,
                &Command::Save.tooltip(ui.ctx()),
                editing,
            ) {
                self.save_or_ask();
            }
            ui.separator();
            if tool(
                ui,
                icon::ARROW_U_UP_LEFT,
                &Command::Undo.tooltip(ui.ctx()),
                editing && self.editor.history.can_undo(),
            ) {
                self.editor.undo();
            }
            if tool(
                ui,
                icon::ARROW_U_UP_RIGHT,
                &Command::Redo.tooltip(ui.ctx()),
                editing && self.editor.history.can_redo(),
            ) {
                self.editor.redo();
            }
            ui.separator();
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
                ui.selectable_value(
                    &mut self.viewport.options.gizmo,
                    mode,
                    RichText::new(glyph).size(15.0),
                )
                .on_hover_text(tip);
            }

            // Play controls, centered.
            let width = 3.0 * 36.0;
            let space = (ui.available_width() / 2.0 - width / 2.0 - 120.0).max(8.0);
            ui.add_space(space);
            let playing = self.editor.is_playing();
            let paused = matches!(self.editor.mode, Mode::Play { paused: true });
            let play = egui::Button::new(RichText::new(icon::PLAY).size(15.0).color(if playing {
                theme::BG_0
            } else {
                theme::PLAY
            }))
            .fill(if playing { theme::PLAY } else { theme::BG_3 })
            .min_size(egui::vec2(30.0, 24.0));
            if ui.add(play).on_hover_text("Play (F5)").clicked() && !playing {
                self.toggle_play();
            }
            let pause = egui::Button::new(RichText::new(icon::PAUSE).size(15.0))
                .fill(if paused {
                    theme::WARN.gamma_multiply(0.6)
                } else {
                    theme::BG_3
                })
                .min_size(egui::vec2(30.0, 24.0));
            if ui
                .add_enabled(playing, pause)
                .on_hover_text("Pause (F6)")
                .clicked()
            {
                self.editor.toggle_pause();
            }
            let stop = egui::Button::new(RichText::new(icon::STOP).size(15.0).color(if playing {
                theme::ERROR
            } else {
                theme::TEXT_DIM
            }))
            .min_size(egui::vec2(30.0, 24.0));
            if ui
                .add_enabled(playing, stop)
                .on_hover_text("Stop (Shift+F5)")
                .clicked()
            {
                self.editor.stop();
                self.viewport.game_focus = false;
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            match &self.editor.status {
                Some(status) if status.error => {
                    ui.label(
                        RichText::new(format!("{}  {}", icon::X_CIRCLE, status.text))
                            .color(theme::ERROR),
                    );
                }
                Some(status) => {
                    ui.label(
                        RichText::new(format!("{}  {}", icon::CHECK_CIRCLE, status.text))
                            .color(theme::TEXT_DIM),
                    );
                }
                None => {
                    ui.label(RichText::new(t("status.ready")).color(theme::TEXT_DIM));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let (mode, color) = match self.editor.mode {
                    Mode::Edit => (t("status.edit"), theme::ACCENT),
                    Mode::Play { paused: false } => (t("status.playing"), theme::PLAY),
                    Mode::Play { paused: true } => (t("status.paused"), theme::WARN),
                };
                ui.label(RichText::new(mode).strong().color(color));
                if let Some((text, color)) = self.export.status() {
                    let open = ui
                        .add(
                            egui::Label::new(RichText::new(text).color(color))
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_text("Click for the export window")
                        .clicked();
                    if open {
                        self.export.show_window();
                    }
                }
                if let Some(RepoState::Ready(status)) = self.git.as_ref().map(|g| &g.state) {
                    let branch = &status.branch;
                    let mut text = format!("{} {}", icon::GIT_BRANCH, branch.label());
                    if branch.ahead > 0 || branch.behind > 0 {
                        text.push_str(&format!(
                            "  {}{} {}{}",
                            icon::ARROW_UP,
                            branch.ahead,
                            icon::ARROW_DOWN,
                            branch.behind
                        ));
                    }
                    if !status.changes.is_empty() {
                        text.push_str(&format!("  {} {}", icon::PENCIL_SIMPLE_LINE, status.changes.len()));
                    }
                    let open = ui
                        .add(
                            egui::Label::new(RichText::new(text).color(theme::TEXT_DIM))
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_text("Git: branch, unpushed/unpulled commits and changed files. Click for Source Control.")
                        .clicked();
                    if open {
                        match self.dock.find_tab(&Tab::SourceControl) {
                            // Found just now, so the index is valid.
                            Some(found) => _ = self.dock.set_active_tab(found),
                            None => self.dock.push_to_focused_leaf(Tab::SourceControl),
                        }
                    }
                }
                let (errors, warnings) = self.console.counts();
                if errors > 0 {
                    ui.label(
                        RichText::new(format!("{} {errors}", icon::X_CIRCLE)).color(theme::ERROR),
                    );
                }
                if warnings > 0 {
                    ui.label(
                        RichText::new(format!("{} {warnings}", icon::WARNING)).color(theme::WARN),
                    );
                }
                let scripts = match &self.editor.scripts_library {
                    Some(lib) if self.editor.has_game_scripts() => format!(
                        "{} {}",
                        icon::SCROLL,
                        lib.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ),
                    Some(_) => format!("{} scripts failed to load", icon::SCROLL),
                    None => format!("{} no game scripts", icon::SCROLL),
                };
                ui.label(RichText::new(scripts).color(theme::TEXT_DIM))
                    .on_hover_text(
                        "Start with --scripts <game library> to run game scripts in Play mode",
                    );
                ui.label(
                    RichText::new(format!(
                        "{} {} nodes",
                        icon::TREE_STRUCTURE,
                        self.editor.tree().node_count()
                    ))
                    .color(theme::TEXT_DIM),
                );
                if let Some(path) = self
                    .editor
                    .selection
                    .and_then(|n| self.editor.relative_path(n))
                {
                    let more = self.editor.selected.len().saturating_sub(1);
                    let text = if more > 0 {
                        format!("{} {path} +{more}", icon::CURSOR)
                    } else {
                        format!("{} {path}", icon::CURSOR)
                    };
                    ui.label(RichText::new(text).color(theme::TEXT_DIM));
                }
            });
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let mut keep = true;
        let mut then: Option<Pending> = None;
        let modal = egui::Modal::new(egui::Id::new("dialog")).show(ctx, |ui| {
            if !matches!(dialog, Dialog::Unsaved(_)) {
                crate::dialogs::set_dialog_width(ui, 440.0);
            }
            match &mut dialog {
                Dialog::Open { filter } => {
                    ui.heading(format!("{}  {}", icon::FOLDER_OPEN, t("dialog.open_title")));
                    ui.add_space(4.0);
                    let edit = ui.add(
                        egui::TextEdit::singleline(filter)
                            .hint_text(format!("{}  {}", icon::MAGNIFYING_GLASS, t("dialog.search_scenes")))
                            .desired_width(f32::INFINITY),
                    );
                    edit.request_focus();
                    ui.add_space(4.0);
                    let needle = filter.to_lowercase();
                    let scenes: Vec<String> = self
                        .editor
                        .assets
                        .of_kind(AssetKind::Scene)
                        .map(|a| a.path.clone())
                        .filter(|p| p.to_lowercase().contains(&needle))
                        .collect();
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        if scenes.is_empty() {
                            crate::panels::empty_hint(ui, icon::FILM_SLATE, &t("dialog.no_scenes"));
                        }
                        for scene in &scenes {
                            let button = egui::Button::new(format!("{}  {scene}", icon::FILM_SLATE))
                                .min_size(egui::vec2(ui.available_width(), 26.0))
                                .frame(false);
                            if ui.add(button).clicked() {
                                then = Some(Pending::Open(scene.clone()));
                                keep = false;
                            }
                        }
                    });
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        if let Some(first) = scenes.first() {
                            then = Some(Pending::Open(first.clone()));
                            keep = false;
                        }
                    }
                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(t("dialog.cancel")).clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::SaveAs { path } => {
                    ui.heading(format!("{}  {}", icon::FLOPPY_DISK_BACK, t("dialog.save_as_title")));
                    ui.label(
                        RichText::new(tf("dialog.path_inside", &[("folder", &self.editor.project().display())]))
                            .color(theme::TEXT_DIM),
                    );
                    let edit = ui.add(egui::TextEdit::singleline(path).desired_width(f32::INFINITY));
                    edit.request_focus();
                    let valid = !path.trim().is_empty() && !path.contains("..");
                    let exists = self.editor.project().join(path.trim()).exists();
                    if exists {
                        ui.label(RichText::new(format!("{}  {}", icon::WARNING, t("dialog.replace_warning"))).color(theme::WARN));
                    }
                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let save = ui.add_enabled(valid, egui::Button::new(RichText::new(t("dialog.save")).strong()).fill(theme::ACCENT_DIM));
                        if save.clicked() || (valid && edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
                            self.editor.save_as(path);
                            keep = false;
                        }
                        if ui.button(t("dialog.cancel")).clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Unsaved(pending) => {
                    let scene = self.editor.scene_path.as_deref().unwrap_or("Untitled");
                    let action = t(match pending {
                        Pending::New => "dialog.before_new",
                        Pending::Open(_) => "dialog.before_open",
                        Pending::Quit => "dialog.before_quit",
                        Pending::CloseProject => "dialog.before_close",
                    });
                    match crate::dialogs::unsaved_changes(ui, scene, &action) {
                        Some(UnsavedChoice::Save) => {
                            if self.editor.scene_path.is_some() {
                                self.editor.save();
                                // A failed save keeps the changes (and says why).
                                if !self.editor.modified {
                                    then = Some(pending.clone());
                                }
                            } else {
                                self.dialog = Some(Dialog::SaveAs { path: NEW_SCENE_PATH.into() });
                            }
                            keep = false;
                        }
                        Some(UnsavedChoice::DontSave) => {
                            then = Some(pending.clone());
                            keep = false;
                        }
                        Some(UnsavedChoice::Cancel) => keep = false,
                        None => {}
                    }
                }
                Dialog::Shortcuts => {
                    ui.heading(format!("{}  {}", icon::KEYBOARD, t("dialog.shortcuts_title")));
                    // The keys come from the command table, so this list cannot go stale.
                    let mut rows: Vec<(String, String)> = Vec::new();
                    for binding in commands::BINDINGS {
                        let keys = ui.ctx().format_shortcut(&binding.shortcut);
                        match rows.iter_mut().find(|(_, what)| *what == binding.command.label()) {
                            Some((existing, _)) => *existing = format!("{existing}, {keys}"),
                            None => rows.push((keys, binding.command.label().to_owned())),
                        }
                    }
                    for name in [
                        "overlays",
                        "trace",
                        "multi_select",
                        "drill",
                        "context_menu",
                        "box_select",
                        "snap_hold",
                        "frame",
                        "fly",
                        "orbit",
                        "pan",
                        "zoom",
                        "release_mouse",
                    ] {
                        rows.push((t(&format!("shortcuts.{name}_keys")), t(&format!("shortcuts.{name}"))));
                    }
                    egui::ScrollArea::vertical().max_height(ui.ctx().content_rect().height() * 0.6).show(ui, |ui| {
                        egui::Grid::new("shortcuts").num_columns(2).spacing([24.0, 6.0]).striped(true).show(ui, |ui| {
                            for (keys, what) in &rows {
                                ui.label(RichText::new(keys).monospace().color(theme::ACCENT));
                                ui.label(what);
                                ui.end_row();
                            }
                        });
                    });
                    ui.separator();
                    if ui.button(t("dialog.close")).clicked() {
                        keep = false;
                    }
                }
                Dialog::About => {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(icon::LIGHTNING).size(40.0).color(theme::ACCENT));
                        ui.heading("April Blink Editor");
                        ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(theme::TEXT_DIM));
                        ui.add_space(6.0);
                        ui.add(egui::Label::new(crate::loading::TAGLINE).wrap());
                        ui.add_space(6.0);
                        ui.label("Scene editing, play-in-editor, effects and assets.");
                        ui.add(egui::Label::new(RichText::new("Game scripts are Rust: the editor builds them with cargo (https://rustup.rs) and hot-reloads them while you work.").color(theme::TEXT_DIM)).wrap());
                        ui.label(RichText::new(format!("Rust {} or newer · {} license", env!("CARGO_PKG_RUST_VERSION"), env!("CARGO_PKG_LICENSE"))).color(theme::TEXT_DIM));
                        ui.label(RichText::new(env!("CARGO_PKG_REPOSITORY")).size(11.5).color(theme::TEXT_DIM));
                        ui.add_space(6.0);
                        if ui.button(t("dialog.close")).clicked() {
                            keep = false;
                        }
                    });
                }
            }
        });
        if modal.should_close() {
            keep = false;
        }
        if keep && self.dialog.is_none() {
            self.dialog = Some(dialog);
        }
        if let Some(pending) = then {
            self.run(pending);
        }
    }
}

fn menu_item(ui: &mut egui::Ui, glyph: &str, text: &str, shortcut: &str, enabled: bool) -> bool {
    let button = egui::Button::new(format!("{glyph}  {text}")).shortcut_text(shortcut);
    let clicked = ui.add_enabled(enabled, button).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

fn tool(ui: &mut egui::Ui, glyph: &str, tip: &str, enabled: bool) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(glyph).size(15.0))
            .frame(false)
            .min_size(egui::vec2(26.0, 24.0)),
    )
    .on_hover_text(tip)
    .clicked()
}

struct Tabs<'a> {
    editor: &'a mut Editor,
    viewport: &'a mut ViewportArea,
    scene_tree: &'a mut SceneTreePanel,
    inspector: &'a mut InspectorPanel,
    effects: &'a mut crate::panels::effects::EffectsPanel,
    animation: &'a mut crate::panels::animation::AnimationPanel,
    config: &'a mut crate::panels::config::ConfigPanel,
    build: &'a mut crate::panels::build::BuildPanel,
    assets: &'a mut AssetsPanel,
    console: &'a mut ConsolePanel,
    profiler: &'a mut ProfilerPanel,
    git_panel: &'a mut GitPanel,
    git: Option<&'a mut GitClient>,
    render_state: Option<&'a egui_wgpu::RenderState>,
}

impl TabViewer for Tabs<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(*tab)
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Scene => {
                let focus = self.viewport.camera.pivot;
                self.scene_tree.show(ui, self.editor, focus);
                if std::mem::take(&mut self.scene_tree.frame_request) {
                    self.viewport.frame_selection(self.editor);
                }
            }
            Tab::Viewport => self.viewport.show(ui, self.editor, self.render_state),
            Tab::Inspector => self.inspector.show(ui, self.editor),
            Tab::Environment => crate::panels::inspector::environment(ui, self.editor),
            Tab::Assets => {
                self.assets
                    .show(ui, self.editor, self.git.as_deref(), self.render_state)
            }
            Tab::Console => self.console.show(ui),
            Tab::Profiler => self.profiler.show(ui, self.editor),
            Tab::Debug => crate::panels::debug::show(ui, self.editor),
            Tab::Effects => self.effects.show(ui, self.editor),
            Tab::Animation => self.animation.show(ui, self.editor),
            Tab::Config => self.config.show(ui, self.editor),
            Tab::Build => {
                let mut grid = self.viewport.options.snap.translate;
                self.build
                    .show(ui, self.editor, self.render_state, &mut grid);
                self.viewport.options.snap.translate = grid;
            }
            Tab::SourceControl => match self.git.as_deref_mut() {
                Some(git) => self.git_panel.show(ui, git, self.editor.project()),
                None => {
                    ui.spinner();
                }
            },
        }
    }

    fn closeable(&mut self, tab: &mut Tab) -> bool {
        *tab != Tab::Viewport
    }

    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        [false, false]
    }

    fn clear_background(&self, tab: &Tab) -> bool {
        *tab != Tab::Viewport
    }
}

impl EditorApp {
    /// Starts the git worker on the first frame, applies its answers and reports finished
    /// operations in the status bar.
    fn poll_git(&mut self, ctx: &egui::Context) {
        let git = self.git.get_or_insert_with(|| {
            let ctx = ctx.clone();
            let root = crate::panels::git::project_root(self.editor.project());
            GitClient::start(root, move || ctx.request_repaint())
        });
        git.poll();
        // The status bar shows the branch, so keep it roughly current even with the
        // Source Control panel closed.
        git.refresh_every(Duration::from_secs(5));
        for done in git.results.drain(..) {
            match done.result {
                Ok(message) => self.editor.info(message),
                Err(err) => {
                    let first = err.lines().next().unwrap_or_default().to_owned();
                    self.editor.fail(format!("{} failed: {first}", done.title));
                }
            }
        }
    }

    pub fn update(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        if let Some(err) = crate::preferences::apply(ctx, &self.settings) {
            self.editor
                .fail(tf("preferences.font_error", &[("error", &err)]));
            self.settings.ui_font = None;
        }
        if let Some(watch) = &mut self.watch {
            watch.poll(&mut self.editor);
            if watch.building().is_some() {
                ctx.request_repaint_after(Duration::from_millis(200));
            }
        }
        self.poll_git(ctx);
        if self.export.poll() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        // Scans run in the background; keep repainting until one lands.
        self.editor.assets.refresh();
        if self.editor.assets.is_scanning() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.poll_gamepads();
        let now = Instant::now();
        let delta = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;
        self.editor.tick(delta);

        let title = self.editor.title();
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_close
            && self.editor.modified
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.dialog = Some(Dialog::Unsaved(Pending::Quit));
        }
        if self.allow_close {
            self.allow_close = false;
            self.editor.modified = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let Some(path) = ctx.data_mut(|d| d.remove_temp::<String>(egui::Id::new(OPEN_REQUEST))) {
            self.guard(Pending::Open(path));
        }
        if self.dialog.is_none() {
            self.shortcuts(ctx);
        }

        if self.custom_frame {
            let document = self
                .editor
                .scene_path
                .clone()
                .unwrap_or_else(|| "Untitled".to_owned());
            let project = chrome::project_name(&self.editor.assets.root);
            let title = chrome::Title {
                document: &document,
                modified: self.editor.modified,
                project: Some(&project),
                playing: self.editor.is_playing(),
            };
            egui::Panel::top("titlebar")
                .exact_size(chrome::HEIGHT)
                .frame(
                    egui::Frame::NONE
                        .fill(theme::BG_0)
                        .inner_margin(egui::Margin {
                            left: 8,
                            ..Default::default()
                        }),
                )
                .show(ui, |ui| chrome::title_bar(ui, &title, |ui| self.menus(ui)));
        } else {
            egui::Panel::top("menu")
                .frame(
                    egui::Frame::NONE
                        .fill(theme::BG_0)
                        .inner_margin(egui::Margin::symmetric(8, 2)),
                )
                .show(ui, |ui| egui::MenuBar::new().ui(ui, |ui| self.menus(ui)));
        }
        egui::Panel::top("toolbar")
            .exact_size(32.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG_1)
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER)),
            )
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status")
            .exact_size(24.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG_0)
                    .inner_margin(egui::Margin::symmetric(10, 0))
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER)),
            )
            .show(ui, |ui| self.status_bar(ui));

        let render_state = frame.wgpu_render_state();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG_0)
                    .inner_margin(egui::Margin::same(3)),
            )
            .show(ui, |ui| {
                let mut style = egui_dock::Style::from_egui(ui.style());
                style.tab_bar.bg_fill = theme::BG_0;
                style.tab_bar.height = 24.0;
                style.tab.tab_body.inner_margin = egui::Margin::same(6);
                style.tab_bar.hline_color = theme::BORDER;
                style.tab.tab_body.bg_fill = theme::BG_1;
                style.tab.tab_body.stroke = egui::Stroke::new(1.0_f32, theme::BORDER);
                style.tab.active.bg_fill = theme::BG_1;
                style.tab.focused.bg_fill = theme::BG_1;
                style.tab.inactive.bg_fill = theme::BG_0;
                style.tab.hovered.bg_fill = theme::BG_2;
                style.tab.active.text_color = egui::Color32::WHITE;
                style.tab.focused.text_color = egui::Color32::WHITE;
                style.tab.inactive.text_color = theme::TEXT_DIM;
                style.tab.hovered.text_color = theme::TEXT;
                style.tab.focused.outline_color = theme::ACCENT_DIM;
                style.tab.hline_below_active_tab_name = true;
                style.separator.width = 2.0;
                style.separator.color_idle = theme::BG_0;
                style.separator.color_hovered = theme::ACCENT_DIM;
                style.separator.color_dragged = theme::ACCENT;
                let mut tabs = Tabs {
                    editor: &mut self.editor,
                    viewport: &mut self.viewport,
                    scene_tree: &mut self.scene_tree,
                    inspector: &mut self.inspector,
                    effects: &mut self.effects,
                    animation: &mut self.animation,
                    config: &mut self.config,
                    build: &mut self.build,
                    assets: &mut self.assets,
                    console: &mut self.console,
                    profiler: &mut self.profiler,
                    git_panel: &mut self.git_panel,
                    git: self.git.as_mut(),
                    render_state,
                };
                DockArea::new(&mut self.dock)
                    .style(style)
                    .show_add_buttons(false)
                    .show_leaf_collapse_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_inside(ui, &mut tabs);
            });

        let assets = self.editor.assets.root.clone();
        let unsaved = self.editor.modified && !self.editor.is_playing();
        if self.export.show(ctx, &assets, unsaved) {
            self.save_or_ask();
        }
        let project = self.editor.project().to_path_buf();
        self.assets.viewer.show(ctx, &project, render_state);
        self.dialogs(ctx);
        self.preferences
            .show(ctx, &mut self.settings, &mut self.viewport.options);
        for command in commands::take_requests(ctx) {
            self.execute(ctx, command, None);
        }
        if self.custom_frame {
            chrome::window_frame(ctx);
        }

        // One undo step per drag or text edit.
        let idle = ctx.input(|i| !i.pointer.any_down()) && !ctx.egui_wants_keyboard_input();
        if idle {
            self.editor.end_gesture();
            // Not while a step is being dragged: once, when it is let go.
            self.save_settings();
        }
        if !self.editor.is_playing() {
            self.viewport.set_cursor_grab(ctx, false);
        }
    }
}
