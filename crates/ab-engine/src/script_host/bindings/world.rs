//! Effects, tweens, audio, navigation, timers, particles.

use super::ui::easing;
use super::Builder;
use crate::script_host::call::{Call, CallError};
use crate::tween::{Repeat, TweenNodes};
use ab_asset::AssetServer;
use ab_audio::{Audio, SoundOptions};
use ab_core::{Quat, Vec3};
use ab_effects::{shake_cameras, EffectNodes, ExplosionOptions, ParticleEmitter, ParticleSettings};
use ab_navigation::{NavBakeSettings, NavGrid};
use ab_physics::{PhysicsWorld, QueryOptions};
use ab_scene::Timer;
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

pub(super) fn add(b: &mut Builder) {
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
