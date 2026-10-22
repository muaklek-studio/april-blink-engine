//! Scene-level physics tests: nodes in a `SceneTree`, stepped with `physics_system`.

use ab_core::{Quat, Time, Vec3};
use ab_physics::{
    physics_system, CharacterBody3D, PhysicsBody3D, PhysicsEvent, PhysicsNodes, PhysicsWorld,
    QueryOptions, Shape,
};
use ab_scene::{NodeHandle, SceneTree};

const DT: f32 = 1.0 / 60.0;

fn new_tree() -> SceneTree {
    let mut tree = SceneTree::new();
    tree.world_mut().insert_resource(Time::new(DT));
    tree.world_mut().insert_resource(PhysicsWorld::new());
    tree
}

/// Runs one fixed step followed by the end-of-frame update (transforms, deferred frees).
fn step(tree: &mut SceneTree) {
    physics_system(tree.world_mut());
    tree.process(DT);
}

fn steps(tree: &mut SceneTree, count: usize) {
    for _ in 0..count {
        step(tree);
    }
}

fn physics(tree: &SceneTree) -> std::cell::Ref<'_, PhysicsWorld> {
    tree.world().resource::<PhysicsWorld>().unwrap()
}

fn spawn_body(
    tree: &mut SceneTree,
    name: &str,
    body: PhysicsBody3D,
    shape: Shape,
    position: Vec3,
) -> NodeHandle {
    let node = tree.create_physics_body3d(name, body);
    tree.add_collision_shape(node, shape).unwrap();
    tree.set_position(node, position).unwrap();
    tree.add_child(tree.root(), node).unwrap();
    node
}

/// 40 x 1 x 40 static slab whose top face is at y = 0.
fn spawn_floor(tree: &mut SceneTree) -> NodeHandle {
    spawn_body(
        tree,
        "Floor",
        PhysicsBody3D::fixed(),
        Shape::cuboid(Vec3::new(40.0, 1.0, 40.0)),
        Vec3::new(0.0, -0.5, 0.0),
    )
}

#[test]
fn cube_falls_with_gravity_and_rests_on_the_floor() {
    let mut tree = new_tree();
    let floor = spawn_floor(&mut tree);
    let cube = spawn_body(
        &mut tree,
        "Cube",
        PhysicsBody3D::rigid(),
        Shape::cuboid(Vec3::ONE),
        Vec3::new(0.0, 5.0, 0.0),
    );

    steps(&mut tree, 10);
    let falling = tree.position(cube).unwrap().y;
    assert!(falling < 5.0, "gravity pulls the cube down, y = {falling}");
    assert!(
        tree.component::<PhysicsBody3D>(cube)
            .unwrap()
            .linear_velocity()
            .y
            < -1.0
    );

    steps(&mut tree, 170);
    let resting = tree.position(cube).unwrap();
    assert!(
        (resting.y - 0.5).abs() < 0.02,
        "cube rests on the floor at y = 0.5, got {resting}"
    );
    // It hits at ~9 m/s and sinks ~10 cm into the floor in one step; Rapier pushes it out
    // with a small sideways settle (about 1.5 cm since Rapier 0.35), then it stays put.
    assert!(
        resting.x.abs() < 0.03 && resting.z.abs() < 0.03,
        "{resting}"
    );
    assert_eq!(physics(&tree).colliding_bodies(cube), [floor]);
    // The floor never moves.
    assert_eq!(tree.position(floor).unwrap(), Vec3::new(0.0, -0.5, 0.0));
}

