use crate::handle::NodeHandle;
use crate::scene_tree::SceneTree;
use crate::signal::Signal;
use ab_script_api as ffi;
use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use tracing::error;

/// Upcast to [`Any`], so scripts can be looked up by their concrete type
/// ([`SceneTree::script`]). Implemented for every type; never implement it yourself.
pub trait AsAny: Any {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Any> AsAny for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Behaviour attached to a node: the Rust-side equivalent of a Godot script.
///
/// Every callback receives the tree and the node the script is attached to, so it can freely
/// read and modify the scene (add children, move nodes, queue frees). While a callback runs
/// the script is detached from its node; callbacks for that same node triggered from inside
/// it are skipped.
#[allow(unused_variables)]
pub trait NodeScript: AsAny {
    /// The node entered the tree (parents before children).
    fn enter_tree(&mut self, tree: &mut SceneTree, node: NodeHandle) {}

    /// The node and all of its children are in the tree. Runs once, children first.
    fn ready(&mut self, tree: &mut SceneTree, node: NodeHandle) {}

    /// Every frame, with the variable frame delta in seconds.
    fn process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {}

    /// Every fixed step, with the fixed delta in seconds.
    fn physics_process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {}

    /// The node is leaving the tree (children before parents).
    fn exit_tree(&mut self, tree: &mut SceneTree, node: NodeHandle) {}

    /// A signal connected to this node with [`SceneTree::connect`] was emitted.
    fn on_signal(&mut self, tree: &mut SceneTree, node: NodeHandle, signal: &Signal) {}

    /// Hot reload: the state to carry over to the new version of this script. Usually
    /// implemented with [`serde_script_state!`](crate::serde_script_state).
    fn save_state(&self) -> Option<String> {
        None
    }

    /// Hot reload: restores what [`NodeScript::save_state`] returned from the old version.
    fn load_state(&mut self, state: &str) {}

    /// Hot reload: called on the new script right after it replaced the old one (instead of
    /// `ready`, which does not run again).
    fn reloaded(&mut self, tree: &mut SceneTree, node: NodeHandle) {}

    /// Type name of the user's script, for profiling.
    #[doc(hidden)]
    fn script_type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// The script object typed access sees (the user's script inside the engine's wrapper).
    #[doc(hidden)]
    fn script_any(&self) -> &dyn Any {
        self.as_any()
    }

    #[doc(hidden)]
    fn script_any_mut(&mut self) -> &mut dyn Any {
        self.as_any_mut()
    }

    /// Whether the script panicked and is disabled.
    #[doc(hidden)]
    fn has_failed(&self) -> bool {
        false
    }

    /// Scripts from a script library: the library's script type and the instance, for
    /// [`SceneTree::script_instance`].
    #[doc(hidden)]
    fn foreign_instance(&mut self) -> Option<(*const (), *mut ())> {
        None
    }

    /// The current value of an exported property (see
    /// [`ScriptRegistry::register_metadata`](crate::ScriptRegistry::register_metadata)).
    fn get_exported_property(&self, _name: &str) -> Option<ffi::Value> {
        None
    }

    /// Sets an exported property from the scene file or the inspector, before `ready` when
    /// the scene loads. Returns whether the script took it. A string value is only valid
    /// during the call: copy it.
    fn set_exported_property(&mut self, _name: &str, _value: ffi::Value) -> bool {
        false
    }
}

/// Wraps every attached script: a panic in a callback is caught, logged with the node's path,
/// and the script is disabled instead of taking the game down. A hot reload (or attaching a
/// new script) replaces it.
///
/// The wrapper is created where the script type is known (`set_script`, the registry), so
/// `catch_unwind` runs in the same copy of `std` as the script, which matters for scripts in
/// a hot-reloaded library: a panic cannot be caught across two copies of `std`.
pub(crate) struct Guarded<T> {
    script: T,
    failed: bool,
}

impl<T: NodeScript> Guarded<T> {
    pub(crate) fn new(script: T) -> Self {
        Self {
            script,
            failed: false,
        }
    }

    fn guard<R: Default>(
        &mut self,
        tree: &mut SceneTree,
        node: NodeHandle,
        callback: &str,
        f: impl FnOnce(&mut T, &mut SceneTree) -> R,
    ) -> R {
        if self.failed {
            return R::default();
        }
        match catch_unwind(AssertUnwindSafe(|| f(&mut self.script, tree))) {
            Ok(result) => result,
            Err(payload) => {
                self.failed = true;
                error!(
                    script = std::any::type_name::<T>(),
                    node = tree.path_of(node).unwrap_or_else(|| node.to_string()),
                    callback,
                    "script panicked: {}; it is disabled until replaced (e.g. by hot reload)",
                    panic_message(payload.as_ref())
                );
                R::default()
            }
        }
    }
}

impl<T: NodeScript> NodeScript for Guarded<T> {
    fn enter_tree(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        self.guard(tree, node, "enter_tree", |s, t| s.enter_tree(t, node))
    }

    fn ready(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        self.guard(tree, node, "ready", |s, t| s.ready(t, node))
    }

    fn process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {
        self.guard(tree, node, "process", |s, t| s.process(t, node, delta))
    }

    fn physics_process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {
        self.guard(tree, node, "physics_process", |s, t| {
            s.physics_process(t, node, delta)
        })
    }

