use crate::gamepad::{GamepadAxis, GamepadButton, GamepadId};
use crate::{KeyCode, MouseButton};
use winit::event::{DeviceEvent, ElementState, MouseScrollDelta, WindowEvent};
use winit::keyboard::PhysicalKey;

/// Approximate pixels per wheel "line", used to normalise pixel-based scrolling (touchpads).
const PIXELS_PER_LINE: f64 = 40.0;

/// OS event, already reduced to what the engine consumes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlatformEvent {
    /// Client area resized, in physical pixels.
    Resized {
        width: u32,
        height: u32,
    },
    ScaleFactorChanged {
        scale_factor: f64,
    },
    Focused(bool),
    CloseRequested,
    /// Physical key (layout independent: `KeyW` is the same key on QWERTY and AZERTY).
    Key {
        code: KeyCode,
        pressed: bool,
        repeat: bool,
    },
    MouseButton {
        button: MouseButton,
        pressed: bool,
    },
    /// Cursor position in physical pixels relative to the top-left of the client area.
    CursorMoved {
        x: f64,
        y: f64,
    },
    CursorLeft,
    /// Raw, unaccelerated mouse motion. Keeps working when the cursor is locked (mouse look).
    MouseMotion {
        dx: f64,
        dy: f64,
    },
    /// Scroll in "lines" (positive y = away from the user).
    MouseWheel {
        dx: f32,
        dy: f32,
    },
    GamepadConnected {
        id: GamepadId,
    },
    GamepadDisconnected {
        id: GamepadId,
    },
    GamepadButton {
        id: GamepadId,
        button: GamepadButton,
        pressed: bool,
    },
    /// Sticks -1..1 (+Y up), triggers 0..1. No dead zone applied.
    GamepadAxis {
        id: GamepadId,
        axis: GamepadAxis,
        value: f32,
    },
}

impl PlatformEvent {
    pub(crate) fn from_window_event(event: &WindowEvent) -> Option<Self> {
        Some(match event {
            WindowEvent::Resized(size) => Self::Resized {
                width: size.width,
                height: size.height,
            },
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => Self::ScaleFactorChanged {
                scale_factor: *scale_factor,
            },
            WindowEvent::Focused(focused) => Self::Focused(*focused),
            WindowEvent::CloseRequested => Self::CloseRequested,
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return None;
                };
                Self::Key {
                    code,
                    pressed: event.state == ElementState::Pressed,
                    repeat: event.repeat,
                }
            }
            WindowEvent::MouseInput { state, button, .. } => Self::MouseButton {
                button: *button,
                pressed: *state == ElementState::Pressed,
            },
            WindowEvent::CursorMoved { position, .. } => Self::CursorMoved {
                x: position.x,
                y: position.y,
            },
            WindowEvent::CursorLeft { .. } => Self::CursorLeft,
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (*x, *y),
                    MouseScrollDelta::PixelDelta(p) => (
                        (p.x / PIXELS_PER_LINE) as f32,
                        (p.y / PIXELS_PER_LINE) as f32,
                    ),
                };
                Self::MouseWheel { dx, dy }
            }
            _ => return None,
        })
    }

    pub(crate) fn from_device_event(event: &DeviceEvent) -> Option<Self> {
        match event {
            DeviceEvent::MouseMotion { delta: (dx, dy) } => {
                Some(Self::MouseMotion { dx: *dx, dy: *dy })
            }
            _ => None,
        }
    }
}
