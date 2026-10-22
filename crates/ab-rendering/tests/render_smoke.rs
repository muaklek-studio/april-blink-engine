//! Offscreen GPU tests: render small scenes, read the pixels back and check what is on screen.
//! They skip (pass with a notice) on machines without a usable GPU adapter, e.g. CI runners.

use ab_core::{Color, Transform3D, Vec3};
use ab_rendering::{
    Camera3D, DirectionalLight3D, Environment, GpuContext, Material, Mesh, MeshInstance3D,
    OffscreenTarget, RenderAssets, RenderError, RenderNodes, RendererConfig, SceneRenderer,
    ScreenOverlay, SpotLight3D,
};
use ab_scene::{NodeHandle, SceneTree};

const SIZE: u32 = 128;

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

struct Scene {
    tree: SceneTree,
    sun: NodeHandle,
}

/// Red cube floating over a grey floor, sun straight down, camera above and in front.
fn scene() -> Scene {
    let mut tree = SceneTree::new();
    let mut assets = RenderAssets::new();
    let cube = assets.add_mesh(Mesh::cube(1.0)).unwrap();
    let floor = assets.add_mesh(Mesh::plane(20.0, 1)).unwrap();
    let red = assets.add_material(Material::from_color(Color::rgb(0.8, 0.05, 0.05)));
    let grey = assets.add_material(Material::from_color(Color::rgb(0.5, 0.5, 0.5)));
    tree.world_mut().insert_resource(assets);
    tree.world_mut().insert_resource(Environment {
        background: Color::rgb(0.0, 0.0, 0.4),
        ambient_energy: 0.05,
        ..Default::default()
    });

    let root = tree.root();
    let camera = tree.create_camera3d(
        "Camera",
        Camera3D {
            fov_y_degrees: 60.0,
            ..Default::default()
        },
    );
    tree.set_transform(
        camera,
        Transform3D::looking_at(Vec3::new(0.0, 4.0, 5.0), Vec3::ZERO, Vec3::Y),
    )
    .unwrap();
    tree.add_child(root, camera).unwrap();

    let box_node = tree.create_mesh_instance3d("Cube", MeshInstance3D::new(cube, red));
    tree.set_position(box_node, Vec3::new(0.0, 1.0, 0.0))
        .unwrap();
    tree.add_child(root, box_node).unwrap();

    let floor_node = tree.create_mesh_instance3d("Floor", MeshInstance3D::new(floor, grey));
    tree.add_child(root, floor_node).unwrap();

    let sun = tree.create_directional_light3d(
        "Sun",
        DirectionalLight3D {
            energy: 3.0,
            shadow_enabled: true,
            ..Default::default()
        },
    );
    tree.rotate(sun, Vec3::X, -std::f32::consts::FRAC_PI_2)
        .unwrap();
    tree.add_child(root, sun).unwrap();

    tree.update_transforms();
    Scene { tree, sun }
}

fn render(gpu: &GpuContext, tree: &SceneTree, msaa: u32) -> Vec<[u8; 4]> {
    let config = RendererConfig {
        msaa_samples: msaa,
        shadow_map_size: 1024,
        ..Default::default()
    };
    let mut renderer = SceneRenderer::new(gpu, OffscreenTarget::FORMAT, &config);
    let target = OffscreenTarget::new(gpu, SIZE, SIZE);
    let stats = renderer.render(gpu, tree.world(), target.view(), SIZE, SIZE);
    assert!(stats.draw_calls >= 1, "{stats:?}");
    target.read_pixels(gpu).unwrap()
}

fn pixel(pixels: &[[u8; 4]], x: u32, y: u32) -> [u8; 4] {
    pixels[(y * SIZE + x) as usize]
}

fn mean_luma(pixels: &[[u8; 4]]) -> f32 {
    pixels
        .iter()
        .map(|p| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32)
        .sum::<f32>()
        / pixels.len() as f32
}

