//! Demo scene: a roofless ruin at night. Moonlight casts shadows through the pillars, a
//! flickering lamp and a spotlight light the interior, and height fog fills the floor.
//! Crates and a metal ball tumble in at the start; the `push` action shoves whatever the
//! camera looks at. Two mannequins stand by: shove one or blow it up and it ragdolls.
//! Wind blows, the crystal hums (positional) and impacts thud.

use ab_engine::core::utils::rand::SimpleRng;
use ab_engine::core::EulerRot;
use ab_engine::prelude::*;
use ab_engine::rendering::Fog;
use std::f32::consts::TAU;
use tracing::info;

pub fn build_scene(engine: &mut Engine) -> anyhow::Result<()> {
    let tree = engine.tree_mut();
    configure_environment(tree);
    load_input_map(tree)?;

    let mut assets = tree
        .world_mut()
        .remove_resource::<RenderAssets>()
        .unwrap_or_default();
    let cube = assets.add_mesh(Mesh::cube(1.0))?;
    let floor_mesh = assets.add_mesh(Mesh::plane(18.0, 18))?;
    let sphere = assets.add_mesh(Mesh::sphere(0.5, 32, 16))?;

    let stone = assets.add_material(
        Material::from_color(Color::from_srgb8(110, 108, 104, 255)).with_roughness(0.9),
    );
    let floor_material = assets
        .add_material(Material::from_color(Color::from_srgb8(72, 60, 50, 255)).with_roughness(0.8));
    let metal = assets.add_material(
        Material::from_color(Color::rgb(0.9, 0.9, 0.95))
            .with_metallic(1.0)
            .with_roughness(0.25),
    );
    let glow = assets.add_material(
        Material::unshaded(Color::rgb(1.0, 0.75, 0.4))
            .with_emission(Color::rgb(1.0, 0.6, 0.25), 3.0),
    );
    let crystal = assets.add_material(
        Material::from_color(Color::rgb(0.1, 0.4, 0.9))
            .with_emission(Color::rgb(0.2, 0.5, 1.0), 1.5)
            .with_roughness(0.2),
    );

    let root = tree.root();
    let main = tree.create_node("Main");
    tree.set_script_named(main, "Stats")?;
    tree.add_child(root, main)?;

    let (wind, hum) = {
        let mut server = tree
            .world()
            .resource_mut::<AssetServer>()
            .ok_or_else(|| anyhow::anyhow!("AssetServer resource missing"))?;
        (
            server.load_audio("audio/wind.wav")?,
            server.load_audio("audio/hum.wav")?,
        )
    };
    let ambience = tree.create_audio_stream_player(
        "Wind",
        AudioPlayer::new(wind)
            .with_looping(true)
            .with_autoplay(true)
            .with_volume_db(-16.0),
    );
    tree.add_child(main, ambience)?;
    let impacts = tree.create_node("ImpactSounds");
    tree.set_script_named(impacts, "ImpactSounds")?;
    tree.add_child(main, impacts)?;

    // Floor, walls and pillars: static bodies with a mesh and a box collider. The meshes are
    // unit cubes, scaled by the body's transform along with the collider.
    let level = tree.create_node3d("Level");
    tree.add_child(main, level)?;
    let solid = |tree: &mut SceneTree,
                 name: &str,
                 mesh,
                 material,
                 transform: Transform3D|
     -> anyhow::Result<NodeHandle> {
        let body = tree.create_physics_body3d(name, PhysicsBody3D::fixed());
        tree.set_transform(body, transform)?;
        let visual = tree.create_mesh_instance3d("Mesh", MeshInstance3D::new(mesh, material));
        tree.add_child(body, visual)?;
        tree.add_collision_shape(body, Shape::cuboid(Vec3::ONE))?;
        tree.add_child(level, body)?;
        Ok(body)
    };

    let floor = tree.create_physics_body3d("Floor", PhysicsBody3D::fixed());
    let floor_visual =
        tree.create_mesh_instance3d("Mesh", MeshInstance3D::new(floor_mesh, floor_material));
    tree.add_child(floor, floor_visual)?;
    // A thick slab under the plane so fast bodies cannot tunnel through.
    let floor_shape = tree.add_collision_shape(floor, Shape::cuboid(Vec3::new(18.0, 1.0, 18.0)))?;
    tree.set_position(floor_shape, Vec3::new(0.0, -0.5, 0.0))?;
    tree.add_child(level, floor)?;

    for (i, (position, size)) in [
        (Vec3::new(0.0, 2.0, -9.0), Vec3::new(18.0, 4.0, 0.4)),
        (Vec3::new(-9.0, 2.0, 0.0), Vec3::new(0.4, 4.0, 18.0)),
        (Vec3::new(9.0, 1.2, 0.0), Vec3::new(0.4, 2.4, 18.0)),
    ]
    .into_iter()
    .enumerate()
    {
        solid(
            tree,
            &format!("Wall{i}"),
            cube,
            stone,
            Transform3D::from_translation(position).with_scale(size),
        )?;
    }
    for (x, z) in [(-4.0, -4.0), (4.0, -4.0), (-4.0, 4.0), (4.0, 4.0)] {
        solid(
            tree,
            "Pillar",
            cube,
            stone,
            Transform3D::from_translation(Vec3::new(x, 2.0, z))
                .with_scale(Vec3::new(0.7, 4.0, 0.7)),
        )?;
    }

    let ball = tree.create_physics_body3d(
        "MetalBall",
        PhysicsBody3D::rigid().with_mass(8.0).with_bounce(0.45),
    );
    tree.set_position(ball, Vec3::new(-2.0, 3.0, 2.5))?;
    let ball_visual = tree.create_mesh_instance3d("Mesh", MeshInstance3D::new(sphere, metal));
    tree.add_child(ball, ball_visual)?;
    tree.add_collision_shape(ball, Shape::sphere(0.5))?;
    tree.add_child(level, ball)?;

    // Centrepiece: a spinning crystal with an orbiting light bulb.
    solid(
        tree,
        "Pedestal",
        cube,
        stone,
        Transform3D::from_translation(Vec3::new(0.0, 0.5, 0.0)),
    )?;
    let pivot = tree.create_node3d("Pivot");
    tree.set_position(pivot, Vec3::new(0.0, 1.8, 0.0))?;
    tree.set_script_named(pivot, "Spinner")?;
    tree.add_child(level, pivot)?;
    let gem = tree.create_mesh_instance3d("Crystal", MeshInstance3D::new(cube, crystal));
    tree.set_transform(
        gem,
        Transform3D::from_rotation(Quat::from_euler(EulerRot::XYZ, 0.6, 0.0, 0.6))
            .with_scale(Vec3::splat(0.6)),
    )?;
    tree.add_child(pivot, gem)?;
    let drone = tree.create_audio_stream_player3d(
        "Hum",
        AudioPlayer::new(hum)
            .with_looping(true)
            .with_autoplay(true)
            .with_volume_db(-6.0)
            .with_attenuation(Attenuation {
                min_distance: 1.5,
                max_distance: 14.0,
            }),
    );
    tree.add_child(gem, drone)?;

    let lamp = tree.create_omni_light3d(
        "Lamp",
        OmniLight3D {
            color: Color::rgb(1.0, 0.65, 0.3),
            energy: 14.0,
            range: 8.0,
        },
    );
    tree.set_position(lamp, Vec3::new(1.6, 0.0, 0.0))?;
    tree.set_script_named(lamp, "Flicker")?;
    tree.add_child(pivot, lamp)?;
    let bulb = tree.create_mesh_instance3d(
        "Bulb",
        MeshInstance3D {
            cast_shadows: false,
            ..MeshInstance3D::new(sphere, glow)
        },
    );
    tree.set_scale(bulb, Vec3::splat(0.2))?;
    tree.add_child(lamp, bulb)?;

    // Lights.
    let moon = tree.create_directional_light3d(
        "Moon",
        DirectionalLight3D {
            color: Color::rgb(0.55, 0.65, 1.0),
            energy: 1.2,
            shadow_enabled: true,
        },
    );
    tree.set_rotation(moon, Quat::from_euler(EulerRot::YXZ, 0.7, -0.75, 0.0))?;
    tree.add_child(main, moon)?;

    let spot = tree.create_spot_light3d(
        "Spot",
        SpotLight3D {
            color: Color::rgb(1.0, 0.95, 0.85),
            energy: 160.0,
            range: 20.0,
            angle_degrees: 22.0,
            edge_softness: 0.35,
            shadow_enabled: true,
        },
    );
    tree.set_transform(
        spot,
        Transform3D::looking_at(
            Vec3::new(-7.5, 3.8, -7.5),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::Y,
        ),
    )?;
    tree.add_child(main, spot)?;

    // Player camera.
    // The fly camera moves this node; explosions shake the pivot below it.
    let camera = tree.create_node3d("Camera");
    tree.set_transform(
        camera,
        Transform3D::looking_at(Vec3::new(3.5, 2.2, 8.5), Vec3::new(0.0, 1.2, 0.0), Vec3::Y),
    )?;
    tree.set_script_named(camera, "FlyCamera")?;
    tree.add_child(main, camera)?;
    let shake = tree.create_camera_shake("Shake");
    tree.add_child(camera, shake)?;
    let lens = tree.create_camera3d(
        "Lens",
        Camera3D {
            current: true,
            ..Default::default()
        },
    );
    tree.add_child(shake, lens)?;
    let ears = tree.create_audio_listener3d("Ears");
    tree.add_child(camera, ears)?;

    // A brazier in the corner: fire, embers and smoke, plus dust hanging in the room.
    solid(
        tree,
        "Brazier",
        cube,
        stone,
        Transform3D::from_translation(Vec3::new(-5.5, 0.4, -6.0))
            .with_scale(Vec3::new(0.9, 0.8, 0.9)),
    )?;
    let flames = tree.create_node3d("Flames");
    tree.set_position(flames, Vec3::new(-5.5, 0.95, -6.0))?;
    tree.add_child(level, flames)?;
    for (name, settings) in [
        ("Fire", ParticleSettings::fire()),
        ("Embers", ParticleSettings::embers()),
        (
            "Smoke",
            ParticleSettings::smoke()
                .with_size(0.3, 1.8)
                .with_amount(30),
        ),
    ] {
        let emitter = tree.create_particles3d(name, ParticleEmitter::new(settings));
        tree.add_child(flames, emitter)?;
    }
    let glow = tree.create_omni_light3d(
        "Glow",
        OmniLight3D {
            color: Color::rgb(1.0, 0.55, 0.2),
            energy: 10.0,
            range: 6.0,
        },
    );
    tree.set_position(glow, Vec3::new(0.0, 0.6, 0.0))?;
    tree.set_script_named(glow, "Flicker")?;
    tree.add_child(flames, glow)?;
    let dust = tree.create_particles3d(
        "Dust",
        ParticleEmitter::new(ParticleSettings::dust().with_amount(80).with_shape(
            EmissionShape::Box {
                size: Vec3::new(14.0, 3.0, 14.0),
            },
        )),
    );
    tree.set_position(dust, Vec3::new(0.0, 1.8, 0.0))?;
    tree.add_child(level, dust)?;

    // Imported glTF content needs the RenderAssets resource back in the world.
    tree.world_mut().insert_resource(assets);
    spawn_imported_props(tree, level)?;
    crate::hud::build_hud(tree)?;

    info!(nodes = tree.node_count(), "demo scene built");
    Ok(())
}

