//! Effects, tweens, audio, navigation, timers, particles.

use super::ui::easing;
use super::Builder;
use crate::blood::{BloodNodes, BloodOptions};
use crate::game_config::{ConfigValue, GameConfig};
use crate::script_host::call::{Call, CallError};
use crate::tween::{Repeat, TweenNodes};
use ab_animation::{AnimationPlayer, Locomotion};
use ab_asset::AssetServer;
use ab_audio::{Audio, SoundOptions};
use ab_core::{Quat, Vec3};
use ab_effects::{shake_cameras, EffectNodes, ExplosionOptions, ParticleEmitter, ParticleSettings};
use ab_navigation::{NavBakeSettings, NavGrid};
use ab_physics::{PhysicsWorld, QueryOptions};
use ab_scene::Timer;
use ab_scene::{NodeHandle, SceneTree};
use ab_ui::Easing;

/// A step of `tween.start`, validated before anything is changed.
enum Step {
    Easing(Easing),
    Repeat(Repeat),
    Delay(f32),
    Then,
    MoveTo(Vec3, f32),
    MoveBy(Vec3, f32),
    RotateTo(Quat, f32),
    ScaleTo(Vec3, f32),
}

fn tween_steps(c: &Call<'_>) -> Result<Vec<Step>, CallError> {
    let mut steps = Vec::new();
    let mut at = 1;
    while at < c.len() {
        let kind = c.str(at)?;
        let (step, used) = match kind {
            "easing" => (Step::Easing(easing(c.str(at + 1)?)?), 2),
            "repeat" => {
                let repeat = match c.str(at + 1)? {
                    "once" => Repeat::Once,
                    "loop" => Repeat::Loop,
                    "ping_pong" => Repeat::PingPong,
                    other => {
                        return Err(CallError::BadArguments(format!("unknown repeat `{other}`")))
                    }
                };
                (Step::Repeat(repeat), 2)
            }
            "delay" => (Step::Delay(c.f32(at + 1)?), 2),
            "then" => (Step::Then, 2),
            "move_to" => (Step::MoveTo(c.vec3(at + 1)?, c.f32(at + 2)?), 3),
            "move_by" => (Step::MoveBy(c.vec3(at + 1)?, c.f32(at + 2)?), 3),
            "rotate_to" => (Step::RotateTo(c.quat(at + 1)?, c.f32(at + 2)?), 3),
            "scale_to" => (Step::ScaleTo(c.vec3(at + 1)?, c.f32(at + 2)?), 3),
            other => {
                return Err(CallError::BadArguments(format!(
                    "unknown tween step `{other}`"
                )))
            }
        };
        steps.push(step);
        at += used;
    }
    Ok(steps)
}

fn sound(c: &Call<'_>, path: &str) -> Result<ab_audio::AudioStream, CallError> {
    c.tree
        .world()
        .resource_mut::<AssetServer>()
        .ok_or_else(|| CallError::failed("no AssetServer"))?
        .load_audio(path)
        .map_err(CallError::failed)
}

fn audio<R>(c: &Call<'_>, f: impl FnOnce(&mut Audio) -> R) -> Result<R, CallError> {
    let mut audio = c
        .tree
        .world()
        .resource_mut::<Audio>()
        .ok_or_else(|| CallError::failed("no audio in this engine"))?;
    Ok(f(&mut audio))
}

/// The node with the `AnimationPlayer` of the model at `node`.
fn player_node(tree: &SceneTree, node: NodeHandle) -> Result<NodeHandle, CallError> {
    tree.subtree(node)
        .into_iter()
        .find(|&n| tree.component::<AnimationPlayer>(n).is_some())
        .ok_or_else(|| CallError::Failed(format!("{node} has no animations")))
}

/// [`player_node`], with a humanoid [`Locomotion`] added there if it has none yet.
fn locomotion_node(tree: &mut SceneTree, node: NodeHandle) -> Result<NodeHandle, CallError> {
    let player = player_node(tree, node)?;
    if tree.component::<Locomotion>(player).is_none() {
        tree.insert_component(player, Locomotion::humanoid())
            .map_err(CallError::failed)?;
    }
    Ok(player)
}

fn game_config<'t>(c: &'t Call<'_>) -> Result<std::cell::Ref<'t, GameConfig>, CallError> {
    c.tree.world().resource::<GameConfig>().ok_or_else(|| {
        CallError::failed("the game has no config (config = \"config.toml\" in game.toml)")
    })
}

