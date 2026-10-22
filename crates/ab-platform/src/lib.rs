//! Platform layer: creates the window, translates OS events into [`PlatformEvent`]s and
//! drives the frame loop with optional frame pacing. Gamepads (feature `gamepad`, via gilrs)
//! are polled by the loop and delivered as events too.
//!
//! The rest of the engine never touches `winit` directly except through the re-exported
//! [`KeyCode`], [`MouseButton`] and [`Window`] types.

mod cursor;
mod event;
mod gamepad;
mod runner;

pub use cursor::{set_cursor_captured, set_fullscreen};
pub use event::PlatformEvent;
pub use gamepad::{GamepadAxis, GamepadButton, GamepadId, Gamepads};
pub use runner::{run, LoopControl, PlatformApp, WindowConfig};
pub use winit::event::MouseButton;
pub use winit::keyboard::KeyCode;
pub use winit::window::Window;
