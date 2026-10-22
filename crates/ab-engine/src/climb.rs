//! Climbing: walls and ladders, and pulling up over the top, for `CharacterBody3D`s.
//!
//! ```no_run
//! use ab_engine::prelude::*;
//! use ab_engine::climb::{ClimbState, Climbing};
//!
//! fn physics_process(tree: &mut SceneTree, player: NodeHandle, input: Vec2, jump: bool, dt: f32) {
//!     if tree.climb_state(player) == ClimbState::None {
//!         // Walking (move_and_slide as usual); grab a wall when pushing into one.
//!         if input.y > 0.5 {
//!             tree.try_grab(player);
//!         }
//!     } else {
//!         // Climbing: up/down, sideways, jump off. It moves the body itself.
//!         tree.climb_step(player, input, jump, dt);
//!     }
//! }
//! ```
//!
//! Walls are found with physics raycasts in front of the character's chest. By default only
//! bodies in the `climbable` group can be climbed (put walls, ladders and vines in it);
//! [`Climber::group`] `None` makes every wall climbable. Reaching the top of a wall with room
//! above it, the character pulls itself up and stands on it ([`ClimbState::Mantling`]).
//!
//! The character's size is read from its capsule (or other) shape the first time; set a
//! [`Climber`] on the node to change speeds and distances.

use ab_core::{Vec2, Vec3};
use ab_physics::{
    CharacterBody3D, CollisionShape3D, PhysicsNodes, PhysicsWorld, QueryOptions, Shape,
};
use ab_scene::{NodeHandle, SceneTree};

/// What a climbing character is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClimbState {
    /// Not climbing (walking, falling: the game moves it).
    #[default]
    None,
    /// On a wall.
    Climbing,
    /// Pulling itself up over the top of the wall.
    Mantling,
}

/// Climbing settings and state of a `CharacterBody3D` ([`Climbing`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Climber {
    /// Climbing speed in m/s.
    pub speed: f32,
    /// How far in front of the body a wall can be grabbed, in metres.
    pub reach: f32,
    /// Seconds to pull up over a ledge.
    pub mantle_time: f32,
    /// Speed away from the wall when jumping off (plus a little upwards).
    pub jump_off_speed: f32,
    /// Only bodies in this group are climbable (`None`: any wall).
    pub group: Option<String>,
    /// Physics layers that count as walls.
    pub collision_mask: u32,
    /// Body size: radius, height, and the feet's height relative to the node's origin
    /// (read from the shape when the climber is added).
    pub radius: f32,
    pub height: f32,
    pub feet: f32,
    state: ClimbState,
    /// Out of the wall.
    normal: Vec3,
    mantle: Option<Mantle>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Mantle {
    from: Vec3,
    to: Vec3,
    elapsed: f32,
}

impl Default for Climber {
    fn default() -> Self {
        Self {
            speed: 1.4,
            reach: 0.45,
            mantle_time: 0.5,
            jump_off_speed: 4.0,
            group: Some("climbable".into()),
            collision_mask: u32::MAX,
            radius: 0.35,
            height: 1.8,
            feet: -0.9,
            state: ClimbState::None,
            normal: Vec3::Z,
            mantle: None,
        }
    }
}

impl Climber {
    /// Sized for `shape` centred on the node.
    pub fn for_shape(shape: &Shape, offset: Vec3) -> Self {
        let (radius, height) = match *shape {
            Shape::Capsule { radius, height } | Shape::Cylinder { radius, height } => {
                (radius, height)
            }
            Shape::Box { size } => (size.x.max(size.z) * 0.5, size.y),
            Shape::Sphere { radius } => (radius, radius * 2.0),
            _ => (0.35, 1.8),
        };
        Self {
            radius,
            height,
            feet: offset.y - height * 0.5,
            ..Self::default()
        }
    }

    pub fn state(&self) -> ClimbState {
        self.state
    }

    /// The direction out of the wall being climbed.
    pub fn wall_normal(&self) -> Vec3 {
        self.normal
    }
}

/// A wall found in front of a character.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WallHit {
    pub point: Vec3,
    /// Out of the wall.
    pub normal: Vec3,
    pub distance: f32,
    pub body: NodeHandle,
}

/// Climbing on [`SceneTree`] (`use ab_engine::climb::Climbing;`).
pub trait Climbing {
    /// Grabs the climbable wall in front of `node` (its facing, -Z), if there is one within
    /// reach. Returns whether it is climbing now.
    fn try_grab(&mut self, node: NodeHandle) -> bool;

    /// Moves a climbing `node` for one physics step: `input.y` up/down, `input.x` right/left
    /// along the wall; `jump` jumps off. Pulls up over the top when it gets there. Returns
    /// the state after the step.
    fn climb_step(&mut self, node: NodeHandle, input: Vec2, jump: bool, delta: f32) -> ClimbState;

