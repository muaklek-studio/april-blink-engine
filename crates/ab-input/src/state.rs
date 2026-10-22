use ab_core::Vec2;
use ab_platform::{GamepadAxis, GamepadButton, GamepadId, KeyCode, MouseButton, PlatformEvent};
use std::collections::{BTreeMap, HashSet};
use std::hash::Hash;

/// Pressed/just-pressed/just-released tracking for one kind of button.
#[derive(Debug, Clone)]
pub struct ButtonState<T> {
    pressed: HashSet<T>,
    just_pressed: HashSet<T>,
    just_released: HashSet<T>,
}

impl<T: Copy + Eq + Hash> ButtonState<T> {
    pub fn press(&mut self, button: T) {
        if self.pressed.insert(button) {
            self.just_pressed.insert(button);
        }
    }

    pub fn release(&mut self, button: T) {
        if self.pressed.remove(&button) {
            self.just_released.insert(button);
        }
    }

    /// Releases everything (used when focus is lost so keys do not stick).
    pub fn release_all(&mut self) {
        self.just_released.extend(self.pressed.drain());
    }

    pub fn is_pressed(&self, button: T) -> bool {
        self.pressed.contains(&button)
    }

    pub fn is_just_pressed(&self, button: T) -> bool {
        self.just_pressed.contains(&button)
    }

    pub fn is_just_released(&self, button: T) -> bool {
        self.just_released.contains(&button)
    }

    pub fn pressed(&self) -> impl Iterator<Item = T> + '_ {
        self.pressed.iter().copied()
    }

    fn end_frame(&mut self) {
        self.just_pressed.clear();
        self.just_released.clear();
    }
}

impl<T> Default for ButtonState<T> {
    fn default() -> Self {
        Self {
            pressed: HashSet::new(),
            just_pressed: HashSet::new(),
            just_released: HashSet::new(),
        }
    }
}

/// State of one connected gamepad.
#[derive(Debug, Clone, Default)]
pub struct GamepadState {
    pub buttons: ButtonState<GamepadButton>,
    axes: [f32; GamepadAxis::ALL.len()],
}

impl GamepadState {
    /// Raw axis value (no dead zone): sticks -1..1 with +Y up, triggers 0..1.
    pub fn axis(&self, axis: GamepadAxis) -> f32 {
        self.axes[axis.index()]
    }

    pub fn is_pressed(&self, button: GamepadButton) -> bool {
        self.buttons.is_pressed(button)
    }
}

/// Keyboard, mouse and gamepad state for the current frame (an ECS resource).
///
/// Poll it directly for raw input, or use [`Actions`](crate::Actions) for named,
/// rebindable actions.
#[derive(Debug, Clone, Default)]
pub struct InputState {
    pub keys: ButtonState<KeyCode>,
    pub mouse_buttons: ButtonState<MouseButton>,
    gamepads: BTreeMap<GamepadId, GamepadState>,
    /// Most recently used gamepad (for single-player games and UI prompts).
    active_gamepad: Option<GamepadId>,
    cursor_position: Option<Vec2>,
    mouse_delta: Vec2,
    wheel_delta: Vec2,
    focused: bool,
    /// Input events received this frame, in order.
    events: Vec<PlatformEvent>,
}

impl InputState {
    pub fn new() -> Self {
        Self {
            focused: true,
            ..Default::default()
        }
    }

