use crate::state::InputState;
use ab_platform::{GamepadAxis, GamepadButton, KeyCode, MouseButton};
use serde::de::{value::Error as ValueError, IntoDeserializer};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// A physical input that can trigger an action.
///
/// Text form (used in input-map TOML files):
/// `key:KeyW`, `mouse:Left`, `wheel:Up`, `pad:South`, `pad_axis:LeftStickY+`.
/// Key names are winit's physical [`KeyCode`] names (layout independent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Binding {
    Key(KeyCode),
    Mouse(MouseButton),
    /// One wheel notch this frame in the given direction.
    Wheel {
        up: bool,
    },
    GamepadButton(GamepadButton),
    /// One direction of an analog axis (+Y is up on sticks).
    GamepadAxis {
        axis: GamepadAxis,
        positive: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid input binding `{0}` (expected e.g. key:KeyW, mouse:Left, wheel:Up, pad:South, pad_axis:LeftStickY+)")]
pub struct BindingParseError(pub String);

/// Maps a stick/trigger value through a dead zone onto 0..1.
pub(crate) fn apply_deadzone(value: f32, deadzone: f32) -> f32 {
    let deadzone = deadzone.clamp(0.0, 0.99);
    if value <= deadzone {
        0.0
    } else {
        ((value - deadzone) / (1.0 - deadzone)).min(1.0)
    }
}

impl Binding {
    /// Current strength 0..1. Gamepad bindings take the strongest value across all pads.
    pub fn strength(&self, input: &InputState, deadzone: f32) -> f32 {
        match *self {
            Binding::Key(key) => input.is_key_pressed(key) as u8 as f32,
            Binding::Mouse(button) => input.is_mouse_pressed(button) as u8 as f32,
            Binding::Wheel { up } => {
                let dy = input.wheel_delta().y;
                ((up && dy > 0.0) || (!up && dy < 0.0)) as u8 as f32
            }
            Binding::GamepadButton(button) => {
                input.gamepads().any(|(_, pad)| pad.is_pressed(button)) as u8 as f32
            }
            Binding::GamepadAxis { axis, positive } => input
                .gamepads()
                .map(|(_, pad)| {
                    let value = pad.axis(axis);
                    apply_deadzone(if positive { value } else { -value }, deadzone)
                })
                .fold(0.0, f32::max),
        }
    }
}

fn parse_named<'a, T: Deserialize<'a>>(name: &'a str) -> Option<T> {
    let deserializer: serde::de::value::StrDeserializer<'a, ValueError> = name.into_deserializer();
    T::deserialize(deserializer).ok()
}

fn find_named<T: fmt::Debug + Copy>(all: &[T], name: &str) -> Option<T> {
    all.iter().copied().find(|item| format!("{item:?}") == name)
}

impl FromStr for Binding {
    type Err = BindingParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let err = || BindingParseError(text.to_owned());
        let (kind, name) = text.trim().split_once(':').ok_or_else(err)?;
        let name = name.trim();
        let binding = match kind.trim() {
            "key" => Binding::Key(parse_named(name).ok_or_else(err)?),
            "mouse" => Binding::Mouse(parse_named(name).ok_or_else(err)?),
            "wheel" => match name {
                "Up" => Binding::Wheel { up: true },
                "Down" => Binding::Wheel { up: false },
                _ => return Err(err()),
            },
            "pad" => Binding::GamepadButton(find_named(&GamepadButton::ALL, name).ok_or_else(err)?),
            "pad_axis" => {
                let (axis, positive) = if let Some(axis) = name.strip_suffix('+') {
                    (axis, true)
                } else if let Some(axis) = name.strip_suffix('-') {
                    (axis, false)
                } else {
                    return Err(err());
                };
                Binding::GamepadAxis {
                    axis: find_named(&GamepadAxis::ALL, axis).ok_or_else(err)?,
                    positive,
                }
            }
            _ => return Err(err()),
        };
        Ok(binding)
    }
}

impl fmt::Display for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Binding::Key(key) => write!(f, "key:{key:?}"),
            Binding::Mouse(button) => write!(f, "mouse:{button:?}"),
            Binding::Wheel { up } => write!(f, "wheel:{}", if *up { "Up" } else { "Down" }),
            Binding::GamepadButton(button) => write!(f, "pad:{button:?}"),
            Binding::GamepadAxis { axis, positive } => {
                write!(f, "pad_axis:{axis:?}{}", if *positive { '+' } else { '-' })
            }
        }
    }
}

impl Serialize for Binding {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Binding {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_platform::{GamepadId, PlatformEvent};

    #[test]
    fn text_round_trip() {
        for text in [
            "key:KeyW",
            "key:Space",
            "key:ArrowUp",
            "mouse:Left",
            "mouse:Back",
            "wheel:Down",
            "pad:South",
            "pad:DPadLeft",
            "pad_axis:LeftStickY+",
            "pad_axis:RightTrigger+",
            "pad_axis:RightStickX-",
        ] {
            let binding: Binding = text.parse().unwrap();
            assert_eq!(binding.to_string(), text);
        }
    }

    #[test]
    fn rejects_unknown_names() {
        for text in [
            "KeyW",
            "key:NotAKey",
            "pad_axis:LeftStickY",
            "joystick:A",
            "pad:Nope",
        ] {
            assert!(text.parse::<Binding>().is_err(), "{text}");
        }
    }

    #[test]
    fn deadzone_rescales() {
        assert_eq!(apply_deadzone(0.1, 0.2), 0.0);
        assert!((apply_deadzone(0.6, 0.2) - 0.5).abs() < 1e-6);
        assert_eq!(apply_deadzone(1.0, 0.2), 1.0);
        assert_eq!(apply_deadzone(-1.0, 0.2), 0.0);
    }

    #[test]
    fn axis_strength_uses_strongest_pad_and_direction() {
        let mut input = InputState::new();
        for (id, value) in [(0, -0.6), (1, 0.9)] {
            input.handle_event(&PlatformEvent::GamepadAxis {
                id: GamepadId(id),
                axis: GamepadAxis::LeftStickX,
                value,
            });
        }
        let right = Binding::GamepadAxis {
            axis: GamepadAxis::LeftStickX,
            positive: true,
        };
        let left = Binding::GamepadAxis {
            axis: GamepadAxis::LeftStickX,
            positive: false,
        };
        assert!((right.strength(&input, 0.0) - 0.9).abs() < 1e-6);
        assert!((left.strength(&input, 0.0) - 0.6).abs() < 1e-6);
        assert_eq!(left.strength(&input, 0.7), 0.0);
    }
}