/// The demo's action map (a project file, not an asset: built into the program).
const INPUT_MAP: &str = include_str!("../input.toml");

/// Loads [`INPUT_MAP`] into the `Actions` resource.
fn load_input_map(tree: &mut SceneTree) -> anyhow::Result<()> {
    let world = tree.world();
    let map = InputMap::from_toml(INPUT_MAP)?;
    info!(actions = map.actions.len(), "input map loaded");
    if let Some(mut actions) = world.resource_mut::<Actions>() {
        actions.set_map(map);
    }
    Ok(())
}

fn configure_environment(tree: &mut SceneTree) {
    let world = tree.world_mut();
    let environment = world.resource_or_insert_with(Environment::default);
    environment.background = Color::rgb(0.012, 0.015, 0.03);
    environment.ambient_color = Color::rgb(0.45, 0.5, 0.7);
    environment.ambient_energy = 0.08;
    environment.fog = Fog {
        enabled: true,
        color: Color::rgb(0.03, 0.035, 0.06),
        density: 0.06,
        height: 0.0,
        height_falloff: 0.35,
        max_opacity: 0.95,
    };
    environment.post.exposure = 1.3;
    environment.post.contrast = 1.08;
    environment.post.saturation = 0.9;
    environment.post.vignette_intensity = 0.45;
    environment.post.vignette_smoothness = 0.7;
}

