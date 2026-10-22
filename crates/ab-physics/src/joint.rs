//! Joints between two bodies (Godot's `Joint3D` family).

use ab_scene::NodeHandle;
use rapier3d::prelude::{ImpulseJointHandle, RigidBodyHandle};

/// How a [`Joint3D`] holds its two bodies together. The joint's node gives the anchor (its
/// position) and the axes (its rotation, sampled when the joint is created): hinges turn
/// around the node's X axis, cone-twist joints twist around it and swing away from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JointKind {
    /// Welded: no relative motion.
    Fixed,
    /// Ball and socket: free rotation around the anchor (Godot's `PinJoint3D`).
    Pin,
    /// Rotation around the X axis only, optionally limited to `[min, max]` radians
    /// (Godot's `HingeJoint3D`; elbows, knees, doors).
    Hinge { limits: Option<[f32; 2]> },
    /// Ball and socket with limits: swing up to `swing` radians away from the X axis and
    /// twist up to `twist` radians around it (Godot's `ConeTwistJoint3D`; shoulders, hips,
    /// necks).
    ConeTwist { swing: f32, twist: f32 },
}

/// Joins `body_a` and `body_b` (two [`PhysicsBody3D`](crate::PhysicsBody3D) nodes) at the
/// joint node's position. The joint is created on the first physics step where both bodies
/// are simulated, from their poses at that moment, and removed when the joint node or a
/// body leaves the tree. Changing any field re-creates it from the current poses.
#[derive(Debug, Clone, PartialEq)]
pub struct Joint3D {
    pub kind: JointKind,
    pub body_a: Option<NodeHandle>,
    pub body_b: Option<NodeHandle>,
    /// Let the two joined bodies collide with each other (off by default, like Godot's
    /// `exclude_nodes_from_collision`).
    pub collide_connected: bool,
    /// Turns the joint off without removing it.
    pub disabled: bool,
    pub(crate) state: Option<JointState>,
}

impl Joint3D {
    pub fn new(kind: JointKind, body_a: NodeHandle, body_b: NodeHandle) -> Self {
        Self {
            kind,
            body_a: Some(body_a),
            body_b: Some(body_b),
            collide_connected: false,
            disabled: false,
            state: None,
        }
    }

    /// A joint whose bodies are set later.
    pub fn unconnected(kind: JointKind) -> Self {
        Self {
            kind,
            body_a: None,
            body_b: None,
            collide_connected: false,
            disabled: false,
            state: None,
        }
    }

    pub fn pin(body_a: NodeHandle, body_b: NodeHandle) -> Self {
        Self::new(JointKind::Pin, body_a, body_b)
    }

    pub fn fixed(body_a: NodeHandle, body_b: NodeHandle) -> Self {
        Self::new(JointKind::Fixed, body_a, body_b)
    }

    /// A hinge limited to `[min, max]` degrees.
    pub fn hinge(
        body_a: NodeHandle,
        body_b: NodeHandle,
        min_degrees: f32,
        max_degrees: f32,
    ) -> Self {
        Self::new(
            JointKind::Hinge {
                limits: Some([min_degrees.to_radians(), max_degrees.to_radians()]),
            },
            body_a,
            body_b,
        )
    }

    /// A cone-twist joint with limits in degrees.
    pub fn cone_twist(
        body_a: NodeHandle,
        body_b: NodeHandle,
        swing_degrees: f32,
        twist_degrees: f32,
    ) -> Self {
        Self::new(
            JointKind::ConeTwist {
                swing: swing_degrees.to_radians(),
                twist: twist_degrees.to_radians(),
            },
            body_a,
            body_b,
        )
    }

    /// Whether the joint exists in the simulation.
    pub fn is_active(&self) -> bool {
        self.state.is_some()
    }

    pub(crate) fn settings(&self) -> JointSettings {
        JointSettings {
            kind: self.kind,
            collide_connected: self.collide_connected,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct JointSettings {
    pub kind: JointKind,
    pub collide_connected: bool,
}

/// Link between a joint component and its Rapier joint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct JointState {
    pub handle: ImpulseJointHandle,
    pub bodies: (RigidBodyHandle, RigidBodyHandle),
    pub settings: JointSettings,
}