#[test]
fn renders_cube_floor_and_background() {
    let Some(gpu) = gpu() else { return };
    let scene = scene();
    for msaa in [1, 4] {
        let pixels = render(&gpu, &scene.tree, msaa);
        // The cube sits a little above the image centre.
        let cube = (0..SIZE / 2)
            .map(|y| pixel(&pixels, SIZE / 2, y))
            .find(|p| p[0] > 80 && p[0] as u16 > 3 * p[1] as u16);
        assert!(
            cube.is_some(),
            "no red cube pixel in the centre column (msaa {msaa})"
        );
        // Top edge looks past the floor's far end: background blue.
        let sky = pixel(&pixels, 2, 0);
        assert!(sky[2] > sky[0] && sky[2] > 40, "background {sky:?}");
        // Bottom edge: lit grey floor.
        let floor = pixel(&pixels, SIZE / 2, SIZE - 2);
        assert!(
            floor[0] > 60 && (floor[0] as i32 - floor[2] as i32).abs() < 30,
            "floor {floor:?}"
        );
    }
}

#[test]
fn directional_shadow_darkens_the_floor() {
    let Some(gpu) = gpu() else { return };
    let scene = scene();
    let with_shadow = mean_luma(&render(&gpu, &scene.tree, 1));
    scene
        .tree
        .component_mut::<DirectionalLight3D>(scene.sun)
        .unwrap()
        .shadow_enabled = false;
    let without_shadow = mean_luma(&render(&gpu, &scene.tree, 1));
    assert!(
        with_shadow + 1.0 < without_shadow,
        "shadow should darken the image: {with_shadow} vs {without_shadow}"
    );
}

#[test]
fn spot_light_and_its_shadow_affect_the_image() {
    let Some(gpu) = gpu() else { return };
    let mut scene = scene();
    scene
        .tree
        .component_mut::<DirectionalLight3D>(scene.sun)
        .unwrap()
        .energy = 0.0;
    let dark = mean_luma(&render(&gpu, &scene.tree, 1));

    let spot = scene.tree.create_spot_light3d(
        "Spot",
        SpotLight3D {
            energy: 40.0,
            range: 12.0,
            angle_degrees: 35.0,
            shadow_enabled: false,
            ..Default::default()
        },
    );
    scene
        .tree
        .set_position(spot, Vec3::new(0.0, 5.0, 0.0))
        .unwrap();
    scene
        .tree
        .rotate(spot, Vec3::X, -std::f32::consts::FRAC_PI_2)
        .unwrap();
    let root = scene.tree.root();
    scene.tree.add_child(root, spot).unwrap();
    scene.tree.update_transforms();
    let lit = mean_luma(&render(&gpu, &scene.tree, 1));
    assert!(
        lit > dark + 5.0,
        "spot light should brighten: {dark} -> {lit}"
    );

    scene
        .tree
        .component_mut::<SpotLight3D>(spot)
        .unwrap()
        .shadow_enabled = true;
    let shadowed = mean_luma(&render(&gpu, &scene.tree, 1));
    assert!(
        shadowed + 0.5 < lit,
        "spot shadow should darken: {lit} -> {shadowed}"
    );
}

#[test]
fn fog_and_vignette_change_the_image() {
    let Some(gpu) = gpu() else { return };
    let scene = scene();
    let base = render(&gpu, &scene.tree, 1);

    {
        let mut env = scene.tree.world().resource_mut::<Environment>().unwrap();
        env.fog.enabled = true;
        env.fog.color = Color::rgb(1.0, 1.0, 1.0);
        env.fog.density = 0.3;
    }
    let fogged = render(&gpu, &scene.tree, 1);
    let far_floor = |p: &[[u8; 4]]| pixel(p, SIZE / 2, SIZE / 3);
    assert!(
        far_floor(&fogged)[1] > far_floor(&base)[1] + 20,
        "fog should whiten distant floor"
    );

    {
        let mut env = scene.tree.world().resource_mut::<Environment>().unwrap();
        env.fog.enabled = false;
        env.post.vignette_intensity = 1.0;
        env.post.vignette_smoothness = 0.6;
    }
    let vignetted = render(&gpu, &scene.tree, 1);
    let corner = |p: &[[u8; 4]]| p[(SIZE * SIZE - 1) as usize];
    assert!(
        corner(&vignetted)[0] + 10 < corner(&base)[0],
        "vignette should darken corners"
    );
    let centre = |p: &[[u8; 4]]| pixel(p, SIZE / 2, SIZE / 2);
    assert_eq!(
        centre(&vignetted),
        centre(&base),
        "vignette leaves the centre untouched"
    );
}

