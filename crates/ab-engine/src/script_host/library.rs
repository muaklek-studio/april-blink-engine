//! Script libraries: loading them, and running their scripts as [`NodeScript`]s.

use super::call::{signal_value, CallContext};
use super::host_api;
use ab_scene::{NodeHandle, NodeScript, SceneTree, ScriptRegistry, Signal};
use ab_script_api as ffi;
use anyhow::{bail, Context as _};
use std::ffi::c_void;
use std::fmt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::{error, info, warn};

/// Library files loaded in this process, and their total size: they are never unloaded, so
/// a long session of hot reloads keeps every version in memory.
static LOADED_COUNT: AtomicUsize = AtomicUsize::new(0);
static LOADED_BYTES: AtomicU64 = AtomicU64::new(0);

/// Every this many loaded versions, a warning suggests restarting.
const WARN_EVERY: usize = 25;

/// A loaded script library: the scripts of a game, built with the `ab-script` SDK.
///
/// Libraries stay loaded until the process ends (objects of an old version may outlive a hot
/// reload), so loading the same file again after a rebuild gives a new, separate version.
#[derive(Clone)]
pub struct ScriptLibrary {
    inner: Arc<Loaded>,
}

struct Loaded {
    /// Where it came from, for messages.
    origin: String,
    sdk_version: String,
    types: Vec<ScriptTypeInfo>,
}

struct ScriptTypeInfo {
    name: &'static str,
    vtable: *const ffi::ScriptType,
    properties: Vec<ab_scene::ScriptPropertyDesc>,
}

// SAFETY: the library's description is immutable and lives forever (the library is never
// unloaded); its functions are callable from any thread (the SDK keeps no thread state).
unsafe impl Send for Loaded {}
// SAFETY: as above.
unsafe impl Sync for Loaded {}

impl fmt::Debug for ScriptLibrary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScriptLibrary")
            .field("origin", &self.inner.origin)
            .field("sdk_version", &self.inner.sdk_version)
            .field("scripts", &self.names().collect::<Vec<_>>())
            .finish()
    }
}

/// How many library files this process has loaded, and their total size in bytes.
pub fn loaded_libraries() -> (usize, u64) {
    (
        LOADED_COUNT.load(Ordering::Relaxed),
        LOADED_BYTES.load(Ordering::Relaxed),
    )
}

fn note_loaded(bytes: u64) {
    let count = LOADED_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let total = LOADED_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
    if count.is_multiple_of(WARN_EVERY) {
        warn!(
            "{count} versions of the game's scripts are loaded ({} MB): old versions stay in \
             memory until the program restarts, so restart it when convenient",
            total / (1024 * 1024)
        );
    }
}

