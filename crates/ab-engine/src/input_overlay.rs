//! On-screen input debugger: held keys, mouse, gamepads, every action with its strength and
//! bindings, and the latest presses. Toggled with
//! [`EngineConfig::debug_input_key`](crate::EngineConfig) (F8).

use ab_core::{Color, Vec2};
use ab_input::{debug, Actions, InputLog, InputState};
use ab_scene::{NodeHandle, SceneTree};
use ab_ui::{Control, Label, LayoutPreset, MouseFilter, Panel, StyleBox, TextStyle, UiNodes};

/// Canvas layer: above game UI and the stats overlay.
const LAYER: i32 = 1001;

/// ECS resource controlling the input overlay.
#[derive(Debug, Clone, Default)]
pub struct InputOverlay {
    pub visible: bool,
    nodes: Option<(NodeHandle, NodeHandle)>,
}

impl InputOverlay {
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }
}

/// Records the frame's input into the [`InputLog`] (the engine runs it after the actions
/// update, so the editor's Debug panel has it too).
pub fn record_input_log(world: &mut ab_ecs::World) {
    let time = world.resource::<ab_core::Time>().map_or(0.0, |t| t.elapsed);
    let (Some(input), Some(actions), Some(mut log)) = (
        world.resource::<InputState>(),
        world.resource::<Actions>(),
        world.resource_mut::<InputLog>(),
    ) else {
        return;
    };
    log.record(time, &input, &actions);
}

/// Creates, shows/hides and refreshes the overlay (every frame while shown: input changes
/// fast).
pub(crate) fn update(tree: &mut SceneTree) {
    let Some(state) = tree.world().resource::<InputOverlay>().map(|o| o.clone()) else {
        return;
    };
    let nodes = state
        .nodes
        .filter(|(layer, label)| tree.is_valid(*layer) && tree.is_valid(*label));
    let (layer, label) = match (nodes, state.visible) {
        (None, false) => return,
        (Some(nodes), _) => nodes,
        (None, true) => create(tree),
    };
    let _ = tree.set_visible(layer, state.visible);
    if let Some(mut overlay) = tree.world().resource_mut::<InputOverlay>() {
        overlay.nodes = Some((layer, label));
    }
    if !state.visible {
        return;
    }
    let text = {
        let world = tree.world();
        match (world.resource::<InputState>(), world.resource::<Actions>()) {
            (Some(input), Some(actions)) => {
                let log = world.resource::<InputLog>();
                format!(
                    "INPUT (F8)\n{}",
                    debug::report(&input, &actions, log.as_deref())
                )
            }
            _ => "INPUT (F8)\nno input".into(),
        }
    };
    if let Some(mut label) = tree.component_mut::<Label>(label) {
        label.text = text;
    }
}

fn create(tree: &mut SceneTree) -> (NodeHandle, NodeHandle) {
    let layer = tree.create_canvas_layer("InputOverlay", LAYER);
    let panel = tree.create_panel(
        "Panel",
        Control::new()
            .with_preset(LayoutPreset::TopLeft, Vec2::new(620.0, 520.0), 16.0)
            .with_mouse_filter(MouseFilter::Ignore),
        Panel {
            style: StyleBox::flat(Color::rgba(0.0, 0.0, 0.0, 0.72))
                .with_corner_radius(6.0)
                .with_border(1.0, Color::rgba(0.45, 0.7, 1.0, 0.45)),
        },
    );
    let label = tree.create_label(
        "Text",
        Control::new().with_preset(LayoutPreset::FullRect, Vec2::ZERO, 12.0),
        Label::new("")
            .with_style(TextStyle::new(15.0).with_color(Color::from_srgb8(180, 215, 255, 255))),
    );
    tree.add_child(tree.root(), layer)
        .expect("fresh node under the root");
    tree.add_child(layer, panel).expect("fresh node");
    tree.add_child(panel, label).expect("fresh node");
    (layer, label)
}
