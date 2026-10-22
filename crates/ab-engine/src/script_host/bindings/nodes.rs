//! Nodes, transforms, groups, scripts, signals, time, input, the engine, scenes and saves.

use super::Builder;
use crate::save::SaveStore;
use crate::scene_file::{instantiate_scene, load_scene, SceneFile};
use crate::script_host::call::{intern, signal_arg, Call, CallError};
use crate::{EngineControl, RenderStats};
use ab_core::{Affine3A, Time, Transform3D};
use ab_input::Actions;
use ab_scene::{ConnectionId, NodeHandle, SceneTree, Signal};

pub(super) fn add(b: &mut Builder) {
    // Tree.
    b.add("tree.root", |c| {
        c.push_node(c.tree.root());
        Ok(())
    });
    b.add("tree.node_count", |c| {
        c.push_int(c.tree.node_count() as i64);
        Ok(())
    });
    b.add("tree.update_transforms", |c| {
        c.tree.update_transforms();
        Ok(())
    });

    // Nodes.
    b.add("node.is_valid", |c| {
        let valid = c
            .raw(0)
            .as_node()
            .is_some_and(|n| c.tree.is_valid(NodeHandle::from_bits(n.0)));
        c.push_bool(valid);
        Ok(())
    });
    b.add("node.is_inside_tree", |c| {
        let node = c.node(0)?;
        c.push_bool(c.tree.is_inside_tree(node));
        Ok(())
    });
    b.add("node.create", |c| {
        let node = c.tree.create_node(c.str(0)?);
        c.push_node(node);
        Ok(())
    });
    b.add("node.create_3d", |c| {
        let node = c.tree.create_node3d(c.str(0)?);
        c.push_node(node);
        Ok(())
    });
    b.add("node.name", |c| {
        let node = c.node(0)?;
        let name = c.tree.name(node).map(|n| n.to_owned()).unwrap_or_default();
        c.push_str(&name);
        Ok(())
    });
    b.add("node.set_name", |c| {
        let node = c.node(0)?;
        Ok(c.tree.set_name(node, c.str(1)?)?)
    });
    b.add("node.class_name", |c| {
        let node = c.node(0)?;
        c.push_str(c.tree.class_name(node).unwrap_or("Node"));
        Ok(())
    });
    b.add("node.get_node", |c| {
        let node = c.node(0)?;
        c.push_opt_node(c.tree.get_node(node, c.str(1)?));
        Ok(())
    });
    b.add("node.find_child", |c| {
        let node = c.node(0)?;
        c.push_opt_node(c.tree.find_child(node, c.str(1)?, true));
        Ok(())
    });
    b.add("node.parent", |c| {
        let node = c.node(0)?;
        c.push_opt_node(c.tree.parent(node));
        Ok(())
    });
    b.add("node.children", |c| {
        let node = c.node(0)?;
        for child in c.tree.children(node) {
            c.push_node(child);
        }
        Ok(())
    });
    b.add("node.child_count", |c| {
        let node = c.node(0)?;
        c.push_int(c.tree.child_count(node) as i64);
        Ok(())
    });
    b.add("node.path", |c| {
        let node = c.node(0)?;
        let path = c.tree.path_of(node).unwrap_or_default();
        c.push_str(&path);
        Ok(())
    });
    b.add("node.add_child", |c| {
        let (parent, child) = (c.node(0)?, c.node(1)?);
        Ok(c.tree.add_child(parent, child)?)
    });
    b.add("node.remove_child", |c| {
        let (parent, child) = (c.node(0)?, c.node(1)?);
        Ok(c.tree.remove_child(parent, child)?)
    });
    b.add("node.reparent", |c| {
        let (node, parent, keep) = (c.node(0)?, c.node(1)?, c.bool(2)?);
        Ok(c.tree.reparent(node, parent, keep)?)
    });
    b.add("node.queue_free", |c| {
        let node = c.node(0)?;
        Ok(c.tree.queue_free(node)?)
    });
    b.add("node.is_visible", |c| {
        let node = c.node(0)?;
        c.push_bool(c.tree.is_visible(node));
        Ok(())
    });
    b.add("node.is_visible_in_tree", |c| {
        let node = c.node(0)?;
        c.push_bool(c.tree.is_visible_in_tree(node));
        Ok(())
    });
    b.add("node.set_visible", |c| {
        let node = c.node(0)?;
        Ok(c.tree.set_visible(node, c.bool(1)?)?)
    });
    b.add("node.set_process", |c| {
        let node = c.node(0)?;
        Ok(c.tree.set_process(node, c.bool(1)?)?)
    });
    b.add("node.set_physics_process", |c| {
        let node = c.node(0)?;
        Ok(c.tree.set_physics_process(node, c.bool(1)?)?)
    });
    b.add("node.is_processing", |c| {
        let node = c.node(0)?;
        c.push_bool(c.tree.is_processing(node));
        Ok(())
    });
    b.add("node.is_physics_processing", |c| {
        let node = c.node(0)?;
        c.push_bool(c.tree.is_physics_processing(node));
        Ok(())
    });

    // Groups.
    b.add("group.add", |c| {
        let node = c.node(0)?;
        Ok(c.tree.add_to_group(node, c.str(1)?)?)
    });
    b.add("group.remove", |c| {
        let node = c.node(0)?;
        c.tree.remove_from_group(node, c.str(1)?);
        Ok(())
    });
    b.add("group.has", |c| {
        let node = c.node(0)?;
        c.push_bool(c.tree.is_in_group(node, c.str(1)?));
        Ok(())
    });
    b.add("group.nodes", |c| {
        for node in c.tree.nodes_in_group(c.str(0)?) {
            c.push_node(node);
        }
        Ok(())
    });

    transforms(b);
    scripts_and_signals(b);
    time_input_engine(b);
    scenes_and_files(b);
}

