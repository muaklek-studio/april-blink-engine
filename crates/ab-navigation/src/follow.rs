use ab_core::Vec3;

/// Walks an agent along waypoints from [`NavGrid::find_path`](crate::NavGrid::find_path).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PathFollower {
    waypoints: Vec<Vec3>,
    next: usize,
    /// Distance (on XZ) at which a waypoint counts as reached.
    pub tolerance: f32,
}

impl PathFollower {
    pub fn new(tolerance: f32) -> Self {
        Self {
            waypoints: Vec::new(),
            next: 0,
            tolerance,
        }
    }

    pub fn set_path(&mut self, waypoints: Vec<Vec3>) {
        self.waypoints = waypoints;
        self.next = 0;
    }

    pub fn clear(&mut self) {
        self.waypoints.clear();
        self.next = 0;
    }

    pub fn is_finished(&self) -> bool {
        self.next >= self.waypoints.len()
    }

    /// Remaining waypoints.
    pub fn remaining(&self) -> &[Vec3] {
        &self.waypoints[self.next.min(self.waypoints.len())..]
    }

    /// Horizontal unit direction from `position` towards the current waypoint (skipping
    /// the ones already reached), or `None` at the end of the path.
    pub fn direction(&mut self, position: Vec3) -> Option<Vec3> {
        while let Some(&target) = self.waypoints.get(self.next) {
            let offset = Vec3::new(target.x - position.x, 0.0, target.z - position.z);
            // Standing exactly on a waypoint counts as reached even with a negative
            // tolerance: a zero offset has no direction.
            if offset.length() > self.tolerance {
                if let Some(direction) = offset.try_normalize() {
                    return Some(direction);
                }
            }
            self.next += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_waypoints_in_order() {
        let mut follower = PathFollower::new(0.1);
        assert!(follower.direction(Vec3::ZERO).is_none());
        follower.set_path(vec![Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 5.0, 2.0)]);
        assert_eq!(follower.direction(Vec3::ZERO), Some(Vec3::X));
        // Reached the first one (height ignored): heads for the second.
        let towards_second = follower.direction(Vec3::new(0.95, 3.0, 0.0)).unwrap();
        assert!(
            towards_second.abs_diff_eq(Vec3::Z, 0.05),
            "{towards_second}"
        );
        assert_eq!(follower.remaining().len(), 1);
        assert!(follower.direction(Vec3::new(1.0, 0.0, 1.95)).is_none());
        assert!(follower.is_finished());
    }

    #[test]
    fn standing_on_a_waypoint_never_gives_nan() {
        for tolerance in [-1.0, 0.0] {
            let mut follower = PathFollower::new(tolerance);
            follower.set_path(vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)]);
            assert_eq!(follower.direction(Vec3::ZERO), Some(Vec3::X), "{tolerance}");
            assert_eq!(follower.direction(Vec3::new(2.0, 1.0, 0.0)), None);
        }
    }
}