impl ScriptLibrary {
    /// Loads a script library file (`.so`, `.dll`, `.dylib`). The file is copied first, so
    /// the original can be rebuilt while the game runs and each version loads separately.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let copy = staging_copy(path)?;
        // SAFETY: loading runs the library's initialisers. Script libraries are the game's
        // own code; everything after this goes through the checked C ABI.
        let library = unsafe { libloading::Library::new(&copy) };
        // Debug builds are large: drop the copy once mapped (on Windows this fails while the
        // library is loaded; the next start clears the staging folder).
        let _ = std::fs::remove_file(&copy);
        let library = library.with_context(|| format!("cannot load {}", path.display()))?;
        note_loaded(std::fs::metadata(path).map_or(0, |m| m.len()));
        // SAFETY: `ab_script::export!` defines the entry point with this signature.
        let entry = unsafe { library.get::<ffi::EntryPoint>(ffi::ENTRY_SYMBOL_NUL) }
            .map(|symbol| *symbol)
            .with_context(|| {
                format!(
                    "{} is not a script library (no `{}`: add `ab_script::export!(register);`)",
                    path.display(),
                    ffi::ENTRY_SYMBOL
                )
            })?;
        let loaded = Self::from_entry(entry, &path.display().to_string())?;
        // Never unloaded: script objects, strings and functions of this version may still be
        // referenced after a newer version replaces it.
        std::mem::forget(library);
        Ok(loaded)
    }

    /// A library linked into the executable: `entry` is its `ab_script_library` function.
    pub fn from_entry(entry: ffi::EntryPoint, origin: &str) -> anyhow::Result<Self> {
        // SAFETY: the entry point of a script library, given the engine's API.
        let description = unsafe { entry(host_api()) };
        if description.is_null() {
            bail!("{origin} refused to load (see the log for why)");
        }
        // SAFETY: a non-null description stays valid as long as the library is loaded,
        // which is forever.
        let description = unsafe { &*description };
        if description.abi_version != ffi::ABI_VERSION {
            bail!(
                "{origin} was built for script ABI {} but this engine uses ABI {}; rebuild \
                 it with a matching ab-script",
                description.abi_version,
                ffi::ABI_VERSION
            );
        }
        let vtables: &[ffi::ScriptType] = if description.script_types.is_null()
            || description.script_type_count == 0
        {
            &[]
        } else {
            // SAFETY: the library describes `script_type_count` types at `script_types`.
            unsafe {
                std::slice::from_raw_parts(description.script_types, description.script_type_count)
            }
        };
        let mut types = Vec::with_capacity(vtables.len());
        for vtable in vtables {
            // SAFETY: names live as long as the library.
            let Some(name) = (unsafe { vtable.name.to_str() }) else {
                bail!("{origin} has a script whose name is not UTF-8");
            };
            let properties = if vtable.properties.is_null() || vtable.property_count == 0 {
                Vec::new()
            } else {
                // SAFETY: the library describes `property_count` properties, whose strings
                // live as long as the library (which is never unloaded).
                unsafe { std::slice::from_raw_parts(vtable.properties, vtable.property_count) }
                    .iter()
                    .map(|p| unsafe { ab_scene::ScriptPropertyDesc::from_ffi(p) })
                    .collect()
            };
            types.push(ScriptTypeInfo {
                name: super::call::intern(name),
                vtable,
                properties,
            });
        }
        // SAFETY: as above.
        let sdk_version = unsafe { description.sdk_version.to_str() }
            .unwrap_or("?")
            .to_owned();
        info!(
            library = origin,
            sdk = sdk_version,
            scripts = types.len(),
            "script library loaded"
        );
        Ok(Self {
            inner: Arc::new(Loaded {
                origin: origin.to_owned(),
                sdk_version,
                types,
            }),
        })
    }

    /// Names of its scripts, as scene files refer to them.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.inner.types.iter().map(|t| t.name)
    }

    /// Version of the `ab-script` SDK it was built with.
    pub fn sdk_version(&self) -> &str {
        &self.inner.sdk_version
    }

    /// Registers its scripts by name (replacing entries of the same name).
    pub fn register(&self, registry: &mut ScriptRegistry) {
        for index in 0..self.inner.types.len() {
            let library = self.inner.clone();
            let name = library.types[index].name;
            let props = library.types[index].properties.clone();
            registry.register_factory(name, move || -> Box<dyn NodeScript> {
                match ForeignScript::new(library.clone(), index) {
                    Some(script) => Box::new(script),
                    // Creation failed (logged by the library): an inert stand-in.
                    None => Box::new(Inert),
                }
            });
            registry.register_metadata(name, props);
        }
    }

    /// A registry with only this library's scripts.
    pub fn registry(&self) -> ScriptRegistry {
        let mut registry = ScriptRegistry::new();
        self.register(&mut registry);
        registry
    }
}

