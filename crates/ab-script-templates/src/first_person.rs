//! First person controller: mouse and gamepad look, walk, sprint, crouch, jump (with coyote
//! time and buffering), climbing walls and pulling up onto ledges, and a camera that moves
//! like a head.
//!
//! Put it on a `CharacterBody3D` (a capsule about 1.8 m tall works). It builds its own camera
//! rig on start, so the scene needs nothing else:
//!
//! ```text
//! Player (CharacterBody3D + this script)   turns left/right
//!   Head (eye height)                       looks up/down
//!     Bob (head motion)
//!       Camera
//! ```
//!
//! Actions: `move_forward`, `move_back`, `move_left`, `move_right`, `jump`, `sprint`,
//! `crouch`, and optionally `look_left`, `look_right`, `look_up`, `look_down` for a gamepad's
//! right stick. Click the window to capture the mouse; Escape releases it (engine default).
//! Walls in the `climbable` group can be climbed: walk into one and press `jump`.
//!
//! The body joins the `player` group (enemies look for it) and adds the `aim_assist` signal's
//! turn (yaw, pitch) to its look (see the `AutoAim` script).

use ab_script::prelude::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct FirstPersonController {
    pub walk_speed: f32,
    pub sprint_speed: f32,
    pub crouch_speed: f32,
    pub acceleration: f32,
    pub air_acceleration: f32,
    pub jump_height: f32,
    pub gravity: f32,
    /// Eye height above the body's centre, standing and crouching.
    pub eye_height: f32,
    pub crouch_eye_height: f32,
    /// Radians per unit of mouse motion (the input map's sensitivity also applies).
    pub mouse_sensitivity: f32,
    /// Radians per second at full stick.
    pub stick_sensitivity: f32,
    /// Field of view, and the extra degrees while sprinting.
    pub fov: f32,
    pub sprint_fov_boost: f32,
    /// Head motion: 1 natural, 0 off.
    pub head_bob: f32,
    #[serde(skip)]
    rig: Option<Rig>,
    #[serde(skip)]
    pitch: f32,
    #[serde(skip)]
    since_ground: f32,
    #[serde(skip)]
    since_jump_press: f32,
    #[serde(skip)]
    eye: f32,
    /// Turn asked for by aim assist since the last frame (yaw, pitch).
    #[serde(skip)]
    assist: Vec2,
}

#[derive(Debug, Clone, Copy)]
struct Rig {
    head: Node,
    camera: Node,
}

impl Default for FirstPersonController {
    fn default() -> Self {
        Self {
            walk_speed: 4.0,
            sprint_speed: 7.0,
            crouch_speed: 2.0,
            acceleration: 40.0,
            air_acceleration: 8.0,
            jump_height: 1.2,
            gravity: 20.0,
            eye_height: 0.7,
            crouch_eye_height: 0.2,
            mouse_sensitivity: 1.0,
            stick_sensitivity: 3.0,
            fov: 75.0,
            sprint_fov_boost: 8.0,
            head_bob: 1.0,
            rig: None,
            pitch: 0.0,
            since_ground: f32::MAX,
            since_jump_press: f32::MAX,
            eye: 0.7,
            assist: Vec2::ZERO,
        }
    }
}

const COYOTE_TIME: f32 = 0.12;
const JUMP_BUFFER: f32 = 0.12;

impl FirstPersonController {
    fn build_rig(&mut self, tree: &mut Tree<'_>, node: Node) -> Option<Rig> {
        let head = tree.create_node3d("Head").ok()?;
        tree.add_child(node, head).ok()?;
        let _ = tree.set_position(head, Vec3::new(0.0, self.eye_height, 0.0));
        let bob = tree.create_node3d("Bob").ok()?;
        tree.add_child(head, bob).ok()?;
        let _ = tree.set_head_bob(bob, self.head_bob, None);
        let camera = tree
            .instantiate_toml(
                "[[node]]\nname = \"Camera\"\ntype = \"Camera3D\"\ncamera = { current = true }\n",
            )
            .ok()?;
        tree.add_child(bob, camera).ok()?;
        let _ = tree.camera(camera).set_fov(self.fov);
        self.eye = self.eye_height;
        Some(Rig { head, camera })
    }
}

