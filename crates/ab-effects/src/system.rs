//! Per-frame update of particles, light flashes and camera shake.

use crate::emitter::ParticleEmitter;
use ab_core::jobs;
use ab_core::{Quat, Time, Vec3};
use ab_ecs::{Entity, World};
use ab_rendering::{OmniLight3D, ParticleDrawList, ParticleInstance};
use ab_scene::{NodeInfo, Transform};

/// Fades an omni light from its energy to zero (muzzle flashes, explosions).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightFlash {
    pub energy: f32,
    pub duration: f32,
    elapsed: f32,
}

impl LightFlash {
    pub fn new(energy: f32, duration: f32) -> Self {
        Self {
            energy,
            duration: duration.max(0.001),
            elapsed: 0.0,
        }
    }

    pub fn is_finished(&self) -> bool {
        self.elapsed >= self.duration
    }
}

/// Trauma-based shake for a camera. Put it on a `Node3D` between the camera and whatever
/// moves it (the shake owns that node's transform), then call
/// [`CameraShake::add_trauma`] on hits and explosions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraShake {
    /// 0..1; the shake strength is trauma².
    pub trauma: f32,
    /// Largest offset in metres at full trauma.
    pub max_offset: f32,
    /// Largest roll/pitch/yaw in radians at full trauma.
    pub max_angle: f32,
    /// Trauma lost per second.
    pub decay: f32,
    /// Shake speed.
    pub frequency: f32,
    time: f32,
}

impl CameraShake {
    pub fn add_trauma(&mut self, amount: f32) {
        self.trauma = (self.trauma + amount).clamp(0.0, 1.0);
    }
}

impl Default for CameraShake {
    fn default() -> Self {
        Self {
            trauma: 0.0,
            max_offset: 0.12,
            max_angle: 0.05,
            decay: 1.2,
            frequency: 18.0,
            time: 0.0,
        }
    }
}

/// Adds trauma to every [`CameraShake`] in the world.
pub fn shake_cameras(world: &World, trauma: f32) {
    for shake in world.query::<&mut CameraShake>().iter() {
        shake.add_trauma(trauma);
    }
}

/// Per-emitter draw buffers, kept between frames so the parallel simulation does not
/// allocate. Registered by [`crate::register`].
#[derive(Debug, Default)]
pub(crate) struct ParticleScratch(Vec<Vec<ParticleInstance>>);

/// Below this many live particles in total, emitters are simulated on the calling thread.
const PARALLEL_PARTICLES: usize = 512;

/// Simulates every emitter in the tree (in parallel over emitters for busy scenes) and
/// appends the visible particles to `out`.
fn simulate_particles(world: &World, delta: f32, out: &mut Vec<ParticleInstance>) {
    let (Some(mut emitters), Some(transforms), Some(infos)) = (
        world.storage_mut::<ParticleEmitter>(),
        world.storage::<Transform>(),
        world.storage::<NodeInfo>(),
    ) else {
        return;
    };
    let mut scratch = world.resource_mut::<ParticleScratch>();
    let mut local = Vec::new();
    let buffers = match scratch.as_deref_mut() {
        Some(ParticleScratch(buffers)) => buffers,
        None => &mut local,
    };
    let (entities, emitters) = emitters.parts_mut();
    buffers.resize_with(emitters.len(), Vec::new);
    let live: usize = emitters.iter().map(|e| e.alive()).sum();
    let mut work: Vec<_> = entities
        .iter()
        .copied()
        .zip(emitters.iter_mut())
        .zip(buffers.iter_mut())
        .collect();
    let min_per_job = if live < PARALLEL_PARTICLES {
        usize::MAX
    } else {
        1
    };
    let (transforms, infos) = (&*transforms, &*infos);
    jobs::par_for_each_mut(&mut work, min_per_job, |((entity, emitter), drawn)| {
        drawn.clear();
        let Some(info) = infos.get(*entity).filter(|i| i.in_tree) else {
            return;
        };
        let Some(transform) = transforms.get(*entity) else {
            return;
        };
        emitter.update(&transform.global, delta);
        if info.visible {
            emitter.draw(&transform.global, drawn);
        }
    });
    out.reserve(work.iter().map(|(_, drawn)| drawn.len()).sum());
    for (_, drawn) in &work {
        out.extend_from_slice(drawn);
    }
}

fn in_tree(world: &World, entity: Entity) -> bool {
    world.get::<NodeInfo>(entity).is_some_and(|i| i.in_tree)
}

/// Simulates every [`ParticleEmitter`] in the tree and fills the [`ParticleDrawList`],
/// fades [`LightFlash`]es, moves [`CameraShake`] nodes and runs the
/// [`ScreenEffects`](crate::ScreenEffects). The engine runs it in
/// `PostUpdate`, after scripts moved things and transforms were propagated.
pub fn effects_system(world: &mut World) {
    let world: &World = world;
    let delta = world.resource::<Time>().map_or(0.0, |t| t.delta);
    crate::screen::update_screen_effects(world);

    if let Some(mut list) = world.resource_mut::<ParticleDrawList>() {
        list.clear();
        simulate_particles(world, delta, &mut list.instances);
    }

    for (entity, flash, light) in world
        .query::<(Entity, &mut LightFlash, &mut OmniLight3D)>()
        .iter()
    {
        if !in_tree(world, entity) {
            continue;
        }
        flash.elapsed += delta;
        let remaining = (1.0 - flash.elapsed / flash.duration).clamp(0.0, 1.0);
        light.energy = flash.energy * remaining * remaining;
    }

    for (shake, transform) in world.query::<(&mut CameraShake, &mut Transform)>().iter() {
        shake.time += delta * shake.frequency;
        shake.trauma = (shake.trauma - shake.decay * delta).max(0.0);
        let strength = shake.trauma * shake.trauma;
        // Smooth pseudo-noise from incommensurate sines.
        let t = shake.time;
        let noise = |a: f32, b: f32| ((t * a).sin() + (t * b + 1.3).sin()) * 0.5;
        transform.local.translation =
            Vec3::new(noise(1.0, 2.7), noise(1.3, 3.1), 0.0) * shake.max_offset * strength;
        transform.local.rotation = Quat::from_euler(
            ab_core::EulerRot::YXZ,
            noise(0.9, 2.3) * shake.max_angle * strength,
            noise(1.1, 2.9) * shake.max_angle * strength,
            noise(0.7, 3.7) * shake.max_angle * strength,
        );
        transform.dirty = true;
    }
}
