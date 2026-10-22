use ab_script::prelude::*;

/// Hunts the player along the navigation grid. It waits `grace` seconds first, re-plans its
/// path a few times per second, and floats with a slow bob.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Ghost {
    pub speed: f32,
    /// Seconds before the hunt starts.
    pub grace: f32,
    /// Seconds between path updates.
    pub repath_interval: f32,
    target: Option<Node>,
    time: f32,
    #[serde(skip, default = "follower")]
    follower: PathFollower,
    #[serde(skip)]
    repath_in: f32,
}

fn follower() -> PathFollower {
    PathFollower::new(0.2)
}

impl Default for Ghost {
    fn default() -> Self {
        Self {
            speed: 2.3,
            grace: 8.0,
            repath_interval: 0.4,
            target: None,
            time: 0.0,
            follower: follower(),
            repath_in: 0.0,
        }
    }
}

impl Ghost {
    pub fn is_hunting(&self) -> bool {
        self.grace <= 0.0
    }

    /// Waypoints it still has to cover along its current path (debugging, tests).
    pub fn path_len(&self) -> usize {
        self.follower.remaining().len()
    }
}

impl Script for Ghost {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        self.target = tree.get_node(node, "../Player");
    }

    fn physics_process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        self.time += delta;
        if let Some(body) = tree.get_node(node, "Body") {
            let _ = tree.set_position(body, Vec3::Y * (self.time * 2.2).sin() * 0.12);
        }
        if self.grace > 0.0 {
            self.grace -= delta;
            return;
        }
        let (Some(target), Some(position)) = (
            self.target.and_then(|t| tree.global_position(t)),
            tree.global_position(node),
        ) else {
            return;
        };

        self.repath_in -= delta;
        if self.repath_in <= 0.0 {
            self.repath_in = self.repath_interval;
            // No grid yet or no path: head straight for the player.
            let path = tree.find_path(position, target);
            self.follower.set_path(path.unwrap_or_else(|| vec![target]));
        }
        let direction = self.follower.direction(position).unwrap_or_else(|| {
            Vec3::new(target.x - position.x, 0.0, target.z - position.z).normalize_or_zero()
        });
        if direction != Vec3::ZERO {
            let _ = tree.set_rotation(
                node,
                Quat::from_rotation_y((-direction.x).atan2(-direction.z)),
            );
        }
        let _ = tree.character(node).set_velocity(direction * self.speed);
        let _ = tree.move_and_slide(node, delta);
    }
}
