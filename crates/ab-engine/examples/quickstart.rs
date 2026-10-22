//! Quickstart: a walking first-person player on a floor, with a HUD label.
//! Run with `cargo run -p ab-engine --example quickstart`.

use ab_engine::prelude::*;

/// Walks with the `move_*` actions (WASD by default) under gravity.
struct Player {
    speed: f32,
}

impl NodeScript for Player {
    fn ready(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        println!("{} is ready", tree.path_of(node).unwrap_or_default());
    }

    fn physics_process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {
        let input = tree
            .world()
            .resource::<Actions>()
            .map(|a| a.vector("move_left", "move_right", "move_back", "move_forward"))
            .unwrap_or_default();
        if let Some(mut body) = tree.component_mut::<CharacterBody3D>(node) {
            body.velocity.x = input.x * self.speed;
            body.velocity.z = -input.y * self.speed;
            body.velocity += PhysicsWorld::DEFAULT_GRAVITY * delta;
        }
        let _ = tree.move_and_slide(node, delta);
    }
}

fn main() -> anyhow::Result<()> {
    let mut config = EngineConfig::default();
    config
        .input_map
        .bind("move_forward", Binding::Key(KeyCode::KeyW))
        .bind("move_back", Binding::Key(KeyCode::KeyS))
        .bind("move_left", Binding::Key(KeyCode::KeyA))
        .bind("move_right", Binding::Key(KeyCode::KeyD));

    ab_engine::run(config, |engine: &mut Engine| {
        let tree = engine.tree_mut();
        let (cube, grey) = {
            let mut assets = tree.world().resource_mut::<RenderAssets>().unwrap();
            let cube = assets.add_mesh(Mesh::cube(1.0))?;
            let grey = assets.add_material(Material::from_color(Color::rgb(0.5, 0.5, 0.5)));
            (cube, grey)
        };

        // A static floor: body + collider + mesh, scaled together.
        let floor = tree.create_physics_body3d("Floor", PhysicsBody3D::fixed());
        tree.set_transform(
            floor,
            Transform3D::from_translation(Vec3::new(0.0, -0.5, 0.0))
                .with_scale(Vec3::new(20.0, 1.0, 20.0)),
        )?;
        tree.add_collision_shape(floor, Shape::cuboid(Vec3::ONE))?;
        let mesh = tree.create_mesh_instance3d("Mesh", MeshInstance3D::new(cube, grey));
        tree.add_child(floor, mesh)?;
        tree.add_child(tree.root(), floor)?;

        // The player, with a first-person camera.
        let player = tree.create_character_body3d("Player", CharacterBody3D::default());
        tree.add_collision_shape(player, Shape::capsule(0.4, 1.8))?;
        tree.set_position(player, Vec3::new(0.0, 1.0, 5.0))?;
        tree.set_script(player, Player { speed: 4.0 })?;
        let camera = tree.create_camera3d(
            "Camera",
            Camera3D {
                current: true,
                ..Default::default()
            },
        );
        tree.set_position(camera, Vec3::new(0.0, 0.7, 0.0))?;
        tree.add_child(player, camera)?;
        tree.add_child(tree.root(), player)?;

        let sun = tree.create_directional_light3d(
            "Sun",
            DirectionalLight3D {
                shadow_enabled: true,
                ..Default::default()
            },
        );
        tree.set_rotation(sun, Quat::from_rotation_x(-1.0))?;
        tree.add_child(tree.root(), sun)?;

        // A HUD label.
        let hud = tree.create_canvas_layer("Hud", 1);
        tree.add_child(tree.root(), hud)?;
        let hint = tree.create_label(
            "Hint",
            Control::new().with_preset(LayoutPreset::TopLeft, Vec2::new(600.0, 40.0), 24.0),
            Label::new("WASD to walk · กด WASD เพื่อเดิน").with_style(TextStyle::new(28.0)),
        );
        tree.add_child(hud, hint)?;
        Ok(())
    })
}
