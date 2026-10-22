//! The connection to the engine: its [`HostApi`](ffi::HostApi), resolved methods, raw calls.

use ab_script_api as ffi;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

static HOST: AtomicPtr<ffi::HostApi> = AtomicPtr::new(std::ptr::null_mut());

/// Remembers the engine's API. Returns why it cannot be used, if it cannot.
pub(crate) fn connect(host: *const ffi::HostApi) -> Result<(), String> {
    if host.is_null() {
        return Err("the engine passed no API".into());
    }
    // SAFETY: the engine passes a valid `HostApi` that lives for the whole process.
    let api = unsafe { &*host };
    if api.abi_version != ffi::ABI_VERSION {
        return Err(format!(
            "this script library uses script ABI {} but the engine uses ABI {}; rebuild it \
             with an ab-script version matching the engine",
            ffi::ABI_VERSION,
            api.abi_version
        ));
    }
    if (api.size as usize) < size_of::<ffi::HostApi>() {
        return Err("the engine's script API is older than this library expects".into());
    }
    HOST.store(host.cast_mut(), Ordering::Release);
    Ok(())
}

pub(crate) fn host() -> Option<&'static ffi::HostApi> {
    let host = HOST.load(Ordering::Acquire);
    // SAFETY: only set by `connect` to a `HostApi` that lives for the whole process.
    (!host.is_null()).then(|| unsafe { &*host })
}

/// The engine's version (empty before the library is loaded).
pub fn engine_version() -> &'static str {
    // SAFETY: the host's version string lives as long as the host.
    host()
        .and_then(|h| unsafe { h.engine_version.to_str() })
        .unwrap_or("")
}

const UNRESOLVED: u32 = u32::MAX;

/// A host method name, resolved to its id on first use. Created by the SDK's `method!`.
#[doc(hidden)]
#[derive(Debug)]
pub struct MethodCell {
    name: &'static str,
    id: AtomicU32,
    warned: AtomicBool,
}

impl MethodCell {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            id: AtomicU32::new(UNRESOLVED),
            warned: AtomicBool::new(false),
        }
    }

    /// True the first time only: problems are logged once per call site, not every frame.
    pub(crate) fn first_warning(&self) -> bool {
        !self.warned.swap(true, Ordering::Relaxed)
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub(crate) fn id(&self) -> ffi::MethodId {
        let id = self.id.load(Ordering::Relaxed);
        if id != UNRESOLVED {
            return ffi::MethodId(id);
        }
        let Some(host) = host() else {
            return ffi::MethodId::UNKNOWN;
        };
        // SAFETY: a valid host; the name is borrowed for the call only.
        let id = unsafe { (host.method)(ffi::Str::new(self.name)) };
        self.id.store(id.0, Ordering::Relaxed);
        id
    }
}

/// `&'static MethodCell` for a method name, resolved once per call site.
macro_rules! method {
    ($name:literal) => {{
        static CELL: $crate::host::MethodCell = $crate::host::MethodCell::new($name);
        &CELL
    }};
}
pub(crate) use method;

/// Writes to the engine's log.
#[doc(hidden)]
pub fn log(level: ffi::LogLevel, target: &str, message: std::fmt::Arguments<'_>) {
    let Some(host) = host() else {
        return;
    };
    let text;
    let message = match message.as_str() {
        Some(text) => text,
        None => {
            text = message.to_string();
            &text
        }
    };
    // SAFETY: a valid host; both strings are borrowed for the call only.
    unsafe { (host.log)(level, ffi::Str::new(target), ffi::Str::new(message)) };
}

/// A [`ffi::ValueSink`] calling `each` for every pushed value.
pub(crate) fn value_sink<F: FnMut(&ffi::Value)>(each: &mut F) -> ffi::ValueSink {
    unsafe extern "C" fn push<F: FnMut(&ffi::Value)>(data: *mut c_void, value: *const ffi::Value) {
        if value.is_null() {
            return;
        }
        // SAFETY: `data` is the `F` passed to `value_sink`, alive during the call that pushes;
        // `value` points to a valid value for the duration of the push.
        let (each, value) = unsafe { (&mut *data.cast::<F>(), &*value) };
        // A panic must not unwind into the engine.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| each(value)));
    }
    ffi::ValueSink {
        data: (each as *mut F).cast(),
        push: push::<F>,
    }
}

/// A [`ffi::StrSink`] appending to `out`.
pub(crate) fn string_sink(out: &mut String) -> ffi::StrSink {
    unsafe extern "C" fn push(data: *mut c_void, text: ffi::Str) {
        // SAFETY: `data` is the `String` passed to `string_sink`, alive during the push; the
        // text is valid during the push.
        let (out, text) = unsafe { (&mut *data.cast::<String>(), text.to_str()) };
        out.push_str(text.unwrap_or("<invalid UTF-8>"));
    }
    ffi::StrSink {
        data: (out as *mut String).cast(),
        push,
    }
}
