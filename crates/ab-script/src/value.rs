//! Types crossing to the engine and their conversions.

use ab_script_api as ffi;
use glam::{Quat, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::fmt;

/// A node of the scene tree. Copy it freely and keep it in your script; once the node is
/// freed every call with it fails (or returns `None`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Node(u64);

impl Node {
    /// The raw id the engine uses (for logs, saves, and tests that talk to the engine).
    pub const fn to_bits(self) -> u64 {
        self.0
    }

    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    pub(crate) fn id(self) -> ffi::NodeId {
        ffi::NodeId(self.0)
    }
}

impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Node({:#x})", self.0)
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.0)
    }
}

/// A linear RGBA color (what lights, materials and UI use).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);
    pub const TRANSPARENT: Self = Self::rgba(0.0, 0.0, 0.0, 0.0);

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// From 8-bit sRGB components (what color pickers show); alpha stays linear.
    pub fn from_srgb8(r: u8, g: u8, b: u8, a: u8) -> Self {
        let linear = |c: u8| {
            let c = f32::from(c) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        Self::rgba(linear(r), linear(g), linear(b), f32::from(a) / 255.0)
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::WHITE
    }
}

/// Why a call to the engine failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

/// The kind of an [`Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The node does not exist (anymore).
    NoNode,
    /// This engine does not have the method: it is older than the SDK.
    UnknownMethod,
    BadArguments,
    /// The engine failed (the message says why).
    Failed,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// A value the engine accepts as an argument. Strings are borrowed for the call.
pub trait IntoValue {
    #[doc(hidden)]
    fn to_value(&self) -> ffi::Value;
}

/// A value the engine can return.
pub trait FromValue: Sized {
    #[doc(hidden)]
    fn from_value(value: &ffi::Value) -> Option<Self>;
}

fn v2(v: Vec2) -> ffi::Vec2 {
    ffi::Vec2 { x: v.x, y: v.y }
}

pub(crate) fn v3(v: Vec3) -> ffi::Vec3 {
    ffi::Vec3 {
        x: v.x,
        y: v.y,
        z: v.z,
    }
}

impl IntoValue for bool {
    fn to_value(&self) -> ffi::Value {
        ffi::Value::bool(*self)
    }
}

macro_rules! int_value {
    ($($t:ty),*) => {$(
        impl IntoValue for $t {
            fn to_value(&self) -> ffi::Value {
                ffi::Value::int(i64::try_from(*self).unwrap_or(i64::MAX))
            }
        }
        impl FromValue for $t {
            fn from_value(value: &ffi::Value) -> Option<Self> {
                value.as_int().and_then(|v| Self::try_from(v).ok())
            }
        }
    )*};
}
int_value!(i32, i64, u32, u64, usize);

impl IntoValue for f32 {
    fn to_value(&self) -> ffi::Value {
        ffi::Value::float(f64::from(*self))
    }
}

impl IntoValue for f64 {
    fn to_value(&self) -> ffi::Value {
        ffi::Value::float(*self)
    }
}

impl IntoValue for Vec2 {
    fn to_value(&self) -> ffi::Value {
        ffi::Value::vec2(v2(*self))
    }
}

impl IntoValue for Vec3 {
    fn to_value(&self) -> ffi::Value {
        ffi::Value::vec3(v3(*self))
    }
}

impl IntoValue for Quat {
    fn to_value(&self) -> ffi::Value {
        ffi::Value::quat(ffi::Quat {
            x: self.x,
            y: self.y,
            z: self.z,
            w: self.w,
        })
    }
}

impl IntoValue for Color {
    fn to_value(&self) -> ffi::Value {
        ffi::Value::color(ffi::Color {
            r: self.r,
            g: self.g,
            b: self.b,
            a: self.a,
        })
    }
}

impl IntoValue for Node {
    fn to_value(&self) -> ffi::Value {
        ffi::Value::node(self.id())
    }
}

impl IntoValue for str {
    fn to_value(&self) -> ffi::Value {
        ffi::Value::str(ffi::Str::new(self))
    }
}

impl IntoValue for String {
    fn to_value(&self) -> ffi::Value {
        self.as_str().to_value()
    }
}

impl<T: IntoValue + ?Sized> IntoValue for &T {
    fn to_value(&self) -> ffi::Value {
        (**self).to_value()
    }
}

impl<T: IntoValue> IntoValue for Option<T> {
    fn to_value(&self) -> ffi::Value {
        self.as_ref().map_or(ffi::Value::NIL, T::to_value)
    }
}

impl FromValue for bool {
    fn from_value(value: &ffi::Value) -> Option<Self> {
        value.as_bool()
    }
}

impl FromValue for f32 {
    fn from_value(value: &ffi::Value) -> Option<Self> {
        value.as_float().map(|v| v as f32)
    }
}

impl FromValue for f64 {
    fn from_value(value: &ffi::Value) -> Option<Self> {
        value.as_float()
    }
}

impl FromValue for Vec2 {
    fn from_value(value: &ffi::Value) -> Option<Self> {
        value.as_vec2().map(|v| Vec2::new(v.x, v.y))
    }
}

