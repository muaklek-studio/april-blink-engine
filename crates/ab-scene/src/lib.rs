//! Scene layer: a Godot-like Node Tree API on top of the [`ab_ecs`] data layer.
//!
//! Every node is an ECS entity carrying [`NodeInfo`], [`Children`] and (when it has a parent)
//! [`Parent`]. `Node3D`s additionally carry a [`Transform`]. [`SceneTree`] owns the
//! [`World`](ab_ecs::World) and provides the user-facing API: `add_child`, `get_node`,
//! `queue_free`, transforms, and the `enter_tree → ready → process → exit_tree` lifecycle
//! dispatched to [`NodeScript`]s.
//!
//! Lifecycle order matches Godot:
//! * `enter_tree`: parents before children (pre-order).
//! * `ready`: children before parents (post-order), once per node.
//! * `process` / `physics_process`: tree order (pre-order).
//! * `exit_tree`: children (last to first) before parents.
//! * `queue_free`: the subtree is removed at the end of the frame.

mod components;
mod error;
mod handle;
mod path;
mod profile;
mod property;
mod registry;
mod scene_tree;
mod script;
mod scripting;
mod signal;
mod systems;

pub use components::{Children, NodeInfo, Parent, Transform};
pub use error::SceneError;
pub use handle::NodeHandle;
pub use path::NodePath;
pub use profile::{ScriptProfile, ScriptTiming};
pub use property::ScriptPropertyDesc;
pub use registry::{ScriptFactory, ScriptRegistry};
pub use scene_tree::SceneTree;
#[doc(hidden)]
pub use script::panic_message;
pub use script::{AsAny, NodeScript};
pub use scripting::{Groups, Timer};
pub use signal::{ConnectionId, Signal, SignalQueue, SignalTap, TappedSignal, SIGNAL_TAP_CAPACITY};
pub use systems::propagate_transforms;

/// Conversions of exported script property values (scene file TOML ↔ script values).
pub mod script_property {
    pub use crate::property::{color_hex, parse_color_hex, to_ffi, to_toml};
}

/// Class names of the built-in node types.
pub mod class {
    pub const NODE: &str = "Node";
    pub const NODE_3D: &str = "Node3D";
    pub const TIMER: &str = "Timer";
}

#[doc(hidden)]
pub mod __private {
    pub use serde_json;
    pub use tracing;
}
