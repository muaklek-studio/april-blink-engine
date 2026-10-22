//! One host call: its context, arguments, results and errors.

use ab_core::{Color, Quat, Vec2, Vec3};
use ab_scene::{NodeHandle, SceneTree, Signal};
use ab_script_api as ffi;
use std::any::Any;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// The engine's side of an [`ffi::Context`]: what a script's event runs against. Lives on the
/// stack of the event dispatch; scripts get a pointer to it.
pub(crate) struct CallContext {
    pub tree: *mut SceneTree,
    /// Message of the last failed call, for [`ffi::HostApi::error`].
    pub error: String,
}

/// Why a binding failed.
#[derive(Debug)]
pub(crate) enum CallError {
    NoNode(NodeHandle),
    BadArguments(String),
    Failed(String),
}

impl CallError {
    pub fn failed(err: impl std::fmt::Display) -> Self {
        Self::Failed(err.to_string())
    }

    pub fn status(&self) -> ffi::Status {
        match self {
            Self::NoNode(_) => ffi::Status::NO_NODE,
            Self::BadArguments(_) => ffi::Status::BAD_ARGUMENTS,
            Self::Failed(_) => ffi::Status::ERROR,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::NoNode(node) => format!("{node} is not a valid node (freed or never created)"),
            Self::BadArguments(message) | Self::Failed(message) => message.clone(),
        }
    }
}

impl From<ab_scene::SceneError> for CallError {
    fn from(err: ab_scene::SceneError) -> Self {
        match err {
            ab_scene::SceneError::InvalidNode(node) => Self::NoNode(node),
            other => Self::Failed(other.to_string()),
        }
    }
}

pub(crate) type CallResult = Result<(), CallError>;

/// A call in progress: the tree, the arguments, where results go.
pub(crate) struct Call<'t> {
    pub tree: &'t mut SceneTree,
    args: &'t [ffi::Value],
    out: Option<&'t ffi::ValueSink>,
}

impl<'t> Call<'t> {
    pub fn new(
        tree: &'t mut SceneTree,
        args: &'t [ffi::Value],
        out: Option<&'t ffi::ValueSink>,
    ) -> Self {
        Self { tree, args, out }
    }

    pub fn len(&self) -> usize {
        self.args.len()
    }

    pub fn raw(&self, index: usize) -> ffi::Value {
        self.args.get(index).copied().unwrap_or(ffi::Value::NIL)
    }

    fn bad(&self, index: usize, expected: &str) -> CallError {
        CallError::BadArguments(format!("argument {} must be {expected}", index + 1))
    }

    /// A node argument that exists.
    pub fn node(&self, index: usize) -> Result<NodeHandle, CallError> {
        let node = self
            .raw(index)
            .as_node()
            .map(|n| NodeHandle::from_bits(n.0))
            .ok_or_else(|| self.bad(index, "a node"))?;
        if self.tree.is_valid(node) {
            Ok(node)
        } else {
            Err(CallError::NoNode(node))
        }
    }

