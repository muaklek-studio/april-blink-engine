//! Running a game from its `game.toml`: the whole `main` of a game is one call.
//!
//! Games script with the `ab-script` SDK; a game executable links the script library and
//! hands its entry point over (shipped games instead run the generic player, which loads
//! the library file next to it):
//!
//! ```ignore
//! fn main() -> anyhow::Result<()> {
//!     ab_engine::run_game(
//!         ab_engine::Game::with_scripts("game.toml", my_game::ab_script_library)
//!             .project(env!("CARGO_MANIFEST_DIR"))
//!             .hot_library("my_game"),
//!     )
//! }
//! ```
//!
//! `game.toml` and the input map are project files, not assets: they sit in the project
//! folder, next to `assets/` (exports pack them apart from the assets; see
//! [`crate::assets::read_project_file`]).
//!
//! ```toml
//! title = "My Game"
//! main_scene = "scenes/main.abscene"  # in the assets
//! input = "input.toml"            # optional action map, in the project folder
//! config = "config.toml"          # optional named values (see crate::game_config)
//! assets = "assets"               # asset folder, in the project folder (the default)
//!
//! [window]
//! width = 1280
//! height = 720
//! fullscreen = false
//!
//! [localization]                  # optional: assets/locales/en.toml, th.toml
//! languages = ["en", "th"]
//! ```
//!
//! The executable understands `--headless <frames>`, `--scene <file>`, `--screenshot <png>
//! [--frames <n>]`, `--fullscreen` / `--windowed`, `--overlay` and `--hot [<library>]`.
//! Scenes switch with [`change_scene`].

use crate::save::SaveStore;
use crate::scene_file::{load_scene, SceneFileError};
use crate::script_host::ScriptLibrary;
use crate::{DebugOverlay, Engine, EngineConfig};
use ab_input::{Actions, InputMap};
use ab_rendering::Environment;
use ab_scene::{NodeHandle, SceneTree, ScriptRegistry};
use anyhow::{bail, Context as _};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{info, warn};

/// The contents of `game.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GameSettings {
    pub title: String,
    /// Scene loaded at start (asset path).
    pub main_scene: String,
    /// Action map, relative to the project folder, e.g. `input.toml`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// The game's named values (see [`crate::game_config`]), relative to the project
    /// folder, e.g. `config.toml`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// Asset folder, relative to the project folder (default `assets`). Shipped games read
    /// their pack instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assets: Option<String>,
    /// Folder name for save games (default: the title).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_name: Option<String>,
    pub window: WindowSettings,
    /// Physics steps per second.
    pub physics_hz: u32,
    /// Worker threads for parallel engine work (default: every logical core but one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_threads: Option<usize>,
    /// The game's languages (see [`crate::localization`]).
    #[serde(skip_serializing_if = "is_default")]
    pub localization: crate::localization::LocalizationSettings,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// `[window]` of `game.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WindowSettings {
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub resizable: bool,
    pub vsync: bool,
    /// Frame rate cap (`None`: as fast as vsync allows).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps_limit: Option<u32>,
    /// Anti-aliasing samples: 1 or 4.
    pub msaa: u32,
}

/// The main scene when `game.toml` names none.
const DEFAULT_MAIN_SCENE: &str = "main.abscene";

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            title: "April Blink Game".into(),
            main_scene: DEFAULT_MAIN_SCENE.into(),
            input: None,
            config: None,
            assets: None,
            save_name: None,
            window: WindowSettings::default(),
            physics_hz: 60,
            worker_threads: None,
            localization: Default::default(),
        }
    }
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fullscreen: false,
            resizable: true,
            vsync: true,
            fps_limit: None,
            msaa: 4,
        }
    }
}

