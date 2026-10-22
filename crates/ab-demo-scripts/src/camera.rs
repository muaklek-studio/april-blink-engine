use ab_script::prelude::*;
use std::f32::consts::FRAC_PI_2;

/// Free-flying camera driven by the `move_*`, `look_*`, `sprint` and `free_look` actions
/// (see `crates/ab-app/input.toml`). Tab captures the mouse for continuous look; Esc releases
/// it, or quits when it is not captured. The `push` action shoves the crate under the crosshair
/// (with sparks); `explode` sets off an explosion where the camera looks.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct FlyCamera {
    pub speed: f32,
    /// Radians per second at full stick deflection.
    pub stick_look_speed: f32,
    yaw: f32,
    pitch: f32,
}

impl Default for FlyCamera {
    fn default() -> Self {
        Self {
            speed: 4.0,
            stick_look_speed: 2.5,
            yaw: 0.0,
            pitch: 0.0,
        }
    }
}

impl Script for FlyCamera {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        let forward = tree.rotation(node).unwrap_or_default() * Vec3::NEG_Z;
        self.yaw = (-forward.x).atan2(-forward.z);
        self.pitch = forward.y.clamp(-1.0, 1.0).asin();
    }

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        if tree.input().is_just_pressed("toggle_mouse_capture") {
            let captured = tree.engine().is_mouse_captured();
            tree.engine().set_mouse_captured(!captured);
        }
        if tree.input().is_just_pressed("quit") {
            if tree.engine().is_mouse_captured() {
                tree.engine().set_mouse_captured(false);
            } else {
                tree.engine().quit();
            }
        }

        let captured = tree.engine().is_mouse_captured();
        let input = tree.input();
        let mut look = input.vector("look_left", "look_right", "look_down", "look_up")
            * self.stick_look_speed
            * delta;
        if captured || input.is_pressed("free_look") {
            // Mouse +Y is down; turning down lowers the pitch.
            let mouse = input.look();
            look += Vec2::new(mouse.x, -mouse.y);
        }
        self.yaw -= look.x;
        self.pitch = (self.pitch + look.y).clamp(-FRAC_PI_2 + 0.01, FRAC_PI_2 - 0.01);

        let push = input.is_just_pressed("push");
        let explode = input.is_just_pressed("explode");
        let planar = input.vector("move_left", "move_right", "move_back", "move_forward");
        let vertical = input.axis("move_down", "move_up");
        let speed = self.speed * if input.is_pressed("sprint") { 3.0 } else { 1.0 };

        let rotation = Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0);
        let _ = tree.set_rotation(node, rotation);
        // Horizontal movement follows the view's heading; vertical stays world-up.
        let horizontal = Quat::from_rotation_y(self.yaw) * Vec3::new(planar.x, 0.0, -planar.y);
        let offset = (horizontal + Vec3::Y * vertical) * speed * delta;
        if offset != Vec3::ZERO {
            let _ = tree.global_translate(node, offset);
        }
        // Clicks on the HUD are for the HUD.
        if push && !tree.is_pointer_over_ui() {
            shove(tree, node);
        }
        if explode {
            blast(tree, node);
        }
    }
}

/// The camera's position and view direction.
fn view(tree: &Tree<'_>, camera: Node) -> Option<(Vec3, Vec3)> {
    let view = tree.global_transform(camera)?;
    Some((view.translation, view.forward()))
}

/// Raycasts along the camera's view and kicks the rigid body it hits at the hit point.
fn shove(tree: &mut Tree<'_>, camera: Node) {
    const REACH: f32 = 12.0;
    const STRENGTH: f32 = 40.0;
    let Some((origin, forward)) = view(tree, camera) else {
        return;
    };
    let Some(hit) = tree
        .physics()
        .cast_ray(origin, forward, REACH, &QueryOptions::new())
    else {
        return;
    };
    // A standing mannequin: knock it over.
    let doll = tree.ragdoll_of(hit.node);
    if let Some(doll) = doll {
        let _ = tree.go_limp(doll);
    }
    let rigid = tree.body(hit.node).kind() == Some(BodyKind::Rigid) || doll.is_some();
    if rigid {
        let _ = tree
            .body(hit.node)
            .apply_impulse_at(forward * STRENGTH, hit.position);
        info!("shove {} at {:.1} m", hit.node, hit.distance);
    }
    // A few sparks where the hand hits.
    let sparks = ParticleEffect::preset("sparks")
        .with_amount(18)
        .with_direction(hit.normal, 60.0)
        .with_speed(1.5, 4.0);
    let root = tree.root();
    let _ = tree.spawn_effect(root, hit.position, &sparks);
}

/// An explosion where the camera looks (or 6 m ahead): fire, smoke, a flash, camera shake,
/// and a push for every rigid body nearby.
fn blast(tree: &mut Tree<'_>, camera: Node) {
    const RADIUS: f32 = 4.0;
    let Some((origin, forward)) = view(tree, camera) else {
        return;
    };
    let center = tree
        .physics()
        .cast_ray(origin, forward, 20.0, &QueryOptions::new())
        .map_or(origin + forward * 6.0, |hit| {
            hit.position + hit.normal * 0.3
        });
    let root = tree.root();
    let _ = tree.spawn_explosion(root, center, Explosion::default());
    let nearby = tree
        .physics()
        .intersect_sphere(center, RADIUS, &QueryOptions::new());
    for node in nearby {
        let Some(position) = tree.global_position(node) else {
            continue;
        };
        let doll = tree.ragdoll_of(node);
        if let Some(doll) = doll {
            let _ = tree.go_limp(doll);
        }
        let mut body = tree.body(node);
        // Ragdoll parts turn rigid at the next physics step; push them anyway.
        if body.kind() == Some(BodyKind::Rigid) || doll.is_some() {
            let away = position - center;
            let falloff = (1.0 - away.length() / RADIUS).max(0.0);
            let push = (away.normalize_or(Vec3::Y) + Vec3::Y * 0.6) * 60.0 * falloff;
            let _ = body.apply_impulse(push);
        }
    }
    info!("explosion at {center}");
}
