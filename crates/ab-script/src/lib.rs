//! # ab-script: game scripts for April Blink Engine
//!
//! Write the behaviour of your game's nodes in Rust. A script crate is a `cdylib` built with
//! any Rust compiler; the editor and shipped games load it through a stable C ABI
//! ([`ab_script_api`]), so they never need to be rebuilt with your code, and your code never
//! needs the engine's sources.
//!
//! ```ignore
//! use ab_script::prelude::*;
//!
//! /// Spins its node around Y.
//! #[derive(Default, Serialize, Deserialize)]
//! #[serde(default)]
//! pub struct Spinner {
//!     speed: f32,
//! }
//!
//! impl Script for Spinner {
//!     serde_state!();
//!
//!     fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
//!         let _ = tree.rotate(node, Vec3::Y, self.speed * delta);
//!     }
//! }
//!
//! fn register(registry: &mut Registry) {
//!     registry.register::<Spinner>("Spinner");
//! }
//!
//! ab_script::export!(register);
//! ```
//!
//! with, in `Cargo.toml`:
//!
//! ```toml
//! [lib]
//! crate-type = ["cdylib"]
//!
//! [dependencies]
//! ab-script = "0.1"
//! ```
//!
//! Scene files attach scripts by their registered name (`script = "Spinner"`). Scripts talk
//! to the engine through the [`Tree`] each callback receives: nodes, transforms, groups,
//! signals, input, physics, UI, effects, audio, navigation, saves.

pub mod host;
mod math;
mod physics;
mod script;
mod tree;
mod ui;
mod util;
mod value;
mod world;

pub use ab_script_api as ffi;
pub use math::{EulerRot, Mat3, Mat4, Quat, Transform, Vec2, Vec3, Vec4};
pub use physics::{
    signals, Body, BodyKind, Character, ClimbRef, ClimbState, Physics, PhysicsEvent, QueryOptions,
    RayHit,
};
pub use script::{script_type, Property, Registry, Script, Signal};
pub use tree::{Connection, Engine, Input, RenderStats, Saves, Tree};
pub use ui::{
    BoxContainerRef, ButtonRef, ControlRef, Easing, HAlign, LabelRef, LayoutPreset, MouseFilter,
    PanelRef, ProgressBarRef, VAlign, PRESSED,
};
pub use util::{PathFollower, Rng};
pub use value::{Arg, Color, Error, ErrorKind, FromValue, IntoValue, Node};
pub use world::{
    AnimationRef, AudioPlayerRef, Blood, CameraRef, Explosion, LightRef, LocomotionRef,
    LocomotionState, MeshRef, NavBakeSettings, ParticleEffect, ParticlesRef, Repeat, SoundOptions,
    TimerRef, TweenBuilder, TIMEOUT, TWEEN_FINISHED,
};

/// Everything a script usually needs: `use ab_script::prelude::*;`.
pub mod prelude {
    pub use crate::{
        debug, error, ffi, info, serde_state, trace, warn, Arg, Blood, BodyKind, Color, Easing,
        EulerRot, Explosion, HAlign, LayoutPreset, MouseFilter, NavBakeSettings, Node,
        ParticleEffect, PathFollower, PhysicsEvent, Property, Quat, QueryOptions, Registry, Repeat,
        Rng, Script, Signal, SoundOptions, Transform, Tree, VAlign, Vec2, Vec3,
    };
    pub use serde::{Deserialize, Serialize};
}

/// Writes to the game's log (and the editor's console) at the given level.
#[macro_export]
macro_rules! log {
    ($level:expr, $($arg:tt)+) => {
        $crate::host::log($level, ::core::module_path!(), ::core::format_args!($($arg)+))
    };
}

/// Logs an error: something failed and the player may notice.
#[macro_export]
macro_rules! error {
    ($($arg:tt)+) => { $crate::log!($crate::ffi::LogLevel::ERROR, $($arg)+) };
}

/// Logs a warning: something is off but the game copes.
#[macro_export]
macro_rules! warn {
    ($($arg:tt)+) => { $crate::log!($crate::ffi::LogLevel::WARN, $($arg)+) };
}

/// Logs what happens in the game (round started, level loaded).
#[macro_export]
macro_rules! info {
    ($($arg:tt)+) => { $crate::log!($crate::ffi::LogLevel::INFO, $($arg)+) };
}

/// Logs details for debugging (hidden by default).
#[macro_export]
macro_rules! debug {
    ($($arg:tt)+) => { $crate::log!($crate::ffi::LogLevel::DEBUG, $($arg)+) };
}

/// Logs very frequent details (hidden by default).
#[macro_export]
macro_rules! trace {
    ($($arg:tt)+) => { $crate::log!($crate::ffi::LogLevel::TRACE, $($arg)+) };
}

#[doc(hidden)]
pub mod __private {
    pub use crate::script::entry;
    pub use serde_json;
}
