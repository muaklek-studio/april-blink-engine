//! [`Script`], the [`Registry`] of a library, and the entry point the engine loads.

use crate::host;
use crate::tree::Tree;
use crate::value::{Arg, FromValue, Node};
use ab_script_api as ffi;
use std::any::TypeId;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::NonNull;
use std::sync::OnceLock;

/// Behaviour attached to a node: the equivalent of a Godot script.
///
/// Every callback gets the [`Tree`] and the node the script is attached to. Implement only
/// the ones you need. State lives in the struct's fields; to keep it across hot reloads,
/// derive `Serialize` and `Deserialize` and add [`serde_state!`](crate::serde_state).
///
/// A panic in a callback is caught: it is logged with the node and the script is disabled
/// until it is replaced (e.g. by hot reload). The game keeps running.
#[allow(unused_variables)]
pub trait Script: Default + 'static {
    /// The node entered the tree (parents before children).
    fn enter_tree(&mut self, tree: &mut Tree<'_>, node: Node) {}

    /// The node and all its children are in the tree. Once, children first.
    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {}

    /// Every frame; `delta` is the frame time in seconds.
    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {}

    /// Every fixed step (60 per second by default): movement and physics.
    fn physics_process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {}

    /// The node is leaving the tree (children before parents).
    fn exit_tree(&mut self, tree: &mut Tree<'_>, node: Node) {}

    /// A signal connected to this node ([`Tree::connect`]) was emitted.
    fn on_signal(&mut self, tree: &mut Tree<'_>, node: Node, signal: &Signal<'_>) {}

    /// Hot reload replaced the previous version of this script (instead of `ready`).
    fn reloaded(&mut self, tree: &mut Tree<'_>, node: Node) {}

    /// Hot reload: the state to carry over to the new version. See
    /// [`serde_state!`](crate::serde_state).
    fn save_state(&self) -> Option<String> {
        None
    }

    /// Hot reload: restores what [`Script::save_state`] returned in the old version.
    fn load_state(&mut self, state: &str) {}
}

/// A signal received in [`Script::on_signal`].
#[derive(Debug, Clone, PartialEq)]
pub struct Signal<'s> {
    name: &'s str,
    source: Node,
    args: Vec<Arg>,
}

impl<'s> Signal<'s> {
    /// A signal as the engine would deliver it (for tests of `on_signal`).
    pub fn new(name: &'s str, source: Node, args: Vec<Arg>) -> Self {
        Self { name, source, args }
    }

    pub fn name(&self) -> &str {
        self.name
    }

    /// The node that emitted it.
    pub fn source(&self) -> Node {
        self.source
    }

    pub fn is(&self, name: &str) -> bool {
        self.name == name
    }

    pub fn args(&self) -> &[Arg] {
        &self.args
    }

    /// The first argument as `T`.
    pub fn arg<T: FromValue>(&self) -> Option<T> {
        self.args.first()?.get()
    }
}

/// Scripts of a library by name: what scene files and [`Tree::set_script`] refer to.
#[derive(Debug, Default)]
pub struct Registry {
    entries: Vec<Entry>,
}

#[derive(Debug)]
struct Entry {
    name: String,
    type_id: TypeId,
    vtable: Vtable,
}

#[derive(Debug, Clone, Copy)]
struct Vtable {
    create: unsafe extern "C" fn() -> *mut c_void,
    destroy: unsafe extern "C" fn(*mut c_void),
    event: unsafe extern "C" fn(
        *mut c_void,
        *mut ffi::Context,
        *const ffi::Event,
        *mut ffi::StrSink,
    ) -> ffi::Status,
    save_state: unsafe extern "C" fn(*const c_void, *mut ffi::StrSink) -> bool,
    load_state: unsafe extern "C" fn(*mut c_void, ffi::Str, *mut ffi::StrSink) -> ffi::Status,
}

impl Registry {
    /// Registers `T` under `name` (replacing an earlier entry with that name).
    pub fn register<T: Script>(&mut self, name: &str) -> &mut Self {
        self.entries.retain(|e| e.name != name);
        self.entries.push(Entry {
            name: name.to_owned(),
            type_id: TypeId::of::<T>(),
            vtable: Vtable {
                create: create::<T>,
                destroy: destroy::<T>,
                event: event::<T>,
                save_state: save_state::<T>,
                load_state: load_state::<T>,
            },
        });
        self
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_str())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The library description handed to the engine, built once.
struct LibraryData {
    /// Owns the names the script types point to.
    _names: Vec<Box<str>>,
    types: Vec<ffi::ScriptType>,
    type_ids: Vec<TypeId>,
    library: ffi::Library,
}

struct Shared(LibraryData);

// SAFETY: the data is immutable after construction; its raw pointers point into the same
// immutable data or to functions.
unsafe impl Sync for Shared {}
// SAFETY: as above.
unsafe impl Send for Shared {}

static LIBRARY: OnceLock<Shared> = OnceLock::new();

fn build(registry: Registry) -> LibraryData {
    let names: Vec<Box<str>> = registry
        .entries
        .iter()
        .map(|e| e.name.clone().into_boxed_str())
        .collect();
    let types: Vec<ffi::ScriptType> = registry
        .entries
        .iter()
        .zip(&names)
        .map(|(entry, name)| ffi::ScriptType {
            name: ffi::Str::new(name),
            flags: 0,
            create: entry.vtable.create,
            destroy: entry.vtable.destroy,
            event: entry.vtable.event,
            save_state: entry.vtable.save_state,
            load_state: entry.vtable.load_state,
        })
        .collect();
    let library = ffi::Library {
        abi_version: ffi::ABI_VERSION,
        sdk_version: ffi::Str::new(env!("CARGO_PKG_VERSION")),
        script_types: types.as_ptr(),
        script_type_count: types.len(),
    };
    LibraryData {
        type_ids: registry.entries.iter().map(|e| e.type_id).collect(),
        _names: names,
        types,
        library,
    }
}

/// The engine's entry into a library (see [`export!`](crate::export)).
///
/// # Safety
/// `host` must be the engine's API (or null).
#[doc(hidden)]
pub unsafe fn entry(host: *const ffi::HostApi, register: fn(&mut Registry)) -> *const ffi::Library {
    if let Err(why) = host::connect(host) {
        if !host.is_null() {
            // SAFETY: a non-null host is valid even when its version does not match: `log`
            // is part of every version's layout.
            let host = unsafe { &*host };
            // SAFETY: the strings are borrowed for the call only.
            unsafe {
                (host.log)(
                    ffi::LogLevel::ERROR,
                    ffi::Str::new("ab_script"),
                    ffi::Str::new(&why),
                );
            }
        }
        return std::ptr::null();
    }
    let shared = LIBRARY.get_or_init(|| {
        let mut registry = Registry::default();
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| register(&mut registry))) {
            crate::host::log(
                ffi::LogLevel::ERROR,
                "ab_script",
                format_args!(
                    "registering the scripts panicked: {}",
                    panic_message(payload.as_ref())
                ),
            );
            registry = Registry::default();
        }
        Shared(build(registry))
    });
    &shared.0.library
}

