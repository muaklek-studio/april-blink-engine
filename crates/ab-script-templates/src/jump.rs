//! Jump: a jump that feels good. Coyote time (jump just after running off a ledge), jump
//! buffering (press just before landing), a lower jump when the button is let go early, and
//! optional extra jumps in the air.
//!
//! Needs: a `CharacterBody3D` moved by another script (`Movement`, or your own) that calls
//! `move_and_slide` and applies gravity; action `jump`.

use ab_script::prelude::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Jump {
    /// Height of a full jump in metres (holding the button).
    pub height: f32,
    /// The gravity the moving script uses, in m/s², to work out the take-off speed.
    pub gravity: f32,
    /// Seconds after leaving the ground that a jump still works.
    pub coyote_time: f32,
    /// Seconds before landing that a press is remembered.
    pub buffer_time: f32,
    /// Letting go while rising keeps this much of the upward speed (lower jumps).
    pub release_cut: f32,
    /// Jumps allowed in the air (1 = double jump).
    pub air_jumps: u32,
    #[serde(skip)]
    since_ground: f32,
    #[serde(skip)]
    since_press: f32,
    #[serde(skip)]
    air_jumps_left: u32,
}

impl Default for Jump {
    fn default() -> Self {
        Self {
            height: 1.3,
            gravity: 20.0,
            coyote_time: 0.12,
            buffer_time: 0.12,
            release_cut: 0.45,
            air_jumps: 0,
            since_ground: f32::MAX,
            since_press: f32::MAX,
            air_jumps_left: 0,
        }
    }
}

/// Signal the node emits when it jumps (for sounds, animations, dust).
pub const JUMPED: &str = "jumped";

impl Script for Jump {
    serde_state!();

    fn process(&mut self, tree: &mut Tree<'_>, _node: Node, delta: f32) {
        // Presses are read every frame (they last one frame), used in the physics step.
        if tree.input().is_just_pressed("jump") {
            self.since_press = 0.0;
        } else {
            self.since_press += delta;
        }
    }

    fn physics_process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        let on_floor = tree.character(node).is_on_floor();
        let mut velocity = tree.character(node).velocity().unwrap_or_default();
        if on_floor {
            self.since_ground = 0.0;
            self.air_jumps_left = self.air_jumps;
        } else {
            self.since_ground += delta;
        }
        let wants = self.since_press <= self.buffer_time;
        let grounded = self.since_ground <= self.coyote_time;
        if wants && (grounded || self.air_jumps_left > 0) {
            if !grounded {
                self.air_jumps_left -= 1;
            }
            // v² = 2 g h
            velocity.y = (2.0 * self.gravity * self.height).sqrt();
            self.since_press = f32::MAX;
            self.since_ground = f32::MAX;
            let _ = tree.emit(node, JUMPED);
        } else if velocity.y > 0.0 && !tree.input().is_pressed("jump") && !on_floor {
            velocity.y *= self.release_cut.powf(delta * 10.0);
        }
        let _ = tree.character(node).set_velocity(velocity);
    }
}