fn transforms(b: &mut Builder) {
    fn push_transform(c: &Call<'_>, t: Transform3D) {
        c.push_vec3(t.translation);
        c.push_quat(t.rotation);
        c.push_vec3(t.scale);
    }
    fn transform_arg(c: &Call<'_>, from: usize) -> Result<Transform3D, CallError> {
        Ok(Transform3D {
            translation: c.vec3(from)?,
            rotation: c.quat(from + 1)?,
            scale: c.vec3(from + 2)?,
        })
    }
    fn not_3d(node: NodeHandle) -> CallError {
        CallError::Failed(format!("{node} is not a 3D node"))
    }

    b.add("node.position", |c| {
        let node = c.node(0)?;
        c.push_vec3(c.tree.position(node).ok_or_else(|| not_3d(node))?);
        Ok(())
    });
    b.add("node.set_position", |c| {
        let node = c.node(0)?;
        Ok(c.tree.set_position(node, c.vec3(1)?)?)
    });
    b.add("node.rotation", |c| {
        let node = c.node(0)?;
        c.push_quat(c.tree.rotation(node).ok_or_else(|| not_3d(node))?);
        Ok(())
    });
    b.add("node.set_rotation", |c| {
        let node = c.node(0)?;
        Ok(c.tree.set_rotation(node, c.quat(1)?)?)
    });
    b.add("node.scale", |c| {
        let node = c.node(0)?;
        c.push_vec3(c.tree.scale(node).ok_or_else(|| not_3d(node))?);
        Ok(())
    });
    b.add("node.set_scale", |c| {
        let node = c.node(0)?;
        Ok(c.tree.set_scale(node, c.vec3(1)?)?)
    });
    b.add("node.transform", |c| {
        let node = c.node(0)?;
        let transform = c.tree.transform(node).ok_or_else(|| not_3d(node))?;
        push_transform(c, transform);
        Ok(())
    });
    b.add("node.set_transform", |c| {
        let node = c.node(0)?;
        Ok(c.tree.set_transform(node, transform_arg(c, 1)?)?)
    });
    b.add("node.global_transform", |c| {
        let node = c.node(0)?;
        let global = c.tree.global_transform(node).ok_or_else(|| not_3d(node))?;
        push_transform(c, Transform3D::from_affine(&global));
        Ok(())
    });
    b.add("node.set_global_transform", |c| {
        let node = c.node(0)?;
        let t = transform_arg(c, 1)?;
        let global = Affine3A::from_scale_rotation_translation(t.scale, t.rotation, t.translation);
        Ok(c.tree.set_global_transform(node, global)?)
    });
    b.add("node.global_position", |c| {
        let node = c.node(0)?;
        c.push_vec3(c.tree.global_position(node).ok_or_else(|| not_3d(node))?);
        Ok(())
    });
    b.add("node.set_global_position", |c| {
        let node = c.node(0)?;
        Ok(c.tree.set_global_position(node, c.vec3(1)?)?)
    });
    b.add("node.translate", |c| {
        let node = c.node(0)?;
        Ok(c.tree.translate(node, c.vec3(1)?)?)
    });
    b.add("node.global_translate", |c| {
        let node = c.node(0)?;
        Ok(c.tree.global_translate(node, c.vec3(1)?)?)
    });
    b.add("node.rotate", |c| {
        let node = c.node(0)?;
        Ok(c.tree
            .rotate(node, c.vec3(1)?.normalize_or(ab_core::Vec3::Y), c.f32(2)?)?)
    });
    b.add("node.rotate_object_local", |c| {
        let node = c.node(0)?;
        Ok(c.tree.rotate_object_local(
            node,
            c.vec3(1)?.normalize_or(ab_core::Vec3::Y),
            c.f32(2)?,
        )?)
    });
    b.add("node.look_at", |c| {
        let node = c.node(0)?;
        Ok(c.tree.look_at(node, c.vec3(1)?, c.vec3(2)?)?)
    });
}

