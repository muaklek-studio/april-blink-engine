use ab_script::prelude::*;

/// Spins its node around Y.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Spinner {
    /// Radians per second.
    pub speed: f32,
}

impl Default for Spinner {
    fn default() -> Self {
        Self { speed: 0.8 }
    }
}

impl Script for Spinner {
    serde_state!();

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        let _ = tree.rotate(node, Vec3::Y, self.speed * delta);
    }
}

/// Randomly modulates an omni light's energy like a candle or failing bulb. The base energy
/// is the light's energy when the script starts.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Flicker {
    pub base: f32,
    target: f32,
    current: f32,
    #[serde(skip, default = "seeded")]
    rng: Rng,
}

fn seeded() -> Rng {
    Rng::new(42)
}

impl Default for Flicker {
    fn default() -> Self {
        Self {
            base: 0.0,
            target: 1.0,
            current: 1.0,
            rng: seeded(),
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
        if self.rng.next_f32() < delta * 12.0 {
            self.target = self.rng.range_f32(0.55, 1.15);
        }
        self.current += (self.target - self.current) * (delta * 18.0).min(1.0);
        let _ = tree.light(node).set_energy(self.base * self.current);
    }
}

/// Logs frame and render statistics once per second.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Stats {
    frames: u32,
    accumulated: f32,
}

impl Script for Stats {
    serde_state!();

    fn process(&mut self, tree: &mut Tree<'_>, _node: Node, delta: f32) {
        self.frames += 1;
        self.accumulated += delta;
        if self.accumulated < 1.0 {
            return;
        }
        let fps = self.frames as f32 / self.accumulated;
        let render = tree.render_stats();
        info!(
            "fps {fps:.1} · frame {:.2} ms · {} draws · {} objects ({} culled) · {} shadow draws · {} lights",
            1000.0 / fps,
            render.draw_calls,
            render.objects,
            render.culled,
            render.shadow_draw_calls,
            render.lights
        );
        self.frames = 0;
        self.accumulated = 0.0;
    }
}