#[test]
fn contact_events_report_body_nodes_once_per_pair() {
    let mut tree = new_tree();
    let floor = spawn_floor(&mut tree);
    // Two shapes on one body: still one contact pair with the floor.
    let dumbbell = spawn_body(
        &mut tree,
        "Dumbbell",
        PhysicsBody3D::rigid(),
        Shape::sphere(0.25),
        Vec3::new(0.0, 1.0, 0.0),
    );
    let second = tree
        .add_collision_shape(dumbbell, Shape::sphere(0.25))
        .unwrap();
    tree.set_position(second, Vec3::new(1.0, 0.0, 0.0)).unwrap();

    let mut started = Vec::new();
    for _ in 0..120 {
        step(&mut tree);
        started.extend(
            physics(&tree)
                .events()
                .iter()
                .filter(|e| matches!(e, PhysicsEvent::ContactStarted { .. }))
                .copied(),
        );
    }
    let (a, b) = (floor.min(dumbbell), floor.max(dumbbell));
    assert_eq!(started, [PhysicsEvent::ContactStarted { a, b }]);

    // Freeing the body ends the contact.
    tree.queue_free(dumbbell).unwrap();
    step(&mut tree); // the node is freed at the end of this frame
    step(&mut tree); // the next step removes its body
    assert!(physics(&tree)
        .events()
        .contains(&PhysicsEvent::ContactEnded { a, b }));
    assert!(physics(&tree).colliding_bodies(floor).is_empty());
    assert_eq!(physics(&tree).body_count(), 1);
}

#[test]
fn raycast_hits_the_closest_object() {
    let mut tree = new_tree();
    let near = spawn_body(
        &mut tree,
        "Near",
        PhysicsBody3D::fixed(),
        Shape::cuboid(Vec3::ONE),
        Vec3::new(0.0, 0.0, -5.0),
    );
    let far = spawn_body(
        &mut tree,
        "Far",
        PhysicsBody3D::fixed().with_collision(2, 2),
        Shape::sphere(1.0),
        Vec3::new(0.0, 0.0, -10.0),
    );
    let trigger = spawn_body(
        &mut tree,
        "Trigger",
        PhysicsBody3D::area(),
        Shape::cuboid(Vec3::ONE),
        Vec3::new(0.0, 0.0, -2.0),
    );
    step(&mut tree);

    let physics = physics(&tree);
    let options = QueryOptions::new();
    let hit = physics
        .cast_ray(Vec3::ZERO, Vec3::NEG_Z, 100.0, &options)
        .expect("the ray hits the near box");
    assert_eq!(hit.node, near);
    assert_eq!(hit.shape, tree.get_node(near, "CollisionShape3D").unwrap());
    assert!((hit.distance - 4.5).abs() < 1e-4, "{hit:?}");
    assert!(hit.position.abs_diff_eq(Vec3::new(0.0, 0.0, -4.5), 1e-4));
    assert!(hit.normal.abs_diff_eq(Vec3::Z, 1e-4));

    // Excluding the near box, or filtering by layer, reaches the sphere behind it.
    let behind = physics
        .cast_ray(
            Vec3::ZERO,
            Vec3::NEG_Z,
            100.0,
            &options.clone().excluding(near),
        )
        .unwrap();
    assert_eq!(behind.node, far);
    assert!((behind.distance - 9.0).abs() < 1e-4);
    let layer_two = physics.cast_ray(
        Vec3::ZERO,
        Vec3::NEG_Z,
        100.0,
        &QueryOptions::new().with_mask(2),
    );
    assert_eq!(layer_two.map(|h| h.node), Some(far));

    // Areas are only hit on request.
    let area_hit = physics
        .cast_ray(
            Vec3::ZERO,
            Vec3::NEG_Z,
            100.0,
            &QueryOptions::new().with_areas(true),
        )
        .unwrap();
    assert_eq!(area_hit.node, trigger);

    // Too short, wrong direction, zero direction.
    assert!(physics
        .cast_ray(Vec3::ZERO, Vec3::NEG_Z, 4.0, &options)
        .is_none());
    assert!(physics
        .cast_ray(Vec3::ZERO, Vec3::Z, 100.0, &options)
        .is_none());
    assert!(physics
        .cast_ray(Vec3::ZERO, Vec3::ZERO, 100.0, &options)
        .is_none());

    // Line of sight and overlap queries.
    let target = Vec3::new(0.0, 0.0, -10.0);
    assert!(!physics.has_line_of_sight(Vec3::ZERO, target, &options.clone().excluding(far)));
    assert!(physics.has_line_of_sight(
        Vec3::ZERO,
        target,
        &options.clone().excluding(far).excluding(near)
    ));
    assert!(physics.has_line_of_sight(
        Vec3::new(5.0, 0.0, 0.0),
        Vec3::new(5.0, 0.0, -20.0),
        &options
    ));
    assert_eq!(
        physics.intersect_sphere(Vec3::new(0.0, 0.0, -7.5), 2.2, &options),
        [near.min(far), near.max(far)]
    );
    assert!(physics
        .intersect_sphere(Vec3::new(0.0, 5.0, 0.0), 1.0, &options)
        .is_empty());
}

