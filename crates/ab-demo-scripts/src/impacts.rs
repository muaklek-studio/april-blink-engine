use ab_script::prelude::*;
use ab_script::SoundOptions;
use std::collections::HashMap;

/// The impact sound, in the assets.
const THUD: &str = "audio/thud.wav";

/// Plays a thud where bodies collide, louder for faster impacts. Speeds are sampled every
/// physics step, so an impact is judged by the speed just before the step that reported it.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ImpactSounds {
    /// Slower contacts (resting, sliding) stay silent.
    pub min_speed: f32,
    /// Speed of the loudest thud.
    pub max_speed: f32,
    /// Thuds per step at most (a collapsing stack would otherwise roar).
    pub max_per_step: usize,
    #[serde(skip)]
    speeds: HashMap<Node, f32>,
    #[serde(skip, default = "seeded")]
    rng: Rng,
    /// The sound failed to play once: stop trying (and logging).
    #[serde(skip)]
    unavailable: bool,
}

fn seeded() -> Rng {
    Rng::new(99)
}

impl Default for ImpactSounds {
    fn default() -> Self {
        Self {
            min_speed: 1.2,
            max_speed: 7.0,
            max_per_step: 4,
            speeds: HashMap::new(),
            rng: seeded(),
            unavailable: false,
        }
    }
}

impl Script for ImpactSounds {
    serde_state!();

    fn physics_process(&mut self, tree: &mut Tree<'_>, _node: Node, _delta: f32) {
        let speed = |speeds: &HashMap<Node, f32>, node| speeds.get(&node).copied().unwrap_or(0.0);
        let mut played = 0;
        for event in tree.physics().events() {
            let PhysicsEvent::ContactStarted { a, b } = event else {
                continue;
            };
            let (impact, body) = if speed(&self.speeds, a) >= speed(&self.speeds, b) {
                (speed(&self.speeds, a), a)
            } else {
                (speed(&self.speeds, b), b)
            };
            if self.unavailable || impact < self.min_speed || played == self.max_per_step {
                continue;
            }
            let Some(position) = tree.global_position(body) else {
                continue;
            };
            let strength = (impact / self.max_speed).min(1.0);
            debug!("impact on {body} at {impact:.1} m/s");
            let options = SoundOptions {
                volume_db: -24.0 + 24.0 * strength,
                pitch_scale: self.rng.range_f32(0.8, 1.2),
            };
            if let Err(err) = tree.play_sound_at(THUD, position, options) {
                warn!("impact sound unavailable: {err}");
                self.unavailable = true;
            }
            played += 1;
        }

        self.speeds.clear();
        for body in tree.physics().bodies(BodyKind::Rigid) {
            let speed = tree
                .body(body)
                .linear_velocity()
                .unwrap_or_default()
                .length();
            self.speeds.insert(body, speed);
        }
    }
}
