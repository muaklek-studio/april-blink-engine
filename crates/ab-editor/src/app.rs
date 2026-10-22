//! The editor window: menu bar, toolbar, dockable panels, status bar, dialogs and
//! keyboard shortcuts.

use crate::chrome;
use crate::console::LogBuffer;
use crate::git::{GitClient, RepoState};
use crate::panels::{
    assets::AssetsPanel, console::ConsolePanel, git::GitPanel, inspector::InspectorPanel,
    profiler::ProfilerPanel, scene_tree::SceneTreePanel,
};
use crate::project::AssetKind;
use crate::projects::{self, BuildState, Project, ScriptBuild};
use crate::state::{Editor, Mode};
use crate::theme;
use crate::viewport::{GizmoMode, Viewport};
use crate::Options;
use ab_engine::core::jobs::{self, Task};
use ab_engine::core::Vec3;
use ab_engine::scene_file::NodeDesc;
use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, RichText};
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
}

impl Tab {
    const ALL: [Tab; 11] = [
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
    ];

    fn title(self) -> String {
        let (glyph, name) = match self {
            Tab::Scene => (icon::TREE_STRUCTURE, "Scene"),
            Tab::Viewport => (icon::CUBE_FOCUS, "Viewport"),
            Tab::Inspector => (icon::SLIDERS_HORIZONTAL, "Inspector"),
            Tab::Environment => (icon::CLOUD_SUN, "Environment"),
            Tab::Assets => (icon::FOLDERS, "Assets"),
            Tab::Console => (icon::TERMINAL_WINDOW, "Console"),
            Tab::Profiler => (icon::CHART_LINE, "Profiler"),
            Tab::Debug => (icon::BUG_BEETLE, "Debug"),
            Tab::SourceControl => (icon::GIT_BRANCH, "Source Control"),
            Tab::Effects => (icon::SPARKLE, "Effects"),
            Tab::Animation => (icon::FILM_STRIP, "Animation"),
        };
        format!("{glyph}  {name}")
    }
}

