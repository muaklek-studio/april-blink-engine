//! The editor's model: the engine holding the open scene, selection, undo history, and the
//! switch between editing and playing.
//!
//! The live scene tree is the document. Edits change it directly (transforms, names,
//! hierarchy) or rebuild one node from its edited description; saving describes the tree
//! with [`save_scene`]. Play mode snapshots the scene, rebuilds it with the game's real
//! scripts, runs the engine, and restores the snapshot on stop.

use crate::history::{History, Snapshot};
use crate::project::AssetIndex;
use crate::scripts::GameScripts;
use ab_engine::audio::{audio_system, AudioOutput};
use ab_engine::effects::effects_system;
use ab_engine::input::{InputMap, InputState};
use ab_engine::prelude::*;
use ab_engine::scene::ScriptPropertyDesc;
use ab_engine::scene_file::{
    apply_environment, instantiate_scene, particle_emitter, save_scene, EnvironmentDesc, NodeDesc,
    SceneOrigin,
};
use anyhow::Context as _;
use std::path::{Path, PathBuf};
use tracing::{error, info, warn};

/// Stand-in for game scripts while editing: scenes keep their script names, nothing runs.
#[derive(Default)]
struct Inert;

impl NodeScript for Inert {}

fn inert() -> Box<dyn NodeScript> {
    Box::new(Inert)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Edit,
    Play { paused: bool },
}

/// What a frame of editing changed, for the status bar.
#[derive(Debug, Clone)]
pub struct Status {
    pub text: String,
    pub error: bool,
}

/// The project whose asset folder is `assets` (Night Relics for the engine's own assets).
fn project_of(assets: &Path) -> Option<ab_project::Project> {
    ab_project::Project::open(assets).ok().or_else(|| {
        crate::scripts::game_crate(assets).and_then(|root| ab_project::Project::open(&root).ok())
    })
}

/// The input map of the project whose asset folder is `assets`.
fn input_map_of(assets: &Path) -> Option<PathBuf> {
    let project = project_of(assets)?;
    Some(project.root.join(project.input?))
}

/// The config file of the project whose asset folder is `assets` (`config` in `game.toml`).
fn config_of(assets: &Path) -> Option<PathBuf> {
    let project = project_of(assets)?;
    Some(project.root.join(project.config?))
}

/// The languages of the project whose asset folder is `assets` (`[localization]` of its
/// `game.toml`), so keyed UI texts show translated in the view.
fn localization_of(assets: &Path) -> Option<ab_engine::localization::LocalizationSettings> {
    let project = project_of(assets)?;
    let text = std::fs::read_to_string(project.root.join(ab_project::GAME_FILE)).ok()?;
    match ab_engine::game::GameSettings::from_toml(&text) {
        Ok(settings) => Some(settings.localization),
        Err(err) => {
            warn!("{}: {err}", ab_project::GAME_FILE);
            None
        }
    }
}

pub struct Editor {
    pub engine: Engine,
    pub assets: AssetIndex,
    /// Scene file being edited, relative to the project.
    pub scene_path: Option<String>,
    pub scene_root: NodeHandle,
    pub environment: EnvironmentDesc,
    /// The active node: shown in the inspector, where the gizmo sits. Change the selection
    /// with [`Editor::select`] and friends, which keep it in [`Editor::selected`].
    pub selection: Option<NodeHandle>,
    /// Every selected node, in the order they were selected (the active one included).
    pub selected: Vec<NodeHandle>,
    pub modified: bool,
    pub history: History,
    pub mode: Mode,
    /// Particles and other effects animate while editing.
    pub preview_effects: bool,
    pub status: Option<Status>,
    /// The game's script library, for Play mode.
    pub scripts_library: Option<PathBuf>,
    /// The game's script files (`src/scripts`), when the project is a game crate.
    pub scripts: Option<GameScripts>,
    /// The game's input map (a project file, next to `game.toml`).
    input_map: Option<PathBuf>,
    edit_registry: ScriptRegistry,
    play_registry: ScriptRegistry,
    play_snapshot: Option<(Snapshot, Environment)>,
    /// An edit gesture (a drag, a text field) is in progress: one undo step for all of it.
    gesture: bool,
    /// Nodes copied or cut, for Paste.
    pub clipboard: Clipboard,
    /// The project's `config.toml`, if it has one.
    pub config_path: Option<PathBuf>,
    /// The piece being placed with the build tool (see [`crate::build`]).
    pub build: Option<BuildPiece>,
    /// The build tool's copy of the piece following the pointer: outside the scene (not
    /// saved, not picked), with its bounds around its origin.
    ghost: Option<(BuildPiece, NodeHandle, ab_engine::rendering::Aabb)>,
    /// Where [`Editor::write_project_file`] keeps the version of a file it replaces (`None`:
    /// no backups, as in tests).
    pub backups: Option<PathBuf>,
}

/// The folder in `store` (the editor's data folder) for backups of `project`'s files: one
/// per project, named after it and its full path (two projects may share a name). The hash
/// is FNV, so the folder stays the same across Rust versions.
pub fn backup_dir(store: &Path, project: &Path) -> PathBuf {
    let hash = ab_engine::core::utils::hash::fnv1a64_str(&project.to_string_lossy());
    let name = project
        .file_name()
        .map_or_else(|| "project".into(), |n| n.to_string_lossy());
    store.join("backups").join(format!("{name}-{hash:016x}"))
}

/// A piece of the build tool: what is placed, and how it is turned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPiece {
    /// A model or prefab (asset path).
    pub path: String,
    /// Quarter turns around the vertical axis.
    pub turns: u8,
}

/// Which side of their bounds [`Editor::align_selected`] lines objects up by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Min,
    Center,
    Max,
}

/// Copied nodes: each a scene file of a node and its children, with where it was in the
/// world (so a paste under another parent lands at the same place).
#[derive(Debug, Clone, Default)]
pub struct Clipboard {
    pub items: Vec<(SceneFile, Option<ab_engine::core::Affine3A>)>,
    /// The text put on the system clipboard for them: a paste of that same text uses the
    /// items (with their world placement) instead of parsing it again.
    pub text: String,
}

/// The system clipboard's text for copied nodes: a marker key and one table per node.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ClipboardText {
    april_blink_nodes: u32,
    #[serde(rename = "scene")]
    scenes: Vec<SceneFile>,
}

impl Clipboard {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Nodes from the system clipboard's text, if it holds nodes copied in an editor.
    pub fn parse(text: &str) -> Option<Vec<SceneFile>> {
        let parsed: ClipboardText = toml::from_str(text).ok()?;
        (parsed.april_blink_nodes == 1 && !parsed.scenes.is_empty()).then_some(parsed.scenes)
    }

    fn to_text(items: &[(SceneFile, Option<ab_engine::core::Affine3A>)]) -> String {
        let text = ClipboardText {
            april_blink_nodes: 1,
            scenes: items.iter().map(|(scene, _)| scene.clone()).collect(),
        };
        toml::to_string(&text).unwrap_or_default()
    }
}

impl Editor {
    pub fn new(project: PathBuf, scripts: Option<PathBuf>, audio: AudioOutput) -> Self {
        let config = EngineConfig {
            asset_root: project.clone(),
            audio_output: audio,
            ..Default::default()
        };
        let mut engine = Engine::new(&config);
        engine
            .tree_mut()
            .world_mut()
            .resource_or_insert_with(ScriptRegistry::new);

        let mut play_registry = ScriptRegistry::new();
        if let Some(library) = &scripts {
            match engine.enable_hot_reload(library) {
                Ok(_) => {
                    if let Some(registry) = engine.tree().world().resource::<ScriptRegistry>() {
                        play_registry = registry.clone();
                        info!(scripts = registry.len(), "game scripts loaded");
                    }
                }
                Err(err) => error!("cannot load the game's scripts: {err:#}"),
            }
        }
        // Unknown names still play (doing nothing) rather than refusing the scene.
        play_registry.set_fallback(Some(std::sync::Arc::new(inert)));
        let mut edit_registry = ScriptRegistry::new();
        edit_registry.set_fallback(Some(std::sync::Arc::new(inert)));
        engine
            .tree_mut()
            .world_mut()
            .insert_resource(edit_registry.clone());

        let scene_root = engine.tree().root();
        let game_scripts = crate::scripts::game_crate(&project).map(GameScripts::new);
        let input_map = input_map_of(&project);
        let project_dir = project.clone();
        if let Some(localization) = localization_of(&project) {
            if let Err(err) =
                ab_engine::localization::install(engine.tree_mut().world_mut(), &localization)
            {
                warn!("the project's languages: {err}");
            }
        }
        let mut editor = Self {
            engine,
            assets: AssetIndex::new(project),
            scene_path: None,
            scene_root,
            environment: EnvironmentDesc::default(),
            selection: None,
            selected: Vec::new(),
            modified: false,
            history: History::default(),
            mode: Mode::Edit,
            preview_effects: true,
            status: None,
            scripts_library: scripts,
            scripts: game_scripts,
            input_map,
            edit_registry,
            play_registry,
            play_snapshot: None,
            gesture: false,
            clipboard: Clipboard::default(),
            config_path: config_of(&project_dir),
            build: None,
            ghost: None,
            backups: None,
        };
        editor.load_config();
        editor.new_scene();
        editor
    }

    pub fn tree(&self) -> &SceneTree {
        self.engine.tree()
    }

    pub fn tree_mut(&mut self) -> &mut SceneTree {
        self.engine.tree_mut()
    }

    /// Writes a file of the project crash-safely (the old file stays whole until the new
    /// one is on disk), creating its folder, and keeps the version it replaces in
    /// [`Editor::backups`].
    pub fn write_project_file(
        &self,
        full: &Path,
        contents: impl AsRef<[u8]>,
    ) -> std::io::Result<()> {
        if let Some(dir) = full.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let backup = self
            .backups
            .as_ref()
            .zip(full.strip_prefix(self.project()).ok())
            .map(|(dir, relative)| {
                let mut backup = dir.join(relative).into_os_string();
                backup.push(".bak");
                PathBuf::from(backup)
            });
        match backup {
            Some(backup) => ab_engine::core::fs::write_with_backup(full, contents, backup),
            None => ab_engine::core::fs::atomic_write(full, contents),
        }
    }

    pub fn project(&self) -> &Path {
        &self.assets.root
    }

    pub fn is_playing(&self) -> bool {
        matches!(self.mode, Mode::Play { .. })
    }

    pub fn info(&mut self, text: impl Into<String>) {
        let text = text.into();
        info!("{text}");
        self.status = Some(Status { text, error: false });
    }

    pub fn fail(&mut self, text: impl Into<String>) {
        let text = text.into();
        error!("{text}");
        self.status = Some(Status { text, error: true });
    }