#[test]
fn trigger_volume_fires_when_the_player_enters_and_leaves() {
    let mut tree = new_tree();
    spawn_floor(&mut tree);
    let area = spawn_body(
        &mut tree,
        "Checkpoint",
        PhysicsBody3D::area(),
        Shape::cuboid(Vec3::new(2.0, 3.0, 2.0)),
        // Hovering just above the floor; touching it would report the floor too.
        Vec3::new(5.0, 1.6, 0.0),
    );
    let player = tree.create_character_body3d(
        "Player",
        CharacterBody3D::default().with_velocity(Vec3::new(4.0, 0.0, 0.0)),
    );
    tree.add_collision_shape(player, Shape::capsule(0.4, 1.8))
        .unwrap();
    tree.set_position(player, Vec3::new(0.0, 0.9, 0.0)).unwrap();
    tree.add_child(tree.root(), player).unwrap();

    let mut events = Vec::new();
    let mut entered_at = None;
    for frame in 0..150 {
        tree.move_and_slide(player, DT).unwrap();
        step(&mut tree);
        for &event in physics(&tree).events() {
            if event == (PhysicsEvent::BodyEntered { area, body: player }) {
                entered_at = Some(tree.position(player).unwrap().x);
            }
            events.push((frame, event));
        }
        if frame == 75 {
            assert_eq!(physics(&tree).overlapping_bodies(area), [player]);
        }
    }
    let kinds: Vec<_> = events.iter().map(|(_, e)| *e).collect();
    assert_eq!(
        kinds,
        [
            PhysicsEvent::BodyEntered { area, body: player },
            PhysicsEvent::BodyExited { area, body: player },
        ]
    );
    // The capsule (radius 0.4) touches the area (x from 4 to 6) at about x = 3.6.
    let entered_at = entered_at.unwrap();
    assert!(
        (3.5..3.8).contains(&entered_at),
        "entered at x = {entered_at}"
    );
    assert!(!physics(&tree).is_overlapping(area, player));
    // Areas never block movement.
    assert!(tree.position(player).unwrap().x > 9.0);
}

#[test]
fn character_lands_on_the_floor_and_slides_along_walls() {
    let mut tree = new_tree();
    spawn_floor(&mut tree);
    let wall = spawn_body(
        &mut tree,
        "Wall",
        PhysicsBody3D::fixed(),
        Shape::cuboid(Vec3::new(1.0, 4.0, 20.0)),
        Vec3::new(3.0, 2.0, 0.0),
    );
    let player = tree.create_character_body3d("Player", CharacterBody3D::default());
    tree.add_collision_shape(player, Shape::capsule(0.5, 2.0))
        .unwrap();
    tree.set_position(player, Vec3::new(0.0, 3.0, 0.0)).unwrap();
    tree.add_child(tree.root(), player).unwrap();

    let gravity = PhysicsWorld::DEFAULT_GRAVITY;
    let tick = |tree: &mut SceneTree, horizontal: Vec3| {
        {
            let mut character = tree.component_mut::<CharacterBody3D>(player).unwrap();
            character.velocity += gravity * DT;
            character.velocity.x = horizontal.x;
            character.velocity.z = horizontal.z;
        }
        let collided = tree.move_and_slide(player, DT).unwrap();
        step(tree);
        collided
    };

    // Fall onto the floor.
    for _ in 0..90 {
        tick(&mut tree, Vec3::ZERO);
    }
    let character = tree.component::<CharacterBody3D>(player).unwrap().clone();
    let position = tree.position(player).unwrap();
    assert!(character.is_on_floor());
    assert!(
        (position.y - 1.0).abs() < 0.05,
        "capsule bottom on the floor: {position}"
    );
    assert!(
        character.velocity.y.abs() < 1e-3,
        "landing cancels the fall: {:?}",
        character.velocity
    );
    assert!(character.floor_normal().abs_diff_eq(Vec3::Y, 1e-3));

    // Walk diagonally into the wall (inner face at x = 2.5): blocked in x, keeps moving in z.
    for _ in 0..90 {
        tick(&mut tree, Vec3::new(3.0, 0.0, -3.0));
    }
    let character = tree.component::<CharacterBody3D>(player).unwrap().clone();
    let position = tree.position(player).unwrap();
    assert!(
        position.x < 2.0 && position.x > 1.9,
        "stopped at the wall: {position}"
    );
    assert!(position.z < -3.0, "slid along the wall: {position}");
    assert!(character.is_on_wall() && character.is_on_floor());
    assert!(
        character.velocity.x.abs() < 1e-3,
        "{:?}",
        character.velocity
    );
    assert!(physics(&tree)
        .cast_ray(
            position,
            Vec3::X,
            1.0,
            &QueryOptions::new().excluding(player)
        )
        .is_some_and(|hit| hit.node == wall));

    // Jump: leaves the floor.
    tree.component_mut::<CharacterBody3D>(player)
        .unwrap()
        .velocity
        .y = 5.0;
    tick(&mut tree, Vec3::ZERO);
    tick(&mut tree, Vec3::ZERO);
    assert!(!tree
        .component::<CharacterBody3D>(player)
        .unwrap()
        .is_on_floor());
    assert!(tree.position(player).unwrap().y > 1.1);
}

