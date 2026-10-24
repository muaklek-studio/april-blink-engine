use ab_script::prelude::*;

/// Makes an omni light flicker like a candle around its starting energy.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Flicker {
    base: f32,
    current: f32,
    target: f32,
    #[serde(skip)]
    rng: Rng,
}

impl Default for Flicker {
    fn default() -> Self {
        Self {
            base: 0.0,
            current: 1.0,
            target: 1.0,
            rng: Rng::from_entropy(),
        }
    }
}

impl Flicker {
    /// Advances the flicker by `delta` seconds; returns the light's new energy.
    fn step(&mut self, delta: f32) -> f32 {
        if self.rng.next_f32() < delta * 9.0 {
            self.target = self.rng.range_f32(0.6, 1.1);
        }
        self.current += (self.target - self.current) * (delta * 14.0).min(1.0);
        self.base * self.current
    }
}

impl Script for Flicker {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        if self.base <= 0.0 {
            self.base = tree.light(node).energy().unwrap_or(1.0);
        }
    }

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        let energy = self.step(delta);
        let _ = tree.light(node).set_energy(energy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flicker(seed: u64) -> Flicker {
        Flicker {
            base: 2.0,
            rng: Rng::new(seed),
            ..Flicker::default()
        }
    }

    #[test]
    fn energy_stays_around_the_base() {
        let mut light = flicker(7);
        for frame in 0..2000 {
            let delta = if frame % 3 == 0 {
                1.0 / 30.0
            } else {
                1.0 / 144.0
            };
            let energy = light.step(delta);
            assert!((1.2..=2.2).contains(&energy), "{energy} at frame {frame}");
        }
    }

    #[test]
    fn it_actually_flickers() {
        let mut light = flicker(3);
        let energies: Vec<f32> = (0..600).map(|_| light.step(1.0 / 60.0)).collect();
        let (min, max) = energies
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &e| (lo.min(e), hi.max(e)));
        assert!(max - min > 0.2, "{min}..{max}");
    }

    #[test]
    fn a_long_frame_reaches_the_target() {
        let mut light = flicker(1);
        light.target = 0.7;
        // A frame long enough to pick a new target always lands exactly on it.
        let energy = light.step(0.5);
        assert_eq!(energy, light.base * light.target);
    }

    #[test]
    fn same_seed_same_flicker() {
        let (mut a, mut b) = (flicker(42), flicker(42));
        for _ in 0..100 {
            assert_eq!(a.step(0.016), b.step(0.016));
        }
    }
}