impl FromValue for Vec3 {
    fn from_value(value: &ffi::Value) -> Option<Self> {
        value.as_vec3().map(|v| Vec3::new(v.x, v.y, v.z))
    }
}

impl FromValue for Quat {
    fn from_value(value: &ffi::Value) -> Option<Self> {
        value.as_quat().map(|q| Quat::from_xyzw(q.x, q.y, q.z, q.w))
    }
}

impl FromValue for Color {
    fn from_value(value: &ffi::Value) -> Option<Self> {
        value.as_color().map(|c| Color::rgba(c.r, c.g, c.b, c.a))
    }
}

impl FromValue for Node {
    fn from_value(value: &ffi::Value) -> Option<Self> {
        value.as_node().map(|n| Node(n.0))
    }
}

impl FromValue for String {
    fn from_value(value: &ffi::Value) -> Option<Self> {
        // SAFETY: values are only read during the push that hands them over.
        unsafe { value.as_str() }.map(str::to_owned)
    }
}

impl<T: FromValue> FromValue for Option<T> {
    fn from_value(value: &ffi::Value) -> Option<Self> {
        if value.is_nil() {
            Some(None)
        } else {
            T::from_value(value).map(Some)
        }
    }
}

/// A signal argument kept by the SDK (signals carry any of these).
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Vec2(Vec2),
    Vec3(Vec3),
    Quat(Quat),
    Color(Color),
    Node(Node),
    Str(String),
}

impl Arg {
    pub(crate) fn from_ffi(value: &ffi::Value) -> Self {
        match value.tag {
            ffi::Tag::BOOL => value.as_bool().map_or(Self::Nil, Self::Bool),
            ffi::Tag::INT => value.as_int().map_or(Self::Nil, Self::Int),
            ffi::Tag::FLOAT => value.as_float().map_or(Self::Nil, Self::Float),
            ffi::Tag::VEC2 => Vec2::from_value(value).map_or(Self::Nil, Self::Vec2),
            ffi::Tag::VEC3 => Vec3::from_value(value).map_or(Self::Nil, Self::Vec3),
            ffi::Tag::QUAT => Quat::from_value(value).map_or(Self::Nil, Self::Quat),
            ffi::Tag::COLOR => Color::from_value(value).map_or(Self::Nil, Self::Color),
            ffi::Tag::NODE => Node::from_value(value).map_or(Self::Nil, Self::Node),
            ffi::Tag::STR => String::from_value(value).map_or(Self::Nil, Self::Str),
            _ => Self::Nil,
        }
    }

    /// The argument as `T`, if it is one (numbers convert between integer and float).
    pub fn get<T: FromValue>(&self) -> Option<T> {
        T::from_value(&self.to_value())
    }
}

impl IntoValue for Arg {
    fn to_value(&self) -> ffi::Value {
        match self {
            Self::Nil => ffi::Value::NIL,
            Self::Bool(v) => v.to_value(),
            Self::Int(v) => v.to_value(),
            Self::Float(v) => v.to_value(),
            Self::Vec2(v) => v.to_value(),
            Self::Vec3(v) => v.to_value(),
            Self::Quat(v) => v.to_value(),
            Self::Color(v) => v.to_value(),
            Self::Node(v) => v.to_value(),
            Self::Str(v) => v.to_value(),
        }
    }
}

macro_rules! arg_from {
    ($($t:ty => $variant:ident),*) => {$(
        impl From<$t> for Arg {
            fn from(value: $t) -> Self {
                Self::$variant(value.into())
            }
        }
    )*};
}
arg_from!(bool => Bool, i32 => Int, i64 => Int, u32 => Int, f32 => Float, f64 => Float,
    Vec2 => Vec2, Vec3 => Vec3, Quat => Quat, Color => Color, Node => Node, String => Str,
    &str => Str);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_round_trip() {
        assert_eq!(f32::from_value(&2.5_f32.to_value()), Some(2.5));
        assert_eq!(u32::from_value(&7_u32.to_value()), Some(7));
        assert_eq!(u32::from_value(&(-1_i64).to_value()), None);
        let v = Vec3::new(1.0, -2.0, 3.5);
        assert_eq!(Vec3::from_value(&v.to_value()), Some(v));
        let q = Quat::from_rotation_y(0.4);
        assert_eq!(Quat::from_value(&q.to_value()), Some(q));
        let n = Node::from_bits(0x1_0000_0002);
        assert_eq!(Node::from_value(&n.to_value()), Some(n));
        assert_eq!(Option::<Node>::from_value(&ffi::Value::NIL), Some(None));
        assert_eq!(String::from_value(&"hé".to_value()).as_deref(), Some("hé"));
        assert_eq!(Arg::from_ffi(&v.to_value()), Arg::Vec3(v));
        assert_eq!(Arg::Int(3).get::<f32>(), Some(3.0));
    }

    #[test]
    fn srgb_conversion() {
        let c = Color::from_srgb8(255, 0, 128, 255);
        assert_eq!((c.r, c.g, c.a), (1.0, 0.0, 1.0));
        assert!((c.b - 0.2158).abs() < 1e-3);
    }
}
