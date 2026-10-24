//! The C ABI between April Blink Engine and script libraries.
//!
//! Rust has no stable ABI: two builds only agree on the layout of Rust types when they use the
//! same compiler, dependencies and settings. Script libraries therefore never see engine types.
//! Everything that crosses the boundary is defined here with `#[repr(C)]` and `extern "C"`, so
//! an editor or game built once loads scripts compiled anywhere, by any Rust version.
//!
//! Script authors use the `ab-script` SDK, which wraps this crate in a safe API; this crate is
//! for the SDK and the engine.
//!
//! # Shape
//!
//! * A script library is a `cdylib` exporting [`ENTRY_SYMBOL`], an [`EntryPoint`]. The engine
//!   calls it once with its [`HostApi`] and gets the [`Library`]: the [`ScriptType`]s it
//!   provides, by name.
//! * The engine creates script instances through [`ScriptType::create`] and drives them with
//!   [`Event`]s (ready, process, signals, …) through [`ScriptType::event`].
//! * During an event, scripts call back into the engine with [`HostApi::call`]: every engine
//!   feature is a *method* named by a string (`"node.set_position"`), resolved once to a
//!   [`MethodId`] by [`HostApi::method`]. Arguments and results are [`Value`]s. New engine
//!   features are new method names, so they never change this ABI.
//!
//! # Rules
//!
//! * Borrowed data ([`Str`], argument arrays, the [`Context`]) is only valid during the call
//!   that hands it over. Results come back through sinks ([`ValueSink`], [`StrSink`]) and are
//!   copied by the receiver during the push.
//! * Strings are UTF-8; receivers check it ([`Str::to_str`]).
//! * No panic or unwind crosses the boundary: both sides catch them and report [`Status`].
//! * A [`Context`] is only valid inside the [`ScriptType::event`] call it was passed to.
//! * Compatibility: a host accepts libraries with its own [`ABI_VERSION`]. The host may append
//!   fields to [`HostApi`]; [`HostApi::size`] tells the library which ones exist.

#![no_std]

use core::ffi::c_void;
use core::fmt;
use core::marker::{PhantomData, PhantomPinned};

/// Version of this ABI. Incremented on any incompatible change.
pub const ABI_VERSION: u32 = 2;

/// Name of the [`EntryPoint`] a script library exports.
pub const ENTRY_SYMBOL: &str = "ab_script_library";

/// [`ENTRY_SYMBOL`], NUL-terminated (for dynamic loaders).
pub const ENTRY_SYMBOL_NUL: &[u8] = b"ab_script_library\0";

/// The function a script library exports as [`ENTRY_SYMBOL`]. Called once per loaded copy of
/// the library; returns its description (valid as long as the library stays loaded), or null
/// if it cannot work with this host (the library logs why through [`HostApi::log`]).
pub type EntryPoint = unsafe extern "C" fn(host: *const HostApi) -> *const Library;

// =============================================================================================
// Plain data
// =============================================================================================

/// A borrowed UTF-8 string.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Str {
    pub ptr: *const u8,
    pub len: usize,
}

impl Str {
    pub const EMPTY: Self = Self {
        ptr: core::ptr::null(),
        len: 0,
    };

    pub const fn new(text: &str) -> Self {
        Self {
            ptr: text.as_ptr(),
            len: text.len(),
        }
    }

    /// The text, or `None` if it is not UTF-8.
    ///
    /// # Safety
    /// `ptr` must point to `len` readable bytes that stay valid and unchanged for `'a`
    /// (or `len` must be 0).
    pub unsafe fn to_str<'a>(self) -> Option<&'a str> {
        if self.len == 0 {
            return Some("");
        }
        if self.ptr.is_null() {
            return None;
        }
        // SAFETY: the caller guarantees `len` readable bytes at `ptr`.
        let bytes = unsafe { core::slice::from_raw_parts(self.ptr, self.len) };
        core::str::from_utf8(bytes).ok()
    }
}

impl Default for Str {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl fmt::Debug for Str {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Str")
            .field("ptr", &self.ptr)
            .field("len", &self.len)
            .finish()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// A unit quaternion.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Default for Quat {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }
    }
}

/// A linear RGBA color.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// A node of the scene tree. Stays unique for the node's lifetime; a freed node's id is
/// never valid again.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

// =============================================================================================
// Values
// =============================================================================================