#[test]
fn character_pushes_rigid_bodies() {
    let mut tree = new_tree();
    spawn_floor(&mut tree);
    let ball = spawn_body(
        &mut tree,
        "Ball",
        PhysicsBody3D::rigid().with_mass(0.5),
        Shape::sphere(0.5),
        Vec3::new(2.0, 0.5, 0.0),
    );
    let player = tree.create_character_body3d(
        "Player",
        CharacterBody3D::default().with_velocity(Vec3::new(3.0, 0.0, 0.0)),
    );
    tree.add_collision_shape(player, Shape::capsule(0.4, 1.8))
        .unwrap();
    tree.set_position(player, Vec3::new(0.0, 0.92, 0.0))
        .unwrap();
    tree.add_child(tree.root(), player).unwrap();

    steps(&mut tree, 1);
    for _ in 0..60 {
        tree.component_mut::<CharacterBody3D>(player)
            .unwrap()
            .velocity = Vec3::new(3.0, 0.0, 0.0);
        tree.move_and_slide(player, DT).unwrap();
        step(&mut tree);
    }
    assert!(
        tree.position(ball).unwrap().x > 2.5,
        "{}",
        tree.position(ball).unwrap()
    );
}

#[test]
fn impulses_and_mass() {
    let mut tree = new_tree();
    tree.world().resource_mut::<PhysicsWorld>().unwrap().gravity = Vec3::ZERO;
    let light = spawn_body(
        &mut tree,
        "Light",
        PhysicsBody3D::rigid().with_mass(1.0),
        Shape::sphere(0.5),
        Vec3::ZERO,
    );
    // Same mass on two shapes of different size.
    let heavy = spawn_body(
        &mut tree,
        "Heavy",
        PhysicsBody3D::rigid().with_mass(4.0),
        Shape::cuboid(Vec3::ONE),
        Vec3::new(0.0, 10.0, 0.0),
    );
    tree.add_collision_shape(heavy, Shape::sphere(0.2)).unwrap();

    for node in [light, heavy] {
        tree.component_mut::<PhysicsBody3D>(node)
            .unwrap()
            .apply_central_impulse(Vec3::new(2.0, 0.0, 0.0));
    }
    step(&mut tree);
    let velocity = |tree: &SceneTree, node| {
        tree.component::<PhysicsBody3D>(node)
            .unwrap()
            .linear_velocity()
    };
    assert!(velocity(&tree, light).abs_diff_eq(Vec3::new(2.0, 0.0, 0.0), 1e-3));
    assert!(velocity(&tree, heavy).abs_diff_eq(Vec3::new(0.5, 0.0, 0.0), 1e-3));

    // Velocity overrides and one-step forces.
    tree.component_mut::<PhysicsBody3D>(light)
        .unwrap()
        .set_linear_velocity(Vec3::new(0.0, 0.0, -1.0));
    step(&mut tree);
    assert!(velocity(&tree, light).abs_diff_eq(Vec3::new(0.0, 0.0, -1.0), 1e-3));
    tree.component_mut::<PhysicsBody3D>(light)
        .unwrap()
        .apply_central_force(Vec3::new(0.0, 60.0, 0.0));
    step(&mut tree);
    step(&mut tree);
    // 60 N on 1 kg for one 1/60 s step: +1 m/s, and the force does not persist.
    assert!(velocity(&tree, light).abs_diff_eq(Vec3::new(0.0, 1.0, -1.0), 1e-3));
}