#[test]
fn bloom_spreads_bright_light_into_its_surroundings() {
    let Some(gpu) = gpu() else { return };
    let scene = scene();
    let base = render(&gpu, &scene.tree, 1);
    {
        let mut env = scene.tree.world().resource_mut::<Environment>().unwrap();
        // Off unless it has an intensity: the default look must not change.
        env.post.bloom.threshold = 0.0;
    }
    assert_eq!(
        render(&gpu, &scene.tree, 1),
        base,
        "bloom is off by default"
    );
    {
        let mut env = scene.tree.world().resource_mut::<Environment>().unwrap();
        env.post.bloom.intensity = 1.0;
    }
    let bloomed = render(&gpu, &scene.tree, 1);
    // Background pixels (no red of their own) pick up red glow from the cube.
    let glowing = base
        .iter()
        .zip(&bloomed)
        .filter(|(b, g)| b[0] < 8 && g[0] > b[0] + 8)
        .count();
    assert!(glowing > 20, "{glowing} background pixels glow");
    assert!(mean_luma(&bloomed) > mean_luma(&base) + 1.0);
    assert!(bloomed.iter().all(|p| p[3] == 255));
}

#[test]
fn screen_overlay_grain_and_aberration_change_the_image() {
    let Some(gpu) = gpu() else { return };
    let scene = scene();
    let base = render(&gpu, &scene.tree, 1);

    let mut tree = scene.tree;
    tree.world_mut().insert_resource(ScreenOverlay {
        color: Color::rgba(1.0, 0.0, 0.0, 1.0),
        ..Default::default()
    });
    let red = render(&gpu, &tree, 1);
    assert!(
        red.iter().all(|p| p[0] == 255 && p[1] == 0 && p[2] == 0),
        "a full overlay covers everything"
    );

    tree.world_mut().insert_resource(ScreenOverlay {
        color: Color::TRANSPARENT,
        chromatic_aberration: 3.0,
    });
    let split = render(&gpu, &tree, 1);
    let changed = base.iter().zip(&split).filter(|(a, b)| a != b).count();
    assert!(changed > 50, "aberration moves colors at edges: {changed}");
    assert_eq!(
        pixel(&split, SIZE / 2, SIZE / 2),
        pixel(&base, SIZE / 2, SIZE / 2),
        "aberration leaves the centre untouched"
    );

    tree.world_mut().insert_resource(ScreenOverlay::default());
    tree.world()
        .resource_mut::<Environment>()
        .unwrap()
        .post
        .grain = 1.0;
    let grainy = render(&gpu, &tree, 1);
    let noisy = base.iter().zip(&grainy).filter(|(a, b)| a != b).count();
    assert!(
        noisy > (SIZE * SIZE / 4) as usize,
        "grain touches most pixels: {noisy}"
    );
    assert!(
        (mean_luma(&grainy) - mean_luma(&base)).abs() < 3.0,
        "grain keeps the brightness"
    );
}

#[test]
fn renders_without_camera_or_assets() {
    let Some(gpu) = gpu() else { return };
    let tree = SceneTree::new();
    let mut renderer =
        SceneRenderer::new(&gpu, OffscreenTarget::FORMAT, &RendererConfig::default());
    let target = OffscreenTarget::new(&gpu, 64, 64);
    let stats = renderer.render(&gpu, tree.world(), target.view(), 64, 64);
    assert_eq!(stats.draw_calls, 0);
    let pixels = target.read_pixels(&gpu).unwrap();
    assert!(
        pixels.windows(2).all(|w| w[0] == w[1]),
        "only the background is drawn"
    );
}