    /// Lets go of the wall.
    fn release_climb(&mut self, node: NodeHandle);

    fn climb_state(&self, node: NodeHandle) -> ClimbState;

    /// The climbable wall in front of `node`, `reach` metres beyond its body at most.
    fn wall_ahead(&self, node: NodeHandle, direction: Vec3) -> Option<WallHit>;
}

impl Climbing for SceneTree {
    fn try_grab(&mut self, node: NodeHandle) -> bool {
        ensure_climber(self, node);
        if self.climb_state(node) != ClimbState::None {
            return true;
        }
        let Some(forward) = self
            .global_transform(node)
            .map(|g| flat(g.transform_vector3(Vec3::NEG_Z)))
        else {
            return false;
        };
        let Some(wall) = self.wall_ahead(node, forward) else {
            return false;
        };
        if let Some(mut climber) = self.component_mut::<Climber>(node) {
            climber.state = ClimbState::Climbing;
            climber.normal = wall.normal;
        }
        if let Some(mut character) = self.component_mut::<CharacterBody3D>(node) {
            character.velocity = Vec3::ZERO;
        }
        face(self, node, -wall.normal);
        true
    }

    fn climb_step(&mut self, node: NodeHandle, input: Vec2, jump: bool, delta: f32) -> ClimbState {
        ensure_climber(self, node);
        let Some(climber) = self.component::<Climber>(node).map(|c| c.clone()) else {
            return ClimbState::None;
        };
        match climber.state {
            ClimbState::None => ClimbState::None,
            ClimbState::Mantling => mantle_step(self, node, &climber, delta),
            ClimbState::Climbing => climb(self, node, &climber, input, jump, delta),
        }
    }

    fn release_climb(&mut self, node: NodeHandle) {
        if let Some(mut climber) = self.component_mut::<Climber>(node) {
            climber.state = ClimbState::None;
            climber.mantle = None;
        }
    }

    fn climb_state(&self, node: NodeHandle) -> ClimbState {
        self.component::<Climber>(node)
            .map_or(ClimbState::None, |c| c.state)
    }

    fn wall_ahead(&self, node: NodeHandle, direction: Vec3) -> Option<WallHit> {
        let climber = self
            .component::<Climber>(node)
            .map(|c| c.clone())
            .unwrap_or_default();
        let origin = self.global_position(node)?;
        let chest = origin + Vec3::Y * (climber.feet + climber.height * 0.65);
        let direction = direction.try_normalize()?;
        let physics = self.world().resource::<PhysicsWorld>()?;
        let query = QueryOptions::new()
            .with_mask(climber.collision_mask)
            .excluding(node);
        let hit = physics.cast_ray(chest, direction, climber.radius + climber.reach, &query)?;
        // Walls only: not floors, ceilings or steep slopes.
        if hit.normal.y.abs() > 0.5 {
            return None;
        }
        if let Some(group) = &climber.group {
            let in_group = std::iter::successors(Some(hit.node), |&n| self.parent(n))
                .take(3)
                .any(|n| self.is_in_group(n, group));
            if !in_group {
                return None;
            }
        }
        Some(WallHit {
            point: hit.position,
            normal: flat(hit.normal),
            distance: hit.distance,
            body: hit.node,
        })
    }
}

/// `v` on the ground plane, normalised.
fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z).normalize_or(Vec3::NEG_Z)
}

/// Adds a [`Climber`] sized from the node's first shape if it has none.
fn ensure_climber(tree: &mut SceneTree, node: NodeHandle) {
    if tree.component::<Climber>(node).is_some() {
        return;
    }
    let climber = tree
        .children(node)
        .into_iter()
        .find_map(|c| {
            let shape = tree.component::<CollisionShape3D>(c)?.shape().clone();
            let offset = tree.transform(c).map_or(Vec3::ZERO, |t| t.translation);
            Some(Climber::for_shape(&shape, offset))
        })
        .unwrap_or_default();
    let _ = tree.insert_component(node, climber);
}

/// Turns `node` about Y to look along `direction`.
fn face(tree: &mut SceneTree, node: NodeHandle, direction: Vec3) {
    let yaw = (-direction.x).atan2(-direction.z);
    if let Some(mut transform) = tree.transform(node) {
        transform.rotation = ab_core::Quat::from_rotation_y(yaw);
        let _ = tree.set_transform(node, transform);
    }
}

fn set_state(tree: &mut SceneTree, node: NodeHandle, f: impl FnOnce(&mut Climber)) {
    if let Some(mut climber) = tree.component_mut::<Climber>(node) {
        f(&mut climber);
    }
}