    /// Applies one platform event. Window events that are not input are ignored.
    pub fn handle_event(&mut self, event: &PlatformEvent) {
        let is_input = match *event {
            PlatformEvent::Key { code, pressed, .. } => {
                if pressed {
                    self.keys.press(code);
                } else {
                    self.keys.release(code);
                }
                true
            }
            PlatformEvent::MouseButton { button, pressed } => {
                if pressed {
                    self.mouse_buttons.press(button);
                } else {
                    self.mouse_buttons.release(button);
                }
                true
            }
            PlatformEvent::CursorMoved { x, y } => {
                self.cursor_position = Some(Vec2::new(x as f32, y as f32));
                true
            }
            PlatformEvent::CursorLeft => {
                self.cursor_position = None;
                false
            }
            PlatformEvent::MouseMotion { dx, dy } => {
                if !self.focused {
                    return;
                }
                self.mouse_delta += Vec2::new(dx as f32, dy as f32);
                true
            }
            PlatformEvent::MouseWheel { dx, dy } => {
                self.wheel_delta += Vec2::new(dx, dy);
                true
            }
            PlatformEvent::Focused(focused) => {
                self.focused = focused;
                if !focused {
                    self.keys.release_all();
                    self.mouse_buttons.release_all();
                }
                false
            }
            PlatformEvent::GamepadConnected { id } => {
                self.gamepads.entry(id).or_default();
                false
            }
            PlatformEvent::GamepadDisconnected { id } => {
                self.gamepads.remove(&id);
                if self.active_gamepad == Some(id) {
                    self.active_gamepad = self.gamepads.keys().next().copied();
                }
                false
            }
            PlatformEvent::GamepadButton {
                id,
                button,
                pressed,
            } => {
                let pad = self.gamepads.entry(id).or_default();
                if pressed {
                    pad.buttons.press(button);
                    self.active_gamepad = Some(id);
                } else {
                    pad.buttons.release(button);
                }
                true
            }
            PlatformEvent::GamepadAxis { id, axis, value } => {
                let pad = self.gamepads.entry(id).or_default();
                pad.axes[axis.index()] = if value.is_finite() { value } else { 0.0 };
                if value.abs() > 0.5 {
                    self.active_gamepad = Some(id);
                }
                true
            }
            PlatformEvent::Resized { .. }
            | PlatformEvent::ScaleFactorChanged { .. }
            | PlatformEvent::CloseRequested => false,
        };
        if is_input {
            self.events.push(*event);
        }
    }

    /// Clears per-frame data. Call once at the end of every frame.
    pub fn end_frame(&mut self) {
        self.keys.end_frame();
        self.mouse_buttons.end_frame();
        for pad in self.gamepads.values_mut() {
            pad.buttons.end_frame();
        }
        self.mouse_delta = Vec2::ZERO;
        self.wheel_delta = Vec2::ZERO;
        self.events.clear();
    }

    /// Input events of this frame, in arrival order (Godot's `_input` stream).
    pub fn events(&self) -> &[PlatformEvent] {
        &self.events
    }

    pub fn is_key_pressed(&self, key: KeyCode) -> bool {
        self.keys.is_pressed(key)
    }

    pub fn is_key_just_pressed(&self, key: KeyCode) -> bool {
        self.keys.is_just_pressed(key)
    }

    pub fn is_key_just_released(&self, key: KeyCode) -> bool {
        self.keys.is_just_released(key)
    }

    pub fn is_mouse_pressed(&self, button: MouseButton) -> bool {
        self.mouse_buttons.is_pressed(button)
    }

    pub fn is_mouse_just_pressed(&self, button: MouseButton) -> bool {
        self.mouse_buttons.is_just_pressed(button)
    }

    pub fn is_mouse_just_released(&self, button: MouseButton) -> bool {
        self.mouse_buttons.is_just_released(button)
    }

    /// `+1` when only `positive` is held, `-1` when only `negative` is, otherwise `0`.
    pub fn axis(&self, negative: KeyCode, positive: KeyCode) -> f32 {
        self.is_key_pressed(positive) as i32 as f32 - self.is_key_pressed(negative) as i32 as f32
    }

    /// Cursor position in physical pixels, `None` when outside the window.
    pub fn cursor_position(&self) -> Option<Vec2> {
        self.cursor_position
    }

    /// Raw mouse motion accumulated this frame (pixels-ish, unaccelerated).
    pub fn mouse_delta(&self) -> Vec2 {
        self.mouse_delta
    }

