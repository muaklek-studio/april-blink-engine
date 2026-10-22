use crate::anim::UiAnimator;
use crate::class;
use crate::control::{Control, MouseFilter};
use crate::widgets::{BoxContainer, Button, CanvasLayer, Label, Panel, ProgressBar, TextureRect};
use ab_ecs::RefMut;
use ab_scene::{NodeHandle, SceneTree};

/// UI node constructors on [`SceneTree`] (`use ab_ui::UiNodes;`). Each creates a detached
/// node; attach it under a [`CanvasLayer`] node or another control.
pub trait UiNodes {
    fn create_canvas_layer(&mut self, name: &str, layer: i32) -> NodeHandle;
    /// A plain `Control` (grouping, anchoring).
    fn create_control(&mut self, name: &str, control: Control) -> NodeHandle;
    fn create_panel(&mut self, name: &str, control: Control, panel: Panel) -> NodeHandle;
    /// Labels let the pointer through.
    fn create_label(&mut self, name: &str, control: Control, label: Label) -> NodeHandle;
    /// Buttons can take focus.
    fn create_button(&mut self, name: &str, control: Control, button: Button) -> NodeHandle;
    /// Texture rects let the pointer through.
    fn create_texture_rect(
        &mut self,
        name: &str,
        control: Control,
        texture: TextureRect,
    ) -> NodeHandle;
    fn create_progress_bar(&mut self, name: &str, control: Control, bar: ProgressBar)
        -> NodeHandle;
    /// `VBoxContainer` or `HBoxContainer` (by `container.vertical`); lets the pointer through.
    fn create_box_container(
        &mut self,
        name: &str,
        control: Control,
        container: BoxContainer,
    ) -> NodeHandle;
    /// The node's animator, added on first use. `None` if the node is not a control.
    fn animate(&mut self, node: NodeHandle) -> Option<RefMut<'_, UiAnimator>>;
}

fn passive(control: Control) -> Control {
    Control {
        mouse_filter: MouseFilter::Ignore,
        ..control
    }
}

impl UiNodes for SceneTree {
    fn create_canvas_layer(&mut self, name: &str, layer: i32) -> NodeHandle {
        self.spawn_node(name, class::CANVAS_LAYER, (CanvasLayer { layer },))
    }

    fn create_control(&mut self, name: &str, control: Control) -> NodeHandle {
        self.spawn_node(name, class::CONTROL, (passive(control),))
    }

    fn create_panel(&mut self, name: &str, control: Control, panel: Panel) -> NodeHandle {
        self.spawn_node(name, class::PANEL, (control, panel))
    }

    fn create_label(&mut self, name: &str, control: Control, label: Label) -> NodeHandle {
        self.spawn_node(name, class::LABEL, (passive(control), label))
    }

    fn create_button(&mut self, name: &str, control: Control, button: Button) -> NodeHandle {
        let control = Control {
            focusable: true,
            ..control
        };
        self.spawn_node(name, class::BUTTON, (control, button))
    }

    fn create_texture_rect(
        &mut self,
        name: &str,
        control: Control,
        texture: TextureRect,
    ) -> NodeHandle {
        self.spawn_node(name, class::TEXTURE_RECT, (passive(control), texture))
    }

    fn create_progress_bar(
        &mut self,
        name: &str,
        control: Control,
        bar: ProgressBar,
    ) -> NodeHandle {
        self.spawn_node(name, class::PROGRESS_BAR, (control, bar))
    }

    fn create_box_container(
        &mut self,
        name: &str,
        control: Control,
        container: BoxContainer,
    ) -> NodeHandle {
        let class_name = if container.vertical {
            class::VBOX_CONTAINER
        } else {
            class::HBOX_CONTAINER
        };
        self.spawn_node(name, class_name, (passive(control), container))
    }

    fn animate(&mut self, node: NodeHandle) -> Option<RefMut<'_, UiAnimator>> {
        self.component::<Control>(node)?;
        if self.component::<UiAnimator>(node).is_none() {
            self.insert_component(node, UiAnimator::default()).ok()?;
        }
        self.component_mut::<UiAnimator>(node)
    }
}