fn climb(
    tree: &mut SceneTree,
    node: NodeHandle,
    climber: &Climber,
    input: Vec2,
    jump: bool,
    delta: f32,
) -> ClimbState {
    let normal = climber.normal;
    if jump {
        set_state(tree, node, |c| c.state = ClimbState::None);
        if let Some(mut character) = tree.component_mut::<CharacterBody3D>(node) {
            character.velocity = normal * climber.jump_off_speed + Vec3::Y * 3.0;
        }
        return ClimbState::None;
    }
    let Some(position) = tree.global_position(node) else {
        return ClimbState::None;
    };
    match tree.wall_ahead(node, -normal) {
        Some(wall) => {
            // Follow the wall's curve, and keep a hand's width from it.
            set_state(tree, node, |c| c.normal = wall.normal);
            let gap = wall.distance - (climber.radius + 0.05);
            let right = Vec3::Y.cross(wall.normal).normalize_or(Vec3::X);
            let along = Vec3::Y * input.y + right * input.x;
            let along = if along.length() > 1.0 {
                along.normalize()
            } else {
                along
            };
            let velocity =
                along * climber.speed - wall.normal * (gap / delta.max(1e-4)).clamp(-2.0, 2.0);
            if let Some(mut character) = tree.component_mut::<CharacterBody3D>(node) {
                character.velocity = velocity;
            }
            let _ = tree.move_and_slide(node, delta);
            face(tree, node, -wall.normal);
            // Climbing down onto the floor: let go.
            let on_floor = tree
                .component::<CharacterBody3D>(node)
                .is_some_and(|c| c.is_on_floor());
            if on_floor && input.y < 0.0 {
                set_state(tree, node, |c| c.state = ClimbState::None);
                return ClimbState::None;
            }
            ClimbState::Climbing
        }
        None => match ledge_top(tree, node, climber, position, normal) {
            // The wall ends at the chest: pull up onto the top.
            Some(top) if input.y > 0.0 => {
                let to = top + Vec3::Y * (-climber.feet + 0.02) - normal * climber.radius;
                set_state(tree, node, |c| {
                    c.state = ClimbState::Mantling;
                    c.mantle = Some(Mantle {
                        from: position,
                        to,
                        elapsed: 0.0,
                    });
                });
                ClimbState::Mantling
            }
            Some(_) => {
                if let Some(mut character) = tree.component_mut::<CharacterBody3D>(node) {
                    character.velocity = Vec3::ZERO;
                }
                ClimbState::Climbing
            }
            // Nothing to hold on to.
            None => {
                set_state(tree, node, |c| c.state = ClimbState::None);
                ClimbState::None
            }
        },
    }
}

/// The top of the wall just ahead and above the chest, with room to stand there.
fn ledge_top(
    tree: &SceneTree,
    node: NodeHandle,
    climber: &Climber,
    position: Vec3,
    normal: Vec3,
) -> Option<Vec3> {
    let physics = tree.world().resource::<PhysicsWorld>()?;
    let query = QueryOptions::new()
        .with_mask(climber.collision_mask)
        .excluding(node);
    let feet = position + Vec3::Y * climber.feet;
    // Down onto the top, from above where the hands are.
    let above = feet + Vec3::Y * (climber.height + 0.5) - normal * (climber.radius * 2.0 + 0.2);
    let top = physics.cast_ray(above, Vec3::NEG_Y, climber.height + 0.5, &query)?;
    let reachable = top.position.y > feet.y + climber.height * 0.4;
    if top.normal.y < 0.7 || !reachable {
        return None;
    }
    // Room to stand on it.
    let clear = physics
        .cast_ray(
            top.position + Vec3::Y * 0.05,
            Vec3::Y,
            climber.height * 0.9,
            &query,
        )
        .is_none();
    clear.then_some(top.position)
}