fn scripts_and_signals(b: &mut Builder) {
    b.add("script.set", |c| {
        let node = c.node(0)?;
        Ok(c.tree.set_script_named(node, c.str(1)?)?)
    });
    b.add("script.add", |c| {
        let node = c.node(0)?;
        Ok(c.tree.add_script_named(node, c.str(1)?)?)
    });
    b.add("script.remove", |c| {
        let node = c.node(0)?;
        c.tree.remove_script_named(node, c.str(1)?);
        Ok(())
    });
    b.add("script.names", |c| {
        let node = c.node(0)?;
        for name in c.tree.script_names(node) {
            c.push_str(&name);
        }
        Ok(())
    });

    b.add("signal.connect", |c| {
        let (source, target) = (c.node(0)?, c.node(2)?);
        let id = c.tree.connect(source, intern(c.str(1)?), target);
        c.push_int(id.to_bits() as i64);
        Ok(())
    });
    b.add("signal.disconnect", |c| {
        let id = ConnectionId::from_bits(c.i64(0)? as u64);
        let existed = c.tree.disconnect(id);
        c.push_bool(existed);
        Ok(())
    });
    b.add("signal.emit", |c| {
        let source = c.node(0)?;
        let mut signal = Signal::new(source, intern(c.str(1)?));
        if let Some(arg) = signal_arg(&c.raw(2)) {
            signal = signal.with_arg_boxed(arg);
        }
        c.tree.emit(signal);
        Ok(())
    });
}

