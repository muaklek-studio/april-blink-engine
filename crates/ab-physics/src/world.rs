use crate::components::{
    BodyKind, BodySettings, BodyState, ColliderState, CollisionShape3D, PendingMotion,
    PhysicsBody3D,
};
use crate::convert::Pose;
use crate::events::{EventCollector, PhysicsEvent};
use crate::joint::{Joint3D, JointKind, JointState};
use crate::query::{QueryOptions, RayHit};
use crate::ragdoll;
use ab_core::{Affine3A, Color, Quat, Time, Transform3D, Vec3};
use ab_ecs::{Entity, Without, World};
use ab_scene::{NodeHandle, NodeInfo, Parent, Signal, SignalQueue, Transform};
use rapier3d::prelude::{
    ActiveCollisionTypes, ActiveEvents, ActiveHooks, CCDSolver, Collider, ColliderBuilder,
    ColliderHandle, ColliderSet, CollisionEvent, DebugRenderBackend, DebugRenderMode,
    DebugRenderObject, DebugRenderPipeline, DebugRenderStyle, DefaultBroadPhase,
    GenericJointBuilder, Group, ImpulseJointHandle, ImpulseJointSet, IntegrationParameters,
    InteractionGroups, InteractionTestMode, IslandManager, JointAxesMask, JointAxis,
    MultibodyJointSet, NarrowPhase, PairFilterContext, PhysicsHooks, PhysicsPipeline, QueryFilter,
    QueryPipeline, Ray, RigidBody, RigidBodyActivation, RigidBodyBuilder, RigidBodyHandle,
    RigidBodySet, RigidBodyType, SoftBodySet, SolverFlags,
};
use std::collections::{HashMap, HashSet};
use std::fmt;
use tracing::warn;

/// Which body and shape node a Rapier collider belongs to.
#[derive(Debug, Clone, Copy)]
struct ColliderInfo {
    shape: Entity,
    body: Entity,
    sensor: bool,
}

/// ECS resource owning the Rapier simulation and its link to the scene.
///
/// [`physics_system`] steps it once per fixed step: it creates, updates and removes Rapier
/// bodies and colliders to match the [`PhysicsBody3D`] and [`CollisionShape3D`] nodes in the
/// tree, steps the simulation, writes rigid-body poses back to their nodes and records
/// [`PhysicsEvent`]s. Scene queries (raycasts, overlaps) read the state of the last step.
pub struct PhysicsWorld {
    /// Gravity acceleration in m/s².
    pub gravity: Vec3,
    integration: IntegrationParameters,
    pipeline: PhysicsPipeline,
    islands: IslandManager,
    pub(crate) broad_phase: DefaultBroadPhase,
    pub(crate) narrow_phase: NarrowPhase,
    pub(crate) bodies: RigidBodySet,
    pub(crate) colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    /// Unused (the engine has no soft bodies), but Rapier's step takes one.
    soft_bodies: SoftBodySet,
    ccd: CCDSolver,
    collector: EventCollector,

    body_entities: HashMap<RigidBodyHandle, Entity>,
    joint_entities: HashMap<ImpulseJointHandle, Entity>,
    collider_info: HashMap<ColliderHandle, ColliderInfo>,
    /// Collider pairs currently touching or overlapping, as reported by Rapier.
    active_pairs: HashSet<(ColliderHandle, ColliderHandle)>,
    /// Touching collider pairs per (ordered) pair of solid bodies.
    contacts: HashMap<(Entity, Entity), u32>,
    /// Overlapping collider pairs per (area, other body).
    overlaps: HashMap<(Entity, Entity), u32>,
    events: Vec<PhysicsEvent>,
    /// Bodies whose mass must be redistributed over their colliders, with their mass.
    mass_dirty: HashMap<RigidBodyHandle, f32>,
    /// Queued velocity changes and impulses, applied once mass properties are current.
    motions: Vec<(RigidBodyHandle, PendingMotion)>,
    /// Bodies with one-step forces to clear before the next step.
    forced: HashSet<RigidBodyHandle>,
    root: Option<Entity>,
}

