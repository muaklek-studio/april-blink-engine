//! Offscreen GPU tests: render small scenes, read the pixels back and check what is on screen.
//! They skip (pass with a notice) on machines without a usable GPU adapter, e.g. CI runners.

use ab_core::{Color, Transform3D, Vec3};
use ab_rendering::test_gpu_context as gpu;
use ab_rendering::{
    Camera3D, DirectionalLight3D, Environment, GpuContext, Material, Mesh, MeshInstance3D,
    OffscreenTarget, RenderAssets, RenderNodes, RendererConfig, SceneRenderer, ScreenOverlay,
    SpotLight3D,
};
use ab_scene::{NodeHandle, SceneTree};

const SIZE: u32 = 128;

struct Scene {
    tree: SceneTree,
    sun: NodeHandle,
    /// The cube's material.
    red: ab_rendering::MaterialHandle,
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
    Scene { tree, sun, red }
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
fn fog_patches_drift_with_the_wind_and_the_sun_lights_the_fog() {
    let Some(gpu) = gpu() else { return };
    let mut scene = scene();
    let set_time = |tree: &mut SceneTree, seconds: f64| {
        let mut time = ab_core::Time::new(1.0 / 60.0);
        time.elapsed = seconds;
        tree.world_mut().insert_resource(time);
    };
    {
        let mut env = scene.tree.world().resource_mut::<Environment>().unwrap();
        env.fog.enabled = true;
        env.fog.color = Color::rgb(0.8, 0.8, 0.8);
        env.fog.density = 0.25;
        env.fog.sun_scattering = 0.0;
    }
    set_time(&mut scene.tree, 0.0);
    let even = render(&gpu, &scene.tree, 1);
    {
        let mut env = scene.tree.world().resource_mut::<Environment>().unwrap();
        env.fog.patchiness = 1.0;
        env.fog.patch_size = 1.5;
        env.fog.wind = Vec3::new(4.0, 0.0, 1.0);
    }
    let patchy = render(&gpu, &scene.tree, 1);
    // How much the floor's brightness varies across the lower half of the image.
    let spread = |p: &[[u8; 4]]| {
        let row: Vec<f32> = (SIZE / 2..SIZE)
            .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
            .map(|(x, y)| f32::from(pixel(p, x, y)[1]))
            .collect();
        let mean = row.iter().sum::<f32>() / row.len() as f32;
        (row.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / row.len() as f32).sqrt()
    };
    assert!(
        spread(&patchy) > spread(&even) + 2.0,
        "patches make the fog uneven: {} vs {}",
        spread(&patchy),
        spread(&even)
    );
    set_time(&mut scene.tree, 0.7);
    let later = render(&gpu, &scene.tree, 1);
    let changed = later.iter().zip(&patchy).filter(|(a, b)| a != b).count();
    assert!(
        changed > (SIZE * SIZE / 10) as usize,
        "the wind moves them: {changed}"
    );

    {
        let mut env = scene.tree.world().resource_mut::<Environment>().unwrap();
        env.fog.patchiness = 0.0;
        env.fog.sun_scattering = 3.0;
        env.fog.sun_scattering_anisotropy = 0.0;
    }
    let lit = render(&gpu, &scene.tree, 1);
    assert!(
        mean_luma(&lit) > mean_luma(&even) + 5.0,
        "sunlight scattered in the fog brightens it: {} vs {}",
        mean_luma(&lit),
        mean_luma(&even)
    );
}

#[test]
fn light_shafts_light_the_air_but_not_in_shadow() {
    let Some(gpu) = gpu() else { return };
    let scene = scene();
    let base = render(&gpu, &scene.tree, 1);
    {
        let mut env = scene.tree.world().resource_mut::<Environment>().unwrap();
        env.light_shafts.enabled = true;
        env.light_shafts.density = 0.3;
        env.light_shafts.anisotropy = 0.0;
        env.light_shafts.intensity = 2.0;
    }
    let shafts = render(&gpu, &scene.tree, 4);
    assert!(
        mean_luma(&shafts) > mean_luma(&base) + 4.0,
        "lit air brightens the view: {} vs {}",
        mean_luma(&shafts),
        mean_luma(&base)
    );
    // A roof above the camera shadows all the air in view: the beams are gone.
    let mut scene = scene;
    {
        let tree = &mut scene.tree;
        let mesh = tree
            .world()
            .resource_mut::<RenderAssets>()
            .unwrap()
            .add_mesh(Mesh::cube(1.0))
            .unwrap();
        let material = tree
            .world()
            .resource_mut::<RenderAssets>()
            .unwrap()
            .add_material(Material::default());
        let roof = tree.create_mesh_instance3d("Roof", MeshInstance3D::new(mesh, material));
        tree.set_transform(
            roof,
            Transform3D::from_trs(
                Vec3::new(0.0, 7.0, 0.0),
                Default::default(),
                Vec3::new(60.0, 0.2, 60.0),
            ),
        )
        .unwrap();
        let root = tree.root();
        tree.add_child(root, roof).unwrap();
        tree.update_transforms();
    }
    let roofed_base = {
        let mut env = scene.tree.world().resource_mut::<Environment>().unwrap();
        env.light_shafts.enabled = false;
        drop(env);
        render(&gpu, &scene.tree, 4)
    };
    scene
        .tree
        .world()
        .resource_mut::<Environment>()
        .unwrap()
        .light_shafts
        .enabled = true;
    let roofed = render(&gpu, &scene.tree, 4);
    let open_gain = mean_luma(&shafts) - mean_luma(&base);
    let roofed_gain = mean_luma(&roofed) - mean_luma(&roofed_base);
    assert!(
        roofed_gain < open_gain * 0.25,
        "shadowed air stays dark: {roofed_gain} vs {open_gain}"
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

#[test]
fn mirrored_meshes_are_not_rendered_inside_out() {
    let Some(gpu) = gpu() else { return };
    let scene = scene();
    let normal = render(&gpu, &scene.tree, 1);
    // The cube is symmetric: mirrored on X it must look the same (negative-determinant
    // transforms flip the triangle winding, which must not flip the culling).
    let mut tree = scene.tree;
    let cube = tree
        .find_child(tree.root(), "Cube", false)
        .expect("cube node");
    tree.set_scale(cube, Vec3::new(-1.0, 1.0, 1.0)).unwrap();
    tree.update_transforms();
    let mirrored = render(&gpu, &tree, 1);
    let luma = |p: [u8; 4]| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
    let differing = normal
        .iter()
        .zip(&mirrored)
        .filter(|(a, b)| (luma(**a) - luma(**b)).abs() > 20.0)
        .count();
    assert!(differing < 10, "{differing} pixels differ once mirrored");
}

#[test]
fn zero_sized_offscreen_targets_are_clamped_and_render() {
    let Some(gpu) = gpu() else { return };
    let scene = scene();
    let target = OffscreenTarget::new(&gpu, 0, 0);
    assert_eq!(target.size(), (1, 1));
    let mut renderer =
        SceneRenderer::new(&gpu, OffscreenTarget::FORMAT, &RendererConfig::default());
    renderer.render(&gpu, scene.tree.world(), target.view(), 0, 0);
    assert_eq!(target.read_pixels(&gpu).unwrap().len(), 1);
}

/// A sphere under a slanted sun, stretched non-uniformly either by its node (static path,
/// CPU inverse-transpose) or by its single joint (skinned path, shader): both must shade alike.
#[test]
fn skinned_normals_follow_non_uniform_joint_scale() {
    let Some(gpu) = gpu() else { return };
    let stretch = Transform3D::from_trs(
        Vec3::new(0.0, 1.0, 0.0),
        ab_core::Quat::from_rotation_z(0.6),
        Vec3::new(2.5, 0.6, 1.0),
    );
    let build = |skinned: bool| {
        let mut tree = SceneTree::new();
        let mut assets = RenderAssets::new();
        let mut sphere = Mesh::sphere(1.0, 32, 16);
        if skinned {
            let count = sphere.vertices.len();
            sphere = sphere.with_skin(vec![ab_rendering::SkinWeights::single(0); count]);
        }
        let mesh = assets.add_mesh(sphere).unwrap();
        let white = assets.add_material(Material::from_color(Color::rgb(0.8, 0.8, 0.8)));
        tree.world_mut().insert_resource(assets);
        tree.world_mut().insert_resource(Environment {
            ambient_energy: 0.0,
            ..Default::default()
        });
        let root = tree.root();
        let camera = tree.create_camera3d("Camera", Camera3D::default());
        tree.set_transform(
            camera,
            Transform3D::looking_at(Vec3::new(0.0, 1.0, 6.0), Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        )
        .unwrap();
        tree.add_child(root, camera).unwrap();
        let sun = tree.create_directional_light3d("Sun", DirectionalLight3D::default());
        tree.set_transform(
            sun,
            Transform3D::looking_at(Vec3::ZERO, Vec3::new(-1.0, -0.5, -1.0), Vec3::Y),
        )
        .unwrap();
        tree.add_child(root, sun).unwrap();
        let body = tree.create_mesh_instance3d("Body", MeshInstance3D::new(mesh, white));
        tree.add_child(root, body).unwrap();
        if skinned {
            let joint = tree.create_node3d("Joint");
            tree.set_transform(joint, stretch).unwrap();
            tree.add_child(root, joint).unwrap();
            tree.insert_component(
                body,
                ab_rendering::Skin {
                    joints: vec![joint],
                    inverse_bind_matrices: vec![ab_core::Mat4::IDENTITY],
                },
            )
            .unwrap();
        } else {
            tree.set_transform(body, stretch).unwrap();
        }
        tree.update_transforms();
        tree
    };
    let reference = render(&gpu, &build(false), 1);
    let skinned = render(&gpu, &build(true), 1);
    let luma = |p: [u8; 4]| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
    let differing = reference
        .iter()
        .zip(&skinned)
        .filter(|(a, b)| (luma(**a) - luma(**b)).abs() > 12.0)
        .count();
    assert!(
        differing < 20,
        "{differing} pixels shade differently when skinned"
    );
}

#[test]
fn textures_over_the_upload_budget_arrive_over_several_frames() {
    let Some(gpu) = gpu() else { return };
    let scene = scene();
    {
        let mut assets = scene.tree.world().resource_mut::<RenderAssets>().unwrap();
        for _ in 0..3 {
            let texture = ab_rendering::Texture::new(
                64,
                64,
                vec![255; 64 * 64 * 4],
                ab_rendering::ColorSpace::Srgb,
            )
            .unwrap();
            let handle = assets.add_texture(texture).unwrap();
            assets.add_material(Material {
                albedo_texture: Some(handle),
                ..Material::default()
            });
        }
    }
    let config = RendererConfig {
        msaa_samples: 1,
        shadow_map_size: 256,
        // Room for one 64² texture (with mips) a frame.
        texture_upload_budget: Some(64 * 64 * 6),
        ..Default::default()
    };
    let mut renderer = SceneRenderer::new(&gpu, OffscreenTarget::FORMAT, &config);
    let target = OffscreenTarget::new(&gpu, SIZE, SIZE);
    let mut waiting = Vec::new();
    for _ in 0..4 {
        renderer.render(&gpu, scene.tree.world(), target.view(), SIZE, SIZE);
        waiting.push(renderer.pending_texture_uploads());
    }
    assert_eq!(waiting, [2, 1, 0, 0]);

    let mut unlimited =
        SceneRenderer::new(&gpu, OffscreenTarget::FORMAT, &RendererConfig::default());
    unlimited.render(&gpu, scene.tree.world(), target.view(), SIZE, SIZE);
    assert_eq!(unlimited.pending_texture_uploads(), 0);
}

#[test]
fn streamed_textures_show_a_preview_at_once_then_sharpen() {
    let Some(gpu) = gpu() else { return };
    let scene = scene();
    {
        // The cube gets a 512² texture of wide green and blue bands: the small mips keep
        // the bands, so the preview already looks close to the final image.
        let mut assets = scene.tree.world().resource_mut::<RenderAssets>().unwrap();
        let mut texels = Vec::with_capacity(512 * 512 * 4);
        for i in 0..512 * 512 {
            let band = (i % 512) / 128;
            texels.extend_from_slice(if band % 2 == 0 {
                &[20, 220, 40, 255]
            } else {
                &[30, 60, 230, 255]
            });
        }
        let texture =
            ab_rendering::Texture::new(512, 512, texels, ab_rendering::ColorSpace::Srgb).unwrap();
        let handle = assets.add_texture(texture).unwrap();
        let cube = assets.material_mut(scene.red).unwrap();
        cube.albedo = Color::WHITE;
        cube.albedo_texture = Some(handle);
    }
    let config = |budget| RendererConfig {
        msaa_samples: 1,
        shadow_map_size: 256,
        texture_upload_budget: budget,
        ..Default::default()
    };
    let target = OffscreenTarget::new(&gpu, SIZE, SIZE);
    let mut whole = SceneRenderer::new(&gpu, OffscreenTarget::FORMAT, &config(None));
    whole.render(&gpu, scene.tree.world(), target.view(), SIZE, SIZE);
    let expected = target.read_pixels(&gpu).unwrap();
    assert!(
        expected
            .iter()
            .any(|p| u16::from(p[1]) > 2 * u16::from(p[0]) && p[1] > 60),
        "the green cube is on screen"
    );

    // Almost no budget: one level a frame.
    let mut streamed = SceneRenderer::new(&gpu, OffscreenTarget::FORMAT, &config(Some(1)));
    let mut waiting = Vec::new();
    for frame in 0..4 {
        streamed.render(&gpu, scene.tree.world(), target.view(), SIZE, SIZE);
        waiting.push(streamed.pending_texture_uploads());
        let pixels = target.read_pixels(&gpu).unwrap();
        let differing = pixels
            .iter()
            .zip(&expected)
            .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 3))
            .count();
        // The preview is blurrier at the bands' edges; the final frame matches.
        let allowed = if frame < 2 { pixels.len() / 20 } else { 0 };
        assert!(
            differing <= allowed,
            "frame {frame}: {differing} pixels differ from the full texture's image"
        );
    }
    // 128² preview first, then 256², then 512².
    assert_eq!(waiting, [1, 1, 0, 0]);
}