/// Copies `path` into a per-process staging folder under a fresh name (dynamic loaders cache
/// libraries by path, and the original must stay free for the next build).
fn staging_copy(path: &Path) -> anyhow::Result<PathBuf> {
    use std::sync::atomic::AtomicU32;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let staging = staging_dir();
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("cannot create {}", staging.display()))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("scripts");
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or(std::env::consts::DLL_EXTENSION);
    let copy = staging.join(format!(
        "{stem}-{}.{extension}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::copy(path, &copy).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(copy)
}

/// Where loaded copies go. Clears what earlier runs left (on Windows a loaded copy cannot
/// be deleted, so they would accumulate), but not the folders of other processes copying
/// a library right now (the editor and a game started together): only folders untouched
/// for a while.
fn staging_dir() -> PathBuf {
    use std::sync::OnceLock;
    use std::time::{Duration, SystemTime};
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let root = std::env::temp_dir().join("april-blink-scripts");
        let stale = |entry: &std::fs::DirEntry| {
            entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| SystemTime::now().duration_since(t).ok())
                .is_some_and(|age| age > Duration::from_secs(600))
        };
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten().filter(stale) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        root.join(std::process::id().to_string())
    })
    .clone()
}

/// What a script that could not be created leaves in its place.
struct Inert;

impl NodeScript for Inert {
    fn has_failed(&self) -> bool {
        true
    }
}

/// A script from a library, driven through its C ABI.
pub(crate) struct ForeignScript {
    library: Arc<Loaded>,
    index: usize,
    instance: NonNull<c_void>,
    failed: bool,
}

impl ForeignScript {
    fn new(library: Arc<Loaded>, index: usize) -> Option<Self> {
        let vtable = library.types[index].vtable;
        // SAFETY: a script type of a loaded library.
        let instance = NonNull::new(unsafe { ((*vtable).create)() })?;
        Some(Self {
            library,
            index,
            instance,
            failed: false,
        })
    }

    fn info(&self) -> &ScriptTypeInfo {
        &self.library.types[self.index]
    }

    fn vtable(&self) -> &ffi::ScriptType {
        // SAFETY: the library (and its description) is never unloaded.
        unsafe { &*self.info().vtable }
    }

    /// Runs an event; a failure disables the script (like a panicking native script).
    fn dispatch(&mut self, tree: &mut SceneTree, node: NodeHandle, event: &ffi::Event) {
        if self.failed {
            return;
        }
        let mut ctx = CallContext {
            tree: &raw mut *tree,
            error: String::new(),
        };
        let mut message = String::new();
        let mut sink = super::string_sink(&mut message);
        // SAFETY: an instance of this type, not otherwise borrowed; the context, event and
        // sink outlive the call.
        let status = unsafe {
            (self.vtable().event)(
                self.instance.as_ptr(),
                (&raw mut ctx).cast(),
                event,
                &mut sink,
            )
        };
        if status.is_ok() {
            return;
        }
        self.failed = true;
        let kind = match ffi::EventKind(event.kind.0) {
            ffi::EventKind::ENTER_TREE => "enter_tree",
            ffi::EventKind::READY => "ready",
            ffi::EventKind::PROCESS => "process",
            ffi::EventKind::PHYSICS_PROCESS => "physics_process",
            ffi::EventKind::EXIT_TREE => "exit_tree",
            ffi::EventKind::SIGNAL => "on_signal",
            ffi::EventKind::RELOADED => "reloaded",
            _ => "event",
        };
        error!(
            script = self.info().name,
            node = tree.path_of(node).unwrap_or_else(|| node.to_string()),
            callback = kind,
            "script {}: {message}; it is disabled until replaced (e.g. by hot reload)",
            if status == ffi::Status::PANICKED {
                "panicked"
            } else {
                "failed"
            }
        );
    }

    fn simple(&mut self, tree: &mut SceneTree, node: NodeHandle, kind: ffi::EventKind, delta: f32) {
        let event = ffi::Event::new(kind, ffi::NodeId(node.to_bits()), delta);
        self.dispatch(tree, node, &event);
    }
}

