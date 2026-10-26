//! Post-processing volumes and depth of field in the post pass.

use ab_core::{Color, Transform3D, Vec3};
use ab_rendering::test_gpu_context as gpu;
use ab_rendering::{
    Camera3D, Environment, GpuContext, Material, Mesh, MeshInstance3D, OffscreenTarget,
    PostOverrides, PostVolume, RenderAssets, RenderNodes, RendererConfig, SceneRenderer,
};
use ab_scene::SceneTree;

const SIZE: u32 = 64;

/// A white unshaded cube filling the left half of the view, black elsewhere.
fn scene() -> SceneTree {
    let mut tree = SceneTree::new();
    let mut assets = RenderAssets::new();
    let cube = assets.add_mesh(Mesh::cube(4.0)).unwrap();
    let white = assets.add_material(Material::unshaded(Color::WHITE));
    tree.world_mut().insert_resource(assets);
    let mut environment = Environment {
        background: Color::BLACK,
        ..Default::default()
    };
    environment.post.tonemap = false;
    tree.world_mut().insert_resource(environment);
    let camera = tree.create_camera3d(
        "Camera",
        Camera3D {
            current: true,
            ..Default::default()
        },
    );
    tree.set_transform(
        camera,
        Transform3D::looking_at(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO, Vec3::Y),
    )
    .unwrap();
    tree.add_child(tree.root(), camera).unwrap();
    let mesh = tree.create_mesh_instance3d("Cube", MeshInstance3D::new(cube, white));
    tree.set_position(mesh, Vec3::new(-2.0, 0.0, 0.0)).unwrap();
    tree.add_child(tree.root(), mesh).unwrap();
    tree.update_transforms();
    tree
}

/// One GPU device per test, used for every frame (as the other GPU tests do).
fn render(gpu: &GpuContext, tree: &SceneTree) -> (Vec<[u8; 4]>, u32) {
    let mut renderer = SceneRenderer::new(gpu, OffscreenTarget::FORMAT, &RendererConfig::default());
    let target = OffscreenTarget::new(gpu, SIZE, SIZE);
    let stats = renderer.render(gpu, tree.world(), target.view(), SIZE, SIZE);
    (target.read_pixels(gpu).unwrap(), stats.post_volumes)
}

fn at(pixels: &[[u8; 4]], x: u32, y: u32) -> [u8; 4] {
    pixels[(y * SIZE + x) as usize]
}

#[test]
fn a_volume_around_the_camera_changes_the_look_and_fades_with_distance() {
    let Some(gpu) = gpu() else { return };
    let mut tree = scene();
    let (plain, counted) = render(&gpu, &tree);
    assert_eq!(counted, 0);
    assert!(
        at(&plain, 8, 32)[0] > 200,
        "white cube: {:?}",
        at(&plain, 8, 32)
    );

    // Exposure 0 around the camera: black.
    let volume = tree.create_node3d("Dark");
    tree.insert_component(
        volume,
        PostVolume {
            half_extents: Vec3::splat(2.0),
            blend_distance: 4.0,
            overrides: PostOverrides {
                exposure: Some(0.0),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    tree.set_position(volume, Vec3::new(0.0, 0.0, 6.0)).unwrap();
    tree.add_child(tree.root(), volume).unwrap();
    tree.update_transforms();
    let (dark, counted) = render(&gpu, &tree);
    assert_eq!(counted, 1);
    assert!(at(&dark, 8, 32)[0] < 10, "{:?}", at(&dark, 8, 32));

    // Moved 4 m away (2 m outside the box, half the blend): about half the exposure.
    tree.set_position(volume, Vec3::new(0.0, 0.0, 10.0))
        .unwrap();
    tree.update_transforms();
    let (half, _) = render(&gpu, &tree);
    let v = at(&half, 8, 32)[0];
    assert!(v > 60 && v < 230, "partly dark: {v}");
}

#[test]
fn depth_of_field_blurs_what_is_out_of_focus() {
    let Some(gpu) = gpu() else { return };
    let mut tree = scene();
    let (sharp, _) = render(&gpu, &tree);
    {
        let mut environment = tree.world_mut().resource_mut::<Environment>().unwrap();
        let dof = &mut environment.post.depth_of_field;
        dof.focus_distance = 0.5;
        dof.focus_range = 1.0;
        // The blur is in pixels at 1080 lines: about 2 px at this size.
        dof.max_blur = 32.0;
    }
    let (blurred, _) = render(&gpu, &tree);
    // Somewhere along the cube's edge, a hard step becomes a gradient.
    let row = |p: &[[u8; 4]]| (0..SIZE).map(|x| at(p, x, 32)[0]).collect::<Vec<_>>();
    let greys = |r: &[u8]| r.iter().filter(|&&v| v > 30 && v < 225).count();
    assert!(
        greys(&row(&blurred)) > greys(&row(&sharp)),
        "sharp {:?}\nblurred {:?}",
        row(&sharp),
        row(&blurred)
    );
}