    /// Wheel lines scrolled this frame.
    pub fn wheel_delta(&self) -> Vec2 {
        self.wheel_delta
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn gamepads(&self) -> impl Iterator<Item = (GamepadId, &GamepadState)> {
        self.gamepads.iter().map(|(id, state)| (*id, state))
    }

    pub fn gamepad(&self, id: GamepadId) -> Option<&GamepadState> {
        self.gamepads.get(&id)
    }

    /// The gamepad that last had a button pressed or a stick pushed.
    pub fn active_gamepad(&self) -> Option<GamepadId> {
        self.active_gamepad
            .filter(|id| self.gamepads.contains_key(id))
            .or_else(|| self.gamepads.keys().next().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, pressed: bool) -> PlatformEvent {
        PlatformEvent::Key {
            code,
            pressed,
            repeat: false,
        }
    }

    #[test]
    fn just_pressed_lasts_one_frame() {
        let mut input = InputState::new();
        input.handle_event(&key(KeyCode::Space, true));
        assert!(input.is_key_pressed(KeyCode::Space));
        assert!(input.is_key_just_pressed(KeyCode::Space));
        input.end_frame();
        assert!(input.is_key_pressed(KeyCode::Space));
        assert!(!input.is_key_just_pressed(KeyCode::Space));
        input.handle_event(&key(KeyCode::Space, false));
        assert!(input.is_key_just_released(KeyCode::Space));
        assert!(!input.is_key_pressed(KeyCode::Space));
    }

    #[test]
    fn key_repeat_does_not_retrigger_just_pressed() {
        let mut input = InputState::new();
        input.handle_event(&key(KeyCode::KeyW, true));
        input.end_frame();
        input.handle_event(&PlatformEvent::Key {
            code: KeyCode::KeyW,
            pressed: true,
            repeat: true,
        });
        assert!(!input.is_key_just_pressed(KeyCode::KeyW));
    }

    #[test]
    fn axis_combines_two_keys() {
        let mut input = InputState::new();
        assert_eq!(input.axis(KeyCode::KeyS, KeyCode::KeyW), 0.0);
        input.handle_event(&key(KeyCode::KeyW, true));
        assert_eq!(input.axis(KeyCode::KeyS, KeyCode::KeyW), 1.0);
        input.handle_event(&key(KeyCode::KeyS, true));
        assert_eq!(input.axis(KeyCode::KeyS, KeyCode::KeyW), 0.0);
    }

    #[test]
    fn mouse_motion_accumulates_and_resets() {
        let mut input = InputState::new();
        input.handle_event(&PlatformEvent::MouseMotion { dx: 1.0, dy: 2.0 });
        input.handle_event(&PlatformEvent::MouseMotion { dx: 3.0, dy: -1.0 });
        assert_eq!(input.mouse_delta(), Vec2::new(4.0, 1.0));
        input.end_frame();
        assert_eq!(input.mouse_delta(), Vec2::ZERO);
    }

    #[test]
    fn losing_focus_releases_everything() {
        let mut input = InputState::new();
        input.handle_event(&key(KeyCode::KeyA, true));
        input.handle_event(&PlatformEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        });
        input.handle_event(&PlatformEvent::Focused(false));
        assert!(!input.is_key_pressed(KeyCode::KeyA));
        assert!(input.is_key_just_released(KeyCode::KeyA));
        assert!(!input.is_mouse_pressed(MouseButton::Left));
        input.handle_event(&PlatformEvent::MouseMotion { dx: 5.0, dy: 5.0 });
        assert_eq!(
            input.mouse_delta(),
            Vec2::ZERO,
            "motion ignored while unfocused"
        );
    }

    #[test]
    fn tracks_gamepads_and_the_active_one() {
        let mut input = InputState::new();
        let (a, b) = (GamepadId(0), GamepadId(1));
        input.handle_event(&PlatformEvent::GamepadConnected { id: a });
        input.handle_event(&PlatformEvent::GamepadConnected { id: b });
        assert_eq!(input.active_gamepad(), Some(a));

        input.handle_event(&PlatformEvent::GamepadButton {
            id: b,
            button: GamepadButton::South,
            pressed: true,
        });
        input.handle_event(&PlatformEvent::GamepadAxis {
            id: b,
            axis: GamepadAxis::LeftStickY,
            value: 0.75,
        });
        assert_eq!(input.active_gamepad(), Some(b));
        let pad = input.gamepad(b).unwrap();
        assert!(pad.buttons.is_just_pressed(GamepadButton::South));
        assert_eq!(pad.axis(GamepadAxis::LeftStickY), 0.75);

        input.end_frame();
        assert!(!input
            .gamepad(b)
            .unwrap()
            .buttons
            .is_just_pressed(GamepadButton::South));
        input.handle_event(&PlatformEvent::GamepadDisconnected { id: b });
        assert_eq!(input.active_gamepad(), Some(a));
        assert_eq!(input.gamepads().count(), 1);
    }

    #[test]
    fn records_input_events_for_the_frame() {
        let mut input = InputState::new();
        input.handle_event(&key(KeyCode::KeyE, true));
        input.handle_event(&PlatformEvent::Resized {
            width: 1,
            height: 1,
        });
        input.handle_event(&PlatformEvent::MouseWheel { dx: 0.0, dy: 1.0 });
        assert_eq!(input.events().len(), 2, "window events are not input");
        input.end_frame();
        assert!(input.events().is_empty());
    }
}
