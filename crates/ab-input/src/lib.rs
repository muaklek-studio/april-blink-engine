//! Input: raw device state and a Godot-style action map.
//!
//! * [`InputState`] (ECS resource) tracks keyboard, mouse and gamepads from
//!   [`PlatformEvent`](ab_platform::PlatformEvent)s, plus this frame's input events.
//! * [`InputMap`] names actions (`"jump"`, `"move_forward"`) and binds them to keys, mouse
//!   buttons, wheel, gamepad buttons or gamepad axis directions; it loads from TOML.
//! * [`Actions`] (ECS resource) evaluates the map every frame: `is_pressed`,
//!   `is_just_pressed`, `strength`, `axis`, `vector` and sensitivity-scaled mouse `look`.

mod actions;
mod binding;
mod state;

pub use ab_platform::{GamepadAxis, GamepadButton, GamepadId, KeyCode, MouseButton};
pub use actions::{Action, ActionEvent, Actions, InputMap, InputMapError};
pub use binding::{Binding, BindingParseError};
pub use state::{ButtonState, GamepadState, InputState};
