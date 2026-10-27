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

/// The default layout's panel groups: the right side, the left side, below the viewport.
/// Only the panels used all the time: each feature (animation, effects, the game's config,
/// building levels, profiling…) has a window of its own (see [`Tab::is_feature`]).
const RIGHT: &[Tab] = &[Tab::Inspector, Tab::Environment];
const LEFT: &[Tab] = &[Tab::Scene];
const BOTTOM: &[Tab] = &[Tab::Assets, Tab::Console];

impl Tab {
    /// A feature's editor: shown in a window of its own (from the Window or Settings menu,
    /// or by double-clicking its node), never docked in the main window.
    fn is_feature(self) -> bool {
        matches!(
            self,
            Tab::Animation
                | Tab::Effects
                | Tab::Config
                | Tab::Build
                | Tab::Profiler
                | Tab::Debug
                | Tab::SourceControl
        )
    }

    /// The panels it shares a place with in the default layout.
    fn group(self) -> &'static [Tab] {
        [RIGHT, LEFT, BOTTOM]
            .into_iter()
            .find(|group| group.contains(&self))
            .unwrap_or(&[])
    }

    /// Size of the panel's own window.
    fn window_size(self) -> [f32; 2] {
        match self {
            Tab::Animation => [1360.0, 820.0],
            Tab::Assets => [1100.0, 700.0],
            Tab::Console | Tab::Profiler | Tab::Build => [960.0, 560.0],
            Tab::SourceControl | Tab::Effects => [760.0, 720.0],
            _ => [460.0, 720.0],
        }
    }
}

fn default_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Viewport]);
    let surface = dock.main_surface_mut();
    let [center, _right] = surface.split_right(NodeIndex::root(), 0.77, RIGHT.to_vec());
    let [center, _left] = surface.split_left(center, 0.22, LEFT.to_vec());
    let [_center, _bottom] = surface.split_below(center, 0.70, BOTTOM.to_vec());
    dock
}

/// A Window menu entry: the window, its command (if it has one), else its title's key.
type MenuTool = (ToolWindow, Option<Command>, &'static str);

/// A tool in a native window of its own (see [`crate::tool_window`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolWindow {
    /// A panel taken out of the main window. The Animation panel's window is the animation
    /// workspace: a preview view, the scene tree and the inspector around the timeline.
    Panel(Tab),
    /// The UI Designer (only in a window).
    UiDesigner,
    /// The Git client: graph, branches, merges, diffs (only in a window).
    Git,
    /// The Cutscene Editor: a view through the shot over the cutscene's timeline (only in a
    /// window).
    Cutscene,
    /// The Animation Graph: a state machine's states and transitions (only in a window).
    AnimationGraph,
    /// The Rigging editor: a character's skeleton over a view of it (only in a window).
    Rigging,
    /// The Audio Mixer: buses, effects, snapshots (only in a window).
    AudioMixer,
    /// The Sound Event editor: a `.absound` asset's clips and settings (only in a window).
    SoundEvent,
}

impl ToolWindow {
    /// Its name in the settings and for `--show window:<name>`.
    fn key(self) -> &'static str {
        match self {
            ToolWindow::Panel(tab) => tab.name(),
            ToolWindow::UiDesigner => "ui_designer",
            ToolWindow::Git => "git",
            ToolWindow::Cutscene => "cutscene_editor",
            ToolWindow::AnimationGraph => "animation_graph",
            ToolWindow::Rigging => "rigging",
            ToolWindow::AudioMixer => "audio_mixer",
            ToolWindow::SoundEvent => "sound_event",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        if key == "ui_designer" {
            return Some(ToolWindow::UiDesigner);
        }
        if key == "git" {
            return Some(ToolWindow::Git);
        }
        if key == "cutscene_editor" {
            return Some(ToolWindow::Cutscene);
        }
        if key == "animation_graph" {
            return Some(ToolWindow::AnimationGraph);
        }
        if key == "rigging" {
            return Some(ToolWindow::Rigging);
        }
        if key == "audio_mixer" {
            return Some(ToolWindow::AudioMixer);
        }
        if key == "sound_event" {
            return Some(ToolWindow::SoundEvent);
        }
        Tab::ALL
            .into_iter()
            .find(|tab| tab.name() == key && *tab != Tab::Viewport)
            .map(ToolWindow::Panel)
    }

    fn spec(self) -> crate::tool_window::Spec {
        let (icon, title, size, min_size) = match self {
            ToolWindow::Panel(Tab::Animation) => (
                icon::FILM_STRIP,
                t("window.animation_editor"),
                Tab::Animation.window_size(),
                [900.0, 520.0],
            ),
            ToolWindow::Panel(Tab::Effects) => (
                icon::SPARKLE,
                t("window.effects_editor"),
                [1280.0, 800.0],
                [760.0, 480.0],
            ),
            ToolWindow::Panel(Tab::Config) => (
                icon::SLIDERS,
                t("window.game_config"),
                [620.0, 720.0],
                [420.0, 320.0],
            ),
            ToolWindow::Panel(Tab::Build) => (
                icon::CUBE,
                t("window.level_builder"),
                [960.0, 620.0],
                [520.0, 360.0],
            ),
            ToolWindow::Panel(Tab::Assets) => (
                icon::FOLDERS,
                t("window.asset_browser"),
                Tab::Assets.window_size(),
                [640.0, 400.0],
            ),
            ToolWindow::Panel(tab) => (
                tab.icon(),
                t(&format!("tab.{}", tab.name())),
                tab.window_size(),
                [320.0, 240.0],
            ),
            ToolWindow::UiDesigner => (
                icon::BROWSERS,
                t("window.ui_designer"),
                [1480.0, 900.0],
                [960.0, 560.0],
            ),
            ToolWindow::Git => (
                icon::GIT_BRANCH,
                t("window.git"),
                [1500.0, 920.0],
                [1000.0, 600.0],
            ),
            ToolWindow::Cutscene => (
                icon::FILM_SLATE,
                t("window.cutscene_editor"),
                [1400.0, 900.0],
                [900.0, 560.0],
            ),
            ToolWindow::AnimationGraph => (
                icon::GRAPH,
                t("window.animation_graph"),
                [1400.0, 860.0],
                [900.0, 520.0],
            ),
            ToolWindow::Rigging => (
                icon::PERSON_SIMPLE,
                t("window.rigging"),
                [1400.0, 900.0],
                [900.0, 560.0],
            ),
            ToolWindow::AudioMixer => (
                icon::FADERS,
                t("window.audio_mixer"),
                [1280.0, 860.0],
                [720.0, 520.0],
            ),
            ToolWindow::SoundEvent => (
                icon::SPEAKER_HIGH,
                t("window.sound_event"),
                [1280.0, 760.0],
                [860.0, 480.0],
            ),
        };
        crate::tool_window::Spec {
            key: self.key().to_owned(),
            icon,
            title,
            size,
            min_size,
            dockable: matches!(self, ToolWindow::Panel(tab) if !tab.is_feature()),
            remember: true,
            escape_closes: false,
        }
    }
}

/// What the selected AnimationPlayer animates: the lowest node holding it and all its
/// targets (a model instance, or the parent of a player and the nodes beside it).
fn animated_root(editor: &Editor) -> Option<ab_engine::scene::NodeHandle> {
    let tree = editor.tree();
    let selected = editor.selection?;
    let player = std::iter::successors(Some(selected), |&n| tree.parent(n))
        .chain(tree.subtree(selected))
        .find(|&n| {
            tree.component::<ab_engine::animation::AnimationPlayer>(n)
                .is_some()
        })?;
    let targets = tree
        .component::<ab_engine::animation::AnimationPlayer>(player)
        .map(|p| p.targets.clone())
        .unwrap_or_default();
    let holds_all = |n: ab_engine::scene::NodeHandle| {
        targets
            .iter()
            .all(|&t| std::iter::successors(Some(t), |&a| tree.parent(a)).any(|a| a == n))
    };
    let root = std::iter::successors(Some(player), |&n| tree.parent(n)).find(|&n| holds_all(n))?;
    // Holding everything in the scene: nothing to isolate.
    (root != editor.scene_root).then_some(root)
}

/// The animation workspace's own panels (the timeline is the Animation panel's).
#[derive(Default)]
struct AnimationWorkspace {
    /// A view of the scene with its own camera, made on first show.
    view: Option<crate::viewport::Viewport>,
    redraw: crate::tool_window::Redraw,
    scene_tree: SceneTreePanel,
    inspector: InspectorPanel,
    /// What is animated (see [`animated_root`]): kept while the selection moves inside it
    /// (to a bone, which is beside the player, not above or below it).
    subject: Option<ab_engine::scene::NodeHandle>,
}

