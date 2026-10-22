//! Blood through the engine: droplets and mist, splats that land, dry and recycle.

use ab_engine::blood::{BloodNodes, BloodOptions, BloodSplats};
use ab_engine::prelude::*;

const DT: f32 = 1.0 / 60.0;

fn engine() -> Engine {
    let config = EngineConfig {
        asset_root: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(),
        audio_output: ab_engine::audio::AudioOutput::Silent,
        ..Default::default()
    };
    let mut engine = Engine::new(&config);
    let tree = engine.tree_mut();
    let floor = tree.create_physics_body3d("Floor", PhysicsBody3D::fixed());
    tree.add_collision_shape(
        floor,
        Shape::Box {
            size: Vec3::new(40.0, 1.0, 40.0),
        },
    )
    .unwrap();
    tree.set_position(floor, Vec3::new(0.0, -0.5, 0.0)).unwrap();
    tree.add_child(tree.root(), floor).unwrap();
    // One step so the floor's collider exists for the raycasts.
    engine.update(DT);
    engine
}

fn splats(engine: &Engine) -> Vec<NodeHandle> {
    let tree = engine.tree();
    tree.subtree(tree.root())
        .into_iter()
        .filter(|&n| {
            tree.name(n)
                .is_some_and(|name| name.starts_with("BloodSplat"))
        })
        .collect()
}

fn cutoff(engine: &Engine, splat: NodeHandle) -> f32 {
    let tree = engine.tree();
    let material = tree.component::<MeshInstance3D>(splat).unwrap().material;
    let assets = tree.world().resource::<RenderAssets>().unwrap();
    assets.material(material).unwrap().alpha_cutoff.unwrap()
}

#[test]
fn droplets_land_as_splats_that_dry_away() {
    let mut engine = engine();
    let root = engine.tree().root();
    let options = BloodOptions {
        splats: 5,
        splat_lifetime: 1.0,
        ..BloodOptions::along(Vec3::new(1.0, 0.3, 0.0))
    };
    let particles = engine
        .tree_mut()
        .spawn_blood(root, Vec3::new(0.0, 1.2, 0.0), options)
        .unwrap();
    let placed = splats(&engine);
    assert_eq!(placed.len(), 5, "every droplet reaches the floor");
    for &splat in &placed {
        let at = engine.tree().global_position(splat).unwrap();
        assert!(at.y > 0.0 && at.y < 0.01, "on the floor's top: {at}");
        assert!(
            cutoff(&engine, splat) > 1.0,
            "hidden until its droplet lands"
        );
    }

    // Landed and spread: fully visible.
    for _ in 0..60 {
        engine.update(DT);
    }
    for &splat in &placed {
        assert_eq!(cutoff(&engine, splat), 0.5);
    }
    assert!(
        !engine.tree().is_valid(particles),
        "the particles are freed once done"
    );

    // Dried away after the lifetime and a quarter more.
    for _ in 0..120 {
        engine.update(DT);
    }
    for &splat in &placed {
        assert!(!engine.tree().is_visible(splat));
    }
}

#[test]
fn the_pool_recycles_the_oldest_splats() {
    let mut engine = engine();
    let root = engine.tree().root();
    let mut pool = BloodSplats::default();
    pool.max = 6;
    engine.tree_mut().world_mut().insert_resource(pool);
    for i in 0..4 {
        let options = BloodOptions {
            splats: 5,
            ..BloodOptions::along(Vec3::NEG_Y)
        };
        let at = Vec3::new(i as f32 * 3.0, 1.0, 0.0);
        engine.tree_mut().spawn_blood(root, at, options).unwrap();
        engine.update(DT);
    }
    let pool = engine.tree().world().resource::<BloodSplats>().unwrap();
    assert_eq!(pool.slots(), 6, "20 splats share 6 slots");
    drop(pool);
    let alive = splats(&engine);
    assert_eq!(alive.len(), 6, "replaced splats are freed");
    // The newest are kept: the last hit's splats are all there.
    let newest = alive
        .iter()
        .filter(|&&n| engine.tree().global_position(n).unwrap().x > 7.0)
        .count();
    assert_eq!(newest, 5);
}
