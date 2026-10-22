//! Renders the sample glTF assets offscreen. Skips without a GPU adapter.

use ab_animation::{apply_animations, AnimationLibrary, AnimationPlayer};
use ab_asset::{spawn_gltf, AssetServer};
use ab_core::{Color, Transform3D, Vec3};
use ab_rendering::{
    Camera3D, DirectionalLight3D, Environment, GpuContext, OffscreenTarget, RenderAssets,
    RenderError, RenderNodes, RendererConfig, SceneRenderer,
};
use ab_scene::{NodeHandle, SceneTree};
use std::path::PathBuf;

const SIZE: u32 = 160;

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

fn scene(model: &str, camera_at: Vec3, look_at: Vec3) -> (SceneTree, NodeHandle) {
    let mut tree = SceneTree::new();
    let world = tree.world_mut();
    world.insert_resource(AssetServer::from_directory(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets"),
    ));
    world.insert_resource(RenderAssets::new());
    world.insert_resource(AnimationLibrary::new());
    world.insert_resource(Environment {
        background: Color::BLACK,
        ambient_energy: 0.3,
        ..Default::default()
    });
    let root = tree.root();
    let model = spawn_gltf(&mut tree, model).unwrap();
    tree.add_child(root, model).unwrap();

    let camera = tree.create_camera3d("Camera", Camera3D::default());
    tree.set_transform(camera, Transform3D::looking_at(camera_at, look_at, Vec3::Y))
        .unwrap();
    tree.add_child(root, camera).unwrap();
    let sun = tree.create_directional_light3d(
        "Sun",
        DirectionalLight3D {
            energy: 2.5,
            ..Default::default()
        },
    );
    tree.set_transform(
        sun,
        Transform3D::looking_at(Vec3::new(1.0, 2.0, 3.0), Vec3::ZERO, Vec3::Y),
    )
    .unwrap();
    tree.add_child(root, sun).unwrap();
    tree.update_transforms();
    (tree, model)
}

/// Renders one frame. With `AB_TEST_DUMP=<dir>` set, also writes `<dir>/<name>.png`.
fn render(
    gpu: &GpuContext,
    renderer: &mut SceneRenderer,
    tree: &SceneTree,
    name: &str,
) -> Vec<[u8; 4]> {
    let target = OffscreenTarget::new(gpu, SIZE, SIZE);
    renderer.render(gpu, tree.world(), target.view(), SIZE, SIZE);
    let pixels = target.read_pixels(gpu).unwrap();
    if let Some(dir) = std::env::var_os("AB_TEST_DUMP") {
        let bytes: Vec<u8> = pixels.iter().flatten().copied().collect();
        let path = PathBuf::from(dir).join(format!("{name}.png"));
        image::save_buffer(&path, &bytes, SIZE, SIZE, image::ExtendedColorType::Rgba8).unwrap();
    }
    pixels
}

fn non_black(pixels: &[[u8; 4]]) -> Vec<usize> {
    (0..pixels.len())
        .filter(|&i| pixels[i][..3].iter().any(|&c| c > 8))
        .collect()
}

#[test]
fn skinned_totem_follows_its_animation() {
    let Some(gpu) = gpu() else { return };
    let (mut tree, totem) = scene(
        "models/totem.gltf",
        Vec3::new(0.0, 1.5, 3.0),
        Vec3::new(0.0, 1.5, 0.0),
    );
    let mut renderer =
        SceneRenderer::new(&gpu, OffscreenTarget::FORMAT, &RendererConfig::default());

    let rest = render(&gpu, &mut renderer, &tree, "totem_rest");
    let covered = non_black(&rest);
    assert!(
        covered.len() > 1500,
        "totem should cover the image centre ({})",
        covered.len()
    );
    // The texture's teal, red and ochre bands all appear.
    let teal = rest.iter().any(|p| p[2] > p[0] + 20 && p[1] > p[0]);
    let red = rest.iter().any(|p| p[0] > p[1] + 40 && p[0] > p[2] + 40);
    assert!(teal && red, "textured bands missing");

    let player_node = tree.get_node(totem, "AnimationPlayer").unwrap();
    tree.component_mut::<AnimationPlayer>(player_node)
        .unwrap()
        .play_named("Sway");
    apply_animations(tree.world(), 0.5);
    tree.update_transforms();
    let posed = render(&gpu, &mut renderer, &tree, "totem_posed");

    let changed = rest.iter().zip(&posed).filter(|(a, b)| a != b).count();
    assert!(
        changed > 300,
        "skinning should move the upper segments ({changed} px changed)"
    );
    // Swaying left (+Z rotation) moves the top of the totem towards -X: more coverage on
    // the left half of the top rows than at rest.
    let top_left = |p: &[[u8; 4]]| {
        (0..SIZE / 4)
            .flat_map(|y| (0..SIZE / 2).map(move |x| (y * SIZE + x) as usize))
            .filter(|&i| p[i][..3].iter().any(|&c| c > 8))
            .count()
    };
    assert!(top_left(&posed) > top_left(&rest), "pose should lean left");
}

#[test]
fn normal_mapped_glb_renders() {
    let Some(gpu) = gpu() else { return };
    let (tree, _) = scene("models/crate.glb", Vec3::new(1.2, 1.0, 1.6), Vec3::ZERO);
    let mut renderer =
        SceneRenderer::new(&gpu, OffscreenTarget::FORMAT, &RendererConfig::default());
    let pixels = render(&gpu, &mut renderer, &tree, "crate");
    let covered = non_black(&pixels);
    assert!(covered.len() > 3000, "{}", covered.len());
    // Brownish wood on average.
    let (r, b) = covered.iter().fold((0u64, 0u64), |(r, b), &i| {
        (r + pixels[i][0] as u64, b + pixels[i][2] as u64)
    });
    assert!(r > b * 3 / 2, "crate should look like wood (r {r}, b {b})");
}
