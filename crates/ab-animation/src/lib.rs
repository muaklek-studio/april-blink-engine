//! Keyframe animation: clips of translation/rotation/scale tracks, an [`AnimationLibrary`]
//! resource that owns them, and an [`AnimationPlayer`] component that plays one clip on a set
//! of target nodes (typically the joints of an imported glTF skeleton).
//!
//! [`animation_system`] advances every player by the frame delta and writes the sampled
//! values into the targets' local transforms. Run it before transform propagation (the engine
//! schedules it in the `Update` stage), so the renderer sees the posed skeleton the same frame.
//! Busy scenes sample their players in parallel on the [`ab_core::jobs`] workers.

mod clip;
mod player;

pub use clip::Sample;
pub use clip::{AnimationClip, Channel, Interpolation, Keyframes, Property};
pub use player::{
    animation_system, apply_animations, AnimationLibrary, AnimationPlayer, AnimationPoses,
    ClipHandle,
};

use ab_ecs::World;

/// Inserts the [`AnimationLibrary`] and the [`AnimationPoses`] scratch buffers if missing.
pub fn register(world: &mut World) {
    world.resource_or_insert_with(AnimationLibrary::new);
    world.resource_or_insert_with(AnimationPoses::default);
}
