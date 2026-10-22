//! Physics: scene queries, bodies, characters and ragdolls.

use crate::host::method;
use crate::math::Vec3;
use crate::tree::Tree;
use crate::value::{Arg, Error, IntoValue, Node};
use ab_script_api as ffi;

/// Filters for scene queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryOptions {
    /// Bodies to ignore (usually the one asking).
    pub exclude: Vec<Node>,
    /// Collision layers to hit (bit mask).
    pub collision_mask: u32,
    /// Hit areas (sensors) too.
    pub include_areas: bool,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            exclude: Vec::new(),
            collision_mask: u32::MAX,
            include_areas: false,
        }
    }
}

impl QueryOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn excluding(mut self, node: Node) -> Self {
        self.exclude.push(node);
        self
    }

    pub fn with_mask(mut self, mask: u32) -> Self {
        self.collision_mask = mask;
        self
    }

    pub fn with_areas(mut self, include: bool) -> Self {
        self.include_areas = include;
        self
    }

    /// The options as the trailing arguments every query takes.
    fn args(&self) -> Vec<ffi::Value> {
        let mut args = vec![
            self.collision_mask.to_value(),
            self.include_areas.to_value(),
        ];
        args.extend(self.exclude.iter().map(Node::to_value));
        args
    }
}

/// What a ray hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// The body (not the collision shape).
    pub node: Node,
    pub position: Vec3,
    pub normal: Vec3,
    pub distance: f32,
}

/// Contacts and overlaps that started or ended during the last physics step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhysicsEvent {
    ContactStarted {
        a: Node,
        b: Node,
    },
    ContactEnded {
        a: Node,
        b: Node,
    },
    /// `body` entered the area `area`.
    BodyEntered {
        area: Node,
        body: Node,
    },
    BodyExited {
        area: Node,
        body: Node,
    },
}

/// Signals physics bodies and areas emit (with the other body as argument).
pub mod signals {
    /// A body started touching this body, or entered this area.
    pub const BODY_ENTERED: &str = "body_entered";
    pub const BODY_EXITED: &str = "body_exited";
}

/// How a body moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BodyKind {
    /// Never moves (walls, floors).
    Static,
    /// Simulated: gravity, collisions, impulses.
    Rigid,
    /// Moved by code; pushes rigid bodies.
    Kinematic,
    /// A character moved with `move_and_slide`.
    Character,
    /// Detects overlaps, blocks nothing.
    Area,
}

impl BodyKind {
    fn name(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Rigid => "rigid",
            Self::Kinematic => "kinematic",
            Self::Character => "character",
            Self::Area => "area",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "static" => Self::Static,
            "rigid" => Self::Rigid,
            "kinematic" => Self::Kinematic,
            "character" => Self::Character,
            "area" => Self::Area,
            _ => return None,
        })
    }
}

/// Scene queries ([`Tree::physics`]).
#[derive(Debug)]
pub struct Physics<'t, 'a> {
    tree: &'t Tree<'a>,
}

impl Physics<'_, '_> {
    /// The closest hit along a ray from `origin` towards `direction`, up to `max_distance`.
    pub fn cast_ray(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        options: &QueryOptions,
    ) -> Option<RayHit> {
        let mut args = vec![
            origin.to_value(),
            direction.to_value(),
            max_distance.to_value(),
        ];
        args.extend(options.args());
        let values = self.tree.values(method!("physics.cast_ray"), &args).ok()?;
        match values.as_slice() {
            [Arg::Node(node), Arg::Vec3(position), Arg::Vec3(normal), distance] => Some(RayHit {
                node: *node,
                position: *position,
                normal: *normal,
                distance: distance.get()?,
            }),
            _ => None,
        }
    }

    /// Whether nothing blocks the line between two points.
    pub fn has_line_of_sight(&self, from: Vec3, to: Vec3, options: &QueryOptions) -> bool {
        let mut args = vec![from.to_value(), to.to_value()];
        args.extend(options.args());
        self.tree
            .query(method!("physics.line_of_sight"), &args)
            .unwrap_or(false)
    }

    /// Bodies with a shape overlapping the sphere.
    pub fn intersect_sphere(&self, center: Vec3, radius: f32, options: &QueryOptions) -> Vec<Node> {
        let mut args = vec![center.to_value(), radius.to_value()];
        args.extend(options.args());
        self.tree.list(method!("physics.intersect_sphere"), &args)
    }