impl GameSettings {
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// The engine configuration these settings describe.
    pub fn engine_config(&self, asset_root: PathBuf) -> EngineConfig {
        let mut config = EngineConfig {
            asset_root,
            physics_hz: self.physics_hz.max(1),
            worker_threads: self.worker_threads.map(|n| n.max(1)),
            ..Default::default()
        };
        config.window.title = self.title.clone();
        config.window.width = self.window.width.max(320);
        config.window.height = self.window.height.max(200);
        config.window.fullscreen = self.window.fullscreen;
        config.window.resizable = self.window.resizable;
        config.window.target_fps = self.window.fps_limit;
        config.renderer.vsync = self.window.vsync;
        config.renderer.msaa_samples = if self.window.msaa >= 4 { 4 } else { 1 };
        config
    }

    /// The asset folder, relative to the project folder.
    pub fn asset_folder(&self) -> &str {
        self.assets.as_deref().unwrap_or("assets")
    }

    /// Folder name for saves.
    pub fn save_folder(&self) -> &str {
        self.save_name.as_deref().unwrap_or(&self.title)
    }
}

/// ECS resource: the scene [`change_scene`] last loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentScene {
    pub node: NodeHandle,
    pub path: String,
}

/// Replaces the current scene with the scene file at `path` (Godot's
/// `change_scene_to_file`): the old scene is freed at the end of the frame, the new one is
/// added under the root right away and its `[environment]` applied over the defaults.
/// Safe to call from scripts of the old scene.
pub fn change_scene(tree: &mut SceneTree, path: &str) -> Result<NodeHandle, SceneFileError> {
    let previous = tree.world().resource::<Environment>().map(|e| e.clone());
    tree.world_mut().insert_resource(Environment::default());
    let node = match load_scene(tree, path) {
        Ok(node) => node,
        Err(err) => {
            if let Some(environment) = previous {
                tree.world_mut().insert_resource(environment);
            }
            return Err(err);
        }
    };
    let old = tree.world().resource::<CurrentScene>().map(|c| c.node);
    if let Some(old) = old.filter(|&n| tree.is_valid(n)) {
        let _ = tree.queue_free(old);
    }
    tree.add_child(tree.root(), node)
        .map_err(|e| SceneFileError::Node {
            path: path.to_owned(),
            node: String::new(),
            reason: e.to_string(),
        })?;
    tree.world_mut().insert_resource(CurrentScene {
        node,
        path: path.to_owned(),
    });
    info!(scene = path, "scene changed");
    Ok(node)
}

/// Loads the current scene again from its file (restart a level).
pub fn reload_scene(tree: &mut SceneTree) -> Result<NodeHandle, SceneFileError> {
    let path = tree
        .world()
        .resource::<CurrentScene>()
        .map(|c| c.path.clone())
        .ok_or_else(|| SceneFileError::Node {
            path: String::new(),
            node: String::new(),
            reason: "no scene was loaded with change_scene".into(),
        })?;
    change_scene(tree, &path)
}

/// A game: its `game.toml`, where its project folder and assets are, its scripts.
#[derive(Debug, Clone)]
pub struct Game {
    /// `game.toml`, relative to the project folder.
    file: String,
    project_dir: PathBuf,
    /// The asset folder, when not the one `game.toml` names.
    asset_root: Option<PathBuf>,
    scripts: Scripts,
    /// Library `--hot` watches when given no path.
    hot_library: Option<PathBuf>,
}

/// Where a game's scripts come from.
#[derive(Debug, Clone)]
enum Scripts {
    /// Native scripts (engine tests and tools).
    Native(fn(&mut ScriptRegistry)),
    /// A script library linked into the executable.
    Linked(ab_script_api::EntryPoint),
    /// A script library file (shipped games: next to the player).
    File(PathBuf),
}

/// Command-line options of a game executable.
#[derive(Debug, Clone, Default, PartialEq)]
struct Cli {
    headless: Option<u64>,
    scene: Option<String>,
    screenshot: Option<PathBuf>,
    frames: Option<u64>,
    fullscreen: Option<bool>,
    overlay: bool,
    hot: Option<Option<PathBuf>>,
}

