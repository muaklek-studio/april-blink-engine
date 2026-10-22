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

impl Script for Flicker {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        if self.base <= 0.0 {
            self.base = tree.light(node).energy().unwrap_or(1.0);
        }
    }

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        if self.rng.next_f32() < delta * 9.0 {
            self.target = self.rng.range_f32(0.6, 1.1);
        }
        self.current += (self.target - self.current) * (delta * 14.0).min(1.0);
        let _ = tree.light(node).set_energy(self.base * self.current);
    }
}