#[test]
fn kinematic_and_static_bodies_follow_their_nodes() {
    let mut tree = new_tree();
    let platform = spawn_body(
        &mut tree,
        "Platform",
        PhysicsBody3D::kinematic(),
        Shape::cuboid(Vec3::new(2.0, 0.2, 2.0)),
        Vec3::ZERO,
    );
    let pillar = spawn_body(
        &mut tree,
        "Pillar",
        PhysicsBody3D::fixed(),
        Shape::cuboid(Vec3::ONE),
        Vec3::new(10.0, 0.0, 0.0),
    );
    step(&mut tree);

    let down = |tree: &SceneTree, x: f32| {
        physics(tree)
            .cast_ray(
                Vec3::new(x, 5.0, 0.0),
                Vec3::NEG_Y,
                10.0,
                &QueryOptions::new(),
            )
            .map(|hit| (hit.node, hit.position.y))
    };
    assert_eq!(down(&tree, 0.0).map(|h| h.0), Some(platform));

    tree.set_position(platform, Vec3::new(0.0, 2.0, 0.0))
        .unwrap();
    tree.set_position(pillar, Vec3::new(-10.0, 0.0, 0.0))
        .unwrap();
    step(&mut tree);
    let (node, y) = down(&tree, 0.0).unwrap();
    assert_eq!(node, platform);
    assert!((y - 2.1).abs() < 1e-4, "platform top at 2.1, hit {y}");
    assert!(down(&tree, 10.0).is_none());
    assert_eq!(down(&tree, -10.0).map(|h| h.0), Some(pillar));
    // Physics never moves them on its own.
    assert_eq!(tree.position(platform).unwrap(), Vec3::new(0.0, 2.0, 0.0));
}

#[test]
fn teleporting_a_rigid_body_and_nested_shapes() {
    let mut tree = new_tree();
    spawn_floor(&mut tree);
    // Body rotated 90° about Y with its shape offset 2 m along its local X.
    let body = tree.create_physics_body3d("Body", PhysicsBody3D::rigid());
    let offset = tree.add_collision_shape(body, Shape::sphere(0.5)).unwrap();
    tree.set_position(offset, Vec3::new(2.0, 0.0, 0.0)).unwrap();
    tree.set_position(body, Vec3::new(0.0, 0.5, 0.0)).unwrap();
    tree.set_rotation(body, Quat::from_rotation_y(std::f32::consts::FRAC_PI_2))
        .unwrap();
    tree.add_child(tree.root(), body).unwrap();
    step(&mut tree);

    let hit = physics(&tree)
        .cast_ray(
            Vec3::new(0.0, 5.0, -2.0),
            Vec3::NEG_Y,
            10.0,
            &QueryOptions::new(),
        )
        .unwrap();
    assert_eq!(hit.node, body);
    assert_eq!(hit.shape, offset);

    tree.set_position(body, Vec3::new(20.0, 0.5, 0.0)).unwrap();
    step(&mut tree);
    let moved = tree.position(body).unwrap();
    assert!((moved.x - 20.0).abs() < 1e-3, "teleported: {moved}");
}

