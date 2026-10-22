//! The particle emitter component and its simulation.

use crate::settings::{EmissionShape, ParticleSettings};
use ab_core::utils::rand::SimpleRng;
use ab_core::{Affine3A, Quat, Vec3};
use ab_rendering::ParticleInstance;

/// Hard cap per emitter, whatever `amount` says.
pub const MAX_PARTICLES: u32 = 10_000;

#[derive(Debug, Clone, Copy)]
struct Particle {
    /// World space, or emitter space with `local_coords`.
    position: Vec3,
    velocity: Vec3,
    age: f32,
    lifetime: f32,
    size_scale: f32,
    rotation: f32,
    spin: f32,
    /// Phase of the turbulence wobble.
    phase: f32,
}

/// Emits and simulates particles at its node (Godot's `CPUParticles3D`). Create one with
/// [`EffectNodes::create_particles3d`](crate::EffectNodes::create_particles3d).
#[derive(Debug, Clone)]
pub struct ParticleEmitter {
    pub settings: ParticleSettings,
    /// Emitting new particles. Existing ones finish their life either way.
    pub emitting: bool,
    /// Emit `amount` particles once, then stop.
    pub one_shot: bool,
    /// Particles follow the emitter (attached trails) instead of staying where they were
    /// born (smoke left behind).
    pub local_coords: bool,
    /// Simulation speed multiplier.
    pub speed_scale: f32,
    particles: Vec<Particle>,
    accumulator: f32,
    burst_done: bool,
    rng: SimpleRng,
}

impl ParticleEmitter {
    pub fn new(settings: ParticleSettings) -> Self {
        Self {
            settings,
            emitting: true,
            one_shot: false,
            local_coords: false,
            speed_scale: 1.0,
            particles: Vec::new(),
            accumulator: 0.0,
            burst_done: false,
            rng: SimpleRng::new(0x5eed_1234),
        }
    }

    /// Emit everything at once, then stop (explosions, impacts).
    pub fn one_shot(mut self) -> Self {
        self.one_shot = true;
        self
    }

    pub fn local(mut self) -> Self {
        self.local_coords = true;
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.rng = SimpleRng::new(seed);
        self
    }

    /// Restarts a one-shot emitter (or clears a continuous one).
    pub fn restart(&mut self) {
        self.particles.clear();
        self.accumulator = 0.0;
        self.burst_done = false;
        self.emitting = true;
    }

    pub fn alive(&self) -> usize {
        self.particles.len()
    }

    /// A one-shot emitter that has emitted and whose particles are all gone.
    pub fn is_finished(&self) -> bool {
        self.one_shot && self.burst_done && self.particles.is_empty()
    }