    pub fn str(&self, index: usize) -> Result<&'t str, CallError> {
        // SAFETY: arguments are valid for the duration of the call.
        unsafe { self.raw(index).as_str() }.ok_or_else(|| self.bad(index, "a string"))
    }

    pub fn bool(&self, index: usize) -> Result<bool, CallError> {
        self.raw(index)
            .as_bool()
            .ok_or_else(|| self.bad(index, "a bool"))
    }

    pub fn f32(&self, index: usize) -> Result<f32, CallError> {
        self.raw(index)
            .as_float()
            .map(|f| f as f32)
            .ok_or_else(|| self.bad(index, "a number"))
    }

    pub fn opt_f32(&self, index: usize) -> Result<Option<f32>, CallError> {
        if self.raw(index).is_nil() {
            return Ok(None);
        }
        self.f32(index).map(Some)
    }

    pub fn i64(&self, index: usize) -> Result<i64, CallError> {
        self.raw(index)
            .as_int()
            .ok_or_else(|| self.bad(index, "an integer"))
    }

    pub fn u32(&self, index: usize) -> Result<u32, CallError> {
        u32::try_from(self.i64(index)?)
            .map_err(|_| self.bad(index, "a non-negative 32-bit integer"))
    }

    pub fn opt_u32(&self, index: usize) -> Result<Option<u32>, CallError> {
        if self.raw(index).is_nil() {
            return Ok(None);
        }
        self.u32(index).map(Some)
    }

    pub fn vec2(&self, index: usize) -> Result<Vec2, CallError> {
        from_vec2(&self.raw(index)).ok_or_else(|| self.bad(index, "a Vec2"))
    }

    pub fn vec3(&self, index: usize) -> Result<Vec3, CallError> {
        from_vec3(&self.raw(index)).ok_or_else(|| self.bad(index, "a Vec3"))
    }

    pub fn opt_vec3(&self, index: usize) -> Result<Option<Vec3>, CallError> {
        if self.raw(index).is_nil() {
            return Ok(None);
        }
        self.vec3(index).map(Some)
    }

    pub fn quat(&self, index: usize) -> Result<Quat, CallError> {
        from_quat(&self.raw(index)).ok_or_else(|| self.bad(index, "a Quat"))
    }

    pub fn color(&self, index: usize) -> Result<Color, CallError> {
        from_color(&self.raw(index)).ok_or_else(|| self.bad(index, "a Color"))
    }

    pub fn opt_color(&self, index: usize) -> Result<Option<Color>, CallError> {
        if self.raw(index).is_nil() {
            return Ok(None);
        }
        self.color(index).map(Some)
    }

    /// Node arguments from `index` on (filters' exclusion lists).
    pub fn nodes_from(&self, index: usize) -> Vec<NodeHandle> {
        self.args
            .iter()
            .skip(index)
            .filter_map(|v| v.as_node())
            .map(|n| NodeHandle::from_bits(n.0))
            .collect()
    }

    // =========================================================================================
    // Results
    // =========================================================================================

    pub fn push(&self, value: ffi::Value) {
        if let Some(out) = self.out {
            // SAFETY: the script's sink is valid for the call; the value lives during the push.
            unsafe { (out.push)(out.data, &value) };
        }
    }

    pub fn push_bool(&self, value: bool) {
        self.push(ffi::Value::bool(value));
    }

    pub fn push_int(&self, value: i64) {
        self.push(ffi::Value::int(value));
    }

    pub fn push_f32(&self, value: f32) {
        self.push(ffi::Value::float(f64::from(value)));
    }

    pub fn push_f64(&self, value: f64) {
        self.push(ffi::Value::float(value));
    }

    pub fn push_vec2(&self, value: Vec2) {
        self.push(to_vec2(value));
    }

    pub fn push_vec3(&self, value: Vec3) {
        self.push(to_vec3(value));
    }

    pub fn push_quat(&self, value: Quat) {
        self.push(to_quat(value));
    }

    pub fn push_node(&self, node: NodeHandle) {
        self.push(ffi::Value::node(ffi::NodeId(node.to_bits())));
    }

    pub fn push_opt_node(&self, node: Option<NodeHandle>) {
        match node {
            Some(node) => self.push_node(node),
            None => self.push(ffi::Value::NIL),
        }
    }

    pub fn push_str(&self, text: &str) {
        self.push(ffi::Value::str(ffi::Str::new(text)));
    }
}

// =============================================================================================
// Conversions
// =============================================================================================

pub(crate) fn to_vec2(v: Vec2) -> ffi::Value {
    ffi::Value::vec2(ffi::Vec2 { x: v.x, y: v.y })
}

pub(crate) fn to_vec3(v: Vec3) -> ffi::Value {
    ffi::Value::vec3(ffi::Vec3 {
        x: v.x,
        y: v.y,
        z: v.z,
    })
}

pub(crate) fn to_quat(q: Quat) -> ffi::Value {
    ffi::Value::quat(ffi::Quat {
        x: q.x,
        y: q.y,
        z: q.z,
        w: q.w,
    })
}

pub(crate) fn to_color(c: Color) -> ffi::Value {
    ffi::Value::color(ffi::Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    })
}

pub(crate) fn from_vec2(value: &ffi::Value) -> Option<Vec2> {
    value.as_vec2().map(|v| Vec2::new(v.x, v.y))
}

pub(crate) fn from_vec3(value: &ffi::Value) -> Option<Vec3> {
    value.as_vec3().map(|v| Vec3::new(v.x, v.y, v.z))
}

pub(crate) fn from_quat(value: &ffi::Value) -> Option<Quat> {
    value
        .as_quat()
        .map(|q| Quat::from_xyzw(q.x, q.y, q.z, q.w).normalize())
        .filter(|q| q.is_finite())
}

