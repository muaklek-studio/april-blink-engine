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

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::{PhysicalPosition, PhysicalSize};
    use winit::event::{DeviceId, TouchPhase};

    fn window(event: WindowEvent) -> Option<PlatformEvent> {
        PlatformEvent::from_window_event(&event)
    }

    fn wheel(delta: MouseScrollDelta) -> Option<PlatformEvent> {
        window(WindowEvent::MouseWheel {
            device_id: DeviceId::dummy(),
            delta,
            phase: TouchPhase::Moved,
        })
    }

    #[test]
    fn window_state_events() {
        assert_eq!(
            window(WindowEvent::Resized(PhysicalSize::new(800, 600))),
            Some(PlatformEvent::Resized {
                width: 800,
                height: 600
            })
        );
        assert_eq!(
            window(WindowEvent::Focused(false)),
            Some(PlatformEvent::Focused(false))
        );
        assert_eq!(
            window(WindowEvent::CloseRequested),
            Some(PlatformEvent::CloseRequested)
        );
    }

    #[test]
    fn mouse_buttons_and_cursor() {
        assert_eq!(
            window(WindowEvent::MouseInput {
                device_id: DeviceId::dummy(),
                state: ElementState::Pressed,
                button: MouseButton::Right,
            }),
            Some(PlatformEvent::MouseButton {
                button: MouseButton::Right,
                pressed: true
            })
        );
        assert_eq!(
            window(WindowEvent::MouseInput {
                device_id: DeviceId::dummy(),
                state: ElementState::Released,
                button: MouseButton::Left,
            }),
            Some(PlatformEvent::MouseButton {
                button: MouseButton::Left,
                pressed: false
            })
        );
        assert_eq!(
            window(WindowEvent::CursorMoved {
                device_id: DeviceId::dummy(),
                position: PhysicalPosition::new(12.5, 40.0),
            }),
            Some(PlatformEvent::CursorMoved { x: 12.5, y: 40.0 })
        );
        assert_eq!(
            window(WindowEvent::CursorLeft {
                device_id: DeviceId::dummy()
            }),
            Some(PlatformEvent::CursorLeft)
        );
    }

    #[test]
    fn wheel_lines_pass_through() {
        assert_eq!(
            wheel(MouseScrollDelta::LineDelta(0.0, -2.0)),
            Some(PlatformEvent::MouseWheel { dx: 0.0, dy: -2.0 })
        );
    }

    #[test]
    fn wheel_pixels_become_lines() {
        assert_eq!(
            wheel(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
                20.0, 120.0
            ))),
            Some(PlatformEvent::MouseWheel { dx: 0.5, dy: 3.0 })
        );
    }

    #[test]
    fn unused_window_events_are_dropped() {
        assert_eq!(window(WindowEvent::Occluded(true)), None);
        assert_eq!(window(WindowEvent::RedrawRequested), None);
        assert_eq!(
            window(WindowEvent::CursorEntered {
                device_id: DeviceId::dummy()
            }),
            None
        );
    }

    #[test]
    fn raw_mouse_motion() {
        assert_eq!(
            PlatformEvent::from_device_event(&DeviceEvent::MouseMotion { delta: (3.0, -1.5) }),
            Some(PlatformEvent::MouseMotion { dx: 3.0, dy: -1.5 })
        );
        assert_eq!(
            PlatformEvent::from_device_event(&DeviceEvent::MouseWheel {
                delta: MouseScrollDelta::LineDelta(0.0, 1.0)
            }),
            None
        );
    }
}