const USAGE: &str = "options: [--headless <frames>] [--scene <file>] \
[--screenshot <file.png> [--frames <n>]] [--fullscreen | --windowed] [--overlay] [--hot [<library>]] \
[--licenses]";

/// Options only developers use; shipped builds ignore them.
const DEVELOPER_OPTIONS: &[&str] = &["--scene", "--screenshot", "--frames", "--overlay", "--hot"];

impl Cli {
    fn parse(args: impl IntoIterator<Item = String>) -> anyhow::Result<Option<Self>> {
        let mut cli = Self::default();
        let mut args = args.into_iter().peekable();
        while let Some(arg) = args.next() {
            if crate::SHIPPING && DEVELOPER_OPTIONS.contains(&arg.as_str()) {
                warn!("{arg} is a developer option; this build ignores it");
                // Skip its value too.
                if arg != "--overlay" {
                    args.next_if(|a| !a.starts_with("--"));
                }
                continue;
            }
            let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--licenses" => {
                    crate::crash::show_text("Licenses", &licenses());
                    return Ok(None);
                }
                "--headless" => cli.headless = Some(value()?.parse()?),
                "--scene" => cli.scene = Some(value()?),
                "--screenshot" => cli.screenshot = Some(value()?.into()),
                "--frames" => cli.frames = Some(value()?.parse()?),
                "--fullscreen" => cli.fullscreen = Some(true),
                "--windowed" => cli.fullscreen = Some(false),
                "--overlay" => cli.overlay = true,
                "--hot" => {
                    cli.hot = Some(args.next_if(|a| !a.starts_with("--")).map(PathBuf::from));
                }
                "-h" | "--help" => {
                    println!("{USAGE}");
                    return Ok(None);
                }
                other => bail!("unknown argument `{other}`\n{USAGE}"),
            }
        }
        Ok(Some(cli))
    }
}

/// Third-party licenses that ship inside the game.
fn licenses() -> String {
    format!(
        "This game uses the Prompt font by Cadson Demak, under the SIL Open Font License 1.1:\n\n{}",
        ab_ui::FONT_LICENSE
    )
}

impl Game {
    /// `file`: path of `game.toml` in the project folder (by default the working directory;
    /// see [`Game::project`]). Shipped games read it from their pack; see [`crate::assets`].
    ///
    /// `register` registers native scripts ([`ab_scene::NodeScript`]); games use the
    /// `ab-script` SDK instead ([`Game::with_scripts`]).
    pub fn new(file: &str, register: fn(&mut ScriptRegistry)) -> Self {
        Self::with_source(file, Scripts::Native(register))
    }

    /// A game whose scripts are an `ab-script` library linked into the executable: `entry`
    /// is the library's `ab_script_library` function (made by `ab_script::export!`).
    pub fn with_scripts(file: &str, entry: ab_script_api::EntryPoint) -> Self {
        Self::with_source(file, Scripts::Linked(entry))
    }

    /// A game whose scripts are an `ab-script` library file (`.dll`, `.so`), loaded at start.
    pub fn with_script_file(file: &str, library: impl Into<PathBuf>) -> Self {
        Self::with_source(file, Scripts::File(library.into()))
    }

    fn with_source(file: &str, scripts: Scripts) -> Self {
        Self {
            file: file.into(),
            project_dir: PathBuf::from("."),
            asset_root: None,
            scripts,
            hot_library: None,
        }
    }

    /// The project folder during development (typically `env!("CARGO_MANIFEST_DIR")`):
    /// `game.toml`, the input map and the asset folder are read from there. Shipped games
    /// read their pack instead, wherever they run.
    pub fn project(mut self, dir: impl Into<PathBuf>) -> Self {
        self.project_dir = dir.into();
        self
    }

    /// Asset directory, instead of the one `game.toml` names.
    pub fn assets(mut self, root: impl Into<PathBuf>) -> Self {
        self.asset_root = Some(root.into());
        self
    }

