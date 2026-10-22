//! Seeing what the input does: a log of recent presses and a readable report of every key,
//! button, axis and action, for the engine's input overlay and the editor's Debug panel.

use crate::actions::Actions;
use crate::state::InputState;
use ab_platform::{GamepadAxis, GamepadButton, PlatformEvent};
use std::collections::VecDeque;
use std::fmt::Write as _;

/// Entries kept in an [`InputLog`].
pub const INPUT_LOG_LENGTH: usize = 32;

/// ECS resource: the latest input events and action changes, newest first, with the game
/// time they happened at.
#[derive(Debug, Clone, Default)]
pub struct InputLog {
    entries: VecDeque<(f64, String)>,
}

impl InputLog {
    /// Adds this frame's key, button and action changes.
    pub fn record(&mut self, time: f64, input: &InputState, actions: &Actions) {
        for event in input.events() {
            if let Some(text) = describe(event) {
                self.push(time, text);
            }
        }
        for event in actions.events() {
            let change = if event.pressed { "pressed" } else { "released" };
            self.push(time, format!("▶ {} {change}", event.action));
        }
    }

    pub fn push(&mut self, time: f64, text: String) {
        self.entries.push_front((time, text));
        self.entries.truncate(INPUT_LOG_LENGTH);
    }

    /// `(seconds, text)`, newest first.
    pub fn entries(&self) -> impl Iterator<Item = (f64, &str)> {
        self.entries.iter().map(|(t, s)| (*t, s.as_str()))
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// A key, button or gamepad event in words; `None` for motion and window events.
pub fn describe(event: &PlatformEvent) -> Option<String> {
    let updown = |pressed: bool| if pressed { "down" } else { "up" };
    Some(match event {
        PlatformEvent::Key {
            code,
            pressed,
            repeat: false,
        } => format!("key {code:?} {}", updown(*pressed)),
        PlatformEvent::MouseButton { button, pressed } => {
            format!("mouse {button:?} {}", updown(*pressed))
        }
        PlatformEvent::MouseWheel { dy, .. } if *dy != 0.0 => {
            format!("wheel {}", if *dy > 0.0 { "up" } else { "down" })
        }
        PlatformEvent::GamepadButton {
            id,
            button,
            pressed,
        } => format!("pad {} {button:?} {}", id.0, updown(*pressed)),
        PlatformEvent::GamepadConnected { id } => format!("pad {} connected", id.0),
        PlatformEvent::GamepadDisconnected { id } => format!("pad {} disconnected", id.0),
        _ => return None,
    })
}

/// Everything about the input right now, as lines of text.
pub fn report(input: &InputState, actions: &Actions, log: Option<&InputLog>) -> String {
    let mut out = String::new();
    let keys: Vec<String> = input.keys.pressed().map(|k| format!("{k:?}")).collect();
    let _ = writeln!(
        out,
        "keys     {}",
        if keys.is_empty() {
            "—".into()
        } else {
            keys.join(" ")
        }
    );
    let buttons: Vec<String> = input
        .mouse_buttons
        .pressed()
        .map(|b| format!("{b:?}"))
        .collect();
    let cursor = input
        .cursor_position()
        .map_or("outside".into(), |p| format!("{:.0}, {:.0}", p.x, p.y));
    let delta = input.mouse_delta();
    let _ = writeln!(
        out,
        "mouse    {cursor}  Δ {:+.1} {:+.1}  {}{}",
        delta.x,
        delta.y,
        buttons.join(" "),
        if input.is_focused() {
            ""
        } else {
            "  (window not focused)"
        }
    );
    let mut any_pad = false;
    for (id, pad) in input.gamepads() {
        any_pad = true;
        let axes: Vec<String> = GamepadAxis::ALL
            .iter()
            .map(|&a| format!("{} {:+.2}", axis_name(a), pad.axis(a)))
            .collect();
        let pressed: Vec<String> = GamepadButton::ALL
            .iter()
            .filter(|&&b| pad.is_pressed(b))
            .map(|b| format!("{b:?}"))
            .collect();
        let active = if input.active_gamepad() == Some(id) {
            " (active)"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "pad {}{active}  {}  {}",
            id.0,
            axes.join(" "),
            pressed.join(" ")
        );
    }
    if !any_pad {
        let _ = writeln!(out, "pad      none connected");
    }
    let look = actions.look();
    let _ = writeln!(out, "look     {:+.3} {:+.3}", look.x, look.y);
    let _ = writeln!(out, "actions");
    for name in actions.names() {
        let strength = actions.strength(name);
        let mark = if actions.is_pressed(name) {
            "●"
        } else {
            "○"
        };
        let held = if actions.is_held(name) { " (held)" } else { "" };
        let bindings: Vec<String> = actions
            .map()
            .actions
            .get(name)
            .map(|a| a.bindings.iter().map(ToString::to_string).collect())
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "  {mark} {name:<16} {strength:.2}{held}  [{}]",
            bindings.join(", ")
        );
    }
    if let Some(log) = log {
        let _ = writeln!(out, "recent");
        for (time, text) in log.entries().take(8) {
            let _ = writeln!(out, "  {time:7.2}s  {text}");
        }
    }
    out
}

fn axis_name(axis: GamepadAxis) -> &'static str {
    match axis {
        GamepadAxis::LeftStickX => "LX",
        GamepadAxis::LeftStickY => "LY",
        GamepadAxis::RightStickX => "RX",
        GamepadAxis::RightStickY => "RY",
        GamepadAxis::LeftTrigger => "LT",
        GamepadAxis::RightTrigger => "RT",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Binding, InputMap};
    use ab_platform::{KeyCode, MouseButton};

    #[test]
    fn logs_presses_and_actions_and_reports_them() {
        let mut map = InputMap::new();
        map.bind("jump", Binding::Key(KeyCode::Space));
        map.bind("fire", Binding::Mouse(MouseButton::Left));
        let mut actions = Actions::new(map);
        let mut input = InputState::new();
        let mut log = InputLog::default();
        input.handle_event(&PlatformEvent::Key {
            code: KeyCode::Space,
            pressed: true,
            repeat: false,
        });
        actions.update(&input);
        log.record(1.5, &input, &actions);
        let entries: Vec<&str> = log.entries().map(|(_, t)| t).collect();
        assert_eq!(entries, ["▶ jump pressed", "key Space down"]);

        let text = report(&input, &actions, Some(&log));
        assert!(text.contains("keys     Space"), "{text}");
        assert!(text.contains("● jump"), "{text}");
        assert!(text.contains("○ fire"), "{text}");
        assert!(text.contains("[key:Space]"), "{text}");
        assert!(text.contains("1.50s  ▶ jump pressed"), "{text}");
        assert!(text.contains("pad      none connected"), "{text}");
    }

    #[test]
    fn the_log_keeps_the_newest_entries() {
        let mut log = InputLog::default();
        for i in 0..100 {
            log.push(f64::from(i), format!("event {i}"));
        }
        assert_eq!(log.entries().count(), INPUT_LOG_LENGTH);
        assert_eq!(log.entries().next(), Some((99.0, "event 99")));
    }
}
