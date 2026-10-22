//! Helpers that need no engine: random numbers, path following.

use crate::math::Vec3;

/// A small, fast random number generator (SplitMix64). Not for cryptography.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// The same seed gives the same sequence (replays, tests).
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Seeded from the clock: different every run.
    pub fn from_entropy() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        // Mix in an address so two generators made in the same instant differ.
        let local = 0_u8;
        Self::new(nanos ^ (std::ptr::addr_of!(local) as u64).rotate_left(32))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / (1u32 << 24) as f32)
    }

    /// Uniform in `[min, max)`.
    pub fn range_f32(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.next_f32()
    }

    /// Uniform in `[min, max)`; `min` if the range is empty.
    pub fn range_u32(&mut self, min: u32, max: u32) -> u32 {
        if max <= min {
            return min;
        }
        min + (self.next_u64() % u64::from(max - min)) as u32
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f32) -> bool {
        self.next_f32() < p
    }
}

impl Default for Rng {
    fn default() -> Self {
        Self::from_entropy()
    }
}

/// Walks an agent along waypoints from [`Tree::find_path`](crate::Tree::find_path).
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

    /// Waypoints not reached yet.
    pub fn remaining(&self) -> &[Vec3] {
        &self.waypoints[self.next.min(self.waypoints.len())..]
    }

    /// Horizontal unit direction from `position` towards the current waypoint (skipping the
    /// ones already reached), or `None` at the end of the path.
    pub fn direction(&mut self, position: Vec3) -> Option<Vec3> {
        while let Some(&target) = self.waypoints.get(self.next) {
            let offset = Vec3::new(target.x - position.x, 0.0, target.z - position.z);
            if offset.length() > self.tolerance {
                return Some(offset.normalize());
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
    fn rng_is_reproducible_and_in_range() {
        let (mut a, mut b) = (Rng::new(42), Rng::new(42));
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        for _ in 0..1000 {
            let f = a.range_f32(2.0, 3.0);
            assert!((2.0..3.0).contains(&f));
            assert!((5..9).contains(&a.range_u32(5, 9)));
        }
        assert_eq!(a.range_u32(4, 4), 4);
        assert_ne!(Rng::from_entropy().next_u64(), Rng::new(0).next_u64());
    }

    #[test]
    fn follows_waypoints_in_order() {
        let mut follower = PathFollower::new(0.1);
        assert!(follower.direction(Vec3::ZERO).is_none());
        follower.set_path(vec![Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 5.0, 2.0)]);
        assert_eq!(follower.direction(Vec3::ZERO), Some(Vec3::X));
        let towards_second = follower.direction(Vec3::new(0.95, 3.0, 0.0)).unwrap();
        assert!(
            towards_second.abs_diff_eq(Vec3::Z, 0.05),
            "{towards_second}"
        );
        assert_eq!(follower.remaining().len(), 1);
        assert!(follower.direction(Vec3::new(1.0, 0.0, 1.95)).is_none());
        assert!(follower.is_finished());
    }
}
