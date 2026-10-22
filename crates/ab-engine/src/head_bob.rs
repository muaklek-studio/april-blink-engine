//! A camera that moves like a head: bobbing with the steps, swaying, rolling into turns and
//! strafes, dipping on landings, breathing when still; and, for full-body first person,
//! riding on the character's head bone.
//!
//! Put a [`HeadBob`] on a pivot node between the character and its camera:
//!
//! ```text
//! Player (CharacterBody3D, yaw)
//!   Head (Node3D at eye height, pitch)   the script turns these for mouse look
//!     Bob (HeadBob)                      the engine moves this one
//!       Camera (Camera3D)
//! ```
//!
//! The bob comes from the body's real velocity, so it matches walking, running, stopping
//! and falling without any calls from the game. The engine runs [`head_bob_system`] each
//! frame.

use ab_core::{Affine3A, Quat, Time, Vec3};
use ab_ecs::{Entity, World};
use ab_physics::{CharacterBody3D, PhysicsBody3D};
use ab_scene::{NodeHandle, Parent, Transform};

/// Head motion for the camera pivot it is on (see the module docs). Distances in metres,
/// angles in degrees.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadBob {
    /// The body whose movement drives the bob; `None`: the nearest ancestor with a
    /// `CharacterBody3D` or physics body.
    pub body: Option<NodeHandle>,
    /// A node to ride on (the character's head bone), kept at its position plus
    /// `follow_offset` (in the bone's frame); `None`: stay where the pivot is placed.
    pub follow: Option<NodeHandle>,
    pub follow_offset: Vec3,
    /// Up-and-down per step at walking speed.
    pub height: f32,
    /// Side-to-side sway per stride.
    pub width: f32,
    /// Roll into each step.
    pub roll: f32,
    /// Metres per step: sets the step rate from the speed.
    pub step_length: f32,
    /// Speed (m/s) the `height`, `width` and `roll` are for; faster moves bob more, up to
    /// twice as much.
    pub reference_speed: f32,
    /// Roll towards the side the character strafes to.
    pub strafe_tilt: f32,
    /// How far the head dips when landing, per m/s of the fall.
    pub landing_dip: f32,
    /// Breathing motion when standing still.
    pub breathing: f32,
    /// How quickly the motion follows changes of speed (per second).
    pub responsiveness: f32,
    state: BobState,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct BobState {
    /// Step cycle, radians (one full cycle = two steps).
    phase: f32,
    /// 0..2: how much of the bob shows, eased towards the speed.
    intensity: f32,
    strafe: f32,
    /// Landing dip: offset and its velocity (a spring).
    dip: f32,
    dip_speed: f32,
    breath: f32,
    was_on_floor: bool,
    last_vertical: f32,
    initialized: bool,
    /// The offset applied last frame, taken off to find the pivot's own place.
    applied: Vec3,
}

impl Default for HeadBob {
    fn default() -> Self {
        Self {
            body: None,
            follow: None,
            follow_offset: Vec3::ZERO,
            height: 0.035,
            width: 0.025,
            roll: 0.6,
            step_length: 0.75,
            reference_speed: 1.6,
            strafe_tilt: 1.5,
            landing_dip: 0.025,
            breathing: 0.004,
            responsiveness: 8.0,
            state: BobState::default(),
        }
    }
}

impl HeadBob {
    /// A calmer bob, for third-person or cinematic cameras.
    pub fn subtle() -> Self {
        Self {
            height: 0.015,
            width: 0.01,
            roll: 0.25,
            strafe_tilt: 0.8,
            landing_dip: 0.012,
            ..Self::default()
        }
    }

    /// The offset and roll (radians) applied this frame.
    pub fn current(&self) -> (Vec3, f32) {
        offset(self)
    }
}

/// The pivot's offset from its rest place and its roll, from the state.
fn offset(bob: &HeadBob) -> (Vec3, f32) {
    let s = &bob.state;
    let step = s.phase;
    // Down at each footfall (twice a cycle), sway once a cycle.
    let up = -bob.height * s.intensity * (1.0 - (step * 2.0).cos()) * 0.5;
    let side = bob.width * s.intensity * step.sin();
    let breath = bob.breathing * (1.0 - s.intensity.min(1.0)) * s.breath.sin();
    let roll = (bob.roll * s.intensity * step.sin() - bob.strafe_tilt * s.strafe).to_radians();
    (Vec3::new(side, up + breath - s.dip, 0.0), roll)
}

/// Advances every [`HeadBob`] and moves its node. The engine runs it in `Update`.
pub fn head_bob_system(world: &mut World) {
    let delta = world.resource::<Time>().map_or(0.0, |t| t.delta);
    let entities: Vec<Entity> = match world.storage::<HeadBob>() {
        Some(bobs) => bobs.iter().map(|(e, _)| e).collect(),
        None => return,
    };
    for entity in entities {
        step_one(world, entity, delta);
    }
}