fn path_of<'t>(
    c: &'t Call<'_>,
    node: NodeHandle,
) -> Result<std::cell::Ref<'t, crate::scene_file::path::Path3D>, CallError> {
    c.tree
        .component::<crate::scene_file::path::Path3D>(node)
        .ok_or_else(|| CallError::Failed(format!("{node} is not a Path3D")))
}

pub(super) fn add(b: &mut Builder) {
    b.add("path.length", |c| {
        let node = c.node(0)?;
        let scale = c
            .tree
            .global_transform(node)
            .map_or(1.0, |g| Vec3::from(g.matrix3.x_axis).length());
        let length = path_of(c, node)?.spline.length() * scale;
        c.push_f64(f64::from(length));
        Ok(())
    });
    b.add("path.point", |c| {
        let node = c.node(0)?;
        let global = c.tree.global_transform(node).unwrap_or_default();
        let scale = Vec3::from(global.matrix3.x_axis).length().max(1e-6);
        let distance = c.f32(1)? / scale;
        let (p, d) = path_of(c, node)?
            .spline
            .at_distance(distance)
            .ok_or_else(|| CallError::failed("the path has no points"))?;
        c.push_vec3(global.transform_point3(p));
        c.push_vec3(global.transform_vector3(d).normalize_or(Vec3::Z));
        Ok(())
    });
    b.add("config.number", |c| {
        let value = game_config(c)?
            .number(c.str(0)?)
            .map_err(CallError::failed)?;
        c.push_f64(value);
        Ok(())
    });
    b.add("config.int", |c| {
        let value = game_config(c)?.int(c.str(0)?).map_err(CallError::failed)?;
        c.push_int(value);
        Ok(())
    });
    b.add("config.bool", |c| {
        let value = game_config(c)?.bool(c.str(0)?).map_err(CallError::failed)?;
        c.push_bool(value);
        Ok(())
    });
    b.add("config.text", |c| {
        let value = game_config(c)?
            .text(c.str(0)?)
            .map_err(CallError::failed)?
            .to_owned();
        c.push_str(&value);
        Ok(())
    });
    b.add("config.set", |c| {
        let key = c.str(0)?;
        let raw = c.raw(1);
        let mut config = c
            .tree
            .world()
            .resource_mut::<GameConfig>()
            .ok_or_else(|| CallError::failed("the game has no config"))?;
        let is_int = matches!(
            config.entry(key).map(|e| &e.default),
            Ok(ConfigValue::Int(_))
        );
        // SAFETY: the value is an argument of the current call.
        let value = if let Some(v) = raw.as_bool() {
            ConfigValue::Bool(v)
        } else if let Some(v) = unsafe { raw.as_str() } {
            ConfigValue::Text(v.to_owned())
        } else if let Some(v) = raw.as_int().filter(|_| is_int) {
            ConfigValue::Int(v)
        } else if let Some(v) = raw.as_float() {
            ConfigValue::Float(v)
        } else {
            return Err(CallError::BadArguments(
                "config values are bools, numbers or text".into(),
            ));
        };
        config.set(key, value).map_err(CallError::failed)
    });
    b.add("effects.spawn", |c| {
        let (parent, position) = (c.node(0)?, c.vec3(1)?);
        let name = c.str(2)?;
        let mut settings = ParticleSettings::preset(name)
            .ok_or_else(|| CallError::BadArguments(format!("unknown particle preset `{name}`")))?;
        if let Some(amount) = c.opt_u32(3)? {
            settings = settings.with_amount(amount);
        }
        if let Some(lifetime) = c.opt_f32(4)? {
            settings = settings.with_lifetime(lifetime);
        }
        if let (Some(start), Some(end)) = (c.opt_f32(5)?, c.opt_f32(6)?) {
            settings = settings.with_size(start, end);
        }
        if let (Some(min), Some(max)) = (c.opt_f32(7)?, c.opt_f32(8)?) {
            settings = settings.with_speed(min, max);
        }
        if let (Some(start), Some(end)) = (c.opt_color(9)?, c.opt_color(10)?) {
            settings = settings.with_colors(start, end);
        }
        if let (Some(direction), Some(spread)) = (c.opt_vec3(11)?, c.opt_f32(12)?) {
            settings = settings.with_direction(direction, spread);
        }
        let node = c.tree.spawn_effect(parent, position, settings)?;
        c.push_node(node);
        Ok(())
    });
    b.add("effects.explosion", |c| {
        let (parent, position) = (c.node(0)?, c.vec3(1)?);
        let options = ExplosionOptions {
            scale: c.f32(2)?.max(0.01),
            flash: c.f32(3)?.max(0.0),
            shake: c.f32(4)?.max(0.0),
            distortion: c.f32(5)?.max(0.0),
        };
        let node = c.tree.spawn_explosion(parent, position, options)?;
        c.push_node(node);
        Ok(())
    });
    b.add("effects.blood", |c| {
        let (parent, position) = (c.node(0)?, c.vec3(1)?);
        let mut options = BloodOptions::along(c.vec3(2)?);
        options.amount = c.f32(3)?.max(0.0);
        options.splats = c.u32(4)?;
        if let Some(lifetime) = c.opt_f32(5)? {
            options.splat_lifetime = lifetime.max(0.0);
        }
        if let Some(color) = c.opt_color(6)? {
            options.color = color;
        }
        let node = c.tree.spawn_blood(parent, position, options)?;
        c.push_node(node);
        Ok(())
    });
    // Animation: `node` is the model (or any node above its AnimationPlayer).
    b.add("animation.play", |c| {
        let node = player_node(c.tree, c.node(0)?)?;
        let name = c.str(1)?.to_owned();
        let fade = c.opt_f32(2)?.unwrap_or(0.0);
        let looping = c.opt_bool(3)?;
        let mut player = c
            .tree
            .component_mut::<AnimationPlayer>(node)
            .expect("found above");
        if !player.crossfade_named(&name, fade) {
            let known: Vec<&str> = player.clip_names().collect();
            return Err(CallError::Failed(format!(
                "no animation `{name}` (the model has: {})",
                known.join(", ")
            )));
        }
        if let Some(looping) = looping {
            player.looping = looping;
        }
        Ok(())
    });
    b.add("animation.stop", |c| {
        let node = player_node(c.tree, c.node(0)?)?;
        if let Some(mut player) = c.tree.component_mut::<AnimationPlayer>(node) {
            player.stop();
        }
        Ok(())
    });
    b.add("animation.set_speed", |c| {
        let node = player_node(c.tree, c.node(0)?)?;
        let speed = c.f32(1)?;
        if let Some(mut player) = c.tree.component_mut::<AnimationPlayer>(node) {
            player.speed = speed;
        }
        Ok(())
    });
    b.add("animation.current", |c| {
        let node = player_node(c.tree, c.node(0)?)?;
        let player = c.tree.component::<AnimationPlayer>(node).expect("found");
        if let Some(name) = player.current_name() {
            c.push_str(name);
        }
        Ok(())
    });
    b.add("animation.is_playing", |c| {
        let node = player_node(c.tree, c.node(0)?)?;
        let playing = c
            .tree
            .component::<AnimationPlayer>(node)
            .is_some_and(|p| p.playing);
        c.push_bool(playing);
        Ok(())
    });
    b.add("animation.clips", |c| {
        let node = player_node(c.tree, c.node(0)?)?;
        let player = c.tree.component::<AnimationPlayer>(node).expect("found");
        let names: Vec<&str> = player.clip_names().collect();
        c.push_str(&names.join(","));
        Ok(())
    });

    b.add("trace.damage", |c| {
        use crate::trace::Tracing;
        let (source, target, amount) = (c.opt_node(0)?, c.node(1)?, c.f32(2)?);
        c.tree.deal_damage(source, target, amount);
        Ok(())
    });
    b.add("trace.heal", |c| {
        use crate::trace::Tracing;
        let (source, target, amount) = (c.opt_node(0)?, c.node(1)?, c.f32(2)?);
        c.tree.heal(source, target, amount);
        Ok(())
    });
    b.add("trace.note", |c| {
        use crate::trace::Tracing;
        let node = c.node(0)?;
        let text = c.str(1)?.to_owned();
        c.tree.trace_note(node, text);
        Ok(())
    });
    b.add("trace.watch", |c| {
        use crate::trace::Tracing;
        let (node, watch) = (c.node(0)?, c.bool(1)?);
        c.tree.trace_watch(node, watch);
        Ok(())
    });
    b.add("trace.health", |c| {
        use crate::trace::Tracing;
        let (node, current, max) = (c.node(0)?, c.f32(1)?, c.f32(2)?);
        c.tree.report_health(node, current, max);
        Ok(())
    });

    b.add("head_bob.set", |c| {
        let node = c.node(0)?;
        let strength = c.f32(1)?.max(0.0);
        let follow = c.opt_node(2)?;
        let base = crate::head_bob::HeadBob::default();
        let mut bob = c
            .tree
            .component::<crate::head_bob::HeadBob>(node)
            .map(|b| b.clone())
            .unwrap_or_default();
        bob.height = base.height * strength;
        bob.width = base.width * strength;
        bob.roll = base.roll * strength;
        bob.strafe_tilt = base.strafe_tilt * strength;
        bob.landing_dip = base.landing_dip * strength;
        bob.breathing = base.breathing * strength;
        bob.follow = follow;
        c.tree
            .insert_component(node, bob)
            .map_err(CallError::failed)?;
        Ok(())
    });

    // Locomotion: added on the model's AnimationPlayer node on first use.
    b.add("locomotion.update", |c| {
        let node = locomotion_node(c.tree, c.node(0)?)?;
        let mut loco = c.tree.component_mut::<Locomotion>(node).expect("added");
        loco.speed = c.f32(1)?;
        loco.grounded = c.bool(2)?;
        loco.vertical_speed = c.f32(3)?;
        loco.climbing = c.bool(4)?;
        Ok(())
    });
    b.add("locomotion.action", |c| {
        let node = locomotion_node(c.tree, c.node(0)?)?;
        let name = c.str(1)?.to_owned();
        let known = c
            .tree
            .component::<AnimationPlayer>(node)
            .is_some_and(|p| p.clip_named(&name).is_some());
        if !known {
            return Err(CallError::Failed(format!("no animation `{name}`")));
        }
        c.tree
            .component_mut::<Locomotion>(node)
            .expect("added")
            .play_action(&name);
        Ok(())
    });
    b.add("locomotion.action_playing", |c| {
        let node = locomotion_node(c.tree, c.node(0)?)?;
        let playing = c
            .tree
            .component::<Locomotion>(node)
            .is_some_and(|l| l.is_action_playing());
        c.push_bool(playing);
        Ok(())
    });
    b.add("locomotion.state", |c| {
        let node = locomotion_node(c.tree, c.node(0)?)?;
        let state = c.tree.component::<Locomotion>(node).map(|l| l.state());
        c.push_str(&format!("{:?}", state.unwrap_or_default()).to_lowercase());
        Ok(())
    });
    b.add("locomotion.set_clips", |c| {
        let node = locomotion_node(c.tree, c.node(0)?)?;
        // "Idle:0,Walk:1.5,Run:4.2"
        let ground: Result<Vec<(String, f32)>, CallError> = c
            .str(1)?
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .map(|entry| {
                let (name, speed) = entry.split_once(':').ok_or_else(|| {
                    CallError::BadArguments(format!("`{entry}`: expected `Clip:speed`"))
                })?;
                let speed = speed.trim().parse::<f32>().map_err(|_| {
                    CallError::BadArguments(format!("`{entry}`: the speed is not a number"))
                })?;
                Ok((name.trim().to_owned(), speed))
            })
            .collect();
        let mut ground = ground?;
        ground.sort_by(|a, b| a.1.total_cmp(&b.1));
        let (jump, fall, climb) = (c.opt_str(2)?, c.opt_str(3)?, c.opt_str(4)?);
        let mut loco = c.tree.component_mut::<Locomotion>(node).expect("added");
        loco.ground = ground;
        let name = |s: Option<&str>| s.filter(|s| !s.is_empty()).map(str::to_owned);
        loco.jump = name(jump);
        loco.fall = name(fall);
        loco.climb = name(climb);
        Ok(())
    });
    b.add("effects.shake_cameras", |c| {
        shake_cameras(c.tree.world(), c.f32(0)?.max(0.0));
        Ok(())
    });

    b.add("tween.start", |c| {
        let node = c.node(0)?;
        let steps = tween_steps(c)?;
        if c.tree.position(node).is_none() {
            return Err(CallError::Failed(format!("{node} is not a 3D node")));
        }
        let mut builder = c.tree.tween(node);
        for step in steps {
            builder = match step {
                Step::Easing(e) => builder.easing(e),
                Step::Repeat(r) => builder.repeat(r),
                Step::Delay(s) => builder.delay(s),
                Step::Then => builder.then(),
                Step::MoveTo(v, s) => builder.move_to(v, s),
                Step::MoveBy(v, s) => builder.move_by(v, s),
                Step::RotateTo(q, s) => builder.rotate_to(q, s),
                Step::ScaleTo(v, s) => builder.scale_to(v, s),
            };
        }
        Ok(())
    });
    b.add("tween.kill", |c| {
        let node = c.node(0)?;
        c.tree.kill_tweens(node);
        Ok(())
    });
    b.add("tween.is_tweening", |c| {
        let node = c.node(0)?;
        c.push_bool(c.tree.is_tweening(node));
        Ok(())
    });

    b.add("audio.play", |c| {
        let stream = sound(c, c.str(0)?)?;
        let options = SoundOptions {
            volume_db: c.f32(1)?,
            pitch_scale: c.f32(2)?.max(0.01),
            ..Default::default()
        };
        audio(c, |a| a.play(&stream, options))
    });
    b.add("audio.play_at", |c| {
        let stream = sound(c, c.str(0)?)?;
        let position = c.vec3(1)?;
        let options = SoundOptions {
            volume_db: c.f32(2)?,
            pitch_scale: c.f32(3)?.max(0.01),
            ..Default::default()
        };
        audio(c, |a| a.play_at(&stream, position, options))
    });
    b.add("audio.master_volume_db", |c| {
        c.push_f32(audio(c, |a| a.master_volume_db())?);
        Ok(())
    });
    b.add("audio.set_master_volume_db", |c| {
        let volume = c.f32(0)?;
        audio(c, |a| a.set_master_volume_db(volume))
    });

    b.add("nav.bake", |c| {
        let settings = NavBakeSettings {
            min: c.vec3(0)?,
            max: c.vec3(1)?,
            cell_size: c.f32(2)?.max(0.01),
            agent_radius: c.f32(3)?.max(0.0),
            agent_height: c.f32(4)?.max(0.01),
            max_step: c.f32(5)?.max(0.0),
            query: QueryOptions::new().with_mask(c.u32(6)?),
            ..Default::default()
        };
        let grid = c
            .tree
            .world()
            .resource::<PhysicsWorld>()
            .map(|physics| NavGrid::bake(&physics, &settings))
            .ok_or_else(|| CallError::failed("this scene has no physics to bake from"))?;
        let walkable = grid.walkable_count();
        c.tree.world_mut().insert_resource(grid);
        c.push_int(walkable as i64);
        Ok(())
    });
    b.add("nav.clear", |c| {
        c.tree.world_mut().remove_resource::<NavGrid>();
        Ok(())
    });
    b.add("nav.has_grid", |c| {
        c.push_bool(c.tree.world().resource::<NavGrid>().is_some());
        Ok(())
    });
    b.add("nav.find_path", |c| {
        let (from, to) = (c.vec3(0)?, c.vec3(1)?);
        let path = c
            .tree
            .world()
            .resource::<NavGrid>()
            .and_then(|grid| grid.find_path(from, to));
        for point in path.into_iter().flatten() {
            c.push_vec3(point);
        }
        Ok(())
    });

    b.add("timer.create", |c| {
        let mut timer = Timer::new(c.f32(1)?.max(0.0));
        if c.bool(2)? {
            timer = timer.one_shot();
        }
        if c.bool(3)? {
            timer = timer.autostart();
        }
        let node = c.tree.create_timer_node(c.str(0)?, timer);
        c.push_node(node);
        Ok(())
    });
    b.add("timer.start", |c| {
        let node = c.node(0)?;
        timer(c, node, Timer::start)
    });
    b.add("timer.stop", |c| {
        let node = c.node(0)?;
        timer(c, node, Timer::stop)
    });
    b.add("particles.restart", |c| {
        let node = c.node(0)?;
        let mut emitter = c
            .tree
            .component_mut::<ParticleEmitter>(node)
            .ok_or_else(|| CallError::Failed(format!("{node} has no particle emitter")))?;
        emitter.restart();
        Ok(())
    });
}

fn timer(
    c: &Call<'_>,
    node: ab_scene::NodeHandle,
    f: impl FnOnce(&mut Timer),
) -> Result<(), CallError> {
    let mut timer = c
        .tree
        .component_mut::<Timer>(node)
        .ok_or_else(|| CallError::Failed(format!("{node} is not a timer")))?;
    f(&mut timer);
    Ok(())
}
