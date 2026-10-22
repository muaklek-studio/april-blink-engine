use crate::{layers, ALTAR, ALTAR_SLOT, PLACED, RELICS, RELIC_PLACED};
use ab_script::prelude::*;
use std::f32::consts::FRAC_PI_2;

/// How far the player can reach to pick up or place things (m).
const REACH: f32 = 2.6;
/// Where a carried relic sits relative to the camera.
const HOLD_OFFSET: Vec3 = Vec3::new(0.38, -0.34, -1.0);
/// Carried relics are shown smaller so they do not cover the view.
const HOLD_SCALE: f32 = 0.55;

/// First-person player: walks and sprints (with stamina), looks with the mouse or right
/// stick, picks up relics and places them on the altar (`interact`).
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Player {
    pub walk_speed: f32,
    pub sprint_speed: f32,
    /// 0..=1. Sprinting drains it; it refills while not sprinting.
    pub stamina: f32,
    pub stick_look_speed: f32,
    exhausted: bool,
    yaw: f32,
    pitch: f32,
    carried: Option<Node>,
    /// Parent the carried relic came from (it goes back there when dropped).
    carried_from: Option<Node>,
    camera: Option<Node>,
    /// What `interact` would do now, for the HUD.
    prompt: String,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            walk_speed: 3.4,
            sprint_speed: 5.6,
            stamina: 1.0,
            stick_look_speed: 2.5,
            exhausted: false,
            yaw: 0.0,
            pitch: 0.0,
            carried: None,
            carried_from: None,
            camera: None,
            prompt: String::new(),
        }
    }
}

impl Player {
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn is_carrying(&self) -> bool {
        self.carried.is_some()
    }

    /// Turns the view from `eye` towards `target` (cutscenes, tests); applied on the next
    /// frame.
    pub fn look_at(&mut self, eye: Vec3, target: Vec3) {
        let to = target - eye;
        self.yaw = (-to.x).atan2(-to.z);
        self.pitch = (to.y / to.length().max(1e-4)).clamp(-1.0, 1.0).asin();
    }

    fn view(&self, tree: &Tree<'_>) -> Option<(Vec3, Vec3)> {
        let camera = tree.global_transform(self.camera?)?;
        Some((camera.translation, camera.forward()))
    }

    /// What the crosshair is on, within reach.
    fn aim(&self, tree: &Tree<'_>, me: Node) -> Option<Node> {
        let (origin, forward) = self.view(tree)?;
        let mut options = QueryOptions::new().excluding(me);
        if let Some(carried) = self.carried {
            options = options.excluding(carried);
        }
        tree.physics()
            .cast_ray(origin, forward, REACH, &options)
            .map(|hit| hit.node)
    }

    fn set_body(tree: &mut Tree<'_>, relic: Node, kind: BodyKind, layer: u32, mask: u32) {
        let mut body = tree.body(relic);
        let _ = body.set_kind(kind);
        let _ = body.set_collision_layer(layer);
        let _ = body.set_collision_mask(mask);
        let _ = body.set_linear_velocity(Vec3::ZERO);
        let _ = body.set_angular_velocity(Vec3::ZERO);
    }

    fn pick_up(&mut self, tree: &mut Tree<'_>, relic: Node) {
        let Some(camera) = self.camera else {
            return;
        };
        self.carried_from = tree.parent(relic);
        if tree.reparent(relic, camera, false).is_err() {
            return;
        }
        let _ = tree.set_transform(
            relic,
            Transform::from_translation(HOLD_OFFSET).with_scale(Vec3::splat(HOLD_SCALE)),
        );
        // Carried: follows the camera and touches nothing.
        Self::set_body(tree, relic, BodyKind::Kinematic, 0, 0);
        self.carried = Some(relic);
    }

    fn place(&mut self, tree: &mut Tree<'_>, me: Node, relic: Node) -> bool {
        let Some(slot) = tree
            .nodes_in_group(ALTAR_SLOT)
            .into_iter()
            .find(|&slot| tree.child_count(slot) == 0)
        else {
            return false;
        };
        if tree.reparent(relic, slot, false).is_err() {
            return false;
        }
        let _ = tree.set_transform(relic, Transform::IDENTITY);
        Self::set_body(
            tree,
            relic,
            BodyKind::Static,
            layers::RELICS,
            layers::LEVEL | layers::RELICS | layers::PLAYER,
        );
        let _ = tree.add_to_group(relic, PLACED);
        self.carried = None;
        let _ = tree.emit_with(me, RELIC_PLACED, relic);
        true
    }

