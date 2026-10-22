use crate::convert::Pose;
use crate::shape::Shape;
use ab_core::Vec3;
use ab_ecs::Entity;
use rapier3d::prelude::{ColliderHandle, RigidBodyHandle};

/// How a [`PhysicsBody3D`] moves. Mirrors Godot's body node classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BodyKind {
    /// `StaticBody3D`: never moves on its own. Moving the node teleports it.
    Static,
    /// `RigidBody3D`: simulated (gravity, forces, collisions). Owns its node's transform.
    Rigid,
    /// `AnimatableBody3D`: moved by code or animation through the node's transform, and
    /// pushes rigid bodies out of the way.
    Kinematic,
    /// `CharacterBody3D`: kinematic body moved with
    /// [`PhysicsNodes::move_and_slide`](crate::PhysicsNodes::move_and_slide); see
    /// [`CharacterBody3D`].
    Character,
    /// `Area3D`: detects overlapping bodies and areas (trigger volume) without colliding.
    Area,
}

impl BodyKind {
    /// Godot class name of the node type.
    pub fn class_name(self) -> &'static str {
        match self {
            Self::Static => crate::class::STATIC_BODY_3D,
            Self::Rigid => crate::class::RIGID_BODY_3D,
            Self::Kinematic => crate::class::ANIMATABLE_BODY_3D,
            Self::Character => crate::class::CHARACTER_BODY_3D,
            Self::Area => crate::class::AREA_3D,
        }
    }
}

/// A physics body (Godot's `CollisionObject3D` family). Its collision geometry comes from
/// [`CollisionShape3D`] nodes on the same node or below it.
///
/// Rigid bodies write their simulated pose to the node every physics step; every other kind
/// follows the node. Setting the node's transform of a rigid body teleports it.
///
/// Collision filtering follows Godot: two objects interact when either one's
/// `collision_mask` contains the other's `collision_layer`. A character's
/// [`move_and_slide`](crate::PhysicsNodes::move_and_slide) and scene queries only see
/// objects whose layer is in their own mask.
#[derive(Debug, Clone)]
pub struct PhysicsBody3D {
    pub kind: BodyKind,
    /// Total mass in kg (rigid bodies), spread over the shapes by volume.
    pub mass: f32,
    pub gravity_scale: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    /// Keeps a rigid body upright (e.g. physics-driven characters).
    pub lock_rotation: bool,
    /// Continuous collision detection, for small fast bodies that would tunnel through walls.
    pub continuous_cd: bool,
    pub can_sleep: bool,
    pub friction: f32,
    /// Restitution: 0 = no bounce, 1 = perfectly elastic.
    pub bounce: f32,
    /// Layers this body is on (bit mask).
    pub collision_layer: u32,
    /// Layers this body interacts with (bit mask).
    pub collision_mask: u32,
    linear_velocity: Vec3,
    angular_velocity: Vec3,
    sleeping: bool,
    pub(crate) pending: PendingMotion,
    pub(crate) state: Option<BodyState>,
}

impl PhysicsBody3D {
    pub fn new(kind: BodyKind) -> Self {
        Self {
            kind,
            mass: 1.0,
            gravity_scale: 1.0,
            linear_damping: 0.0,
            angular_damping: 0.0,
            lock_rotation: false,
            continuous_cd: false,
            can_sleep: true,
            friction: 0.5,
            bounce: 0.0,
            collision_layer: 1,
            collision_mask: 1,
            linear_velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            sleeping: false,
            pending: PendingMotion::default(),
            state: None,
        }
    }

    pub fn fixed() -> Self {
        Self::new(BodyKind::Static)
    }

    pub fn rigid() -> Self {
        Self::new(BodyKind::Rigid)
    }

    pub fn kinematic() -> Self {
        Self::new(BodyKind::Kinematic)
    }

    pub fn character() -> Self {
        Self::new(BodyKind::Character)
    }

    pub fn area() -> Self {
        Self::new(BodyKind::Area)
    }

    pub fn with_mass(mut self, mass: f32) -> Self {
        self.mass = mass;
        self
    }

