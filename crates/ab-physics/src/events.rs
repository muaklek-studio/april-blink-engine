use ab_scene::NodeHandle;
use rapier3d::prelude::{
    ColliderSet, CollisionEvent, ContactPair, EventHandler, Real, RigidBodySet, SoftBodySet,
    SoftBodyTearEvent,
};
use std::sync::Mutex;

/// Something that happened during the last physics step. Read them with
/// [`PhysicsWorld::events`](crate::PhysicsWorld::events) from `physics_process`; each step
/// replaces the previous step's list.
///
/// Nodes are the body nodes (not the shape nodes). Bodies with several shapes report one
/// start and one end per pair of bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhysicsEvent {
    /// Two solid bodies started touching.
    ContactStarted { a: NodeHandle, b: NodeHandle },
    /// Two solid bodies stopped touching (including when one was removed).
    ContactEnded { a: NodeHandle, b: NodeHandle },
    /// `body` (any body, or another area) started overlapping `area`: Godot's
    /// `body_entered` / `area_entered`.
    BodyEntered { area: NodeHandle, body: NodeHandle },
    /// `body` stopped overlapping `area` (including when either was removed).
    BodyExited { area: NodeHandle, body: NodeHandle },
}

impl PhysicsEvent {
    /// Whether `node` is one of the two nodes involved.
    pub fn involves(&self, node: NodeHandle) -> bool {
        match *self {
            Self::ContactStarted { a, b } | Self::ContactEnded { a, b } => a == node || b == node,
            Self::BodyEntered { area, body } | Self::BodyExited { area, body } => {
                area == node || body == node
            }
        }
    }
}

/// Collects Rapier's collision events during a step. Rapier requires `Sync` handlers.
#[derive(Default)]
pub(crate) struct EventCollector {
    events: Mutex<Vec<CollisionEvent>>,
}

impl EventCollector {
    pub fn drain(&self) -> Vec<CollisionEvent> {
        std::mem::take(&mut *self.lock())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<CollisionEvent>> {
        // A panic while holding the lock cannot leave the Vec inconsistent.
        self.events.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl EventHandler for EventCollector {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        event: CollisionEvent,
        _contact_pair: Option<&ContactPair>,
    ) {
        self.lock().push(event);
    }

    fn handle_contact_force_event(
        &self,
        _dt: Real,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        _contact_pair: &ContactPair,
        _total_force_magnitude: Real,
    ) {
    }

    fn handle_soft_body_tear_event(&self, _soft_bodies: &SoftBodySet, _event: &SoftBodyTearEvent) {}
}
