//! 3D physics on top of [Rapier](https://rapier.rs), exposed through Godot-style nodes.
//!
//! * [`PhysicsBody3D`]: a static, rigid, kinematic (animatable), character or area body.
//! * [`CollisionShape3D`]: geometry for the nearest body on its node or an ancestor.
//! * [`CharacterBody3D`] + [`PhysicsNodes::move_and_slide`]: kinematic character controller.
//! * [`Joint3D`]: fixed, pin, hinge and cone-twist joints between two bodies.
//! * [`Ragdoll`]: jointed bodies that follow a skeleton (kinematic hit boxes) or go limp
//!   and drive it ([`PhysicsNodes::create_skeleton_ragdoll`],
//!   [`PhysicsNodes::set_ragdoll_active`], [`PhysicsNodes::apply_ragdoll_impulse`]).
//! * [`PhysicsWorld`] (ECS resource): the simulation, [`PhysicsEvent`]s of the last step,
//!   area overlaps and scene queries ([`PhysicsWorld::cast_ray`],
//!   [`PhysicsWorld::has_line_of_sight`], [`PhysicsWorld::intersect_sphere`]).
//!
//! [`physics_system`] runs once per fixed step after `physics_process`:
//!
//! 1. node → physics: bodies and colliders are created, updated or removed to match the tree
//!    (nodes outside the tree are not simulated); static, kinematic, character and area
//!    bodies follow their nodes; queued velocities, impulses and forces are applied;
//! 2. the simulation advances by the fixed delta;
//! 3. physics → node: rigid bodies write their pose to their nodes; contact and overlap
//!    changes become [`PhysicsEvent`]s.
//!
//! ```
//! use ab_core::Vec3;
//! use ab_ecs::World;
//! use ab_physics::{physics_system, PhysicsBody3D, PhysicsNodes, PhysicsWorld, Shape};
//! use ab_scene::SceneTree;
//!
//! let mut tree = SceneTree::new();
//! tree.world_mut().insert_resource(PhysicsWorld::new());
//! let ball = tree.create_physics_body3d("Ball", PhysicsBody3D::rigid());
//! tree.add_collision_shape(ball, Shape::sphere(0.5)).unwrap();
//! tree.set_position(ball, Vec3::new(0.0, 10.0, 0.0)).unwrap();
//! tree.add_child(tree.root(), ball).unwrap();
//!
//! for _ in 0..60 {
//!     physics_system(tree.world_mut());
//! }
//! assert!(tree.position(ball).unwrap().y < 6.0); // fell for one second
//! ```

mod character;
mod components;
mod convert;
mod events;
mod joint;
mod nodes;
mod query;
mod ragdoll;
mod shape;
mod world;

pub use components::{BodyKind, CharacterBody3D, CollisionShape3D, PhysicsBody3D};
pub use events::PhysicsEvent;
pub use joint::{Joint3D, JointKind};
pub use nodes::PhysicsNodes;
pub use query::{QueryOptions, RayHit};
pub use ragdoll::{Ragdoll, RagdollBone, SkeletonRagdoll};
pub use shape::Shape;
pub use world::{physics_system, PhysicsDebugOptions, PhysicsWorld};

use ab_scene::{NodeHandle, SceneError};

/// Names of the signals physics emits (deliver them with `SceneTree::connect`).
///
/// * On an area: [`BODY_ENTERED`](signals::BODY_ENTERED) / [`BODY_EXITED`](signals::BODY_EXITED)
///   when a body (or another area) starts/stops overlapping it.
/// * On a solid body: the same signals when another solid body starts/stops touching it.
///
/// The argument is the other body's [`NodeHandle`].
pub mod signals {
    pub const BODY_ENTERED: &str = "body_entered";
    pub const BODY_EXITED: &str = "body_exited";
}

/// Class names of the physics node types.
pub mod class {
    pub const STATIC_BODY_3D: &str = "StaticBody3D";
    pub const RIGID_BODY_3D: &str = "RigidBody3D";
    pub const ANIMATABLE_BODY_3D: &str = "AnimatableBody3D";
    pub const CHARACTER_BODY_3D: &str = "CharacterBody3D";
    pub const AREA_3D: &str = "Area3D";
    pub const COLLISION_SHAPE_3D: &str = "CollisionShape3D";
    pub const JOINT_3D: &str = "Joint3D";
    pub const RAGDOLL: &str = "Ragdoll";
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PhysicsError {
    #[error("{0} is not a CharacterBody3D")]
    NotACharacter(NodeHandle),
    #[error("{0} is not a Ragdoll")]
    NotARagdoll(NodeHandle),
    #[error("the ragdoll has no bone long enough for a body")]
    NoRagdollBones,
    #[error(transparent)]
    Scene(#[from] SceneError),
}