fn step_one(world: &World, entity: Entity, delta: f32) {
    let Some(mut bob) = world.get::<HeadBob>(entity).map(|b| b.clone()) else {
        return;
    };
    let body = bob
        .body
        .map(|n| n.entity())
        .or_else(|| ancestors(world, entity).find(|&e| is_body(world, e)));
    let (velocity, on_floor) = body.map_or((Vec3::ZERO, true), |b| motion(world, b));
    let body_global = body.map_or(Affine3A::IDENTITY, |b| global_of(world, b));
    let right = body_global.transform_vector3(Vec3::X).normalize_or(Vec3::X);
    let horizontal = Vec3::new(velocity.x, 0.0, velocity.z);
    let speed = horizontal.length();

    let s = &mut bob.state;
    if !s.initialized {
        s.initialized = true;
        s.was_on_floor = on_floor;
    }
    let ease = 1.0 - (-bob.responsiveness * delta).exp();
    let target = if on_floor {
        (speed / bob.reference_speed.max(0.1)).min(2.0)
    } else {
        0.0
    };
    s.intensity += (target - s.intensity) * ease;
    let strafe = if speed > 0.1 {
        horizontal.dot(right) / speed.max(1.0)
    } else {
        0.0
    };
    s.strafe += (strafe.clamp(-1.0, 1.0) - s.strafe) * ease;
    if on_floor && speed > 0.05 {
        // A step every `step_length` metres; a cycle is two steps.
        s.phase = (s.phase + speed / bob.step_length.max(0.05) * std::f32::consts::PI * delta)
            .rem_euclid(std::f32::consts::TAU);
    } else {
        // Settle back to the neutral point of the cycle.
        let target = if s.phase > std::f32::consts::PI {
            std::f32::consts::TAU
        } else {
            0.0
        };
        s.phase += (target - s.phase) * ease;
    }
    s.breath = (s.breath + delta * 1.6).rem_euclid(std::f32::consts::TAU);
    // Landing: kick the dip spring by the fall speed.
    if on_floor && !s.was_on_floor && s.last_vertical < -2.0 {
        s.dip_speed += -s.last_vertical * bob.landing_dip * 10.0;
    }
    s.was_on_floor = on_floor;
    s.last_vertical = velocity.y;
    // Critically damped-ish spring back to 0.
    let (stiffness, damping) = (90.0, 14.0);
    s.dip_speed += (-stiffness * s.dip - damping * s.dip_speed) * delta;
    s.dip += s.dip_speed * delta;

    let (offset, roll) = offset(&bob);
    // Rest place: riding on the followed node, or where the pivot was placed.
    let parent_global = world
        .get::<Parent>(entity)
        .map_or(Affine3A::IDENTITY, |p| global_of(world, p.0.entity()));
    let base = match bob.follow {
        Some(follow) => {
            let head = global_of(world, follow.entity());
            let at = head.transform_point3(bob.follow_offset);
            Some(parent_global.inverse().transform_point3(at))
        }
        None => None,
    };
    if let Some(mut transform) = world.get_mut::<Transform>(entity) {
        // The pivot's own placement, without last frame's motion.
        let rest = base.unwrap_or(transform.local.translation - bob.state.applied);
        transform.local.translation = rest + offset;
        transform.local.rotation = Quat::from_rotation_z(roll);
        transform.dirty = true;
    }
    bob.state.applied = offset;
    if let Some(mut stored) = world.get_mut::<HeadBob>(entity) {
        stored.state = bob.state;
    }
}

fn ancestors(world: &World, entity: Entity) -> impl Iterator<Item = Entity> + '_ {
    std::iter::successors(
        world.get::<Parent>(entity).map(|p| p.0.entity()),
        move |&e| world.get::<Parent>(e).map(|p| p.0.entity()),
    )
}

fn is_body(world: &World, entity: Entity) -> bool {
    world.has::<CharacterBody3D>(entity) || world.has::<PhysicsBody3D>(entity)
}

/// Velocity and whether it stands on something.
fn motion(world: &World, body: Entity) -> (Vec3, bool) {
    if let Some(character) = world.get::<CharacterBody3D>(body) {
        return (character.velocity, character.is_on_floor());
    }
    if let Some(physics) = world.get::<PhysicsBody3D>(body) {
        return (physics.linear_velocity(), true);
    }
    (Vec3::ZERO, true)
}

