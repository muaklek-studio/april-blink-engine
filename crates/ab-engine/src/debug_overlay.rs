//! On-screen debug overlay: frame rate, frame-time breakdown, slowest systems and scene
//! counters. Toggled with [`EngineConfig::debug_overlay_key`](crate::EngineConfig) (F3).

use crate::profiler::Profiler;
use crate::RenderStats;
use ab_core::{Color, Vec2};
use ab_physics::PhysicsWorld;
use ab_scene::{NodeHandle, SceneTree};
use ab_ui::{Control, Label, LayoutPreset, MouseFilter, Panel, StyleBox, TextStyle, UiNodes};

/// Canvas layer of the overlay: above game UI.
const OVERLAY_LAYER: i32 = 1000;
/// Seconds between text refreshes (readable numbers, little allocation).
const REFRESH: f32 = 0.25;

/// ECS resource controlling the overlay.
#[derive(Debug, Clone, Default)]
pub struct DebugOverlay {
    pub visible: bool,
    nodes: Option<(NodeHandle, NodeHandle)>,
    since_refresh: f32,
}

impl DebugOverlay {
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }
}

/// Creates, shows/hides and refreshes the overlay nodes.
pub(crate) fn update(tree: &mut SceneTree, delta: f32) {
    let Some(state) = tree.world().resource::<DebugOverlay>().map(|o| o.clone()) else {
        return;
    };
    let nodes = state
        .nodes
        .filter(|(layer, label)| tree.is_valid(*layer) && tree.is_valid(*label));
    let (layer, label) = match (nodes, state.visible) {
        (None, false) => return,
        (Some((layer, label)), _) => (layer, label),
        (None, true) => create(tree),
    };
    let _ = tree.set_visible(layer, state.visible);
    let refresh = state.since_refresh + delta;
    let due = state.visible && (refresh >= REFRESH || nodes.is_none());
    if let Some(mut overlay) = tree.world().resource_mut::<DebugOverlay>() {
        overlay.nodes = Some((layer, label));
        overlay.since_refresh = if due { 0.0 } else { refresh };
    }
    if due {
        let text = report(tree);
        if let Some(mut label) = tree.component_mut::<Label>(label) {
            label.text = text;
        }
    }
}

fn create(tree: &mut SceneTree) -> (NodeHandle, NodeHandle) {
    let layer = tree.create_canvas_layer("DebugOverlay", OVERLAY_LAYER);
    let panel = tree.create_panel(
        "Panel",
        Control::new()
            .with_preset(LayoutPreset::BottomRight, Vec2::new(600.0, 160.0), 16.0)
            .with_mouse_filter(MouseFilter::Ignore),
        Panel {
            style: StyleBox::flat(Color::rgba(0.0, 0.0, 0.0, 0.72))
                .with_corner_radius(6.0)
                .with_border(1.0, Color::rgba(0.4, 1.0, 0.6, 0.4)),
        },
    );
    let label = tree.create_label(
        "Text",
        Control::new().with_preset(LayoutPreset::FullRect, Vec2::ZERO, 12.0),
        Label::new("")
            .with_style(TextStyle::new(17.0).with_color(Color::from_srgb8(170, 255, 190, 255))),
    );
    tree.add_child(tree.root(), layer)
        .expect("fresh node under the root");
    tree.add_child(layer, panel).expect("fresh node");
    tree.add_child(panel, label).expect("fresh node");
    (layer, label)
}

/// The overlay text.
pub(crate) fn report(tree: &SceneTree) -> String {
    let world = tree.world();
    let Some(profiler) = world.resource::<Profiler>() else {
        return String::new();
    };
    let frame = profiler.frame_statistics();
    let ms = |name: &str| profiler.average_ms(name).unwrap_or(0.0);
    let mut text = format!(
        "FPS {:.0}  ·  {:.2} ms (min {:.1} / max {:.1})\n\
         CPU {:.2} ms: fixed {:.2} · scripts {:.2} · ui {:.2} · render {:.2}\n",
        frame.fps,
        frame.average_ms,
        frame.min_ms,
        frame.max_ms,
        ms("frame"),
        ms("fixed"),
        ms("scripts.process") + ms("scripts.physics_process"),
        ms("ui_layout"),
        ms("render"),
    );
    let systems: Vec<String> = profiler
        .slowest("system.", 4)
        .iter()
        .map(|e| {
            format!(
                "{} {:.2}",
                e.name.trim_start_matches("system."),
                e.average_ms
            )
        })
        .collect();
    text.push_str(&format!("systems: {}\n", systems.join(" · ")));
    let scripts: Vec<String> = profiler
        .slowest("script.", 4)
        .iter()
        .map(|e| {
            format!(
                "{} {:.2}",
                e.name.trim_start_matches("script."),
                e.average_ms
            )
        })
        .collect();
    if !scripts.is_empty() {
        text.push_str(&format!("scripts: {}\n", scripts.join(" · ")));
    }
    let render = world
        .resource::<RenderStats>()
        .map(|r| r.0)
        .unwrap_or_default();
    let (bodies, colliders, joints) = world.resource::<PhysicsWorld>().map_or((0, 0, 0), |p| {
        (p.body_count(), p.collider_count(), p.joint_count())
    });
    text.push_str(&format!(
        "nodes {} · bodies {} / colliders {} / joints {} (F4: physics view)\n\
         draws {} · objects {} · culled {} · shadow draws {} · lights {} · workers {}",
        tree.node_count(),
        bodies,
        colliders,
        joints,
        render.draw_calls,
        render.objects,
        render.culled,
        render.shadow_draw_calls,
        render.lights,
        ab_core::jobs::worker_count(),
    ));
    text
}