impl PhysicsWorld {
    /// Default gravity: Earth, pointing down (-Y).
    pub const DEFAULT_GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);

    pub fn new() -> Self {
        Self {
            gravity: Self::DEFAULT_GRAVITY,
            integration: IntegrationParameters::default(),
            pipeline: PhysicsPipeline::new(),
            islands: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            soft_bodies: SoftBodySet::new(),
            ccd: CCDSolver::new(),
            collector: EventCollector::default(),
            body_entities: HashMap::new(),
            joint_entities: HashMap::new(),
            collider_info: HashMap::new(),
            active_pairs: HashSet::new(),
            contacts: HashMap::new(),
            overlaps: HashMap::new(),
            events: Vec::new(),
            mass_dirty: HashMap::new(),
            motions: Vec::new(),
            forced: HashSet::new(),
            root: None,
        }
    }

    /// Events of the last physics step.
    pub fn events(&self) -> &[PhysicsEvent] {
        &self.events
    }

    /// Number of joints in the simulation.
    pub fn joint_count(&self) -> usize {
        self.impulse_joints.len()
    }

    /// Number of bodies in the simulation.
    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }

    /// Number of colliders in the simulation.
    pub fn collider_count(&self) -> usize {
        self.colliders.len()
    }

    /// Bodies (and areas) currently inside `area`.
    pub fn overlapping_bodies(&self, area: NodeHandle) -> Vec<NodeHandle> {
        let mut found: Vec<NodeHandle> = self
            .overlaps
            .keys()
            .filter(|(a, _)| *a == area.entity())
            .map(|&(_, other)| NodeHandle::from_entity(other))
            .collect();
        found.sort_unstable();
        found
    }

    pub fn is_overlapping(&self, area: NodeHandle, body: NodeHandle) -> bool {
        self.overlaps.contains_key(&(area.entity(), body.entity()))
    }

    /// Solid bodies currently touching `body`.
    pub fn colliding_bodies(&self, body: NodeHandle) -> Vec<NodeHandle> {
        let me = body.entity();
        let mut found: Vec<NodeHandle> = self
            .contacts
            .keys()
            .filter_map(|&(a, b)| match (a == me, b == me) {
                (true, _) => Some(NodeHandle::from_entity(b)),
                (_, true) => Some(NodeHandle::from_entity(a)),
                _ => None,
            })
            .collect();
        found.sort_unstable();
        found
    }

    // =========================================================================================
    // Scene queries
    // =========================================================================================

    /// Closest hit along a ray from `origin` towards `direction` (any length), up to
    /// `max_distance`. A ray starting inside a shape hits it at distance 0.
    pub fn cast_ray(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        options: &QueryOptions,
    ) -> Option<RayHit> {
        let direction = direction.try_normalize()?;
        let ray = Ray::new(origin, direction);
        self.with_queries(options, |queries| {
            queries.cast_ray_and_get_normal(&ray, max_distance, true)
        })
        .and_then(|(handle, hit)| {
            let info = self.collider_info.get(&handle)?;
            Some(RayHit {
                node: NodeHandle::from_entity(info.body),
                shape: NodeHandle::from_entity(info.shape),
                position: ray.point_at(hit.time_of_impact),
                normal: hit.normal,
                distance: hit.time_of_impact,
            })
        })
    }

    /// Whether nothing blocks the straight line from `from` to `to` (AI sight checks,
    /// interaction prompts). Exclude the looker and the target in `options`.
    pub fn has_line_of_sight(&self, from: Vec3, to: Vec3, options: &QueryOptions) -> bool {
        let distance = from.distance(to);
        distance <= f32::EPSILON || self.cast_ray(from, to - from, distance, options).is_none()
    }

    /// Bodies with a shape overlapping the sphere (explosions, proximity checks), sorted.
    pub fn intersect_sphere(
        &self,
        center: Vec3,
        radius: f32,
        options: &QueryOptions,
    ) -> Vec<NodeHandle> {
        let ball = rapier3d::prelude::Ball::new(radius.max(0.0));
        let position = rapier3d::math::Pose::from_translation(center);
        let mut found: Vec<NodeHandle> = self.with_queries(options, |queries| {
            queries
                .intersect_shape(position, &ball)
                .filter_map(|(handle, _)| self.collider_info.get(&handle))
                .map(|info| NodeHandle::from_entity(info.body))
                .collect()
        });
        found.sort_unstable();
        found.dedup();
        found
    }

    /// Runs `f` with a scene query pipeline filtered by `options`.
    fn with_queries<R>(
        &self,
        options: &QueryOptions,
        f: impl FnOnce(&QueryPipeline<'_>) -> R,
    ) -> R {
        let predicate = |handle: ColliderHandle, _: &Collider| {
            self.collider_info.get(&handle).is_some_and(|info| {
                !options
                    .exclude
                    .iter()
                    .any(|node| node.entity() == info.body)
            })
        };
        let mut filter = QueryFilter::new()
            .groups(InteractionGroups::new(
                Group::ALL,
                Group::from_bits_retain(options.collision_mask),
                InteractionTestMode::And,
            ))
            .predicate(&predicate);
        if !options.include_areas {
            filter = filter.exclude_sensors();
        }
        f(&self.query_pipeline(filter))
    }

    /// Scene queries over the state of the last step.
    pub(crate) fn query_pipeline<'a>(&'a self, filter: QueryFilter<'a>) -> QueryPipeline<'a> {
        self.broad_phase.as_query_pipeline(
            self.narrow_phase.query_dispatcher(),
            &self.bodies,
            &self.colliders,
            filter,
        )
    }

    // =========================================================================================
    // Stepping
    // =========================================================================================

    /// Syncs the scene into the simulation, advances it by `dt` seconds and syncs the results
    /// back. Normally run by [`physics_system`].
    pub fn step(&mut self, world: &World, dt: f32) {
        self.events.clear();
        if let Some(root) = self.scene_root(world) {
            ab_scene::propagate_transforms(world, NodeHandle::from_entity(root));
        }
        ragdoll::before_step(world);
        self.sync_bodies(world);
        self.sync_colliders(world);
        self.sync_joints(world);
        self.distribute_mass();
        self.apply_motions();

        self.integration.dt = dt;
        self.pipeline.step(
            self.gravity,
            &self.integration,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.soft_bodies,
            &mut self.ccd,
            &LayerFilter,
            &self.collector,
        );

        self.write_back(world);
        ragdoll::after_step(world);
        for event in self.collector.drain() {
            let (a, b, started) = match event {
                CollisionEvent::Started(a, b, _) => (a, b, true),
                CollisionEvent::Stopped(a, b, _) => (a, b, false),
            };
            let key = pair_key(a, b);
            let changed = if started {
                self.active_pairs.insert(key)
            } else {
                self.active_pairs.remove(&key)
            };
            if changed {
                self.pair_changed(a, b, started);
            }
        }
        self.queue_signals(world);
    }

    /// Turns this step's events into node signals.
    fn queue_signals(&self, world: &World) {
        use crate::signals::{BODY_ENTERED, BODY_EXITED};
        let Some(mut queue) = world.resource_mut::<SignalQueue>() else {
            return;
        };
        let mut send = |source: NodeHandle, name: &'static str, other: NodeHandle| {
            queue.push(Signal::new(source, name).with_arg(other));
        };
        for event in &self.events {
            match *event {
                PhysicsEvent::BodyEntered { area, body } => send(area, BODY_ENTERED, body),
                PhysicsEvent::BodyExited { area, body } => send(area, BODY_EXITED, body),
                PhysicsEvent::ContactStarted { a, b } => {
                    send(a, BODY_ENTERED, b);
                    send(b, BODY_ENTERED, a);
                }
                PhysicsEvent::ContactEnded { a, b } => {
                    send(a, BODY_EXITED, b);
                    send(b, BODY_EXITED, a);
                }
            }
        }
    }

    /// The tree root (the in-tree node without a parent), cached between steps.
    fn scene_root(&mut self, world: &World) -> Option<Entity> {
        let is_root = |e: Entity| {
            world.get::<NodeInfo>(e).is_some_and(|i| i.in_tree) && !world.has::<Parent>(e)
        };
        if self.root.is_none_or(|root| !is_root(root)) {
            self.root = world
                .query_filtered::<(Entity, &NodeInfo), Without<Parent>>()
                .iter()
                .find(|(_, info)| info.in_tree)
                .map(|(entity, _)| entity);
        }
        self.root
    }

    /// Creates, updates and removes Rapier bodies to match the [`PhysicsBody3D`] nodes.
    fn sync_bodies(&mut self, world: &World) {
        let mut live = HashSet::new();
        if let (Some(mut bodies), Some(transforms), Some(infos)) = (
            world.storage_mut::<PhysicsBody3D>(),
            world.storage::<Transform>(),
            world.storage::<NodeInfo>(),
        ) {
            for (entity, body) in bodies.iter_mut() {
                let global = transforms
                    .get(entity)
                    .filter(|_| infos.get(entity).is_some_and(|i| i.in_tree))
                    .map(|t| t.global);
                // Outside the tree (or not a Node3D): not simulated. The sweep below removes
                // the Rapier body; it is recreated if the node comes back.
                let Some(global) = global else {
                    body.state = None;
                    continue;
                };
                let (_, node_pose) = decompose(&global);
                let state = body
                    .state
                    .filter(|s| self.body_entities.get(&s.handle) == Some(&entity))
                    .unwrap_or_else(|| self.create_body(entity, body, node_pose));
                body.state = Some(self.update_body(body, state, node_pose));
                live.insert(state.handle);
            }
        }
        let stale: Vec<RigidBodyHandle> = self
            .body_entities
            .keys()
            .filter(|handle| !live.contains(handle))
            .copied()
            .collect();
        for handle in stale {
            self.remove_body(handle);
        }
    }

    fn create_body(&mut self, entity: Entity, body: &PhysicsBody3D, at: Pose) -> BodyState {
        let settings = body.settings();
        let mut rigid_body = RigidBodyBuilder::new(body_type(settings.kind))
            .pose(at.to_rapier())
            .linvel(body.linear_velocity())
            .angvel(body.angular_velocity())
            .user_data(u128::from(entity.to_bits()))
            .build();
        apply_body_settings(&mut rigid_body, &settings);
        let handle = self.bodies.insert(rigid_body);
        self.body_entities.insert(handle, entity);
        self.mass_dirty.insert(handle, settings.mass);
        BodyState {
            handle,
            pose: at,
            settings,
        }
    }

    /// Pushes setting changes, node moves and queued impulses to the Rapier body.
    fn update_body(
        &mut self,
        body: &mut PhysicsBody3D,
        mut state: BodyState,
        at: Pose,
    ) -> BodyState {
        let settings = body.settings();
        let rigid_body = &mut self.bodies[state.handle];
        if settings != state.settings {
            apply_body_settings(rigid_body, &settings);
            if settings.kind != state.settings.kind || settings.mass != state.settings.mass {
                self.mass_dirty.insert(state.handle, settings.mass);
            }
            state.settings = settings;
        }

        if !at.approx_eq(&state.pose) {
            match settings.kind {
                // Teleport.
                BodyKind::Rigid | BodyKind::Static => rigid_body.set_position(at.to_rapier(), true),
                // Moved over the step, so the solver sees the motion and pushes bodies away.
                BodyKind::Kinematic | BodyKind::Character | BodyKind::Area => {
                    rigid_body.set_next_kinematic_position(at.to_rapier())
                }
            }
            state.pose = at;
        }

        let pending = std::mem::take(&mut body.pending);
        if settings.kind == BodyKind::Rigid && !pending.is_empty() {
            self.motions.push((state.handle, pending));
        }
        state
    }

    /// Applies queued motion. Runs after [`Self::distribute_mass`] so impulses see the
    /// body's final mass.
    fn apply_motions(&mut self) {
        for handle in std::mem::take(&mut self.forced) {
            if let Some(body) = self.bodies.get_mut(handle) {
                body.reset_forces(false);
                body.reset_torques(false);
            }
        }
        for (handle, pending) in self.motions.drain(..) {
            let Some(body) = self.bodies.get_mut(handle) else {
                continue;
            };
            if let Some(velocity) = pending.linear_velocity {
                body.set_linvel(velocity, true);
            }
            if let Some(velocity) = pending.angular_velocity {
                body.set_angvel(velocity, true);
            }
            if pending.impulse != Vec3::ZERO {
                body.apply_impulse(pending.impulse, true);
            }
            if pending.torque_impulse != Vec3::ZERO {
                body.apply_torque_impulse(pending.torque_impulse, true);
            }
            for (impulse, at) in pending.point_impulses {
                body.apply_impulse_at_point(impulse, at, true);
            }
            if pending.force != Vec3::ZERO || pending.torque != Vec3::ZERO {
                body.add_force(pending.force, true);
                body.add_torque(pending.torque, true);
                self.forced.insert(handle);
            }
        }
    }

    /// Creates, re-creates and removes Rapier joints to match the [`Joint3D`] nodes.
    fn sync_joints(&mut self, world: &World) {
        let mut live = HashSet::new();
        if let (Some(mut joints), Some(transforms), Some(infos), Some(bodies)) = (
            world.storage_mut::<Joint3D>(),
            world.storage::<Transform>(),
            world.storage::<NodeInfo>(),
            world.storage::<PhysicsBody3D>(),
        ) {
            for (entity, joint) in joints.iter_mut() {
                let global = transforms
                    .get(entity)
                    .filter(|_| !joint.disabled)
                    .filter(|_| infos.get(entity).is_some_and(|i| i.in_tree))
                    .map(|t| t.global);
                let handle_of = |node: Option<NodeHandle>| {
                    node.and_then(|n| bodies.get(n.entity()))
                        .and_then(|b| b.state)
                        .map(|s| s.handle)
                        .filter(|h| self.bodies.get(*h).is_some())
                };
                let (Some(global), Some(a), Some(b)) =
                    (global, handle_of(joint.body_a), handle_of(joint.body_b))
                else {
                    joint.state = None;
                    continue;
                };
                if a == b {
                    joint.state = None;
                    continue;
                }
                let settings = joint.settings();
                let current = joint.state.filter(|s| {
                    s.bodies == (a, b)
                        && s.settings == settings
                        && self.joint_entities.get(&s.handle) == Some(&entity)
                        && self.impulse_joints.get(s.handle).is_some()
                });
                let state = match current {
                    Some(state) => state,
                    None => {
                        if let Some(old) = joint.state {
                            self.remove_joint(old.handle);
                        }
                        let handle = self.create_joint(a, b, &global, joint);
                        self.joint_entities.insert(handle, entity);
                        JointState {
                            handle,
                            bodies: (a, b),
                            settings,
                        }
                    }
                };
                joint.state = Some(state);
                live.insert(state.handle);
            }
        }
        let stale: Vec<ImpulseJointHandle> = self
            .joint_entities
            .keys()
            .filter(|h| !live.contains(h))
            .copied()
            .collect();
        for handle in stale {
            self.remove_joint(handle);
        }
    }

    fn create_joint(
        &mut self,
        a: RigidBodyHandle,
        b: RigidBodyHandle,
        global: &Affine3A,
        joint: &Joint3D,
    ) -> ImpulseJointHandle {
        let (_, frame) = decompose(global);
        let frame_in = |handle: RigidBodyHandle| {
            Pose::from_rapier(self.bodies[handle].position()).relative(frame)
        };
        let (frame_a, frame_b) = (frame_in(a), frame_in(b));
        let locked = match joint.kind {
            JointKind::Fixed => JointAxesMask::LOCKED_FIXED_AXES,
            JointKind::Pin | JointKind::ConeTwist { .. } => JointAxesMask::LOCKED_SPHERICAL_AXES,
            JointKind::Hinge { .. } => JointAxesMask::LOCKED_REVOLUTE_AXES,
        };
        let mut builder = GenericJointBuilder::new(locked)
            .local_frame1(frame_a.to_rapier())
            .local_frame2(frame_b.to_rapier())
            .contacts_enabled(joint.collide_connected);
        match joint.kind {
            JointKind::Hinge {
                limits: Some([min, max]),
            } => builder = builder.limits(JointAxis::AngX, [min.min(max), min.max(max)]),
            JointKind::ConeTwist { swing, twist } => {
                let (swing, twist) = (swing.abs(), twist.abs());
                builder = builder
                    .limits(JointAxis::AngX, [-twist, twist])
                    .limits(JointAxis::AngY, [-swing, swing])
                    .limits(JointAxis::AngZ, [-swing, swing]);
            }
            _ => {}
        }
        self.impulse_joints.insert(a, b, builder.build(), true)
    }

    fn remove_joint(&mut self, handle: ImpulseJointHandle) {
        self.joint_entities.remove(&handle);
        self.impulse_joints.remove(handle, true);
    }

    // =========================================================================================
    // Debug drawing
    // =========================================================================================

    /// Calls `line` for every debug line of the simulation's current state: shapes (colored
    /// by body kind; sleeping bodies dimmed, areas violet), joints, contacts.
    pub fn debug_lines(&self, options: &PhysicsDebugOptions, line: impl FnMut(Vec3, Vec3, Color)) {
        let mut mode = DebugRenderMode::empty();
        if options.shapes {
            mode |= DebugRenderMode::COLLIDER_SHAPES;
        }
        if options.joints {
            mode |= DebugRenderMode::IMPULSE_JOINTS;
        }
        if options.contacts {
            mode |= DebugRenderMode::CONTACTS;
        }
        if options.aabbs {
            mode |= DebugRenderMode::COLLIDER_AABBS;
        }
        if mode.is_empty() {
            return;
        }
        let style = DebugRenderStyle {
            subdivisions: 12,
            border_subdivisions: 3,
            ..Default::default()
        };
        let mut backend = LineBackend {
            bodies: &self.bodies,
            line,
        };
        DebugRenderPipeline::new(style, mode).render(
            &mut backend,
            &self.bodies,
            &self.colliders,
            &self.impulse_joints,
            &self.multibody_joints,
            &self.narrow_phase,
            &self.soft_bodies,
        );
    }

    fn remove_body(&mut self, handle: RigidBodyHandle) {
        self.body_entities.remove(&handle);
        self.mass_dirty.remove(&handle);
        self.forced.remove(&handle);
        let attached = self
            .bodies
            .get(handle)
            .map(|b| b.colliders().to_vec())
            .unwrap_or_default();
        for collider in attached {
            self.forget_collider(collider);
        }
        self.bodies.remove(
            handle,
            &mut self.islands,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.soft_bodies,
            true,
        );
    }

    /// Creates, updates and removes Rapier colliders to match the [`CollisionShape3D`] nodes.
    /// Each shape attaches to the nearest simulated body on its node or an ancestor.
    fn sync_colliders(&mut self, world: &World) {
        let mut live = HashSet::new();
        if let (Some(mut shapes), Some(transforms), Some(infos), Some(bodies)) = (
            world.storage_mut::<CollisionShape3D>(),
            world.storage::<Transform>(),
            world.storage::<NodeInfo>(),
            world.storage::<PhysicsBody3D>(),
        ) {
            let parents = world.storage::<Parent>();
            for (entity, shape) in shapes.iter_mut() {
                let global = transforms
                    .get(entity)
                    .filter(|_| !shape.disabled)
                    .filter(|_| infos.get(entity).is_some_and(|i| i.in_tree))
                    .map(|t| t.global);
                let owner = std::iter::successors(Some(entity), |&e| {
                    parents.as_ref()?.get(e).map(|p| p.0.entity())
                })
                .find_map(|e| bodies.get(e).map(|b| (e, b.state)));
                let (Some(global), Some((body_entity, Some(body)))) = (global, owner) else {
                    shape.state = None;
                    continue;
                };

                let (scale, shape_pose) = decompose(&global);
                let offset = body.pose.relative(shape_pose);
                let settings = body.settings.collider;
                let reusable = shape.state.filter(|s| {
                    s.body == body.handle
                        && s.body_entity == body_entity
                        && self.collider_info.contains_key(&s.handle)
                        && s.revision == shape.revision()
                        && s.settings == settings
                        && s.scale.abs_diff_eq(scale, 1e-5)
                });
                let state = match reusable {
                    Some(mut state) => {
                        if !state.offset.approx_eq(&offset) {
                            self.colliders[state.handle]
                                .set_position_wrt_parent(offset.to_rapier());
                            self.mass_dirty.insert(body.handle, body.settings.mass);
                            state.offset = offset;
                        }
                        state
                    }
                    None => {
                        let Some(geometry) = shape.shape().build(scale) else {
                            if shape.state.is_some() || shape.revision() != shape.failed_revision {
                                warn!(?entity, shape = ?shape.shape(), "degenerate collision shape ignored");
                            }
                            shape.failed_revision = shape.revision();
                            shape.state = None;
                            continue;
                        };
                        let collider = ColliderBuilder::new(geometry)
                            .position(offset.to_rapier())
                            .friction(settings.friction)
                            .restitution(settings.bounce)
                            // Broad and narrow phase see every pair; `LayerFilter` applies
                            // the layer/mask rule. Queries test their mask against `layer`.
                            .collision_groups(InteractionGroups::new(
                                Group::from_bits_retain(settings.collision_layer),
                                Group::ALL,
                                InteractionTestMode::And,
                            ))
                            .active_hooks(
                                ActiveHooks::FILTER_CONTACT_PAIRS
                                    | ActiveHooks::FILTER_INTERSECTION_PAIR,
                            )
                            .sensor(settings.sensor)
                            .active_events(ActiveEvents::COLLISION_EVENTS)
                            .active_collision_types(if settings.sensor {
                                // Areas also see static, kinematic and character bodies.
                                ActiveCollisionTypes::all()
                            } else {
                                ActiveCollisionTypes::default()
                            })
                            .user_data(pack_user_data(
                                entity,
                                settings.collision_layer,
                                settings.collision_mask,
                            ))
                            .build();
                        let handle = self.colliders.insert_with_parent(
                            collider,
                            body.handle,
                            &mut self.bodies,
                        );
                        self.collider_info.insert(
                            handle,
                            ColliderInfo {
                                shape: entity,
                                body: body_entity,
                                sensor: settings.sensor,
                            },
                        );
                        self.mass_dirty.insert(body.handle, body.settings.mass);
                        ColliderState {
                            handle,
                            body: body.handle,
                            body_entity,
                            revision: shape.revision(),
                            scale,
                            offset,
                            settings,
                        }
                    }
                };
                shape.state = Some(state);
                live.insert(state.handle);
            }
        }
        let stale: Vec<ColliderHandle> = self
            .collider_info
            .keys()
            .filter(|handle| !live.contains(handle))
            .copied()
            .collect();
        for handle in stale {
            if let Some(parent) = self.colliders.get(handle).and_then(|c| c.parent()) {
                if let Some(&entity) = self.body_entities.get(&parent) {
                    let mass = world.get::<PhysicsBody3D>(entity).map_or(1.0, |b| b.mass);
                    self.mass_dirty.insert(parent, mass);
                }
            }
            self.forget_collider(handle);
            self.colliders.remove(
                handle,
                &mut self.islands,
                &mut self.bodies,
                &mut self.soft_bodies,
                true,
            );
        }
    }

    /// Ends the contacts and overlaps of a collider that is about to be removed and drops
    /// its bookkeeping. Rapier's own stop events for it are then ignored.
    fn forget_collider(&mut self, handle: ColliderHandle) {
        let pairs: Vec<_> = self
            .active_pairs
            .iter()
            .filter(|(a, b)| *a == handle || *b == handle)
            .copied()
            .collect();
        for (a, b) in pairs {
            self.active_pairs.remove(&(a, b));
            self.pair_changed(a, b, false);
        }
        self.collider_info.remove(&handle);
    }

    /// Spreads each rigid body's mass over its colliders in proportion to their volume, so
    /// the body weighs exactly `mass` and its inertia follows its shapes.
    fn distribute_mass(&mut self) {
        for (handle, mass) in std::mem::take(&mut self.mass_dirty) {
            let Some(body) = self.bodies.get_mut(handle) else {
                continue;
            };
            let attached = body.colliders().to_vec();
            let volume: f32 = attached
                .iter()
                .filter_map(|&c| self.colliders.get(c))
                .map(|c| c.volume())
                .sum();
            let mass = mass.max(1e-3);
            // Volume-less shapes (triangle meshes): put the mass on the body itself.
            let (density, extra_mass) = if volume > f32::EPSILON {
                (mass / volume, 0.0)
            } else {
                (0.0, mass)
            };
            body.set_additional_mass(extra_mass, true);
            for &collider in &attached {
                if let Some(collider) = self.colliders.get_mut(collider) {
                    collider.set_density(density);
                }
            }
            // Rapier would only pick the new mass up during the step; impulses applied
            // before it need it now.
            if let Some(body) = self.bodies.get_mut(handle) {
                body.recompute_mass_properties_from_colliders(&self.colliders);
            }
        }
    }

    /// Copies rigid-body poses to their nodes and velocities to their components.
    fn write_back(&mut self, world: &World) {
        let (Some(mut bodies), Some(mut transforms)) = (
            world.storage_mut::<PhysicsBody3D>(),
            world.storage_mut::<Transform>(),
        ) else {
            return;
        };
        let parents = world.storage::<Parent>();
        for (entity, body) in bodies.iter_mut() {
            let Some(mut state) = body.state else {
                continue;
            };
            let Some(rigid_body) = self.bodies.get(state.handle) else {
                continue;
            };
            body.sync_from_simulation(
                rigid_body.linvel(),
                rigid_body.angvel(),
                rigid_body.is_sleeping(),
            );
            let simulated = Pose::from_rapier(rigid_body.position());
            if state.settings.kind != BodyKind::Rigid || simulated.approx_eq(&state.pose) {
                continue;
            }

            // A plain Node parent breaks transform inheritance, as in propagation.
            let parent_global = parents
                .as_ref()
                .and_then(|p| p.get(entity))
                .and_then(|p| transforms.get(p.0.entity()))
                .map_or(Affine3A::IDENTITY, |t| t.global);
            let Some(transform) = transforms.get_mut(entity) else {
                continue;
            };
            let scale = transform.global.to_scale_rotation_translation().0;
            let global = Affine3A::from_scale_rotation_translation(
                scale,
                simulated.rotation,
                simulated.position,
            );
            transform.local = Transform3D::from_affine(&(parent_global.inverse() * global));
            transform.global = parent_global * transform.local.to_affine();
            transform.dirty = true;
            // Remember the pose as the node will report it, so rounding is not mistaken for
            // a teleport next step.
            state.pose = decompose(&transform.global).1;
            body.state = Some(state);
        }
    }

    /// Updates contact/overlap counts for a collider pair and emits events on transitions.
    fn pair_changed(&mut self, a: ColliderHandle, b: ColliderHandle, started: bool) {
        let (Some(&a), Some(&b)) = (self.collider_info.get(&a), self.collider_info.get(&b)) else {
            return;
        };
        if a.body == b.body {
            return;
        }
        let node = NodeHandle::from_entity;
        if a.sensor || b.sensor {
            for (area, other) in [(a, b), (b, a)] {
                if area.sensor && bump(&mut self.overlaps, (area.body, other.body), started) {
                    let (area, body) = (node(area.body), node(other.body));
                    self.events.push(if started {
                        PhysicsEvent::BodyEntered { area, body }
                    } else {
                        PhysicsEvent::BodyExited { area, body }
                    });
                }
            }
        } else {
            let key = (a.body.min(b.body), a.body.max(b.body));
            if bump(&mut self.contacts, key, started) {
                let (a, b) = (node(key.0), node(key.1));
                self.events.push(if started {
                    PhysicsEvent::ContactStarted { a, b }
                } else {
                    PhysicsEvent::ContactEnded { a, b }
                });
            }
        }
    }
}

