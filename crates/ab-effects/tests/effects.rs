//! Effects in a scene: draw list, one-shot cleanup, explosions, camera shake, screen effects.

use ab_core::Color;
use ab_core::{Time, Vec3};
use ab_effects::{
    effects_system, flash_screen, hit_stop, CameraShake, EffectNodes, ExplosionOptions, LightFlash,
    ParticleEmitter, ParticleSettings, ScreenEffects,
};
use ab_rendering::{OmniLight3D, ParticleDrawList, ScreenOverlay};
use ab_scene::SceneTree;

const DT: f32 = 1.0 / 60.0;

fn tree() -> SceneTree {
    let mut tree = SceneTree::new();
    let mut time = Time::new(DT);
    time.begin_frame(DT);
    tree.world_mut().insert_resource(time);
    ab_effects::register(tree.world_mut());
    tree
}

fn frame(tree: &mut SceneTree) {
    tree.process(DT);
    effects_system(tree.world_mut());
}

fn drawn(tree: &SceneTree) -> usize {
    tree.world()
        .resource::<ParticleDrawList>()
        .unwrap()
        .instances
        .len()
}

#[test]
fn emitters_fill_the_draw_list_where_they_are() {
    let mut tree = tree();
    let fire = tree.create_particles3d("Fire", ParticleEmitter::new(ParticleSettings::fire()));
    tree.set_position(fire, Vec3::new(5.0, 0.0, 0.0)).unwrap();
    tree.add_child(tree.root(), fire).unwrap();
    for _ in 0..60 {
        frame(&mut tree);
    }
    let list = tree.world().resource::<ParticleDrawList>().unwrap().clone();
    assert!(list.instances.len() > 30, "{}", list.instances.len());
    assert!(list.instances.iter().all(|p| p.additive));
    let center =
        list.instances.iter().map(|p| p.position).sum::<Vec3>() / list.instances.len() as f32;
    assert!(
        (center.x - 5.0).abs() < 0.5 && center.y > 0.2,
        "rising from the emitter: {center}"
    );

    // Hidden: simulated but not drawn. Detached: neither.
    tree.set_visible(fire, false).unwrap();
    frame(&mut tree);
    assert_eq!(drawn(&tree), 0);
    tree.set_visible(fire, true).unwrap();
    tree.remove_child(tree.root(), fire).unwrap();
    frame(&mut tree);
    assert_eq!(drawn(&tree), 0);
}

#[test]
fn one_shot_effects_free_themselves() {
    let mut tree = tree();
    let root = tree.root();
    let effect = tree
        .spawn_effect(
            root,
            Vec3::new(0.0, 1.0, 0.0),
            ParticleSettings::sparks().with_amount(30),
        )
        .unwrap();
    frame(&mut tree);
    assert_eq!(drawn(&tree), 30);
    assert_eq!(tree.global_position(effect), Some(Vec3::new(0.0, 1.0, 0.0)));
    for _ in 0..90 {
        frame(&mut tree);
    }
    assert!(!tree.is_valid(effect), "freed after its particles died");
    assert_eq!(drawn(&tree), 0);
}

#[test]
fn explosions_flash_shake_and_clean_up() {
    let mut tree = tree();
    let pivot = tree.create_camera_shake("Shake");
    tree.add_child(tree.root(), pivot).unwrap();
    let root = tree.root();
    let explosion = tree
        .spawn_explosion(root, Vec3::new(2.0, 0.0, 0.0), ExplosionOptions::default())
        .unwrap();
    let flash = tree.get_node(explosion, "Flash").unwrap();
    assert!(tree.component::<CameraShake>(pivot).unwrap().trauma > 0.5);

    frame(&mut tree);
    assert!(
        overlay(&tree).chromatic_aberration > 0.5,
        "the blast distorts the image"
    );
    assert!(drawn(&tree) > 80, "fireball + smoke + sparks");
    let first = tree.component::<OmniLight3D>(flash).unwrap().energy;
    assert!(
        tree.position(pivot).unwrap() != Vec3::ZERO,
        "the camera pivot shakes"
    );
    for _ in 0..20 {
        frame(&mut tree);
    }
    assert!(tree.component::<OmniLight3D>(flash).unwrap().energy < first * 0.5);
    for _ in 0..60 * 4 {
        frame(&mut tree);
    }
    assert!(!tree.is_valid(explosion), "freed when everything finished");
    let shake = *tree.component::<CameraShake>(pivot).unwrap();
    assert_eq!(shake.trauma, 0.0);
    assert!(tree.position(pivot).unwrap().length() < 1e-6, "settled");
    assert!(LightFlash::new(1.0, 0.5).duration > 0.0);
}

fn overlay(tree: &SceneTree) -> ScreenOverlay {
    *tree.world().resource::<ScreenOverlay>().unwrap()
}

#[test]
fn screen_effects_drive_the_overlay_and_time() {
    let mut tree = tree();
    flash_screen(tree.world(), Color::rgba(1.0, 0.0, 0.0, 1.0), 0.5);
    hit_stop(tree.world(), 0.05);
    frame(&mut tree);
    let red = overlay(&tree);
    assert!(red.color.a > 0.9 && red.color.r > 0.99, "{red:?}");
    assert_eq!(tree.world().resource::<Time>().unwrap().time_scale, 0.0);
    for _ in 0..40 {
        frame(&mut tree);
    }
    assert_eq!(overlay(&tree), ScreenOverlay::default(), "flash over");
    assert_eq!(tree.world().resource::<Time>().unwrap().time_scale, 1.0);

    tree.world()
        .resource_mut::<ScreenEffects>()
        .unwrap()
        .fade_out(Color::BLACK, 0.25);
    for _ in 0..20 {
        frame(&mut tree);
    }
    assert!(tree
        .world()
        .resource::<ScreenEffects>()
        .unwrap()
        .is_faded_out());
    assert_eq!(overlay(&tree).color, Color::BLACK);
}

#[test]
fn busy_scenes_simulate_every_emitter() {
    // Enough particles to take the parallel path; each emitter keeps its own particles.
    let mut tree = tree();
    let mut emitters = Vec::new();
    for i in 0..24 {
        let node = tree.create_particles3d(
            &format!("Sparks{i}"),
            ParticleEmitter::new(ParticleSettings::sparks().with_amount(60)).one_shot(),
        );
        tree.set_position(node, Vec3::new(i as f32 * 10.0, 0.0, 0.0))
            .unwrap();
        tree.add_child(tree.root(), node).unwrap();
        emitters.push(node);
    }
    frame(&mut tree);
    frame(&mut tree);
    let list = tree.world().resource::<ParticleDrawList>().unwrap().clone();
    let alive: usize = emitters
        .iter()
        .map(|&e| tree.component::<ParticleEmitter>(e).unwrap().alive())
        .sum();
    assert_eq!(alive, 24 * 60);
    assert_eq!(list.instances.len(), alive);
    for i in 0..24 {
        let x = i as f32 * 10.0;
        let near = list
            .instances
            .iter()
            .filter(|p| (p.position.x - x).abs() < 4.0)
            .count();
        assert_eq!(near, 60, "emitter {i} drew its own particles");
    }
}