impl AnimationWorkspace {
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        animation: &mut crate::panels::animation::AnimationPanel,
        editor: &mut Editor,
        main: &ViewportArea,
        render_state: Option<&egui_wgpu::RenderState>,
    ) {
        let view = self.view.get_or_insert_with(|| {
            let mut view = main.sharing(crate::viewport::ViewKind::Perspective);
            view.frame_selection(editor);
            view
        });
        // The animated model alone: the node holding the player and everything it moves.
        self.subject = match animated_root(editor) {
            Some(root) => Some(root),
            None => self.subject.filter(|&s| {
                let tree = editor.tree();
                tree.is_valid(s)
                    && editor.selection.is_none_or(|n| {
                        std::iter::successors(Some(n), |&a| tree.parent(a)).any(|a| a == s)
                    })
            }),
        };
        view.isolate = self.subject;
        self.scene_tree.root = self.subject;
        let focus = view.camera.pivot;
        // Every frame while in use or playing a clip; a few times a second otherwise.
        let active = crate::tool_window::in_use(ui.ctx()) || animation.is_playing();
        view.redraw = self.redraw.due(active, editor.revision());
        crate::tool_window::side(
            ui,
            "animation-scene",
            true,
            250.0,
            Tab::Scene.icon(),
            &t("tab.scene"),
            |ui| {
                self.scene_tree.show(ui, editor, focus);
            },
        );
        if std::mem::take(&mut self.scene_tree.frame_request) {
            view.frame_selection(editor);
        }
        crate::tool_window::side(
            ui,
            "animation-inspector",
            false,
            320.0,
            Tab::Inspector.icon(),
            &t("tab.inspector"),
            |ui| self.inspector.show(ui, editor),
        );
        egui::Panel::bottom("animation-timeline")
            .resizable(true)
            .default_size(340.0)
            .size_range(160.0..=900.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG_1)
                    .inner_margin(egui::Margin::same(6))
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER)),
            )
            .show(ui, |ui| animation.show(ui, editor));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::BG_0))
            .show(ui, |ui| {
                // The main view sets the camera the game is drawn with: keep it. The game's
                // interface would cover what is animated.
                keeping_camera(editor, |editor| {
                    crate::ui_designer::with_only_ui(editor, None, |editor| {
                        view.show(ui, editor, render_state)
                    })
                });
            });
    }
}

/// The Rigging editor's window: the bones on the left, a view with the skeleton drawn over
/// it, the bone's limits, the humanoid parts and the chains on the right.
#[derive(Default)]
struct RiggingWorkspace {
    view: Option<crate::viewport::Viewport>,
    redraw: crate::tool_window::Redraw,
    panel: crate::panels::rigging::RiggingPanel,
    /// The model the view was framed on.
    framed: Option<ab_engine::scene::NodeHandle>,
}

impl RiggingWorkspace {
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        main: &ViewportArea,
        render_state: Option<&egui_wgpu::RenderState>,
    ) {
        self.panel.follow(editor);
        let view = self
            .view
            .get_or_insert_with(|| main.sharing(crate::viewport::ViewKind::Perspective));
        if self.panel.model() != self.framed {
            self.framed = self.panel.model();
            view.frame_selection(editor);
        }
        let active = crate::tool_window::in_use(ui.ctx());
        view.redraw = self.redraw.due(active, editor.revision());
        crate::tool_window::toolbar(ui, "rigging", |ui| self.panel.toolbar(ui, editor));
        crate::tool_window::side(
            ui,
            "rigging-bones",
            true,
            260.0,
            icon::BONE,
            &t("rigging.bones"),
            |ui| {
                self.panel.bones_ui(ui, editor);
            },
        );
        crate::tool_window::side(
            ui,
            "rigging-details",
            false,
            340.0,
            icon::SLIDERS,
            &t("rigging.details"),
            |ui| {
                self.panel.details_ui(ui, editor);
            },
        );
        view.overlay = self.panel.overlay(editor);
        // The character alone, its bones not hidden behind selection boxes.
        view.isolate = self.panel.model();
        view.options.highlight_selection = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::BG_0))
            .show(ui, |ui| {
                keeping_camera(editor, |editor| view.show(ui, editor, render_state));
            });
    }
}

/// The Cutscene Editor's window: a view of the scene, through the cutscene's camera while
/// Look Through is on, above the timeline.
#[derive(Default)]
struct CutsceneWorkspace {
    /// A view with its own camera, made on first show.
    view: Option<crate::viewport::Viewport>,
    redraw: crate::tool_window::Redraw,
    panel: crate::panels::cutscene::CutscenePanel,
}

impl CutsceneWorkspace {
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        main: &ViewportArea,
        render_state: Option<&egui_wgpu::RenderState>,
    ) {
        let view = self
            .view
            .get_or_insert_with(|| main.sharing(crate::viewport::ViewKind::Perspective));
        let active = crate::tool_window::in_use(ui.ctx()) || self.panel.is_playing();
        view.redraw = self.redraw.due(active, editor.revision());
        // Key Camera records where the free view is.
        let free = (view.look_through.is_none()).then(|| (view.view_global(), view.camera.fov));
        egui::Panel::bottom("cutscene-timeline")
            .resizable(true)
            .default_size(300.0)
            .size_range(160.0..=800.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG_1)
                    .inner_margin(egui::Margin::same(6))
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER)),
            )
            .show(ui, |ui| self.panel.show(ui, editor, free));
        view.look_through = self.panel.preview_camera(editor);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::BG_0))
            .show(ui, |ui| {
                keeping_camera(editor, |editor| view.show(ui, editor, render_state));
            });
    }
}

/// The Effects Editor's window: a view of the effect, framed on it, beside the effect's
/// layers and settings.
#[derive(Default)]
struct EffectsWorkspace {
    /// A view with its own camera, made on first show.
    view: Option<crate::viewport::Viewport>,
    redraw: crate::tool_window::Redraw,
    /// The node the view was last framed on: a new effect is framed again.
    framed: Option<ab_engine::scene::NodeHandle>,
}

impl EffectsWorkspace {
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        effects: &mut crate::panels::effects::EffectsPanel,
        editor: &mut Editor,
        main: &ViewportArea,
        render_state: Option<&egui_wgpu::RenderState>,
    ) {
        let view = self
            .view
            .get_or_insert_with(|| main.sharing(crate::viewport::ViewKind::Perspective));
        // The effect the selection belongs to, framed when it changes.
        let effect = editor
            .selection
            .map(|n| crate::features::owner(editor, n).unwrap_or(n));
        if effect != self.framed {
            self.framed = effect;
            view.frame_selection(editor);
        }
        // The effect alone, not the scene around it.
        view.isolate = effect;
        // Particles move: drawn every frame while previewed.
        let active = crate::tool_window::in_use(ui.ctx()) || editor.preview_effects;
        view.redraw = self.redraw.due(active, editor.revision());
        crate::tool_window::side(
            ui,
            "effects-settings",
            false,
            440.0,
            Tab::Effects.icon(),
            &t("window.effects_editor"),
            // The panel scrolls its settings itself, under its fixed layer list.
            |ui| effects.show(ui, editor),
        );
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::BG_0))
            .show(ui, |ui| {
                keeping_camera(editor, |editor| {
                    crate::ui_designer::with_only_ui(editor, None, |editor| {
                        view.show(ui, editor, render_state)
                    })
                });
            });
    }
}

/// Runs `draw` (another view of the scene) and puts back the camera the main view chose.
pub(crate) fn keeping_camera(editor: &mut Editor, draw: impl FnOnce(&mut Editor)) {
    use ab_engine::rendering::CameraOverride;
    let saved = editor
        .tree_mut()
        .world_mut()
        .remove_resource::<CameraOverride>();
    draw(editor);
    let world = editor.tree_mut().world_mut();
    world.remove_resource::<CameraOverride>();
    if let Some(camera) = saved {
        world.insert_resource(camera);
    }
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
    Open {
        filter: String,
    },
    /// `then`: what Save in the Unsaved Changes dialog was for, done once saved.
    SaveAs {
        path: String,
        then: Option<Pending>,
    },
    Unsaved(Pending),
    Shortcuts,
    About,
}

const OPEN_REQUEST: &str = "ab-editor.open-request";

/// Most the decoded-image cache keeps (the oldest images go first).
const IMAGE_CACHE_BYTES: u64 = 2 << 30;

/// How often the editor wakes when nothing moves and no input comes.
const IDLE_POLL: Duration = Duration::from_millis(250);

/// Where Save As suggests saving a scene that has no file yet.
const NEW_SCENE_PATH: &str = "scenes/new_scene.abscene";

/// Asks the app to open a scene (from a panel), checking for unsaved changes first.
pub fn request_open(ctx: &egui::Context, path: String) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(OPEN_REQUEST), path));
}

/// Asks the editor to open a sound event (`.absound`) in the Sound Event editor.
pub fn request_sound_event(ctx: &egui::Context, path: String) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(SOUND_EVENT_REQUEST), path));
}

const SOUND_EVENT_REQUEST: &str = "ab-editor.sound-event-request";

