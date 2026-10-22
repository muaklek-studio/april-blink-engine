//! Melee combat: attacks chained into combos. Press `attack` again during an attack's combo
//! window to go on to the next one; each hit lands part way through its animation, damaging
//! whatever is in front (anything with a `Health` script), pushing it, and shaking the camera.
//!
//! Put it on the character body (with its rigged model as a child, or not: the hits work
//! without animations). Action: `attack`.

use ab_script::prelude::*;

/// One attack of the combo.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Attack {
    /// The model's animation for it (played once; skipped when the model does not have it).
    pub clip: String,
    /// Seconds the attack takes.
    pub duration: f32,
    /// When the hit lands, in seconds from the start.
    pub hit_time: f32,
    pub damage: f32,
    /// Reach in front of the body, and the radius of the hit.
    pub reach: f32,
    pub radius: f32,
    /// Push given to what is hit (N·s, for physics bodies and ragdolls).
    pub knockback: f32,
}

impl Default for Attack {
    fn default() -> Self {
        Self {
            clip: "Attack".into(),
            duration: 0.55,
            hit_time: 0.3,
            damage: 15.0,
            reach: 1.0,
            radius: 0.7,
            knockback: 30.0,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MeleeCombat {
    /// The combo, in order.
    pub combo: Vec<Attack>,
    /// Seconds at the end of each attack (and after it) when a press continues the combo.
    pub combo_window: f32,
    #[serde(skip)]
    step: Option<usize>,
    #[serde(skip)]
    elapsed: f32,
    #[serde(skip)]
    hit_done: bool,
    #[serde(skip)]
    queued: bool,
    #[serde(skip)]
    model: Option<Node>,
}

impl Default for MeleeCombat {
    fn default() -> Self {
        Self {
            combo: vec![
                Attack::default(),
                Attack {
                    damage: 20.0,
                    ..Attack::default()
                },
                Attack {
                    duration: 0.8,
                    hit_time: 0.45,
                    damage: 35.0,
                    radius: 1.0,
                    knockback: 80.0,
                    ..Attack::default()
                },
            ],
            combo_window: 0.35,
            step: None,
            elapsed: 0.0,
            hit_done: false,
            queued: false,
            model: None,
        }
    }
}

/// Signal the body emits when an attack starts (argument: its index in the combo), and when
/// one connects (argument: the damage).
pub const ATTACKED: &str = "attacked";
pub const HIT: &str = "hit";

impl MeleeCombat {
    fn start(&mut self, tree: &mut Tree<'_>, node: Node, step: usize) {
        self.step = Some(step);
        self.elapsed = 0.0;
        self.hit_done = false;
        self.queued = false;
        let attack = &self.combo[step];
        if let Some(model) = self.model {
            if tree.animation(model).clips().contains(&attack.clip) {
                let _ = tree.locomotion(model).play_action(&attack.clip);
            }
        }
        let _ = tree.emit_with(node, ATTACKED, step as f32);
    }

    /// Damages and pushes everything in front of the body.
    fn strike(&mut self, tree: &mut Tree<'_>, node: Node, attack: &Attack) {
        let Some(body) = tree.global_transform(node) else {
            return;
        };
        let forward = Vec3::new(body.forward().x, 0.0, body.forward().z).normalize_or_zero();
        let centre = body.translation + forward * attack.reach + Vec3::Y * 0.2;
        let options = QueryOptions::new().excluding(node);
        let mut struck: Vec<Node> = Vec::new();
        for hit in tree
            .physics()
            .intersect_sphere(centre, attack.radius, &options)
        {
            // The node with health: the hit body or one above it (a ragdoll part's character).
            let target = damageable(tree, hit);
            if let Some(target) = target.filter(|t| *t != node && !struck.contains(t)) {
                struck.push(target);
                let _ = tree.deal_damage(Some(node), target, attack.damage);
            }
            let _ = tree
                .body(hit)
                .apply_impulse_at(forward * attack.knockback, centre);
        }
        if !struck.is_empty() {
            tree.shake_cameras(0.25);
            let _ = tree.emit_with(node, HIT, attack.damage);
        }
    }
}

/// `node` or the nearest ancestor in the `damageable` group.
fn damageable(tree: &Tree<'_>, node: Node) -> Option<Node> {
    let mut current = Some(node);
    while let Some(n) = current {
        if tree.is_in_group(n, "damageable") {
            return Some(n);
        }
        current = tree.parent(n);
    }
    None
}

impl Script for MeleeCombat {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        self.model = tree
            .children(node)
            .into_iter()
            .find(|&c| !tree.animation(c).clips().is_empty());
    }

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        if self.combo.is_empty() {
            return;
        }
        let pressed = tree.input().is_just_pressed("attack");
        let Some(step) = self.step else {
            if pressed {
                self.start(tree, node, 0);
            }
            return;
        };
        let attack = self.combo[step].clone();
        self.elapsed += delta;
        if !self.hit_done && self.elapsed >= attack.hit_time {
            self.hit_done = true;
            self.strike(tree, node, &attack);
        }
        // A press late in the attack queues the next one.
        if pressed && self.elapsed >= attack.duration - self.combo_window {
            self.queued = true;
        }
        if self.elapsed >= attack.duration {
            let next = step + 1;
            if self.queued && next < self.combo.len() {
                self.start(tree, node, next);
            } else if self.elapsed >= attack.duration + self.combo_window || self.queued {
                // The combo ends (or restarts from the first attack when pressed at its end).
                self.step = None;
                if self.queued {
                    self.start(tree, node, 0);
                }
            }
        }
    }
}
