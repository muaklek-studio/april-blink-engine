//! Third person controller: an orbiting camera on a spring arm that pulls in when a wall is
//! in the way, movement relative to the camera, the character turning to face where it goes,
//! jump, sprint, dash, climbing, and its model's animations picked and blended from all of
//! that (idle, walk, run, jump, fall, climb).
//!
//! Put it on a `CharacterBody3D` whose child is the character's model (a rigged glTF, e.g.
//! `models/mannequin.gltf`, with its feet at the body's bottom). It makes its own camera.
//!
//! Actions: `move_forward`, `move_back`, `move_left`, `move_right`, `jump`, `sprint`,
//! `dash`, optionally `look_left`/`look_right`/`look_up`/`look_down` (right stick). Mouse
//! look works while the mouse is captured (click the window). Walls in the `climbable` group
//! can be climbed: run into one and press `jump`.
//!
//! The body joins the `player` group (enemies look for it) and adds the `aim_assist` signal's
//! turn (yaw, pitch) to the camera (see the `AutoAim` script).

use ab_script::prelude::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ThirdPersonController {
    pub walk_speed: f32,
    pub run_speed: f32,
    pub acceleration: f32,
    pub air_acceleration: f32,
    pub jump_height: f32,
    pub gravity: f32,
    /// Turning to face the movement, per second.
    pub turn_speed: f32,
    pub dash_speed: f32,
    pub dash_time: f32,
    pub dash_cooldown: f32,
    /// Camera: distance behind, height of the point it orbits, look sensitivities.
    pub camera_distance: f32,
    pub camera_height: f32,
    pub mouse_sensitivity: f32,
    pub stick_sensitivity: f32,
    /// How quickly the camera follows the character (per second).
    pub camera_follow: f32,
    /// glTF models face +Z, the engine's forward is -Z: turn the model around on start so it
    /// looks where the body goes.
    pub turn_model_around: bool,
    #[serde(skip)]
    rig: Option<CameraRig>,
    #[serde(skip)]
    model: Option<Node>,
    #[serde(skip)]
    yaw: f32,
    #[serde(skip)]
    pitch: f32,
    #[serde(skip)]
    arm: f32,
    #[serde(skip)]
    since_ground: f32,
    #[serde(skip)]
    since_jump_press: f32,
    #[serde(skip)]
    dash_left: f32,
    #[serde(skip)]
    dash_cooldown_left: f32,
    /// Turn asked for by aim assist since the last frame (yaw, pitch).
    #[serde(skip)]
    assist: Vec2,
}

#[derive(Debug, Clone, Copy)]
struct CameraRig {
    pivot: Node,
    camera: Node,
}

impl Default for ThirdPersonController {
    fn default() -> Self {
        Self {
            walk_speed: 2.0,
            run_speed: 5.5,
            acceleration: 25.0,
            air_acceleration: 5.0,
            jump_height: 1.2,
            gravity: 20.0,
            turn_speed: 10.0,
            dash_speed: 14.0,
            dash_time: 0.2,
            dash_cooldown: 0.7,
            camera_distance: 4.0,
            camera_height: 1.5,
            mouse_sensitivity: 1.0,
            stick_sensitivity: 3.0,
            camera_follow: 12.0,
            turn_model_around: true,
            rig: None,
            model: None,
            yaw: 0.0,
            pitch: -0.25,
            arm: 4.0,
            since_ground: f32::MAX,
            since_jump_press: f32::MAX,
            dash_left: 0.0,
            dash_cooldown_left: 0.0,
            assist: Vec2::ZERO,
        }
    }
}

const COYOTE_TIME: f32 = 0.12;
const JUMP_BUFFER: f32 = 0.15;

impl ThirdPersonController {
    /// A pivot that follows the character (not its child: the character turns on its own),
    /// with the camera on it.
    fn build_rig(&mut self, tree: &mut Tree<'_>, node: Node) -> Option<CameraRig> {
        let parent = tree.parent(node)?;
        let pivot = tree.create_node3d("CameraPivot").ok()?;
        tree.add_child(parent, pivot).ok()?;
        let camera = tree
            .instantiate_toml("[[node]]\nname = \"Camera\"\ntype = \"Camera3D\"\ncamera = { current = true, fov = 65.0 }\n")
            .ok()?;
        tree.add_child(pivot, camera).ok()?;
        let at = tree.global_position(node)?;
        let _ = tree.set_global_position(pivot, at + Vec3::Y * self.camera_height);
        self.yaw = tree.global_transform(node).map_or(0.0, |t| {
            let f = t.forward();
            (-f.x).atan2(-f.z)
        });
        self.arm = self.camera_distance;
        Some(CameraRig { pivot, camera })
    }