/// The engine-facing description of `T`, if this library registered it (and the engine has
/// loaded the library).
///
/// For code linked with the script library that also drives the engine directly (tests,
/// tools): the engine's `SceneTree::script_instance(node, script_type)` returns the instance
/// of this type attached to a node, as a pointer to `T`.
pub fn script_type<T: Script>() -> Option<*const ffi::ScriptType> {
    let data = &LIBRARY.get()?.0;
    let index = data
        .type_ids
        .iter()
        .position(|&id| id == TypeId::of::<T>())?;
    Some(&data.types[index])
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

fn push_str(sink: *mut ffi::StrSink, text: &str) {
    if sink.is_null() {
        return;
    }
    // SAFETY: the engine passes a valid sink for the duration of the call.
    let sink = unsafe { &*sink };
    // SAFETY: as above; the text is borrowed for the push only.
    unsafe { (sink.push)(sink.data, ffi::Str::new(text)) };
}

unsafe extern "C" fn create<T: Script>() -> *mut c_void {
    match catch_unwind(T::default) {
        Ok(script) => Box::into_raw(Box::new(script)).cast(),
        Err(payload) => {
            crate::host::log(
                ffi::LogLevel::ERROR,
                "ab_script",
                format_args!(
                    "creating {} panicked: {}",
                    std::any::type_name::<T>(),
                    panic_message(payload.as_ref())
                ),
            );
            std::ptr::null_mut()
        }
    }
}

unsafe extern "C" fn destroy<T: Script>(instance: *mut c_void) {
    if instance.is_null() {
        return;
    }
    // SAFETY: made by `create::<T>` and destroyed once by the engine.
    let script = unsafe { Box::from_raw(instance.cast::<T>()) };
    // A panicking `Drop` must not unwind into the engine.
    let _ = catch_unwind(AssertUnwindSafe(move || drop(script)));
}

unsafe extern "C" fn event<T: Script>(
    instance: *mut c_void,
    ctx: *mut ffi::Context,
    event: *const ffi::Event,
    error: *mut ffi::StrSink,
) -> ffi::Status {
    let (Some(ctx), false, false) = (NonNull::new(ctx), instance.is_null(), event.is_null()) else {
        push_str(error, "invalid event");
        return ffi::Status::BAD_ARGUMENTS;
    };
    // SAFETY: the engine passes an instance made by `create::<T>`, not otherwise borrowed
    // during the event, and a valid event.
    let (script, event) = unsafe { (&mut *instance.cast::<T>(), &*event) };
    // SAFETY: the context is the event's, alive during this call.
    let mut tree = unsafe { Tree::from_context(ctx) };
    let node = Node::from_bits(event.node.0);
    let result = catch_unwind(AssertUnwindSafe(|| match event.kind {
        ffi::EventKind::ENTER_TREE => script.enter_tree(&mut tree, node),
        ffi::EventKind::READY => script.ready(&mut tree, node),
        ffi::EventKind::PROCESS => script.process(&mut tree, node, event.delta),
        ffi::EventKind::PHYSICS_PROCESS => script.physics_process(&mut tree, node, event.delta),
        ffi::EventKind::EXIT_TREE => script.exit_tree(&mut tree, node),
        ffi::EventKind::RELOADED => script.reloaded(&mut tree, node),
        ffi::EventKind::SIGNAL => {
            let args: &[ffi::Value] = if event.args.is_null() || event.arg_count == 0 {
                &[]
            } else {
                // SAFETY: the engine passes `arg_count` values valid during the event.
                unsafe { std::slice::from_raw_parts(event.args, event.arg_count) }
            };
            // SAFETY: the name is valid during the event.
            let name = unsafe { event.signal.to_str() }.unwrap_or("");
            let signal = Signal {
                name,
                source: Node::from_bits(event.source.0),
                args: args.iter().map(Arg::from_ffi).collect(),
            };
            script.on_signal(&mut tree, node, &signal);
        }
        _ => {}
    }));
    match result {
        Ok(()) => ffi::Status::OK,
        Err(payload) => {
            push_str(error, &panic_message(payload.as_ref()));
            ffi::Status::PANICKED
        }
    }
}

unsafe extern "C" fn save_state<T: Script>(
    instance: *const c_void,
    out: *mut ffi::StrSink,
) -> bool {
    if instance.is_null() || out.is_null() {
        return false;
    }
    // SAFETY: an instance made by `create::<T>`, only read here.
    let script = unsafe { &*instance.cast::<T>() };
    match catch_unwind(AssertUnwindSafe(|| script.save_state())) {
        Ok(Some(state)) => {
            push_str(out, &state);
            true
        }
        _ => false,
    }
}

unsafe extern "C" fn load_state<T: Script>(
    instance: *mut c_void,
    state: ffi::Str,
    error: *mut ffi::StrSink,
) -> ffi::Status {
    if instance.is_null() {
        return ffi::Status::BAD_ARGUMENTS;
    }
    // SAFETY: an instance made by `create::<T>`, not otherwise borrowed; the state string is
    // valid during the call.
    let (script, state) = unsafe { (&mut *instance.cast::<T>(), state.to_str()) };
    let Some(state) = state else {
        push_str(error, "the saved state is not UTF-8");
        return ffi::Status::BAD_ARGUMENTS;
    };
    match catch_unwind(AssertUnwindSafe(|| script.load_state(state))) {
        Ok(()) => ffi::Status::OK,
        Err(payload) => {
            push_str(error, &panic_message(payload.as_ref()));
            ffi::Status::PANICKED
        }
    }
}

/// Exports the library's scripts to the engine: put it once in the crate root, naming the
/// function that registers them.
///
/// ```ignore
/// fn register(registry: &mut ab_script::Registry) {
///     registry.register::<Player>("Player");
/// }
/// ab_script::export!(register);
/// ```
#[macro_export]
macro_rules! export {
    ($register:path) => {
        /// The script library's entry point, loaded by the engine.
        ///
        /// # Safety
        /// Only the engine calls it, with its API.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn ab_script_library(
            host: *const $crate::ffi::HostApi,
        ) -> *const $crate::ffi::Library {
            // SAFETY: forwarded from the engine's loader.
            unsafe { $crate::__private::entry(host, $register) }
        }
    };
}

/// Implements [`Script::save_state`] and [`Script::load_state`] with serde (JSON), so the
/// script keeps its fields across hot reloads. The struct must derive `Serialize` and
/// `Deserialize`; add `#[serde(default)]` so new fields get their defaults.
#[macro_export]
macro_rules! serde_state {
    () => {
        fn save_state(&self) -> ::core::option::Option<::std::string::String> {
            $crate::__private::serde_json::to_string(self).ok()
        }

        fn load_state(&mut self, state: &str) {
            match $crate::__private::serde_json::from_str(state) {
                ::core::result::Result::Ok(restored) => *self = restored,
                ::core::result::Result::Err(err) => $crate::warn!(
                    "cannot restore the state of {}: {err}",
                    ::core::any::type_name::<Self>()
                ),
            }
        }
    };
}