impl Default for PhysicsWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for PhysicsWorld {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhysicsWorld")
            .field("gravity", &self.gravity)
            .field("bodies", &self.bodies.len())
            .field("colliders", &self.colliders.len())
            .field("contacts", &self.contacts.len())
            .field("overlaps", &self.overlaps.len())
            .finish()
    }
}

/// Steps the [`PhysicsWorld`] resource by [`Time::fixed_delta`]. The engine runs it in
/// `PostFixedUpdate`, right after `physics_process`, so velocities and impulses set by
/// scripts take effect in the same step.
pub fn physics_system(world: &mut World) {
    let world: &World = world;
    let dt = world
        .resource::<Time>()
        .map_or(1.0 / 60.0, |time| time.fixed_delta);
    if let Some(mut physics) = world.resource_mut::<PhysicsWorld>() {
        physics.step(world, dt);
    }
}

/// What [`PhysicsWorld::debug_lines`] draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicsDebugOptions {
    pub shapes: bool,
    pub joints: bool,
    pub contacts: bool,
    pub aabbs: bool,
}

impl Default for PhysicsDebugOptions {
    fn default() -> Self {
        Self {
            shapes: true,
            joints: true,
            contacts: true,
            aabbs: false,
        }
    }
}

/// Turns Rapier's debug output into colored lines.
struct LineBackend<'a, F> {
    bodies: &'a RigidBodySet,
    line: F,
}