    fn update_camera(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        let Some(rig) = self.rig else {
            return;
        };
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
        self.yaw += assist.x - look.x;
        self.pitch = (self.pitch - look.y + assist.y).clamp(-1.2, 0.6);
        let rotation = Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch);
        // Follow the character smoothly.
        let target = tree.global_position(node).unwrap_or_default() + Vec3::Y * self.camera_height;
        let current = tree.global_position(rig.pivot).unwrap_or(target);
        let follow = 1.0 - (-self.camera_follow * delta).exp();
        let pivot_at = current.lerp(target, follow);
        let _ = tree.set_global_transform(
            rig.pivot,
            Transform::from_translation(pivot_at).with_rotation(rotation),
        );
        // Spring arm: pull in when something is between the character and the camera, ease
        // back out when it is clear.
        let back = rotation * Vec3::Z;
        let options = QueryOptions::new().excluding(node);
        let free = tree
            .physics()
            .cast_ray(pivot_at, back, self.camera_distance + 0.3, &options)
            .map_or(self.camera_distance, |hit| (hit.distance - 0.3).max(0.3));
        self.arm = if free < self.arm {
            free
        } else {
            self.arm + (free - self.arm) * (1.0 - (-4.0 * delta).exp())
        };
        let _ = tree.set_position(rig.camera, Vec3::new(0.0, 0.0, self.arm));
    }
}

impl Script for ThirdPersonController {
    serde_state!();

    fn on_signal(&mut self, _tree: &mut Tree<'_>, _node: Node, signal: &Signal<'_>) {
        if signal.is("aim_assist") {
            self.assist += signal.arg::<Vec2>().unwrap_or_default();
        }
    }

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        self.rig = self.build_rig(tree, node);
        let _ = tree.add_to_group(node, "player");
        let _ = tree.connect(node, "aim_assist", node);
        // The model is the child with animations.
        self.model = tree
            .children(node)
            .into_iter()
            .find(|&c| !tree.animation(c).clips().is_empty());
        if let Some(model) = self.model.filter(|_| self.turn_model_around) {
            let _ = tree.set_rotation(model, Quat::from_rotation_y(std::f32::consts::PI));
        }
    }

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        if tree.input().is_just_pressed("jump") {
            self.since_jump_press = 0.0;
        } else {
            self.since_jump_press += delta;
        }
        self.dash_cooldown_left = (self.dash_cooldown_left - delta).max(0.0);
        if tree.input().is_just_pressed("dash") && self.dash_cooldown_left <= 0.0 {
            self.dash_left = self.dash_time;
            self.dash_cooldown_left = self.dash_cooldown;
            let _ = tree.add_to_group(node, "invulnerable");
        }
        self.update_camera(tree, node, delta);
        // Animations follow the movement.
        if let Some(model) = self.model {
            let velocity = tree.character(node).velocity().unwrap_or_default();
            let climbing = tree.climb(node).is_climbing();
            let grounded = tree.character(node).is_on_floor() || climbing;
            let horizontal = Vec2::new(velocity.x, velocity.z).length();
            let vertical = if climbing {
                tree.input()
                    .vector("move_left", "move_right", "move_back", "move_forward")
                    .y
            } else {
                velocity.y
            };
            let _ = tree
                .locomotion(model)
                .update(horizontal, grounded, vertical, climbing);
        }
    }

    fn physics_process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        let input = tree
            .input()
            .vector("move_left", "move_right", "move_back", "move_forward");
        let jump = self.since_jump_press <= JUMP_BUFFER;
        if tree.climb(node).is_climbing() {
            let jump_off = jump && input.y < 0.5;
            if jump_off {
                self.since_jump_press = f32::MAX;
            }
            tree.climb(node).step(input, jump_off, delta);
            return;
        }
        // Camera-relative direction on the ground.
        let yaw = Quat::from_rotation_y(self.yaw);
        let wish = (yaw * Vec3::new(input.x, 0.0, -input.y)).clamp_length_max(1.0);
        if jump && wish.length_squared() > 0.25 {
            // Face the wall first, so the grab looks where the player pushes.
            face(tree, node, wish, 1.0);
            if tree.climb(node).grab() {
                self.since_jump_press = f32::MAX;
                return;
            }
        }

        let on_floor = tree.character(node).is_on_floor();
        if on_floor {
            self.since_ground = 0.0;
        } else {
            self.since_ground += delta;
        }
        let mut velocity = tree.character(node).velocity().unwrap_or_default();
        if self.dash_left > 0.0 {
            self.dash_left -= delta;
            let forward = tree
                .global_transform(node)
                .map_or(Vec3::NEG_Z, |t| t.forward());
            let direction = if wish.length_squared() > 0.01 {
                wish.normalize()
            } else {
                Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero()
            };
            velocity.x = direction.x * self.dash_speed;
            velocity.z = direction.z * self.dash_speed;
            if self.dash_left <= 0.0 {
                let _ = tree.remove_from_group(node, "invulnerable");
            }
        } else {
            let speed = if tree.input().is_pressed("sprint") {
                self.run_speed
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
        }
        if wish.length_squared() > 0.01 {
            face(tree, node, wish, self.turn_speed * delta);
        }
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

    fn exit_tree(&mut self, tree: &mut Tree<'_>, _node: Node) {
        if let Some(rig) = self.rig.take() {
            let _ = tree.queue_free(rig.pivot);
        }
    }
}

/// Turns `node` about Y towards `direction`, by at most `amount` (0..1 of the way).
fn face(tree: &mut Tree<'_>, node: Node, direction: Vec3, amount: f32) {
    let yaw = (-direction.x).atan2(-direction.z);
    let target = Quat::from_rotation_y(yaw);
    let current = tree.rotation(node).unwrap_or(Quat::IDENTITY);
    let _ = tree.set_rotation(node, current.slerp(target, amount.clamp(0.0, 1.0)));
}
