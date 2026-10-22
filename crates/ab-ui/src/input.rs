//! Pointer hover/click, focus and keyboard/gamepad navigation.

use crate::frame::HitTarget;
use crate::widgets::Button;
use crate::Control;
use ab_core::Vec2;
use ab_ecs::World;
use ab_input::{GamepadButton, InputState, KeyCode, MouseButton};
use ab_scene::{NodeHandle, Signal, SignalQueue};

/// Something the UI did this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiEvent {
    /// A button was activated (Godot's `pressed` signal).
    Pressed(NodeHandle),
    /// Keyboard/gamepad focus moved (or was cleared).
    FocusChanged(Option<NodeHandle>),
}

/// ECS resource: layout results shared with input, and the interaction state.
#[derive(Debug, Clone, Default)]
pub struct UiState {
    pub(crate) scale: f32,
    pub(crate) viewport: Vec2,
    pub(crate) targets: Vec<HitTarget>,
    hovered: Option<NodeHandle>,
    focus: Option<NodeHandle>,
    events: Vec<UiEvent>,
}

impl UiState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pixels per UI unit as of the last layout.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Size of the last laid-out target in pixels.
    pub fn viewport(&self) -> Vec2 {
        self.viewport
    }

    /// The control under the pointer that blocks input, if any.
    pub fn hovered(&self) -> Option<NodeHandle> {
        self.hovered
    }

    /// Whether the pointer is over UI that blocks input: game code should ignore clicks then.
    pub fn is_pointer_over_ui(&self) -> bool {
        self.hovered.is_some()
    }

    pub fn focused(&self) -> Option<NodeHandle> {
        self.focus
    }

    /// Gives a control keyboard/gamepad focus (Godot's `grab_focus`).
    pub fn grab_focus(&mut self, node: NodeHandle) {
        if self.focus != Some(node) {
            self.focus = Some(node);
            self.events.push(UiEvent::FocusChanged(Some(node)));
        }
    }

    pub fn release_focus(&mut self) {
        if self.focus.take().is_some() {
            self.events.push(UiEvent::FocusChanged(None));
        }
    }

    /// Events of this frame.
    pub fn events(&self) -> &[UiEvent] {
        &self.events
    }

    /// Whether `button` was activated this frame.
    pub fn was_pressed(&self, button: NodeHandle) -> bool {
        self.events.contains(&UiEvent::Pressed(button))
    }
}

