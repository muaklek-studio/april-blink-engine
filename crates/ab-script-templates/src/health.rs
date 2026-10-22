//! Health: hit points that other scripts damage with a signal, a short invulnerability after
//! each hit, and a death that lets the character go limp (its ragdoll) with blood.
//!
//! Damage it from any script, without knowing this script's type (the attacker shows in the
//! engine's damage trace: F9, or the editor's Debug panel):
//!
//! ```ignore
//! let _ = tree.deal_damage(Some(attacker), target, 25.0);
//! // or, without the attacker: tree.emit_with(target, "damage", 25.0)
//! ```
//!
//! Signals it emits on its node: `damaged` (amount), `healed` (amount), `died`. Its node is in
//! the `damageable` group while alive (weapons look for it up the hit node's parents) and
//! ignores damage while in the `invulnerable` group (a dash).

use ab_script::prelude::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Health {
    pub max: f32,
    pub current: f32,
    /// Seconds without damage after a hit.
    pub invulnerable_after_hit: f32,
    /// Regained per second after `regen_delay` seconds without damage (0: none).
    pub regen_per_second: f32,
    pub regen_delay: f32,
    /// Bleed when hit and go limp (ragdoll) when killed.
    pub bleed: bool,
    pub ragdoll_on_death: bool,
    /// Free the node this many seconds after death (0: keep it).
    pub remove_after: f32,
    #[serde(skip)]
    since_hit: f32,
    #[serde(skip)]
    dead_for: Option<f32>,
}

impl Default for Health {
    fn default() -> Self {
        Self {
            max: 100.0,
            current: 100.0,
            invulnerable_after_hit: 0.2,
            regen_per_second: 0.0,
            regen_delay: 4.0,
            bleed: true,
            ragdoll_on_death: true,
            remove_after: 0.0,
            since_hit: f32::MAX,
            dead_for: None,
        }
    }
}

/// Signal names.
pub const DAMAGE: &str = "damage";
pub const HEAL: &str = "heal";
pub const DAMAGED: &str = "damaged";
pub const HEALED: &str = "healed";
pub const DIED: &str = "died";

impl Health {
    pub fn is_dead(&self) -> bool {
        self.dead_for.is_some()
    }

    pub fn fraction(&self) -> f32 {
        (self.current / self.max.max(1e-3)).clamp(0.0, 1.0)
    }

    fn take(&mut self, tree: &mut Tree<'_>, node: Node, amount: f32) {
        if self.is_dead() || amount <= 0.0 {
            return;
        }
        if tree.is_in_group(node, "invulnerable") || self.since_hit < self.invulnerable_after_hit {
            return;
        }
        self.current = (self.current - amount).max(0.0);
        self.since_hit = 0.0;
        let _ = tree.report_health(node, self.current, self.max);
        let _ = tree.emit_with(node, DAMAGED, amount);
        let at = tree.global_position(node).unwrap_or_default() + Vec3::Y;
        if self.bleed {
            let root = tree.root();
            let blood = Blood {
                amount: (amount / 25.0).clamp(0.3, 3.0),
                splats: 2,
                ..Blood::along(Vec3::Y)
            };
            let _ = tree.spawn_blood(root, at, blood);
        }
        if self.current <= 0.0 {
            self.die(tree, node);
        }
    }

    fn die(&mut self, tree: &mut Tree<'_>, node: Node) {
        self.dead_for = Some(0.0);
        let _ = tree.remove_from_group(node, "damageable");
        let _ = tree.emit(node, DIED);
        if self.ragdoll_on_death {
            // The character's ragdoll: one under it, else made from its rigged model.
            let doll = tree
                .children(node)
                .into_iter()
                .find_map(|c| find_ragdoll(tree, c))
                .or_else(|| {
                    let model = tree
                        .children(node)
                        .into_iter()
                        .find(|&c| !tree.animation(c).clips().is_empty())?;
                    tree.humanoid_ragdoll(model, None).ok()
                });
            if let Some(doll) = doll {
                let _ = tree.go_limp(doll);
            }
        }
    }
}

fn find_ragdoll(tree: &Tree<'_>, node: Node) -> Option<Node> {
    if tree.class_name(node).as_deref() == Some("Ragdoll") {
        return Some(node);
    }
    tree.children(node)
        .into_iter()
        .find_map(|c| find_ragdoll(tree, c))
}

impl Script for Health {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        self.current = self.current.min(self.max);
        let _ = tree.add_to_group(node, "damageable");
        let _ = tree.report_health(node, self.current, self.max);
        // Damage and healing arrive as signals on this node.
        let _ = tree.connect(node, DAMAGE, node);
        let _ = tree.connect(node, HEAL, node);
    }

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        self.since_hit += delta;
        if let Some(dead_for) = &mut self.dead_for {
            *dead_for += delta;
            if self.remove_after > 0.0 && *dead_for >= self.remove_after {
                let _ = tree.queue_free(node);
            }
            return;
        }
        if self.regen_per_second > 0.0 && self.since_hit > self.regen_delay {
            let before = self.current;
            self.current = (self.current + self.regen_per_second * delta).min(self.max);
            // Whole points only: no call every frame.
            if self.current.floor() != before.floor() {
                let _ = tree.report_health(node, self.current, self.max);
            }
        }
    }

    fn on_signal(&mut self, tree: &mut Tree<'_>, node: Node, signal: &Signal<'_>) {
        let amount = signal.arg::<f32>().unwrap_or(0.0);
        if signal.is(DAMAGE) {
            self.take(tree, node, amount);
        } else if signal.is(HEAL) && !self.is_dead() {
            self.current = (self.current + amount).min(self.max);
            let _ = tree.report_health(node, self.current, self.max);
            let _ = tree.emit_with(node, HEALED, amount);
        }
    }
}