#[test]
fn bodies_leave_the_simulation_with_their_nodes() {
    let mut tree = new_tree();
    let holder = tree.create_node3d("Holder");
    tree.add_child(tree.root(), holder).unwrap();
    let area = spawn_body(
        &mut tree,
        "Area",
        PhysicsBody3D::area(),
        Shape::sphere(2.0),
        Vec3::ZERO,
    );
    let crate_body = tree.create_physics_body3d("Crate", PhysicsBody3D::rigid().with_mass(2.0));
    tree.add_collision_shape(crate_body, Shape::cuboid(Vec3::ONE))
        .unwrap();
    tree.add_child(holder, crate_body).unwrap();
    tree.world().resource_mut::<PhysicsWorld>().unwrap().gravity = Vec3::ZERO;

    step(&mut tree);
    assert_eq!(physics(&tree).body_count(), 2);
    assert_eq!(physics(&tree).collider_count(), 2);
    assert_eq!(physics(&tree).overlapping_bodies(area), [crate_body]);

    // Detached: removed from the simulation, the area sees it leave.
    tree.remove_child(holder, crate_body).unwrap();
    step(&mut tree);
    assert_eq!(physics(&tree).body_count(), 1);
    assert_eq!(physics(&tree).collider_count(), 1);
    assert!(physics(&tree).events().contains(&PhysicsEvent::BodyExited {
        area,
        body: crate_body
    }));
    assert!(!tree
        .component::<PhysicsBody3D>(crate_body)
        .unwrap()
        .is_simulated());

    // Re-attached: back in, entering again.
    tree.add_child(holder, crate_body).unwrap();
    step(&mut tree);
    assert_eq!(physics(&tree).body_count(), 2);
    assert!(physics(&tree)
        .events()
        .contains(&PhysicsEvent::BodyEntered {
            area,
            body: crate_body
        }));

    // Disabling the shape removes just the collider; freeing the area ends the overlap.
    let shape = tree.get_node(crate_body, "CollisionShape3D").unwrap();
    tree.component_mut::<ab_physics::CollisionShape3D>(shape)
        .unwrap()
        .disabled = true;
    step(&mut tree);
    assert_eq!(physics(&tree).collider_count(), 1);
    assert!(physics(&tree).overlapping_bodies(area).is_empty());

    tree.queue_free(holder).unwrap();
    tree.queue_free(area).unwrap();
    tree.process(DT);
    step(&mut tree);
    assert_eq!(physics(&tree).body_count(), 0);
    assert_eq!(physics(&tree).collider_count(), 0);
}

#[test]
fn shape_changes_rebuild_colliders() {
    let mut tree = new_tree();
    let body = spawn_body(
        &mut tree,
        "Body",
        PhysicsBody3D::fixed(),
        Shape::sphere(1.0),
        Vec3::ZERO,
    );
    step(&mut tree);
    let top = |tree: &SceneTree| {
        physics(tree)
            .cast_ray(
                Vec3::new(0.0, 10.0, 0.0),
                Vec3::NEG_Y,
                20.0,
                &QueryOptions::new(),
            )
            .map(|hit| hit.position.y)
    };
    assert!((top(&tree).unwrap() - 1.0).abs() < 1e-4);

    let shape = tree.get_node(body, "CollisionShape3D").unwrap();
    tree.component_mut::<ab_physics::CollisionShape3D>(shape)
        .unwrap()
        .set_shape(Shape::sphere(2.0));
    step(&mut tree);
    assert!((top(&tree).unwrap() - 2.0).abs() < 1e-4);

    // Scaling the node scales the shape.
    tree.set_scale(body, Vec3::splat(0.5)).unwrap();
    step(&mut tree);
    assert!((top(&tree).unwrap() - 1.0).abs() < 1e-4);

    // Degenerate geometry is dropped instead of crashing.
    tree.component_mut::<ab_physics::CollisionShape3D>(shape)
        .unwrap()
        .set_shape(Shape::convex_hull(vec![Vec3::ZERO, Vec3::X]));
    step(&mut tree);
    assert!(top(&tree).is_none());
    assert_eq!(physics(&tree).collider_count(), 0);
}

#[test]
fn move_and_slide_errors() {
    let mut tree = new_tree();
    let plain = tree.create_node3d("Plain");
    assert_eq!(
        tree.move_and_slide(plain, DT),
        Err(ab_physics::PhysicsError::NotACharacter(plain))
    );
    let node = tree.create_node("NotSpatial");
    assert!(tree.move_and_slide(node, DT).is_err());

    // Before its first physics step a character moves freely.
    let player =
        tree.create_character_body3d("Player", CharacterBody3D::default().with_velocity(Vec3::X));
    assert_eq!(tree.move_and_slide(player, 0.5), Ok(false));
    assert_eq!(tree.position(player).unwrap(), Vec3::new(0.5, 0.0, 0.0));
}