    pub fn with_friction(mut self, friction: f32) -> Self {
        self.friction = friction;
        self
    }

    pub fn with_bounce(mut self, bounce: f32) -> Self {
        self.bounce = bounce;
        self
    }

    pub fn with_collision(mut self, layer: u32, mask: u32) -> Self {
        self.collision_layer = layer;
        self.collision_mask = mask;
        self
    }

    /// Linear velocity in m/s as of the last physics step (or as last set).
    pub fn linear_velocity(&self) -> Vec3 {
        self.linear_velocity
    }

    /// Angular velocity in rad/s (axis × speed) as of the last physics step (or as last set).
    pub fn angular_velocity(&self) -> Vec3 {
        self.angular_velocity
    }

    /// Overrides a rigid body's linear velocity at the next physics step.
    pub fn set_linear_velocity(&mut self, velocity: Vec3) {
        self.linear_velocity = velocity;
        self.pending.linear_velocity = Some(velocity);
    }

    pub fn set_angular_velocity(&mut self, velocity: Vec3) {
        self.angular_velocity = velocity;
        self.pending.angular_velocity = Some(velocity);
    }

    /// Instant change of momentum through the centre of mass (jumps, explosions, hits).
    pub fn apply_central_impulse(&mut self, impulse: Vec3) {
        self.pending.impulse += impulse;
    }

    /// Impulse applied at a global-space `point`; off-centre hits also spin the body.
    pub fn apply_impulse_at(&mut self, impulse: Vec3, point: Vec3) {
        self.pending.point_impulses.push((impulse, point));
    }

    pub fn apply_torque_impulse(&mut self, impulse: Vec3) {
        self.pending.torque_impulse += impulse;
    }

    /// Force through the centre of mass for the next physics step only; call it every step
    /// for a continuous push.
    pub fn apply_central_force(&mut self, force: Vec3) {
        self.pending.force += force;
    }

    /// Torque for the next physics step only.
    pub fn apply_torque(&mut self, torque: Vec3) {
        self.pending.torque += torque;
    }

    /// Whether the simulation put this rigid body to sleep (at rest, not simulated).
    pub fn is_sleeping(&self) -> bool {
        self.sleeping
    }

    /// Whether the body exists in the physics world yet (bodies are created at the first
    /// physics step after their node enters the tree).
    pub fn is_simulated(&self) -> bool {
        self.state.is_some()
    }

    pub(crate) fn settings(&self) -> BodySettings {
        BodySettings {
            kind: self.kind,
            mass: self.mass,
            gravity_scale: self.gravity_scale,
            linear_damping: self.linear_damping,
            angular_damping: self.angular_damping,
            lock_rotation: self.lock_rotation,
            continuous_cd: self.continuous_cd,
            can_sleep: self.can_sleep,
            collider: ColliderSettings {
                friction: self.friction,
                bounce: self.bounce,
                collision_layer: self.collision_layer,
                collision_mask: self.collision_mask,
                sensor: self.kind == BodyKind::Area,
            },
        }
    }

    pub(crate) fn sync_from_simulation(&mut self, linear: Vec3, angular: Vec3, sleeping: bool) {
        self.linear_velocity = linear;
        self.angular_velocity = angular;
        self.sleeping = sleeping;
    }
}

/// Velocity overrides and impulses queued by gameplay code until the next physics step.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PendingMotion {
    pub linear_velocity: Option<Vec3>,
    pub angular_velocity: Option<Vec3>,
    pub impulse: Vec3,
    pub torque_impulse: Vec3,
    pub point_impulses: Vec<(Vec3, Vec3)>,
    pub force: Vec3,
    pub torque: Vec3,
}

impl PendingMotion {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Body parameters as last applied to Rapier; a change triggers an update.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BodySettings {
    pub kind: BodyKind,
    pub mass: f32,
    pub gravity_scale: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub lock_rotation: bool,
    pub continuous_cd: bool,
    pub can_sleep: bool,
    pub collider: ColliderSettings,
}

/// The part of a body's settings that lives on its colliders.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ColliderSettings {
    pub friction: f32,
    pub bounce: f32,
    pub collision_layer: u32,
    pub collision_mask: u32,
    pub sensor: bool,
}