impl<F: FnMut(Vec3, Vec3, Color)> DebugRenderBackend for LineBackend<'_, F> {
    fn draw_line(&mut self, object: DebugRenderObject<'_>, a: Vec3, b: Vec3, _color: [f32; 4]) {
        let color = match object {
            DebugRenderObject::Collider(_, collider) => {
                let body = collider.parent().and_then(|h| self.bodies.get(h));
                let base = if collider.is_sensor() {
                    Color::rgb(0.7, 0.35, 1.0)
                } else {
                    match body.map(|b| b.body_type()) {
                        Some(RigidBodyType::Dynamic) => Color::rgb(1.0, 0.55, 0.15),
                        Some(RigidBodyType::Fixed) | None => Color::rgb(0.35, 0.8, 0.45),
                        Some(_) => Color::rgb(0.25, 0.75, 1.0),
                    }
                };
                if body.is_some_and(|b| b.is_sleeping()) {
                    Color::rgb(base.r * 0.45, base.g * 0.45, base.b * 0.45)
                } else {
                    base
                }
            }
            DebugRenderObject::ColliderAabb(..) => Color::rgb(0.5, 0.5, 0.5),
            DebugRenderObject::ImpulseJoint(..) | DebugRenderObject::MultibodyJoint(..) => {
                Color::rgb(1.0, 0.9, 0.2)
            }
            DebugRenderObject::ContactPair(..) => Color::rgb(1.0, 0.2, 0.25),
            DebugRenderObject::RigidBody(..) | DebugRenderObject::SoftBody(..) => Color::WHITE,
        };
        (self.line)(a, b, color);
    }
}

