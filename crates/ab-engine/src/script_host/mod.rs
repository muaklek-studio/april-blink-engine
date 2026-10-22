//! The engine's side of the script ABI ([`ab_script_api`]): it loads script libraries built
//! with the `ab-script` SDK and runs their scripts.
//!
//! Script libraries never see engine types: every engine feature they use is a *binding*, a
//! method called by name with [`ab_script_api::Value`] arguments (the list is in
//! [`method_names`]). Adding a binding never changes the ABI, so libraries keep working
//! with newer engines, and a binding a library needs but an older engine lacks fails with a
//! clear message instead of crashing.
//!
//! ```no_run
//! # fn main() -> anyhow::Result<()> {
//! use ab_engine::script_host::ScriptLibrary;
//! let library = ScriptLibrary::load("target/debug/libmy_game.so".as_ref())?;
//! let mut engine = ab_engine::Engine::new(&Default::default());
//! let world = engine.tree_mut().world_mut();
//! library.register(world.resource_or_insert_with(ab_engine::scene::ScriptRegistry::new));
//! # Ok(()) }
//! ```

mod bindings;
mod call;
mod library;

pub use library::ScriptLibrary;

use ab_scene::NodeHandle;
use ab_script_api as ffi;
use call::{Call, CallContext};
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Names of every binding scripts can call, sorted.
pub fn method_names() -> Vec<&'static str> {
    let mut names: Vec<_> = bindings::table().names().collect();
    names.sort_unstable();
    names
}

/// Names of every node property scripts can read or write (`node.get_property`), sorted.
pub fn property_names() -> Vec<&'static str> {
    let mut names = bindings::property_names();
    names.sort_unstable();
    names
}

/// The engine's API, handed to script libraries.
pub fn host_api() -> *const ffi::HostApi {
    &HOST.0
}

struct Host(ffi::HostApi);

// SAFETY: the API is immutable: a version number, a static string and functions.
unsafe impl Sync for Host {}

static HOST: Host = Host(ffi::HostApi {
    abi_version: ffi::ABI_VERSION,
    size: size_of::<ffi::HostApi>() as u32,
    engine_version: ffi::Str::new(env!("CARGO_PKG_VERSION")),
    log: host_log,
    method: host_method,
    call: host_call,
    error: host_error,
    script: host_script,
});

unsafe extern "C" fn host_log(level: ffi::LogLevel, target: ffi::Str, message: ffi::Str) {
    // SAFETY: the library passes strings valid for the call.
    let (target, message) = unsafe { (target.to_str(), message.to_str()) };
    let (target, message) = (
        target.unwrap_or("script"),
        message.unwrap_or("<invalid UTF-8>"),
    );
    match level {
        ffi::LogLevel::ERROR => tracing::error!(target: "script", script = target, "{message}"),
        ffi::LogLevel::WARN => tracing::warn!(target: "script", script = target, "{message}"),
        ffi::LogLevel::INFO => tracing::info!(target: "script", script = target, "{message}"),
        ffi::LogLevel::DEBUG => tracing::debug!(target: "script", script = target, "{message}"),
        _ => tracing::trace!(target: "script", script = target, "{message}"),
    }
}

unsafe extern "C" fn host_method(name: ffi::Str) -> ffi::MethodId {
    // SAFETY: the library passes a string valid for the call.
    let Some(name) = (unsafe { name.to_str() }) else {
        return ffi::MethodId::UNKNOWN;
    };
    bindings::table().id(name)
}

unsafe extern "C" fn host_call(
    ctx: *mut ffi::Context,
    method: ffi::MethodId,
    args: *const ffi::Value,
    arg_count: usize,
    out: *mut ffi::ValueSink,
) -> ffi::Status {
    if ctx.is_null() {
        return ffi::Status::ERROR;
    }
    // SAFETY: contexts are made by `ForeignScript::dispatch` and only used during the event.
    let ctx = unsafe { &mut *ctx.cast::<CallContext>() };
    let Some(binding) = bindings::table().get(method) else {
        ctx.error = format!("unknown method id {}", method.0);
        return ffi::Status::UNKNOWN_METHOD;
    };
    let args: &[ffi::Value] = if args.is_null() || arg_count == 0 {
        &[]
    } else {
        // SAFETY: the library passes `arg_count` values valid for the call.
        unsafe { std::slice::from_raw_parts(args, arg_count) }
    };
    // SAFETY: a non-null sink is valid for the call.
    let out = (!out.is_null()).then(|| unsafe { &*out });
    // SAFETY: the tree outlives the event this call belongs to, and nothing else uses it
    // while the script's code runs.
    let tree = unsafe { &mut *ctx.tree };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut call = Call::new(tree, args, out);
        (binding.run)(&mut call)
    }));
    match result {
        Ok(Ok(())) => ffi::Status::OK,
        Ok(Err(err)) => {
            ctx.error = format!("{}: {}", binding.name, err.message());
            err.status()
        }
        Err(payload) => {
            ctx.error = format!(
                "{}: the engine panicked: {}",
                binding.name,
                ab_scene::panic_message(payload.as_ref())
            );
            ffi::Status::PANICKED
        }
    }
}

unsafe extern "C" fn host_error(ctx: *mut ffi::Context, out: *mut ffi::StrSink) {
    if ctx.is_null() || out.is_null() {
        return;
    }
    // SAFETY: as in `host_call`; the sink is valid for the call.
    let (ctx, out) = unsafe { (&*ctx.cast::<CallContext>(), &*out) };
    // SAFETY: the message lives during the push.
    unsafe { (out.push)(out.data, ffi::Str::new(&ctx.error)) };
}

unsafe extern "C" fn host_script(
    ctx: *mut ffi::Context,
    node: ffi::NodeId,
    script_type: *const ffi::ScriptType,
) -> *mut c_void {
    if ctx.is_null() || script_type.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: as in `host_call`.
    let ctx = unsafe { &mut *ctx.cast::<CallContext>() };
    // SAFETY: as in `host_call`.
    let tree = unsafe { &mut *ctx.tree };
    tree.script_instance(NodeHandle::from_bits(node.0), script_type.cast())
        .map_or(std::ptr::null_mut(), |p| p.cast())
}

/// A [`ffi::StrSink`] appending to `out`.
pub(crate) fn string_sink(out: &mut String) -> ffi::StrSink {
    unsafe extern "C" fn push(data: *mut c_void, text: ffi::Str) {
        // SAFETY: `data` is the `String` given to `string_sink`, alive during the push; the
        // text is valid during the push.
        let (out, text) = unsafe { (&mut *data.cast::<String>(), text.to_str()) };
        out.push_str(text.unwrap_or("<invalid UTF-8>"));
    }
    ffi::StrSink {
        data: (out as *mut String).cast(),
        push,
    }
}
