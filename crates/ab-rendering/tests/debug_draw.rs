//! Debug lines: drawn over the scene, hidden behind geometry unless drawn on top.

use ab_core::{Color, Transform3D, Vec3};
use ab_rendering::{
    Camera3D, DebugDraw, Environment, GpuContext, Material, Mesh, MeshInstance3D, OffscreenTarget,
    RenderAssets, RenderError, RenderNodes, RendererConfig, SceneRenderer,
};
use ab_scene::SceneTree;

const SIZE: u32 = 96;

fn gpu() -> Option<GpuContext> {
    match GpuContext::new_headless() {
        Ok(gpu) => Some(gpu),
        Err(RenderError::NoAdapter) => {
            eprintln!("skipping GPU test: no adapter available");
            None
        }
        Err(e) => panic!("GPU initialisation failed: {e}"),
    }
}

/// Camera at z = 5 looking at a grey unit cube at the origin.
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
    tree.world_mut().insert_resource(DebugDraw::default());
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
    (target.read_pixels(gpu).unwrap(), stats.debug_lines)
}

/// The reddest pixel of the middle row.
fn reddest_in_middle_row(pixels: &[[u8; 4]]) -> u8 {
    let row = (SIZE / 2) as usize * SIZE as usize;
    pixels[row..row + SIZE as usize]
        .iter()
        .map(|p| p[0].saturating_sub(p[1]))
        .max()
        .unwrap()
}

fn draw(tree: &SceneTree, f: impl FnOnce(&mut DebugDraw)) {
    let mut debug = tree.world().resource_mut::<DebugDraw>().unwrap();
    debug.clear();
    f(&mut debug);
}

#[test]
fn debug_lines_are_depth_tested_unless_on_top() {
    let Some(gpu) = gpu() else {
        return;
    };
    let tree = scene();
    let red = Color::rgb(4.0, 0.0, 0.0);
    // In front of the cube: visible.
    draw(&tree, |d| {
        d.line(Vec3::new(-2.0, 0.0, 1.0), Vec3::new(2.0, 0.0, 1.0), red)
    });
    let (pixels, count) = render(&gpu, &tree);
    assert_eq!(count, 1);
    assert!(
        reddest_in_middle_row(&pixels) > 120,
        "line in front is drawn"
    );

    // Behind the cube, only inside its silhouette: hidden.
    draw(&tree, |d| {
        d.line(Vec3::new(-0.3, 0.0, -1.0), Vec3::new(0.3, 0.0, -1.0), red)
    });
    let (pixels, _) = render(&gpu, &tree);
    assert!(reddest_in_middle_row(&pixels) < 20, "line behind is hidden");

    // The same line on top: visible.
    draw(&tree, |d| {
        d.on_top()
            .line(Vec3::new(-0.3, 0.0, -1.0), Vec3::new(0.3, 0.0, -1.0), red);
    });
    let (pixels, _) = render(&gpu, &tree);
    assert!(reddest_in_middle_row(&pixels) > 120, "on top wins");

    // Disabled: nothing.
    tree.world().resource_mut::<DebugDraw>().unwrap().enabled = false;
    let (_, count) = render(&gpu, &tree);
    assert_eq!(count, 0);
}