/// Collider user data: the shape node in the low 64 bits, then the mask and the layer.
fn pack_user_data(entity: Entity, layer: u32, mask: u32) -> u128 {
    u128::from(entity.to_bits()) | (u128::from(mask) << 64) | (u128::from(layer) << 96)
}

/// `(layer, mask)` of a collider.
pub(crate) fn collider_layers(collider: &Collider) -> (u32, u32) {
    let data = collider.user_data;
    ((data >> 96) as u32, (data >> 64) as u32)
}

/// Godot's rule: two objects interact when either one's mask contains the other's layer.
struct LayerFilter;

impl LayerFilter {
    fn allowed(context: &PairFilterContext<'_>) -> bool {
        let (Some(a), Some(b)) = (
            context.colliders.get(context.collider1),
            context.colliders.get(context.collider2),
        ) else {
            return false;
        };
        let ((layer_a, mask_a), (layer_b, mask_b)) = (collider_layers(a), collider_layers(b));
        mask_a & layer_b != 0 || mask_b & layer_a != 0
    }
}

impl PhysicsHooks for LayerFilter {
    fn filter_contact_pair(&self, context: &PairFilterContext<'_>) -> Option<SolverFlags> {
        Self::allowed(context).then_some(SolverFlags::COMPUTE_RIGID_IMPULSES)
    }

