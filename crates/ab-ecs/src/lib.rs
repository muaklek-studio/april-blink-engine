//! Minimal ECS used as the internal data layer of April Blink Engine.
//!
//! * [`Entity`]: generational index; stale handles are detected, never aliased.
//! * Storage: one [`SparseSet`] per component type, O(1) insert/remove/lookup and dense
//!   iteration.
//! * [`World`]: entities, component storages and singleton resources. Component and resource
//!   access goes through `RefCell`s, so conflicting borrows (e.g. `&mut T` twice) panic with a
//!   clear message instead of causing undefined behaviour.
//! * [`World::query`]: iterate entities matching `&T`, `&mut T`, `Option<&T>`, [`Entity`] and
//!   tuples of those, optionally filtered by [`With`]/[`Without`].
//! * [`Commands`]: deferred structural changes, applied with [`World::flush`].
//! * [`Schedule`]: ordered stages of [`System`]s.
//!
//! ```
//! use ab_ecs::World;
//!
//! struct Position(f32);
//! struct Velocity(f32);
//!
//! let mut world = World::new();
//! let e = world.spawn((Position(0.0), Velocity(2.0)));
//! for (pos, vel) in world.query::<(&mut Position, &Velocity)>().iter() {
//!     pos.0 += vel.0;
//! }
//! assert_eq!(world.get::<Position>(e).unwrap().0, 2.0);
//! ```

mod bundle;
mod command;
mod component;
mod entity;
mod query;
mod schedule;
mod sparse_set;
mod system;
mod world;

pub use bundle::Bundle;
pub use command::Commands;
pub use component::Component;
pub use entity::{Entity, EntityAllocator};
pub use query::{Query, QueryData, QueryFilter, QueryIter, With, Without};
pub use schedule::{Schedule, ScheduleStage};
pub use sparse_set::SparseSet;
pub use system::{named, IntoSystem, System};
pub use world::{EcsError, Resource, World};

/// Re-exported so callers can name the guard types returned by [`World::get`] and friends.
pub use std::cell::{Ref, RefMut};