/// A node's global transform from the locals up its parents (current even before this
/// frame's propagation).
fn global_of(world: &World, entity: Entity) -> Affine3A {
    let mut global = Affine3A::IDENTITY;
    let mut current = Some(entity);
    while let Some(e) = current {
        if let Some(t) = world.get::<Transform>(e) {
            global = t.local.to_affine() * global;
        }
        current = world.get::<Parent>(e).map(|p| p.0.entity());
    }
    global
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, EngineConfig};
    use ab_physics::PhysicsNodes;
    use ab_scene::SceneTree;

    const DT: f32 = 1.0 / 60.0;

    /// Player (character) → Head at eye height → Bob (HeadBob); returns (engine, player, bob).
    fn rig() -> (Engine, NodeHandle, NodeHandle) {
        let mut engine = Engine::new(&EngineConfig {
            audio_output: ab_audio::AudioOutput::Silent,
            ..Default::default()
        });
        let tree = engine.tree_mut();
        let player = tree.create_character_body3d("Player", CharacterBody3D::default());
        tree.add_child(tree.root(), player).unwrap();
        let head = tree.create_node3d("Head");
        tree.add_child(player, head).unwrap();
        tree.set_position(head, Vec3::new(0.0, 0.7, 0.0)).unwrap();
        let bob = tree.create_node3d("Bob");
        tree.add_child(head, bob).unwrap();
        tree.insert_component(bob, HeadBob::default()).unwrap();
        (engine, player, bob)
    }

    /// Makes the character "move" (velocity only, standing on a floor) for `frames`.
    fn walk(
        engine: &mut Engine,
        player: NodeHandle,
        velocity: Vec3,
        frames: usize,
    ) -> Vec<(Vec3, f32)> {
        let mut out = Vec::new();
        for _ in 0..frames {
            set_motion(engine.tree_mut(), player, velocity, true);
            head_bob_system(engine.tree_mut().world_mut());
            let bob = engine
                .tree()
                .world()
                .get::<HeadBob>(bob_of(engine.tree(), player).entity())
                .unwrap()
                .clone();
            out.push(bob.current());
        }
        out
    }

    fn bob_of(tree: &SceneTree, player: NodeHandle) -> NodeHandle {
        tree.get_node(player, "Head/Bob").unwrap()
    }

    fn set_motion(tree: &mut SceneTree, player: NodeHandle, velocity: Vec3, on_floor: bool) {
        let mut character = tree.component_mut::<CharacterBody3D>(player).unwrap();
        character.velocity = velocity;
        character.set_on_floor(on_floor);
    }

    fn time(engine: &mut Engine) {
        engine.tree().world().resource_mut::<Time>().unwrap().delta = DT;
    }

    #[test]
    fn walking_bobs_running_bobs_more_and_standing_settles() {
        let (mut engine, player, _) = rig();
        time(&mut engine);
        let still = walk(&mut engine, player, Vec3::ZERO, 60);
        let walking = walk(&mut engine, player, Vec3::new(0.0, 0.0, -1.6), 120);
        let running = walk(&mut engine, player, Vec3::new(0.0, 0.0, -4.0), 120);
        let range = |samples: &[(Vec3, f32)]| {
            let ys: Vec<f32> = samples[60..].iter().map(|(o, _)| o.y).collect();
            ys.iter().copied().fold(f32::MIN, f32::max)
                - ys.iter().copied().fold(f32::MAX, f32::min)
        };
        assert!(range(&still) < 0.01, "only breathing: {}", range(&still));
        assert!(range(&walking) > 0.025, "steps: {}", range(&walking));
        assert!(
            range(&running) > range(&walking) * 1.5,
            "{} vs {}",
            range(&running),
            range(&walking)
        );
        // Strafing right rolls the head right (negative roll about Z).
        let strafing = walk(&mut engine, player, Vec3::new(3.0, 0.0, 0.0), 120);
        let average_roll: f32 = strafing[60..].iter().map(|(_, r)| r).sum::<f32>() / 60.0;
        assert!(average_roll < -0.005, "{average_roll}");
        let settled = walk(&mut engine, player, Vec3::ZERO, 120);
        assert!(
            settled.last().unwrap().0.length() < 0.006,
            "{:?}",
            settled.last()
        );
    }

    #[test]
    fn landing_dips_and_springs_back() {
        let (mut engine, player, bob) = rig();
        time(&mut engine);
        for _ in 0..10 {
            set_motion(engine.tree_mut(), player, Vec3::new(0.0, -8.0, 0.0), false);
            head_bob_system(engine.tree_mut().world_mut());
        }
        let mut lowest = 0.0_f32;
        for _ in 0..60 {
            set_motion(engine.tree_mut(), player, Vec3::ZERO, true);
            head_bob_system(engine.tree_mut().world_mut());
            let y = engine.tree().transform(bob).unwrap().translation.y;
            lowest = lowest.min(y);
        }
        assert!(lowest < -0.05, "dipped: {lowest}");
        let y = engine.tree().transform(bob).unwrap().translation.y;
        assert!(y.abs() < 0.01, "back up: {y}");
    }

    #[test]
    fn rides_on_a_followed_head_bone() {
        let (mut engine, player, bob) = rig();
        time(&mut engine);
        let tree = engine.tree_mut();
        let bone = tree.create_node3d("HeadBone");
        tree.add_child(player, bone).unwrap();
        tree.set_position(bone, Vec3::new(0.1, 0.8, 0.05)).unwrap();
        tree.component_mut::<HeadBob>(bob).unwrap().follow = Some(bone);
        head_bob_system(tree.world_mut());
        tree.update_transforms();
        let at = tree.global_position(bob).unwrap();
        assert!(at.distance(Vec3::new(0.1, 0.8, 0.05)) < 0.01, "{at}");
    }
}
