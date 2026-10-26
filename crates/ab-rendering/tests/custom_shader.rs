//! Custom surface shaders in the forward pass: drawn with their parameters, rebuilt when
//! their code changes, and replaced by the standard surface when they do not build.

use ab_core::{Color, Transform3D, Vec3};
use ab_rendering::test_gpu_context as gpu;
use ab_rendering::{
    Camera3D, Environment, GpuContext, Material, MaterialHandle, Mesh, MeshInstance3D,
    OffscreenTarget, RenderAssets, RenderNodes, RendererConfig, SceneRenderer, ShaderHandle,
};
use ab_scene::SceneTree;

const SIZE: u32 = 64;

const TINT: &str = "// param red = 0.0
// param green = 0.0

fn surface(input: SurfaceInput, base: Surface) -> Surface {
    var s = base;
    s.albedo = vec3<f32>(input.params.red, input.params.green, 0.0);
    return s;
}
";

/// A unit cube in front of the camera, unshaded grey, with the custom shader.
fn scene(code: &str) -> (SceneTree, ShaderHandle, MaterialHandle) {
    let mut tree = SceneTree::new();
    let mut assets = RenderAssets::new();
    let cube = assets.add_mesh(Mesh::cube(1.0)).unwrap();
    let shader = assets.add_shader("tint.wgsl", code);
    let mut material = Material::unshaded(Color::rgb(0.5, 0.5, 0.5));
    material.shader = Some(shader);
    material.shader_params[0] = 1.0;
    let material = assets.add_material(material);
    tree.world_mut().insert_resource(assets);
    tree.world_mut().insert_resource(Environment {
        background: Color::BLACK,
        ..Default::default()
    });
    let camera = tree.create_camera3d(
        "Camera",
        Camera3D {
            current: true,
            ..Default::default()
        },
    );
    tree.set_transform(
        camera,
        Transform3D::looking_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, Vec3::Y),
    )
    .unwrap();
    tree.add_child(tree.root(), camera).unwrap();
    let mesh = tree.create_mesh_instance3d("Cube", MeshInstance3D::new(cube, material));
    tree.add_child(tree.root(), mesh).unwrap();
    tree.update_transforms();
    (tree, shader, material)
}

fn center(gpu: &GpuContext, renderer: &mut SceneRenderer, tree: &SceneTree) -> [u8; 4] {
    let target = OffscreenTarget::new(gpu, SIZE, SIZE);
    renderer.render(gpu, tree.world(), target.view(), SIZE, SIZE);
    target.read_pixels(gpu).unwrap()[(SIZE / 2 * SIZE + SIZE / 2) as usize]
}

#[test]
fn custom_shaders_draw_reload_and_fall_back() {
    let Some(gpu) = gpu() else { return };
    let (mut tree, shader, material) = scene(TINT);
    let mut renderer =
        SceneRenderer::new(&gpu, OffscreenTarget::FORMAT, &RendererConfig::default());
    let c = center(&gpu, &mut renderer, &tree);
    assert!(
        c[0] > 200 && c[1] < 30 && c[2] < 30,
        "red from the shader: {c:?}"
    );

    // Parameters are the material's.
    {
        let mut assets = tree.world_mut().resource_mut::<RenderAssets>().unwrap();
        let m = assets.material_mut(material).unwrap();
        m.shader_params = [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    }
    let c = center(&gpu, &mut renderer, &tree);
    assert!(c[1] > 200 && c[0] < 30, "green: {c:?}");

    // New code is drawn from the next frame.
    let blue = TINT.replace("input.params.red, input.params.green, 0.0", "0.0, 0.0, 1.0");
    tree.world_mut()
        .resource_mut::<RenderAssets>()
        .unwrap()
        .set_shader_code(shader, blue);
    let c = center(&gpu, &mut renderer, &tree);
    assert!(
        c[2] > 200 && c[0] < 30 && c[1] < 30,
        "blue after reload: {c:?}"
    );

    // Code that does not build: the standard surface (the material's grey).
    tree.world_mut()
        .resource_mut::<RenderAssets>()
        .unwrap()
        .set_shader_code(
            shader,
            "fn surface(input: SurfaceInput, base: Surface) -> Surface { oops }",
        );
    let c = center(&gpu, &mut renderer, &tree);
    assert!(
        c[0] == c[1] && c[1] == c[2] && c[0] > 100 && c[0] < 220,
        "grey: {c:?}"
    );
}