    fn exit_tree(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        self.guard(tree, node, "exit_tree", |s, t| s.exit_tree(t, node))
    }

    fn on_signal(&mut self, tree: &mut SceneTree, node: NodeHandle, signal: &Signal) {
        self.guard(tree, node, "on_signal", |s, t| s.on_signal(t, node, signal))
    }

    fn save_state(&self) -> Option<String> {
        catch_unwind(AssertUnwindSafe(|| self.script.save_state()))
            .ok()
            .flatten()
    }

    fn load_state(&mut self, state: &str) {
        let script = &mut self.script;
        if catch_unwind(AssertUnwindSafe(|| script.load_state(state))).is_err() {
            error!(
                script = std::any::type_name::<T>(),
                "load_state panicked; the script keeps its defaults"
            );
        }
    }

    fn reloaded(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        self.guard(tree, node, "reloaded", |s, t| s.reloaded(t, node))
    }

    fn script_type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }

    fn script_any(&self) -> &dyn Any {
        self.script.as_any()
    }

    fn script_any_mut(&mut self) -> &mut dyn Any {
        self.script.as_any_mut()
    }

    fn has_failed(&self) -> bool {
        self.failed
    }

    fn get_exported_property(&self, name: &str) -> Option<ffi::Value> {
        catch_unwind(AssertUnwindSafe(|| self.script.get_exported_property(name)))
            .ok()
            .flatten()
    }

    fn set_exported_property(&mut self, name: &str, value: ffi::Value) -> bool {
        if self.failed {
            return false;
        }
        let script = &mut self.script;
        catch_unwind(AssertUnwindSafe(|| {
            script.set_exported_property(name, value)
        }))
        .unwrap_or_else(|_| {
            error!(
                script = std::any::type_name::<T>(),
                property = name,
                "set_exported_property panicked; the property keeps its value"
            );
            false
        })
    }
}

/// Text of a panic payload (`panic!("...")` produces `&str` or `String`).
/// The message of a caught panic.
#[doc(hidden)]
pub fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("(non-string panic payload)")
}

/// Implements [`NodeScript::save_state`] / [`NodeScript::load_state`] with serde (JSON), for
/// scripts deriving `Serialize` and `Deserialize`. Mark the struct `#[serde(default)]` so
/// fields added in a new version start at their default, and skip fields that cannot be
/// serialized with `#[serde(skip)]`.
///
/// ```
/// use ab_scene::{serde_script_state, NodeScript};
///
/// #[derive(Default, serde::Serialize, serde::Deserialize)]
/// #[serde(default)]
/// struct Spinner {
///     speed: f32,
/// }
///
/// impl NodeScript for Spinner {
///     serde_script_state!();
/// }
/// ```
#[macro_export]
macro_rules! serde_script_state {
    () => {
        fn save_state(&self) -> ::core::option::Option<::std::string::String> {
            $crate::__private::serde_json::to_string(self).ok()
        }

        fn load_state(&mut self, state: &str) {
            match $crate::__private::serde_json::from_str(state) {
                ::core::result::Result::Ok(restored) => *self = restored,
                ::core::result::Result::Err(err) => $crate::__private::tracing::warn!(
                    script = ::core::any::type_name::<Self>(),
                    "hot reload: state not restored ({err}); starting from defaults"
                ),
            }
        }
    };
}

/// One script attached to a node.
pub(crate) struct ScriptEntry {
    /// Stable within the node, so a running script finds its place again even if scripts
    /// were added or removed meanwhile.
    pub(crate) id: u64,
    /// `None` while the script is executing.
    pub(crate) script: Option<Box<dyn NodeScript>>,
    /// Registry name, for scripts that can be hot reloaded.
    pub(crate) name: Option<String>,
    /// Exported properties set on this instance (from the scene file or the inspector),
    /// kept to save them and to set them again on a hot-reloaded instance.
    pub(crate) properties: toml::Table,
}

/// Component holding a node's scripts, run in this order (like a Unity object's
/// components).
#[derive(Default)]
pub(crate) struct ScriptSlot {
    pub(crate) entries: Vec<ScriptEntry>,
}

/// Source of script ids. Process-wide, so a slot recreated while one of its scripts runs
/// (`set_script` from a callback) never reuses an id that a running pass still holds.
static NEXT_SCRIPT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl ScriptSlot {
    pub(crate) fn push(&mut self, script: Box<dyn NodeScript>, name: Option<String>) -> u64 {
        let id = NEXT_SCRIPT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.entries.push(ScriptEntry {
            id,
            script: Some(script),
            name,
            properties: toml::Table::new(),
        });
        id
    }

    /// Detaches the script `id` to run it; `None` if it is gone or already running.
    pub(crate) fn take(&mut self, id: u64) -> Option<Box<dyn NodeScript>> {
        self.entries.iter_mut().find(|e| e.id == id)?.script.take()
    }

    /// Puts a script back after it ran, unless it was removed or replaced meanwhile.
    pub(crate) fn restore(&mut self, id: u64, script: Box<dyn NodeScript>) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            if entry.script.is_none() {
                entry.script = Some(script);
            }
        }
    }

    /// Ids of the attached scripts that are not running, in order.
    pub(crate) fn ids(&self) -> Vec<u64> {
        self.entries
            .iter()
            .filter(|e| e.script.is_some())
            .map(|e| e.id)
            .collect()
    }
}
