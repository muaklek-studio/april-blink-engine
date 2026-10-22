//! Every particle preset side by side, with its name, an explosion every few seconds, bloom,
//! and the screen effects on keys: `cargo run -p ab-engine --example effects`.
//!
//! 1 flash · 2 fade out / in · 3 hit stop · B bloom · G film grain · Esc quit

use ab_engine::prelude::*;

/// Sets off an explosion and a burst of sparks every few seconds.
#[derive(Default)]
struct Show {
    timer: f32,
}

impl NodeScript for Show {
    fn process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {
        self.timer += delta;
        let quit = tree
            .world()
            .resource::<InputState>()
            .is_some_and(|i| i.is_key_just_pressed(KeyCode::Escape));
        if quit {
            if let Some(mut control) = tree.world().resource_mut::<EngineControl>() {
                control.quit();
            }
        }
        let pressed = |key| {
            tree.world()
                .resource::<InputState>()
                .is_some_and(|i| i.is_key_just_pressed(key))
        };
        let (flash, fade, stop, bloom, grain) = (
            pressed(KeyCode::Digit1),
            pressed(KeyCode::Digit2),
            pressed(KeyCode::Digit3),
            pressed(KeyCode::KeyB),
            pressed(KeyCode::KeyG),
        );
        if let Some(mut screen) = tree.world().resource_mut::<ScreenEffects>() {
            if flash {
                screen.flash(Color::rgba(1.0, 0.15, 0.1, 0.6), 0.4);
            }
            if fade {
                if screen.fade_amount() > 0.5 {
                    screen.fade_in(0.6);
                } else {
                    screen.fade_out(Color::BLACK, 0.6);
                }
            }
            if stop {
                screen.hit_stop(0.15);
                screen.distort(2.0, 0.4);
            }
        }
        if let Some(mut environment) = tree.world().resource_mut::<Environment>() {
            if bloom {
                let on = environment.post.bloom.is_enabled();
                environment.post.bloom.intensity = if on { 0.0 } else { 0.2 };
            }
            if grain {
                environment.post.grain = if environment.post.grain > 0.0 {
                    0.0
                } else {
                    0.5
                };
            }
        }
        if self.timer < 3.0 {
            return;
        }
        self.timer = 0.0;
        let _ = tree.spawn_explosion(node, Vec3::new(0.0, 3.2, -4.0), ExplosionOptions::default());
        let _ = tree.spawn_effect(node, Vec3::new(8.0, 1.5, 0.0), ParticleSettings::sparks());
        let _ = tree.spawn_effect(node, Vec3::new(-8.0, 0.0, 2.0), ParticleSettings::splash());
    }
}

fn main() -> anyhow::Result<()> {
    let mut config = EngineConfig::default();
    config.window.title = "Effects · April Blink Engine".into();
    ab_engine::run(config, |engine: &mut Engine| {
        let tree = engine.tree_mut();
        {
            let mut environment = tree.world().resource_mut::<Environment>().unwrap();
            environment.background = Color::rgb(0.01, 0.012, 0.02);
            environment.ambient_energy = 0.25;
            environment.post.bloom.intensity = 0.2;
            environment.post.vignette_intensity = 0.3;
        }
        let (cube, grey) = {
            let mut assets = tree.world().resource_mut::<RenderAssets>().unwrap();
            let cube = assets.add_mesh(Mesh::cube(1.0))?;
            let grey = assets.add_material(Material::from_color(Color::rgb(0.25, 0.25, 0.27)));
            (cube, grey)
        };
        let floor = tree.create_mesh_instance3d("Floor", MeshInstance3D::new(cube, grey));
        tree.set_transform(
            floor,
            Transform3D::from_translation(Vec3::new(0.0, -0.5, 0.0))
                .with_scale(Vec3::new(30.0, 1.0, 12.0)),
        )?;
        tree.add_child(tree.root(), floor)?;

        let camera = tree.create_node3d("Camera");
        tree.set_transform(
            camera,
            Transform3D::looking_at(Vec3::new(0.0, 2.5, 11.0), Vec3::new(0.0, 1.5, 0.0), Vec3::Y),
        )?;
        tree.add_child(tree.root(), camera)?;
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

        let hud = tree.create_canvas_layer("Labels", 1);
        tree.add_child(tree.root(), hud)?;
        let presets = [
            "smoke",
            "fire",
            "embers",
            "magic",
            "wisps",
            "dust",
            "snow",
            "heal",
            "fireflies",
            "leaves",
        ];
        for (i, name) in presets.iter().enumerate() {
            let x = -10.8 + i as f32 * 2.4;
            let mut settings = ParticleSettings::preset(name).expect("built-in preset");
            let height = match *name {
                "smoke" | "fire" | "embers" | "heal" => 0.2,
                "snow" | "leaves" => 4.0,
                _ => 1.2,
            };
            if matches!(*name, "dust" | "snow" | "fireflies" | "leaves") {
                let flat = matches!(*name, "snow" | "leaves");
                settings = settings.with_shape(EmissionShape::Box {
                    size: Vec3::new(2.0, if flat { 0.0 } else { 2.0 }, 2.0),
                });
            }
            let emitter = tree.create_particles3d(name, ParticleEmitter::new(settings));
            tree.set_position(emitter, Vec3::new(x, height, 0.0))?;
            tree.add_child(tree.root(), emitter)?;
            let label = tree.create_label(
                name,
                Control::at(
                    Vec2::new(60.0 + i as f32 * 180.0, 900.0),
                    Vec2::new(170.0, 40.0),
                ),
                Label::new(*name)
                    .with_style(TextStyle::new(26.0))
                    .with_align(HAlign::Center, VAlign::Center),
            );
            tree.add_child(hud, label)?;
        }
        let show = tree.create_node3d("Show");
        tree.set_script(show, Show::default())?;
        tree.add_child(tree.root(), show)?;
        Ok(())
    })
}
