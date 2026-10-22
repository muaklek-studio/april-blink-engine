//! Enemy: wanders around where it starts, notices the player it can see (in range, in front,
//! nothing in between) or hear (close by), chases it (along the baked navigation when there
//! is some), attacks in reach, searches where it last saw it, and gives up after a while. Its
//! model's animations follow (idle, walk, run, `Attack`).
//!
//! Put it on a `CharacterBody3D` with a `Health` script (it stops when `died`), and the
//! model as a child. The player is the node in the `player` group (the controller templates
//! join it). Enemies join the `enemies` group (the `AutoAim` script looks there).

use ab_script::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EnemyState {
    #[default]
    Wander,
    Chase,
    Attack,
    Search,
    Dead,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EnemyAi {
    pub walk_speed: f32,
    pub run_speed: f32,
    /// Seeing: how far, and the half-angle of the view in degrees.
    pub sight_range: f32,
    pub sight_angle: f32,
    /// Noticing the player this close, seen or not.
    pub hearing_range: f32,
    pub attack_range: f32,
    pub attack_damage: f32,
    /// Seconds between attacks, and when in the attack the hit lands.
    pub attack_cooldown: f32,
    pub attack_hit_time: f32,
    /// Seconds spent searching after losing sight of the player.
    pub search_time: f32,
    /// How far from its start it wanders.
    pub wander_radius: f32,
    pub gravity: f32,
    pub state: EnemyState,
    #[serde(skip)]
    home: Option<Vec3>,
    #[serde(skip)]
    goal: Option<Vec3>,
    #[serde(skip)]
    path: Vec<Vec3>,
    #[serde(skip)]
    repath_in: f32,
    #[serde(skip)]
    last_seen: Option<Vec3>,
    #[serde(skip)]
    timer: f32,
    #[serde(skip)]
    attack_elapsed: Option<f32>,
    #[serde(skip)]
    model: Option<Node>,
    #[serde(skip)]
    rng: Option<Rng>,
}

impl Default for EnemyAi {
    fn default() -> Self {
        Self {
            walk_speed: 1.4,
            run_speed: 4.0,
            sight_range: 18.0,
            sight_angle: 70.0,
            hearing_range: 3.0,
            attack_range: 1.6,
            attack_damage: 10.0,
            attack_cooldown: 1.2,
            attack_hit_time: 0.3,
            search_time: 6.0,
            wander_radius: 6.0,
            gravity: 20.0,
            state: EnemyState::Wander,
            home: None,
            goal: None,
            path: Vec::new(),
            repath_in: 0.0,
            last_seen: None,
            timer: 0.0,
            attack_elapsed: None,
            model: None,
            rng: None,
        }
    }
}

impl EnemyAi {
    /// Whether the enemy at `node` notices `target` now.
    fn notices(&self, tree: &Tree<'_>, node: Node, target: Node) -> bool {
        let (Some(me), Some(at)) = (tree.global_transform(node), tree.global_position(target))
        else {
            return false;
        };
        let to = at - me.translation;
        let distance = to.length();
        if distance <= self.hearing_range {
            return true;
        }
        if distance > self.sight_range {
            return false;
        }
        let forward = Vec3::new(me.forward().x, 0.0, me.forward().z).normalize_or_zero();
        let flat = Vec3::new(to.x, 0.0, to.z).normalize_or_zero();
        if forward.dot(flat) < self.sight_angle.to_radians().cos() {
            return false;
        }
        let eyes = me.translation + Vec3::Y * 0.6;
        let options = QueryOptions::new().excluding(node).excluding(target);
        tree.physics()
            .has_line_of_sight(eyes, at + Vec3::Y * 0.5, &options)
    }

    /// Velocity on the ground towards `goal` (along the navigation path when there is one).
    fn steer(&mut self, tree: &Tree<'_>, node: Node, goal: Vec3, speed: f32, delta: f32) -> Vec3 {
        let Some(at) = tree.global_position(node) else {
            return Vec3::ZERO;
        };
        self.repath_in -= delta;
        if self.repath_in <= 0.0 || self.goal.is_none_or(|g| g.distance(goal) > 1.0) {
            self.repath_in = 0.5;
            self.goal = Some(goal);
            self.path = tree.find_path(at, goal).unwrap_or_else(|| vec![goal]);
        }
        while self
            .path
            .first()
            .is_some_and(|p| Vec2::new(p.x - at.x, p.z - at.z).length() < 0.4)
        {
            self.path.remove(0);
        }
        let Some(next) = self.path.first() else {
            return Vec3::ZERO;
        };
        Vec3::new(next.x - at.x, 0.0, next.z - at.z).normalize_or_zero() * speed
    }

    fn set_state(&mut self, state: EnemyState) {
        if self.state != state {
            self.state = state;
            self.timer = 0.0;
            self.path.clear();
            self.goal = None;
        }
    }
}

impl Script for EnemyAi {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        self.home = tree.global_position(node);
        let _ = tree.add_to_group(node, "enemies");
        let _ = tree.connect(node, "died", node);
        self.model = tree
            .children(node)
            .into_iter()
            .find(|&c| !tree.animation(c).clips().is_empty());
        if let Some(model) = self.model {
            // glTF models face +Z; the engine's forward is -Z.
            let _ = tree.set_rotation(model, Quat::from_rotation_y(std::f32::consts::PI));
        }
    }

    fn physics_process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        if self.state == EnemyState::Dead {
            return;
        }
        self.timer += delta;
        let player = tree.nodes_in_group("player").into_iter().next();
        let sees = player.is_some_and(|p| self.notices(tree, node, p));
        let player_at = player.and_then(|p| tree.global_position(p));
        if sees {
            self.last_seen = player_at;
        }
        let at = tree.global_position(node).unwrap_or_default();
        let mut wish = Vec3::ZERO;
        match self.state {
            EnemyState::Wander => {
                if sees {
                    self.set_state(EnemyState::Chase);
                } else {
                    let home = *self.home.get_or_insert(at);
                    let reached = self
                        .goal
                        .is_none_or(|g| Vec2::new(g.x - at.x, g.z - at.z).length() < 0.6);
                    if reached || self.timer > 8.0 {
                        let rng = self.rng.get_or_insert_with(Rng::from_entropy);
                        let angle = rng.range_f32(0.0, std::f32::consts::TAU);
                        let reach = rng.range_f32(1.0, self.wander_radius);
                        self.goal = Some(home + Vec3::new(angle.cos(), 0.0, angle.sin()) * reach);
                        self.timer = 0.0;
                        self.path.clear();
                    }
                    if let Some(goal) = self.goal {
                        wish = self.steer(tree, node, goal, self.walk_speed, delta);
                    }
                }
            }
            EnemyState::Chase => match (sees, player_at) {
                (true, Some(target)) if target.distance(at) <= self.attack_range => {
                    self.set_state(EnemyState::Attack);
                }
                (true, Some(target)) => {
                    wish = self.steer(tree, node, target, self.run_speed, delta)
                }
                _ => self.set_state(EnemyState::Search),
            },
            EnemyState::Attack => {
                let elapsed = self.attack_elapsed.get_or_insert(0.0);
                if *elapsed == 0.0 {
                    if let Some(model) = self.model {
                        let _ = tree.locomotion(model).play_action("Attack");
                    }
                }
                let before = *elapsed;
                *elapsed += delta;
                let now = *elapsed;
                if before < self.attack_hit_time && now >= self.attack_hit_time {
                    // The hit lands if the player is still in reach.
                    if let (Some(player), Some(target)) = (player, player_at) {
                        if target.distance(at) <= self.attack_range * 1.3 {
                            let _ = tree.deal_damage(Some(node), player, self.attack_damage);
                        }
                    }
                }
                if now >= self.attack_cooldown {
                    self.attack_elapsed = None;
                    self.set_state(EnemyState::Chase);
                }
                if let Some(target) = player_at {
                    let to = target - at;
                    face(tree, node, to, 10.0 * delta);
                }
            }
            EnemyState::Search => {
                if sees {
                    self.set_state(EnemyState::Chase);
                } else if self.timer > self.search_time {
                    self.set_state(EnemyState::Wander);
                } else if let Some(spot) = self.last_seen {
                    wish = self.steer(tree, node, spot, self.walk_speed, delta);
                }
            }
            EnemyState::Dead => {}
        }
        if wish.length_squared() > 0.01 {
            face(tree, node, wish, 8.0 * delta);
        }
        let mut velocity = tree.character(node).velocity().unwrap_or_default();
        velocity.x = wish.x;
        velocity.z = wish.z;
        velocity.y = if tree.character(node).is_on_floor() {
            -0.5
        } else {
            velocity.y - self.gravity * delta
        };
        let _ = tree.character(node).set_velocity(velocity);
        let _ = tree.move_and_slide(node, delta);
        if let Some(model) = self.model {
            let on_floor = tree.character(node).is_on_floor();
            let speed = Vec2::new(velocity.x, velocity.z).length();
            let _ = tree
                .locomotion(model)
                .update(speed, on_floor, velocity.y, false);
        }
    }

    fn on_signal(&mut self, tree: &mut Tree<'_>, node: Node, signal: &Signal<'_>) {
        if signal.is("died") {
            self.state = EnemyState::Dead;
            let _ = tree.remove_from_group(node, "enemies");
        }
    }
}

/// Turns `node` about Y towards `direction`, by at most `amount` (0..1 of the way).
fn face(tree: &mut Tree<'_>, node: Node, direction: Vec3, amount: f32) {
    if Vec2::new(direction.x, direction.z).length_squared() < 1e-6 {
        return;
    }
    let yaw = (-direction.x).atan2(-direction.z);
    let target = Quat::from_rotation_y(yaw);
    let current = tree.rotation(node).unwrap_or(Quat::IDENTITY);
    let _ = tree.set_rotation(node, current.slerp(target, amount.clamp(0.0, 1.0)));
}
