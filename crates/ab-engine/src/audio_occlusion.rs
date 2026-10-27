//! Muffles 3D sounds behind walls: each frame, before the audio update, a ray goes from
//! the listener to every voice that asks for occlusion
//! ([`ab_audio::Attenuation::occlusion`]).

use ab_audio::Audio;
use ab_core::Vec3;
use ab_ecs::World;
use ab_physics::{PhysicsWorld, QueryOptions};

/// Hits this close to the sound are its own body, not a wall.
const SOURCE_RADIUS: f32 = 0.5;
/// Steps past a shape the ray starts inside (the listener's own character).
const INSIDE_STEP: f32 = 0.3;

/// Whether solid geometry lies between `from` and `to`.
pub fn blocked(physics: &PhysicsWorld, from: Vec3, to: Vec3) -> bool {
    let options = QueryOptions::new();
    let mut origin = from;
    for _ in 0..3 {
        let offset = to - origin;
        let distance = offset.length();
        if distance <= SOURCE_RADIUS {
            return false;
        }
        let direction = offset / distance;
        let Some(hit) = physics.cast_ray(origin, direction, distance, &options) else {
            return false;
        };
        if hit.distance < 1e-3 {
            origin = hit.position + direction * INSIDE_STEP;
            continue;
        }
        return hit.position.distance(to) > SOURCE_RADIUS;
    }
    false
}

/// Reports to [`Audio`] which voices are blocked from the listener.
pub fn audio_occlusion_system(world: &mut World) {
    let world: &World = world;
    let (Some(mut audio), Some(physics)) = (
        world.resource_mut::<Audio>(),
        world.resource::<PhysicsWorld>(),
    ) else {
        return;
    };
    let listener = audio.listener_transform().0;
    for (voice, position) in audio.occlusion_requests() {
        let amount = if blocked(&physics, listener, position) {
            1.0
        } else {
            0.0
        };
        audio.set_occlusion(voice, amount);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_core::Time;
    use ab_physics::{physics_system, PhysicsBody3D, PhysicsNodes, Shape};
    use ab_scene::SceneTree;

    /// A 1 m thick wall across x = 5, and a box around the origin (the listener's body).
    fn level() -> SceneTree {
        let mut tree = SceneTree::new();
        tree.world_mut().insert_resource(Time::new(1.0 / 60.0));
        tree.world_mut().insert_resource(PhysicsWorld::new());
        for (name, size, position) in [
            ("Wall", Vec3::new(1.0, 10.0, 10.0), Vec3::new(5.0, 0.0, 0.0)),
            ("Player", Vec3::splat(0.5), Vec3::ZERO),
        ] {
            let node = tree.create_physics_body3d(name, PhysicsBody3D::fixed());
            tree.add_collision_shape(node, Shape::cuboid(size)).unwrap();
            tree.set_position(node, position).unwrap();
            tree.add_child(tree.root(), node).unwrap();
        }
        physics_system(tree.world_mut());
        tree.process(1.0 / 60.0);
        tree
    }

    #[test]
    fn walls_block_but_the_listeners_own_body_and_the_source_do_not() {
        let tree = level();
        let physics = tree.world().resource::<PhysicsWorld>().unwrap();
        assert!(
            blocked(&physics, Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)),
            "behind the wall"
        );
        assert!(
            !blocked(&physics, Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0)),
            "in front of it"
        );
        assert!(
            !blocked(&physics, Vec3::ZERO, Vec3::new(4.6, 0.0, 0.0)),
            "a sound on the wall's face is not behind it"
        );
        assert!(
            !blocked(&physics, Vec3::ZERO, Vec3::new(0.0, 0.0, 8.0)),
            "beside it"
        );
    }
}