    fn spawn(&mut self, emitter: &Affine3A) {
        let s = &self.settings;
        let rng = &mut self.rng;
        let local = match s.shape {
            EmissionShape::Point => Vec3::ZERO,
            EmissionShape::Sphere { radius } => random_in_sphere(rng) * radius,
            EmissionShape::Box { size } => {
                Vec3::new(
                    rng.range_f32(-0.5, 0.5),
                    rng.range_f32(-0.5, 0.5),
                    rng.range_f32(-0.5, 0.5),
                ) * size
            }
            EmissionShape::Ring { radius } => {
                let angle = rng.range_f32(0.0, std::f32::consts::TAU);
                Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius)
            }
        };
        let direction = random_in_cone(
            rng,
            s.direction.normalize_or(Vec3::Y),
            s.spread.to_radians(),
        );
        let speed = rng.range_f32(s.speed[0], s.speed[1].max(s.speed[0]));
        let lifetime =
            s.lifetime.max(0.01) * (1.0 - s.lifetime_random.clamp(0.0, 1.0) * rng.next_f32());
        let size_scale = 1.0 - s.size_random.clamp(0.0, 1.0) * rng.next_f32();
        let spin = rng.range_f32(s.spin[0], s.spin[1].max(s.spin[0]));
        let rotation = rng.range_f32(0.0, std::f32::consts::TAU);
        let phase = rng.range_f32(0.0, 100.0);
        let (position, velocity) = if self.local_coords {
            (local, direction * speed)
        } else {
            (
                emitter.transform_point3(local),
                emitter.transform_vector3(direction).normalize_or_zero() * speed,
            )
        };
        self.particles.push(Particle {
            position,
            velocity,
            age: 0.0,
            lifetime,
            size_scale,
            rotation,
            spin,
            phase,
        });
    }

    /// Advances the simulation by `delta` seconds with the emitter at `transform`.
    pub fn update(&mut self, transform: &Affine3A, delta: f32) {
        let delta = delta * self.speed_scale.max(0.0);
        let amount = self.settings.amount.min(MAX_PARTICLES);
        if self.emitting {
            if self.one_shot {
                if !self.burst_done {
                    for _ in 0..amount {
                        self.spawn(transform);
                    }
                    self.burst_done = true;
                    self.emitting = false;
                }
            } else {
                self.accumulator += delta * amount as f32 / self.settings.lifetime.max(0.01);
                while self.accumulator >= 1.0 && (self.particles.len() as u32) < amount {
                    self.spawn(transform);
                    self.accumulator -= 1.0;
                }
                self.accumulator = self.accumulator.min(1.0);
            }
        }

        let s = &self.settings;
        let gravity = if self.local_coords {
            // Gravity is a world-space acceleration.
            transform.inverse().transform_vector3(s.gravity)
        } else {
            s.gravity
        };
        let damping = 1.0 / (1.0 + s.damping.max(0.0) * delta);
        self.particles.retain_mut(|p| {
            p.age += delta;
            if p.age >= p.lifetime {
                return false;
            }
            let wobble = if s.turbulence > 0.0 {
                let t = p.age + p.phase;
                Vec3::new((t * 1.7).sin(), (t * 2.3).sin(), (t * 1.9).cos()) * s.turbulence
            } else {
                Vec3::ZERO
            };
            p.velocity = (p.velocity + (gravity + wobble) * delta) * damping;
            p.position += p.velocity * delta;
            p.rotation += p.spin * delta;
            true
        });
    }

    /// Appends the particles' draw data.
    pub fn draw(&self, transform: &Affine3A, out: &mut Vec<ParticleInstance>) {
        let s = &self.settings;
        let scale = if self.local_coords {
            transform.to_scale_rotation_translation().0.max_element()
        } else {
            1.0
        };
        for p in &self.particles {
            let t = (p.age / p.lifetime).clamp(0.0, 1.0);
            let mut color = s.color.sample(t);
            color.r *= s.emission;
            color.g *= s.emission;
            color.b *= s.emission;
            let (position, velocity) = if self.local_coords {
                (
                    transform.transform_point3(p.position),
                    transform.transform_vector3(p.velocity),
                )
            } else {
                (p.position, p.velocity)
            };
            out.push(ParticleInstance {
                position,
                size: (s.size[0] + (s.size[1] - s.size[0]) * t) * p.size_scale * scale,
                color,
                velocity,
                stretch: s.stretch,
                rotation: p.rotation,
                hardness: s.hardness,
                additive: s.additive,
            });
        }
    }
}

fn random_in_sphere(rng: &mut SimpleRng) -> Vec3 {
    loop {
        let p = Vec3::new(
            rng.range_f32(-1.0, 1.0),
            rng.range_f32(-1.0, 1.0),
            rng.range_f32(-1.0, 1.0),
        );
        if p.length_squared() <= 1.0 {
            return p;
        }
    }
}