impl Script for FirstPersonController {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        self.rig = self.build_rig(tree, node);
        let _ = tree.add_to_group(node, "player");
        let _ = tree.connect(node, "aim_assist", node);
    }

    fn on_signal(&mut self, _tree: &mut Tree<'_>, _node: Node, signal: &Signal<'_>) {
        if signal.is("aim_assist") {
            self.assist += signal.arg::<Vec2>().unwrap_or_default();
        }
    }

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        let Some(rig) = self.rig else {
            return;
        };
        if tree.input().is_just_pressed("jump") {
            self.since_jump_press = 0.0;
        } else {
            self.since_jump_press += delta;
        }
        // Look: the mouse while captured, the right stick always.
        let mut look = if tree.engine().is_mouse_captured() {
            tree.input().look() * self.mouse_sensitivity
        } else {
            Vec2::ZERO
        };
        if tree.input().has_action("look_right") {
            look += tree
                .input()
                .vector("look_left", "look_right", "look_down", "look_up")
                * Vec2::new(1.0, -1.0)
                * self.stick_sensitivity
                * delta;
        }
        let assist = std::mem::take(&mut self.assist);
        let _ = tree.rotate(node, Vec3::Y, -look.x + assist.x);
        self.pitch = (self.pitch - look.y + assist.y).clamp(-1.5, 1.5);
        let _ = tree.set_rotation(rig.head, Quat::from_rotation_x(self.pitch));

        // Crouch: lower the eyes smoothly.
        let crouching = tree.input().is_pressed("crouch");
        let target_eye = if crouching {
            self.crouch_eye_height
        } else {
            self.eye_height
        };
        self.eye += (target_eye - self.eye) * (1.0 - (-12.0 * delta).exp());
        let _ = tree.set_position(rig.head, Vec3::new(0.0, self.eye, 0.0));

        // Sprinting widens the view a little.
        let speed = tree
            .character(node)
            .velocity()
            .map_or(0.0, |v| Vec2::new(v.x, v.z).length());
        let sprinting = tree.input().is_pressed("sprint") && speed > self.walk_speed + 0.5;
        let fov_target = self.fov
            + if sprinting {
                self.sprint_fov_boost
            } else {
                0.0
            };
        let fov = tree.camera(rig.camera).fov().unwrap_or(self.fov);
        let _ = tree
            .camera(rig.camera)
            .set_fov(fov + (fov_target - fov) * (1.0 - (-8.0 * delta).exp()));
    }

    fn physics_process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        let input = tree
            .input()
            .vector("move_left", "move_right", "move_back", "move_forward");
        let jump = self.since_jump_press <= JUMP_BUFFER;

        // On a wall: the climbing system moves the body.
        if tree.climb(node).is_climbing() {
            let jump_off = jump && input.y < 0.5;
            if jump_off {
                self.since_jump_press = f32::MAX;
            }
            tree.climb(node).step(input, jump_off, delta);
            return;
        }
        // Walking into a climbable wall and jumping grabs it.
        if jump && input.y > 0.5 && tree.climb(node).grab() {
            self.since_jump_press = f32::MAX;
            return;
        }

        let on_floor = tree.character(node).is_on_floor();
        if on_floor {
            self.since_ground = 0.0;
        } else {
            self.since_ground += delta;
        }
        let mut velocity = tree.character(node).velocity().unwrap_or_default();
        let Some(body) = tree.global_transform(node) else {
            return;
        };
        let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
        let wish =
            (flat(body.forward()) * input.y + flat(body.right()) * input.x).clamp_length_max(1.0);
        let speed = if tree.input().is_pressed("crouch") {
            self.crouch_speed
        } else if tree.input().is_pressed("sprint") && input.y > 0.1 {
            self.sprint_speed
        } else {
            self.walk_speed
        };
        let rate = if on_floor {
            self.acceleration
        } else {
            self.air_acceleration
        };
        let horizontal = Vec3::new(velocity.x, 0.0, velocity.z);
        let change = (wish * speed - horizontal).clamp_length_max(rate * delta);
        velocity.x += change.x;
        velocity.z += change.z;

        if jump && self.since_ground <= COYOTE_TIME {
            velocity.y = (2.0 * self.gravity * self.jump_height).sqrt();
            self.since_jump_press = f32::MAX;
            self.since_ground = f32::MAX;
        } else if on_floor && velocity.y <= 0.0 {
            velocity.y = -0.5;
        } else {
            velocity.y -= self.gravity * delta;
        }
        let _ = tree.character(node).set_velocity(velocity);
        let _ = tree.move_and_slide(node, delta);
    }
}
