//! Movement: walk and run a `CharacterBody3D` with the move actions, relative to the camera,
//! with gravity. The simplest controller; add the `Jump` and `Dash` scripts to the same node
//! for more.
//!
//! Needs: a `CharacterBody3D` with a collision shape; actions `move_forward`, `move_back`,
//! `move_left`, `move_right` (and `sprint`). The current camera sets what "forward" means;
//! without one, the body's own -Z does.

use ab_script::prelude::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Movement {
    /// Walking speed in m/s.
    pub walk_speed: f32,
    /// Running speed (holding `sprint`).
    pub run_speed: f32,
    /// How fast the speed changes, in m/s²: high is snappy, low is slippery.
    pub acceleration: f32,
    /// Speed change in the air (less control than on the ground).
    pub air_acceleration: f32,
    /// Downward acceleration in m/s².
    pub gravity: f32,
    /// Turn the body to face where it moves (third person); off for first person.
    pub face_movement: bool,
    /// How quickly it turns to face the movement, per second.
    pub turn_speed: f32,
    /// The camera movement is relative to (found once, again if it goes away).
    #[serde(skip)]
    camera: Option<Node>,
}

impl Default for Movement {
    fn default() -> Self {
        Self {
            walk_speed: 3.0,
            run_speed: 6.0,
            acceleration: 30.0,
            air_acceleration: 6.0,
            gravity: 20.0,
            face_movement: true,
            turn_speed: 12.0,
            camera: None,
        }
    }
}

/// Group a node is in while something else moves it (a dash, a cutscene): movement then only
/// applies gravity.
const BUSY_GROUP: &str = "movement_busy";

impl Script for Movement {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        // Enemies look for the player here.
        let _ = tree.add_to_group(node, "player");
    }

    fn physics_process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        let input = tree
            .input()
            .vector("move_left", "move_right", "move_back", "move_forward");
        let sprint = tree.input().is_pressed("sprint");
        let on_floor = tree.character(node).is_on_floor();
        let mut velocity = tree.character(node).velocity().unwrap_or_default();

        if self.camera.is_none_or(|c| !tree.is_valid(c)) {
            self.camera = current_camera(tree);
        }
        if !tree.is_in_group(node, BUSY_GROUP) {
            let wish = camera_relative(tree, self.camera, node, input);
            let speed = if sprint {
                self.run_speed
            } else {
                self.walk_speed
            };
            let target = wish * speed;
            let rate = if on_floor {
                self.acceleration
            } else {
                self.air_acceleration
            };
            // Move the horizontal velocity towards the target by at most `rate * delta`.
            let horizontal = Vec3::new(velocity.x, 0.0, velocity.z);
            let change = (target - horizontal).clamp_length_max(rate * delta);
            velocity.x += change.x;
            velocity.z += change.z;
            if self.face_movement && wish.length_squared() > 0.01 {
                face(tree, node, wish, self.turn_speed * delta);
            }
        }
        if on_floor && velocity.y < 0.0 {
            velocity.y = -0.5; // stay pressed onto slopes
        } else {
            velocity.y -= self.gravity * delta;
        }
        let _ = tree.character(node).set_velocity(velocity);
        let _ = tree.move_and_slide(node, delta);
    }
}

/// The input direction on the ground, relative to `camera` (or the node).
fn camera_relative(tree: &Tree<'_>, camera: Option<Node>, node: Node, input: Vec2) -> Vec3 {
    let view = camera
        .and_then(|c| tree.global_transform(c))
        .or_else(|| tree.global_transform(node));
    let Some(view) = view else {
        return Vec3::ZERO;
    };
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
    (flat(view.forward()) * input.y + flat(view.right()) * input.x).clamp_length_max(1.0)
}

/// The camera the game renders from.
fn current_camera(tree: &mut Tree<'_>) -> Option<Node> {
    let root = tree.root();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if tree.class_name(n).as_deref() == Some("Camera3D") && tree.camera(n).is_current() {
            return Some(n);
        }
        stack.extend(tree.children(n));
    }
    None
}

/// Turns `node` about Y towards `direction`, by at most `amount` (0..1 of the way).
fn face(tree: &mut Tree<'_>, node: Node, direction: Vec3, amount: f32) {
    let yaw = (-direction.x).atan2(-direction.z);
    let target = Quat::from_rotation_y(yaw);
    let current = tree.rotation(node).unwrap_or(Quat::IDENTITY);
    let _ = tree.set_rotation(node, current.slerp(target, amount.clamp(0.0, 1.0)));
}