/// What a [`Value`] holds.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tag(pub u32);

impl Tag {
    pub const NIL: Self = Self(0);
    pub const BOOL: Self = Self(1);
    pub const INT: Self = Self(2);
    pub const FLOAT: Self = Self(3);
    pub const VEC2: Self = Self(4);
    pub const VEC3: Self = Self(5);
    pub const QUAT: Self = Self(6);
    pub const COLOR: Self = Self(7);
    pub const NODE: Self = Self(8);
    pub const STR: Self = Self(9);
}

/// Payload of a [`Value`]; read it through the accessors on [`Value`] (only the field the
/// tag names is initialized).
#[repr(C)]
#[derive(Clone, Copy)]
pub union Data {
    /// [`Tag::INT`], and [`Tag::BOOL`] as 0 or 1.
    pub int: i64,
    pub float: f64,
    pub vec2: Vec2,
    pub vec3: Vec3,
    pub quat: Quat,
    pub color: Color,
    pub node: NodeId,
    pub str: Str,
}

/// An argument or result of a host method, or a signal argument.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Value {
    pub tag: Tag,
    pub data: Data,
}

impl fmt::Debug for Data {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Data { .. }")
    }
}

impl Value {
    pub const NIL: Self = Self {
        tag: Tag::NIL,
        data: Data { int: 0 },
    };

    pub const fn bool(value: bool) -> Self {
        Self {
            tag: Tag::BOOL,
            data: Data { int: value as i64 },
        }
    }

    pub const fn int(value: i64) -> Self {
        Self {
            tag: Tag::INT,
            data: Data { int: value },
        }
    }

    pub const fn float(value: f64) -> Self {
        Self {
            tag: Tag::FLOAT,
            data: Data { float: value },
        }
    }

    pub const fn vec2(value: Vec2) -> Self {
        Self {
            tag: Tag::VEC2,
            data: Data { vec2: value },
        }
    }

    pub const fn vec3(value: Vec3) -> Self {
        Self {
            tag: Tag::VEC3,
            data: Data { vec3: value },
        }
    }

    pub const fn quat(value: Quat) -> Self {
        Self {
            tag: Tag::QUAT,
            data: Data { quat: value },
        }
    }

    pub const fn color(value: Color) -> Self {
        Self {
            tag: Tag::COLOR,
            data: Data { color: value },
        }
    }

    pub const fn node(value: NodeId) -> Self {
        Self {
            tag: Tag::NODE,
            data: Data { node: value },
        }
    }

    /// A borrowed string: valid as long as the text it points to.
    pub const fn str(value: Str) -> Self {
        Self {
            tag: Tag::STR,
            data: Data { str: value },
        }
    }

    pub fn is_nil(&self) -> bool {
        self.tag == Tag::NIL
    }

    pub fn as_bool(&self) -> Option<bool> {
        if self.tag == Tag::BOOL {
            // SAFETY: the tag says which field is set.
            Some(unsafe { self.data.int } != 0)
        } else {
            None
        }
    }

    /// Integers, and floats with no fractional part.
    pub fn as_int(&self) -> Option<i64> {
        match self.tag {
            // SAFETY: the tag says which field is set.
            Tag::INT => Some(unsafe { self.data.int }),
            Tag::FLOAT => {
                // SAFETY: as above.
                let float = unsafe { self.data.float };
                // Exactly representable integers only (`fract` is not in `core`).
                let int = float as i64;
                (float.abs() < 9.0e15 && int as f64 == float).then_some(int)
            }
            _ => None,
        }
    }

    /// Floats and integers.
    pub fn as_float(&self) -> Option<f64> {
        match self.tag {
            // SAFETY: the tag says which field is set.
            Tag::FLOAT => Some(unsafe { self.data.float }),
            // SAFETY: as above.
            Tag::INT => Some(unsafe { self.data.int } as f64),
            _ => None,
        }
    }

    pub fn as_vec2(&self) -> Option<Vec2> {
        if self.tag == Tag::VEC2 {
            // SAFETY: the tag says which field is set.
            Some(unsafe { self.data.vec2 })
        } else {
            None
        }
    }

    pub fn as_vec3(&self) -> Option<Vec3> {
        if self.tag == Tag::VEC3 {
            // SAFETY: the tag says which field is set.
            Some(unsafe { self.data.vec3 })
        } else {
            None
        }
    }

