use crate::CRATES_GROUP;
use ab_script::prelude::*;
use ab_script::PRESSED;

/// Drives the HUD built by the demo: FPS readout, the "crates at rest" bar and the Drop /
/// Mute buttons. Lives on a node next to the HUD's panels and finds them by path.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HudScript {
    fps: Option<Node>,
    bar: Option<Node>,
    drop_button: Option<Node>,
    mute_button: Option<Node>,
    muted: bool,
    frames: u32,
    elapsed: f32,
    #[serde(skip)]
    rng: Rng,
}

impl HudScript {
    /// Lifts every crate a few metres and gives it a random tilt; physics does the rest.
    fn drop_crates(&mut self, tree: &mut Tree<'_>) {
        for node in tree.nodes_in_group(CRATES_GROUP) {
            let Some(position) = tree.position(node) else {
                continue;
            };
            let lifted = position + Vec3::Y * self.rng.range_f32(3.0, 6.0);
            let rotation = Quat::from_euler(
                EulerRot::YXZ,
                self.rng.range_f32(0.0, std::f32::consts::TAU),
                self.rng.range_f32(-0.6, 0.6),
                self.rng.range_f32(-0.6, 0.6),
            );
            let _ = tree.set_position(node, lifted);
            let _ = tree.set_rotation(node, rotation);
            let mut body = tree.body(node);
            let _ = body.set_linear_velocity(Vec3::ZERO);
            let _ = body.set_angular_velocity(Vec3::ZERO);
        }
    }

    fn toggle_mute(&mut self, tree: &mut Tree<'_>) {
        self.muted = !self.muted;
        tree.set_master_volume_db(if self.muted { -80.0 } else { 0.0 });
        if let Some(button) = self.mute_button {
            let text = if self.muted {
                "เปิดเสียง · Unmute"
            } else {
                "ปิดเสียง · Mute"
            };
            let _ = tree.button(button).set_text(text);
        }
    }
}

impl Script for HudScript {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        self.fps = tree.get_node(node, "../TitleCard/Column/Fps");
        self.bar = tree.get_node(node, "../PhysicsPanel/Stack/AtRest");
        self.drop_button = tree.get_node(node, "../PhysicsPanel/Stack/Buttons/Drop");
        self.mute_button = tree.get_node(node, "../PhysicsPanel/Stack/Buttons/Mute");
        // Connections target this node, so they reach new script versions after a reload.
        for button in [self.drop_button, self.mute_button].into_iter().flatten() {
            let _ = tree.connect(button, PRESSED, node);
        }
        let crates = tree.nodes_in_group(CRATES_GROUP).len();
        if let Some(bar) = self.bar {
            let _ = tree.progress_bar(bar).set_max(crates.max(1) as f32);
        }
    }

    fn on_signal(&mut self, tree: &mut Tree<'_>, _node: Node, signal: &Signal<'_>) {
        if !signal.is(PRESSED) {
            return;
        }
        if Some(signal.source()) == self.drop_button {
            self.drop_crates(tree);
        } else if Some(signal.source()) == self.mute_button {
            self.toggle_mute(tree);
        }
    }

    fn process(&mut self, tree: &mut Tree<'_>, _node: Node, delta: f32) {
        self.frames += 1;
        self.elapsed += delta;
        if self.elapsed >= 0.5 {
            let fps = self.frames as f32 / self.elapsed;
            if let Some(label) = self.fps {
                let _ = tree
                    .label(label)
                    .set_text(&format!("FPS {fps:.0} · {:.2} ms", 1000.0 / fps));
            }
            self.frames = 0;
            self.elapsed = 0.0;
        }

        let resting = tree
            .nodes_in_group(CRATES_GROUP)
            .into_iter()
            .filter(|&c| tree.body(c).is_sleeping())
            .count();
        if let Some(bar) = self.bar {
            let _ = tree.progress_bar(bar).set_value(resting as f32);
        }
    }
}