/// Drops crates (`crate.glb`, normal-mapped, rigid bodies) around the room and places two
/// animated totems (`totem.gltf`, skinned, static colliders).
fn spawn_imported_props(tree: &mut SceneTree, level: NodeHandle) -> anyhow::Result<()> {
    let mut rng = SimpleRng::new(7);
    for _ in 0..10 {
        let position = Vec3::new(rng.range_f32(-7.5, 7.5), 0.0, rng.range_f32(-7.5, 7.5));
        if position.x.abs() < 2.5 && position.z.abs() < 2.5 {
            continue; // keep the centre clear
        }
        let size = rng.range_f32(0.6, 1.2);
        // The crate model is a 1 m cube; the body carries its size so the collider matches.
        let body = tree.create_physics_body3d(
            "Crate",
            PhysicsBody3D::rigid().with_mass(20.0 * size * size * size),
        );
        tree.set_transform(
            body,
            Transform3D::from_trs(
                position.with_y(size * 0.5 + rng.range_f32(0.5, 3.0)),
                Quat::from_euler(
                    EulerRot::YXZ,
                    rng.range_f32(0.0, TAU),
                    rng.range_f32(-0.4, 0.4),
                    rng.range_f32(-0.4, 0.4),
                ),
                Vec3::splat(size),
            ),
        )?;
        let model = spawn_gltf(tree, "models/crate.glb")?;
        tree.add_child(body, model)?;
        tree.add_collision_shape(body, Shape::cuboid(Vec3::ONE))?;
        tree.add_child(level, body)?;
        tree.add_to_group(body, ab_demo_scripts::CRATES_GROUP)?;
    }

    for (x, speed, clip) in [(-2.2, 1.0, "Sway"), (2.2, 0.6, "Spin")] {
        // 3 m tall, 0.7 m wide at the base.
        let body = tree.create_physics_body3d("Totem", PhysicsBody3D::fixed());
        tree.set_position(body, Vec3::new(x, 0.0, -1.5))?;
        let shape = tree.add_collision_shape(body, Shape::cuboid(Vec3::new(0.7, 3.0, 0.7)))?;
        tree.set_position(shape, Vec3::new(0.0, 1.5, 0.0))?;
        let totem = spawn_gltf(tree, "models/totem.gltf")?;
        tree.add_child(body, totem)?;
        tree.add_child(level, body)?;
        let player = tree
            .get_node(totem, "AnimationPlayer")
            .ok_or_else(|| anyhow::anyhow!("totem.gltf has no animations"))?;
        if let Some(mut player) = tree.component_mut::<AnimationPlayer>(player) {
            player.speed = speed;
            player.play_named(clip);
        }
    }

    // Two mannequins standing frozen until shoved or caught in a blast (then ragdolls).
    for (name, x, z, yaw) in [("DummyWest", -4.6, 1.2, 0.9), ("DummyEast", 4.4, 0.6, -1.1)] {
        let dummy = ab_engine::ragdoll::spawn_humanoid_ragdoll(
            tree,
            &ab_engine::ragdoll::HumanoidRagdoll {
                name: name.into(),
                active: false,
                color: Color::from_srgb8(150, 160, 178, 255),
                ..Default::default()
            },
        )?;
        tree.set_transform(
            dummy,
            Transform3D::from_trs(Vec3::new(x, 0.0, z), Quat::from_rotation_y(yaw), Vec3::ONE),
        )?;
        tree.add_child(level, dummy)?;
    }
    Ok(())
}