    pub fn as_quat(&self) -> Option<Quat> {
        if self.tag == Tag::QUAT {
            // SAFETY: the tag says which field is set.
            Some(unsafe { self.data.quat })
        } else {
            None
        }
    }

    pub fn as_color(&self) -> Option<Color> {
        if self.tag == Tag::COLOR {
            // SAFETY: the tag says which field is set.
            Some(unsafe { self.data.color })
        } else {
            None
        }
    }

    pub fn as_node(&self) -> Option<NodeId> {
        if self.tag == Tag::NODE {
            // SAFETY: the tag says which field is set.
            Some(unsafe { self.data.node })
        } else {
            None
        }
    }

    /// The string, if this is one and it is UTF-8.
    ///
    /// # Safety
    /// The string must still be valid (see [`Value::str`]) for `'a`.
    pub unsafe fn as_str<'a>(&self) -> Option<&'a str> {
        if self.tag != Tag::STR {
            return None;
        }
        // SAFETY: the tag says the field is set; the caller guarantees the text is alive.
        unsafe { self.data.str.to_str() }
    }
}

impl Default for Value {
    fn default() -> Self {
        Self::NIL
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = f.debug_tuple("Value");
        match self.tag {
            Tag::NIL => out.field(&"nil"),
            Tag::BOOL => out.field(&self.as_bool()),
            Tag::INT => out.field(&self.as_int()),
            Tag::FLOAT => out.field(&self.as_float()),
            Tag::VEC2 => out.field(&self.as_vec2()),
            Tag::VEC3 => out.field(&self.as_vec3()),
            Tag::QUAT => out.field(&self.as_quat()),
            Tag::COLOR => out.field(&self.as_color()),
            Tag::NODE => out.field(&self.as_node()),
            // SAFETY: only the pointer and length are printed, not the text.
            Tag::STR => out.field(unsafe { &self.data.str }),
            other => out.field(&other),
        };
        out.finish()
    }
}

/// Receives values from the other side, one at a time. The receiver copies what it needs
/// during `push` (a pushed string is only valid during the push).
#[repr(C)]
#[derive(Debug)]
pub struct ValueSink {
    pub data: *mut c_void,
    pub push: unsafe extern "C" fn(data: *mut c_void, value: *const Value),
}

/// Receives a string from the other side (copied during `push`).
#[repr(C)]
#[derive(Debug)]
pub struct StrSink {
    pub data: *mut c_void,
    pub push: unsafe extern "C" fn(data: *mut c_void, text: Str),
}

// =============================================================================================
// The host
// =============================================================================================

/// The engine's state during an [`Event`]: opaque, only valid during that event.
#[repr(C)]
#[derive(Debug)]
pub struct Context {
    _private: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

/// A host method resolved with [`HostApi::method`]. Stable for the lifetime of the process.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MethodId(pub u32);

impl MethodId {
    /// No such method.
    pub const UNKNOWN: Self = Self(0);
}

/// Outcome of a call across the boundary.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[must_use]
pub struct Status(pub i32);

impl Status {
    pub const OK: Self = Self(0);
    /// Failed; the message is available ([`HostApi::error`], or the error sink).
    pub const ERROR: Self = Self(1);
    pub const UNKNOWN_METHOD: Self = Self(2);
    pub const BAD_ARGUMENTS: Self = Self(3);
    /// The node does not exist (anymore).
    pub const NO_NODE: Self = Self(4);
    /// The code on the other side panicked; it was caught there.
    pub const PANICKED: Self = Self(5);

