//! Visual effects: particles (smoke, fire, sparks, dust, magic, rain, snow, explosions, ...),
//! fading light flashes, camera shake and full-screen effects (flashes, fades, hit stop,
//! distortion), made to be easy to drop into a game.
//!
//! ```
//! use ab_core::Vec3;
//! use ab_effects::{EffectNodes, ParticleEmitter, ParticleSettings};
//! use ab_scene::SceneTree;
//!
//! let mut tree = SceneTree::new();
//! ab_effects::register(tree.world_mut());
//!
//! // A smoking chimney: a continuous emitter from a preset.
//! let smoke = tree.create_particles3d("Smoke", ParticleEmitter::new(ParticleSettings::smoke()));
//! tree.add_child(tree.root(), smoke).unwrap();
//!
//! // A one-shot burst of sparks at an impact point, freed automatically when done.
//! let root = tree.root();
//! tree.spawn_effect(root, Vec3::new(1.0, 0.5, 0.0), ParticleSettings::sparks()).unwrap();
//!
//! // Presets are a starting point; adjust anything.
//! let green_fire = ParticleSettings::fire()
//!     .with_colors(ab_core::Color::rgb(0.3, 1.0, 0.4), ab_core::Color::rgba(0.0, 0.3, 0.0, 0.0))
//!     .with_amount(80);
//! # let _ = green_fire;
//! ```
//!
//! Screen effects are one resource away:
//!
//! ```
//! # use ab_core::Color;
//! # let mut world = ab_ecs::World::new();
//! # ab_effects::register(&mut world);
//! let mut screen = world.resource_mut::<ab_effects::ScreenEffects>().unwrap();
//! screen.flash(Color::rgba(1.0, 0.2, 0.2, 0.5), 0.25); // hurt
//! screen.hit_stop(0.06);                               // make the hit land
//! screen.distort(1.5, 0.4);                            // and wobble the image
//! ```
//!
//! [`effects_system`] simulates everything once per frame (the engine schedules it) and
//! fills the renderer's [`ParticleDrawList`](ab_rendering::ParticleDrawList). Particles are
//! simulated on the CPU (up to [`MAX_PARTICLES`] per emitter) and drawn as camera-facing
//! quads in the HDR pass: depth-tested, fogged, sorted back to front; additive ones glow.
//! Busy scenes simulate their emitters in parallel on the [`ab_core::jobs`] workers.

mod emitter;
mod nodes;
mod screen;
mod settings;
mod system;

pub use emitter::{ParticleEmitter, MAX_PARTICLES};
pub use nodes::{class, EffectNodes, ExplosionOptions};
pub use screen::{flash_screen, hit_stop, ScreenEffects};
pub use settings::{ColorRamp, EmissionShape, ParticleSettings};
pub use system::{effects_system, shake_cameras, CameraShake, LightFlash};

use ab_ecs::World;
use ab_rendering::{ParticleDrawList, ScreenOverlay};

/// Inserts the [`ParticleDrawList`], [`ScreenEffects`] and [`ScreenOverlay`] resources (and
/// the simulation's scratch buffers) if missing.
pub fn register(world: &mut World) {
    world.resource_or_insert_with(ParticleDrawList::default);
    world.resource_or_insert_with(ScreenEffects::default);
    world.resource_or_insert_with(ScreenOverlay::default);
    world.resource_or_insert_with(system::ParticleScratch::default);
}