fn default_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Viewport]);
    let surface = dock.main_surface_mut();
    let [center, _right] = surface.split_right(
        NodeIndex::root(),
        0.77,
        vec![Tab::Inspector, Tab::Environment, Tab::Effects, Tab::Debug],
    );
    let [center, _left] = surface.split_left(center, 0.22, vec![Tab::Scene, Tab::SourceControl]);
    let [_center, _bottom] = surface.split_below(
        center,
        0.70,
        vec![Tab::Assets, Tab::Animation, Tab::Console, Tab::Profiler],
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
    viewport: Viewport,
    dock: DockState<Tab>,
    scene_tree: SceneTreePanel,
    inspector: InspectorPanel,
    effects: crate::panels::effects::EffectsPanel,
    animation: crate::panels::animation::AnimationPanel,
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
        let project = std::fs::canonicalize(&project).unwrap_or(project);
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
        let mut viewport = Viewport::new(render_state);
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
                .find(|t| t.title().to_lowercase().ends_with(name.as_str()));
            if let Some(found) = tab.and_then(|t| dock.find_tab(&t)) {
                // Found just now, so the index is valid.
                _ = dock.set_active_tab(found);
            }
        }
        let mut export = crate::export::ExportWindow::default();
        if options.show.iter().any(|name| name == "export") {
            export.show_window();
        }
        Self {
            editor,
            viewport,
            dock,
            scene_tree: SceneTreePanel::default(),
            inspector: InspectorPanel::default(),
            effects: Default::default(),
            animation: Default::default(),
            assets: AssetsPanel::default(),
            console: ConsolePanel::new(logs),
            profiler: ProfilerPanel::default(),
            git_panel: GitPanel::default(),
            git: None,
            dialog: None,
            last_frame: Instant::now(),
            title: String::new(),
            allow_close: false,
            custom_frame: !options.system_titlebar,
            watch,
            close_project: false,
            export,
        }
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
                path: "scenes/new_scene.toml".into(),
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
        let shortcut = |m: Modifiers, k: Key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)))
        };
        // Play controls work everywhere, even while the game has the keyboard.
        if shortcut(Modifiers::SHIFT, Key::F5) {
            self.editor.stop();
            self.viewport.game_focus = false;
        }
        if shortcut(Modifiers::NONE, Key::F5) {
            self.toggle_play();
        }
        if shortcut(Modifiers::NONE, Key::F6) {
            self.editor.toggle_pause();
        }
        if shortcut(Modifiers::NONE, Key::F7) {
            self.editor.step_frame();
        }
        if self.viewport.game_focus && self.editor.is_playing() {
            return;
        }
        let ctrl = Modifiers::COMMAND;
        let ctrl_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        if shortcut(ctrl_shift, Key::S) {
            self.dialog = Some(Dialog::SaveAs {
                path: self
                    .editor
                    .scene_path
                    .clone()
                    .unwrap_or_else(|| "scenes/new_scene.toml".into()),
            });
        }
        if shortcut(ctrl_shift, Key::E) {
            self.export.show_window();
        }
        if shortcut(ctrl, Key::S) {
            self.save_or_ask();
        }
        if shortcut(ctrl, Key::N) {
            self.guard(Pending::New);
        }
        if shortcut(ctrl, Key::O) {
            self.dialog = Some(Dialog::Open {
                filter: String::new(),
            });
        }
        if shortcut(ctrl_shift, Key::Z) || shortcut(ctrl, Key::Y) {
            self.editor.redo();
        }
        if shortcut(ctrl, Key::Z) {
            self.editor.undo();
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if shortcut(ctrl, Key::D) {
            self.editor.duplicate_selected();
        }
        if shortcut(Modifiers::NONE, Key::Delete) {
            self.editor.delete_selected();
        }
        if shortcut(ctrl, Key::A) {
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
        if shortcut(Modifiers::NONE, Key::Escape) && self.editor.mode == crate::state::Mode::Edit {
            self.editor.select(None);
        }
        if shortcut(Modifiers::NONE, Key::F2) {
            self.scene_tree.start_rename(&self.editor);
        }
        if shortcut(Modifiers::NONE, Key::F1) {
            self.dialog = Some(Dialog::Shortcuts);
        }
        // Gizmo keys, unless flying the camera (WASD/QE).
        let flying = ctx.input(|i| i.pointer.secondary_down());
        if !flying {
            for (key, mode) in [
                (Key::Q, GizmoMode::Select),
                (Key::W, GizmoMode::Translate),
                (Key::E, GizmoMode::Rotate),
                (Key::R, GizmoMode::Scale),
            ] {
                if shortcut(Modifiers::NONE, key) {
                    self.viewport.options.gizmo = mode;
                }
            }
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
        ui.menu_button("File", |ui| {
            ui.set_min_width(220.0);
            if menu_item(ui, icon::FILE_PLUS, "New Scene", "Ctrl+N", editing) {
                self.guard(Pending::New);
            }
            if menu_item(ui, icon::FOLDER_OPEN, "Open Scene…", "Ctrl+O", editing) {
                self.dialog = Some(Dialog::Open {
                    filter: String::new(),
                });
            }
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
            if menu_item(ui, icon::FLOPPY_DISK, "Save", "Ctrl+S", editing) {
                self.save_or_ask();
            }
            if menu_item(
                ui,
                icon::FLOPPY_DISK_BACK,
                "Save As…",
                "Ctrl+Shift+S",
                editing,
            ) {
                self.dialog = Some(Dialog::SaveAs {
                    path: self
                        .editor
                        .scene_path
                        .clone()
                        .unwrap_or_else(|| "scenes/new_scene.toml".into()),
                });
            }
            ui.separator();
            if menu_item(ui, icon::PACKAGE, "Export Game…", "Ctrl+Shift+E", true) {
                self.export.show_window();
            }
            ui.separator();
            if self.watch.is_some() && menu_item(ui, icon::FOLDERS, "Close Project", "", editing) {
                self.guard(Pending::CloseProject);
            }
            if menu_item(ui, icon::SIGN_OUT, "Quit", "", true) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        ui.menu_button("Edit", |ui| {
            ui.set_min_width(220.0);
            let selection = self.editor.selection.filter(|_| editing);
            if menu_item(
                ui,
                icon::ARROW_U_UP_LEFT,
                "Undo",
                "Ctrl+Z",
                editing && self.editor.history.can_undo(),
            ) {
                self.editor.undo();
            }
            if menu_item(
                ui,
                icon::ARROW_U_UP_RIGHT,
                "Redo",
                "Ctrl+Y",
                editing && self.editor.history.can_redo(),
            ) {
                self.editor.redo();
            }
            ui.separator();
            if menu_item(ui, icon::PENCIL_SIMPLE, "Rename", "F2", selection.is_some()) {
                self.scene_tree.start_rename(&self.editor);
            }
            if menu_item(ui, icon::COPY, "Duplicate", "Ctrl+D", selection.is_some()) {
                self.editor.duplicate_selected();
            }
            if menu_item(ui, icon::TRASH, "Delete", "Del", selection.is_some()) {
                self.editor.delete_selected();
            }
            if menu_item(ui, icon::SELECTION_ALL, "Select All", "Ctrl+A", editing) {
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
            ui.separator();
            if menu_item(ui, icon::CROSSHAIR, "Frame Selection", "F", true) {
                self.viewport.frame_selection(&self.editor);
            }
        });
        ui.add_enabled_ui(editing, |ui| {
            ui.menu_button("Create", |ui| {
                let at = self.viewport.camera.pivot;
                crate::create::menu(ui, &mut self.editor, at);
            });
        });
        ui.menu_button("View", |ui| {
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
            let options = &mut self.viewport.options;
            ui.checkbox(&mut options.grid, "Grid");
            ui.checkbox(&mut options.icons, "Node icons");
            ui.checkbox(&mut options.collision, "Collision shapes");
            ui.checkbox(&mut options.ui, "Edit UI in the view");
            ui.checkbox(&mut options.stats, "Statistics");
            ui.checkbox(&mut options.editor_lighting, "Editor lighting");
            ui.checkbox(&mut self.editor.preview_effects, "Preview effects");
            ui.separator();
            if ui
                .button(format!("{}  Reset Layout", icon::LAYOUT))
                .clicked()
            {
                self.dock = default_layout();
                ui.close();
            }
        });
        ui.menu_button("Play", |ui| {
            ui.set_min_width(220.0);
            let playing = self.editor.is_playing();
            if menu_item(ui, icon::PLAY, "Play", "F5", !playing) {
                self.toggle_play();
            }
            let paused = matches!(self.editor.mode, Mode::Play { paused: true });
            if menu_item(
                ui,
                icon::PAUSE,
                if paused { "Resume" } else { "Pause" },
                "F6",
                playing,
            ) {
                self.editor.toggle_pause();
            }
            if menu_item(ui, icon::STOP, "Stop", "Shift+F5", playing) {
                self.editor.stop();
            }
            let paused = matches!(self.editor.mode, Mode::Play { paused: true });
            if menu_item(ui, icon::SKIP_FORWARD, "Step Frame", "F7", paused) {
                self.editor.step_frame();
            }
        });
        ui.menu_button("Help", |ui| {
            ui.set_min_width(200.0);
            if menu_item(ui, icon::KEYBOARD, "Keyboard Shortcuts", "F1", true) {
                self.dialog = Some(Dialog::Shortcuts);
            }
            if menu_item(ui, icon::INFO, "About", "", true) {
                self.dialog = Some(Dialog::About);
            }
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let editing = !self.editor.is_playing();
            if tool(ui, icon::FILE_PLUS, "New scene (Ctrl+N)", editing) {
                self.guard(Pending::New);
            }
            if tool(ui, icon::FOLDER_OPEN, "Open scene (Ctrl+O)", editing) {
                self.dialog = Some(Dialog::Open {
                    filter: String::new(),
                });
            }
            if tool(ui, icon::FLOPPY_DISK, "Save (Ctrl+S)", editing) {
                self.save_or_ask();
            }
            ui.separator();
            if tool(
                ui,
                icon::ARROW_U_UP_LEFT,
                "Undo (Ctrl+Z)",
                editing && self.editor.history.can_undo(),
            ) {
                self.editor.undo();
            }
            if tool(
                ui,
                icon::ARROW_U_UP_RIGHT,
                "Redo (Ctrl+Y)",
                editing && self.editor.history.can_redo(),
            ) {
                self.editor.redo();
            }
            ui.separator();
            for (mode, glyph, tip) in [
                (GizmoMode::Select, icon::CURSOR, "Select (Q)"),
                (GizmoMode::Translate, icon::ARROWS_OUT_CARDINAL, "Move (W)"),
                (GizmoMode::Rotate, icon::ARROWS_CLOCKWISE, "Rotate (E)"),
                (GizmoMode::Scale, icon::ARROWS_OUT, "Scale (R)"),
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
                    ui.label(RichText::new("Ready").color(theme::TEXT_DIM));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let (mode, color) = match self.editor.mode {
                    Mode::Edit => ("EDIT", theme::ACCENT),
                    Mode::Play { paused: false } => ("PLAYING", theme::PLAY),
                    Mode::Play { paused: true } => ("PAUSED", theme::WARN),
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
            ui.set_width(440.0);
            match &mut dialog {
                Dialog::Open { filter } => {
                    ui.heading(format!("{}  Open Scene", icon::FOLDER_OPEN));
                    ui.add_space(4.0);
                    let edit = ui.add(
                        egui::TextEdit::singleline(filter)
                            .hint_text(format!("{}  Search scenes", icon::MAGNIFYING_GLASS))
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
                            crate::panels::empty_hint(ui, icon::FILM_SLATE, "No scene files in the project");
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
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::SaveAs { path } => {
                    ui.heading(format!("{}  Save Scene As", icon::FLOPPY_DISK_BACK));
                    ui.label(
                        RichText::new(format!("Path inside {}", self.editor.project().display()))
                            .color(theme::TEXT_DIM),
                    );
                    let edit = ui.add(egui::TextEdit::singleline(path).desired_width(f32::INFINITY));
                    edit.request_focus();
                    let valid = !path.trim().is_empty() && !path.contains("..");
                    let exists = self.editor.project().join(path.trim()).exists();
                    if exists {
                        ui.label(RichText::new(format!("{}  The file exists and will be replaced.", icon::WARNING)).color(theme::WARN));
                    }
                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let save = ui.add_enabled(valid, egui::Button::new(RichText::new("Save").strong()).fill(theme::ACCENT_DIM));
                        if save.clicked() || (valid && edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
                            self.editor.save_as(path);
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Unsaved(pending) => {
                    ui.heading(format!("{}  Unsaved Changes", icon::WARNING));
                    let scene = self.editor.scene_path.as_deref().unwrap_or("Untitled");
                    ui.label(format!("Save the changes to {scene} first?"));
                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(RichText::new("Save").strong()).fill(theme::ACCENT_DIM)).clicked() {
                            if self.editor.scene_path.is_some() {
                                self.editor.save();
                                if !self.editor.modified {
                                    then = Some(pending.clone());
                                }
                            } else {
                                self.dialog = Some(Dialog::SaveAs { path: "scenes/new_scene.toml".into() });
                            }
                            keep = false;
                        }
                        if ui.button(RichText::new("Don't Save").color(theme::ERROR)).clicked() {
                            then = Some(pending.clone());
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Shortcuts => {
                    ui.heading(format!("{}  Keyboard Shortcuts", icon::KEYBOARD));
                    egui::Grid::new("shortcuts").num_columns(2).spacing([24.0, 6.0]).striped(true).show(ui, |ui| {
                        for (keys, what) in [
                            ("F5 / Shift+F5 / F6", "Play / Stop / Pause"),
                            ("F7 (paused)", "Step one frame"),
                            ("F3 / F4 (playing)", "Stats overlay / physics view"),
                            ("F8 / F9 (playing)", "Input view / damage and object trace"),
                            ("Ctrl+S, Ctrl+Shift+S", "Save, Save As"),
                            ("Ctrl+Shift+E", "Export the game (Windows / Linux)"),
                            ("Ctrl+N, Ctrl+O", "New scene, Open scene"),
                            ("Ctrl+Z, Ctrl+Y", "Undo, Redo"),
                            ("Ctrl+D, Del, F2", "Duplicate, Delete, Rename"),
                            ("Q W E R", "Select, Move, Rotate, Scale"),
                            ("Ctrl+click, Shift+click", "Add to / remove from the selection"),
                            ("Drag on empty space", "Box select (Shift/Ctrl: add)"),
                            ("Ctrl+A, Esc", "Select all, Select none"),
                            ("Hold Ctrl while dragging", "Snap (0.5 m, 15°, 0.1)"),
                            ("F / double-click in tree", "Frame selection"),
                            ("Right mouse + WASD/QE", "Fly (Shift: faster, wheel: speed)"),
                            ("Middle mouse / Alt+Left", "Orbit"),
                            ("Shift+Middle mouse", "Pan"),
                            ("Mouse wheel", "Zoom"),
                            ("Esc (playing)", "Release the mouse"),
                        ] {
                            ui.label(RichText::new(keys).monospace().color(theme::ACCENT));
                            ui.label(what);
                            ui.end_row();
                        }
                    });
                    ui.separator();
                    if ui.button("Close").clicked() {
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
                        if ui.button("Close").clicked() {
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
    viewport: &'a mut Viewport,
    scene_tree: &'a mut SceneTreePanel,
    inspector: &'a mut InspectorPanel,
    effects: &'a mut crate::panels::effects::EffectsPanel,
    animation: &'a mut crate::panels::animation::AnimationPanel,
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
            Tab::Assets => self.assets.show(ui, self.editor, self.git.as_deref()),
            Tab::Console => self.console.show(ui),
            Tab::Profiler => self.profiler.show(ui, self.editor),
            Tab::Debug => crate::panels::debug::show(ui, self.editor),
            Tab::Effects => self.effects.show(ui, self.editor),
            Tab::Animation => self.animation.show(ui, self.editor),
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
            .exact_size(22.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG_0)
                    .inner_margin(egui::Margin::symmetric(8, 0)),
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
        self.dialogs(ctx);
        if self.custom_frame {
            chrome::window_frame(ctx);
        }

        // One undo step per drag or text edit.
        let idle = ctx.input(|i| !i.pointer.any_down()) && !ctx.egui_wants_keyboard_input();
        if idle {
            self.editor.end_gesture();
        }
        if !self.editor.is_playing() {
            self.viewport.set_cursor_grab(ctx, false);
        }
    }
}