    pub fn is_ok(self) -> bool {
        self == Self::OK
    }
}

/// Severity for [`HostApi::log`].
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LogLevel(pub u32);

impl LogLevel {
    pub const ERROR: Self = Self(1);
    pub const WARN: Self = Self(2);
    pub const INFO: Self = Self(3);
    pub const DEBUG: Self = Self(4);
    pub const TRACE: Self = Self(5);
}

/// What the engine offers to script libraries. Lives for the whole process.
#[repr(C)]
#[derive(Debug)]
pub struct HostApi {
    pub abi_version: u32,
    /// `size_of::<HostApi>()` in the host: fields beyond it do not exist.
    pub size: u32,
    /// The engine's version, e.g. `"0.1.0"`.
    pub engine_version: Str,
    /// Writes to the game's log (and the editor's console).
    pub log: unsafe extern "C" fn(level: LogLevel, target: Str, message: Str),
    /// Resolves a method name; [`MethodId::UNKNOWN`] if this engine has no such method.
    pub method: unsafe extern "C" fn(name: Str) -> MethodId,
    /// Calls a method. Results are pushed into `out` (which may be null to ignore them).
    pub call: unsafe extern "C" fn(
        ctx: *mut Context,
        method: MethodId,
        args: *const Value,
        arg_count: usize,
        out: *mut ValueSink,
    ) -> Status,
    /// The message of the last failed [`HostApi::call`] in this context.
    pub error: unsafe extern "C" fn(ctx: *mut Context, out: *mut StrSink),
    /// The instance of `script_type` attached to `node`, or null. Only for types of the
    /// calling library (so the caller knows the instance's layout); the instance stays valid
    /// until the caller's next host call or the end of the event, whichever comes first.
    pub script: unsafe extern "C" fn(
        ctx: *mut Context,
        node: NodeId,
        script_type: *const ScriptType,
    ) -> *mut c_void,
}

// =============================================================================================
// The library
// =============================================================================================

/// What a script library provides. Returned by its [`EntryPoint`].
#[repr(C)]
#[derive(Debug)]
pub struct Library {
    /// [`ABI_VERSION`] the library was built against.
    pub abi_version: u32,
    /// Version of the SDK that built it.
    pub sdk_version: Str,
    pub script_types: *const ScriptType,
    pub script_type_count: usize,
}

/// A property exported by a script, shown in the editor inspector.
#[repr(C)]
#[derive(Clone, Debug)]
pub struct PropertyDesc {
    pub name: Str,
    /// The type of this property.
    pub kind: Tag,
    /// The default value.
    pub default_value: Value,
    /// Property flags (bit 0: hidden).
    pub flags: u32,
    /// Optional tooltip/documentation.
    pub doc: Str,
}

/// One kind of script, attached to nodes by [`ScriptType::name`].
#[repr(C)]
#[derive(Debug)]
pub struct ScriptType {
    pub name: Str,
    /// Reserved, 0.
    pub flags: u32,
    /// A new instance with its default state; null if creating it failed (logged).
    pub create: unsafe extern "C" fn() -> *mut c_void,
    pub destroy: unsafe extern "C" fn(instance: *mut c_void),
    /// Runs one event. On failure the message is pushed into `error`.
    pub event: unsafe extern "C" fn(
        instance: *mut c_void,
        ctx: *mut Context,
        event: *const Event,
        error: *mut StrSink,
    ) -> Status,
    /// Hot reload: pushes the state to carry over to a new version of the library.
    /// Returns whether there is one.
    pub save_state: unsafe extern "C" fn(instance: *const c_void, out: *mut StrSink) -> bool,
    /// Hot reload: restores a state saved by [`ScriptType::save_state`] (possibly by an older
    /// version of the library).
    pub load_state:
        unsafe extern "C" fn(instance: *mut c_void, state: Str, error: *mut StrSink) -> Status,
    /// Exported properties (for the editor inspector). May be null if count is 0.
    pub properties: *const PropertyDesc,
    pub property_count: usize,
    /// Get a property value by name. Returns `true` if the property exists.
    pub get_property: Option<unsafe extern "C" fn(instance: *const c_void, name: Str, out: *mut Value) -> Status>,
    /// Set a property value by name. Returns `true` if the property exists and was set.
    pub set_property: Option<unsafe extern "C" fn(instance: *mut c_void, name: Str, value: *const Value) -> Status>,
}

/// Which [`Event`] it is.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EventKind(pub u32);

impl EventKind {
    /// The node entered the tree (parents before children).
    pub const ENTER_TREE: Self = Self(1);
    /// The node and its children are in the tree; once, children first.
    pub const READY: Self = Self(2);
    /// Every frame; [`Event::delta`] is the frame time in seconds.
    pub const PROCESS: Self = Self(3);
    /// Every fixed step; [`Event::delta`] is the step in seconds.
    pub const PHYSICS_PROCESS: Self = Self(4);
    /// The node is leaving the tree (children before parents).
    pub const EXIT_TREE: Self = Self(5);
    /// A signal connected to the node: [`Event::signal`], [`Event::source`], [`Event::args`].
    pub const SIGNAL: Self = Self(6);
    /// Hot reload replaced the previous version of this script (instead of `READY`).
    pub const RELOADED: Self = Self(7);
}

/// Something that happened to the node a script is attached to.
#[repr(C)]
#[derive(Debug)]
pub struct Event {
    pub kind: EventKind,
    pub node: NodeId,
    pub delta: f32,
    /// Signals: the signal's name.
    pub signal: Str,
    /// Signals: the node that emitted it.
    pub source: NodeId,
    /// Signals: its arguments.
    pub args: *const Value,
    pub arg_count: usize,
}

impl Event {
    /// An event without signal data.
    pub fn new(kind: EventKind, node: NodeId, delta: f32) -> Self {
        Self {
            kind,
            node,
            delta,
            signal: Str::EMPTY,
            source: NodeId(0),
            args: core::ptr::null(),
            arg_count: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use core::mem::{align_of, offset_of, size_of};
    use std::format;

    /// The layout is the ABI: these only change with [`ABI_VERSION`].
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn layouts_are_frozen() {
        assert_eq!(size_of::<Str>(), 16);
        assert_eq!(size_of::<Value>(), 24);
        assert_eq!(align_of::<Value>(), 8);
        assert_eq!(size_of::<Event>(), 64);
        assert_eq!(size_of::<HostApi>(), 64);
        assert_eq!(size_of::<ScriptType>(), 96);
        assert_eq!(size_of::<Library>(), 40);
    }

    #[test]
    fn values_round_trip() {
        assert_eq!(Value::bool(true).as_bool(), Some(true));
        assert_eq!(Value::int(-3).as_int(), Some(-3));
        assert_eq!(Value::int(2).as_float(), Some(2.0));
        assert_eq!(Value::float(4.0).as_int(), Some(4));
        assert_eq!(Value::float(4.5).as_int(), None);
        assert_eq!(Value::node(NodeId(7)).as_node(), Some(NodeId(7)));
        assert_eq!(Value::NIL.as_float(), None);
        let v = Vec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        };
        assert_eq!(Value::vec3(v).as_vec3(), Some(v));
        assert_eq!(Value::vec3(v).as_vec2(), None);
        let text = "สวัสดี";
        // SAFETY: `text` outlives the value.
        assert_eq!(unsafe { Value::str(Str::new(text)).as_str() }, Some(text));
        // SAFETY: empty strings need no memory.
        assert_eq!(unsafe { Str::EMPTY.to_str() }, Some(""));
        let bad = [0xff_u8, 0xfe];
        let bad = Str {
            ptr: bad.as_ptr(),
            len: 2,
        };
        // SAFETY: `bad` points to two bytes.
        assert_eq!(unsafe { bad.to_str() }, None);
    }

    /// Bumping [`ABI_VERSION`] must come with updating the layouts pinned in this module.
    #[test]
    fn abi_version_and_entry_symbol() {
        assert_eq!(ABI_VERSION, 2);
        assert_eq!(ENTRY_SYMBOL, "ab_script_library");
        assert_eq!(
            ENTRY_SYMBOL_NUL.split_last(),
            Some((&0, ENTRY_SYMBOL.as_bytes()))
        );
    }

    #[test]
    fn plain_data_layouts() {
        assert_eq!((size_of::<Vec2>(), align_of::<Vec2>()), (8, 4));
        assert_eq!((size_of::<Vec3>(), align_of::<Vec3>()), (12, 4));
        assert_eq!((size_of::<Quat>(), align_of::<Quat>()), (16, 4));
        assert_eq!((size_of::<Color>(), align_of::<Color>()), (16, 4));
        assert_eq!(offset_of!(Quat, w), 12);
        assert_eq!(offset_of!(Color, a), 12);
        assert_eq!((size_of::<NodeId>(), align_of::<NodeId>()), (8, 8));
        for size in [
            size_of::<Tag>(),
            size_of::<MethodId>(),
            size_of::<Status>(),
            size_of::<LogLevel>(),
            size_of::<EventKind>(),
        ] {
            assert_eq!(size, 4);
        }
        assert_eq!(size_of::<Context>(), 0);
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn field_offsets_are_frozen() {
        assert_eq!(offset_of!(Str, ptr), 0);
        assert_eq!(offset_of!(Str, len), 8);
        assert_eq!(size_of::<Data>(), 16);
        assert_eq!(offset_of!(Value, tag), 0);
        assert_eq!(offset_of!(Value, data), 8);

        assert_eq!(size_of::<ValueSink>(), 16);
        assert_eq!(offset_of!(ValueSink, push), 8);
        assert_eq!(size_of::<StrSink>(), 16);
        assert_eq!(offset_of!(StrSink, push), 8);

        assert_eq!(offset_of!(HostApi, abi_version), 0);
        assert_eq!(offset_of!(HostApi, size), 4);
        assert_eq!(offset_of!(HostApi, engine_version), 8);
        assert_eq!(offset_of!(HostApi, log), 24);
        assert_eq!(offset_of!(HostApi, method), 32);
        assert_eq!(offset_of!(HostApi, call), 40);
        assert_eq!(offset_of!(HostApi, error), 48);
        assert_eq!(offset_of!(HostApi, script), 56);

        assert_eq!(offset_of!(Library, abi_version), 0);
        assert_eq!(offset_of!(Library, sdk_version), 8);
        assert_eq!(offset_of!(Library, script_types), 24);
        assert_eq!(offset_of!(Library, script_type_count), 32);

        assert_eq!(size_of::<PropertyDesc>(), 72);
        assert_eq!(offset_of!(PropertyDesc, name), 0);
        assert_eq!(offset_of!(PropertyDesc, kind), 16);
        assert_eq!(offset_of!(PropertyDesc, default_value), 24);
        assert_eq!(offset_of!(PropertyDesc, flags), 48);
        assert_eq!(offset_of!(PropertyDesc, doc), 56);

        assert_eq!(offset_of!(ScriptType, name), 0);
        assert_eq!(offset_of!(ScriptType, flags), 16);
        assert_eq!(offset_of!(ScriptType, create), 24);
        assert_eq!(offset_of!(ScriptType, destroy), 32);
        assert_eq!(offset_of!(ScriptType, event), 40);
        assert_eq!(offset_of!(ScriptType, save_state), 48);
        assert_eq!(offset_of!(ScriptType, load_state), 56);
        assert_eq!(offset_of!(ScriptType, properties), 64);
        assert_eq!(offset_of!(ScriptType, property_count), 72);
        assert_eq!(offset_of!(ScriptType, get_property), 80);
        assert_eq!(offset_of!(ScriptType, set_property), 88);

        assert_eq!(offset_of!(Event, kind), 0);
        assert_eq!(offset_of!(Event, node), 8);
        assert_eq!(offset_of!(Event, delta), 16);
        assert_eq!(offset_of!(Event, signal), 24);
        assert_eq!(offset_of!(Event, source), 40);
        assert_eq!(offset_of!(Event, args), 48);
        assert_eq!(offset_of!(Event, arg_count), 56);
    }

    /// The numeric values of the enum-like newtypes cross the boundary too.
    #[test]
    fn constants_are_frozen() {
        let tags = [
            Tag::NIL,
            Tag::BOOL,
            Tag::INT,
            Tag::FLOAT,
            Tag::VEC2,
            Tag::VEC3,
            Tag::QUAT,
            Tag::COLOR,
            Tag::NODE,
            Tag::STR,
        ];
        for (i, tag) in tags.into_iter().enumerate() {
            assert_eq!(tag.0 as usize, i);
        }
        let statuses = [
            Status::OK,
            Status::ERROR,
            Status::UNKNOWN_METHOD,
            Status::BAD_ARGUMENTS,
            Status::NO_NODE,
            Status::PANICKED,
        ];
        for (i, status) in statuses.into_iter().enumerate() {
            assert_eq!(status.0 as usize, i);
        }
        let levels = [
            LogLevel::ERROR,
            LogLevel::WARN,
            LogLevel::INFO,
            LogLevel::DEBUG,
            LogLevel::TRACE,
        ];
        for (i, level) in levels.into_iter().enumerate() {
            assert_eq!(level.0 as usize, i + 1);
        }
        let kinds = [
            EventKind::ENTER_TREE,
            EventKind::READY,
            EventKind::PROCESS,
            EventKind::PHYSICS_PROCESS,
            EventKind::EXIT_TREE,
            EventKind::SIGNAL,
            EventKind::RELOADED,
        ];
        for (i, kind) in kinds.into_iter().enumerate() {
            assert_eq!(kind.0 as usize, i + 1);
        }
        assert_eq!(MethodId::UNKNOWN, MethodId(0));
    }

    #[test]
    fn status_is_ok_only_for_ok() {
        assert!(Status::OK.is_ok());
        assert!(!Status::ERROR.is_ok());
        assert!(!Status::PANICKED.is_ok());
        assert!(!Status(-1).is_ok());
    }

    #[test]
    fn defaults() {
        assert!(Value::default().is_nil());
        assert_eq!(
            Quat::default(),
            Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0
            }
        );
        assert_eq!(
            Vec3::default(),
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0
            }
        );
        let empty = Str::default();
        assert!(empty.ptr.is_null());
        assert_eq!(empty.len, 0);
    }

