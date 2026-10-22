//! Crosshair: four lines and a dot in the middle of the screen. The lines spread while the
//! player moves and fires, close again when still, turn red over something that can be
//! damaged, and flash when a shot connects.
//!
//! Put it on the player (next to `Gun`, which emits the `fired` and `hit` signals it listens
//! to on the same node). It aims along the current camera.

use ab_script::prelude::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Crosshair {
    /// Gap between the lines and the centre, in pixels: at rest, and the most it opens.
    pub gap: f32,
    pub max_gap: f32,
    /// Line length and thickness, in pixels.
    pub length: f32,
    pub thickness: f32,
    pub color: Color,
    pub target_color: Color,
    /// How far ahead a target turns the crosshair red, in metres.
    pub range: f32,
    #[serde(skip)]
    parts: Vec<Node>,
    #[serde(skip)]
    spread: f32,
    #[serde(skip)]
    flash: f32,
    #[serde(skip)]
    camera: Option<Node>,
}

impl Default for Crosshair {
    fn default() -> Self {
        Self {
            gap: 6.0,
            max_gap: 26.0,
            length: 9.0,
            thickness: 2.0,
            color: Color::rgba(1.0, 1.0, 1.0, 0.9),
            target_color: Color::rgba(1.0, 0.25, 0.2, 0.95),
            range: 120.0,
            parts: Vec::new(),
            spread: 0.0,
            flash: 0.0,
            camera: None,
        }
    }
}

/// Size of the box the crosshair is drawn in, centred on the screen.
const BOX: f32 = 96.0;

impl Crosshair {
    /// Places the four lines (left, right, up, down) and the dot for a gap.
    fn layout(&self, tree: &mut Tree<'_>, gap: f32) {
        let c = BOX / 2.0;
        let (l, t) = (self.length, self.thickness);
        let rects = [
            (Vec2::new(c - gap - l, c - t / 2.0), Vec2::new(l, t)),
            (Vec2::new(c + gap, c - t / 2.0), Vec2::new(l, t)),
            (Vec2::new(c - t / 2.0, c - gap - l), Vec2::new(t, l)),
            (Vec2::new(c - t / 2.0, c + gap), Vec2::new(t, l)),
            (Vec2::new(c - t / 2.0, c - t / 2.0), Vec2::new(t, t)),
        ];
        for (&part, (position, size)) in self.parts.iter().zip(rects) {
            let _ = tree.control(part).set_rect(position, size);
        }
    }

    fn paint(&self, tree: &mut Tree<'_>, color: Color) {
        for &part in &self.parts {
            let _ = tree.panel(part).set_color(color);
        }
    }
}

impl Script for Crosshair {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        let Ok(layer) = tree.create_canvas_layer("Crosshair", 20) else {
            return;
        };
        let root = tree.root();
        let _ = tree.add_child(root, layer);
        let Ok(frame) = tree.create_control("Frame") else {
            return;
        };
        let _ = tree.add_child(layer, frame);
        let _ = tree
            .control(frame)
            .set_preset(LayoutPreset::Center, Vec2::splat(BOX), 0.0);
        let _ = tree.control(frame).set_mouse_filter(MouseFilter::Ignore);
        for name in ["Left", "Right", "Up", "Down", "Dot"] {
            if let Ok(part) = tree.create_panel(name, self.color) {
                let _ = tree.add_child(frame, part);
                let _ = tree.control(part).set_mouse_filter(MouseFilter::Ignore);
                self.parts.push(part);
            }
        }
        self.layout(tree, self.gap);
        let _ = tree.connect(node, "fired", node);
        let _ = tree.connect(node, "hit", node);
    }

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        // Spread from movement, settling back.
        let speed = tree
            .character(node)
            .velocity()
            .map_or(0.0, |v| Vec2::new(v.x, v.z).length());
        let target = (speed / 6.0).min(1.0) * 0.5;
        self.spread += (target - self.spread) * (1.0 - (-6.0 * delta).exp());
        self.spread = self.spread.clamp(0.0, 1.0);
        self.flash = (self.flash - delta * 6.0).max(0.0);
        let gap = self.gap + (self.max_gap - self.gap) * self.spread + self.flash * 6.0;
        self.layout(tree, gap);

        // Red over something that can be hurt.
        if self.camera.is_none_or(|c| !tree.is_valid(c)) {
            self.camera = current_camera(tree);
        }
        let on_target = self
            .camera
            .and_then(|c| tree.global_transform(c))
            .and_then(|view| {
                let options = QueryOptions::new().excluding(node);
                tree.physics()
                    .cast_ray(view.translation, view.forward(), self.range, &options)
            })
            .is_some_and(|hit| damageable(tree, hit.node));
        let color = if self.flash > 0.0 {
            Color::WHITE
        } else if on_target {
            self.target_color
        } else {
            self.color
        };
        self.paint(tree, color);
    }

    fn on_signal(&mut self, _tree: &mut Tree<'_>, _node: Node, signal: &Signal<'_>) {
        if signal.is("fired") {
            self.spread = (self.spread + 0.18).min(1.0);
        } else if signal.is("hit") {
            self.flash = 1.0;
        }
    }
}

/// Whether `node` or one of its ancestors can be damaged.
fn damageable(tree: &Tree<'_>, node: Node) -> bool {
    let mut current = Some(node);
    while let Some(n) = current {
        if tree.is_in_group(n, "damageable") {
            return true;
        }
        current = tree.parent(n);
    }
    false
}

/// The camera the game renders from.
fn current_camera(tree: &mut Tree<'_>) -> Option<Node> {
    let mut stack = vec![tree.root()];
    while let Some(n) = stack.pop() {
        if tree.class_name(n).as_deref() == Some("Camera3D") && tree.camera(n).is_current() {
            return Some(n);
        }
        stack.extend(tree.children(n));
    }
    None
}
