//! Dash: a quick burst in the moving direction (or straight ahead), with a cooldown, a
//! moment of invulnerability and a trail of dust.
//!
//! Needs: a `CharacterBody3D`; action `dash` (and the move actions for the direction).
//! While dashing the node is in the `movement_busy` group, so the `Movement` script lets it
//! fly, and in the `invulnerable` group, so the `Health` script ignores damage.

use ab_script::prelude::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Dash {
    /// Speed of the dash in m/s.
    pub speed: f32,
    /// How long it lasts, in seconds.
    pub duration: f32,
    /// Seconds before the next dash.
    pub cooldown: f32,
    /// Untouchable while dashing.
    pub invulnerable: bool,
    #[serde(skip)]
    time_left: f32,
    #[serde(skip)]
    cooldown_left: f32,
    #[serde(skip)]
    direction: Vec3,
}

impl Default for Dash {
    fn default() -> Self {
        Self {
            speed: 16.0,
            duration: 0.18,
            cooldown: 0.6,
            invulnerable: true,
            time_left: 0.0,
            cooldown_left: 0.0,
            direction: Vec3::ZERO,
        }
    }
}

const BUSY_GROUP: &str = "movement_busy";
const INVULNERABLE_GROUP: &str = "invulnerable";
/// Signal the node emits when a dash starts.
pub const DASHED: &str = "dashed";

impl Script for Dash {
    serde_state!();

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        self.cooldown_left = (self.cooldown_left - delta).max(0.0);
        if self.time_left > 0.0 || self.cooldown_left > 0.0 || !tree.input().is_just_pressed("dash")
        {
            return;
        }
        // Where the stick or keys point, relative to the body; straight ahead otherwise.
        let input = tree
            .input()
            .vector("move_left", "move_right", "move_back", "move_forward");
        let Some(body) = tree.global_transform(node) else {
            return;
        };
        let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
        let wish = flat(body.forward()) * input.y + flat(body.right()) * input.x;
        self.direction = if wish.length_squared() > 0.01 {
            wish.normalize()
        } else {
            flat(body.forward())
        };
        self.time_left = self.duration;
        self.cooldown_left = self.cooldown + self.duration;
        let _ = tree.add_to_group(node, BUSY_GROUP);
        if self.invulnerable {
            let _ = tree.add_to_group(node, INVULNERABLE_GROUP);
        }
        let _ = tree.emit(node, DASHED);
    }

    fn physics_process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        if self.time_left <= 0.0 {
            return;
        }
        self.time_left -= delta;
        let mut velocity = tree.character(node).velocity().unwrap_or_default();
        velocity.x = self.direction.x * self.speed;
        velocity.z = self.direction.z * self.speed;
        let _ = tree.character(node).set_velocity(velocity);
        // Dust behind.
        if let Some(at) = tree.global_position(node) {
            let root = tree.root();
            let dust = ParticleEffect::preset("dust")
                .with_amount(3)
                .with_lifetime(0.4)
                .with_size(0.25, 0.6)
                .with_direction(-self.direction, 30.0)
                .with_speed(0.5, 1.5);
            let _ = tree.spawn_effect(root, at - Vec3::Y * 0.6, &dust);
        }
        if self.time_left <= 0.0 {
            let _ = tree.remove_from_group(node, BUSY_GROUP);
            let _ = tree.remove_from_group(node, INVULNERABLE_GROUP);
            // Keep a little of the speed.
            velocity.x *= 0.3;
            velocity.z *= 0.3;
            let _ = tree.character(node).set_velocity(velocity);
        }
    }
}
