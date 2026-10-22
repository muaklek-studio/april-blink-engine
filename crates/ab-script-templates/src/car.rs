//! Car: an arcade car on a `RigidBody3D`. Each wheel is a ray with a spring and a damper
//! (suspension), tyres grip sideways and let go under the handbrake (drifts), the engine
//! pushes on the driven wheels, and a chase camera follows. Flipped over, `jump` held for a
//! second puts it back on its wheels.
//!
//! Put it on a `RigidBody3D` with a box shape for the body (about 1.8 × 0.6 × 4 m, mass
//! around 1200 kg); the wheels are only rays, so nothing else is needed. Child nodes named
//! `WheelFL`, `WheelFR`, `WheelRL`, `WheelRR` (meshes) are moved and spun with the wheels.
//!
//! Actions: `move_forward` (throttle), `move_back` (brake, then reverse), `move_left`,
//! `move_right` (steering), `jump` (handbrake).

use ab_script::prelude::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Car {
    /// Wheel mounts in the body's space (front is -Z): half the track width, height of the
    /// mounts, and how far the axles are from the centre.
    pub half_track: f32,
    pub mount_height: f32,
    pub front_axle: f32,
    pub rear_axle: f32,
    pub wheel_radius: f32,
    /// Suspension: length at rest, stiffness (N/m per wheel), damping (N·s/m per wheel).
    pub suspension_length: f32,
    pub stiffness: f32,
    pub damping: f32,
    /// Engine force (N), braking force (N), top speed (m/s).
    pub engine_force: f32,
    pub brake_force: f32,
    pub top_speed: f32,
    /// Steering lock in degrees, less at speed.
    pub max_steer: f32,
    /// Sideways grip (0..1 of the slide removed per step), and with the handbrake on.
    pub grip: f32,
    pub drift_grip: f32,
    pub rear_wheel_drive: bool,
    /// Chase camera: distance behind, height, how fast it follows.
    pub camera_distance: f32,
    pub camera_height: f32,
    pub camera_follow: f32,
    #[serde(skip)]
    steer: f32,
    #[serde(skip)]
    wheels: [Option<Node>; 4],
    #[serde(skip)]
    spin: f32,
    #[serde(skip)]
    camera: Option<Node>,
    #[serde(skip)]
    flipped_for: f32,
}

impl Default for Car {
    fn default() -> Self {
        Self {
            half_track: 0.8,
            mount_height: -0.1,
            front_axle: 1.3,
            rear_axle: 1.3,
            wheel_radius: 0.35,
            suspension_length: 0.35,
            stiffness: 30000.0,
            damping: 3500.0,
            engine_force: 7000.0,
            brake_force: 9000.0,
            top_speed: 32.0,
            max_steer: 32.0,
            grip: 0.85,
            drift_grip: 0.25,
            rear_wheel_drive: true,
            camera_distance: 7.0,
            camera_height: 2.6,
            camera_follow: 5.0,
            steer: 0.0,
            wheels: [None; 4],
            spin: 0.0,
            camera: None,
            flipped_for: 0.0,
        }
    }
}

const WHEEL_NAMES: [&str; 4] = ["WheelFL", "WheelFR", "WheelRL", "WheelRR"];

impl Car {
    /// Wheel mounts in the body's space: front left, front right, rear left, rear right.
    fn mounts(&self) -> [Vec3; 4] {
        let (x, y) = (self.half_track, self.mount_height);
        [
            Vec3::new(-x, y, -self.front_axle),
            Vec3::new(x, y, -self.front_axle),
            Vec3::new(-x, y, self.rear_axle),
            Vec3::new(x, y, self.rear_axle),
        ]
    }

    fn follow_camera(&mut self, tree: &mut Tree<'_>, body: &Transform, delta: f32) {
        let Some(camera) = self.camera else {
            return;
        };
        let back = Vec3::new(-body.forward().x, 0.0, -body.forward().z).normalize_or(Vec3::Z);
        let wanted = body.translation + back * self.camera_distance + Vec3::Y * self.camera_height;
        let at = tree.global_position(camera).unwrap_or(wanted);
        let follow = 1.0 - (-self.camera_follow * delta).exp();
        let position = at.lerp(wanted, follow);
        let _ = tree.set_global_position(camera, position);
        let _ = tree.look_at(camera, body.translation + Vec3::Y, Vec3::Y);
    }
}

