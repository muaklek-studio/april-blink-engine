//! UI: creating controls, placing them, animating them.

use super::Builder;
use crate::script_host::call::{Call, CallError};
use ab_scene::NodeHandle;
use ab_ui::{
    BoxContainer, Button, Control, Easing, Label, LayoutPreset, Panel, ProgressBar, StyleBox,
    TweenTarget, UiNodes, UiState,
};

pub(crate) fn easing(name: &str) -> Result<Easing, CallError> {
    Ok(match name {
        "linear" => Easing::Linear,
        "ease_in" => Easing::EaseIn,
        "ease_out" => Easing::EaseOut,
        "ease_in_out" => Easing::EaseInOut,
        "back_out" => Easing::BackOut,
        "elastic_out" => Easing::ElasticOut,
        "bounce_out" => Easing::BounceOut,
        "sine_in_out" => Easing::SineInOut,
        other => return Err(CallError::BadArguments(format!("unknown easing `{other}`"))),
    })
}

fn preset(name: &str) -> Result<LayoutPreset, CallError> {
    crate::scene_file::ui::preset(name).map_err(CallError::BadArguments)
}

fn control(c: &Call<'_>, node: NodeHandle, f: impl FnOnce(&mut Control)) -> Result<(), CallError> {
    let mut control = c
        .tree
        .component_mut::<Control>(node)
        .ok_or_else(|| CallError::Failed(format!("{node} is not a control")))?;
    f(&mut control);
    Ok(())
}

fn animate(
    c: &mut Call<'_>,
    node: NodeHandle,
    f: impl FnOnce(&mut ab_ui::UiAnimator),
) -> Result<(), CallError> {
    let mut animator = c
        .tree
        .animate(node)
        .ok_or_else(|| CallError::Failed(format!("{node} is not a control")))?;
    f(&mut animator);
    Ok(())
}

pub(super) fn add(b: &mut Builder) {
    b.add("ui.create_canvas_layer", |c| {
        let layer = i32::try_from(c.i64(1)?)
            .map_err(|_| CallError::BadArguments("layer out of range".into()))?;
        let node = c.tree.create_canvas_layer(c.str(0)?, layer);
        c.push_node(node);
        Ok(())
    });
    b.add("ui.create_control", |c| {
        let node = c.tree.create_control(c.str(0)?, Control::new());
        c.push_node(node);
        Ok(())
    });
    b.add("ui.create_label", |c| {
        let node = c
            .tree
            .create_label(c.str(0)?, Control::new(), Label::new(c.str(1)?));
        c.push_node(node);
        Ok(())
    });
    b.add("ui.create_button", |c| {
        let node = c
            .tree
            .create_button(c.str(0)?, Control::new(), Button::new(c.str(1)?));
        c.push_node(node);
        Ok(())
    });
    b.add("ui.create_panel", |c| {
        let panel = Panel {
            style: StyleBox::flat(c.color(1)?),
        };
        let node = c.tree.create_panel(c.str(0)?, Control::new(), panel);
        c.push_node(node);
        Ok(())
    });
    b.add("ui.create_progress_bar", |c| {
        let bar = ProgressBar::new(c.f32(1)?, c.f32(2)?, c.f32(3)?);
        let node = c.tree.create_progress_bar(c.str(0)?, Control::new(), bar);
        c.push_node(node);
        Ok(())
    });
    b.add("ui.create_box_container", |c| {
        let container = if c.bool(1)? {
            BoxContainer::vertical()
        } else {
            BoxContainer::horizontal()
        };
        let node = c
            .tree
            .create_box_container(c.str(0)?, Control::new(), container);
        c.push_node(node);
        Ok(())
    });
    b.add("ui.is_pointer_over", |c| {
        let over = c
            .tree
            .world()
            .resource::<UiState>()
            .is_some_and(|ui| ui.is_pointer_over_ui());
        c.push_bool(over);
        Ok(())
    });

    b.add("control.set_preset", |c| {
        let node = c.node(0)?;
        let (preset, size, margin) = (preset(c.str(1)?)?, c.vec2(2)?, c.f32(3)?);
        control(c, node, |control| {
            *control = control.clone().with_preset(preset, size, margin);
        })
    });
    b.add("control.set_rect", |c| {
        let node = c.node(0)?;
        let (position, size) = (c.vec2(1)?, c.vec2(2)?);
        control(c, node, |control| {
            let placed = Control::at(position, size);
            control.anchors = placed.anchors;
            control.offsets = placed.offsets;
        })
    });
    b.add("control.slide_in", |c| {
        let node = c.node(0)?;
        let (offset, seconds) = (c.vec2(1)?, c.f32(2)?);
        animate(c, node, |a| {
            a.slide_in(offset, seconds);
        })
    });
    b.add("control.slide_out", |c| {
        let node = c.node(0)?;
        let (offset, seconds) = (c.vec2(1)?, c.f32(2)?);
        animate(c, node, |a| {
            a.slide_out(offset, seconds);
        })
    });
    b.add("control.tween_alpha", |c| {
        let node = c.node(0)?;
        let (from, to, seconds, delay) = (c.f32(1)?, c.f32(2)?, c.f32(3)?, c.f32(4)?);
        let easing = easing(c.str(5)?)?;
        animate(c, node, |a| {
            a.tween(TweenTarget::Alpha { from, to }, seconds, delay, easing);
        })
    });
    b.add("control.stop_animations", |c| {
        let node = c.node(0)?;
        animate(c, node, ab_ui::UiAnimator::stop)
    });
    b.add("control.is_animating", |c| {
        let node = c.node(0)?;
        let animating = c
            .tree
            .component::<ab_ui::UiAnimator>(node)
            .is_some_and(|a| a.is_animating());
        c.push_bool(animating);
        Ok(())
    });
}
