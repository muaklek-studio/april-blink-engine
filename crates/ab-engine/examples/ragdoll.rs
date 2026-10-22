//! Ragdolls and debug tools: mannequins drop onto stairs every few seconds; Space knocks over
//! the standing one; F4 shows physics shapes and joints, F3 the profiler overlay, T toggles
//! slow motion. `cargo run -p ab-engine --example ragdoll` (Esc quits).

use ab_engine::prelude::*;
use ab_engine::ragdoll::{self, HumanoidRagdoll};

/// Drops a limp mannequin every few seconds (keeping the last few), knocks over the standing
/// one on Space, toggles slow motion on T and draws the drop point.
#[derive(Default)]
struct Show {
    timer: f32,
    dropped: Vec<NodeHandle>,
    standing: Option<NodeHandle>,
}

const DROP_AT: Vec3 = Vec3::new(0.0, 4.0, -1.0);

impl NodeScript for Show {
    fn ready(&mut self, tree: &mut SceneTree, node: NodeHandle) {
        let options = HumanoidRagdoll {
            name: "Standing".into(),
            active: false,
            color: Color::from_srgb8(110, 150, 220, 255),
            ..Default::default()
        };
        if let Ok(doll) = ragdoll::spawn_humanoid_ragdoll(tree, &options) {
            let _ = tree.set_position(doll, Vec3::new(3.0, 0.0, 1.0));
            let _ = tree.add_child(node, doll);
            self.standing = Some(doll);
        }
    }

    fn process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {
        let (quit, knock, slow) =
            tree.world()
                .resource::<InputState>()
                .map_or((false, false, false), |i| {
                    (
                        i.is_key_just_pressed(KeyCode::Escape),
                        i.is_key_just_pressed(KeyCode::Space),
                        i.is_key_just_pressed(KeyCode::KeyT),
                    )
                });
        if quit {
            if let Some(mut control) = tree.world().resource_mut::<EngineControl>() {
                control.quit();
            }
        }
        if slow {
            if let Some(mut time) = tree.world().resource_mut::<Time>() {
                time.time_scale = if time.time_scale < 1.0 { 1.0 } else { 0.25 };
            }
        }
        if knock {
            if let Some(doll) = self.standing {
                let chest = tree.global_position(doll).unwrap_or_default() + Vec3::Y * 1.3;
                let _ = ragdoll::go_limp(tree, doll, Some((Vec3::new(-25.0, 5.0, -10.0), chest)));
            }
        }

        // Debug drawing from game code: where the next mannequin falls.
        if let Some(mut debug) = tree.world().resource_mut::<DebugDraw>() {
            debug
                .on_top()
                .cross(DROP_AT, 0.4, Color::rgb(1.0, 0.85, 0.2));
            debug.arrow(DROP_AT, DROP_AT - Vec3::Y, Color::rgb(1.0, 0.85, 0.2));
        }

        self.timer += delta;
        if self.timer < 2.5 {
            return;
        }
        self.timer = 0.0;
        // Spawning is profiled: it shows as "example.spawn" in the F3 overlay.
        let doll = ab_engine::profile(tree, "example.spawn", |tree| {
            let doll = ragdoll::spawn_humanoid_ragdoll(tree, &HumanoidRagdoll::default()).ok()?;
            let spin =
                Quat::from_rotation_x(1.2) * Quat::from_rotation_y(self.dropped.len() as f32);
            tree.set_transform(doll, Transform3D::from_trs(DROP_AT, spin, Vec3::ONE))
                .ok()?;
            tree.add_child(node, doll).ok()?;
            Some(doll)
        });
        if let Some(doll) = doll {
            self.dropped.push(doll);
        }
        if self.dropped.len() > 6 {
            let oldest = self.dropped.remove(0);
            let _ = tree.queue_free(oldest);
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut config = EngineConfig::default();
    config.window.title = "Ragdolls · April Blink Engine".into();
    ab_engine::run(config, |engine: &mut Engine| {
        let tree = engine.tree_mut();
        {
            let mut environment = tree.world().resource_mut::<Environment>().unwrap();
            environment.background = Color::rgb(0.05, 0.06, 0.08);
            environment.ambient_energy = 0.5;
        }
        let (cube, grey) = {
            let mut assets = tree.world().resource_mut::<RenderAssets>().unwrap();
            let cube = assets.add_mesh(Mesh::cube(1.0))?;
            let grey = assets.add_material(Material::from_color(Color::rgb(0.35, 0.36, 0.4)));
            (cube, grey)
        };
        // Floor and three steps.
        for (i, (center, size)) in [
            (Vec3::new(0.0, -0.5, 0.0), Vec3::new(20.0, 1.0, 20.0)),
            (Vec3::new(0.0, 0.25, -1.0), Vec3::new(3.0, 0.5, 2.0)),
            (Vec3::new(0.0, 0.75, -2.0), Vec3::new(3.0, 0.5, 2.0)),
            (Vec3::new(0.0, 1.25, -3.0), Vec3::new(3.0, 0.5, 2.0)),
        ]
        .into_iter()
        .enumerate()
        {
            let body = tree.create_physics_body3d(&format!("Step{i}"), PhysicsBody3D::fixed());
            tree.add_collision_shape(body, Shape::Box { size })?;
            let mesh = tree.create_mesh_instance3d("Mesh", MeshInstance3D::new(cube, grey));
            tree.set_scale(mesh, size)?;
            tree.add_child(body, mesh)?;
            tree.set_position(body, center)?;
            tree.add_child(tree.root(), body)?;
        }

        let sun = tree.create_directional_light3d(
            "Sun",
            DirectionalLight3D {
                energy: 3.0,
                shadow_enabled: true,
                ..Default::default()
            },
        );
        tree.set_transform(
            sun,
            Transform3D::looking_at(Vec3::new(4.0, 8.0, 6.0), Vec3::ZERO, Vec3::Y),
        )?;
        tree.add_child(tree.root(), sun)?;
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
            Transform3D::looking_at(Vec3::new(5.0, 3.5, 7.0), Vec3::new(0.0, 0.8, -1.0), Vec3::Y),
        )?;
        tree.add_child(tree.root(), camera)?;

        let show = tree.create_node3d("Show");
        tree.set_script(show, Show::default())?;
        tree.add_child(tree.root(), show)?;
        // Start with the physics view on, to see the joints.
        if let Some(mut debug) = tree.world().resource_mut::<DebugSettings>() {
            debug.physics = true;
        }
        Ok(())
    })
}
