//! Gamepad input. The types are always available; the gilrs backend only with the `gamepad`
//! feature (enabled by default).

use crate::event::PlatformEvent;
use std::fmt;

/// Identifies a connected gamepad for as long as it stays connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GamepadId(pub usize);

impl fmt::Display for GamepadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "gamepad{}", self.0)
    }
}

/// Buttons by position, Xbox-style naming where it is unambiguous (South = A / Cross).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GamepadButton {
    South,
    East,
    North,
    West,
    LeftBumper,
    RightBumper,
    /// Digital state of the analog trigger (see [`GamepadAxis::LeftTrigger`]).
    LeftTrigger,
    RightTrigger,
    Select,
    Start,
    Mode,
    LeftStick,
    RightStick,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

impl GamepadButton {
    pub const ALL: [GamepadButton; 17] = [
        Self::South,
        Self::East,
        Self::North,
        Self::West,
        Self::LeftBumper,
        Self::RightBumper,
        Self::LeftTrigger,
        Self::RightTrigger,
        Self::Select,
        Self::Start,
        Self::Mode,
        Self::LeftStick,
        Self::RightStick,
        Self::DPadUp,
        Self::DPadDown,
        Self::DPadLeft,
        Self::DPadRight,
    ];
}

/// Analog axes. Sticks report -1..1 with **+Y up** and +X right; triggers report 0..1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GamepadAxis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
}

impl GamepadAxis {
    pub const ALL: [GamepadAxis; 6] = [
        Self::LeftStickX,
        Self::LeftStickY,
        Self::RightStickX,
        Self::RightStickY,
        Self::LeftTrigger,
        Self::RightTrigger,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }
}

/// Polls connected gamepads and turns their state changes into [`PlatformEvent`]s.
pub struct Gamepads {
    #[cfg(feature = "gamepad")]
    gilrs: gilrs::Gilrs,
}

impl Gamepads {
    /// Opens the gamepad backend. Returns `None` (after logging why) when gamepads are
    /// unavailable: feature disabled, or no OS support/permissions.
    pub fn new() -> Option<Self> {
        #[cfg(feature = "gamepad")]
        {
            match gilrs::Gilrs::new() {
                Ok(gilrs) => Some(Self { gilrs }),
                Err(err) => {
                    tracing::warn!("gamepads unavailable: {err}");
                    None
                }
            }
        }
        #[cfg(not(feature = "gamepad"))]
        {
            tracing::debug!("built without the `gamepad` feature");
            None
        }
    }

    /// Reports gamepads that were connected before the backend started.
    pub fn connected(&self, mut emit: impl FnMut(PlatformEvent)) {
        #[cfg(feature = "gamepad")]
        for (id, pad) in self.gilrs.gamepads() {
            tracing::info!(id = usize::from(id), name = pad.name(), "gamepad connected");
            emit(PlatformEvent::GamepadConnected {
                id: GamepadId(id.into()),
            });
        }
        #[cfg(not(feature = "gamepad"))]
        let _ = &mut emit;
    }

    /// Drains pending backend events.
    pub fn poll(&mut self, mut emit: impl FnMut(PlatformEvent)) {
        #[cfg(feature = "gamepad")]
        while let Some(gilrs::Event { id, event, .. }) = self.gilrs.next_event() {
            if let Some(event) = backend::translate(&self.gilrs, id, event) {
                emit(event);
            }
        }
        #[cfg(not(feature = "gamepad"))]
        let _ = &mut emit;
    }
}

impl fmt::Debug for Gamepads {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gamepads").finish_non_exhaustive()
    }
}

#[cfg(feature = "gamepad")]
mod backend {
    use super::{GamepadAxis, GamepadButton, GamepadId};
    use crate::event::PlatformEvent;
    use gilrs::{Axis, Button, EventType};

    fn button(button: Button) -> Option<GamepadButton> {
        Some(match button {
            Button::South => GamepadButton::South,
            Button::East => GamepadButton::East,
            Button::North => GamepadButton::North,
            Button::West => GamepadButton::West,
            Button::LeftTrigger => GamepadButton::LeftBumper,
            Button::RightTrigger => GamepadButton::RightBumper,
            Button::LeftTrigger2 => GamepadButton::LeftTrigger,
            Button::RightTrigger2 => GamepadButton::RightTrigger,
            Button::Select => GamepadButton::Select,
            Button::Start => GamepadButton::Start,
            Button::Mode => GamepadButton::Mode,
            Button::LeftThumb => GamepadButton::LeftStick,
            Button::RightThumb => GamepadButton::RightStick,
            Button::DPadUp => GamepadButton::DPadUp,
            Button::DPadDown => GamepadButton::DPadDown,
            Button::DPadLeft => GamepadButton::DPadLeft,
            Button::DPadRight => GamepadButton::DPadRight,
            Button::C | Button::Z | Button::Unknown => return None,
        })
    }

    fn axis(axis: Axis) -> Option<GamepadAxis> {
        Some(match axis {
            Axis::LeftStickX => GamepadAxis::LeftStickX,
            Axis::LeftStickY => GamepadAxis::LeftStickY,
            Axis::RightStickX => GamepadAxis::RightStickX,
            Axis::RightStickY => GamepadAxis::RightStickY,
            _ => return None,
        })
    }

    pub(super) fn translate(
        gilrs: &gilrs::Gilrs,
        id: gilrs::GamepadId,
        event: EventType,
    ) -> Option<PlatformEvent> {
        let id_value = GamepadId(id.into());
        Some(match event {
            EventType::Connected => {
                tracing::info!(
                    id = id_value.0,
                    name = gilrs.gamepad(id).name(),
                    "gamepad connected"
                );
                PlatformEvent::GamepadConnected { id: id_value }
            }
            EventType::Disconnected => {
                tracing::info!(id = id_value.0, "gamepad disconnected");
                PlatformEvent::GamepadDisconnected { id: id_value }
            }
            EventType::ButtonPressed(b, _) => PlatformEvent::GamepadButton {
                id: id_value,
                button: button(b)?,
                pressed: true,
            },
            EventType::ButtonReleased(b, _) => PlatformEvent::GamepadButton {
                id: id_value,
                button: button(b)?,
                pressed: false,
            },
            // Analog triggers arrive as button value changes.
            EventType::ButtonChanged(Button::LeftTrigger2, value, _) => {
                PlatformEvent::GamepadAxis {
                    id: id_value,
                    axis: GamepadAxis::LeftTrigger,
                    value,
                }
            }
            EventType::ButtonChanged(Button::RightTrigger2, value, _) => {
                PlatformEvent::GamepadAxis {
                    id: id_value,
                    axis: GamepadAxis::RightTrigger,
                    value,
                }
            }
            EventType::AxisChanged(a, value, _) => PlatformEvent::GamepadAxis {
                id: id_value,
                axis: axis(a)?,
                value,
            },
            _ => return None,
        })
    }
}