/// Link between a body component and its Rapier body.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BodyState {
    pub handle: RigidBodyHandle,
    /// Node pose last pushed to or read from Rapier; a different node pose means gameplay
    /// code moved the node.
    pub pose: Pose,
    pub settings: BodySettings,
}

/// Collision geometry for the nearest [`PhysicsBody3D`] on this node or an ancestor
/// (Godot's `CollisionShape3D`). Shapes without a body do nothing.
#[derive(Debug, Clone)]
pub struct CollisionShape3D {
    shape: Shape,
    revision: u32,
    /// Disabled shapes are removed from the simulation.
    pub disabled: bool,
    pub(crate) state: Option<ColliderState>,
    /// Revision whose geometry could not be built (warned about once).
    pub(crate) failed_revision: u32,
}

impl CollisionShape3D {
    pub fn new(shape: Shape) -> Self {
        Self {
            shape,
            revision: 0,
            disabled: false,
            state: None,
            failed_revision: u32::MAX,
        }
    }

    pub fn shape(&self) -> &Shape {
        &self.shape
    }

    /// Replaces the geometry; the collider is rebuilt at the next physics step.
    pub fn set_shape(&mut self, shape: Shape) {
        self.shape = shape;
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn revision(&self) -> u32 {
        self.revision
    }
}

/// Link between a shape component and its Rapier collider.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ColliderState {
    pub handle: ColliderHandle,
    pub body: RigidBodyHandle,
    pub body_entity: Entity,
    pub revision: u32,
    pub scale: Vec3,
    pub offset: Pose,
    pub settings: ColliderSettings,
}

/// Movement state of a `CharacterBody3D`, used by
/// [`PhysicsNodes::move_and_slide`](crate::PhysicsNodes::move_and_slide). As in Godot,
/// gameplay code owns `velocity` (including gravity) and `move_and_slide` slides it along
/// whatever the body hits.
#[derive(Debug, Clone)]
pub struct CharacterBody3D {
    /// Velocity in m/s, adjusted by `move_and_slide` after collisions.
    pub velocity: Vec3,
    pub up: Vec3,
    /// Steepest slope (radians) that still counts as floor and can be walked up.
    pub floor_max_angle: f32,
    /// Distance the body is pulled down to stay on the floor (stairs and slopes going
    /// down). 0 disables snapping.
    pub floor_snap_length: f32,
    /// Highest step climbed automatically. 0 disables auto-stepping.
    pub max_step_height: f32,
    /// Whether rigid bodies the character walks into get pushed.
    pub push_rigid_bodies: bool,
    on_floor: bool,
    on_wall: bool,
    on_ceiling: bool,
    floor_normal: Vec3,
}

impl CharacterBody3D {
    pub fn with_velocity(mut self, velocity: Vec3) -> Self {
        self.velocity = velocity;
        self
    }

    pub fn is_on_floor(&self) -> bool {
        self.on_floor
    }

    pub fn is_on_wall(&self) -> bool {
        self.on_wall
    }

    pub fn is_on_ceiling(&self) -> bool {
        self.on_ceiling
    }

    /// Normal of the floor under the body, or `up` when not on the floor.
    pub fn floor_normal(&self) -> Vec3 {
        self.floor_normal
    }

    pub(crate) fn set_contacts(&mut self, floor: Option<Vec3>, wall: bool, ceiling: bool) {
        self.on_floor = floor.is_some();
        self.floor_normal = floor.unwrap_or(self.up);
        self.on_wall = wall;
        self.on_ceiling = ceiling;
    }
}

impl Default for CharacterBody3D {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            up: Vec3::Y,
            floor_max_angle: 45f32.to_radians(),
            floor_snap_length: 0.1,
            max_step_height: 0.0,
            push_rigid_bodies: true,
            on_floor: false,
            on_wall: false,
            on_ceiling: false,
            floor_normal: Vec3::Y,
        }
    }
}
