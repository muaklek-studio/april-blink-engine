//! Navigation on a walkability grid over the XZ plane: good for indoor levels and small
//! outdoor areas, where AI agents (a chasing ghost, patrolling guards) need to get around
//! walls and furniture.
//!
//! * [`NavGrid::bake`] samples the physics world: each cell is walkable when a downward ray
//!   finds a gentle enough floor and an agent-sized column above it is free.
//! * [`NavGrid::find_path`] runs A* (8 directions, no corner cutting, limited step height)
//!   and straightens the result, returning world-space waypoints.
//! * [`PathFollower`] walks an agent along those waypoints.
//!
//! Bake after the physics world has stepped at least once (queries use the last step's
//! state), and again when the level changes.

mod follow;
mod grid;

pub use follow::PathFollower;
pub use grid::{Cell, NavBakeSettings, NavGrid};