    #[test]
    fn values_only_answer_for_their_tag() {
        let all = [
            Value::NIL,
            Value::bool(false),
            Value::int(1),
            Value::float(1.5),
            Value::vec2(Vec2 { x: 1.0, y: 2.0 }),
            Value::vec3(Vec3::default()),
            Value::quat(Quat::default()),
            Value::color(Color::default()),
            Value::node(NodeId(1)),
            Value::str(Str::new("x")),
        ];
        for (i, value) in all.iter().enumerate() {
            assert_eq!(value.tag.0 as usize, i);
            assert_eq!(value.is_nil(), i == 0);
            assert_eq!(value.as_bool().is_some(), i == 1);
            // Ints and floats convert into each other.
            assert_eq!(value.as_float().is_some(), i == 2 || i == 3);
            assert_eq!(value.as_vec2().is_some(), i == 4);
            assert_eq!(value.as_vec3().is_some(), i == 5);
            assert_eq!(value.as_quat().is_some(), i == 6);
            assert_eq!(value.as_color().is_some(), i == 7);
            assert_eq!(value.as_node().is_some(), i == 8);
            // SAFETY: the only string value points to a static.
            assert_eq!(unsafe { value.as_str() }.is_some(), i == 9);
        }
        assert_eq!(Value::bool(false).as_bool(), Some(false));
        assert_eq!(Value::bool(true).as_int(), None);
    }