    /// Script names to offer: the game library's, the script files' (even before they are
    /// built), plus any the scene already uses.
    pub fn script_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.play_registry.names().map(str::to_owned).collect();
        if let Some(scripts) = &self.scripts {
            names.extend(scripts.all_types().map(|(name, _)| name.to_owned()));
        }
        let tree = self.tree();
        names.extend(
            tree.subtree(self.scene_root)
                .into_iter()
                .flat_map(|n| tree.script_names(n)),
        );
        names.sort();
        names.dedup();
        names
    }

    /// Script names the game registers itself (in `lib.rs`), which new scripts must not take.
    pub fn registered_script_names(&self) -> Vec<String> {
        self.play_registry.names().map(str::to_owned).collect()
    }

    pub fn script_metadata(&self, name: &str) -> Option<Vec<ScriptPropertyDesc>> {
        if let Some(m) = self.play_registry.metadata(name) {
            return Some(m.to_vec());
        }
        self.edit_registry.metadata(name).map(|m| m.to_vec())
    }

    /// Adds script `name` to `node`'s scripts (one undo step), like adding a component in
    /// Unity. A script the node already has is not added twice.
    pub fn attach_script(&mut self, node: NodeHandle, name: &str) {
        if self.is_playing() {
            self.fail("Stop the game to attach scripts");
            return;
        }
        let Some(old) = self.describe_node(node) else {
            return;
        };
        let mut list = old.script_list();
        if list.iter().any(|s| s == name) {
            self.info(format!("{} already has {name}", old.name));
            return;
        }
        list.push(name.to_owned());
        let mut new = old.clone();
        new.set_script_list(list);
        let node_name = new.name.clone();
        self.apply_desc(node, &old, new);
        self.select(Some(node));
        let built = self.play_registry.names().any(|n| n == name);
        if built {
            self.info(format!("Attached {name} to {node_name}"));
        } else {
            self.info(format!(
                "Attached {name} to {node_name} (it runs in Play mode once the scripts are built)"
            ));
        }
    }

    /// Whether the game's script library is loaded.
    pub fn has_game_scripts(&self) -> bool {
        !self.play_registry.is_empty()
    }

    /// Title for the window: scene name, `*` when unsaved.
    pub fn title(&self) -> String {
        let scene = self.scene_path.as_deref().unwrap_or("Untitled");
        let dirty = if self.modified { "*" } else { "" };
        format!("{scene}{dirty} — April Blink Editor")
    }

    // =========================================================================================
    // Documents
    // =========================================================================================

    /// A starter scene: a root, a sun, a floor and a camera.
    pub fn new_scene(&mut self) {
        let text = ab_project::NEW_SCENE;
        let scene = SceneFile::from_toml(text).expect("the template parses");
        if self.replace_scene(&scene).is_ok() {
            self.scene_path = None;
            self.modified = false;
            self.history.clear();
            self.select(None);
            self.info("New scene");
        }
    }

    /// Opens a scene file (path relative to the project).
    pub fn open_scene(&mut self, path: &str) {
        let result = (|| -> anyhow::Result<()> {
            let full = self.project().join(path);
            let text = std::fs::read_to_string(&full)
                .with_context(|| format!("cannot read {}", full.display()))?;
            let scene = SceneFile::from_toml(&text).with_context(|| format!("invalid {path}"))?;
            self.replace_scene(&scene)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.scene_path = Some(path.to_owned());
                self.modified = false;
                self.history.clear();
                self.select(None);
                self.load_input_map();
                self.info(format!("Opened {path}"));
            }
            Err(err) => self.fail(format!("{err:#}")),
        }
    }

    /// Adds the input actions `template` reads to the game's input map (those it has not
    /// got yet). Returns the actions added.
    pub fn add_template_actions(
        &mut self,
        template: &ab_project::script_templates::ScriptTemplate,
    ) -> anyhow::Result<Vec<String>> {
        let Some(path) = self.input_map.clone() else {
            return Ok(Vec::new());
        };
        // A missing map starts empty; an unreadable one must not be replaced by the new
        // actions alone.
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
        };
        let (text, added) = ab_project::script_templates::add_actions(&text, template)?;
        if !added.is_empty() {
            self.write_project_file(&path, text)
                .with_context(|| format!("writing {}", path.display()))?;
            self.load_input_map();
        }
        Ok(added)
    }

    /// Loads the game's input map, for Play mode.
    fn load_input_map(&mut self) {
        let Some(path) = &self.input_map else {
            return;
        };
        let map = std::fs::read_to_string(path)
            .map_err(anyhow::Error::from)
            .and_then(|text| Ok(InputMap::from_toml(&text)?));
        match map {
            Ok(map) => {
                if let Some(mut actions) = self.tree().world().resource_mut::<Actions>() {
                    actions.set_map(map);
                }
                info!(map = %path.display(), "input map loaded");
            }
            Err(err) => warn!("{}: {err}", path.display()),
        }
    }

    /// Saves to the open scene's file.
    pub fn save(&mut self) {
        match self.scene_path.clone() {
            Some(path) => self.write_scene(path),
            None => self.fail("Choose a file name with Save As"),
        }
    }

    /// Saves as `path` with the `.abscene` extension (added when missing).
    pub fn save_as(&mut self, path: &str) {
        let path = path.trim().replace('\\', "/");
        self.write_scene(ab_engine::scene_file::with_scene_extension(&path));
    }

    fn write_scene(&mut self, path: String) {
        if self.is_playing() {
            // The tree holds the running game, not the scene being edited.
            self.fail("Stop the game before saving");
            return;
        }
        let text = self.describe().to_toml();
        let full = self.project().join(&path);
        match self.write_project_file(&full, text) {
            Ok(()) => {
                self.scene_path = Some(path.clone());
                self.modified = false;
                self.assets.request_rescan();
                self.info(format!("Saved {path}"));
            }
            Err(err) => self.fail(format!("cannot save {}: {err}", full.display())),
        }
    }

    /// The open scene as a scene file.
    pub fn describe(&self) -> SceneFile {
        let mut scene = save_scene(self.tree(), self.scene_root);
        if self.environment != EnvironmentDesc::default() {
            scene.environment = Some(self.environment.clone());
        }
        scene
    }

    /// Builds `scene` in place of the current one. On error the current scene stays.
    fn replace_scene(&mut self, scene: &SceneFile) -> anyhow::Result<()> {
        let tree = self.engine.tree_mut();
        let root = instantiate_scene(tree, scene, "scene")?;
        for child in tree.children(tree.root()) {
            let _ = tree.queue_free(child);
        }
        tree.flush_deletions();
        let world = tree.world_mut();
        world.insert_resource(PhysicsWorld::new());
        world.insert_resource(Environment::default());
        self.environment = scene.environment.clone().unwrap_or_default();
        if let Err(err) = apply_environment(tree, &self.environment) {
            warn!("environment: {err}");
        }
        tree.add_child(tree.root(), root)?;
        tree.update_transforms();
        self.scene_root = root;
        Ok(())
    }

    // =========================================================================================
    // Undo
    // =========================================================================================

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            scene: self.describe(),
            selection: self.selection.and_then(|n| self.relative_path(n)),
            selected: self
                .selected
                .iter()
                .filter_map(|&n| self.relative_path(n))
                .collect(),
        }
    }

    fn restore(&mut self, snapshot: &Snapshot) {
        if let Err(err) = self.replace_scene(&snapshot.scene) {
            self.fail(format!("cannot restore the scene: {err:#}"));
            return;
        }
        self.restore_selection(snapshot);
    }

    fn restore_selection(&mut self, snapshot: &Snapshot) {
        let nodes = snapshot
            .selected
            .iter()
            .filter_map(|p| self.node_at(p))
            .collect();
        let active = snapshot.selection.as_deref().and_then(|p| self.node_at(p));
        self.set_selection(nodes, active);
    }

    /// Records the state before an edit. Within one gesture (a drag, a text field being
    /// typed in) only the first call records. While playing, edits are live tweaks: not
    /// recorded, discarded on stop.
    pub fn begin_edit(&mut self) {
        if self.is_playing() {
            return;
        }
        if !self.gesture {
            let snapshot = self.snapshot();
            self.history.push(snapshot);
            self.gesture = true;
        }
        self.modified = true;
    }

    /// Runs `edit`, a series of edits, as one undo step.
    pub fn as_one_step(&mut self, edit: impl FnOnce(&mut Self)) {
        if self.is_playing() {
            edit(self);
            return;
        }
        let before = self.snapshot();
        let steps = self.history.undo_len();
        edit(self);
        if self.history.undo_len() > steps {
            self.history.truncate_undo(steps);
            self.history.push(before);
        }
    }

    /// Ends the current gesture (called every frame once the pointer is up and no text
    /// field has focus).
    pub fn end_gesture(&mut self) {
        self.gesture = false;
    }

    pub fn undo(&mut self) {
        if self.is_playing() {
            return;
        }
        // An open gesture would merge the next edit into the restored state.
        self.gesture = false;
        let current = self.snapshot();
        if let Some(previous) = self.history.undo(current) {
            self.restore(&previous);
            self.modified = true;
            self.info("Undo");
        }
    }

    pub fn redo(&mut self) {
        if self.is_playing() {
            return;
        }
        self.gesture = false;
        let current = self.snapshot();
        if let Some(next) = self.history.redo(current) {
            self.restore(&next);
            self.modified = true;
            self.info("Redo");
        }
    }

    // =========================================================================================
    // Paths
    // =========================================================================================

    /// Path from the scene root (`"."` for the root itself).
    pub fn relative_path(&self, node: NodeHandle) -> Option<String> {
        let tree = self.tree();
        if node == self.scene_root {
            return Some(".".into());
        }
        let mut names = Vec::new();
        let mut current = node;
        while current != self.scene_root {
            names.push(tree.name(current)?.to_owned());
            current = tree.parent(current)?;
        }
        names.reverse();
        Some(names.join("/"))
    }

    pub fn node_at(&self, path: &str) -> Option<NodeHandle> {
        self.tree().get_node(self.scene_root, path)
    }

    /// Whether `node` belongs to the edited scene (and not to the contents of an instanced
    /// model, which are not saved).
    pub fn is_editable(&self, node: NodeHandle) -> bool {
        self.tree().is_valid(node) && self.instance_owner(node) == Some(node)
    }

    /// The scene node `node` belongs to: itself, or the instance root that contains it.
    pub fn instance_owner(&self, node: NodeHandle) -> Option<NodeHandle> {
        let tree = self.tree();
        let mut owner = node;
        let mut current = node;
        loop {
            if current == self.scene_root {
                return Some(owner);
            }
            let parent = tree.parent(current)?;
            // What a path or generator built belongs to it.
            if tree
                .component::<ab_engine::scene_file::Generated>(current)
                .is_some()
            {
                owner = parent;
            }
            if tree
                .component::<ab_engine::scene_file::SceneInstance>(parent)
                .is_some()
            {
                owner = parent;
            }
            current = parent;
        }
    }

    /// Whether `node` is part of a prefab instance (a scene file instanced in this one),
    /// directly: its changes are kept as overrides of that instance.
    pub fn is_override_target(&self, node: NodeHandle) -> bool {
        let tree = self.tree();
        let Some(owner) = self.instance_owner(node).filter(|&o| o != node) else {
            return false;
        };
        if !ab_engine::scene_file::prefab::is_prefab_instance(tree, owner)
            || tree
                .component::<ab_engine::scene_file::prefab::MissingInstance>(owner)
                .is_some()
        {
            return false;
        }
        // Not inside a model or a prefab instanced by the prefab.
        let mut current = tree.parent(node);
        while let Some(parent) = current {
            if parent == owner {
                return true;
            }
            if self.is_instance(parent) {
                return false;
            }
            current = tree.parent(parent);
        }
        false
    }

    /// Whether `node`'s properties (transform, components, scripts) can be changed: a node
    /// of this scene, or of a prefab instance.
    pub fn can_edit_properties(&self, node: NodeHandle) -> bool {
        self.is_editable(node) || self.is_override_target(node)
    }

    pub fn is_instance(&self, node: NodeHandle) -> bool {
        self.tree()
            .component::<ab_engine::scene_file::SceneInstance>(node)
            .is_some()
    }

    fn unique_name(&self, parent: NodeHandle, base: &str, except: Option<NodeHandle>) -> String {
        let tree = self.tree();
        let taken = |name: &str| {
            tree.children(parent)
                .into_iter()
                .any(|c| Some(c) != except && tree.name(c).is_some_and(|n| &*n == name))
        };
        if !taken(base) {
            return base.to_owned();
        }
        let stem = base.trim_end_matches(|c: char| c.is_ascii_digit());
        let stem = if stem.is_empty() { base } else { stem };
        (2..)
            .map(|i| format!("{stem}{i}"))
            .find(|n| !taken(n))
            .expect("some suffix is free")
    }

    // =========================================================================================
    // Selection
    // =========================================================================================

    /// Selects only `node`, or nothing.
    pub fn select(&mut self, node: Option<NodeHandle>) {
        self.selection = node;
        self.selected = node.into_iter().collect();
    }

    /// Selects `nodes` (duplicates dropped); `active` is shown in the inspector, the last
    /// node when it is not among them.
    pub fn set_selection(&mut self, nodes: Vec<NodeHandle>, active: Option<NodeHandle>) {
        let mut unique = Vec::with_capacity(nodes.len());
        for node in nodes {
            if !unique.contains(&node) {
                unique.push(node);
            }
        }
        self.selection = active
            .filter(|a| unique.contains(a))
            .or_else(|| unique.last().copied());
        self.selected = unique;
    }

    /// Adds `nodes` to the selection; the last one becomes active.
    pub fn add_to_selection(&mut self, nodes: &[NodeHandle]) {
        let mut all = self.selected.clone();
        all.extend_from_slice(nodes);
        let active = nodes.last().copied().or(self.selection);
        self.set_selection(all, active);
    }

    /// Ctrl+click: adds `node` (making it active), or takes it out of the selection.
    pub fn toggle_selected(&mut self, node: NodeHandle) {
        if self.is_selected(node) {
            let rest: Vec<NodeHandle> = self
                .selected
                .iter()
                .copied()
                .filter(|&n| n != node)
                .collect();
            let active = self.selection.filter(|&a| a != node);
            self.set_selection(rest, active);
        } else {
            self.add_to_selection(&[node]);
        }
    }

    pub fn is_selected(&self, node: NodeHandle) -> bool {
        self.selected.contains(&node)
    }

    /// The selected nodes that still exist.
    pub fn selected_nodes(&self) -> Vec<NodeHandle> {
        let tree = self.tree();
        self.selected
            .iter()
            .copied()
            .filter(|&n| tree.is_valid(n))
            .collect()
    }

    /// The selected nodes that are not inside another selected node: what moving,
    /// duplicating and deleting act on (a child moves with its parent already).
    pub fn selected_roots(&self) -> Vec<NodeHandle> {
        let nodes = self.selected_nodes();
        let tree = self.tree();
        nodes
            .iter()
            .copied()
            .filter(|&n| {
                !nodes
                    .iter()
                    .any(|&other| other != n && tree.is_ancestor_of(other, n))
            })
            .collect()
    }

    /// Drops deleted nodes from the selection.
    fn prune_selection(&mut self) {
        if self.selected.iter().all(|&n| self.tree().is_valid(n))
            && self.selection.is_none_or(|n| self.tree().is_valid(n))
        {
            return;
        }
        let nodes = self.selected_nodes();
        let active = self.selection.filter(|&n| self.tree().is_valid(n));
        self.set_selection(nodes, active);
    }

    // =========================================================================================
    // Edits
    // =========================================================================================

    /// Whether nodes can be put under `parent`: a node of this scene, not an instance (an
    /// instance is saved as its file, so what is added inside it would be lost).
    pub fn can_add_children(&self, parent: NodeHandle) -> bool {
        self.is_editable(parent) && !self.is_instance(parent)
    }

    /// Says why nodes inside an instance cannot be added, moved or deleted.
    fn refuse_instance_contents(&mut self) {
        self.fail(crate::i18n::t("status.instance_contents"));
    }

    /// Where new nodes go: under the selection, unless it is an instance (whose contents are
    /// not saved), then beside it.
    fn insertion_parent(&self) -> NodeHandle {
        match self.selection.filter(|&n| self.is_editable(n)) {
            Some(node) if self.is_instance(node) => {
                self.tree().parent(node).unwrap_or(self.scene_root)
            }
            Some(node) => node,
            None => self.scene_root,
        }
    }

    /// Adds `scene` (its first node is the root, e.g. a body with its mesh and shape) and
    /// selects it. `at` is a global position for 3D nodes.
    pub fn add_scene(&mut self, scene: SceneFile, at: Option<Vec3>) {
        let parent = self.insertion_parent();
        if let Some(node) = self.add_scene_under(scene, parent) {
            if let Some(at) = at.filter(|_| self.tree().is_node3d(node)) {
                let tree = self.engine.tree_mut();
                let _ = tree.set_global_position(node, at);
                tree.update_transforms();
            }
        }
    }

    /// Adds `scene` under `parent` and selects it (one undo step). Returns its root.
    fn add_scene_under(&mut self, mut scene: SceneFile, parent: NodeHandle) -> Option<NodeHandle> {
        if self.is_playing() || scene.nodes.is_empty() {
            return None;
        }
        if !self.can_add_children(parent) {
            self.refuse_instance_contents();
            return None;
        }
        scene.nodes[0].name = self.unique_name(parent, &scene.nodes[0].name, None);
        scene.nodes[0].parent = None;
        scene.environment = None;
        let before = self.snapshot();
        let tree = self.engine.tree_mut();
        let node = match instantiate_scene(tree, &scene, "new node") {
            Ok(node) => node,
            Err(err) => {
                self.fail(format!("{err}"));
                return None;
            }
        };
        if let Err(err) = tree.add_child(parent, node) {
            let _ = tree.queue_free(node);
            tree.flush_deletions();
            self.fail(format!("{err}"));
            return None;
        }
        tree.update_transforms();
        ab_engine::scene_file::generator::rebuild_grounded(tree, node);
        // Humanoid ragdolls find their character now that they are in the scene.
        let failed = ab_engine::ragdoll::build_rigs(tree, node);
        self.history.push(before);
        self.modified = true;
        self.select(Some(node));
        let name = self
            .tree()
            .name(node)
            .map(|n| n.to_owned())
            .unwrap_or_default();
        match failed.first() {
            Some((_, err)) => self.fail(format!("Added {name}, but its ragdoll: {err}")),
            None => self.info(format!("Added {name}")),
        }
        Some(node)
    }

    /// Adds a single node.
    pub fn add_node(&mut self, desc: NodeDesc, at: Option<Vec3>) {
        self.add_scene(
            SceneFile {
                environment: None,
                nodes: vec![desc],
            },
            at,
        );
    }

    pub fn delete(&mut self, node: NodeHandle) {
        self.delete_nodes(&[node]);
    }

    /// Deletes the selected nodes (one undo step).
    pub fn delete_selected(&mut self) {
        let nodes = self.selected_roots();
        self.delete_nodes(&nodes);
    }

    /// Deletes `nodes` (with their children) as one undo step; the scene root and the
    /// contents of instances are left alone.
    pub fn delete_nodes(&mut self, nodes: &[NodeHandle]) {
        let inside_instance = self.any_instance_contents(nodes);
        let nodes: Vec<NodeHandle> = nodes
            .iter()
            .copied()
            .filter(|&n| n != self.scene_root && self.is_editable(n))
            .collect();
        if self.is_playing() {
            return;
        }
        if nodes.is_empty() {
            if inside_instance {
                self.refuse_instance_contents();
            }
            return;
        }
        let before = self.snapshot();
        let tree = self.engine.tree_mut();
        let first = tree
            .name(nodes[0])
            .map(|n| n.to_owned())
            .unwrap_or_default();
        for &node in &nodes {
            // A node inside another deleted one is already gone.
            if tree.is_valid(node) {
                let _ = tree.queue_free(node);
            }
        }
        tree.flush_deletions();
        self.history.push(before);
        self.modified = true;
        self.prune_selection();
        if inside_instance {
            self.refuse_instance_contents();
        } else if nodes.len() == 1 {
            self.info(format!("Deleted {first}"));
        } else {
            self.info(format!("Deleted {} nodes", nodes.len()));
        }
    }

    /// Whether some of `nodes` are inside an instance (not nodes of this scene).
    fn any_instance_contents(&self, nodes: &[NodeHandle]) -> bool {
        nodes
            .iter()
            .any(|&n| n != self.scene_root && self.tree().is_valid(n) && !self.is_editable(n))
    }

    pub fn duplicate(&mut self, node: NodeHandle) {
        self.duplicate_nodes(&[node]);
    }

    /// Duplicates the selected nodes (one undo step) and selects the copies.
    pub fn duplicate_selected(&mut self) {
        let nodes = self.selected_roots();
        self.duplicate_nodes(&nodes);
    }

    /// Duplicates each of `nodes` next to itself (one undo step) and selects the copies.
    pub fn duplicate_nodes(&mut self, nodes: &[NodeHandle]) {
        let refused = self.any_instance_contents(nodes);
        let nodes: Vec<NodeHandle> = nodes
            .iter()
            .copied()
            .filter(|&n| n != self.scene_root && self.is_editable(n))
            .collect();
        if self.is_playing() {
            return;
        }
        if refused {
            self.refuse_instance_contents();
        }
        if nodes.is_empty() {
            return;
        }
        let before = self.snapshot();
        let mut copies = Vec::with_capacity(nodes.len());
        let mut active = None;
        let mut first = String::new();
        for &node in &nodes {
            let Some(parent) = self.tree().parent(node) else {
                continue;
            };
            let mut scene = save_scene(self.tree(), node);
            let base = scene.nodes[0].name.clone();
            scene.nodes[0].name = self.unique_name(parent, &base, None);
            let tree = self.engine.tree_mut();
            match instantiate_scene(tree, &scene, "duplicate") {
                Ok(copy) => {
                    let index = tree.children(parent).iter().position(|&c| c == node);
                    let _ = tree.add_child(parent, copy);
                    if let Some(index) = index {
                        let _ = tree.move_child(parent, copy, index + 1);
                    }
                    if self.selection == Some(node) {
                        active = Some(copy);
                    }
                    if copies.is_empty() {
                        first = base;
                    }
                    copies.push(copy);
                }
                Err(err) => {
                    self.fail(format!("{err}"));
                }
            }
        }
        if copies.is_empty() {
            return;
        }
        self.tree_mut().update_transforms();
        self.history.push(before);
        self.modified = true;
        let count = copies.len();
        self.set_selection(copies, active);
        if refused {
            self.refuse_instance_contents();
        } else if count == 1 {
            self.info(format!("Duplicated {first}"));
        } else {
            self.info(format!("Duplicated {count} nodes"));
        }
    }

    // =========================================================================================
    // Clipboard
    // =========================================================================================

    /// Copies the selected nodes (with their children). Returns the text for the system
    /// clipboard, or `None` when nothing can be copied.
    pub fn copy_selected(&mut self) -> Option<String> {
        let nodes: Vec<NodeHandle> = self
            .selected_roots()
            .into_iter()
            .filter(|&n| n != self.scene_root && self.is_editable(n))
            .collect();
        if nodes.is_empty() {
            return None;
        }
        let tree = self.tree();
        let items: Vec<_> = nodes
            .iter()
            .map(|&n| (save_scene(tree, n), tree.global_transform(n)))
            .collect();
        let text = Clipboard::to_text(&items);
        self.clipboard = Clipboard {
            items,
            text: text.clone(),
        };
        let count = nodes.len();
        self.info(crate::i18n::tf("status.copied", &[("count", &count)]));
        Some(text)
    }

    /// Copies, then deletes, the selected nodes.
    pub fn cut_selected(&mut self) -> Option<String> {
        if self.is_playing() {
            return None;
        }
        let text = self.copy_selected()?;
        let count = self.clipboard.items.len();
        self.delete_selected();
        self.info(crate::i18n::tf("status.cut", &[("count", &count)]));
        Some(text)
    }

    /// Whether [`Editor::paste`] has something to paste (`system`: the system clipboard's
    /// text, when known).
    pub fn can_paste(&self, system: Option<&str>) -> bool {
        !self.is_playing()
            && (!self.clipboard.is_empty() || system.and_then(Clipboard::parse).is_some())
    }

    /// Pastes copied nodes under `parent` (the selection's parent when `None`), as one undo
    /// step, and selects them. `text` is the system clipboard's text: nodes copied in
    /// another editor window come from it. `at` moves the first pasted node there (the rest
    /// keep their offsets to it); otherwise they land where they were copied from.
    pub fn paste(&mut self, text: Option<&str>, parent: Option<NodeHandle>, at: Option<Vec3>) {
        if self.is_playing() {
            return;
        }
        let items = match text {
            Some(text) if text != self.clipboard.text => match Clipboard::parse(text) {
                Some(scenes) => scenes.into_iter().map(|s| (s, None)).collect(),
                None => self.clipboard.items.clone(),
            },
            _ => self.clipboard.items.clone(),
        };
        if items.is_empty() {
            self.fail(crate::i18n::t("status.nothing_to_paste"));
            return;
        }
        let parent = match parent {
            Some(p) if !self.can_add_children(p) => {
                self.refuse_instance_contents();
                return;
            }
            Some(p) => p,
            None => self.paste_parent(),
        };
        let anchor = items
            .first()
            .and_then(|(_, global)| *global)
            .map(|g| Vec3::from(g.translation));
        let mut pasted = Vec::new();
        self.as_one_step(|editor| {
            for (scene, global) in items {
                let Some(node) = editor.add_scene_under(scene, parent) else {
                    continue;
                };
                if let Some(mut global) = global {
                    if let (Some(at), Some(anchor)) = (at, anchor) {
                        global.translation += ab_engine::core::Vec3A::from(at - anchor);
                    }
                    editor.set_global_transform(node, global);
                } else if let Some(at) = at.filter(|_| pasted.is_empty()) {
                    let _ = editor.tree_mut().set_global_position(node, at);
                }
                pasted.push(node);
            }
        });
        // Placing the nodes opened a gesture: the next edit is a step of its own.
        self.end_gesture();
        if pasted.is_empty() {
            return;
        }
        self.tree_mut().update_transforms();
        let count = pasted.len();
        let active = pasted.first().copied();
        self.set_selection(pasted, active);
        self.info(crate::i18n::tf("status.pasted", &[("count", &count)]));
    }

    /// Where Paste puts nodes: beside the active node (in its parent), else in the scene root.
    fn paste_parent(&self) -> NodeHandle {
        self.selection
            .filter(|&n| n != self.scene_root && self.is_editable(n))
            .and_then(|n| self.tree().parent(n))
            .filter(|&p| self.is_editable(p) && !self.is_instance(p))
            .unwrap_or(self.scene_root)
    }

    // =========================================================================================
    // Building
    // =========================================================================================

    /// The build tool's ghost of the current piece, made (or turned) as needed: its node
    /// and its bounds around its origin. `None` without a piece, or when it cannot load.
    pub fn build_ghost(&mut self) -> Option<(NodeHandle, ab_engine::rendering::Aabb)> {
        use ab_engine::rendering::Bounds;
        let piece = self.build.clone()?;
        if let Some((built, node, bounds)) = &self.ghost {
            if *built == piece && self.tree().is_valid(*node) {
                return Some((*node, *bounds));
            }
        }
        self.remove_ghost();
        let tree = self.engine.tree_mut();
        let scene = SceneFile {
            environment: None,
            nodes: vec![NodeDesc {
                name: "BuildGhost".into(),
                instance: Some(piece.path.clone()),
                rotation: Some([0.0, f32::from(piece.turns % 4) * 90.0, 0.0]),
                ..Default::default()
            }],
        };
        let node = match instantiate_scene(tree, &scene, "build tool") {
            Ok(node) => node,
            Err(err) => {
                self.build = None;
                self.fail(format!("{err}"));
                return None;
            }
        };
        let root = tree.root();
        let _ = tree.add_child(root, node);
        tree.update_transforms();
        let bounds = tree
            .subtree_bounds(node)
            .unwrap_or(ab_engine::rendering::Aabb {
                min: Vec3::splat(-0.5),
                max: Vec3::splat(0.5),
            });
        self.ghost = Some((piece, node, bounds));
        Some((node, bounds))
    }

    /// Takes the build tool's ghost out of the world.
    pub fn remove_ghost(&mut self) {
        if let Some((_, node, _)) = self.ghost.take() {
            let tree = self.engine.tree_mut();
            if tree.is_valid(node) {
                let _ = tree.queue_free(node);
                tree.flush_deletions();
            }
        }
    }

    /// Places the build tool's piece: an instance of `piece` in the scene root, at
    /// `position`, turned by quarter turns. Selects it; one undo step.
    pub fn place_piece(&mut self, piece: &BuildPiece, position: Vec3) -> Option<NodeHandle> {
        let stem = std::path::Path::new(&piece.path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Piece".into());
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
        let degrees = f32::from(piece.turns % 4) * 90.0;
        let desc = NodeDesc {
            name,
            instance: Some(piece.path.clone()),
            position: Some(position.to_array()),
            rotation: (degrees != 0.0).then_some([0.0, degrees, 0.0]),
            ..Default::default()
        };
        let scene = SceneFile {
            environment: None,
            nodes: vec![desc],
        };
        let root = self.scene_root;
        let node = self.add_scene_under(scene, root)?;
        self.end_gesture();
        Some(node)
    }

    // =========================================================================================
    // Arranging
    // =========================================================================================

    /// World bounds of `node` (its meshes, else its collision shapes, else a point).
    pub fn world_bounds(&self, node: NodeHandle) -> Option<ab_engine::rendering::Aabb> {
        use ab_engine::rendering::{Aabb, Bounds};
        let tree = self.tree();
        tree.subtree_bounds(node)
            .or_else(|| tree.global_position(node).map(|p| Aabb { min: p, max: p }))
    }

    /// The selected roots that can be moved, with their bounds.
    fn arrangeable(&self) -> Vec<(NodeHandle, ab_engine::rendering::Aabb)> {
        self.selected_roots()
            .into_iter()
            .filter(|&n| n != self.scene_root && self.can_edit_properties(n))
            .filter_map(|n| Some((n, self.world_bounds(n)?)))
            .collect()
    }

    fn shift(&mut self, node: NodeHandle, offset: Vec3) {
        if offset.length_squared() < 1e-12 {
            return;
        }
        if let Some(mut global) = self.tree().global_transform(node) {
            global.translation += ab_engine::core::Vec3A::from(offset);
            self.set_global_transform(node, global);
        }
    }

    /// Lines the selection up on `axis` (0 = X, 1 = Y, 2 = Z) with the active object: by
    /// their low sides, centres or high sides. One undo step.
    pub fn align_selected(&mut self, axis: usize, align: Align) {
        let items = self.arrangeable();
        let Some(&(_, target)) = items
            .iter()
            .find(|(n, _)| Some(*n) == self.selection)
            .or(items.first())
        else {
            return;
        };
        let side = |b: &ab_engine::rendering::Aabb| match align {
            Align::Min => b.min[axis],
            Align::Center => b.center()[axis],
            Align::Max => b.max[axis],
        };
        let goal = side(&target);
        self.as_one_step(|editor| {
            for (node, bounds) in &items {
                let mut offset = Vec3::ZERO;
                offset[axis] = goal - side(bounds);
                editor.shift(*node, offset);
            }
        });
        self.end_gesture();
    }

    /// Spaces the selection evenly on `axis` between its two outermost objects (equal gaps
    /// between their bounds). One undo step.
    pub fn distribute_selected(&mut self, axis: usize) {
        let mut items = self.arrangeable();
        if items.len() < 3 {
            self.fail("Select three objects or more to distribute them");
            return;
        }
        items.sort_by(|a, b| a.1.center()[axis].total_cmp(&b.1.center()[axis]));
        let start = items[0].1.max[axis];
        let end = items[items.len() - 1].1.min[axis];
        let inner: f32 = items[1..items.len() - 1]
            .iter()
            .map(|(_, b)| b.size()[axis])
            .sum();
        let gap = (end - start - inner) / (items.len() - 1) as f32;
        self.as_one_step(|editor| {
            let mut at = start + gap;
            for (node, bounds) in &items[1..items.len() - 1] {
                let mut offset = Vec3::ZERO;
                offset[axis] = at - bounds.min[axis];
                editor.shift(*node, offset);
                at += bounds.size()[axis] + gap;
            }
        });
        self.end_gesture();
    }

    /// Puts each selected object down on what is below it: the highest top of the objects
    /// under it, or the ground (y = 0); one sunk into the ground comes up onto it. One undo
    /// step.
    pub fn drop_selected(&mut self) {
        let items = self.arrangeable();
        let others: Vec<ab_engine::rendering::Aabb> = self
            .tree()
            .children(self.scene_root)
            .into_iter()
            .filter(|n| !items.iter().any(|(i, _)| i == n))
            .filter_map(|n| self.world_bounds(n))
            .collect();
        self.as_one_step(|editor| {
            for (node, bounds) in &items {
                let floor = others
                    .iter()
                    .filter(|o| {
                        o.max.y <= bounds.min.y + 1e-3
                            && o.min.x < bounds.max.x
                            && o.max.x > bounds.min.x
                            && o.min.z < bounds.max.z
                            && o.max.z > bounds.min.z
                    })
                    .map(|o| o.max.y)
                    .fold(0.0_f32, f32::max);
                editor.shift(*node, Vec3::new(0.0, floor - bounds.min.y, 0.0));
            }
        });
        self.end_gesture();
    }

    /// Makes `count` copies of each selected object in a row along `axis`, each its own
    /// size (plus `gap`) further: walls, fences, rows of chairs. One undo step; the copies
    /// are selected.
    pub fn duplicate_in_row(&mut self, axis: usize, count: usize, gap: f32) {
        let items = self.arrangeable();
        let mut copies = Vec::new();
        self.as_one_step(|editor| {
            for (node, bounds) in &items {
                let Some(parent) = editor.tree().parent(*node) else {
                    continue;
                };
                let step = bounds.size()[axis] + gap;
                let scene = save_scene(editor.tree(), *node);
                let Some(global) = editor.tree().global_transform(*node) else {
                    continue;
                };
                for i in 1..=count {
                    let Some(copy) = editor.add_scene_under(scene.clone(), parent) else {
                        break;
                    };
                    let mut moved = global;
                    let mut offset = Vec3::ZERO;
                    offset[axis] = step * i as f32;
                    moved.translation += ab_engine::core::Vec3A::from(offset);
                    editor.set_global_transform(copy, moved);
                    copies.push(copy);
                }
            }
        });
        self.end_gesture();
        let active = copies.last().copied();
        self.set_selection(copies, active);
    }

    /// Turns what a generator made into ordinary nodes of the scene (a group in its place),
    /// to edit them by hand. Refused for meshes the scene cannot describe (a terrain).
    pub fn bake_generator(&mut self, node: NodeHandle) -> Result<NodeHandle, String> {
        use ab_engine::scene_file::{generator::GeneratorNode, Generated, SceneOrigin};
        let tree = self.tree();
        if tree.component::<GeneratorNode>(node).is_none() || !self.is_editable(node) {
            return Err("select a generator of this scene".into());
        }
        let built = tree
            .children(node)
            .into_iter()
            .find(|&c| tree.component::<Generated>(c).is_some())
            .ok_or("the generator made nothing")?;
        let made = tree.children(built);
        let generated_mesh = made.iter().any(|&m| {
            tree.subtree(m).into_iter().any(|n| {
                tree.component::<MeshInstance3D>(n).is_some()
                    && tree.component::<SceneOrigin>(n).is_none_or(|o| o.mesh.is_none())
                    && self.instance_owner(n) == Some(n)
            })
        });
        if generated_mesh {
            return Err("this generator makes meshes (a terrain) that only it can make: it cannot be baked".into());
        }
        let scenes: Vec<SceneFile> = made.iter().map(|&m| save_scene(tree, m)).collect();
        let parent = tree.parent(node).ok_or("no parent")?;
        let name = tree.name(node).map(|n| n.to_owned()).unwrap_or_default();
        let transform = tree.transform(node);
        let mut group = None;
        self.as_one_step(|editor| {
            editor.delete_nodes(&[node]);
            let desc = NodeDesc {
                name: name.clone(),
                kind: Some("Node3D".into()),
                ..Default::default()
            };
            let Some(g) = editor.add_scene_under(SceneFile { environment: None, nodes: vec![desc] }, parent) else {
                return;
            };
            if let Some(t) = transform {
                let _ = editor.tree_mut().set_transform(g, t);
            }
            for scene in scenes {
                editor.add_scene_under(scene, g);
            }
            group = Some(g);
        });
        let group = group.ok_or("the group could not be made")?;
        self.tree_mut().update_transforms();
        self.select(Some(group));
        self.info(format!("Baked {name}: its parts are ordinary nodes now"));
        Ok(group)
    }

    // =========================================================================================
    // Prefabs
    // =========================================================================================

    /// The instance's prefab file and how many overrides it has, for a prefab instance.
    #[cfg(test)]
    pub fn prefab_of(&self, node: NodeHandle) -> Option<(String, usize)> {
        let tree = self.tree();
        if !ab_engine::scene_file::prefab::is_prefab_instance(tree, node) {
            return None;
        }
        let desc = self.describe_node(node)?;
        Some((desc.instance?, desc.overrides.len()))
    }

    /// Drops the instance's overrides: it becomes the prefab as it is (one undo step).
    pub fn revert_overrides(&mut self, instance: NodeHandle) {
        let Some(mut desc) = self.describe_node(instance) else {
            return;
        };
        if desc.overrides.is_empty() || !self.is_editable(instance) {
            return;
        }
        desc.overrides.clear();
        self.begin_edit();
        self.rebuild_node(instance, desc);
        self.end_gesture();
        self.info("Overrides reverted");
    }

    /// Writes the instance as it is now into its prefab file; every instance of it in this
    /// scene is rebuilt from the new file, keeping its own overrides.
    pub fn apply_to_prefab(&mut self, instance: NodeHandle) -> Result<(), String> {
        use ab_engine::scene_file::{prefab, save_contents, SceneInstance};
        let tree = self.tree();
        let path = tree
            .component::<SceneInstance>(instance)
            .map(|i| i.0.clone())
            .ok_or("not an instance")?;
        let prefab_now = prefab::read_prefab(tree, &path)?;
        let mut contents = save_contents(tree, instance);
        // The prefab keeps its own root name and placement.
        let root = contents.nodes.first_mut().ok_or("empty instance")?;
        let original = prefab_now.nodes.first().ok_or("empty prefab")?;
        root.name = original.name.clone();
        root.position = original.position;
        root.rotation = original.rotation;
        root.scale = original.scale;
        contents.environment = prefab_now.environment.clone();
        // The other instances, described against the prefab as it was.
        let others: Vec<(NodeHandle, NodeDesc)> = tree
            .subtree(self.scene_root)
            .into_iter()
            .filter(|&n| {
                n != instance
                    && tree
                        .component::<SceneInstance>(n)
                        .is_some_and(|i| i.0 == path)
            })
            .filter_map(|n| Some((n, self.describe_node(n)?)))
            .collect();
        let mut own = self.describe_node(instance).ok_or("not described")?;
        own.overrides.clear();
        self.write_project_file(&self.project().join(&path), contents.to_toml())
            .map_err(|e| format!("{path}: {e}"))?;
        self.begin_edit();
        self.rebuild_node(instance, own);
        for (node, desc) in others {
            self.rebuild_node(node, desc);
        }
        self.end_gesture();
        self.assets.request_rescan();
        self.info(format!("Saved the instance into {path}"));
        Ok(())
    }

    /// Saves `node` and its children as a prefab file at `path` (relative to the project)
    /// and puts an instance of it in its place. Returns the instance.
    pub fn create_prefab(&mut self, node: NodeHandle, path: &str) -> Result<NodeHandle, String> {
        if node == self.scene_root || !self.is_editable(node) || self.is_instance(node) {
            return Err("pick a node of this scene (not an instance) to make a prefab of".into());
        }
        let path = ab_engine::scene_file::with_scene_extension(path.trim());
        let full = self.project().join(&path);
        if full.exists() {
            return Err(format!("{path} already exists"));
        }
        let mut prefab = save_scene(self.tree(), node);
        let root = prefab.nodes.first_mut().ok_or("nothing to save")?;
        // The instance keeps the placement; the prefab is at its own origin.
        let (position, rotation, scale) = (
            root.position.take(),
            root.rotation.take(),
            root.scale.take(),
        );
        self.write_project_file(&full, prefab.to_toml())
            .map_err(|e| format!("{path}: {e}"))?;
        let tree = self.tree();
        let parent = tree.parent(node).ok_or("no parent")?;
        let index = tree.children(parent).iter().position(|&c| c == node);
        let name = tree.name(node).map(|n| n.to_owned()).unwrap_or_default();
        let desc = NodeDesc {
            name: name.clone(),
            instance: Some(path.clone()),
            position,
            rotation,
            scale,
            ..Default::default()
        };
        let mut created = None;
        self.as_one_step(|editor| {
            editor.delete_nodes(&[node]);
            let scene = SceneFile {
                environment: None,
                nodes: vec![desc],
            };
            created = editor.add_scene_under(scene, parent);
            if let (Some(instance), Some(index)) = (created, index) {
                let tree = editor.tree_mut();
                let _ = tree.set_name(instance, &name);
                let _ = tree.move_child(parent, instance, index);
            }
        });
        self.assets.request_rescan();
        let instance = created.ok_or("the prefab did not load")?;
        self.info(format!("Created prefab {path}"));
        Ok(instance)
    }

    pub fn rename(&mut self, node: NodeHandle, name: &str) {
        let name = name.trim();
        if name.is_empty() || name.contains(['/', ':', '.']) {
            return;
        }
        let Some(parent) = self.tree().parent(node) else {
            return;
        };
        let name = self.unique_name(parent, name, Some(node));
        if self.tree().name(node).is_some_and(|n| *n == name) {
            return;
        }
        self.begin_edit();
        let _ = self.tree_mut().set_name(node, &name);
    }

    /// Moves `node` under `new_parent` (keeping where it is in the world), at `index` among
    /// the new siblings when given.
    pub fn reparent(&mut self, node: NodeHandle, new_parent: NodeHandle, index: Option<usize>) {
        if self.is_playing() || node == self.scene_root || node == new_parent {
            return;
        }
        if self.tree().is_valid(node)
            && self.tree().is_valid(new_parent)
            && (!self.is_editable(node) || !self.can_add_children(new_parent))
        {
            self.refuse_instance_contents();
            return;
        }
        if !self.is_editable(node)
            || !self.can_add_children(new_parent)
            || self.tree().is_ancestor_of(node, new_parent)
        {
            return;
        }
        let before = self.snapshot();
        let same_parent = self.tree().parent(node) == Some(new_parent);
        if !same_parent {
            let base = self
                .tree()
                .name(node)
                .map(|n| n.to_owned())
                .unwrap_or_default();
            let name = self.unique_name(new_parent, &base, Some(node));
            let tree = self.tree_mut();
            let _ = tree.set_name(node, &name);
            if let Err(err) = tree.reparent(node, new_parent, true) {
                self.fail(format!("{err}"));
                return;
            }
        }
        if let Some(index) = index {
            let _ = self.tree_mut().move_child(new_parent, node, index);
        }
        self.tree_mut().update_transforms();
        self.history.push(before);
        self.modified = true;
    }

    /// Moves `node` one place up (`-1`) or down (`1`) among its siblings.
    pub fn move_sibling(&mut self, node: NodeHandle, step: isize) {
        if self.is_playing() {
            return;
        }
        let Some(parent) = self.tree().parent(node) else {
            return;
        };
        let children = self.tree().children(parent);
        let Some(index) = children.iter().position(|&c| c == node) else {
            return;
        };
        let target = index as isize + step;
        if target < 0 || target as usize >= children.len() || !self.is_editable(node) {
            return;
        }
        let before = self.snapshot();
        let _ = self.tree_mut().move_child(parent, node, target as usize);
        self.history.push(before);
        self.modified = true;
    }

    pub fn set_transform(&mut self, node: NodeHandle, transform: Transform3D) {
        if self.tree().transform(node) == Some(transform) {
            return;
        }
        if !self.is_playing() {
            self.begin_edit();
        }
        let tree = self.tree_mut();
        let _ = tree.set_transform(node, transform);
        tree.update_transforms();
    }

    pub fn set_global_transform(&mut self, node: NodeHandle, global: ab_engine::core::Affine3A) {
        if !self.is_playing() {
            self.begin_edit();
        }
        let tree = self.tree_mut();
        let _ = tree.set_global_transform(node, global);
        tree.update_transforms();
    }

    /// The node's own description (without children).
    pub fn describe_node(&self, node: NodeHandle) -> Option<NodeDesc> {
        if !self.tree().is_valid(node) {
            return None;
        }
        let mut desc = save_scene(self.tree(), node).nodes.into_iter().next()?;
        desc.parent = None;
        Some(desc)
    }

    /// Applies an edited description of `node`: cheap changes in place, the rest by
    /// rebuilding the node (its children move to the rebuilt node).
    pub fn apply_desc(&mut self, node: NodeHandle, old: &NodeDesc, new: NodeDesc) {
        if *old == new || !self.can_edit_properties(node) {
            return;
        }
        self.begin_edit();
        // A prefab's node keeps its name: overrides find it by its path.
        if new.name != old.name && self.is_editable(node) {
            self.rename(node, &new.name);
        }
        let common = |d: &NodeDesc| NodeDesc {
            name: String::new(),
            position: None,
            rotation: None,
            scale: None,
            visible: None,
            groups: Vec::new(),
            script: None,
            scripts: Vec::new(),
            particles: None,
            canvas: None,
            control: None,
            label: None,
            button: None,
            panel: None,
            progress: None,
            box_container: None,
            image: None,
            ..d.clone()
        };
        let tree = self.engine.tree_mut();
        if new.visible != old.visible {
            let _ = tree.set_visible(node, new.visible.unwrap_or(true));
        }
        if new.groups != old.groups {
            for group in &old.groups {
                tree.remove_from_group(node, group);
            }
            for group in &new.groups {
                let _ = tree.add_to_group(node, group);
            }
        }
        let scripts = new.script_list();
        if scripts != old.script_list() {
            tree.remove_script(node);
            for name in &scripts {
                let _ = tree.add_script_named(node, name);
            }
        }
        if scripts != old.script_list() || new.script_properties != old.script_properties {
            for name in &scripts {
                let properties = new.script_properties.get(name).cloned().unwrap_or_default();
                tree.set_script_properties(node, name, properties);
            }
        }
        if new.particles != old.particles {
            if let Some(p) = &new.particles {
                match particle_emitter(p) {
                    Ok(fresh) => {
                        if let Some(mut emitter) = tree.component_mut::<ParticleEmitter>(node) {
                            // Keep the particles alive: only settings and flags change.
                            emitter.settings = fresh.settings;
                            emitter.one_shot = fresh.one_shot;
                            emitter.local_coords = fresh.local_coords;
                            if emitter.emitting != fresh.emitting {
                                emitter.emitting = fresh.emitting;
                            }
                        }
                        if let Some(mut origin) = tree.component_mut::<SceneOrigin>(node) {
                            origin.particles = Some(p.clone());
                        }
                    }
                    Err(err) => {
                        self.fail(err);
                        return;
                    }
                }
            }
        }
        // UI widgets update in place (a drag in the viewport changes them every frame).
        let ui_changed =
            ab_engine::scene_file::ui::ui_tables(old) != ab_engine::scene_file::ui::ui_tables(&new);
        if ui_changed {
            let tree = self.engine.tree_mut();
            let in_place = tree.component::<ab_engine::ui::Control>(node).is_some()
                || tree.class_name(node) == Some(ab_engine::ui::class::CANVAS_LAYER);
            if !in_place {
                self.rebuild_node(node, new);
                return;
            }
            if let Err(err) = ab_engine::scene_file::ui::apply(tree, node, &new) {
                self.fail(err);
                return;
            }
        }
        if common(old) != common(&new) {
            self.rebuild_node(node, new);
        }
    }

    fn rebuild_node(&mut self, node: NodeHandle, mut desc: NodeDesc) {
        let tree = self.engine.tree_mut();
        let Some(parent) = tree.parent(node) else {
            return;
        };
        desc.parent = None;
        // Keep the current transform/name (possibly changed this frame).
        desc.name = tree.name(node).map(|n| n.to_owned()).unwrap_or(desc.name);
        let transform = tree.transform(node);
        let scene = SceneFile {
            environment: None,
            nodes: vec![desc],
        };
        let fresh = match instantiate_scene(tree, &scene, "edit") {
            Ok(fresh) => fresh,
            Err(err) => {
                self.fail(format!("{err}"));
                return;
            }
        };
        if let Some(transform) = transform {
            let _ = tree.set_transform(fresh, transform);
        }
        let index = tree.children(parent).iter().position(|&c| c == node);
        // A humanoid ragdoll's bodies are made from its rig, an instance's contents from
        // its file: rebuilt, not moved over.
        let rebuilt = tree
            .component::<ab_engine::ragdoll::RagdollRig>(node)
            .is_some()
            || tree
                .component::<ab_engine::scene_file::SceneInstance>(node)
                .is_some();
        if !rebuilt {
            // What the node built is built again by the fresh node.
            let kept: Vec<NodeHandle> = tree
                .children(node)
                .into_iter()
                .filter(|&c| {
                    tree.component::<ab_engine::scene_file::Generated>(c)
                        .is_none()
                })
                .collect();
            for child in kept {
                let _ = tree.reparent(child, fresh, false);
            }
        }
        let _ = tree.queue_free(node);
        tree.flush_deletions();
        let _ = tree.add_child(parent, fresh);
        ab_engine::scene_file::generator::rebuild_grounded(tree, fresh);
        let failed = ab_engine::ragdoll::build_rigs(tree, fresh);
        // An AnimationPlayer finds its targets now that it has a parent again.
        let animation = tree
            .component::<ab_engine::scene_file::SceneOrigin>(fresh)
            .and_then(|o| o.animation.clone());
        if let Some(animation) = animation {
            if let Err(err) = ab_engine::scene_file::load_animations(tree, fresh, &animation) {
                warn!("{err}");
            }
        }
        if let Some(index) = index {
            let _ = tree.move_child(parent, fresh, index);
        }
        tree.update_transforms();
        if let Some((_, err)) = failed.first() {
            self.fail(format!("Ragdoll: {err}"));
        }
        for selected in &mut self.selected {
            if *selected == node {
                *selected = fresh;
            }
        }
        if self.selection == Some(node) {
            self.selection = Some(fresh);
        }
    }

    pub fn set_environment(&mut self, desc: EnvironmentDesc) {
        if desc == self.environment {
            return;
        }
        self.begin_edit();
        self.environment = desc;
        let tree = self.engine.tree_mut();
        tree.world_mut().insert_resource(Environment::default());
        if let Err(err) = apply_environment(tree, &self.environment) {
            self.fail(err);
        }
    }

    // =========================================================================================
    // Play mode
    // =========================================================================================

    /// Reads the project's config file into the engine (each Play starts from the file's
    /// values). Returns the problem, if any; the last good values stay.
    pub fn load_config(&mut self) -> Option<String> {
        let path = self.config_path.clone()?;
        let loaded = std::fs::read_to_string(&path)
            .map_err(|e| format!("{}: {e}", path.display()))
            .and_then(|text| {
                ab_engine::game_config::GameConfig::from_toml(&text).map_err(|e| e.to_string())
            });
        match loaded {
            Ok(config) => {
                self.tree_mut().world_mut().insert_resource(config);
                None
            }
            Err(err) => {
                warn!("{err}");
                Some(err)
            }
        }
    }

    pub fn play(&mut self) {
        if self.is_playing() {
            return;
        }
        self.build = None;
        self.remove_ghost();
        if let Some(err) = self.load_config() {
            self.fail(format!("config: {err}"));
        }
        let snapshot = self.snapshot();
        let environment = self
            .tree()
            .world()
            .resource::<Environment>()
            .map(|e| e.clone())
            .unwrap_or_default();
        let missing: Vec<String> = snapshot
            .scene
            .nodes
            .iter()
            .flat_map(|n| n.script_list())
            .filter(|name| !self.play_registry.contains(name))
            .collect();
        if !missing.is_empty() {
            warn!(
                "scripts not loaded (they will do nothing): {} — start the editor with \
                 --scripts <game library>",
                missing.join(", ")
            );
        }
        let registry = self.play_registry.clone();
        self.tree_mut().world_mut().insert_resource(registry);
        if let Err(err) = self.replace_scene(&snapshot.scene) {
            let registry = self.edit_registry.clone();
            self.tree_mut().world_mut().insert_resource(registry);
            self.fail(format!("cannot start: {err:#}"));
            return;
        }
        self.restore_selection(&snapshot);
        if let Some(mut profiler) = self.tree().world().resource_mut::<Profiler>() {
            profiler.reset();
        }
        if let Some(mut tracer) = self.tree().world().resource_mut::<Tracer>() {
            tracer.restart();
        }
        self.play_snapshot = Some((snapshot, environment));
        self.mode = Mode::Play { paused: false };
        self.info("Playing — Shift+F5 to stop");
    }

    /// Advances a paused game by one 60 Hz frame.
    pub fn step_frame(&mut self) {
        if self.mode == (Mode::Play { paused: true }) {
            self.engine.update(1.0 / 60.0);
        }
    }

    pub fn toggle_pause(&mut self) {
        if let Mode::Play { paused } = self.mode {
            self.mode = Mode::Play { paused: !paused };
        }
    }

    pub fn stop(&mut self) {
        let Some((snapshot, environment)) = self.play_snapshot.take() else {
            return;
        };
        self.mode = Mode::Edit;
        // Keep script versions hot reload brought in while playing.
        let current = self
            .tree()
            .world()
            .resource::<ScriptRegistry>()
            .map(|r| r.clone());
        if let Some(registry) = current {
            self.play_registry = registry;
        }
        let tree = self.engine.tree_mut();
        for child in tree.children(tree.root()) {
            let _ = tree.queue_free(child);
        }
        tree.flush_deletions();
        let world = tree.world_mut();
        // Stop sounds of the freed nodes.
        audio_system(world);
        world.insert_resource(InputState::new());
        world.insert_resource(self.edit_registry.clone());
        if let Some(mut time) = world.resource_mut::<Time>() {
            time.time_scale = 1.0;
        }
        if let Some(mut debug) = world.resource_mut::<DebugDraw>() {
            debug.clear();
        }
        if let Some(mut control) = world.resource_mut::<EngineControl>() {
            control.set_mouse_captured(false);
        }
        self.restore(&snapshot);
        self.tree_mut().world_mut().insert_resource(environment);
        self.info("Stopped");
    }

    // =========================================================================================
    // Frame
    // =========================================================================================

    /// Advances the scene by `delta`: the whole engine in play mode, only effect previews
    /// while editing.
    pub fn tick(&mut self, delta: f32) {
        if self.is_playing() {
            self.build = None;
        }
        if self.build.is_none() && self.ghost.is_some() {
            self.remove_ghost();
        }
        match self.mode {
            Mode::Play { paused: false } => self.engine.update(delta),
            Mode::Play { paused: true } => {}
            Mode::Edit => {
                // A rebuilt script library (a script was created or saved): new versions for
                // Play mode, while the edited scene keeps its inert stand-ins.
                match self.engine.poll_script_library() {
                    Some(Ok(fresh)) => {
                        let count = fresh.len();
                        self.play_registry.extend(fresh);
                        self.info(format!("Scripts reloaded ({count} scripts)"));
                    }
                    Some(Err(err)) => self.fail(format!("cannot load the new scripts: {err:#}")),
                    None => {}
                }
                let world = self.engine.tree_mut().world_mut();
                if let Some(mut time) = world.resource_mut::<Time>() {
                    time.begin_frame(if self.preview_effects { delta } else { 0.0 });
                }
                // Scripts are inert stand-ins; this propagates transforms and frees nodes.
                self.engine.tree_mut().process(0.0);
                effects_system(self.engine.tree_mut().world_mut());
                if let Some(mut time) = self.engine.tree().world().resource_mut::<Time>() {
                    time.end_frame();
                }
            }
        }
        self.prune_selection();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> Editor {
        let project = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        Editor::new(project, None, AudioOutput::Silent)
    }

    fn cube() -> NodeDesc {
        SceneFile::from_toml(
            "[[node]]\nname = \"Cube\"\ntype = \"MeshInstance3D\"\nmesh = { primitive = \"cube\" }\n",
        )
        .unwrap()
        .nodes
        .remove(0)
    }

    /// Three cubes at the scene root; nothing selected.
    fn three_cubes(editor: &mut Editor) -> [NodeHandle; 3] {
        let nodes = std::array::from_fn(|_| {
            editor.select(None);
            editor.add_node(cube(), None);
            editor.selection.unwrap()
        });
        editor.select(None);
        nodes
    }

    /// What is added under an instance, moved into it or deleted inside it is not saved
    /// (an instance is saved as its file): refused with a message, the scene unchanged.
    #[test]
    fn instance_contents_cannot_gain_or_lose_nodes() {
        let dir = std::env::temp_dir().join(format!("ab-prefab-contents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut editor = Editor::new(dir.clone(), None, AudioOutput::Silent);
        editor.new_scene();
        editor.select(None);
        editor.add_scene(crate::create::PHYSICS[0].scene(), None);
        let wall = editor.selection.unwrap();
        let instance = editor.create_prefab(wall, "prefabs/wall").unwrap();
        let mesh = editor.tree().get_node(instance, "Mesh").unwrap();
        editor.select(None);
        editor.add_node(cube(), None);
        let cube_node = editor.selection.unwrap();
        editor.end_gesture();
        let before = editor.describe();
        let steps = editor.history.undo_len();
        let refused = |editor: &Editor| {
            editor
                .status
                .as_ref()
                .is_some_and(|s| s.error && s.text == crate::i18n::t("status.instance_contents"))
        };

        editor.delete(mesh);
        assert!(refused(&editor), "delete inside");
        editor.status = None;
        editor.reparent(cube_node, mesh, None);
        assert!(refused(&editor), "move inside");
        editor.status = None;
        editor.select(Some(cube_node));
        editor.copy_selected();
        editor.paste(None, Some(mesh), None);
        assert!(refused(&editor), "paste inside");
        editor.status = None;
        editor.duplicate(mesh);
        assert!(refused(&editor), "duplicate inside");
        // Rows of copies of a part of a prefab would go inside it.
        editor.set_selection(vec![mesh], Some(mesh));
        editor.duplicate_in_row(0, 2, 0.0);
        assert_eq!(editor.describe(), before, "nothing changed");
        assert_eq!(editor.history.undo_len(), steps, "no undo steps");
        assert!(editor.tree().is_valid(mesh));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prefab_workflow_create_override_apply_revert() {
        let dir = std::env::temp_dir().join(format!("ab-prefab-editor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut editor = Editor::new(dir.clone(), None, AudioOutput::Silent);
        editor.new_scene();
        editor.select(None);
        editor.add_scene(
            crate::create::PHYSICS[0].scene(),
            Some(Vec3::new(2.0, 0.0, 0.0)),
        );
        let wall = editor.selection.unwrap();
        let instance = editor.create_prefab(wall, "prefabs/wall").unwrap();
        assert!(dir.join("prefabs/wall.abscene").is_file());
        assert_eq!(
            editor.tree().global_position(instance),
            Some(Vec3::new(2.0, 0.0, 0.0))
        );
        assert_eq!(
            editor.prefab_of(instance),
            Some(("prefabs/wall.abscene".into(), 0))
        );
        editor.end_gesture();

        // A second instance, then a change inside the first: an override of the first only.
        editor.select(None);
        editor.add_node(
            NodeDesc {
                name: "Wall2".into(),
                instance: Some("prefabs/wall.abscene".into()),
                ..Default::default()
            },
            None,
        );
        let second = editor.selection.unwrap();
        let mesh = editor.tree().get_node(instance, "Mesh").unwrap();
        assert!(editor.is_override_target(mesh) && !editor.is_editable(mesh));
        assert!(editor.can_edit_properties(mesh));
        let old = editor.describe_node(mesh).unwrap();
        let mut new = old.clone();
        new.visible = Some(false);
        new.name = "Renamed".into();
        editor.apply_desc(mesh, &old, new);
        editor.end_gesture();
        let instance = editor.node_at("Wall").unwrap();
        assert!(
            editor.tree().get_node(instance, "Mesh").is_some(),
            "prefab nodes keep their names"
        );
        assert_eq!(editor.prefab_of(instance).unwrap().1, 1);
        assert_eq!(editor.prefab_of(second).unwrap().1, 0);

        // Revert: back to the prefab.
        editor.revert_overrides(instance);
        let instance = editor.node_at("Wall").unwrap();
        assert_eq!(editor.prefab_of(instance).unwrap().1, 0);
        editor.undo();
        let instance = editor.node_at("Wall").unwrap();
        assert_eq!(
            editor.prefab_of(instance).unwrap().1,
            1,
            "undo brings it back"
        );

        // Apply: the prefab file changes, both instances show it, neither has overrides.
        editor.apply_to_prefab(instance).unwrap();
        let text = std::fs::read_to_string(dir.join("prefabs/wall.abscene")).unwrap();
        assert!(text.contains("visible = false"), "{text}");
        for name in ["Wall", "Wall2"] {
            let node = editor.node_at(name).unwrap();
            assert_eq!(editor.prefab_of(node).unwrap().1, 0, "{name}");
            let mesh = editor.tree().get_node(node, "Mesh").unwrap();
            assert!(!editor.tree().is_visible(mesh), "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn align_distribute_drop_and_rows() {
        let mut editor = editor();
        let [a, b, c] = three_cubes(&mut editor);
        let at = |editor: &Editor, n| editor.tree().global_position(n).unwrap();
        for (node, x, y) in [(a, 0.0, 0.0), (b, 3.0, 2.0), (c, 10.0, 5.0)] {
            editor.set_transform(node, Transform3D::from_translation(Vec3::new(x, y, 1.0)));
        }
        editor.end_gesture();
        editor.set_selection(vec![a, b, c], Some(a));
        editor.align_selected(1, Align::Min);
        assert!([a, b, c]
            .iter()
            .all(|&n| (at(&editor, n).y - 0.0).abs() < 1e-4));
        editor.distribute_selected(0);
        // Unit cubes from x = 0 to 10: equal gaps of 4 between them.
        assert!((at(&editor, b).x - 5.0).abs() < 1e-4, "{}", at(&editor, b));
        editor.undo();
        assert!(
            (at(&editor, editor.node_at("Cube2").unwrap()).x - 3.0).abs() < 1e-4,
            "one undo step"
        );

        // Dropped: bottoms on the ground (cubes are centred: y = 0.5).
        let [a, b, c] = ["Cube", "Cube2", "Cube3"].map(|n| editor.node_at(n).unwrap());
        editor.set_selection(vec![a, b, c], Some(a));
        editor.drop_selected();
        assert!([a, b, c]
            .iter()
            .all(|&n| (at(&editor, n).y - 0.5).abs() < 1e-4));

        // Three copies of one cube along Z, each a cube further.
        editor.set_selection(vec![a], Some(a));
        let start = at(&editor, a);
        editor.duplicate_in_row(2, 3, 0.5);
        assert_eq!(editor.selected.len(), 3);
        let last = editor.selection.unwrap();
        assert!((at(&editor, last).z - (start.z + 4.5)).abs() < 1e-4);
    }

    #[test]
    fn copy_paste_and_cut() {
        let mut editor = editor();
        let [a, b, _] = three_cubes(&mut editor);
        editor.set_transform(a, Transform3D::from_translation(Vec3::new(2.0, 0.0, 0.0)));
        editor.end_gesture();
        assert!(!editor.can_paste(None));
        editor.set_selection(vec![a, b], Some(a));
        let offset =
            editor.tree().global_position(b).unwrap() - editor.tree().global_position(a).unwrap();
        let b_path = editor.relative_path(b).unwrap();
        let text = editor.copy_selected().unwrap();
        assert!(Clipboard::parse(&text).is_some_and(|s| s.len() == 2));
        assert!(Clipboard::parse("just some text").is_none());

        // Paste: beside the selection, as one undo step, with the copies selected.
        let before = editor.tree().children(editor.scene_root).len();
        let steps = editor.history.undo_len();
        editor.paste(Some(&text), None, None);
        assert_eq!(editor.tree().children(editor.scene_root).len(), before + 2);
        assert_eq!(editor.history.undo_len(), steps + 1);
        let copy = editor.selection.unwrap();
        assert!(copy != a && editor.selected.len() == 2);
        assert_eq!(
            editor.tree().global_position(copy),
            Some(Vec3::new(2.0, 0.0, 0.0))
        );
        editor.undo();
        assert_eq!(editor.tree().children(editor.scene_root).len(), before);
        // Undo rebuilt the scene: find the nodes again.
        let b = editor.node_at(&b_path).unwrap();

        // Into another parent: the copy keeps its place in the world.
        editor.select(None);
        editor.add_node(
            NodeDesc {
                name: "Holder".into(),
                kind: Some("Node3D".into()),
                position: Some([0.0, 5.0, 0.0]),
                ..Default::default()
            },
            None,
        );
        let holder = editor.selection.unwrap();
        editor.paste(None, Some(holder), None);
        let inside = editor.tree().children(holder);
        assert_eq!(inside.len(), 2);
        assert_eq!(
            editor.tree().global_position(inside[0]),
            Some(Vec3::new(2.0, 0.0, 0.0))
        );
        // At a point (Paste Here): the first lands there, the second keeps its offset.
        editor.paste(
            None,
            Some(editor.scene_root),
            Some(Vec3::new(10.0, 0.0, 0.0)),
        );
        let pasted = editor.selected.clone();
        let place = |n| editor.tree().global_position(n).unwrap();
        assert_eq!(place(pasted[0]), Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(place(pasted[1]) - place(pasted[0]), offset);

        // Cut: copied and gone; Paste brings it back.
        editor.set_selection(vec![b], None);
        let count = editor.tree().children(editor.scene_root).len();
        editor.cut_selected().unwrap();
        assert!(!editor.tree().is_valid(b));
        assert_eq!(editor.tree().children(editor.scene_root).len(), count - 1);
        editor.paste(None, None, None);
        assert_eq!(editor.tree().children(editor.scene_root).len(), count);

        // Nodes copied in another editor window arrive as text.
        let mut other = Editor::new(editor.project().to_path_buf(), None, AudioOutput::Silent);
        other.new_scene();
        let children = other.tree().children(other.scene_root).len();
        other.paste(Some(&text), None, None);
        assert_eq!(other.tree().children(other.scene_root).len(), children + 2);
        // The scene root itself cannot be copied.
        other.select(Some(other.scene_root));
        assert!(other.copy_selected().is_none());
    }

    #[test]
    fn selection_toggle_add_and_roots() {
        let mut editor = editor();
        let [a, b, c] = three_cubes(&mut editor);
        editor.select(Some(a));
        editor.toggle_selected(b);
        assert_eq!(editor.selected, [a, b]);
        assert_eq!(editor.selection, Some(b), "the added node is active");
        editor.toggle_selected(b);
        assert_eq!(editor.selected, [a]);
        assert_eq!(editor.selection, Some(a));
        editor.add_to_selection(&[b, c, b]);
        assert_eq!(editor.selected, [a, b, c]);
        assert_eq!(editor.selection, Some(b));
        editor.set_selection(vec![c, a], Some(b));
        assert_eq!(
            editor.selection,
            Some(a),
            "an active node outside falls back to the last"
        );
        editor.select(None);
        assert!(editor.selected.is_empty() && editor.selection.is_none());

        // A child whose parent is selected is not a root.
        editor.reparent(c, a, None);
        editor.set_selection(vec![a, b, c], None);
        assert_eq!(editor.selected_roots(), [a, b]);
    }

    #[test]
    fn multi_delete_and_duplicate_are_one_undo_step() {
        let mut editor = editor();
        let [a, b, _] = three_cubes(&mut editor);
        let children = |editor: &Editor| editor.tree().children(editor.scene_root).len();
        let before = children(&editor);

        editor.set_selection(vec![a, b], Some(a));
        editor.duplicate_selected();
        assert_eq!(children(&editor), before + 2);
        assert_eq!(editor.selected.len(), 2, "the copies are selected");
        assert!(!editor.is_selected(a) && !editor.is_selected(b));
        editor.undo();
        assert_eq!(children(&editor), before);

        let a = editor.node_at("Cube").unwrap();
        let b = editor.node_at("Cube2").unwrap();
        editor.set_selection(vec![a, b], Some(b));
        editor.delete_selected();
        assert_eq!(children(&editor), before - 2);
        assert!(editor.selected.is_empty());
        editor.undo();
        assert_eq!(children(&editor), before);
        assert_eq!(editor.selected.len(), 2, "undo brings the selection back");
        assert_eq!(
            editor
                .selection
                .and_then(|n| editor.relative_path(n))
                .as_deref(),
            Some("Cube2")
        );
    }

    #[test]
    fn one_step_groups_edits() {
        let mut editor = editor();
        let [a, b, c] = three_cubes(&mut editor);
        let steps = editor.history.undo_len();
        editor.as_one_step(|editor| {
            editor.reparent(b, a, None);
            editor.reparent(c, a, None);
        });
        assert_eq!(editor.history.undo_len(), steps + 1);
        editor.undo();
        let tree = editor.tree();
        assert_eq!(
            tree.children(editor.scene_root).len(),
            3 + 3,
            "template + cubes"
        );
    }

    #[test]
    fn new_scene_has_the_template() {
        let editor = editor();
        let tree = editor.tree();
        assert!(editor.node_at("Sun").is_some());
        assert!(editor.node_at("Camera").is_some());
        assert_eq!(tree.parent(editor.scene_root), Some(tree.root()));
        assert!(!editor.modified);
    }

    #[test]
    fn add_rename_duplicate_delete_with_undo() {
        let mut editor = editor();
        let initial = editor.describe();
        editor.add_node(cube(), Some(Vec3::new(1.0, 2.0, 3.0)));
        let cube = editor.selection.unwrap();
        assert_eq!(editor.relative_path(cube).as_deref(), Some("Cube"));
        assert_eq!(
            editor.tree().global_position(cube),
            Some(Vec3::new(1.0, 2.0, 3.0))
        );

        editor.add_node(self::cube(), None);
        let second = editor.selection.unwrap();
        // Added under the selected cube.
        assert_eq!(editor.relative_path(second).as_deref(), Some("Cube/Cube"));
        editor.end_gesture();

        editor.duplicate(cube);
        let copy = editor.selection.unwrap();
        assert_eq!(editor.relative_path(copy).as_deref(), Some("Cube2"));
        assert!(editor.node_at("Cube2/Cube").is_some(), "children copied");

        editor.rename(copy, "Crate");
        editor.end_gesture();
        assert!(editor.node_at("Crate").is_some());
        // Names stay unique among siblings.
        editor.rename(copy, "Cube");
        editor.end_gesture();
        assert_eq!(editor.relative_path(copy).as_deref(), Some("Cube2"));

        editor.delete(cube);
        assert!(editor.node_at("Cube").is_none());
        for _ in 0..6 {
            editor.undo();
        }
        assert_eq!(editor.describe(), initial);
        editor.redo();
        assert!(editor.node_at("Cube").is_some());
    }

    #[test]
    fn editing_a_description_rebuilds_and_keeps_children() {
        let mut editor = editor();
        editor.add_node(cube(), None);
        let parent = editor.selection.unwrap();
        editor.add_node(cube(), None);
        editor.end_gesture();
        let old = editor.describe_node(parent).unwrap();
        let mut new = old.clone();
        new.mesh = Some(ab_engine::scene_file::MeshDesc::Sphere {
            radius: 2.0,
            segments: 8,
            rings: 4,
        });
        new.position = Some([0.0, 5.0, 0.0]);
        editor.set_transform(
            parent,
            Transform3D::from_translation(Vec3::new(0.0, 5.0, 0.0)),
        );
        editor.select(Some(parent));
        editor.apply_desc(parent, &old, new.clone());
        let rebuilt = editor.selection.unwrap();
        assert_ne!(rebuilt, parent);
        assert_eq!(editor.describe_node(rebuilt).unwrap().mesh, new.mesh);
        assert_eq!(
            editor.tree().position(rebuilt),
            Some(Vec3::new(0.0, 5.0, 0.0))
        );
        assert!(editor.node_at("Cube/Cube").is_some(), "child kept");
        // One gesture: a single undo restores both edits.
        editor.end_gesture();
        editor.undo();
        let back = editor.node_at("Cube").unwrap();
        assert_eq!(editor.describe_node(back).unwrap(), old);
    }

    #[test]
    fn reparent_keeps_world_position() {
        let mut editor = editor();
        editor.select(None);
        editor.add_node(cube(), Some(Vec3::new(2.0, 0.0, 0.0)));
        let a = editor.selection.unwrap();
        editor.select(None);
        editor.add_node(cube(), Some(Vec3::new(0.0, 0.0, 5.0)));
        let b = editor.selection.unwrap();
        editor.reparent(b, a, None);
        assert_eq!(editor.relative_path(b).as_deref(), Some("Cube/Cube2"));
        let at = editor.tree().global_position(b).unwrap();
        assert!(at.distance(Vec3::new(0.0, 0.0, 5.0)) < 1e-4);
        // Not into its own subtree.
        editor.reparent(a, b, None);
        assert_eq!(editor.tree().parent(a), Some(editor.scene_root));
    }

    #[test]
    fn play_runs_and_stop_restores() {
        let mut editor = editor();
        editor.add_node(cube(), Some(Vec3::new(0.0, 3.0, 0.0)));
        let before = editor.describe();
        editor.play();
        assert!(editor.is_playing());
        let cube = editor.node_at("Cube").unwrap();
        editor
            .tree_mut()
            .set_position(cube, Vec3::splat(9.0))
            .unwrap();
        for _ in 0..10 {
            editor.tick(1.0 / 60.0);
        }
        editor.stop();
        assert_eq!(editor.mode, Mode::Edit);
        assert_eq!(editor.describe(), before, "play changes are discarded");
        assert_eq!(editor.tree().children(editor.tree().root()).len(), 1);
    }

    #[test]
    fn a_ragdoll_falls_while_playing_and_is_back_after_stop() {
        let mut editor = editor();
        editor.select(None);
        editor.add_scene(crate::create::ragdoll(true), Some(Vec3::new(0.0, 0.0, 0.0)));
        let head_at = |editor: &Editor| {
            let head = editor.node_at("Ragdoll/Head").unwrap();
            editor.tree().global_position(head).unwrap()
        };
        let standing = head_at(&editor);
        editor.play();
        for _ in 0..120 {
            editor.tick(1.0 / 60.0);
        }
        assert!(head_at(&editor).y < standing.y - 0.5, "fell over");
        // Paused, a step advances exactly one frame.
        editor.toggle_pause();
        let frame = editor
            .tree()
            .world()
            .resource::<Time>()
            .unwrap()
            .frame_count;
        editor.tick(1.0 / 60.0);
        editor.step_frame();
        assert_eq!(
            editor
                .tree()
                .world()
                .resource::<Time>()
                .unwrap()
                .frame_count,
            frame + 1
        );
        // Live tweaks while playing are not undo steps.
        let can_undo = editor.history.can_undo();
        let head = editor.node_at("Ragdoll/Head").unwrap();
        editor.set_transform(head, Transform3D::from_translation(Vec3::splat(3.0)));
        assert_eq!(editor.history.can_undo(), can_undo);
        editor.stop();
        assert!((head_at(&editor) - standing).length() < 1e-4, "restored");
        assert_eq!(
            editor.tree().world().resource::<Time>().unwrap().time_scale,
            1.0
        );
    }

    #[test]
    fn opens_and_saves_scene_files() {
        let mut editor = editor();
        editor.open_scene("slice/level.abscene");
        assert_eq!(editor.status.as_ref().map(|s| s.error), Some(false));
        let described = editor.describe();
        assert!(described.environment.is_some());
        assert!(described.nodes.len() > 20);
        // Unknown game scripts are kept by name.
        assert!(described
            .nodes
            .iter()
            .any(|n| n.script.as_deref() == Some("Player")));

        let dir = std::env::temp_dir().join(format!("ab-editor-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut other = Editor::new(dir.clone(), None, AudioOutput::Silent);
        other.save_as("levels/new");
        assert_eq!(other.scene_path.as_deref(), Some("levels/new.abscene"));
        let text = std::fs::read_to_string(dir.join("levels/new.abscene")).unwrap();
        assert_eq!(SceneFile::from_toml(&text).unwrap(), other.describe());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saving_keeps_the_replaced_version_as_a_backup() {
        let root = std::env::temp_dir().join(format!("ab-editor-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("game");
        std::fs::create_dir_all(&project).unwrap();
        let mut editor = Editor::new(project.clone(), None, AudioOutput::Silent);
        let backups = backup_dir(&root.join("store"), &project);
        assert_ne!(backups, backup_dir(&root.join("store"), &root.join("other/game")));
        editor.backups = Some(backups.clone());

        editor.save_as("levels/a");
        let first = std::fs::read_to_string(project.join("levels/a.abscene")).unwrap();
        assert!(!backups.exists(), "a new file has nothing to back up");
        editor.add_node(
            NodeDesc {
                name: "Extra".into(),
                ..Default::default()
            },
            None,
        );
        editor.save();
        let backup = backups.join("levels/a.abscene.bak");
        assert_eq!(std::fs::read_to_string(backup).unwrap(), first);
        assert_ne!(
            std::fs::read_to_string(project.join("levels/a.abscene")).unwrap(),
            first
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unreadable_input_map_is_not_overwritten() {
        let project = std::env::temp_dir().join(format!("ab-editor-input-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("game.toml"),
            "title = \"T\"\ninput = \"input.toml\"\n",
        )
        .unwrap();
        // Not UTF-8: reading it fails, and the map must stay as it is.
        let original = b"[actions]\njump = [\"Space\"]\n# \xff\n".to_vec();
        std::fs::write(project.join("input.toml"), &original).unwrap();
        let mut editor = Editor::new(project.clone(), None, AudioOutput::Silent);
        let template = ab_project::script_templates::SCRIPT_TEMPLATES
            .iter()
            .find(|t| t.actions().next().is_some())
            .expect("a template with actions");
        assert!(editor.add_template_actions(template).is_err());
        assert_eq!(std::fs::read(project.join("input.toml")).unwrap(), original);
        std::fs::remove_dir_all(project).unwrap();
    }

    #[test]
    fn several_scripts_attach_save_and_undo() {
        let mut editor = editor();
        editor.add_node(cube(), None);
        let cube = editor.selection.unwrap();
        editor.end_gesture();
        editor.attach_script(cube, "Mover");
        editor.end_gesture();
        editor.attach_script(cube, "Health");
        editor.end_gesture();
        editor.attach_script(cube, "Mover");
        assert_eq!(
            editor.tree().script_names(cube),
            ["Mover", "Health"],
            "no duplicates"
        );
        let desc = editor.describe_node(cube).unwrap();
        assert_eq!(desc.scripts, ["Mover", "Health"]);
        assert!(editor.script_names().contains(&"Health".to_owned()));

        // Reordering and removing through the description, as the Inspector does.
        let mut reordered = desc.clone();
        reordered.set_script_list(vec!["Health".into(), "Mover".into()]);
        editor.apply_desc(cube, &desc, reordered);
        editor.end_gesture();
        let cube = editor.node_at("Cube").unwrap();
        assert_eq!(editor.tree().script_names(cube), ["Health", "Mover"]);

        editor.undo();
        let cube = editor.node_at("Cube").unwrap();
        assert_eq!(editor.tree().script_names(cube), ["Mover", "Health"]);
        editor.undo();
        let cube = editor.node_at("Cube").unwrap();
        assert_eq!(
            editor.tree().script_names(cube),
            ["Mover"],
            "one step per attach"
        );
        assert_eq!(
            editor.describe_node(cube).unwrap().script.as_deref(),
            Some("Mover")
        );
    }

    /// The Unity-style flow for real: create a script in a new game, build the library the
    /// way the editor does, attach the script by name, press Play and watch it run
    /// (minutes): `cargo test -p ab-editor created_script -- --ignored`.
    #[test]
    #[ignore]
    fn a_created_script_runs_in_play_mode() {
        use crate::projects::{create, BuildState, NewProject, Profile, ScriptBuild};
        use crate::scripts::GameScripts;
        let location = std::env::temp_dir().join(format!("ab-flow-{}", std::process::id()));
        std::fs::create_dir_all(&location).unwrap();
        let new = NewProject {
            title: "Script Flow Check".into(),
            location: location.clone(),
            new_folder: true,
        };
        let project = create(&new).unwrap();
        let root = location;
        let result = std::panic::catch_unwind(move || {
            let mut scripts = GameScripts::new(project.root.clone());
            let (name, path) = scripts.create("spinner", &[]).unwrap();
            let (lifter, lifter_path) = scripts.create("lifter", &[]).unwrap();
            // What a user would type in the code editor.
            for (path, body) in [
                (&path, "let _ = tree.rotate(node, Vec3::Y, delta);"),
                (
                    &lifter_path,
                    "let _ = tree.translate(node, Vec3::Y * delta);",
                ),
            ] {
                let source = std::fs::read_to_string(path)
                    .unwrap()
                    .replace("let _ = (tree, node, delta);", body);
                std::fs::write(path, source).unwrap();
            }
            let mut build = ScriptBuild::start(&project, Profile::Debug).unwrap();
            assert_eq!(*build.wait(), BuildState::Succeeded);

            let mut editor = Editor::new(
                project.assets.clone(),
                project.editor_library(),
                AudioOutput::Silent,
            );
            editor.open_scene(project.main_scene.as_deref().unwrap());
            assert!(editor.script_names().contains(&name));
            let target = editor
                .tree()
                .children(editor.scene_root)
                .into_iter()
                .find(|&n| editor.tree().is_node3d(n))
                .expect("a 3D node in the empty template's scene");
            editor.attach_script(target, &name);
            assert_eq!(
                editor.tree().script_name(target).as_deref(),
                Some(name.as_str())
            );
            editor.undo();
            assert_eq!(editor.tree().script_name(target), None, "one undo step");
            editor.redo();
            // A second script on the same node, Unity-style.
            let target = editor
                .tree()
                .children(editor.scene_root)
                .into_iter()
                .find(|&n| editor.tree().script_name(n).as_deref() == Some(name.as_str()))
                .unwrap();
            editor.attach_script(target, &lifter);
            let target = editor
                .tree()
                .children(editor.scene_root)
                .into_iter()
                .find(|&n| editor.tree().script_name(n).as_deref() == Some(name.as_str()))
                .unwrap();
            // Children of the scene root are found by name.
            let path = editor.tree().name(target).unwrap().to_owned();

            assert_eq!(
                editor.tree().script_names(target),
                [name.clone(), lifter.clone()]
            );
            let before = editor.tree().transform(target).unwrap().rotation;
            let height = editor.tree().transform(target).unwrap().translation.y;
            editor.play();
            for _ in 0..30 {
                editor.tick(1.0 / 60.0);
            }
            let node = editor.node_at(&path).unwrap();
            let rotation = editor.tree().transform(node).unwrap().rotation;
            let lifted = editor.tree().transform(node).unwrap().translation.y - height;
            assert!(lifted > 0.3, "the second script ran too: {lifted}");
            assert!(
                rotation.angle_between(before) > 0.3,
                "the script ran: {rotation:?}"
            );
            editor.stop();
            let node = editor.node_at(&path).unwrap();
            let after = editor.tree().transform(node).unwrap().rotation;
            assert!(
                after.abs_diff_eq(before, 1e-5),
                "stopping restores the edited scene: {path} {before:?} -> {after:?}, \
                 played {rotation:?}, script {:?}",
                editor.tree().script_name(node)
            );
        });
        let _ = std::fs::remove_dir_all(&root);
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}
