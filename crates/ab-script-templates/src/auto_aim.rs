//! Auto aim: helps the player aim at enemies. While `aim` is held (or always, with
//! `always_on`), it picks the enemy nearest the centre of the view (in range, in a cone, in
//! sight) and turns the view towards it; `lock_on` keeps a target until it is lost.
//!
//! Put it on the player next to a controller template: it sends the controller the turn it
//! wants as the `aim_assist` signal (a `Vec2` of yaw and pitch in radians), which the
//! `FirstPersonController` and `ThirdPersonController` add to the player's own look. Targets
//! are the nodes in the `enemies` group (the `EnemyAi` script joins it).
//!
//! Signal on its node: `target_changed` (the new target, or nothing when lost).

use ab_script::prelude::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoAim {
    /// Help without holding `aim`.
    pub always_on: bool,
    pub range: f32,
    /// Half-angle of the cone targets are picked in, in degrees.
    pub cone: f32,
    /// How strongly the view is pulled, per second (1 slowly, 10 snaps).
    pub strength: f32,
    /// Aim at this height above the target's origin (its chest).
    pub aim_height: f32,
    #[serde(skip)]
    target: Option<Node>,
    #[serde(skip)]
    locked: bool,
    #[serde(skip)]
    camera: Option<Node>,
}

impl Default for AutoAim {
    fn default() -> Self {
        Self {
            always_on: false,
            range: 40.0,
            cone: 12.0,
            strength: 6.0,
            aim_height: 1.0,
            target: None,
            locked: false,
            camera: None,
        }
    }
}

pub const AIM_ASSIST: &str = "aim_assist";
pub const TARGET_CHANGED: &str = "target_changed";

impl AutoAim {
    /// Where the enemy should be aimed at, if it can be seen from `eye`.
    fn aim_point(&self, tree: &Tree<'_>, player: Node, eye: Vec3, enemy: Node) -> Option<Vec3> {
        let point = tree.global_position(enemy)? + Vec3::Y * self.aim_height;
        if point.distance(eye) > self.range {
            return None;
        }
        let options = QueryOptions::new().excluding(player).excluding(enemy);
        tree.physics()
            .has_line_of_sight(eye, point, &options)
            .then_some(point)
    }

    /// The enemy closest to the view's centre, inside the cone.
    fn pick(&self, tree: &Tree<'_>, player: Node, view: &Transform) -> Option<Node> {
        let limit = self.cone.to_radians().cos();
        tree.nodes_in_group("enemies")
            .into_iter()
            .filter_map(|enemy| {
                let point = self.aim_point(tree, player, view.translation, enemy)?;
                let alignment = (point - view.translation)
                    .normalize_or_zero()
                    .dot(view.forward());
                (alignment >= limit).then_some((enemy, alignment))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(enemy, _)| enemy)
    }
}

impl Script for AutoAim {
    serde_state!();

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        if self.camera.is_none_or(|c| !tree.is_valid(c)) {
            self.camera = find_camera(tree, node);
        }
        let Some(view) = self.camera.and_then(|c| tree.global_transform(c)) else {
            return;
        };
        if tree.input().is_just_pressed("lock_on") {
            self.locked = !self.locked && self.target.is_some();
        }
        let active = self.always_on || self.locked || tree.input().is_pressed("aim");
        let keep = self.target.filter(|&t| {
            tree.is_valid(t)
                && tree.is_in_group(t, "enemies")
                && self.aim_point(tree, node, view.translation, t).is_some()
        });
        let target = if !active {
            None
        } else if self.locked {
            keep
        } else {
            self.pick(tree, node, &view)
        };
        if target != self.target {
            self.target = target;
            if target.is_none() {
                self.locked = false;
            }
            let arg = target.map_or(Arg::Nil, Arg::from);
            let _ = tree.emit_with(node, TARGET_CHANGED, arg);
        }
        let Some(point) = target.and_then(|t| self.aim_point(tree, node, view.translation, t))
        else {
            return;
        };
        // The yaw and pitch between where the view looks and the target, a part of it now.
        let forward = view.forward();
        let wanted = (point - view.translation).normalize_or_zero();
        let yaw = |v: Vec3| (-v.x).atan2(-v.z);
        let pitch = |v: Vec3| v.y.clamp(-1.0, 1.0).asin();
        let mut delta_yaw = yaw(wanted) - yaw(forward);
        if delta_yaw > std::f32::consts::PI {
            delta_yaw -= std::f32::consts::TAU;
        } else if delta_yaw < -std::f32::consts::PI {
            delta_yaw += std::f32::consts::TAU;
        }
        let delta_pitch = pitch(wanted) - pitch(forward);
        let pull = (1.0 - (-self.strength * delta).exp()).clamp(0.0, 1.0);
        let _ = tree.emit_with(node, AIM_ASSIST, Vec2::new(delta_yaw, delta_pitch) * pull);
    }
}

/// The camera below `node`, else the first current camera in the scene.
fn find_camera(tree: &mut Tree<'_>, node: Node) -> Option<Node> {
    for start in [node, tree.root()] {
        let mut stack = vec![start];
        while let Some(n) = stack.pop() {
            if tree.class_name(n).as_deref() == Some("Camera3D")
                && (start == node || tree.camera(n).is_current())
            {
                return Some(n);
            }
            stack.extend(tree.children(n));
        }
    }
    None
}