    #[test]
    fn floats_become_ints_only_when_exact() {
        assert_eq!(Value::float(-7.0).as_int(), Some(-7));
        assert_eq!(Value::float(-0.0).as_int(), Some(0));
        assert_eq!(Value::float(1e15).as_int(), Some(1_000_000_000_000_000));
        assert_eq!(Value::float(1e16).as_int(), None);
        assert_eq!(Value::float(f64::NAN).as_int(), None);
        assert_eq!(Value::float(f64::INFINITY).as_int(), None);
        assert_eq!(Value::float(f64::NEG_INFINITY).as_int(), None);
        assert_eq!(Value::float(0.1).as_int(), None);
    }

    #[test]
    fn null_strings_with_a_length_are_rejected() {
        let null = Str {
            ptr: core::ptr::null(),
            len: 4,
        };
        // SAFETY: a null pointer is checked before reading.
        assert_eq!(unsafe { null.to_str() }, None);
        let dangling = Str {
            ptr: core::ptr::null(),
            len: 0,
        };
        // SAFETY: nothing is read for an empty string.
        assert_eq!(unsafe { dangling.to_str() }, Some(""));
    }

    #[test]
    fn value_debug_shows_the_payload() {
        assert_eq!(format!("{:?}", Value::NIL), "Value(\"nil\")");
        assert_eq!(format!("{:?}", Value::int(5)), "Value(Some(5))");
        assert_eq!(
            format!("{:?}", Value::node(NodeId(9))),
            "Value(Some(NodeId(9)))"
        );
        let unknown = Value {
            tag: Tag(77),
            data: Data { int: 0 },
        };
        assert_eq!(format!("{unknown:?}"), "Value(Tag(77))");
    }

    #[test]
    fn plain_events_have_no_signal_data() {
        let event = Event::new(EventKind::PROCESS, NodeId(3), 0.016);
        assert_eq!(event.kind, EventKind::PROCESS);
        assert_eq!(event.node, NodeId(3));
        assert_eq!(event.delta, 0.016);
        assert_eq!(event.signal.len, 0);
        assert_eq!(event.source, NodeId(0));
        assert!(event.args.is_null());
        assert_eq!(event.arg_count, 0);
    }
}