pub(crate) fn from_color(value: &ffi::Value) -> Option<Color> {
    value.as_color().map(|c| Color::rgba(c.r, c.g, c.b, c.a))
}

/// Signal names live as long as the program (engine signals use `&'static str`). Names from
/// scripts are interned: each distinct name is allocated once.
pub(crate) fn intern(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(&known) = names.get(name) {
        return known;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(leaked);
    leaked
}

/// A script's signal argument as the engine stores it (what native scripts read with
/// `Signal::arg::<T>()`).
pub(crate) fn signal_arg(value: &ffi::Value) -> Option<Box<dyn Any>> {
    Some(match value.tag {
        ffi::Tag::BOOL => Box::new(value.as_bool()?),
        ffi::Tag::INT => Box::new(value.as_int()?),
        ffi::Tag::FLOAT => Box::new(value.as_float()? as f32),
        ffi::Tag::VEC2 => Box::new(from_vec2(value)?),
        ffi::Tag::VEC3 => Box::new(from_vec3(value)?),
        ffi::Tag::QUAT => Box::new(from_quat(value)?),
        ffi::Tag::COLOR => Box::new(from_color(value)?),
        ffi::Tag::NODE => Box::new(NodeHandle::from_bits(value.as_node()?.0)),
        // SAFETY: the argument is valid during the call that converts it.
        ffi::Tag::STR => Box::new(unsafe { value.as_str() }?.to_owned()),
        _ => return None,
    })
}

/// An engine signal's argument for a script, keeping strings alive in `strings`.
pub(crate) fn signal_value(signal: &Signal, strings: &mut Vec<String>) -> Option<ffi::Value> {
    if let Some(node) = signal.arg::<NodeHandle>() {
        return Some(ffi::Value::node(ffi::NodeId(node.to_bits())));
    }
    if let Some(&v) = signal.arg::<bool>() {
        return Some(ffi::Value::bool(v));
    }
    if let Some(&v) = signal.arg::<f32>() {
        return Some(ffi::Value::float(f64::from(v)));
    }
    if let Some(&v) = signal.arg::<f64>() {
        return Some(ffi::Value::float(v));
    }
    let int = signal
        .arg::<i64>()
        .copied()
        .or_else(|| signal.arg::<i32>().map(|&v| i64::from(v)))
        .or_else(|| signal.arg::<u32>().map(|&v| i64::from(v)))
        .or_else(|| signal.arg::<usize>().and_then(|&v| i64::try_from(v).ok()));
    if let Some(v) = int {
        return Some(ffi::Value::int(v));
    }
    if let Some(&v) = signal.arg::<Vec2>() {
        return Some(to_vec2(v));
    }
    if let Some(&v) = signal.arg::<Vec3>() {
        return Some(to_vec3(v));
    }
    if let Some(&v) = signal.arg::<Quat>() {
        return Some(to_quat(v));
    }
    if let Some(&v) = signal.arg::<Color>() {
        return Some(to_color(v));
    }
    let text = signal
        .arg::<String>()
        .cloned()
        .or_else(|| signal.arg::<&'static str>().map(|s| (*s).to_owned()))?;
    strings.push(text);
    let text = strings.last().expect("just pushed");
    Some(ffi::Value::str(ffi::Str::new(text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interned_names_are_shared() {
        let a = intern("relic_placed");
        let b = intern(&String::from("relic_placed"));
        assert!(std::ptr::eq(a, b));
        assert_eq!(a, "relic_placed");
    }

    #[test]
    fn signal_args_cross_both_ways() {
        let node = NodeHandle::from_bits(42);
        let value = ffi::Value::node(ffi::NodeId(42));
        let signal = Signal::new(node, "x").with_arg_boxed(signal_arg(&value).unwrap());
        assert_eq!(signal.arg::<NodeHandle>(), Some(&node));
        let mut strings = Vec::new();
        assert_eq!(
            signal_value(&signal, &mut strings).and_then(|v| v.as_node()),
            Some(ffi::NodeId(42))
        );
        let text = Signal::new(node, "x").with_arg("hello".to_owned());
        let value = signal_value(&text, &mut strings).unwrap();
        // SAFETY: `strings` keeps the text alive.
        assert_eq!(unsafe { value.as_str() }, Some("hello"));
        let float = Signal::new(node, "x").with_arg(2.5_f32);
        assert_eq!(
            signal_value(&float, &mut strings).and_then(|v| v.as_float()),
            Some(2.5)
        );
    }
}