#[test]
fn areas_and_bodies_emit_signals() {
    use ab_scene::{NodeScript, Signal};

    #[derive(Default)]
    struct Log(Vec<(&'static str, NodeHandle)>);
    impl NodeScript for Log {
        fn on_signal(&mut self, _: &mut SceneTree, _: NodeHandle, signal: &Signal) {
            self.0
                .push((signal.name, *signal.arg::<NodeHandle>().unwrap()));
        }
    }

    let mut tree = new_tree();
    let floor = spawn_floor(&mut tree);
    let area = spawn_body(
        &mut tree,
        "Area",
        PhysicsBody3D::area(),
        Shape::cuboid(Vec3::new(2.0, 2.0, 2.0)),
        Vec3::new(0.0, 3.0, 0.0),
    );
    let ball = spawn_body(
        &mut tree,
        "Ball",
        PhysicsBody3D::rigid(),
        Shape::sphere(0.25),
        Vec3::new(0.0, 6.0, 0.0),
    );
    let watcher = tree.create_node("Watcher");
    tree.set_script(watcher, Log::default()).unwrap();
    tree.add_child(tree.root(), watcher).unwrap();
    for (source, name) in [
        (area, ab_physics::signals::BODY_ENTERED),
        (area, ab_physics::signals::BODY_EXITED),
        (floor, ab_physics::signals::BODY_ENTERED),
    ] {
        tree.connect(source, name, watcher);
    }
    // `step` flushes signals at the start of each process.
    steps(&mut tree, 150);
    let log = tree.script::<Log>(watcher).unwrap().0.clone();
    assert_eq!(
        log,
        [
            ("body_entered", ball),
            ("body_exited", ball),
            ("body_entered", ball),
        ],
        "fell through the area, then landed on the floor"
    );
}

#[test]
fn layers_follow_godot_rules() {
    let mut tree = new_tree();
    spawn_floor(&mut tree); // layer 1, mask 1
                            // Either mask selecting the other's layer is enough.
    let seen_by_its_mask = spawn_body(
        &mut tree,
        "A",
        PhysicsBody3D::rigid().with_collision(4, 1),
        Shape::sphere(0.5),
        Vec3::new(-2.0, 3.0, 0.0),
    );
    let seen_by_nobody = spawn_body(
        &mut tree,
        "B",
        PhysicsBody3D::rigid().with_collision(4, 0),
        Shape::sphere(0.5),
        Vec3::new(2.0, 3.0, 0.0),
    );
    // A character on layer 4 with mask 1 stands on the floor.
    let walker = tree.create_character_body3d("Walker", CharacterBody3D::default());
    tree.add_collision_shape(walker, Shape::capsule(0.4, 1.8))
        .unwrap();
    tree.set_position(walker, Vec3::new(0.0, 2.0, 3.0)).unwrap();
    if let Some(mut body) = tree.component_mut::<PhysicsBody3D>(walker) {
        body.collision_layer = 4;
        body.collision_mask = 1;
    }
    tree.add_child(tree.root(), walker).unwrap();
    for _ in 0..120 {
        if let Some(mut c) = tree.component_mut::<CharacterBody3D>(walker) {
            c.velocity += PhysicsWorld::DEFAULT_GRAVITY * DT;
        }
        tree.move_and_slide(walker, DT).unwrap();
        step(&mut tree);
    }
    assert!((tree.position(seen_by_its_mask).unwrap().y - 0.5).abs() < 0.05);
    assert!(
        tree.position(seen_by_nobody).unwrap().y < -2.0,
        "fell through"
    );
    assert!((tree.position(walker).unwrap().y - 0.9).abs() < 0.05);
    // Raycasts test their own mask against layers.
    let physics = physics(&tree);
    let down = |mask| {
        physics
            .cast_ray(
                Vec3::new(-2.0, 5.0, 0.0),
                Vec3::NEG_Y,
                10.0,
                &QueryOptions::new().with_mask(mask),
            )
            .map(|h| h.node)
    };
    assert_eq!(down(4), Some(seen_by_its_mask));
    assert_ne!(down(1), Some(seen_by_its_mask));
}