    /// The asset directory for `settings`: the project's during development; for a shipped
    /// game (its `game.toml` packed), `assets` next to it, where loose files patch the pack.
    pub fn asset_root(&self, settings: &GameSettings) -> PathBuf {
        if let Some(root) = &self.asset_root {
            return root.clone();
        }
        let executable = std::env::current_exe().ok();
        if crate::assets::is_packed(executable.as_deref(), &self.file) {
            PathBuf::from("assets")
        } else {
            self.project_dir.join(settings.asset_folder())
        }
    }

    /// Reads the project file `name` (`game.toml`, the input map).
    fn project_file(&self, name: &str) -> anyhow::Result<String> {
        let executable = std::env::current_exe().ok();
        let bytes =
            crate::assets::read_project_file(&self.project_dir, executable.as_deref(), name)?;
        String::from_utf8(bytes).with_context(|| format!("{name} is not UTF-8"))
    }

    /// File stem of the game's script library for `--hot` (e.g. `my_game` for
    /// `libmy_game.so` next to the executable).
    pub fn hot_library(mut self, stem: &str) -> Self {
        let dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(PathBuf::from))
            .unwrap_or_default();
        self.hot_library = Some(dir.join(format!(
            "{}{stem}{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        )));
        self
    }

    /// The script library `--hot` watches when given no path.
    pub fn hot_library_path(mut self, library: impl Into<PathBuf>) -> Self {
        self.hot_library = Some(library.into());
        self
    }

    /// The library `--hot` watches: the one given, else [`Game::hot_library`]'s. An error
    /// when there is neither (there would be no file to watch).
    fn hot_library_for(&self, hot: Option<Option<PathBuf>>) -> anyhow::Result<Option<PathBuf>> {
        match hot {
            None => Ok(None),
            Some(Some(path)) => Ok(Some(path)),
            Some(None) => match &self.hot_library {
                Some(path) => Ok(Some(path.clone())),
                None => bail!(
                    "--hot needs the script library to watch (`--hot <library>`): this game \
                     names none"
                ),
            },
        }
    }

    /// Reads `game.toml`.
    pub fn settings(&self) -> anyhow::Result<GameSettings> {
        let text = self
            .project_file(&self.file)
            .with_context(|| format!("cannot read {}", self.file))?;
        GameSettings::from_toml(&text).with_context(|| format!("invalid {}", self.file))
    }

    /// Prepares a running engine: registers the scripts, loads the action map, sets up saves
    /// and loads `scene` (or the main scene). Returns the scene's root.
    pub fn setup(
        &self,
        engine: &mut Engine,
        settings: &GameSettings,
        scene: Option<&str>,
    ) -> anyhow::Result<NodeHandle> {
        let tree = engine.tree_mut();
        {
            let library = match &self.scripts {
                Scripts::Native(_) => None,
                Scripts::Linked(entry) => Some(ScriptLibrary::from_entry(*entry, &settings.title)?),
                Scripts::File(path) => Some(ScriptLibrary::load(path)?),
            };
            let registry = tree
                .world_mut()
                .resource_or_insert_with(ScriptRegistry::new);
            match (&self.scripts, library) {
                (Scripts::Native(register), _) => register(registry),
                (_, Some(library)) => library.register(registry),
                _ => {}
            }
        }
        if let Some(input) = &settings.input {
            let text = self
                .project_file(input)
                .with_context(|| format!("cannot read the input map {input}"))?;
            let map = InputMap::from_toml(&text).with_context(|| format!("invalid {input}"))?;
            if let Some(mut actions) = tree.world().resource_mut::<Actions>() {
                actions.set_map(map);
            }
        }
        tree.world_mut()
            .insert_resource(SaveStore::for_game(settings.save_folder()));
        let mut localization = settings.localization.clone();
        if let Some(file) = &settings.config {
            let text = self
                .project_file(file)
                .with_context(|| format!("cannot read the config {file}"))?;
            let config = crate::game_config::GameConfig::from_toml(&text)
                .with_context(|| format!("invalid {file}"))?;
            // `Game.DefaultLanguage` picks the language when game.toml does not.
            if localization.default_language.is_none() {
                localization.default_language =
                    config.text("Game.DefaultLanguage").ok().map(str::to_owned);
            }
            info!(values = config.len(), "config loaded");
            tree.world_mut().insert_resource(config);
        }
        if let Err(err) = crate::localization::install(tree.world_mut(), &localization) {
            warn!("languages not loaded: {err}");
        }
        Ok(change_scene(tree, scene.unwrap_or(&settings.main_scene))?)
    }

    /// Parses the command line and runs the game (windowed unless told otherwise).
    ///
    /// The log is also written to `<data folder>/logs/latest.log`, and a panic or error that
    /// ends the game leaves a crash report there (see [`crate::crash`]).
    pub fn run(self) -> anyhow::Result<()> {
        self.run_with_args(std::env::args().skip(1))
    }

    /// [`Game::run`] with these command-line arguments (without the program name).
    pub fn run_with_args(self, args: impl IntoIterator<Item = String>) -> anyhow::Result<()> {
        ab_core::init_logging();
        crate::crash::install_panic_hook();
        let Some(cli) = Cli::parse(args)? else {
            return Ok(());
        };
        let settings = match self.settings() {
            Ok(settings) => settings,
            // No title or data folder yet: report under the executable's name.
            Err(err) => {
                let name = std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "The game".into());
                return crate::crash::run_reporting_crashes(&name, None, || Err(err));
            }
        };
        let log_dir = SaveStore::for_game(settings.save_folder())
            .dir()
            .join("logs");
        let log_dir = match ab_core::log_to_directory(&log_dir) {
            Ok(path) => {
                info!(log = %path.display(), "logging to file");
                Some(log_dir)
            }
            Err(err) => {
                warn!("cannot write a log file in {}: {err}", log_dir.display());
                None
            }
        };
        let title = settings.title.clone();
        crate::crash::run_reporting_crashes(&title, log_dir.as_deref(), || {
            self.run_with(cli, settings)
        })
    }

    fn run_with(self, cli: Cli, mut settings: GameSettings) -> anyhow::Result<()> {
        if let Some(fullscreen) = cli.fullscreen {
            settings.window.fullscreen = fullscreen;
        }
        let config = settings.engine_config(self.asset_root(&settings));
        info!(
            title = settings.title,
            scene = settings.main_scene,
            "starting game"
        );
        let hot = self.hot_library_for(cli.hot.clone())?;
        let scene = cli.scene.clone();
        let overlay = cli.overlay;
        let game = self.clone();
        let game_settings = settings.clone();
        let setup = move |engine: &mut Engine| -> anyhow::Result<()> {
            if let Some(mut debug) = engine.tree().world().resource_mut::<DebugOverlay>() {
                debug.visible = overlay;
            }
            game.setup(engine, &game_settings, scene.as_deref())?;
            if let Some(library) = hot {
                if let Err(err) = engine.enable_hot_reload(&library) {
                    warn!(
                        "hot reload: {err:#}; watching {} for a new build",
                        library.display()
                    );
                }
            }
            Ok(())
        };

        if let Some(path) = cli.screenshot {
            // The window's size as the engine uses it (clamped like `engine_config` does).
            let (width, height) = (config.window.width, config.window.height);
            let engine =
                crate::run_headless(config.clone(), cli.frames.unwrap_or(90), 1.0 / 60.0, setup)?;
            let pixels = crate::render_offscreen(&engine, &config.renderer, width, height)?;
            let bytes: Vec<u8> = pixels.into_iter().flatten().collect();
            image::save_buffer(
                &path,
                &bytes,
                width,
                height,
                image::ExtendedColorType::Rgba8,
            )?;
            info!(path = %path.display(), "screenshot saved");
            return Ok(());
        }
        if let Some(frames) = cli.headless {
            let engine = crate::run_headless(config, frames, 1.0 / 60.0, setup)?;
            let failed = engine
                .tree()
                .subtree(engine.tree().root())
                .into_iter()
                .filter(|&n| engine.tree().has_failed_script(n))
                .count();
            info!(
                frames = engine.time().frame_count,
                nodes = engine.tree().node_count(),
                failed_scripts = failed,
                "headless run finished"
            );
            if failed > 0 {
                bail!("{failed} script(s) panicked during the run");
            }
            return Ok(());
        }
        crate::run(config, setup)
    }
}

/// Runs `game`; see [`Game::run`].
pub fn run_game(game: Game) -> anyhow::Result<()> {
    game.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn developer_options_follow_the_build() {
        let args = ["--overlay", "--scene", "x.abscene", "--headless", "5"].map(String::from);
        let cli = Cli::parse(args).unwrap().unwrap();
        assert_eq!(cli.headless, Some(5), "the export's check needs --headless");
        assert_eq!(cli.overlay, !crate::SHIPPING);
        assert_eq!(cli.scene.is_some(), !crate::SHIPPING);
        let config = EngineConfig::default();
        assert_eq!(config.debug_overlay_key.is_some(), !crate::SHIPPING);
        assert_eq!(config.debug_physics_key.is_some(), !crate::SHIPPING);
        assert!(config.fullscreen_key.is_some(), "players keep F11");
        assert!(licenses().contains("SIL OPEN FONT LICENSE"));
    }

    #[test]
    fn settings_parse_with_defaults() {
        let settings = GameSettings::from_toml(
            "title = \"T\"\nmain_scene = \"a.toml\"\n[window]\nfullscreen = true\nmsaa = 1\n",
        )
        .unwrap();
        assert_eq!(settings.window.width, 1280);
        assert!(settings.window.fullscreen);
        let config = settings.engine_config("x".into());
        assert_eq!(config.window.title, "T");
        assert!(config.window.fullscreen);
        assert_eq!(config.renderer.msaa_samples, 1);
        assert_eq!(settings.save_folder(), "T");
        assert!(GameSettings::from_toml("titel = \"typo\"").is_err());
        // What `--screenshot` renders at: never a zero-sized image.
        let tiny = GameSettings::from_toml("[window]\nwidth = 0\nheight = 0\n").unwrap();
        let config = tiny.engine_config("x".into());
        assert_eq!((config.window.width, config.window.height), (320, 200));
    }

    #[test]
    fn command_line() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let cli = Cli::parse(args(&["--headless", "60", "--scene", "b.abscene", "--hot"]))
            .unwrap()
            .unwrap();
        assert_eq!(cli.headless, Some(60));
        assert_eq!(cli.scene.as_deref(), Some("b.abscene"));
        assert_eq!(cli.hot, Some(None));
        let cli = Cli::parse(args(&["--hot", "lib.so", "--windowed"]))
            .unwrap()
            .unwrap();
        assert_eq!(cli.hot, Some(Some(PathBuf::from("lib.so"))));
        assert_eq!(cli.fullscreen, Some(false));
        assert!(Cli::parse(args(&["--nope"])).is_err());
        // `--hot` without a library: the game's, or a clear error (not an empty path).
        let game = Game::new("game.toml", |_| {});
        assert!(game.hot_library_for(Some(None)).is_err());
        assert_eq!(game.hot_library_for(None).unwrap(), None);
        let given = game.hot_library_for(Some(Some("x.so".into()))).unwrap();
        assert_eq!(given, Some(PathBuf::from("x.so")));
        let game = game.hot_library_path("lib.so");
        let default = game.hot_library_for(Some(None)).unwrap();
        assert_eq!(default, Some(PathBuf::from("lib.so")));
        assert!(Cli::parse(args(&["--headless"])).is_err());
    }
}