fn time_input_engine(b: &mut Builder) {
    fn time<R>(c: &Call<'_>, f: impl FnOnce(&Time) -> R) -> Result<R, CallError> {
        let time = c
            .tree
            .world()
            .resource::<Time>()
            .ok_or_else(|| CallError::failed("no Time resource"))?;
        Ok(f(&time))
    }
    b.add("time.elapsed", |c| {
        c.push_f64(time(c, |t| t.elapsed)?);
        Ok(())
    });
    b.add("time.frame", |c| {
        c.push_int(time(c, |t| t.frame_count as i64)?);
        Ok(())
    });
    b.add("time.fixed_delta", |c| {
        c.push_f32(time(c, |t| t.fixed_delta)?);
        Ok(())
    });
    b.add("time.scale", |c| {
        c.push_f32(time(c, |t| t.time_scale)?);
        Ok(())
    });
    b.add("time.fps", |c| {
        c.push_f32(time(c, Time::fps)?);
        Ok(())
    });
    b.add("time.set_scale", |c| {
        let scale = c.f32(0)?.max(0.0);
        if let Some(mut time) = c.tree.world().resource_mut::<Time>() {
            time.time_scale = scale;
        }
        Ok(())
    });

    fn actions<R>(c: &Call<'_>, f: impl FnOnce(&Actions) -> R) -> Result<R, CallError> {
        let actions = c
            .tree
            .world()
            .resource::<Actions>()
            .ok_or_else(|| CallError::failed("no input map"))?;
        Ok(f(&actions))
    }
    b.add("input.is_pressed", |c| {
        let action = c.str(0)?;
        c.push_bool(actions(c, |a| a.is_pressed(action))?);
        Ok(())
    });
    b.add("input.is_just_pressed", |c| {
        let action = c.str(0)?;
        c.push_bool(actions(c, |a| a.is_just_pressed(action))?);
        Ok(())
    });
    b.add("input.is_just_released", |c| {
        let action = c.str(0)?;
        c.push_bool(actions(c, |a| a.is_just_released(action))?);
        Ok(())
    });
    b.add("input.strength", |c| {
        let action = c.str(0)?;
        c.push_f32(actions(c, |a| a.strength(action))?);
        Ok(())
    });
    b.add("input.axis", |c| {
        let (negative, positive) = (c.str(0)?, c.str(1)?);
        c.push_f32(actions(c, |a| a.axis(negative, positive))?);
        Ok(())
    });
    b.add("input.vector", |c| {
        let (l, r, d, u) = (c.str(0)?, c.str(1)?, c.str(2)?, c.str(3)?);
        c.push_vec2(actions(c, |a| a.vector(l, r, d, u))?);
        Ok(())
    });
    b.add("input.look", |c| {
        c.push_vec2(actions(c, Actions::look)?);
        Ok(())
    });
    b.add("input.has_action", |c| {
        let action = c.str(0)?;
        c.push_bool(actions(c, |a| a.map().has_action(action))?);
        Ok(())
    });

    fn control<R>(c: &Call<'_>, f: impl FnOnce(&mut EngineControl) -> R) -> Result<R, CallError> {
        let mut control = c
            .tree
            .world()
            .resource_mut::<EngineControl>()
            .ok_or_else(|| CallError::failed("not running in an engine"))?;
        Ok(f(&mut control))
    }
    b.add("engine.quit", |c| control(c, EngineControl::quit));
    b.add("engine.is_mouse_captured", |c| {
        c.push_bool(control(c, |e| e.is_mouse_captured())?);
        Ok(())
    });
    b.add("engine.set_mouse_captured", |c| {
        let captured = c.bool(0)?;
        control(c, |e| e.set_mouse_captured(captured))
    });
    b.add("engine.is_fullscreen", |c| {
        c.push_bool(control(c, |e| e.is_fullscreen())?);
        Ok(())
    });
    b.add("engine.set_fullscreen", |c| {
        let fullscreen = c.bool(0)?;
        control(c, |e| e.set_fullscreen(fullscreen))
    });
    b.add("render.stats", |c| {
        let stats = c
            .tree
            .world()
            .resource::<RenderStats>()
            .map(|s| s.0)
            .unwrap_or_default();
        for value in [
            stats.draw_calls,
            stats.objects,
            stats.culled,
            stats.shadow_draw_calls,
            stats.lights,
        ] {
            c.push_int(i64::from(value));
        }
        Ok(())
    });
}