    /// Contacts and overlaps of the last physics step.
    pub fn events(&self) -> Vec<PhysicsEvent> {
        let values = self
            .tree
            .values(method!("physics.events"), &[])
            .unwrap_or_default();
        values
            .as_chunks::<3>()
            .0
            .iter()
            .filter_map(|event| match event {
                [Arg::Str(kind), Arg::Node(a), Arg::Node(b)] => Some(match kind.as_str() {
                    "contact_started" => PhysicsEvent::ContactStarted { a: *a, b: *b },
                    "contact_ended" => PhysicsEvent::ContactEnded { a: *a, b: *b },
                    "body_entered" => PhysicsEvent::BodyEntered { area: *a, body: *b },
                    "body_exited" => PhysicsEvent::BodyExited { area: *a, body: *b },
                    _ => return None,
                }),
                _ => None,
            })
            .collect()
    }

    /// Every body of `kind` in the tree.
    pub fn bodies(&self, kind: BodyKind) -> Vec<Node> {
        self.tree
            .list(method!("physics.bodies"), &[kind.name().to_value()])
    }

    /// Gravity (m/s²).
    pub fn gravity(&self) -> Vec3 {
        self.tree
            .query(method!("physics.gravity"), &[])
            .unwrap_or(Vec3::new(0.0, -9.81, 0.0))
    }
}

/// A `PhysicsBody3D` ([`Tree::body`]): static, rigid, kinematic bodies and areas.
#[derive(Debug)]
pub struct Body<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

macro_rules! property {
    ($get:ident, $set:ident, $t:ty, $path:literal, $doc:literal) => {
        #[doc = $doc]
        pub fn $get(&self) -> Option<$t> {
            self.tree.get(self.node, $path)
        }

        #[doc = $doc]
        pub fn $set(&mut self, value: $t) -> Result<(), Error> {
            self.tree.set(self.node, $path, value)
        }
    };
}
pub(crate) use property;

impl Body<'_, '_> {
    pub fn kind(&self) -> Option<BodyKind> {
        self.tree
            .get::<String>(self.node, "body.kind")
            .and_then(|k| BodyKind::parse(&k))
    }

    pub fn set_kind(&mut self, kind: BodyKind) -> Result<(), Error> {
        self.tree.set(self.node, "body.kind", kind.name())
    }

    property!(
        mass,
        set_mass,
        f32,
        "body.mass",
        "Mass in kg (rigid bodies)."
    );
    property!(
        linear_velocity,
        set_linear_velocity,
        Vec3,
        "body.linear_velocity",
        "Velocity in m/s."
    );
    property!(
        angular_velocity,
        set_angular_velocity,
        Vec3,
        "body.angular_velocity",
        "Spin in rad/s around each axis."
    );
    property!(
        collision_layer,
        set_collision_layer,
        u32,
        "body.collision_layer",
        "Layers the body is on (bit mask)."
    );
    property!(
        collision_mask,
        set_collision_mask,
        u32,
        "body.collision_mask",
        "Layers the body collides with (bit mask)."
    );
    property!(
        gravity_scale,
        set_gravity_scale,
        f32,
        "body.gravity_scale",
        "1 = normal gravity, 0 = none."
    );
    property!(
        linear_damping,
        set_linear_damping,
        f32,
        "body.linear_damping",
        "Slows movement over time."
    );
    property!(
        angular_damping,
        set_angular_damping,
        f32,
        "body.angular_damping",
        "Slows spinning over time."
    );
    property!(
        friction,
        set_friction,
        f32,
        "body.friction",
        "Surface friction of its shapes."
    );
    property!(
        bounce,
        set_bounce,
        f32,
        "body.bounce",
        "Restitution of its shapes (0 = no bounce)."
    );

    /// Asleep: at rest and skipped by the simulation until something wakes it.
    pub fn is_sleeping(&self) -> bool {
        self.tree.get(self.node, "body.sleeping").unwrap_or(false)
    }

    /// A push through the center of mass (N·s).
    pub fn apply_impulse(&mut self, impulse: Vec3) -> Result<(), Error> {
        self.tree.run(
            method!("body.apply_impulse"),
            &[self.node.to_value(), impulse.to_value()],
        )
    }

    /// A push at a world point (also spins the body).
    pub fn apply_impulse_at(&mut self, impulse: Vec3, point: Vec3) -> Result<(), Error> {
        self.tree.run(
            method!("body.apply_impulse_at"),
            &[self.node.to_value(), impulse.to_value(), point.to_value()],
        )
    }

    pub fn apply_torque_impulse(&mut self, impulse: Vec3) -> Result<(), Error> {
        self.tree.run(
            method!("body.apply_torque_impulse"),
            &[self.node.to_value(), impulse.to_value()],
        )
    }