fn mantle_step(
    tree: &mut SceneTree,
    node: NodeHandle,
    climber: &Climber,
    delta: f32,
) -> ClimbState {
    let Some(mut mantle) = climber.mantle else {
        set_state(tree, node, |c| c.state = ClimbState::None);
        return ClimbState::None;
    };
    mantle.elapsed += delta;
    let t = (mantle.elapsed / climber.mantle_time.max(0.05)).min(1.0);
    // Up first, then over the edge.
    let rise = (t / 0.6).min(1.0);
    let over = ((t - 0.5) / 0.5).clamp(0.0, 1.0);
    let ease = |x: f32| x * x * (3.0 - 2.0 * x);
    let horizontal = mantle.from.lerp(mantle.to, ease(over));
    let y = mantle.from.y + (mantle.to.y - mantle.from.y) * ease(rise);
    let _ = tree.set_global_position(node, Vec3::new(horizontal.x, y, horizontal.z));
    if let Some(mut character) = tree.component_mut::<CharacterBody3D>(node) {
        character.velocity = Vec3::ZERO;
    }
    let done = t >= 1.0;
    set_state(tree, node, |c| {
        c.mantle = (!done).then_some(mantle);
        if done {
            c.state = ClimbState::None;
        }
    });
    if done {
        ClimbState::None
    } else {
        ClimbState::Mantling
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, EngineConfig};
    use ab_physics::PhysicsBody3D;

    const DT: f32 = 1.0 / 60.0;

    /// A floor, a 3 m wall in front of the character (at z = -1, top at y = 3), and the
    /// character standing at the origin facing -Z.
    fn scene(climbable: bool) -> (Engine, NodeHandle) {
        let mut engine = Engine::new(&EngineConfig {
            audio_output: ab_audio::AudioOutput::Silent,
            ..Default::default()
        });
        let tree = engine.tree_mut();
        let root = tree.root();
        let floor = tree.create_physics_body3d("Floor", PhysicsBody3D::fixed());
        tree.add_collision_shape(
            floor,
            Shape::Box {
                size: Vec3::new(30.0, 1.0, 30.0),
            },
        )
        .unwrap();
        tree.set_position(floor, Vec3::new(0.0, -0.5, 0.0)).unwrap();
        tree.add_child(root, floor).unwrap();
        let wall = tree.create_physics_body3d("Wall", PhysicsBody3D::fixed());
        tree.add_collision_shape(
            wall,
            Shape::Box {
                size: Vec3::new(6.0, 3.0, 1.0),
            },
        )
        .unwrap();
        tree.set_position(wall, Vec3::new(0.0, 1.5, -1.5)).unwrap();
        tree.add_child(root, wall).unwrap();
        if climbable {
            tree.add_to_group(wall, "climbable").unwrap();
        }
        let player = tree.create_character_body3d("Player", CharacterBody3D::default());
        tree.add_collision_shape(
            player,
            Shape::Capsule {
                radius: 0.3,
                height: 1.8,
            },
        )
        .unwrap();
        tree.set_position(player, Vec3::new(0.0, 0.9, -0.6))
            .unwrap();
        tree.add_child(root, player).unwrap();
        engine.update(DT);
        (engine, player)
    }

    #[test]
    fn climbs_a_wall_and_pulls_up_onto_it() {
        let (mut engine, player) = scene(true);
        let tree = engine.tree_mut();
        assert!(tree.try_grab(player));
        assert_eq!(tree.climb_state(player), ClimbState::Climbing);
        let climber = tree.component::<Climber>(player).unwrap().clone();
        assert_eq!(
            (climber.radius, climber.height),
            (0.3, 1.8),
            "sized from the capsule"
        );
        assert!((climber.wall_normal() - Vec3::Z).length() < 1e-3);
        let mut states = Vec::new();
        for _ in 0..400 {
            let state = engine
                .tree_mut()
                .climb_step(player, Vec2::new(0.0, 1.0), false, DT);
            states.push(state);
            engine.update(DT);
            if state == ClimbState::None {
                break;
            }
        }
        assert!(states.contains(&ClimbState::Mantling), "{states:?}");
        assert_eq!(states.last(), Some(&ClimbState::None));
        let at = engine.tree().global_position(player).unwrap();
        assert!(at.y > 3.5 && at.z < -1.0, "standing on top: {at}");
    }

    #[test]
    fn sideways_down_and_jumping_off() {
        let (mut engine, player) = scene(true);
        assert!(engine.tree_mut().try_grab(player));
        for _ in 0..30 {
            engine
                .tree_mut()
                .climb_step(player, Vec2::new(1.0, 0.5), false, DT);
            engine.update(DT);
        }
        let at = engine.tree().global_position(player).unwrap();
        assert!(at.x.abs() > 0.3 && at.y > 1.0, "moved along the wall: {at}");
        assert!((at.z + 0.65).abs() < 0.1, "kept its distance: {at}");
        let state = engine.tree_mut().climb_step(player, Vec2::ZERO, true, DT);
        assert_eq!(state, ClimbState::None);
        let velocity = engine
            .tree()
            .component::<CharacterBody3D>(player)
            .unwrap()
            .velocity;
        assert!(
            velocity.z > 3.0 && velocity.y > 0.0,
            "away from the wall: {velocity}"
        );
    }

    #[test]
    fn only_climbable_walls_can_be_grabbed() {
        let (mut engine, player) = scene(false);
        assert!(!engine.tree_mut().try_grab(player));
        engine
            .tree_mut()
            .component_mut::<Climber>(player)
            .unwrap()
            .group = None;
        assert!(
            engine.tree_mut().try_grab(player),
            "any wall when no group is required"
        );
    }
}