fn scenes_and_files(b: &mut Builder) {
    fn scene_error(err: impl std::fmt::Display) -> CallError {
        CallError::failed(err)
    }
    b.add("scene.load", |c| {
        let path = c.str(0)?;
        let node = load_scene(c.tree, path).map_err(scene_error)?;
        c.push_node(node);
        Ok(())
    });
    b.add("scene.instantiate", |c| {
        let (path, parent) = (c.str(0)?, c.node(1)?);
        let node = load_scene(c.tree, path).map_err(scene_error)?;
        if let Err(err) = c.tree.add_child(parent, node) {
            let _ = c.tree.queue_free(node);
            return Err(err.into());
        }
        c.push_node(node);
        Ok(())
    });
    // An effect scene (assets/effects/...) at a position, freed when it has played.
    b.add("effects.spawn_scene", |c| {
        let (parent, position, path) = (c.node(0)?, c.vec3(1)?, c.str(2)?);
        let node = load_scene(c.tree, path).map_err(scene_error)?;
        if let Err(err) = c.tree.add_child(parent, node) {
            let _ = c.tree.queue_free(node);
            return Err(err.into());
        }
        let _ = c.tree.set_global_position(node, position);
        ab_effects::free_when_finished(c.tree, node)?;
        c.push_node(node);
        Ok(())
    });
    b.add("scene.from_toml", |c| {
        let scene = SceneFile::from_toml(c.str(0)?).map_err(scene_error)?;
        let node = instantiate_scene(c.tree, &scene, "<script>").map_err(scene_error)?;
        c.push_node(node);
        Ok(())
    });
    b.add("scene.change", |c| {
        let path = c.str(0)?;
        let node = crate::change_scene(c.tree, path).map_err(scene_error)?;
        c.push_node(node);
        Ok(())
    });
    b.add("scene.reload", |c| {
        let node = crate::reload_scene(c.tree).map_err(scene_error)?;
        c.push_node(node);
        Ok(())
    });
    b.add("assets.read_text", |c| {
        let path = c.str(0)?;
        let bytes = c
            .tree
            .world()
            .resource::<ab_asset::AssetServer>()
            .ok_or_else(|| CallError::failed("no AssetServer"))?
            .read(path)
            .map_err(CallError::failed)?;
        let text = String::from_utf8(bytes)
            .map_err(|_| CallError::Failed(format!("{path} is not UTF-8 text")))?;
        c.push_str(&text);
        Ok(())
    });

    fn store(tree: &SceneTree) -> Result<SaveStore, CallError> {
        tree.world()
            .resource::<SaveStore>()
            .map(|s| s.clone())
            .ok_or_else(|| CallError::failed("this game has no save folder"))
    }
    b.add("save.write", |c| {
        let store = store(c.tree)?;
        store
            .save(c.str(0)?, &serde_json::Value::String(c.str(1)?.to_owned()))
            .map_err(CallError::failed)
    });
    b.add("save.write_json", |c| {
        let store = store(c.tree)?;
        let value: serde_json::Value = serde_json::from_str(c.str(1)?)
            .map_err(|err| CallError::BadArguments(format!("not JSON: {err}")))?;
        store.save(c.str(0)?, &value).map_err(CallError::failed)
    });
    b.add("save.read", |c| {
        let store = store(c.tree)?;
        match store
            .load::<serde_json::Value>(c.str(0)?)
            .map_err(CallError::failed)?
        {
            None => {}
            Some(serde_json::Value::String(text)) => c.push_str(&text),
            Some(value) => c.push_str(&value.to_string()),
        }
        Ok(())
    });
    b.add("save.exists", |c| {
        let store = store(c.tree)?;
        c.push_bool(store.exists(c.str(0)?));
        Ok(())
    });
    b.add("save.delete", |c| {
        let store = store(c.tree)?;
        store
            .delete(c.str(0)?)
            .map(|_| ())
            .map_err(CallError::failed)
    });
}
