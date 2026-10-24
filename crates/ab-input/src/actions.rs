use crate::binding::Binding;
use crate::state::InputState;
use ab_core::Vec2;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const fn default_deadzone() -> f32 {
    0.2
}

const fn default_sensitivity() -> f32 {
    1.0
}

/// One named action and the inputs that trigger it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    #[serde(default)]
    pub bindings: Vec<Binding>,
    /// Analog values at or below this are ignored; the rest is rescaled to 0..1.
    #[serde(default = "default_deadzone")]
    pub deadzone: f32,
}

impl Default for Action {
    fn default() -> Self {
        Self {
            bindings: Vec::new(),
            deadzone: default_deadzone(),
        }
    }
}

/// Named actions → bindings, plus mouse-look settings. Serialisable to TOML:
///
/// ```toml
/// mouse_sensitivity = 0.0025
/// invert_y = false
///
/// [actions.jump]
/// bindings = ["key:Space", "pad:South"]
///
/// [actions.move_forward]
/// bindings = ["key:KeyW", "pad_axis:LeftStickY+"]
/// deadzone = 0.25
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputMap {
    /// Multiplier from raw mouse motion to [`Actions::look`].
    #[serde(default = "default_sensitivity")]
    pub mouse_sensitivity: f32,
    /// Flip the vertical look axis.
    #[serde(default)]
    pub invert_y: bool,
    #[serde(default)]
    pub actions: BTreeMap<String, Action>,
}

#[derive(Debug, thiserror::Error)]
pub enum InputMapError {
    #[error("invalid input map: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("cannot serialise input map: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl InputMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `binding` to `action`, creating the action if needed. Duplicate bindings are
    /// ignored.
    pub fn bind(&mut self, action: &str, binding: Binding) -> &mut Self {
        let action = self.actions.entry(action.to_owned()).or_default();
        if !action.bindings.contains(&binding) {
            action.bindings.push(binding);
        }
        self
    }

    /// [`InputMap::bind`] for several bindings.
    pub fn bind_all(
        &mut self,
        action: &str,
        bindings: impl IntoIterator<Item = Binding>,
    ) -> &mut Self {
        for binding in bindings {
            self.bind(action, binding);
        }
        self
    }

    /// Removes one binding from an action (rebinding UIs).
    pub fn unbind(&mut self, action: &str, binding: Binding) {
        if let Some(action) = self.actions.get_mut(action) {
            action.bindings.retain(|b| *b != binding);
        }
    }

    pub fn has_action(&self, action: &str) -> bool {
        self.actions.contains_key(action)
    }

    pub fn from_toml(text: &str) -> Result<Self, InputMapError> {
        Ok(toml::from_str(text)?)
    }

    pub fn to_toml(&self) -> Result<String, InputMapError> {
        Ok(toml::to_string_pretty(self)?)
    }
}