impl Script for Car {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        for (slot, name) in self.wheels.iter_mut().zip(WHEEL_NAMES) {
            *slot = tree.find_child(node, name);
        }
        // A chase camera beside the car (not under it: it should not roll with it).
        if let Some(parent) = tree.parent(node) {
            let camera = tree.instantiate_toml(
                "[[node]]\nname = \"CarCamera\"\ntype = \"Camera3D\"\ncamera = { current = true, fov = 70.0 }\n",
            );
            if let Ok(camera) = camera {
                let _ = tree.add_child(parent, camera);
                self.camera = Some(camera);
            }
        }
        let _ = tree.add_to_group(node, "player");
    }

    fn physics_process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        let Some(body) = tree.global_transform(node) else {
            return;
        };
        let mass = tree.body(node).mass().unwrap_or(1200.0).max(1.0);
        let linear = tree.body(node).linear_velocity().unwrap_or_default();
        let angular = tree.body(node).angular_velocity().unwrap_or_default();
        let throttle = tree.input().axis("move_back", "move_forward");
        let steer_input = tree.input().axis("move_right", "move_left");
        let handbrake = tree.input().is_pressed("jump");
        let up = body.up();
        let forward = body.forward();
        let speed = linear.dot(forward);

        // Steering eases in and is gentler at speed.
        let lock = self.max_steer.to_radians() / (1.0 + speed.abs() * 0.05);
        self.steer += (steer_input * lock - self.steer) * (1.0 - (-8.0 * delta).exp());

        let options = QueryOptions::new().excluding(node);
        let reach = self.suspension_length + self.wheel_radius;
        let mut grounded = 0;
        for (i, mount) in self.mounts().into_iter().enumerate() {
            let front = i < 2;
            let origin = body.transform_point(mount);
            let wheel_forward = if front {
                Quat::from_axis_angle(up, self.steer) * forward
            } else {
                forward
            };
            let wheel_right = wheel_forward.cross(up).normalize_or_zero();
            let hit = tree.physics().cast_ray(origin, -up, reach, &options);
            // Place the wheel mesh at its suspension's length.
            let extension = hit.map_or(self.suspension_length, |h| h.distance - self.wheel_radius);
            if let Some(wheel) = self.wheels[i] {
                let local = mount - Vec3::Y * extension.max(0.0);
                let roll = Quat::from_rotation_x(-self.spin);
                let yaw = Quat::from_rotation_y(if front { self.steer } else { 0.0 });
                let _ = tree.set_position(wheel, local);
                let _ = tree.set_rotation(wheel, yaw * roll);
            }
            let Some(hit) = hit else {
                continue;
            };
            grounded += 1;
            // Velocity of the body at the wheel.
            let point_velocity = linear + angular.cross(origin - body.translation);
            // Suspension: spring by compression, damper by speed along the axis.
            let compression = self.suspension_length - (hit.distance - self.wheel_radius);
            let spring = self.stiffness * compression - self.damping * point_velocity.dot(up);
            let mut force = up * spring.max(0.0);
            // Tyres: cancel part of the sideways slide (less with the handbrake on the rear).
            let grip = if handbrake && !front {
                self.drift_grip
            } else {
                self.grip
            };
            let slide = point_velocity.dot(wheel_right);
            force -= wheel_right * slide * grip * mass / 4.0 / delta;
            // Engine and brakes along the wheel.
            let driven = if self.rear_wheel_drive { !front } else { front };
            let along = point_velocity.dot(wheel_forward);
            if throttle > 0.0 || (throttle < 0.0 && along < 0.5) {
                if driven && along.abs() < self.top_speed {
                    force += wheel_forward * throttle * self.engine_force / 2.0;
                }
            } else if throttle < 0.0 {
                force -= wheel_forward * along.signum() * self.brake_force / 4.0;
            }
            if handbrake && !front {
                force -= wheel_forward * along.signum() * self.brake_force / 4.0;
            }
            let _ = tree
                .body(node)
                .apply_impulse_at(force * delta, hit.position);
        }
        // Wheels spin with the ground speed.
        self.spin = (self.spin + speed / self.wheel_radius * delta) % std::f32::consts::TAU;

        // On its roof or side: hold the handbrake to flip it back.
        if grounded == 0 && up.y < 0.3 && handbrake {
            self.flipped_for += delta;
            if self.flipped_for > 1.0 {
                let yaw = (-forward.x).atan2(-forward.z);
                let upright = Transform::from_translation(body.translation + Vec3::Y * 1.5)
                    .with_rotation(Quat::from_rotation_y(yaw));
                let _ = tree.set_global_transform(node, upright);
                let _ = tree.body(node).set_linear_velocity(Vec3::ZERO);
                let _ = tree.body(node).set_angular_velocity(Vec3::ZERO);
                self.flipped_for = 0.0;
            }
        } else {
            self.flipped_for = 0.0;
        }
        self.follow_camera(tree, &body, delta);
    }

    fn exit_tree(&mut self, tree: &mut Tree<'_>, _node: Node) {
        if let Some(camera) = self.camera.take() {
            let _ = tree.queue_free(camera);
        }
    }
}
