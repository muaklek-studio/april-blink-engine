//! Baking from a physics scene and walking a character along the path.

use ab_core::{Time, Vec3};
use ab_navigation::{NavBakeSettings, NavGrid, PathFollower};
use ab_physics::{
    physics_system, CharacterBody3D, PhysicsBody3D, PhysicsNodes, PhysicsWorld, QueryOptions, Shape,
};
use ab_scene::{NodeHandle, SceneTree};

const DT: f32 = 1.0 / 60.0;

fn solid(tree: &mut SceneTree, size: Vec3, position: Vec3) -> NodeHandle {
    let node = tree.create_physics_body3d("Solid", PhysicsBody3D::fixed());
    tree.add_collision_shape(node, Shape::cuboid(size)).unwrap();
    tree.set_position(node, position).unwrap();
    tree.add_child(tree.root(), node).unwrap();
    node
}

/// 12×12 m floor; a wall across x = 0 from z = -6 to z = 3 (gap at the far end); a 1 m
/// high platform in one corner.
fn level() -> SceneTree {
    let mut tree = SceneTree::new();
    tree.world_mut().insert_resource(Time::new(DT));
    tree.world_mut().insert_resource(PhysicsWorld::new());
    solid(
        &mut tree,
        Vec3::new(12.0, 1.0, 12.0),
        Vec3::new(0.0, -0.5, 0.0),
    );
    solid(
        &mut tree,
        Vec3::new(0.4, 3.0, 9.0),
        Vec3::new(0.0, 1.5, -1.5),
    );
    solid(
        &mut tree,
        Vec3::new(2.0, 1.0, 2.0),
        Vec3::new(-5.0, 0.5, -5.0),
    );
    physics_system(tree.world_mut());
    tree
}

fn settings() -> NavBakeSettings {
    NavBakeSettings {
        min: Vec3::new(-6.0, -1.0, -6.0),
        max: Vec3::new(6.0, 5.0, 6.0),
        cell_size: 0.25,
        agent_radius: 0.3,
        ..Default::default()
    }
}

#[test]
fn bake_marks_floor_walls_and_ledges() {
    let tree = level();
    let grid = NavGrid::bake(
        &tree.world().resource::<PhysicsWorld>().unwrap(),
        &settings(),
    );
    assert_eq!((grid.width(), grid.depth()), (48, 48));
    let at = |x: f32, z: f32| grid.cell_at(Vec3::new(x, 0.0, z)).unwrap();
    assert_eq!(grid.height(at(3.0, 3.0)), Some(0.0), "open floor");
    assert!(!grid.is_walkable(at(0.0, 0.0)), "wall");
    assert!(
        !grid.is_walkable(at(0.35, 0.0)),
        "too close to the wall for the agent's radius"
    );
    assert!(grid.is_walkable(at(0.75, 0.0)));
    // The platform top is walkable but 1 m up: not reachable from the floor.
    let top = grid.height(at(-5.0, -5.0)).unwrap();
    assert!((top - 1.0).abs() < 1e-3);
    assert!(grid
        .find_path(Vec3::new(3.0, 0.0, 3.0), Vec3::new(-5.0, 1.0, -5.0))
        .is_none());
}

#[test]
fn a_character_walks_the_path_around_the_wall() {
    let mut tree = level();
    let grid = NavGrid::bake(
        &tree.world().resource::<PhysicsWorld>().unwrap(),
        &settings(),
    );
    let agent = tree.create_character_body3d("Agent", CharacterBody3D::default());
    tree.add_collision_shape(agent, Shape::capsule(0.3, 1.6))
        .unwrap();
    let start = Vec3::new(-2.0, 0.81, -4.0);
    let goal = Vec3::new(2.0, 0.0, -4.0);
    tree.set_position(agent, start).unwrap();
    tree.add_child(tree.root(), agent).unwrap();

    let path = grid.find_path(start, goal).expect("around the wall");
    assert!(path.iter().any(|p| p.z > 3.0), "uses the gap: {path:?}");
    let mut follower = PathFollower::new(0.15);
    follower.set_path(path);

    for _ in 0..1200 {
        let position = tree.position(agent).unwrap();
        let direction = follower.direction(position).unwrap_or(Vec3::ZERO);
        if let Some(mut body) = tree.component_mut::<CharacterBody3D>(agent) {
            let fall = body.velocity.y + PhysicsWorld::DEFAULT_GRAVITY.y * DT;
            body.velocity = direction * 3.0 + Vec3::Y * fall;
        }
        tree.move_and_slide(agent, DT).unwrap();
        physics_system(tree.world_mut());
        tree.process(DT);
        if follower.is_finished() {
            break;
        }
    }
    let end = tree.position(agent).unwrap();
    assert!(follower.is_finished(), "stuck at {end}");
    assert!(
        Vec3::new(end.x - goal.x, 0.0, end.z - goal.z).length() < 0.3,
        "{end}"
    );
    // It never went through the wall: still on the floor and the wall is still between
    // start and goal.
    let physics = tree.world().resource::<PhysicsWorld>().unwrap();
    assert!(!physics.has_line_of_sight(
        Vec3::new(-2.0, 1.0, -4.0),
        Vec3::new(2.0, 1.0, -4.0),
        &QueryOptions::new().excluding(agent)
    ));
}
