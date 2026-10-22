//! Particles in the HDR pass: placement, depth testing against opaque geometry, blending.

use ab_core::{Color, Transform3D, Vec3};
use ab_rendering::test_gpu_context as gpu;
use ab_rendering::{
    Camera3D, Environment, GpuContext, Material, Mesh, MeshInstance3D, OffscreenTarget,
    ParticleDrawList, ParticleInstance, RenderAssets, RenderNodes, RendererConfig, SceneRenderer,
};
use ab_scene::SceneTree;

const SIZE: u32 = 96;

fn particle(position: Vec3, color: Color, additive: bool) -> ParticleInstance {
    ParticleInstance {
        position,
        size: 1.0,
        color,
        velocity: Vec3::ZERO,
        stretch: 0.0,
        rotation: 0.0,
        hardness: 0.9,
        additive,
    }
}

/// Camera at z = 5 looking at the origin; a unit cube at the origin, unlit grey.
fn scene() -> SceneTree {
    let mut tree = SceneTree::new();
    let mut assets = RenderAssets::new();
    let cube = assets.add_mesh(Mesh::cube(1.0)).unwrap();
    let grey = assets.add_material(Material::unshaded(Color::rgb(0.2, 0.2, 0.2)));
    tree.world_mut().insert_resource(assets);
    tree.world_mut().insert_resource(Environment {
        background: Color::BLACK,
        ..Default::default()
    });
    let camera = tree.create_camera3d(
        "Camera",
        Camera3D {
            current: true,
            fov_y_degrees: 60.0,
            ..Default::default()
        },
    );
    tree.set_transform(
        camera,
        Transform3D::looking_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y),
    )
    .unwrap();
    tree.add_child(tree.root(), camera).unwrap();
    let mesh = tree.create_mesh_instance3d("Cube", MeshInstance3D::new(cube, grey));
    tree.add_child(tree.root(), mesh).unwrap();
    tree.update_transforms();
    tree
}

fn render(gpu: &GpuContext, tree: &SceneTree) -> (Vec<[u8; 4]>, u32) {
    let mut renderer = SceneRenderer::new(gpu, OffscreenTarget::FORMAT, &RendererConfig::default());
    let target = OffscreenTarget::new(gpu, SIZE, SIZE);
    let stats = renderer.render(gpu, tree.world(), target.view(), SIZE, SIZE);
    (target.read_pixels(gpu).unwrap(), stats.particles)
}

fn center(pixels: &[[u8; 4]]) -> [u8; 4] {
    pixels[(SIZE / 2 * SIZE + SIZE / 2) as usize]
}

#[test]
fn particles_draw_in_front_and_hide_behind_geometry() {
    let Some(gpu) = gpu() else { return };
    let mut tree = scene();
    let (plain, _) = render(&gpu, &tree);
    let base = center(&plain);

    // A red alpha particle in front of the cube covers it.
    tree.world_mut().insert_resource(ParticleDrawList {
        instances: vec![particle(Vec3::new(0.0, 0.0, 2.0), Color::RED, false)],
    });
    let (front, count) = render(&gpu, &tree);
    assert_eq!(count, 1);
    let c = center(&front);
    assert!(c[0] > 200 && c[1] < 40, "red over the cube: {c:?}");

    // The same particle behind the cube is hidden by it.
    tree.world_mut().insert_resource(ParticleDrawList {
        instances: vec![particle(Vec3::new(0.0, 0.0, -2.0), Color::RED, false)],
    });
    let (behind, _) = render(&gpu, &tree);
    assert_eq!(center(&behind), base, "depth-tested against the cube");

    // An additive particle adds light: the grey gets brighter, green on top of it.
    tree.world_mut().insert_resource(ParticleDrawList {
        instances: vec![particle(
            Vec3::new(0.0, 0.0, 2.0),
            Color::rgba(0.0, 0.6, 0.0, 1.0),
            true,
        )],
    });
    let (glow, _) = render(&gpu, &tree);
    let g = center(&glow);
    assert!(
        g[1] > base[1] + 60 && g[0] >= base[0].saturating_sub(2),
        "{g:?} vs {base:?}"
    );

    // Behind the camera: culled.
    tree.world_mut().insert_resource(ParticleDrawList {
        instances: vec![particle(Vec3::new(0.0, 0.0, 9.0), Color::RED, false)],
    });
    assert_eq!(render(&gpu, &tree).1, 0);
}

#[test]
fn nearer_alpha_particles_draw_on_top() {
    let Some(gpu) = gpu() else { return };
    let mut tree = scene();
    // Listed far-to-near reversed on purpose: the renderer sorts them.
    tree.world_mut().insert_resource(ParticleDrawList {
        instances: vec![
            particle(Vec3::new(0.0, 0.0, 3.0), Color::BLUE, false),
            particle(Vec3::new(0.0, 0.0, 2.0), Color::RED, false),
        ],
    });
    let c = center(&render(&gpu, &tree).0);
    assert!(
        c[2] > 200 && c[0] < 40,
        "the nearer (blue) particle wins: {c:?}"
    );
}