impl Default for InputMap {
    fn default() -> Self {
        Self {
            mouse_sensitivity: default_sensitivity(),
            invert_y: false,
            actions: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ActionValue {
    strength: f32,
    pressed: bool,
    was_pressed: bool,
    /// Released and pressed again between two updates.
    repressed: bool,
}

/// An action changing state this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionEvent {
    pub action: String,
    pub pressed: bool,
}

/// Evaluated actions for the current frame (an ECS resource), Godot's `Input` singleton.
///
/// The engine calls [`Actions::update`] once per frame before scripts run. Querying an
/// unknown action returns "not pressed" / `0.0`.
///
/// `just_pressed` / `just_released` last exactly one frame. Like in Godot, a fixed step does
/// not run every frame, so check them from `process`, not `physics_process`.
#[derive(Debug, Clone, Default)]
pub struct Actions {
    map: InputMap,
    values: BTreeMap<String, ActionValue>,
    look: Vec2,
    events: Vec<ActionEvent>,
    /// Actions held down from code (debug tools, tests, tutorials), with their strength.
    held: BTreeMap<String, f32>,
}

impl Actions {
    pub fn new(map: InputMap) -> Self {
        Self {
            map,
            ..Default::default()
        }
    }

    pub fn map(&self) -> &InputMap {
        &self.map
    }

    /// Replaces the bindings (e.g. after the player rebinds keys). Held actions are
    /// re-evaluated on the next update.
    pub fn set_map(&mut self, map: InputMap) {
        self.map = map;
        self.values
            .retain(|name, _| self.map.actions.contains_key(name));
    }

    /// Mutable access to the bindings.
    pub fn map_mut(&mut self) -> &mut InputMap {
        &mut self.map
    }

    /// Re-evaluates every action from the raw input state.
    pub fn update(&mut self, input: &InputState) {
        self.events.clear();
        for (name, action) in &self.map.actions {
            let strength = action
                .bindings
                .iter()
                .map(|b| b.strength(input, action.deadzone))
                .fold(0.0, f32::max)
                .max(self.held.get(name).copied().unwrap_or(0.0));
            // A new press of a button held across the update, as long as nothing else keeps
            // the action down (another binding, or a hold from code).
            let held = self.held.get(name).is_some_and(|&s| s > 0.0);
            let repressed = !held
                && action.bindings.iter().any(|b| b.repressed(input))
                && action
                    .bindings
                    .iter()
                    .all(|b| b.repressed(input) || b.strength(input, action.deadzone) <= 0.0);
            let value = self.values.entry(name.clone()).or_default();
            value.was_pressed = value.pressed;
            value.strength = strength;
            value.pressed = strength > 0.0;
            value.repressed = repressed && value.was_pressed && value.pressed;
            if value.repressed {
                self.events.push(ActionEvent {
                    action: name.clone(),
                    pressed: true,
                });
            } else if value.pressed != value.was_pressed {
                self.events.push(ActionEvent {
                    action: name.clone(),
                    pressed: value.pressed,
                });
            }
        }
        let mut look = input.mouse_delta() * self.map.mouse_sensitivity;
        if self.map.invert_y {
            look.y = -look.y;
        }
        self.look = look;
    }

    fn value(&self, action: &str) -> ActionValue {
        self.values.get(action).copied().unwrap_or_default()
    }

    pub fn is_pressed(&self, action: &str) -> bool {
        self.value(action).pressed
    }

    /// Pressed this frame. Also `true` when every input holding the action was released
    /// and pressed again between two updates (no `just_released` is reported for that).
    pub fn is_just_pressed(&self, action: &str) -> bool {
        let v = self.value(action);
        v.pressed && (!v.was_pressed || v.repressed)
    }

    pub fn is_just_released(&self, action: &str) -> bool {
        let v = self.value(action);
        !v.pressed && v.was_pressed
    }

    /// 0..1: 1 for digital inputs, the dead-zoned value for analog ones.
    pub fn strength(&self, action: &str) -> f32 {
        self.value(action).strength
    }

    /// `strength(positive) - strength(negative)`, in -1..1.
    pub fn axis(&self, negative: &str, positive: &str) -> f32 {
        self.strength(positive) - self.strength(negative)
    }

    /// Two axes as a vector, clamped to length 1 so diagonals are not faster.
    pub fn vector(
        &self,
        negative_x: &str,
        positive_x: &str,
        negative_y: &str,
        positive_y: &str,
    ) -> Vec2 {
        Vec2::new(
            self.axis(negative_x, positive_x),
            self.axis(negative_y, positive_y),
        )
        .clamp_length_max(1.0)
    }

    /// Mouse motion this frame scaled by `mouse_sensitivity` (+X right, +Y down unless
    /// `invert_y`).
    pub fn look(&self) -> Vec2 {
        self.look
    }

    /// Actions that changed state this frame.
    pub fn events(&self) -> &[ActionEvent] {
        &self.events
    }

    /// The names of the actions, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.map.actions.keys().map(String::as_str)
    }

    /// Holds `action` down at `strength` (0..1) as if its input were, from the next update
    /// until [`Actions::release`]: for debug tools, tests and tutorials.
    pub fn hold(&mut self, action: &str, strength: f32) {
        self.held
            .insert(action.to_owned(), strength.clamp(0.0, 1.0));
    }

    /// Stops holding `action` down.
    pub fn release(&mut self, action: &str) {
        self.held.remove(action);
    }

    /// Whether `action` is held down from code.
    pub fn is_held(&self, action: &str) -> bool {
        self.held.contains_key(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_platform::{GamepadAxis, GamepadButton, GamepadId, KeyCode, PlatformEvent};

    fn map() -> InputMap {
        let mut map = InputMap::new();
        map.bind_all(
            "move_forward",
            [
                Binding::Key(KeyCode::KeyW),
                Binding::GamepadAxis {
                    axis: GamepadAxis::LeftStickY,
                    positive: true,
                },
            ],
        )
        .bind("move_back", Binding::Key(KeyCode::KeyS))
        .bind("move_left", Binding::Key(KeyCode::KeyA))
        .bind("move_right", Binding::Key(KeyCode::KeyD))
        .bind_all(
            "jump",
            [
                Binding::Key(KeyCode::Space),
                Binding::GamepadButton(GamepadButton::South),
            ],
        );
        map.mouse_sensitivity = 0.5;
        map
    }

    fn key(input: &mut InputState, code: KeyCode, pressed: bool) {
        input.handle_event(&PlatformEvent::Key {
            code,
            pressed,
            repeat: false,
        });
    }

    #[test]
    fn held_actions_press_like_their_inputs() {
        let mut actions = Actions::new(map());
        let input = InputState::new();
        actions.hold("jump", 1.0);
        actions.update(&input);
        assert!(actions.is_just_pressed("jump") && actions.is_held("jump"));
        actions.hold("move_forward", 0.5);
        actions.update(&input);
        assert_eq!(actions.strength("move_forward"), 0.5);
        actions.release("jump");
        actions.update(&input);
        assert!(actions.is_just_released("jump"));
    }

    #[test]
    fn digital_actions_have_pressed_edges() {
        let mut input = InputState::new();
        let mut actions = Actions::new(map());
        key(&mut input, KeyCode::Space, true);
        actions.update(&input);
        assert!(actions.is_pressed("jump") && actions.is_just_pressed("jump"));
        assert_eq!(
            actions.events(),
            [ActionEvent {
                action: "jump".into(),
                pressed: true
            }]
        );

        input.end_frame();
        actions.update(&input);
        assert!(actions.is_pressed("jump") && !actions.is_just_pressed("jump"));
        assert!(actions.events().is_empty());

        key(&mut input, KeyCode::Space, false);
        actions.update(&input);
        assert!(actions.is_just_released("jump"));
        assert!(!actions.is_pressed("unknown action"));
    }

    #[test]
    fn movement_vector_combines_keys_and_stick() {
        let mut input = InputState::new();
        let mut actions = Actions::new(map());
        key(&mut input, KeyCode::KeyW, true);
        key(&mut input, KeyCode::KeyD, true);
        actions.update(&input);
        let v = actions.vector("move_left", "move_right", "move_back", "move_forward");
        assert!(
            (v.length() - 1.0).abs() < 1e-6,
            "diagonal is normalised: {v}"
        );
        assert!(v.x > 0.0 && v.y > 0.0);

        let mut input = InputState::new();
        input.handle_event(&PlatformEvent::GamepadAxis {
            id: GamepadId(0),
            axis: GamepadAxis::LeftStickY,
            value: 0.6,
        });
        actions.update(&input);
        // Dead zone 0.2: (0.6 - 0.2) / 0.8 = 0.5
        assert!((actions.strength("move_forward") - 0.5).abs() < 1e-6);
        input.handle_event(&PlatformEvent::GamepadAxis {
            id: GamepadId(0),
            axis: GamepadAxis::LeftStickY,
            value: 0.1,
        });
        actions.update(&input);
        assert!(!actions.is_pressed("move_forward"), "inside the dead zone");
    }

    #[test]
    fn gamepad_button_triggers_action() {
        let mut input = InputState::new();
        let mut actions = Actions::new(map());
        input.handle_event(&PlatformEvent::GamepadButton {
            id: GamepadId(3),
            button: GamepadButton::South,
            pressed: true,
        });
        actions.update(&input);
        assert!(actions.is_just_pressed("jump"));
    }

    #[test]
    fn look_applies_sensitivity_and_inversion() {
        let mut input = InputState::new();
        input.handle_event(&PlatformEvent::MouseMotion { dx: 10.0, dy: 4.0 });
        let mut actions = Actions::new(map());
        actions.update(&input);
        assert_eq!(actions.look(), Vec2::new(5.0, 2.0));
        actions.map_mut().invert_y = true;
        actions.update(&input);
        assert_eq!(actions.look(), Vec2::new(5.0, -2.0));
    }

    #[test]
    fn toml_round_trip_and_defaults() {
        let original = map();
        let text = original.to_toml().unwrap();
        assert!(text.contains("\"key:KeyW\""), "{text}");
        assert_eq!(InputMap::from_toml(&text).unwrap(), original);

        let parsed = InputMap::from_toml(
            r#"
            [actions.fire]
            bindings = ["mouse:Left", "pad_axis:RightTrigger+"]
            "#,
        )
        .unwrap();
        assert_eq!(parsed.mouse_sensitivity, 1.0);
        assert_eq!(parsed.actions["fire"].deadzone, 0.2);
        assert_eq!(parsed.actions["fire"].bindings.len(), 2);

        let err = InputMap::from_toml("[actions.x]\nbindings = [\"key:Nope\"]").unwrap_err();
        assert!(err.to_string().contains("key:Nope"), "{err}");
    }

    #[test]
    fn tap_within_one_frame_is_not_lost() {
        let mut input = InputState::new();
        let mut actions = Actions::new(map());
        // Pressed and released between two updates (touchpad click, low frame rate).
        key(&mut input, KeyCode::Space, true);
        key(&mut input, KeyCode::Space, false);
        input.handle_event(&PlatformEvent::GamepadButton {
            id: GamepadId(0),
            button: GamepadButton::South,
            pressed: true,
        });
        input.handle_event(&PlatformEvent::GamepadButton {
            id: GamepadId(0),
            button: GamepadButton::South,
            pressed: false,
        });
        actions.update(&input);
        assert!(
            actions.is_just_pressed("jump"),
            "the tap presses the action"
        );
        input.end_frame();
        actions.update(&input);
        assert!(
            actions.is_just_released("jump"),
            "and releases it next frame"
        );
        input.end_frame();
        actions.update(&input);
        assert!(!actions.is_pressed("jump") && !actions.is_just_released("jump"));
    }

    #[test]
    fn release_and_press_within_one_frame_presses_again() {
        let mut input = InputState::new();
        let mut actions = Actions::new(map());
        key(&mut input, KeyCode::Space, true);
        actions.update(&input);
        input.end_frame();
        actions.update(&input);
        assert!(actions.is_pressed("jump") && !actions.is_just_pressed("jump"));
        // Released and pressed again between two updates (fast mashing, a frame hitch).
        key(&mut input, KeyCode::Space, false);
        key(&mut input, KeyCode::Space, true);
        actions.update(&input);
        assert!(actions.is_just_pressed("jump"), "a new press");
        assert_eq!(
            actions.events(),
            [ActionEvent {
                action: "jump".into(),
                pressed: true
            }]
        );
        input.end_frame();
        actions.update(&input);
        assert!(actions.is_pressed("jump") && !actions.is_just_pressed("jump"));

        // Another binding holding the action down: no new press.
        input.handle_event(&PlatformEvent::GamepadButton {
            id: GamepadId(0),
            button: GamepadButton::South,
            pressed: true,
        });
        actions.update(&input);
        input.end_frame();
        key(&mut input, KeyCode::Space, false);
        key(&mut input, KeyCode::Space, true);
        actions.update(&input);
        assert!(actions.is_pressed("jump") && !actions.is_just_pressed("jump"));
    }

    #[test]
    fn rebinding_takes_effect_next_update() {
        let mut input = InputState::new();
        let mut actions = Actions::new(map());
        key(&mut input, KeyCode::KeyJ, true);
        actions.update(&input);
        assert!(!actions.is_pressed("jump"));
        actions
            .map_mut()
            .unbind("jump", Binding::Key(KeyCode::Space));
        actions.map_mut().bind("jump", Binding::Key(KeyCode::KeyJ));
        actions.update(&input);
        assert!(actions.is_just_pressed("jump"));
    }
}