/// Uniform direction within `angle` radians of `axis`.
fn random_in_cone(rng: &mut SimpleRng, axis: Vec3, angle: f32) -> Vec3 {
    let cos_max = angle.min(std::f32::consts::PI).cos();
    let z = rng.range_f32(cos_max, 1.0);
    let phi = rng.range_f32(0.0, std::f32::consts::TAU);
    let r = (1.0 - z * z).max(0.0).sqrt();
    let local = Vec3::new(r * phi.cos(), r * phi.sin(), z);
    Quat::from_rotation_arc(Vec3::Z, axis) * local
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_core::Color;

    fn update(emitter: &mut ParticleEmitter, seconds: f32) {
        let steps = (seconds * 60.0).round() as usize;
        for _ in 0..steps {
            emitter.update(&Affine3A::IDENTITY, 1.0 / 60.0);
        }
    }

    #[test]
    fn continuous_emission_reaches_its_amount() {
        let mut emitter = ParticleEmitter::new(ParticleSettings {
            amount: 30,
            lifetime: 1.0,
            ..Default::default()
        });
        update(&mut emitter, 0.5);
        assert!((13..=17).contains(&emitter.alive()), "{}", emitter.alive());
        update(&mut emitter, 2.0);
        assert!((28..=30).contains(&emitter.alive()), "{}", emitter.alive());
        emitter.emitting = false;
        update(&mut emitter, 1.1);
        assert_eq!(emitter.alive(), 0, "everything died out");
        assert!(!emitter.is_finished(), "not one-shot");
    }

    #[test]
    fn one_shot_bursts_then_finishes() {
        let mut emitter =
            ParticleEmitter::new(ParticleSettings::sparks().with_amount(25)).one_shot();
        update(&mut emitter, 1.0 / 60.0);
        assert_eq!(emitter.alive(), 25);
        update(&mut emitter, 1.2);
        assert!(emitter.is_finished());
        emitter.restart();
        update(&mut emitter, 1.0 / 60.0);
        assert_eq!(emitter.alive(), 25);
    }

    #[test]
    fn particles_move_fall_and_fade() {
        let settings = ParticleSettings {
            amount: 1,
            lifetime: 1.0,
            speed: [2.0, 2.0],
            spread: 0.0,
            direction: Vec3::X,
            gravity: Vec3::new(0.0, -10.0, 0.0),
            size: [1.0, 3.0],
            color: crate::ColorRamp::two(
                Color::rgba(1.0, 0.0, 0.0, 1.0),
                Color::rgba(1.0, 0.0, 0.0, 0.0),
            ),
            emission: 2.0,
            ..Default::default()
        };
        let mut emitter = ParticleEmitter::new(settings).one_shot();
        // Emitter moved to x = 10: world-space particles start there.
        let at = Affine3A::from_translation(Vec3::new(10.0, 0.0, 0.0));
        for _ in 0..30 {
            emitter.update(&at, 1.0 / 60.0);
        }
        let mut out = Vec::new();
        emitter.draw(&at, &mut out);
        let p = out[0];
        assert!(
            (p.position.x - 11.0).abs() < 0.05,
            "moved 1 m along +X: {}",
            p.position
        );
        assert!(p.position.y < -1.0, "fell: {}", p.position);
        assert!((p.size - 2.0).abs() < 0.1, "half-way size: {}", p.size);
        assert!((p.color.a - 0.5).abs() < 0.05 && (p.color.r - 2.0).abs() < 1e-4);
    }

    #[test]
    fn local_particles_follow_the_emitter() {
        let mut emitter = ParticleEmitter::new(ParticleSettings {
            amount: 1,
            lifetime: 5.0,
            speed: [0.0, 0.0],
            ..Default::default()
        })
        .one_shot()
        .local();
        emitter.update(&Affine3A::IDENTITY, 0.016);
        let moved = Affine3A::from_translation(Vec3::new(0.0, 5.0, 0.0));
        let mut out = Vec::new();
        emitter.draw(&moved, &mut out);
        assert!(out[0].position.abs_diff_eq(Vec3::new(0.0, 5.0, 0.0), 1e-5));
    }

    #[test]
    fn cone_directions_stay_inside_the_spread() {
        let mut rng = SimpleRng::new(1);
        for _ in 0..500 {
            let d = random_in_cone(&mut rng, Vec3::Y, 20f32.to_radians());
            assert!((d.length() - 1.0).abs() < 1e-4);
            assert!(d.dot(Vec3::Y) >= 20f32.to_radians().cos() - 1e-4);
        }
        let any = random_in_cone(&mut rng, Vec3::NEG_Y, std::f32::consts::PI);
        assert!((any.length() - 1.0).abs() < 1e-4);
    }
}