    fn drop_carried(&mut self, tree: &mut Tree<'_>, relic: Node) {
        let parent = self
            .carried_from
            .filter(|&p| tree.is_valid(p))
            .unwrap_or_else(|| tree.root());
        if tree.reparent(relic, parent, true).is_err() {
            return;
        }
        let _ = tree.set_scale(relic, Vec3::ONE);
        Self::set_body(
            tree,
            relic,
            BodyKind::Rigid,
            layers::RELICS,
            layers::LEVEL | layers::RELICS | layers::PLAYER,
        );
        if let Some((_, forward)) = self.view(tree) {
            let _ = tree.body(relic).apply_impulse(forward * 1.5);
        }
        self.carried = None;
    }
}

impl Script for Player {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        self.camera = tree.get_node(node, "Shake/Camera");
        let forward = tree.rotation(node).unwrap_or_default() * Vec3::NEG_Z;
        self.yaw = (-forward.x).atan2(-forward.z);
    }

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        let captured = tree.engine().is_mouse_captured();
        let input = tree.input();
        let mut look = input.vector("look_left", "look_right", "look_down", "look_up")
            * self.stick_look_speed
            * delta;
        if captured {
            let mouse = input.look();
            look += Vec2::new(mouse.x, -mouse.y);
        }
        let interact = input.is_just_pressed("interact");
        self.yaw -= look.x;
        self.pitch = (self.pitch + look.y).clamp(-FRAC_PI_2 + 0.05, FRAC_PI_2 - 0.05);
        let _ = tree.set_rotation(node, Quat::from_rotation_y(self.yaw));
        if let Some(camera) = self.camera {
            let _ = tree.set_rotation(camera, Quat::from_rotation_x(self.pitch));
        }

        // Interaction: what is under the crosshair decides what `interact` does.
        let aimed = self.aim(tree, node);
        let on = |tree: &Tree<'_>, group: &str| aimed.is_some_and(|n| tree.is_in_group(n, group));
        let (on_relic, on_placed, on_altar) = (on(tree, RELICS), on(tree, PLACED), on(tree, ALTAR));
        self.prompt = match self.carried {
            None if on_relic && !on_placed => "E  หยิบเครื่องราง · Pick up the relic".into(),
            None => String::new(),
            Some(_) if on_altar => "E  วางบนแท่นบูชา · Place it on the altar".into(),
            Some(_) => "E  วางลง · Drop".into(),
        };
        // A click that captures the mouse is not an interaction.
        if !interact || !captured {
            return;
        }
        match (self.carried, aimed) {
            (None, Some(target)) if on_relic && !on_placed => self.pick_up(tree, target),
            (Some(relic), _) if on_altar => {
                if !self.place(tree, node, relic) {
                    self.drop_carried(tree, relic);
                }
            }
            (Some(relic), _) => self.drop_carried(tree, relic),
            _ => {}
        }
    }

    fn physics_process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        let input = tree.input();
        let direction_input = input.vector("move_left", "move_right", "move_back", "move_forward");
        let sprint = input.is_pressed("sprint");
        let moving = direction_input.length_squared() > 0.01;
        let sprinting = sprint && moving && !self.exhausted && self.stamina > 0.0;
        if sprinting {
            self.stamina = (self.stamina - delta * 0.3).max(0.0);
            self.exhausted = self.stamina <= 0.0;
        } else {
            self.stamina = (self.stamina + delta * 0.2).min(1.0);
            // Wait for a third of the bar before sprinting again.
            self.exhausted &= self.stamina < 0.33;
        }
        let speed = if sprinting {
            self.sprint_speed
        } else {
            self.walk_speed
        };
        let direction = Quat::from_rotation_y(self.yaw)
            * Vec3::new(direction_input.x, 0.0, -direction_input.y).clamp_length_max(1.0);
        let gravity = tree.physics().gravity().y;
        let mut character = tree.character(node);
        let fall = character.velocity().unwrap_or_default().y + gravity * delta;
        let _ = character.set_velocity(direction * speed + Vec3::Y * fall);
        let _ = character.move_and_slide(delta);
    }
}