/// An asset dropped into the scene (the view or the Scene tree): a model or scene becomes
/// an instance, a sound or sound event a 3D sound player, and a material dropped on an
/// object becomes its mesh's material.
pub fn drop_asset(
    editor: &mut Editor,
    path: &str,
    at: Option<Vec3>,
    target: Option<ab_engine::scene::NodeHandle>,
) {
    match AssetKind::of(std::path::Path::new(path)) {
        AssetKind::Material => {
            let tree = editor.tree();
            let mesh = target.and_then(|t| {
                tree.subtree(t)
                    .into_iter()
                    .find(|&n| tree.class_name(n) == Some("MeshInstance3D"))
            });
            let Some(mesh) = mesh.filter(|&m| editor.is_editable(m)) else {
                editor.fail(t("scene_tree.drop_material_on_mesh"));
                return;
            };
            let Some(old) = editor.describe_node(mesh) else {
                return;
            };
            let mut new = old.clone();
            new.material = Some(ab_engine::scene_file::MaterialDesc {
                file: Some(path.to_owned()),
                ..Default::default()
            });
            // Selected first: the selection follows the node if it is rebuilt.
            editor.select(Some(mesh));
            editor.apply_desc(mesh, &old, new);
        }
        AssetKind::Audio | AssetKind::SoundEvent => {
            editor.add_node(
                NodeDesc {
                    name: node_name(path),
                    kind: Some("AudioStreamPlayer3D".into()),
                    audio: Some(ab_engine::scene_file::AudioDesc {
                        stream: path.to_owned(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                at,
            );
        }
        _ => instance_asset(editor, path, at),
    }
}

/// A node name from a file's name (no path separators or dots).
fn node_name(path: &str) -> String {
    let stem = std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Instance".into());
    stem.chars()
        .map(|c| {
            if crate::panels::is_name_char(c) || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Adds a model or scene file to the scene as an instance.
pub fn instance_asset(editor: &mut Editor, path: &str, at: Option<Vec3>) {
    let name = node_name(path);
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
    /// When the open scene's file was last looked at for changes made outside the editor.
    disk_checked: Instant,
    title: String,
    allow_close: bool,
    /// The editor draws its own title bar (no `--system-titlebar`).
    custom_frame: bool,
    /// Rebuilds the game's scripts when their sources change (projects opened from the
    /// project manager; `cargo xtask editor` watches them itself).
    watch: Option<ScriptWatch>,
    /// Keeps the project's packed assets (in git) and its files in step.
    asset_sync: Option<crate::asset_sync::AssetSync>,
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
    /// `--language` for this run, and the saved language it stands in for: the override is
    /// never written to the settings.
    language_override: Option<(String, String)>,
    /// Edit → Preferences.
    preferences: crate::preferences::PreferencesWindow,
    /// Controllers, opened on first use (`Some(None)`: none can be read here).
    gamepads: Option<Option<ab_engine::platform::Gamepads>>,
    /// Tools open in native windows of their own, in the order they were opened.
    windows: Vec<ToolWindow>,
    ui_designer: crate::ui_designer::UiDesigner,
    animation_workspace: AnimationWorkspace,
    effects_workspace: EffectsWorkspace,
    cutscene_workspace: CutsceneWorkspace,
    state_graph: crate::panels::state_graph::StateGraphPanel,
    rigging_workspace: RiggingWorkspace,
    audio_mixer: crate::panels::audio_mixer::AudioMixerPanel,
    palette: crate::palette::Palette,
    sound_event: crate::panels::sound_event::SoundEventEditor,
    /// Help → About (the machine is read the first time it opens).
    about: crate::about::About,
    git_window: crate::git_window::GitWindow,
    /// Where each tool window opens: taken from the settings when it opens, then fixed.
    placements: std::collections::BTreeMap<String, crate::tool_window::Placement>,
    /// Windows moved or resized lately, kept in the settings once they stay put a moment
    /// (not on every frame of a drag).
    moved: std::collections::BTreeMap<String, crate::tool_window::Placement>,
    moved_at: Option<Instant>,
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
        // Screenshots and tests start from the defaults, not this machine's preferences, and
        // leave the project's folder alone.
        let settings_store = options.screenshot.is_none().then(EditorSettings::store);
        // A real project keeps its caches, logs and backups in its own data folder, and its
        // binary assets packed for git: unpacked now, before the first scene loads.
        let game = settings_store
            .as_ref()
            .and_then(|_| ab_project::Project::open(&project_dir).ok());
        let data = game.as_ref().map(ab_project::Project::data_dir);
        crate::console::log_to(data.as_ref().map(|d| d.join("logs")).as_deref());
        let asset_sync = game
            .clone()
            .map(|game| crate::asset_sync::AssetSync::start(game, &mut editor));
        let images = data.as_ref().map(|d| d.join("cache/images"));
        ab_engine::asset::set_decode_cache(images.clone());
        if let Some(images) = images {
            let trim = std::thread::Builder::new()
                .name("ab-cache-trim".into())
                .spawn(move || {
                    let freed = crate::projects::trim_cache(&images, IMAGE_CACHE_BYTES);
                    if freed > 0 {
                        tracing::info!(freed, "decoded-image cache trimmed");
                    }
                });
            if let Err(err) = trim {
                tracing::debug!("cache not trimmed: {err}");
            }
        }
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
        editor.backups = match (&settings_store, &data) {
            (Some(_), Some(data)) => Some(data.join("backups")),
            (Some(store), None) => Some(crate::state::backup_dir(store.dir(), &project_dir)),
            (None, _) => None,
        };
        let mut settings = settings_store
            .as_ref()
            .map(EditorSettings::load)
            .unwrap_or_default();
        let language_override = options.language.as_ref().map(|language| {
            let saved = std::mem::replace(&mut settings.language, language.clone());
            (language.clone(), saved)
        });
        viewport.options.snap = settings.snap;
        viewport.options.highlight_selection = settings.highlight_selection;
        viewport.options.highlight_hover = settings.highlight_hover;
        viewport.options.tips = settings.show_tips;
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
        // The tool windows open when the editor last closed, and `--show window:<name>`.
        let windows: Vec<ToolWindow> = settings
            .windows
            .iter()
            .map(String::as_str)
            .chain(
                options
                    .show
                    .iter()
                    .filter_map(|name| name.strip_prefix("window:")),
            )
            .filter_map(ToolWindow::from_key)
            // `--show sound_event:<asset>`: that event in its editor.
            .chain(
                options
                    .show
                    .iter()
                    .any(|name| name.starts_with("sound_event:"))
                    .then_some(ToolWindow::SoundEvent),
            )
            .fold(Vec::new(), |mut all, window| {
                if !all.contains(&window) {
                    all.push(window);
                }
                all
            });
        for window in &windows {
            if let ToolWindow::Panel(tab) = window {
                if let Some(found) = dock.find_tab(tab) {
                    dock.remove_tab(found);
                }
            }
        }
        let placements = settings.window_rects.clone();
        // `--show unsaved`: the Unsaved Changes dialog (screenshots of it).
        let dialog = options
            .show
            .iter()
            .any(|name| name == "unsaved")
            .then_some(Dialog::Unsaved(Pending::New))
            // `--show about`: Help → About (screenshots of it).
            .or_else(|| {
                options
                    .show
                    .iter()
                    .any(|name| name == "about")
                    .then_some(Dialog::About)
            });
        // `--show palette` / `--show palette:<query>`: the command palette (screenshots).
        let mut palette = crate::palette::Palette::default();
        if let Some(query) = options.show.iter().find_map(|name| {
            (name == "palette")
                .then_some("")
                .or_else(|| name.strip_prefix("palette:"))
        }) {
            palette.open_with(query);
        }
        let mut sound_event = crate::panels::sound_event::SoundEventEditor::default();
        if let Some(path) = options
            .show
            .iter()
            .find_map(|name| name.strip_prefix("sound_event:"))
        {
            sound_event.open(&editor, path);
        }
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
            // `--show memory`: the profiler's memory view (screenshots of it).
            profiler: {
                let mut panel = ProfilerPanel::default();
                panel.memory = options.show.iter().any(|name| name == "memory");
                panel
            },
            git_panel: GitPanel::default(),
            git: None,
            dialog,
            last_frame: Instant::now(),
            disk_checked: Instant::now(),
            title: String::new(),
            allow_close: false,
            custom_frame: !options.system_titlebar,
            watch,
            asset_sync,
            close_project: false,
            export,
            saved_settings: settings.clone(),
            settings,
            settings_store,
            language_override,
            preferences: Default::default(),
            gamepads: None,
            windows,
            ui_designer: Default::default(),
            animation_workspace: Default::default(),
            effects_workspace: Default::default(),
            cutscene_workspace: Default::default(),
            state_graph: Default::default(),
            rigging_workspace: Default::default(),
            audio_mixer: Default::default(),
            palette,
            sound_event,
            about: crate::about::About::new(render_state.map(|s| s.adapter.get_info())),
            git_window: Default::default(),
            placements,
            moved: Default::default(),
            moved_at: None,
        }
    }

    // =========================================================================================
    // Tool windows
    // =========================================================================================

    /// Opens `window`, or raises it when it is open already. A panel leaves the main window.
    fn open_window(&mut self, ctx: &egui::Context, window: ToolWindow) {
        if self.windows.contains(&window) {
            crate::tool_window::focus(ctx, window.key());
            return;
        }
        if let ToolWindow::Panel(tab) = window {
            if tab == Tab::Viewport {
                return;
            }
            if let Some(found) = self.dock.find_tab(&tab) {
                self.dock.remove_tab(found);
            }
        }
        self.place(window.key());
        self.windows.push(window);
    }

    /// The window `key` opens where it was last.
    fn place(&mut self, key: &str) {
        let last = self
            .moved
            .get(key)
            .or_else(|| self.settings.window_rects.get(key));
        if let Some(rect) = last.copied() {
            self.placements.insert(key.to_owned(), rect);
        }
    }

    /// Takes where the tool windows are now; keeps it once they have stayed put a second.
    fn track_placements(&mut self, rects: Vec<(String, crate::tool_window::Placement)>) {
        for (key, rect) in rects {
            let kept = self
                .moved
                .get(&key)
                .or_else(|| self.settings.window_rects.get(&key));
            if kept != Some(&rect) {
                self.moved.insert(key, rect);
                self.moved_at = Some(Instant::now());
            }
        }
        if self
            .moved_at
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(1))
        {
            let moved = std::mem::take(&mut self.moved);
            self.settings.window_rects.extend(moved);
            self.moved_at = None;
        }
    }

    fn close_window(&mut self, window: ToolWindow) {
        self.windows.retain(|w| *w != window);
        if window == ToolWindow::Panel(Tab::Animation) {
            // Its preview poses the scene: not once nobody sees it.
            self.animation.end_preview(&mut self.editor);
        }
    }

    /// Puts a panel's window back into the main window, with the panels it was with.
    fn dock_window(&mut self, window: ToolWindow) {
        if matches!(window, ToolWindow::Panel(tab) if tab.is_feature()) {
            // A feature stays in its own window.
            return;
        }
        self.close_window(window);
        if let ToolWindow::Panel(tab) = window {
            self.dock_tab(tab);
        }
    }

    /// Opens what the scene tree (or Create) asked for: a UI node in the UI Editor, an
    /// AnimationPlayer in the Animation Editor, an effect in the Effects Editor.
    fn open_requested(&mut self, ctx: &egui::Context) {
        use crate::features::Feature;
        let Some(node) = self.editor.open_request.take() else {
            return;
        };
        match Feature::of(&self.editor, node) {
            Some(Feature::Ui) => {
                self.ui_designer.edit(&mut self.editor, node);
                self.open_window(ctx, ToolWindow::UiDesigner);
            }
            Some(Feature::Animation) => {
                self.editor.select(Some(node));
                self.open_window(ctx, ToolWindow::Panel(Tab::Animation));
            }
            Some(Feature::Effect) => {
                self.editor.select(Some(node));
                self.open_window(ctx, ToolWindow::Panel(Tab::Effects));
            }
            Some(Feature::Cutscene) => {
                self.editor.select(Some(node));
                self.open_window(ctx, ToolWindow::Cutscene);
            }
            Some(Feature::AnimationGraph) => {
                self.editor.select(Some(node));
                self.open_window(ctx, ToolWindow::AnimationGraph);
            }
            None => {}
        }
    }

    fn dock_tab(&mut self, tab: Tab) {
        if tab.is_feature() {
            // Features open in their own window.
            let window = ToolWindow::Panel(tab);
            if !self.windows.contains(&window) {
                self.place(window.key());
                self.windows.push(window);
            }
            return;
        }
        if let Some(found) = self.dock.find_tab(&tab) {
            // Found just now, so the index is valid.
            _ = self.dock.set_active_tab(found);
            return;
        }
        let sibling = tab
            .group()
            .iter()
            .filter(|t| **t != tab)
            .find_map(|t| self.dock.find_tab(t));
        match sibling.and_then(|found| self.dock.leaf_mut(found.node_path()).ok()) {
            Some(leaf) => leaf.append_tab(tab),
            None => self.dock.push_to_focused_leaf(tab),
        }
    }

    /// Shows a panel wherever it is: raises its window, or makes it the active tab (put
    /// back in the main window when it was closed).
    fn reveal_panel(&mut self, ctx: &egui::Context, tab: Tab) {
        if self.windows.contains(&ToolWindow::Panel(tab)) {
            crate::tool_window::focus(ctx, tab.name());
        } else {
            self.dock_tab(tab);
        }
    }

    /// The dock's panels, for the main window or a tool window.
    fn tabs<'a>(&'a mut self, render_state: Option<&'a egui_wgpu::RenderState>) -> Tabs<'a> {
        Tabs {
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
            pop_out: None,
        }
    }

    /// Shows the tool windows and the asset windows, and does what they asked.
    fn show_windows(
        &mut self,
        ctx: &egui::Context,
        host: &mut crate::tool_window::Host,
        render_state: Option<&egui_wgpu::RenderState>,
    ) {
        let mut closed = Vec::new();
        let mut docked = Vec::new();
        for window in self.windows.clone() {
            let spec = window.spec();
            let response = match window {
                ToolWindow::UiDesigner => {
                    let (designer, editor, viewport) =
                        (&mut self.ui_designer, &mut self.editor, &self.viewport);
                    host.show(ctx, &spec, |ui| {
                        designer.show(ui, editor, render_state, || viewport.share_renderer());
                    })
                }
                ToolWindow::Git => {
                    let (window, git) = (&mut self.git_window, self.git.as_mut());
                    let project = self.editor.project().to_path_buf();
                    host.show(ctx, &spec, |ui| window.show(ui, git, &project))
                }
                ToolWindow::Cutscene => {
                    let (workspace, editor, viewport) = (
                        &mut self.cutscene_workspace,
                        &mut self.editor,
                        &self.viewport,
                    );
                    host.show(ctx, &spec, |ui| {
                        workspace.show(ui, editor, viewport, render_state)
                    })
                }
                ToolWindow::AnimationGraph => {
                    let (graph, editor) = (&mut self.state_graph, &mut self.editor);
                    host.show(ctx, &spec, |ui| {
                        egui::Frame::NONE
                            .inner_margin(egui::Margin::same(6))
                            .show(ui, |ui| {
                                ui.set_min_size(ui.available_size());
                                graph.show(ui, editor);
                            });
                    })
                }
                ToolWindow::Rigging => {
                    let (workspace, editor, viewport) = (
                        &mut self.rigging_workspace,
                        &mut self.editor,
                        &self.viewport,
                    );
                    host.show(ctx, &spec, |ui| {
                        workspace.show(ui, editor, viewport, render_state)
                    })
                }
                ToolWindow::AudioMixer => {
                    let (mixer, editor) = (&mut self.audio_mixer, &mut self.editor);
                    host.show(ctx, &spec, |ui| {
                        egui::Frame::NONE
                            .inner_margin(egui::Margin::same(6))
                            .show(ui, |ui| {
                                ui.set_min_size(ui.available_size());
                                mixer.show(ui, editor);
                            });
                    })
                }
                ToolWindow::SoundEvent => {
                    let (event, editor) = (&mut self.sound_event, &mut self.editor);
                    host.show(ctx, &spec, |ui| {
                        egui::Frame::NONE
                            .inner_margin(egui::Margin::same(6))
                            .show(ui, |ui| {
                                ui.set_min_size(ui.available_size());
                                event.show(ui, editor);
                            });
                    })
                }
                ToolWindow::Panel(Tab::Animation) => {
                    let (workspace, animation, editor, viewport) = (
                        &mut self.animation_workspace,
                        &mut self.animation,
                        &mut self.editor,
                        &self.viewport,
                    );
                    host.show(ctx, &spec, |ui| {
                        workspace.show(ui, animation, editor, viewport, render_state);
                    })
                }
                ToolWindow::Panel(Tab::Effects) => {
                    let (workspace, effects, editor, viewport) = (
                        &mut self.effects_workspace,
                        &mut self.effects,
                        &mut self.editor,
                        &self.viewport,
                    );
                    host.show(ctx, &spec, |ui| {
                        workspace.show(ui, effects, editor, viewport, render_state);
                    })
                }
                ToolWindow::Panel(mut tab) => {
                    let mut tabs = self.tabs(render_state);
                    host.show(ctx, &spec, |ui| {
                        egui::Frame::NONE
                            .inner_margin(egui::Margin::same(6))
                            .show(ui, |ui| {
                                ui.set_min_size(ui.available_size());
                                tabs.ui(ui, &mut tab);
                            });
                    })
                }
            };
            if response.dock {
                docked.push(window);
            } else if response.close {
                closed.push(window);
            }
        }
        for window in closed {
            self.close_window(window);
        }
        for window in docked {
            self.dock_window(window);
        }

        let project = self.editor.project().to_path_buf();
        for action in self.assets.viewer.show(ctx, host, &project, render_state) {
            use crate::asset_viewer::ViewerAction;
            match action {
                ViewerAction::Reveal(path) => {
                    self.assets.reveal(&path);
                    self.reveal_panel(ctx, Tab::Assets);
                }
                ViewerAction::AddToScene(path) if !self.editor.is_playing() => {
                    let at = self.viewport.placement();
                    instance_asset(&mut self.editor, &path, Some(at));
                }
                ViewerAction::OpenScene(path) => self.guard(Pending::Open(path)),
                ViewerAction::AddToScene(_) => {}
            }
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
        self.settings.show_tips = options.tips;
        self.settings.windows = self.windows.iter().map(|w| w.key().to_owned()).collect();
        if self.settings == self.saved_settings {
            return;
        }
        if let Some(store) = &self.settings_store {
            // The engine folder may have been found since start-up: keep the newest.
            if self.settings.engine_root == self.saved_settings.engine_root {
                self.settings.engine_root = EditorSettings::load(store).engine_root;
            }
            match &self.language_override {
                // Still the run's `--language`: keep the saved choice on disk.
                Some((run, saved)) if self.settings.language == *run => {
                    let mut kept = self.settings.clone();
                    kept.language = saved.clone();
                    kept.save(store);
                }
                _ => self.settings.save(store),
            }
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
        self.animation.discard_changes();
        self.discard_config();
    }

    /// Forgets Game Config edits not saved (the file's values come back).
    fn discard_config(&mut self) {
        if self.editor.config_modified {
            let _ = self.editor.load_config();
            self.editor.config_modified = false;
        }
    }

    /// Whether anything would be lost by leaving the scene: its own changes, clip files
    /// keyed in the Animation panel, or Game Config values.
    fn has_unsaved(&self) -> bool {
        self.editor.modified || self.animation.has_unsaved() || self.editor.config_modified
    }

    /// Saves clip files and Game Config values with changes; whether they all were.
    fn save_side_files(&mut self) -> bool {
        let clips = self.animation.save_all(&mut self.editor);
        let config = !self.editor.config_modified || self.config.save_to_file(&mut self.editor);
        clips && config
    }

    /// Stops the game (back to the edited scene), so what follows sees the scene and its
    /// unsaved changes, not the game's state.
    fn stop_playing(&mut self) {
        if self.editor.is_playing() {
            self.editor.stop();
            self.viewport.game_focus = false;
        }
    }

    /// Runs `pending` now, or asks first when there are unsaved changes.
    fn guard(&mut self, pending: Pending) {
        // While playing, `modified` is about the edited scene: stop first, then ask.
        self.stop_playing();
        if self.has_unsaved() {
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
                self.viewport.frame_selection_everywhere(&self.editor);
            }
            Pending::Quit => self.allow_close = true,
            Pending::CloseProject => self.close_project = true,
        }
    }

    fn save_or_ask(&mut self) {
        if self.editor.is_playing() {
            self.editor.fail("Stop the game before saving");
            return;
        }
        // An animation preview poses nodes: save where they really are.
        self.animation.end_preview(&mut self.editor);
        self.save_side_files();
        if self.editor.scene_path.is_some() {
            self.editor.save();
        } else {
            self.dialog = Some(Dialog::SaveAs {
                path: NEW_SCENE_PATH.into(),
                then: None,
            });
        }
    }

    /// Saves as `path`; returns `then` to run when the save worked (a failed save keeps
    /// the changes, and the action waits).
    fn finish_save_as(&mut self, path: &str, then: Option<Pending>) -> Option<Pending> {
        self.animation.end_preview(&mut self.editor);
        self.editor.save_as(path);
        then.filter(|_| !self.editor.modified)
    }

    fn toggle_play(&mut self) {
        if self.editor.is_playing() {
            self.editor.stop();
            self.viewport.game_focus = false;
        } else {
            // Play snapshots the scene: not with an animation preview's pose in it.
            self.animation.end_preview(&mut self.editor);
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
        // With a key selected in the Animation panel, Delete removes the key, not the nodes.
        if command == Command::Delete && editing && self.animation.takes_delete() {
            self.animation.delete_selected_key();
            return;
        }
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
                    then: None,
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
            Command::FrameSelection => self.viewport.frame_selection_everywhere(&self.editor),
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
            Command::OpenUiDesigner => self.open_window(ctx, ToolWindow::UiDesigner),
            Command::OpenAnimationEditor => {
                self.open_window(ctx, ToolWindow::Panel(Tab::Animation));
            }
            Command::OpenAssetBrowser => self.open_window(ctx, ToolWindow::Panel(Tab::Assets)),
            Command::OpenGit => self.open_window(ctx, ToolWindow::Git),
            Command::OpenCutsceneEditor => self.open_window(ctx, ToolWindow::Cutscene),
            Command::OpenAnimationGraph => self.open_window(ctx, ToolWindow::AnimationGraph),
            Command::OpenRigging => self.open_window(ctx, ToolWindow::Rigging),
            Command::OpenAudioMixer => self.open_window(ctx, ToolWindow::AudioMixer),
            Command::OpenSoundEvent => self.open_window(ctx, ToolWindow::SoundEvent),
            Command::SearchEverything => self.palette.toggle(),
            Command::AddNode if editing => self.palette.open_with("+ "),
            Command::AddNode => {}
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
                format!(
                    "{}  {}",
                    icon::CLOCK_COUNTER_CLOCKWISE,
                    t("menu.open_recent")
                ),
                |ui| {
                    // Opened or saved lately (and still there), newest first.
                    let project = self.editor.project();
                    let scenes: Vec<String> = self
                        .settings
                        .recent_scenes(project)
                        .iter()
                        .filter(|s| project.join(s).is_file())
                        .cloned()
                        .collect();
                    if scenes.is_empty() {
                        ui.label(RichText::new(t("menu.no_recent")).color(theme::TEXT_DIM));
                    }
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
            // The scene root and the inside of instanced models cannot be cut, duplicated,
            // deleted or renamed: those commands are off when that is all there is.
            let selection = self.editor.selection.is_some() && editing;
            let root = self.editor.scene_root;
            let changeable = selection
                && self
                    .editor
                    .selected
                    .iter()
                    .any(|&n| n != root && self.editor.is_editable(n));
            let renamable = selection
                && self
                    .editor
                    .selection
                    .is_some_and(|n| n != root && self.editor.is_editable(n));
            commands::menu_item(ui, Command::Undo, editing && self.editor.history.can_undo());
            commands::menu_item(ui, Command::Redo, editing && self.editor.history.can_redo());
            ui.separator();
            commands::menu_item(ui, Command::Cut, changeable);
            commands::menu_item(ui, Command::Copy, changeable);
            commands::menu_item(ui, Command::Paste, self.editor.can_paste(None));
            commands::menu_item(ui, Command::Duplicate, changeable);
            commands::menu_item(ui, Command::Delete, changeable);
            commands::menu_item(ui, Command::Rename, renamable);
            ui.separator();
            commands::menu_item(ui, Command::SelectAll, editing);
            commands::menu_item(ui, Command::SelectNone, !self.editor.selected.is_empty());
            commands::menu_item(ui, Command::FrameSelection, true);
            ui.separator();
            ui.menu_button(format!("{}  {}", icon::MAGNET, t("menu.snapping")), |ui| {
                crate::viewport::snap_settings(ui, &mut self.viewport.options.snap);
            });
            ui.separator();
            // The game's settings and the editor's, where most editors keep them.
            let ctx = ui.ctx().clone();
            let config = ToolWindow::Panel(Tab::Config);
            if ui
                .add(
                    egui::Button::new(format!("{}  {}", icon::SLIDERS, t("window.game_config")))
                        .selected(self.windows.contains(&config)),
                )
                .on_hover_text(t("menu.game_config_hint"))
                .clicked()
            {
                self.open_window(&ctx, config);
                ui.close();
            }
            commands::menu_item(ui, Command::Preferences, true);
        });
        ui.add_enabled_ui(editing, |ui| {
            ui.menu_button(t("menu.create"), |ui| {
                let at = self.viewport.placement();
                crate::create::menu(ui, &mut self.editor, at);
            });
        });
        ui.menu_button(t("menu.view"), |ui| {
            ui.set_min_width(220.0);
            for tab in Tab::ALL.into_iter().filter(|t| !t.is_feature()) {
                let windowed = self.windows.contains(&ToolWindow::Panel(tab));
                let mut checked = windowed || self.dock.find_tab(&tab).is_some();
                if ui.checkbox(&mut checked, tab.title()).clicked() {
                    match self.dock.find_tab(&tab) {
                        _ if windowed && !checked => self.close_window(ToolWindow::Panel(tab)),
                        Some(found) if !checked && tab != Tab::Viewport => {
                            self.dock.remove_tab(found);
                        }
                        None if checked && !windowed => self.dock_tab(tab),
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
                format!("{} (H)", t("menu.highlight_selection")),
            );
            ui.checkbox(&mut options.highlight_hover, t("menu.highlight_hover"));
            ui.checkbox(&mut options.tips, t("menu.tips"));
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
                for window in std::mem::take(&mut self.windows) {
                    self.close_window(window);
                }
                ui.close();
            }
        });
        self.window_menu(ui);
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
            ui.set_min_width(220.0);
            commands::menu_item(ui, Command::SearchEverything, true);
            commands::menu_item(ui, Command::Shortcuts, true);
            if menu_item(ui, icon::INFO, &t("menu.about"), "", true) {
                self.dialog = Some(Dialog::About);
            }
        });
    }

    /// Window: the tools in windows of their own.
    fn window_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button(t("window.menu"), |ui| {
            ui.set_min_width(250.0);
            let ctx = ui.ctx().clone();
            // The tools, grouped by what they work on.
            let groups: [(&str, &[MenuTool]); 4] = [
                (
                    "window.group_scene",
                    &[
                        (
                            ToolWindow::Panel(Tab::Assets),
                            Some(Command::OpenAssetBrowser),
                            "",
                        ),
                        (ToolWindow::UiDesigner, Some(Command::OpenUiDesigner), ""),
                        (
                            ToolWindow::Panel(Tab::Effects),
                            None,
                            "window.effects_editor",
                        ),
                        (ToolWindow::Panel(Tab::Build), None, "window.level_builder"),
                    ],
                ),
                (
                    "window.group_animation",
                    &[
                        (
                            ToolWindow::Panel(Tab::Animation),
                            Some(Command::OpenAnimationEditor),
                            "",
                        ),
                        (
                            ToolWindow::AnimationGraph,
                            Some(Command::OpenAnimationGraph),
                            "",
                        ),
                        (ToolWindow::Rigging, Some(Command::OpenRigging), ""),
                        (ToolWindow::Cutscene, Some(Command::OpenCutsceneEditor), ""),
                    ],
                ),
                (
                    "window.group_audio",
                    &[
                        (ToolWindow::AudioMixer, Some(Command::OpenAudioMixer), ""),
                        (ToolWindow::SoundEvent, Some(Command::OpenSoundEvent), ""),
                    ],
                ),
                (
                    "window.group_project",
                    &[
                        (ToolWindow::Git, Some(Command::OpenGit), ""),
                        (ToolWindow::Panel(Tab::Profiler), None, "tab.profiler"),
                        (ToolWindow::Panel(Tab::Debug), None, "tab.debug"),
                    ],
                ),
            ];
            for (i, (heading, tools)) in groups.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                ui.label(RichText::new(t(heading)).small().color(theme::TEXT_DIM));
                for &(window, command, key) in tools.iter() {
                    let open = self.windows.contains(&window);
                    let (glyph, label, keys) = match command {
                        Some(c) => (c.icon(), c.label(), c.shortcut_text(&ctx)),
                        None => {
                            let ToolWindow::Panel(tab) = window else {
                                continue;
                            };
                            (tab.icon(), t(key), String::new())
                        }
                    };
                    let button = egui::Button::new(format!("{glyph}  {label}"))
                        .selected(open)
                        .shortcut_text(keys);
                    if ui.add(button).clicked() {
                        self.open_window(&ctx, window);
                        ui.close();
                    }
                }
            }
            ui.separator();
            ui.menu_button(
                format!("{}  {}", icon::ARROW_SQUARE_OUT, t("window.pop_out_panel")),
                |ui| {
                    for tab in Tab::ALL
                        .into_iter()
                        .filter(|t| *t != Tab::Viewport && !t.is_feature())
                    {
                        let open = self.windows.contains(&ToolWindow::Panel(tab));
                        if ui
                            .add(egui::Button::new(tab.title()).selected(open))
                            .clicked()
                        {
                            self.open_window(&ctx, ToolWindow::Panel(tab));
                            ui.close();
                        }
                    }
                },
            );
            ui.separator();
            let any = !self.windows.is_empty();
            if ui
                .add_enabled(
                    any,
                    egui::Button::new(format!(
                        "{}  {}",
                        icon::ARROW_SQUARE_IN,
                        t("window.dock_all")
                    )),
                )
                .clicked()
            {
                for window in self.windows.clone() {
                    self.dock_window(window);
                }
                ui.close();
            }
            if ui
                .add_enabled(
                    any || self.assets.viewer.is_open(),
                    egui::Button::new(format!("{}  {}", icon::X_SQUARE, t("window.close_all"))),
                )
                .clicked()
            {
                for window in std::mem::take(&mut self.windows) {
                    self.close_window(window);
                }
                self.assets.viewer.close_all();
                ui.close();
            }
            if !self.windows.is_empty() {
                ui.separator();
                ui.label(
                    RichText::new(t("window.open_now"))
                        .small()
                        .color(theme::TEXT_DIM),
                );
                for window in self.windows.clone() {
                    let spec = window.spec();
                    if ui
                        .button(format!("{}  {}", spec.icon, spec.title))
                        .clicked()
                    {
                        crate::tool_window::focus(&ctx, &spec.key);
                        ui.close();
                    }
                }
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

            // Find anything, and add nodes: on the right, always in sight.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let search = egui::Button::new(
                    RichText::new(format!(
                        "{}  {}   {}",
                        icon::MAGNIFYING_GLASS,
                        t("palette.button"),
                        Command::SearchEverything.shortcut_text(ui.ctx())
                    ))
                    .color(theme::TEXT_DIM),
                )
                .fill(theme::BG_0)
                .min_size(egui::vec2(240.0, 24.0));
                if ui.add(search).on_hover_text(t("palette.tooltip")).clicked() {
                    self.palette.toggle();
                }
                let add = egui::Button::new(RichText::new(format!(
                    "{}  {}",
                    icon::PLUS,
                    t("palette.add")
                )))
                .min_size(egui::vec2(0.0, 24.0));
                if ui
                    .add_enabled(editing, add)
                    .on_hover_text(Command::AddNode.tooltip(ui.ctx()))
                    .clicked()
                {
                    self.palette.open_with("+ ");
                }
            });
        });
    }

    /// Everything the command palette can find.
    fn palette_entries(&self, ctx: &egui::Context) -> Vec<crate::palette::Entry> {
        use crate::palette::{Action, Entry, Group};
        let editing = !self.editor.is_playing();
        let mut entries: Vec<Entry> = Command::ALL
            .into_iter()
            .filter(|c| *c != Command::SearchEverything)
            .map(|c| Entry {
                group: Group::Command,
                icon: c.icon(),
                label: c.label(),
                detail: c.shortcut_text(ctx),
                keywords: c.name().replace('_', " "),
                action: Action::Command(c),
            })
            .collect();
        // Windows that have no command of their own, and the panels.
        let windows = [
            (Tab::Effects, "window.effects_editor"),
            (Tab::Build, "window.level_builder"),
            (Tab::Config, "window.game_config"),
            (Tab::Profiler, "tab.profiler"),
            (Tab::Debug, "tab.debug"),
        ];
        for (tab, key) in windows {
            entries.push(Entry {
                group: Group::Command,
                icon: tab.icon(),
                label: t(key),
                detail: t("window.menu"),
                keywords: tab.name().replace('_', " "),
                action: Action::Window(tab.name().to_owned()),
            });
        }
        for tab in Tab::ALL
            .into_iter()
            .filter(|t| !t.is_feature() && *t != Tab::Viewport)
        {
            entries.push(Entry {
                group: Group::Command,
                icon: tab.icon(),
                label: t(&format!("tab.{}", tab.name())),
                detail: t("menu.view"),
                keywords: format!("{} panel", tab.name().replace('_', " ")),
                action: Action::Window(tab.name().to_owned()),
            });
        }
        if editing {
            for (i, c) in crate::create::catalog(&self.editor).into_iter().enumerate() {
                entries.push(Entry {
                    group: Group::Create,
                    icon: theme::class_icon(c.class),
                    label: c.label,
                    detail: c.category,
                    keywords: format!("add create new {}", c.class),
                    action: Action::Create(i),
                });
            }
            let tree = self.editor.tree();
            for node in tree.subtree(self.editor.scene_root) {
                if node == self.editor.scene_root {
                    continue;
                }
                let class = tree.class_name(node).unwrap_or("Node");
                entries.push(Entry {
                    group: Group::Node,
                    icon: theme::class_icon(class),
                    label: tree.name(node).map(|n| n.to_owned()).unwrap_or_default(),
                    detail: class.to_owned(),
                    keywords: tree.path_of(node).unwrap_or_default(),
                    action: Action::Node(node),
                });
            }
        }
        for asset in self
            .editor
            .assets
            .all()
            .filter(|a| a.kind != AssetKind::Folder)
        {
            entries.push(Entry {
                group: Group::Asset,
                icon: asset.kind.icon(),
                label: asset.name.clone(),
                detail: asset.path.clone(),
                keywords: String::new(),
                action: Action::Asset(asset.path.clone(), asset.kind),
            });
        }
        entries
    }

    /// Carries out what was picked in the command palette.
    fn run_palette(&mut self, ctx: &egui::Context, action: crate::palette::Action) {
        use crate::palette::Action;
        match action {
            Action::Command(command) => self.execute(ctx, command, None),
            Action::Window(key) => match ToolWindow::from_key(&key) {
                // A docked panel comes to the front where it is.
                Some(ToolWindow::Panel(tab)) if !tab.is_feature() => {
                    match self.dock.find_tab(&tab) {
                        Some(found) => _ = self.dock.set_active_tab(found),
                        None if !self.windows.contains(&ToolWindow::Panel(tab)) => {
                            self.dock_tab(tab)
                        }
                        None => self.open_window(ctx, ToolWindow::Panel(tab)),
                    }
                }
                Some(window) => self.open_window(ctx, window),
                None => {}
            },
            Action::Create(index) => {
                if let Some(item) = crate::create::catalog(&self.editor).into_iter().nth(index) {
                    let at = self.viewport.placement();
                    crate::create::add(&mut self.editor, item, at);
                }
            }
            Action::Asset(path, kind) => match kind {
                AssetKind::Scene => self.guard(Pending::Open(path)),
                AssetKind::Model => instance_asset(&mut self.editor, &path, None),
                AssetKind::SoundEvent => {
                    self.sound_event.open(&self.editor, &path);
                    self.open_window(ctx, ToolWindow::SoundEvent);
                }
                kind if crate::asset_viewer::can_view(kind) => {
                    let _ = self.assets.viewer.open(self.editor.project(), &path, kind);
                }
                _ => {
                    self.assets.reveal(&path);
                    match self.dock.find_tab(&Tab::Assets) {
                        Some(found) => _ = self.dock.set_active_tab(found),
                        None => self.open_window(ctx, ToolWindow::Panel(Tab::Assets)),
                    }
                }
            },
            Action::Node(node) => {
                self.editor.select(Some(node));
                self.viewport.frame_selection_everywhere(&self.editor);
            }
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            // The counters on the right first; the message gets what is left, cut with
            // "…" when long (all of it on hover).
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
                        .on_hover_text("Git: branch, unpushed/unpulled commits and changed files. Click for the Git window.")
                        .clicked();
                    if open {
                        self.open_window(ui.ctx(), ToolWindow::Git);
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
                    Some(_) => format!("{} {}", icon::SCROLL, t("status.scripts_failed")),
                    None => format!("{} {}", icon::SCROLL, t("status.no_scripts")),
                };
                ui.label(RichText::new(scripts).color(theme::TEXT_DIM))
                    .on_hover_text(t("status.scripts_hint"));
                ui.label(
                    RichText::new(format!(
                        "{} {}",
                        icon::TREE_STRUCTURE,
                        tf("status.nodes", &[("count", &self.editor.tree().node_count())])
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
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let (text, color) = match &self.editor.status {
                        Some(status) if status.error => (
                            format!("{}  {}", icon::X_CIRCLE, status.text),
                            theme::ERROR,
                        ),
                        Some(status) => (
                            format!("{}  {}", icon::CHECK_CIRCLE, status.text),
                            theme::TEXT_DIM,
                        ),
                        None => (t("status.ready"), theme::TEXT_DIM),
                    };
                    let label = ui.add(
                        egui::Label::new(RichText::new(&text).color(color)).truncate(),
                    );
                    if label.hovered() {
                        label.on_hover_text(text);
                    }
                });
            });
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let mut keep = true;
        let mut then: Option<Pending> = None;
        // Picked in the Open dialog: the current scene may still have unsaved changes.
        let mut open: Option<Pending> = None;
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
                            .hint_text(format!(
                                "{}  {}",
                                icon::MAGNIFYING_GLASS,
                                t("dialog.search_scenes")
                            ))
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
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            if scenes.is_empty() {
                                crate::panels::empty_hint(
                                    ui,
                                    icon::FILM_SLATE,
                                    &t("dialog.no_scenes"),
                                );
                            }
                            for scene in &scenes {
                                let button =
                                    egui::Button::new(format!("{}  {scene}", icon::FILM_SLATE))
                                        .min_size(egui::vec2(ui.available_width(), 26.0))
                                        .frame(false);
                                if ui.add(button).clicked() {
                                    open = Some(Pending::Open(scene.clone()));
                                    keep = false;
                                }
                            }
                        });
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        if let Some(first) = scenes.first() {
                            open = Some(Pending::Open(first.clone()));
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
                Dialog::SaveAs { path, then: after } => {
                    ui.heading(format!(
                        "{}  {}",
                        icon::FLOPPY_DISK_BACK,
                        t("dialog.save_as_title")
                    ));
                    ui.label(
                        RichText::new(tf(
                            "dialog.path_inside",
                            &[("folder", &self.editor.project().display())],
                        ))
                        .color(theme::TEXT_DIM),
                    );
                    let edit =
                        ui.add(egui::TextEdit::singleline(path).desired_width(f32::INFINITY));
                    edit.request_focus();
                    let (valid, exists) = save_as_check(self.editor.project(), path);
                    if exists {
                        ui.label(
                            RichText::new(format!(
                                "{}  {}",
                                icon::WARNING,
                                t("dialog.replace_warning")
                            ))
                            .color(theme::WARN),
                        );
                    }
                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let save = ui.add_enabled(
                            valid,
                            egui::Button::new(RichText::new(t("dialog.save")).strong())
                                .fill(theme::ACCENT_DIM),
                        );
                        if save.clicked()
                            || (valid
                                && edit.lost_focus()
                                && ui.input(|i| i.key_pressed(Key::Enter)))
                        {
                            then = self.finish_save_as(path, after.take());
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
                            self.animation.end_preview(&mut self.editor);
                            let side_saved = self.save_side_files();
                            if !self.editor.modified {
                                // Only clip files or config values had changes.
                                if side_saved {
                                    then = Some(pending.clone());
                                }
                            } else if self.editor.scene_path.is_some() {
                                self.editor.save();
                                // A failed save keeps the changes (and says why).
                                if !self.has_unsaved() {
                                    then = Some(pending.clone());
                                }
                            } else {
                                // Saved under a name first, then what was asked.
                                self.dialog = Some(Dialog::SaveAs {
                                    path: NEW_SCENE_PATH.into(),
                                    then: Some(pending.clone()),
                                });
                            }
                            keep = false;
                        }
                        Some(UnsavedChoice::DontSave) => {
                            self.animation.discard_changes();
                            self.discard_config();
                            then = Some(pending.clone());
                            keep = false;
                        }
                        Some(UnsavedChoice::Cancel) => keep = false,
                        None => {}
                    }
                }
                Dialog::Shortcuts => {
                    ui.heading(format!(
                        "{}  {}",
                        icon::KEYBOARD,
                        t("dialog.shortcuts_title")
                    ));
                    // The keys come from the command table, so this list cannot go stale.
                    let mut rows: Vec<(String, String)> = Vec::new();
                    for binding in commands::BINDINGS {
                        let keys = ui.ctx().format_shortcut(&binding.shortcut);
                        match rows
                            .iter_mut()
                            .find(|(_, what)| *what == binding.command.label())
                        {
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
                        "close_window",
                        "viewer_close",
                        "ui_nudge",
                    ] {
                        rows.push((
                            t(&format!("shortcuts.{name}_keys")),
                            t(&format!("shortcuts.{name}")),
                        ));
                    }
                    egui::ScrollArea::vertical()
                        .max_height(ui.ctx().content_rect().height() * 0.6)
                        .show(ui, |ui| {
                            egui::Grid::new("shortcuts")
                                .num_columns(2)
                                .spacing([24.0, 6.0])
                                .striped(true)
                                .show(ui, |ui| {
                                    for (keys, what) in &rows {
                                        ui.label(
                                            RichText::new(keys).monospace().color(theme::ACCENT),
                                        );
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
                    ui.set_width(460.0);
                    if self.about.show(ui) {
                        keep = false;
                    }
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
        if let Some(pending) = open {
            self.guard(pending);
        }
    }
}

/// Whether Save As can save to `path` (inside the project), and whether that replaces a
/// file: the file [`Editor::save_as`] writes, with its `.abscene` extension.
fn save_as_check(project: &std::path::Path, path: &str) -> (bool, bool) {
    let path = path.trim().replace('\\', "/");
    let target = ab_engine::scene_file::with_scene_extension(&path);
    // The file needs a name: not a folder (`scenes/`) nor a hidden file the asset list skips.
    let named = path
        .rsplit('/')
        .next()
        .is_some_and(|name| !name.is_empty() && !name.starts_with('.'));
    let valid = named
        && !path.split('/').any(|part| part == "..")
        && !path.starts_with('/')
        && !std::path::Path::new(&path).is_absolute()
        && !path.contains(':');
    (valid, valid && project.join(target).exists())
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
    /// A tab's Open in New Window was chosen.
    pop_out: Option<Tab>,
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
                    self.viewport.frame_selection_everywhere(self.editor);
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

    fn context_menu(&mut self, ui: &mut egui::Ui, tab: &mut Tab, _path: egui_dock::NodePath) {
        if *tab == Tab::Viewport {
            return;
        }
        if ui
            .button(format!(
                "{}  {}",
                icon::ARROW_SQUARE_OUT,
                t("window.pop_out")
            ))
            .clicked()
        {
            self.pop_out = Some(*tab);
            ui.close();
        }
    }

    fn on_tab_button(&mut self, tab: &mut Tab, response: &egui::Response) {
        // A double click on a tab takes the panel out into a window of its own.
        if *tab != Tab::Viewport && response.double_clicked() {
            self.pop_out = Some(*tab);
        }
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
            let notify = move || ctx.request_repaint();
            match &self.asset_sync {
                Some(sync) => GitClient::start_with(root, notify, sync.git_hook()),
                None => GitClient::start(root, notify),
            }
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
        self.open_requested(ctx);
        if self.export.poll() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        // Scans run in the background; keep repainting until one lands.
        self.editor.assets.refresh();
        if self.editor.assets.is_scanning() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.poll_gamepads();
        if let Some(sync) = &mut self.asset_sync {
            sync.poll(&mut self.editor);
        }
        let now = Instant::now();
        let delta = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;
        self.editor.tick(delta);
        // The views redraw only while something moves; idle, a few frames a second still
        // pick up what happens off screen (scripts rebuilt, files changed on disk).
        ctx.request_repaint_after(IDLE_POLL);
        // Not while a dialog is open: the unsaved-changes prompt is about the scene as it is.
        if self.dialog.is_none() && self.disk_checked.elapsed() >= Duration::from_secs(1) {
            self.disk_checked = now;
            self.editor.check_scene_on_disk();
            self.editor.check_shaders_on_disk();
        }

        let title = self.editor.title();
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.has_unsaved()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            // Saving from the dialog must write the edited scene, not the game's state.
            self.stop_playing();
            self.dialog = Some(Dialog::Unsaved(Pending::Quit));
        }
        if self.allow_close {
            self.allow_close = false;
            self.editor.modified = false;
            self.animation.discard_changes();
            self.editor.config_modified = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let Some(path) = ctx.data_mut(|d| d.remove_temp::<String>(egui::Id::new(OPEN_REQUEST))) {
            self.guard(Pending::Open(path));
        }
        if let Some(path) =
            ctx.data_mut(|d| d.remove_temp::<String>(egui::Id::new(SOUND_EVENT_REQUEST)))
        {
            self.sound_event.open(&self.editor, &path);
            self.open_window(ctx, ToolWindow::SoundEvent);
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
        let mut pop_out = None;
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
                    pop_out: None,
                };
                DockArea::new(&mut self.dock)
                    .style(style)
                    .show_add_buttons(false)
                    .show_leaf_collapse_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_inside(ui, &mut tabs);
                pop_out = tabs.pop_out;
            });
        if let Some(tab) = pop_out {
            self.open_window(ctx, ToolWindow::Panel(tab));
        }

        let assets = self.editor.assets.root.clone();
        let unsaved = self.editor.modified && !self.editor.is_playing();
        if self.export.show(ctx, &assets, unsaved) {
            self.save_or_ask();
        }
        // The tool windows, after the main window's panels (they borrow the same state).
        let mut host =
            crate::tool_window::Host::new(&self.editor, self.custom_frame, self.placements.clone());
        self.show_windows(ctx, &mut host, render_state);
        self.track_placements(std::mem::take(&mut host.rects));
        if self.moved_at.is_some() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        if self.dialog.is_none() {
            for (command, text) in std::mem::take(&mut host.commands) {
                self.execute(ctx, command, text);
            }
        }
        self.dialogs(ctx);
        self.preferences
            .show(ctx, &mut self.settings, &mut self.viewport.options);
        for command in commands::take_requests(ctx) {
            self.execute(ctx, command, None);
        }
        if self.palette.is_open() {
            let entries = self.palette_entries(ctx);
            if let Some(action) = self.palette.show(ctx, &entries) {
                self.run_palette(ctx, action);
            }
        }
        if self.custom_frame {
            chrome::window_frame(ctx);
        }

        // One undo step per drag or text edit.
        let idle =
            ctx.input(|i| !i.pointer.any_down()) && !ctx.egui_wants_keyboard_input() && !host.busy;
        if let Some(scene) = &self.editor.scene_path {
            if self.settings.recent_scenes(self.editor.project()).first() != Some(scene) {
                let project = self.editor.project().to_path_buf();
                self.settings.add_recent_scene(&project, scene);
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> EditorApp {
        let project =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let options = Options {
            // Not this machine's preferences (as for screenshots).
            screenshot: Some(PathBuf::from("unused.png")),
            ..Default::default()
        };
        EditorApp::new(None, project, &options, LogBuffer::default(), None)
    }

    #[test]
    fn sounds_dropped_in_become_3d_players_and_materials_go_on_meshes() {
        let mut app = app();
        let editor = &mut app.editor;
        drop_asset(
            editor,
            "audio/thud.wav",
            Some(Vec3::new(1.0, 0.0, 2.0)),
            None,
        );
        let player = editor.selection.unwrap();
        assert_eq!(
            editor.tree().class_name(player),
            Some("AudioStreamPlayer3D")
        );
        assert_eq!(
            editor.tree().position(player),
            Some(Vec3::new(1.0, 0.0, 2.0))
        );
        let desc = editor.describe_node(player).unwrap();
        assert_eq!(desc.audio.unwrap().stream, "audio/thud.wav");

        // On nothing: refused with a message.
        drop_asset(editor, "materials/gold.abmat", None, None);
        assert!(editor.status.as_ref().is_some_and(|s| s.error));

        editor.select(None);
        editor.add_scene(crate::create::MESHES[0].scene(), Some(Vec3::ZERO));
        let cube = editor.selection.unwrap();
        drop_asset(editor, "materials/gold.abmat", None, Some(cube));
        let desc = editor.describe_node(editor.selection.unwrap()).unwrap();
        assert_eq!(
            desc.material.and_then(|m| m.file).as_deref(),
            Some("materials/gold.abmat")
        );
    }

    #[test]
    fn the_palette_finds_commands_nodes_to_add_files_and_scene_nodes() {
        let mut app = app();
        app.editor.assets.rescan();
        let ctx = egui::Context::default();
        let entries = app.palette_entries(&ctx);
        let find = |query: &str| {
            let mut palette = crate::palette::Palette::default();
            palette.open_with(query);
            palette.matches(&entries).first().map(|e| e.action.clone())
        };
        assert_eq!(
            find("> audio mixer"),
            Some(crate::palette::Action::Command(Command::OpenAudioMixer))
        );
        assert!(matches!(
            find("+ cube"),
            Some(crate::palette::Action::Create(_))
        ));
        assert!(matches!(
            find("/ thud.wav"),
            Some(crate::palette::Action::Asset(..))
        ));
        // Every node type to add is there and adds without an error.
        let created = crate::create::catalog(&app.editor).len();
        assert!(created > 30, "{created}");
        let ctx = egui::Context::default();
        app.run_palette(&ctx, find("+ omni light").unwrap());
        let added = app.editor.selection.unwrap();
        assert_eq!(app.editor.tree().class_name(added), Some("OmniLight3D"));
        app.run_palette(&ctx, crate::palette::Action::Node(app.editor.scene_root));
    }

    #[test]
    fn opening_a_scene_while_playing_still_asks_about_unsaved_changes() {
        let mut app = app();
        app.editor.add_node(
            NodeDesc {
                name: "Unsaved".into(),
                kind: Some("Node3D".into()),
                ..Default::default()
            },
            None,
        );
        assert!(app.editor.modified);
        app.editor.play();
        app.guard(Pending::Open("slice/level.abscene".into()));
        assert!(
            matches!(app.dialog, Some(Dialog::Unsaved(Pending::Open(_)))),
            "asked first"
        );
        assert!(!app.editor.is_playing(), "back to the edited scene");
        assert!(
            app.editor.node_at("Unsaved").is_some(),
            "the edit is still there"
        );
        assert_eq!(app.editor.scene_path, None);
    }

    #[test]
    fn save_in_the_unsaved_dialog_of_an_untitled_scene_then_does_what_was_asked() {
        let project = std::env::temp_dir().join(format!("ab-app-save-as-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(&project).unwrap();
        let options = Options {
            screenshot: Some(PathBuf::from("unused.png")),
            ..Default::default()
        };
        let mut app = EditorApp::new(None, project.clone(), &options, LogBuffer::default(), None);
        app.editor.add_node(
            NodeDesc {
                name: "Unsaved".into(),
                kind: Some("Node3D".into()),
                ..Default::default()
            },
            None,
        );
        // A save that fails (a folder is in the way) keeps the changes: nothing runs.
        std::fs::create_dir_all(project.join("scenes/taken.abscene")).unwrap();
        assert!(app
            .finish_save_as("scenes/taken", Some(Pending::Quit))
            .is_none());
        assert!(app.editor.modified);
        // Saved: then quit.
        let then = app.finish_save_as("scenes/level", Some(Pending::Quit));
        assert!(project.join("scenes/level.abscene").is_file());
        assert!(matches!(then, Some(Pending::Quit)));
        app.run(then.unwrap());
        assert!(app.allow_close);
        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn save_as_checks_the_file_it_writes() {
        let project =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        // `slice/level` is saved as `slice/level.abscene`, which exists.
        assert_eq!(save_as_check(&project, "slice/level"), (true, true));
        assert_eq!(
            save_as_check(&project, "slice\\level.abscene"),
            (true, true)
        );
        assert_eq!(save_as_check(&project, "scenes/brand_new"), (true, false));
        // Outside the project: refused.
        assert!(!save_as_check(&project, "../outside").0);
        assert!(!save_as_check(&project, "/tmp/outside").0);
        assert!(!save_as_check(&project, "C:/outside").0);
        assert!(!save_as_check(&project, "  ").0);
    }
}