    /// A force for the next physics step only (call it every step to keep pushing).
    pub fn apply_force(&mut self, force: Vec3) -> Result<(), Error> {
        self.tree.run(
            method!("body.apply_force"),
            &[self.node.to_value(), force.to_value()],
        )
    }

    pub fn apply_torque(&mut self, torque: Vec3) -> Result<(), Error> {
        self.tree.run(
            method!("body.apply_torque"),
            &[self.node.to_value(), torque.to_value()],
        )
    }

    /// Bodies currently touching this one.
    pub fn colliding_bodies(&self) -> Vec<Node> {
        self.tree
            .list(method!("body.colliding_bodies"), &[self.node.to_value()])
    }

    /// Bodies inside this area.
    pub fn overlapping_bodies(&self) -> Vec<Node> {
        self.tree
            .list(method!("body.overlapping_bodies"), &[self.node.to_value()])
    }
}

/// A `CharacterBody3D` ([`Tree::character`]).
#[derive(Debug)]
pub struct Character<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl Character<'_, '_> {
    property!(
        velocity,
        set_velocity,
        Vec3,
        "character.velocity",
        "Velocity `move_and_slide` moves by (m/s)."
    );
    property!(
        up,
        set_up,
        Vec3,
        "character.up",
        "Which way is up (floors face it)."
    );
    property!(
        floor_max_angle,
        set_floor_max_angle,
        f32,
        "character.floor_max_angle",
        "Steepest walkable slope, radians."
    );
    property!(
        floor_snap_length,
        set_floor_snap_length,
        f32,
        "character.floor_snap_length",
        "Keeps the character on slopes and steps going down (m)."
    );
    property!(
        max_step_height,
        set_max_step_height,
        f32,
        "character.max_step_height",
        "Climbs steps up to this height (m; 0 = off)."
    );
    property!(
        push_rigid_bodies,
        set_push_rigid_bodies,
        bool,
        "character.push_rigid_bodies",
        "Pushes rigid bodies it walks into."
    );

    pub fn is_on_floor(&self) -> bool {
        self.tree
            .get(self.node, "character.on_floor")
            .unwrap_or(false)
    }

    pub fn is_on_wall(&self) -> bool {
        self.tree
            .get(self.node, "character.on_wall")
            .unwrap_or(false)
    }

    pub fn is_on_ceiling(&self) -> bool {
        self.tree
            .get(self.node, "character.on_ceiling")
            .unwrap_or(false)
    }

    /// Normal of the floor it stands on (its `up` when airborne).
    pub fn floor_normal(&self) -> Vec3 {
        self.tree
            .get(self.node, "character.floor_normal")
            .unwrap_or(Vec3::Y)
    }

    /// Moves by `velocity * delta`, sliding along what it hits. Returns whether it hit
    /// something.
    pub fn move_and_slide(&mut self, delta: f32) -> Result<bool, Error> {
        Ok(self
            .tree
            .fetch(
                method!("character.move_and_slide"),
                &[self.node.to_value(), delta.to_value()],
            )?
            .unwrap_or(false))
    }
}

impl<'a> Tree<'a> {
    /// Scene queries: raycasts, overlaps, contact events.
    pub fn physics(&self) -> Physics<'_, 'a> {
        Physics { tree: self }
    }

    /// The node's `PhysicsBody3D`.
    pub fn body(&mut self, node: Node) -> Body<'_, 'a> {
        Body { tree: self, node }
    }

    /// The node's `CharacterBody3D`.
    pub fn character(&mut self, node: Node) -> Character<'_, 'a> {
        Character { tree: self, node }
    }

    /// Shortcut for `tree.character(node).move_and_slide(delta)`.
    pub fn move_and_slide(&mut self, node: Node, delta: f32) -> Result<bool, Error> {
        self.character(node).move_and_slide(delta)
    }

    /// The ragdoll (`Ragdoll` node) that `node` is a part of, if any.
    pub fn ragdoll_of(&self, node: Node) -> Option<Node> {
        self.query(method!("ragdoll.of"), &[node.to_value()])
    }

    /// Makes a ragdoll go limp (fully simulated).
    pub fn go_limp(&mut self, ragdoll: Node) -> Result<(), Error> {
        self.run(method!("ragdoll.go_limp"), &[ragdoll.to_value()])
    }

    pub fn is_ragdoll_active(&self, ragdoll: Node) -> bool {
        self.query(method!("ragdoll.is_active"), &[ragdoll.to_value()])
            .unwrap_or(false)
    }
}