/// Applies this frame's input to the UI laid out last frame: hover, mouse clicks
/// (activation on release over the button, as in Godot), Tab / Shift+Tab focus cycling,
/// arrow keys and the gamepad D-pad while something has focus, Enter / Space / gamepad A to
/// activate, Escape to drop focus. The engine runs it in `PreUpdate`.
pub fn ui_input_system(world: &mut World) {
    let world: &World = world;
    let (Some(mut state), Some(input)) = (
        world.resource_mut::<UiState>(),
        world.resource::<InputState>(),
    ) else {
        return;
    };
    state.events.clear();
    let Some(controls) = world.storage::<Control>() else {
        state.hovered = None;
        return;
    };
    let mut buttons = world.storage_mut::<Button>();
    let alive = |node: NodeHandle| controls.contains(node.entity());

    // Hover: topmost blocking control under the pointer.
    let cursor = input
        .cursor_position()
        .filter(|_| state.scale > 0.0)
        .map(|p| p / state.scale);
    state.hovered = cursor.and_then(|point| {
        state
            .targets
            .iter()
            .rev()
            .find(|t| t.blocks && t.rect.contains(point))
            .map(|t| NodeHandle::from_entity(t.entity))
            .filter(|&n| alive(n))
    });
    let hovered = state.hovered;
    if state.focus.is_some_and(|f| !alive(f)) {
        state.release_focus();
    }

    let mut pressed: Vec<NodeHandle> = Vec::new();
    let enabled = |buttons: &Option<std::cell::RefMut<'_, ab_ecs::SparseSet<Button>>>,
                   n: NodeHandle| {
        buttons
            .as_ref()
            .and_then(|b| b.get(n.entity()))
            .is_some_and(|b| !b.disabled)
    };

    // Mouse.
    if input.is_mouse_just_pressed(MouseButton::Left) {
        match hovered {
            Some(node) => {
                if enabled(&buttons, node) {
                    if let Some(button) = buttons.as_mut().and_then(|b| b.get_mut(node.entity())) {
                        button.held = true;
                    }
                }
                let focusable = state
                    .targets
                    .iter()
                    .any(|t| t.entity == node.entity() && t.focusable);
                if focusable {
                    state.grab_focus(node);
                }
            }
            None => state.release_focus(),
        }
    }
    let released = input.is_mouse_just_released(MouseButton::Left)
        || !input.is_mouse_pressed(MouseButton::Left);
    if let Some(buttons) = buttons.as_mut() {
        for (entity, button) in buttons.iter_mut() {
            let node = NodeHandle::from_entity(entity);
            button.clicked = false;
            button.hovered = hovered == Some(node) && !button.disabled;
            if released && button.held {
                button.held = false;
                if hovered == Some(node) && !button.disabled {
                    pressed.push(node);
                }
            }
        }
    }

    // Keyboard and gamepad navigation.
    let pad = input.active_gamepad().and_then(|id| input.gamepad(id));
    let pad_pressed = |b| pad.is_some_and(|p| p.buttons.is_just_pressed(b));
    let shift =
        input.is_key_pressed(KeyCode::ShiftLeft) || input.is_key_pressed(KeyCode::ShiftRight);
    let tab = input.is_key_just_pressed(KeyCode::Tab);
    let has_focus = state.focus.is_some();
    let next = (tab && !shift)
        || (has_focus
            && (input.is_key_just_pressed(KeyCode::ArrowDown)
                || pad_pressed(GamepadButton::DPadDown)));
    let previous = (tab && shift)
        || (has_focus
            && (input.is_key_just_pressed(KeyCode::ArrowUp) || pad_pressed(GamepadButton::DPadUp)));
    if next || previous {
        let order: Vec<NodeHandle> = state
            .targets
            .iter()
            .filter(|t| t.focusable)
            .map(|t| NodeHandle::from_entity(t.entity))
            .filter(|&n| {
                !buttons
                    .as_ref()
                    .and_then(|b| b.get(n.entity()))
                    .is_some_and(|b| b.disabled)
            })
            .collect();
        if !order.is_empty() {
            let current = state.focus.and_then(|f| order.iter().position(|&n| n == f));
            let index = match (current, next) {
                (Some(i), true) => (i + 1) % order.len(),
                (Some(i), false) => (i + order.len() - 1) % order.len(),
                (None, true) => 0,
                (None, false) => order.len() - 1,
            };
            state.grab_focus(order[index]);
        }
    }
    if input.is_key_just_pressed(KeyCode::Escape) {
        state.release_focus();
    }
    let activate = input.is_key_just_pressed(KeyCode::Enter)
        || input.is_key_just_pressed(KeyCode::NumpadEnter)
        || input.is_key_just_pressed(KeyCode::Space)
        || pad_pressed(GamepadButton::South);
    if let Some(focus) = state.focus.filter(|_| activate) {
        if enabled(&buttons, focus) {
            pressed.push(focus);
        }
    }

    let focus = state.focus;
    if let Some(buttons) = buttons.as_mut() {
        for (entity, button) in buttons.iter_mut() {
            let node = NodeHandle::from_entity(entity);
            button.focused = focus == Some(node);
            if pressed.contains(&node) {
                button.clicked = true;
            }
        }
    }
    if let Some(mut queue) = world.resource_mut::<SignalQueue>() {
        for &node in &pressed {
            queue.push(Signal::new(node, Button::PRESSED));
        }
    }
    for node in pressed {
        state.events.push(UiEvent::Pressed(node));
    }
}