impl Drop for ForeignScript {
    fn drop(&mut self) {
        // SAFETY: made by this type's `create`, destroyed once.
        unsafe { (self.vtable().destroy)(self.instance.as_ptr()) };
    }
}

impl NodeScript for ForeignScript {
    fn enter_tree(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        self.simple(tree, node, ffi::EventKind::ENTER_TREE, 0.0);
    }

    fn ready(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        self.simple(tree, node, ffi::EventKind::READY, 0.0);
    }

    fn process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {
        self.simple(tree, node, ffi::EventKind::PROCESS, delta);
    }

    fn physics_process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {
        self.simple(tree, node, ffi::EventKind::PHYSICS_PROCESS, delta);
    }

    fn exit_tree(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        self.simple(tree, node, ffi::EventKind::EXIT_TREE, 0.0);
    }

    fn reloaded(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        self.simple(tree, node, ffi::EventKind::RELOADED, 0.0);
    }

    fn on_signal(&mut self, tree: &mut SceneTree, node: NodeHandle, signal: &Signal) {
        let mut strings = Vec::new();
        let args: Vec<ffi::Value> = signal_value(signal, &mut strings).into_iter().collect();
        let mut event = ffi::Event::new(ffi::EventKind::SIGNAL, ffi::NodeId(node.to_bits()), 0.0);
        event.signal = ffi::Str::new(signal.name);
        event.source = ffi::NodeId(signal.source.to_bits());
        event.args = args.as_ptr();
        event.arg_count = args.len();
        self.dispatch(tree, node, &event);
    }

    fn save_state(&self) -> Option<String> {
        let mut state = String::new();
        let mut sink = super::string_sink(&mut state);
        // SAFETY: an instance of this type, only read; the sink outlives the call.
        let saved = unsafe { (self.vtable().save_state)(self.instance.as_ptr(), &mut sink) };
        saved.then_some(state)
    }

    fn load_state(&mut self, state: &str) {
        let mut message = String::new();
        let mut sink = super::string_sink(&mut message);
        // SAFETY: an instance of this type; the state and sink outlive the call.
        let status = unsafe {
            (self.vtable().load_state)(self.instance.as_ptr(), ffi::Str::new(state), &mut sink)
        };
        if !status.is_ok() {
            error!(
                script = self.info().name,
                "restoring the script's state failed: {message}"
            );
        }
    }

    fn script_type_name(&self) -> &'static str {
        self.info().name
    }

    fn has_failed(&self) -> bool {
        self.failed
    }

    fn foreign_instance(&mut self) -> Option<(*const (), *mut ())> {
        Some((self.info().vtable.cast(), self.instance.as_ptr().cast()))
    }

    fn get_exported_property(&self, name: &str) -> Option<ffi::Value> {
        if self.failed {
            return None;
        }
        let get = self.vtable().get_property?;
        let mut out = ffi::Value::NIL;
        // SAFETY: the instance is alive and of this type; `name` outlives the call.
        let status = unsafe { get(self.instance.as_ptr(), ffi::Str::new(name), &mut out) };
        status.is_ok().then_some(out)
    }

    fn set_exported_property(&mut self, name: &str, value: ffi::Value) -> bool {
        if self.failed {
            return false;
        }
        let Some(set) = self.vtable().set_property else {
            return false;
        };
        // SAFETY: as above; `value` (and any string in it) outlives the call.
        let status = unsafe { set(self.instance.as_ptr(), ffi::Str::new(name), &value) };
        if status == ffi::Status::PANICKED {
            error!(
                script = self.info().name,
                property = name,
                "set_property panicked; the script is disabled until replaced"
            );
            self.failed = true;
        }
        status.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_libraries_are_counted() {
        let (count, bytes) = loaded_libraries();
        note_loaded(1000);
        let (after, after_bytes) = loaded_libraries();
        // Other tests may load libraries meanwhile: at least this one is counted.
        assert!(after > count && after_bytes >= bytes + 1000);
    }
}