    fn filter_intersection_pair(&self, context: &PairFilterContext<'_>) -> bool {
        Self::allowed(context)
    }
}

/// Splits a global transform into scale and the rigid pose Rapier simulates.
///
/// Always finite: a node scaled to zero (a common "shrink, then free" effect) or a NaN from a
/// script would otherwise reach Rapier and spread through every body it touches.
fn decompose(global: &Affine3A) -> (Vec3, Pose) {
    let (scale, rotation, position) = global.to_scale_rotation_translation();
    let length = rotation.length();
    let rotation = if rotation.is_finite() && length > 1e-6 {
        rotation / length
    } else {
        Quat::IDENTITY
    };
    let finite = |v: Vec3| if v.is_finite() { v } else { Vec3::ZERO };
    (
        finite(scale),
        Pose {
            position: finite(position),
            rotation,
        },
    )
}

fn body_type(kind: BodyKind) -> RigidBodyType {
    match kind {
        BodyKind::Static => RigidBodyType::Fixed,
        BodyKind::Rigid => RigidBodyType::Dynamic,
        BodyKind::Kinematic | BodyKind::Character | BodyKind::Area => {
            RigidBodyType::KinematicPositionBased
        }
    }
}

fn apply_body_settings(body: &mut RigidBody, settings: &BodySettings) {
    if body.body_type() != body_type(settings.kind) {
        body.set_body_type(body_type(settings.kind), true);
    }
    body.set_gravity_scale(settings.gravity_scale, true);
    body.set_linear_damping(settings.linear_damping);
    body.set_angular_damping(settings.angular_damping);
    body.lock_rotations(settings.lock_rotation, true);
    body.enable_ccd(settings.continuous_cd);
    *body.activation_mut() = if settings.can_sleep {
        RigidBodyActivation::active()
    } else {
        RigidBodyActivation::cannot_sleep()
    };
}

fn pair_key(a: ColliderHandle, b: ColliderHandle) -> (ColliderHandle, ColliderHandle) {
    if a.into_raw_parts() <= b.into_raw_parts() {
        (a, b)
    } else {
        (b, a)
    }
}

/// Adds or removes one collider pair from a per-body-pair count. Returns whether the body
/// pair started (0 → 1) or ended (1 → 0).
fn bump(counts: &mut HashMap<(Entity, Entity), u32>, key: (Entity, Entity), started: bool) -> bool {
    if started {
        let count = counts.entry(key).or_insert(0);
        *count += 1;
        *count == 1
    } else {
        match counts.get_mut(&key) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => {
                counts.remove(&key);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degenerate_transforms_decompose_to_finite_poses() {
        let zero_scale = Affine3A::from_scale_rotation_translation(
            Vec3::ZERO,
            Quat::from_rotation_y(1.0),
            Vec3::new(1.0, 2.0, 3.0),
        );
        let nan = Affine3A::from_translation(Vec3::new(f32::NAN, 0.0, 0.0));
        for global in [zero_scale, nan, Affine3A::IDENTITY] {
            let (scale, pose) = decompose(&global);
            assert!(scale.is_finite() && pose.position.is_finite() && pose.rotation.is_finite());
            assert!((pose.rotation.length() - 1.0).abs() < 1e-4);
        }
        assert_eq!(decompose(&zero_scale).1.position, Vec3::new(1.0, 2.0, 3.0));
    }
}
