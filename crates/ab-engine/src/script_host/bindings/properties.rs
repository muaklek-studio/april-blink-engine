//! Component properties by path (`"light.energy"`), for `node.get_property` and
//! `node.set_property`.

use super::Builder;
use crate::script_host::call::{from_color, from_vec2, from_vec3, Call, CallError, CallResult};
use ab_audio::AudioPlayer;
use ab_core::Color;
use ab_ecs::Component;
use ab_effects::ParticleEmitter;
use ab_physics::{BodyKind, CharacterBody3D, PhysicsBody3D};
use ab_rendering::{
    Camera3D, DirectionalLight3D, MaterialHandle, MeshInstance3D, OmniLight3D, RenderAssets,
    SpotLight3D,
};
use ab_scene::{NodeHandle, SceneTree, Timer};
use ab_script_api as ffi;
use ab_ui::{
    BoxContainer, Button, Control, HAlign, Label, MouseFilter, Panel, ProgressBar, VAlign,
};

/// Every property path scripts can use.
const PROPERTIES: &[&str] = &[
    "light.energy",
    "light.color",
    "light.range",
    "light.shadows",
    "camera.fov",
    "camera.near",
    "camera.far",
    "camera.current",
    "mesh.color",
    "mesh.emission",
    "mesh.emission_energy",
    "mesh.cast_shadows",
    "particles.emitting",
    "particles.amount",
    "body.kind",
    "body.mass",
    "body.linear_velocity",
    "body.angular_velocity",
    "body.collision_layer",
    "body.collision_mask",
    "body.gravity_scale",
    "body.linear_damping",
    "body.angular_damping",
    "body.friction",
    "body.bounce",
    "body.sleeping",
    "character.velocity",
    "character.up",
    "character.floor_max_angle",
    "character.floor_snap_length",
    "character.max_step_height",
    "character.push_rigid_bodies",
    "character.on_floor",
    "character.on_wall",
    "character.on_ceiling",
    "character.floor_normal",
    "control.modulate",
    "control.alpha",
    "control.translation",
    "control.min_size",
    "control.expand",
    "control.mouse_filter",
    "label.text",
    "label.key",
    "label.font_size",
    "label.color",
    "label.outline_width",
    "label.outline_color",
    "label.autowrap",
    "label.h_align",
    "label.v_align",
    "button.text",
    "button.key",
    "button.disabled",
    "button.font_size",
    "progress.value",
    "progress.min",
    "progress.max",
    "progress.fill_color",
    "progress.background_color",
    "progress.corner_radius",
    "progress.show_percentage",
    "panel.color",
    "panel.corner_radius",
    "panel.border_width",
    "panel.border_color",
    "box.separation",
    "box.alignment",
    "timer.wait_time",
    "timer.one_shot",
    "timer.time_left",
    "timer.stopped",
    "audio_player.volume_db",
    "audio_player.pitch_scale",
    "audio_player.looping",
    "audio_player.playing",
];

pub(crate) fn names() -> Vec<&'static str> {
    PROPERTIES.to_vec()
}

pub(super) fn add(b: &mut Builder) {
    b.add("node.get_property", |c| {
        let node = c.node(0)?;
        let path = c.str(1)?;
        let value = get(c.tree, node, path)?;
        match value {
            Out::Str(text) => c.push_str(&text),
            Out::Value(value) => c.push(value),
        }
        Ok(())
    });
    b.add("node.set_property", |c| {
        let node = c.node(0)?;
        let path = c.str(1)?;
        set(c, node, path, c.raw(2))
    });
}

/// A property's value: plain data, or text kept alive until it is pushed.
enum Out {
    Value(ffi::Value),
    Str(String),
}

fn float(v: f32) -> Out {
    Out::Value(ffi::Value::float(f64::from(v)))
}

fn boolean(v: bool) -> Out {
    Out::Value(ffi::Value::bool(v))
}

fn int(v: u32) -> Out {
    Out::Value(ffi::Value::int(i64::from(v)))
}

fn color(v: Color) -> Out {
    Out::Value(crate::script_host::call::to_color(v))
}

fn vec3(v: ab_core::Vec3) -> Out {
    Out::Value(crate::script_host::call::to_vec3(v))
}

fn vec2(v: ab_core::Vec2) -> Out {
    Out::Value(crate::script_host::call::to_vec2(v))
}

fn text(v: &str) -> Out {
    Out::Str(v.to_owned())
}

fn missing(node: NodeHandle, what: &str) -> CallError {
    CallError::Failed(format!("{node} has no {what}"))
}

fn unknown(path: &str) -> CallError {
    CallError::BadArguments(format!("unknown property `{path}`"))
}

/// Reads component `T` of `node`.
/// A label's or button's translation key (empty without one).
fn text_key(tree: &SceneTree, node: NodeHandle) -> String {
    tree.component::<crate::localization::TextKey>(node)
        .map(|k| k.key.clone())
        .unwrap_or_default()
}

fn read<T: Component, R>(
    tree: &SceneTree,
    node: NodeHandle,
    what: &str,
    f: impl FnOnce(&T) -> R,
) -> Result<R, CallError> {
    tree.component::<T>(node)
        .map(|c| f(&c))
        .ok_or_else(|| missing(node, what))
}

/// Changes component `T` of `node`.
fn write<T: Component>(
    tree: &SceneTree,
    node: NodeHandle,
    what: &str,
    f: impl FnOnce(&mut T),
) -> CallResult {
    let mut component = tree
        .component_mut::<T>(node)
        .ok_or_else(|| missing(node, what))?;
    f(&mut component);
    Ok(())
}

/// The node's light, whichever kind it is.
enum LightField {
    Energy,
    Color,
    Range,
    Shadows,
}

fn light_get(tree: &SceneTree, node: NodeHandle, field: LightField) -> Result<Out, CallError> {
    if let Some(l) = tree.component::<OmniLight3D>(node) {
        return Ok(match field {
            LightField::Energy => float(l.energy),
            LightField::Color => color(l.color),
            LightField::Range => float(l.range),
            LightField::Shadows => boolean(false),
        });
    }
    if let Some(l) = tree.component::<SpotLight3D>(node) {
        return Ok(match field {
            LightField::Energy => float(l.energy),
            LightField::Color => color(l.color),
            LightField::Range => float(l.range),
            LightField::Shadows => boolean(l.shadow_enabled),
        });
    }
    if let Some(l) = tree.component::<DirectionalLight3D>(node) {
        return Ok(match field {
            LightField::Energy => float(l.energy),
            LightField::Color => color(l.color),
            LightField::Range => float(f32::INFINITY),
            LightField::Shadows => boolean(l.shadow_enabled),
        });
    }
    Err(missing(node, "light"))
}

fn light_set(c: &Call<'_>, node: NodeHandle, field: LightField, value: &ffi::Value) -> CallResult {
    let bad = || CallError::BadArguments("wrong value type for the light".into());
    let tree = &*c.tree;
    let number = || value.as_float().map(|v| v as f32).ok_or_else(bad);
    if let Some(mut l) = tree.component_mut::<OmniLight3D>(node) {
        match field {
            LightField::Energy => l.energy = number()?.max(0.0),
            LightField::Color => l.color = from_color(value).ok_or_else(bad)?,
            LightField::Range => l.range = number()?.max(0.0),
            LightField::Shadows => {}
        }
        return Ok(());
    }
    if let Some(mut l) = tree.component_mut::<SpotLight3D>(node) {
        match field {
            LightField::Energy => l.energy = number()?.max(0.0),
            LightField::Color => l.color = from_color(value).ok_or_else(bad)?,
            LightField::Range => l.range = number()?.max(0.0),
            LightField::Shadows => l.shadow_enabled = value.as_bool().ok_or_else(bad)?,
        }
        return Ok(());
    }
    if let Some(mut l) = tree.component_mut::<DirectionalLight3D>(node) {
        match field {
            LightField::Energy => l.energy = number()?.max(0.0),
            LightField::Color => l.color = from_color(value).ok_or_else(bad)?,
            LightField::Range => {}
            LightField::Shadows => l.shadow_enabled = value.as_bool().ok_or_else(bad)?,
        }
        return Ok(());
    }
    Err(missing(node, "light"))
}

fn body_kind_name(kind: BodyKind) -> &'static str {
    match kind {
        BodyKind::Static => "static",
        BodyKind::Rigid => "rigid",
        BodyKind::Kinematic => "kinematic",
        BodyKind::Character => "character",
        BodyKind::Area => "area",
    }
}

fn h_align_name(a: HAlign) -> &'static str {
    match a {
        HAlign::Left => "left",
        HAlign::Center => "center",
        HAlign::Right => "right",
    }
}

fn v_align_name(a: VAlign) -> &'static str {
    match a {
        VAlign::Top => "top",
        VAlign::Center => "center",
        VAlign::Bottom => "bottom",
    }
}

fn mesh_material<R>(
    tree: &SceneTree,
    node: NodeHandle,
    f: impl FnOnce(&ab_rendering::Material) -> R,
) -> Result<R, CallError> {
    let handle = read::<MeshInstance3D, _>(tree, node, "MeshInstance3D", |m| m.material)?;
    let assets = tree
        .world()
        .resource::<RenderAssets>()
        .ok_or_else(|| CallError::failed("no RenderAssets"))?;
    let material = assets
        .material(handle)
        .ok_or_else(|| CallError::failed("the mesh's material is missing"))?;
    Ok(f(material))
}

/// The material a mesh instance owns alone (marked with this component).
struct OwnMaterial(MaterialHandle);

/// Changes the node's own copy of its material (copied on the first change, so other meshes
/// sharing the material keep theirs).
fn mesh_material_mut(
    c: &mut Call<'_>,
    node: NodeHandle,
    f: impl FnOnce(&mut ab_rendering::Material),
) -> CallResult {
    let current = read::<MeshInstance3D, _>(c.tree, node, "MeshInstance3D", |m| m.material)?;
    let owned = c
        .tree
        .component::<OwnMaterial>(node)
        .is_some_and(|own| own.0 == current);
    let handle = {
        let mut assets = c
            .tree
            .world()
            .resource_mut::<RenderAssets>()
            .ok_or_else(|| CallError::failed("no RenderAssets"))?;
        if owned {
            current
        } else {
            let copy = assets
                .material(current)
                .cloned()
                .ok_or_else(|| CallError::failed("the mesh's material is missing"))?;
            assets.add_material(copy)
        }
    };
    if !owned {
        write::<MeshInstance3D>(c.tree, node, "MeshInstance3D", |m| m.material = handle)?;
        c.tree.insert_component(node, OwnMaterial(handle))?;
    }
    let mut assets = c
        .tree
        .world()
        .resource_mut::<RenderAssets>()
        .ok_or_else(|| CallError::failed("no RenderAssets"))?;
    let material = assets
        .material_mut(handle)
        .ok_or_else(|| CallError::failed("the mesh's material is missing"))?;
    f(material);
    Ok(())
}

fn get(tree: &SceneTree, node: NodeHandle, path: &str) -> Result<Out, CallError> {
    Ok(match path {
        "light.energy" => light_get(tree, node, LightField::Energy)?,
        "light.color" => light_get(tree, node, LightField::Color)?,
        "light.range" => light_get(tree, node, LightField::Range)?,
        "light.shadows" => light_get(tree, node, LightField::Shadows)?,

        "camera.fov" => float(read::<Camera3D, _>(tree, node, "Camera3D", |c| {
            c.fov_y_degrees
        })?),
        "camera.near" => float(read::<Camera3D, _>(tree, node, "Camera3D", |c| c.near)?),
        "camera.far" => float(read::<Camera3D, _>(tree, node, "Camera3D", |c| c.far)?),
        "camera.current" => boolean(read::<Camera3D, _>(tree, node, "Camera3D", |c| c.current)?),

        "mesh.color" => color(mesh_material(tree, node, |m| m.albedo)?),
        "mesh.emission" => color(mesh_material(tree, node, |m| m.emission)?),
        "mesh.emission_energy" => float(mesh_material(tree, node, |m| m.emission_energy)?),
        "mesh.cast_shadows" => boolean(read::<MeshInstance3D, _>(
            tree,
            node,
            "MeshInstance3D",
            |m| m.cast_shadows,
        )?),

        "particles.emitting" => boolean(read::<ParticleEmitter, _>(
            tree,
            node,
            "Particles3D",
            |p| p.emitting,
        )?),
        "particles.amount" => int(read::<ParticleEmitter, _>(
            tree,
            node,
            "Particles3D",
            |p| p.settings.amount,
        )?),

        "body.kind" => text(read::<PhysicsBody3D, _>(
            tree,
            node,
            "PhysicsBody3D",
            |b| body_kind_name(b.kind),
        )?),
        "body.mass" => float(read::<PhysicsBody3D, _>(
            tree,
            node,
            "PhysicsBody3D",
            |b| b.mass,
        )?),
        "body.linear_velocity" => vec3(read::<PhysicsBody3D, _>(
            tree,
            node,
            "PhysicsBody3D",
            PhysicsBody3D::linear_velocity,
        )?),
        "body.angular_velocity" => vec3(read::<PhysicsBody3D, _>(
            tree,
            node,
            "PhysicsBody3D",
            PhysicsBody3D::angular_velocity,
        )?),
        "body.collision_layer" => int(read::<PhysicsBody3D, _>(
            tree,
            node,
            "PhysicsBody3D",
            |b| b.collision_layer,
        )?),
        "body.collision_mask" => int(read::<PhysicsBody3D, _>(
            tree,
            node,
            "PhysicsBody3D",
            |b| b.collision_mask,
        )?),
        "body.gravity_scale" => float(read::<PhysicsBody3D, _>(
            tree,
            node,
            "PhysicsBody3D",
            |b| b.gravity_scale,
        )?),
        "body.linear_damping" => float(read::<PhysicsBody3D, _>(
            tree,
            node,
            "PhysicsBody3D",
            |b| b.linear_damping,
        )?),
        "body.angular_damping" => float(read::<PhysicsBody3D, _>(
            tree,
            node,
            "PhysicsBody3D",
            |b| b.angular_damping,
        )?),
        "body.friction" => float(read::<PhysicsBody3D, _>(
            tree,
            node,
            "PhysicsBody3D",
            |b| b.friction,
        )?),
        "body.bounce" => float(read::<PhysicsBody3D, _>(
            tree,
            node,
            "PhysicsBody3D",
            |b| b.bounce,
        )?),
        "body.sleeping" => boolean(read::<PhysicsBody3D, _>(
            tree,
            node,
            "PhysicsBody3D",
            PhysicsBody3D::is_sleeping,
        )?),

        "character.velocity" => vec3(read::<CharacterBody3D, _>(
            tree,
            node,
            "CharacterBody3D",
            |b| b.velocity,
        )?),
        "character.up" => vec3(read::<CharacterBody3D, _>(
            tree,
            node,
            "CharacterBody3D",
            |b| b.up,
        )?),
        "character.floor_max_angle" => float(read::<CharacterBody3D, _>(
            tree,
            node,
            "CharacterBody3D",
            |b| b.floor_max_angle,
        )?),
        "character.floor_snap_length" => float(read::<CharacterBody3D, _>(
            tree,
            node,
            "CharacterBody3D",
            |b| b.floor_snap_length,
        )?),
        "character.max_step_height" => float(read::<CharacterBody3D, _>(
            tree,
            node,
            "CharacterBody3D",
            |b| b.max_step_height,
        )?),
        "character.push_rigid_bodies" => boolean(read::<CharacterBody3D, _>(
            tree,
            node,
            "CharacterBody3D",
            |b| b.push_rigid_bodies,
        )?),
        "character.on_floor" => boolean(read::<CharacterBody3D, _>(
            tree,
            node,
            "CharacterBody3D",
            CharacterBody3D::is_on_floor,
        )?),
        "character.on_wall" => boolean(read::<CharacterBody3D, _>(
            tree,
            node,
            "CharacterBody3D",
            CharacterBody3D::is_on_wall,
        )?),
        "character.on_ceiling" => boolean(read::<CharacterBody3D, _>(
            tree,
            node,
            "CharacterBody3D",
            CharacterBody3D::is_on_ceiling,
        )?),
        "character.floor_normal" => vec3(read::<CharacterBody3D, _>(
            tree,
            node,
            "CharacterBody3D",
            CharacterBody3D::floor_normal,
        )?),

        "control.modulate" => color(read::<Control, _>(tree, node, "Control", |c| c.modulate)?),
        "control.alpha" => float(read::<Control, _>(tree, node, "Control", |c| c.modulate.a)?),
        "control.translation" => vec2(read::<Control, _>(tree, node, "Control", |c| {
            c.translation
        })?),
        "control.min_size" => vec2(read::<Control, _>(tree, node, "Control", |c| c.min_size)?),
        "control.expand" => boolean(read::<Control, _>(tree, node, "Control", |c| c.expand)?),
        "control.mouse_filter" => text(read::<Control, _>(tree, node, "Control", |c| {
            match c.mouse_filter {
                MouseFilter::Stop => "stop",
                MouseFilter::Ignore => "ignore",
            }
        })?),

        "label.text" => Out::Str(read::<Label, _>(tree, node, "Label", |l| l.text.clone())?),
        "label.key" => {
            read::<Label, _>(tree, node, "Label", |_| ())?;
            Out::Str(text_key(tree, node))
        }
        "label.font_size" => float(read::<Label, _>(tree, node, "Label", |l| l.style.size)?),
        "label.color" => color(read::<Label, _>(tree, node, "Label", |l| l.style.color)?),
        "label.outline_width" => float(read::<Label, _>(tree, node, "Label", |l| {
            l.style.outline_width
        })?),
        "label.outline_color" => color(read::<Label, _>(tree, node, "Label", |l| {
            l.style.outline_color
        })?),
        "label.autowrap" => boolean(read::<Label, _>(tree, node, "Label", |l| l.autowrap)?),
        "label.h_align" => text(read::<Label, _>(tree, node, "Label", |l| {
            h_align_name(l.h_align)
        })?),
        "label.v_align" => text(read::<Label, _>(tree, node, "Label", |l| {
            v_align_name(l.v_align)
        })?),

        "button.text" => Out::Str(read::<Button, _>(tree, node, "Button", |b| b.text.clone())?),
        "button.key" => {
            read::<Button, _>(tree, node, "Button", |_| ())?;
            Out::Str(text_key(tree, node))
        }
        "button.disabled" => boolean(read::<Button, _>(tree, node, "Button", |b| b.disabled)?),
        "button.font_size" => float(read::<Button, _>(tree, node, "Button", |b| {
            b.text_style.size
        })?),

        "progress.value" => float(read::<ProgressBar, _>(tree, node, "ProgressBar", |p| {
            p.value
        })?),
        "progress.min" => float(read::<ProgressBar, _>(tree, node, "ProgressBar", |p| {
            p.min
        })?),
        "progress.max" => float(read::<ProgressBar, _>(tree, node, "ProgressBar", |p| {
            p.max
        })?),
        "progress.fill_color" => color(read::<ProgressBar, _>(tree, node, "ProgressBar", |p| {
            p.fill.color
        })?),
        "progress.background_color" => {
            color(read::<ProgressBar, _>(tree, node, "ProgressBar", |p| {
                p.background.color
            })?)
        }
        "progress.corner_radius" => {
            float(read::<ProgressBar, _>(tree, node, "ProgressBar", |p| {
                p.fill.corner_radius
            })?)
        }
        "progress.show_percentage" => {
            boolean(read::<ProgressBar, _>(tree, node, "ProgressBar", |p| {
                p.show_percentage
            })?)
        }

        "panel.color" => color(read::<Panel, _>(tree, node, "Panel", |p| p.style.color)?),
        "panel.corner_radius" => float(read::<Panel, _>(tree, node, "Panel", |p| {
            p.style.corner_radius
        })?),
        "panel.border_width" => float(read::<Panel, _>(tree, node, "Panel", |p| {
            p.style.border_width
        })?),
        "panel.border_color" => color(read::<Panel, _>(tree, node, "Panel", |p| {
            p.style.border_color
        })?),

        "box.separation" => float(read::<BoxContainer, _>(tree, node, "BoxContainer", |b| {
            b.separation
        })?),
        "box.alignment" => text(read::<BoxContainer, _>(tree, node, "BoxContainer", |b| {
            h_align_name(b.alignment)
        })?),

        "timer.wait_time" => float(read::<Timer, _>(tree, node, "Timer", |t| t.wait_time)?),
        "timer.one_shot" => boolean(read::<Timer, _>(tree, node, "Timer", |t| t.one_shot)?),
        "timer.time_left" => float(read::<Timer, _>(tree, node, "Timer", Timer::time_left)?),
        "timer.stopped" => boolean(read::<Timer, _>(tree, node, "Timer", Timer::is_stopped)?),

        "audio_player.volume_db" => {
            float(read::<AudioPlayer, _>(tree, node, "audio player", |p| {
                p.volume_db
            })?)
        }
        "audio_player.pitch_scale" => {
            float(read::<AudioPlayer, _>(tree, node, "audio player", |p| {
                p.pitch_scale
            })?)
        }
        "audio_player.looping" => {
            boolean(read::<AudioPlayer, _>(tree, node, "audio player", |p| {
                p.looping
            })?)
        }
        "audio_player.playing" => boolean(read::<AudioPlayer, _>(
            tree,
            node,
            "audio player",
            AudioPlayer::is_playing,
        )?),

        _ => return Err(unknown(path)),
    })
}

fn set(c: &mut Call<'_>, node: NodeHandle, path: &str, value: ffi::Value) -> CallResult {
    let wrong = |expected: &str| CallError::BadArguments(format!("`{path}` takes {expected}"));
    let f = || {
        value
            .as_float()
            .map(|v| v as f32)
            .ok_or_else(|| wrong("a number"))
    };
    let b = || value.as_bool().ok_or_else(|| wrong("a bool"));
    let u = || {
        value
            .as_int()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| wrong("a non-negative integer"))
    };
    let col = || from_color(&value).ok_or_else(|| wrong("a Color"));
    let v3 = || from_vec3(&value).ok_or_else(|| wrong("a Vec3"));
    let v2 = || from_vec2(&value).ok_or_else(|| wrong("a Vec2"));
    // SAFETY: the value is an argument of the current call.
    let s = || unsafe { value.as_str() }.ok_or_else(|| wrong("a string"));
    let h_align = || match s()? {
        "left" => Ok(HAlign::Left),
        "center" => Ok(HAlign::Center),
        "right" => Ok(HAlign::Right),
        _ => Err(wrong("\"left\", \"center\" or \"right\"")),
    };
    let tree = &*c.tree;
    match path {
        "light.energy" => light_set(c, node, LightField::Energy, &value),
        "light.color" => light_set(c, node, LightField::Color, &value),
        "light.range" => light_set(c, node, LightField::Range, &value),
        "light.shadows" => light_set(c, node, LightField::Shadows, &value),

        "camera.fov" => {
            let v = f()?.clamp(1.0, 179.0);
            write::<Camera3D>(tree, node, "Camera3D", |c| c.fov_y_degrees = v)
        }
        "camera.near" => {
            let v = f()?.max(1e-4);
            write::<Camera3D>(tree, node, "Camera3D", |c| c.near = v)
        }
        "camera.far" => {
            let v = f()?;
            write::<Camera3D>(tree, node, "Camera3D", |c| c.far = v)
        }
        "camera.current" => make_current(c, node, b()?),

        "mesh.color" => {
            let v = col()?;
            mesh_material_mut(c, node, |m| m.albedo = v)
        }
        "mesh.emission" => {
            let v = col()?;
            mesh_material_mut(c, node, |m| m.emission = v)
        }
        "mesh.emission_energy" => {
            let v = f()?.max(0.0);
            mesh_material_mut(c, node, |m| m.emission_energy = v)
        }
        "mesh.cast_shadows" => {
            let v = b()?;
            write::<MeshInstance3D>(tree, node, "MeshInstance3D", |m| m.cast_shadows = v)
        }

        "particles.emitting" => {
            let v = b()?;
            write::<ParticleEmitter>(tree, node, "Particles3D", |p| p.emitting = v)
        }
        "particles.amount" => {
            let v = u()?;
            write::<ParticleEmitter>(tree, node, "Particles3D", |p| p.settings.amount = v)
        }

        "body.kind" => {
            let kind = match s()? {
                "static" => BodyKind::Static,
                "rigid" => BodyKind::Rigid,
                "kinematic" => BodyKind::Kinematic,
                "character" => BodyKind::Character,
                "area" => BodyKind::Area,
                _ => return Err(wrong("static, rigid, kinematic, character or area")),
            };
            write::<PhysicsBody3D>(tree, node, "PhysicsBody3D", |b| b.kind = kind)
        }
        "body.mass" => {
            let v = f()?.max(1e-3);
            write::<PhysicsBody3D>(tree, node, "PhysicsBody3D", |b| b.mass = v)
        }
        "body.linear_velocity" => {
            let v = v3()?;
            write::<PhysicsBody3D>(tree, node, "PhysicsBody3D", |b| b.set_linear_velocity(v))
        }
        "body.angular_velocity" => {
            let v = v3()?;
            write::<PhysicsBody3D>(tree, node, "PhysicsBody3D", |b| b.set_angular_velocity(v))
        }
        "body.collision_layer" => {
            let v = u()?;
            write::<PhysicsBody3D>(tree, node, "PhysicsBody3D", |b| b.collision_layer = v)
        }
        "body.collision_mask" => {
            let v = u()?;
            write::<PhysicsBody3D>(tree, node, "PhysicsBody3D", |b| b.collision_mask = v)
        }
        "body.gravity_scale" => {
            let v = f()?;
            write::<PhysicsBody3D>(tree, node, "PhysicsBody3D", |b| b.gravity_scale = v)
        }
        "body.linear_damping" => {
            let v = f()?.max(0.0);
            write::<PhysicsBody3D>(tree, node, "PhysicsBody3D", |b| b.linear_damping = v)
        }
        "body.angular_damping" => {
            let v = f()?.max(0.0);
            write::<PhysicsBody3D>(tree, node, "PhysicsBody3D", |b| b.angular_damping = v)
        }
        "body.friction" => {
            let v = f()?.max(0.0);
            write::<PhysicsBody3D>(tree, node, "PhysicsBody3D", |b| b.friction = v)
        }
        "body.bounce" => {
            let v = f()?.max(0.0);
            write::<PhysicsBody3D>(tree, node, "PhysicsBody3D", |b| b.bounce = v)
        }

        "character.velocity" => {
            let v = v3()?;
            write::<CharacterBody3D>(tree, node, "CharacterBody3D", |b| b.velocity = v)
        }
        "character.up" => {
            let v = v3()?.normalize_or(ab_core::Vec3::Y);
            write::<CharacterBody3D>(tree, node, "CharacterBody3D", |b| b.up = v)
        }
        "character.floor_max_angle" => {
            let v = f()?;
            write::<CharacterBody3D>(tree, node, "CharacterBody3D", |b| b.floor_max_angle = v)
        }
        "character.floor_snap_length" => {
            let v = f()?.max(0.0);
            write::<CharacterBody3D>(tree, node, "CharacterBody3D", |b| b.floor_snap_length = v)
        }
        "character.max_step_height" => {
            let v = f()?.max(0.0);
            write::<CharacterBody3D>(tree, node, "CharacterBody3D", |b| b.max_step_height = v)
        }
        "character.push_rigid_bodies" => {
            let v = b()?;
            write::<CharacterBody3D>(tree, node, "CharacterBody3D", |b| b.push_rigid_bodies = v)
        }

        "control.modulate" => {
            let v = col()?;
            write::<Control>(tree, node, "Control", |c| c.modulate = v)
        }
        "control.alpha" => {
            let v = f()?.clamp(0.0, 1.0);
            write::<Control>(tree, node, "Control", |c| c.modulate.a = v)
        }
        "control.translation" => {
            let v = v2()?;
            write::<Control>(tree, node, "Control", |c| c.translation = v)
        }
        "control.min_size" => {
            let v = v2()?;
            write::<Control>(tree, node, "Control", |c| c.min_size = v)
        }
        "control.expand" => {
            let v = b()?;
            write::<Control>(tree, node, "Control", |c| c.expand = v)
        }
        "control.mouse_filter" => {
            let filter = match s()? {
                "stop" => MouseFilter::Stop,
                "ignore" => MouseFilter::Ignore,
                _ => return Err(wrong("\"stop\" or \"ignore\"")),
            };
            write::<Control>(tree, node, "Control", |c| c.mouse_filter = filter)
        }

        "label.text" => {
            let v = s()?;
            write::<Label>(tree, node, "Label", |l| {
                if l.text != v {
                    v.clone_into(&mut l.text);
                }
            })
        }
        "label.font_size" => {
            let v = f()?.max(1.0);
            write::<Label>(tree, node, "Label", |l| l.style.size = v)
        }
        "label.color" => {
            let v = col()?;
            write::<Label>(tree, node, "Label", |l| l.style.color = v)
        }
        "label.outline_width" => {
            let v = f()?.max(0.0);
            write::<Label>(tree, node, "Label", |l| l.style.outline_width = v)
        }
        "label.outline_color" => {
            let v = col()?;
            write::<Label>(tree, node, "Label", |l| l.style.outline_color = v)
        }
        "label.autowrap" => {
            let v = b()?;
            write::<Label>(tree, node, "Label", |l| l.autowrap = v)
        }
        "label.h_align" => {
            let v = h_align()?;
            write::<Label>(tree, node, "Label", |l| l.h_align = v)
        }
        "label.v_align" => {
            let v = match s()? {
                "top" => VAlign::Top,
                "center" => VAlign::Center,
                "bottom" => VAlign::Bottom,
                _ => return Err(wrong("\"top\", \"center\" or \"bottom\"")),
            };
            write::<Label>(tree, node, "Label", |l| l.v_align = v)
        }

        "button.text" => {
            let v = s()?;
            write::<Button>(tree, node, "Button", |b| v.clone_into(&mut b.text))
        }
        "button.key" => {
            let v = s()?;
            let fallback = read::<Button, _>(tree, node, "Button", |b| b.text.clone())?;
            crate::localization::set_text_key(c.tree, node, Some(v), &fallback);
            Ok(())
        }
        "label.key" => {
            let v = s()?;
            let fallback = read::<Label, _>(tree, node, "Label", |l| l.text.clone())?;
            crate::localization::set_text_key(c.tree, node, Some(v), &fallback);
            Ok(())
        }
        "button.disabled" => {
            let v = b()?;
            write::<Button>(tree, node, "Button", |b| b.disabled = v)
        }
        "button.font_size" => {
            let v = f()?.max(1.0);
            write::<Button>(tree, node, "Button", |b| b.text_style.size = v)
        }

        "progress.value" => {
            let v = f()?;
            write::<ProgressBar>(tree, node, "ProgressBar", |p| p.value = v)
        }
        "progress.min" => {
            let v = f()?;
            write::<ProgressBar>(tree, node, "ProgressBar", |p| p.min = v)
        }
        "progress.max" => {
            let v = f()?;
            write::<ProgressBar>(tree, node, "ProgressBar", |p| p.max = v)
        }
        "progress.fill_color" => {
            let v = col()?;
            write::<ProgressBar>(tree, node, "ProgressBar", |p| p.fill.color = v)
        }
        "progress.background_color" => {
            let v = col()?;
            write::<ProgressBar>(tree, node, "ProgressBar", |p| p.background.color = v)
        }
        "progress.corner_radius" => {
            let v = f()?.max(0.0);
            write::<ProgressBar>(tree, node, "ProgressBar", |p| {
                p.fill.corner_radius = v;
                p.background.corner_radius = v;
            })
        }
        "progress.show_percentage" => {
            let v = b()?;
            write::<ProgressBar>(tree, node, "ProgressBar", |p| p.show_percentage = v)
        }

        "panel.color" => {
            let v = col()?;
            write::<Panel>(tree, node, "Panel", |p| p.style.color = v)
        }
        "panel.corner_radius" => {
            let v = f()?.max(0.0);
            write::<Panel>(tree, node, "Panel", |p| p.style.corner_radius = v)
        }
        "panel.border_width" => {
            let v = f()?.max(0.0);
            write::<Panel>(tree, node, "Panel", |p| p.style.border_width = v)
        }
        "panel.border_color" => {
            let v = col()?;
            write::<Panel>(tree, node, "Panel", |p| p.style.border_color = v)
        }

        "box.separation" => {
            let v = f()?.max(0.0);
            write::<BoxContainer>(tree, node, "BoxContainer", |b| b.separation = v)
        }
        "box.alignment" => {
            let v = h_align()?;
            write::<BoxContainer>(tree, node, "BoxContainer", |b| b.alignment = v)
        }

        "timer.wait_time" => {
            let v = f()?.max(0.0);
            write::<Timer>(tree, node, "Timer", |t| t.wait_time = v)
        }
        "timer.one_shot" => {
            let v = b()?;
            write::<Timer>(tree, node, "Timer", |t| t.one_shot = v)
        }

        "audio_player.volume_db" => {
            let v = f()?;
            write::<AudioPlayer>(tree, node, "audio player", |p| p.volume_db = v)
        }
        "audio_player.pitch_scale" => {
            let v = f()?.max(0.01);
            write::<AudioPlayer>(tree, node, "audio player", |p| p.pitch_scale = v)
        }
        "audio_player.looping" => {
            let v = b()?;
            write::<AudioPlayer>(tree, node, "audio player", |p| p.looping = v)
        }
        "audio_player.playing" => {
            let v = b()?;
            write::<AudioPlayer>(tree, node, "audio player", |p| {
                if v {
                    p.play()
                } else {
                    p.stop()
                }
            })
        }

        "body.sleeping"
        | "character.on_floor"
        | "character.on_wall"
        | "character.on_ceiling"
        | "character.floor_normal"
        | "timer.time_left"
        | "timer.stopped" => Err(CallError::BadArguments(format!("`{path}` is read-only"))),
        _ => Err(unknown(path)),
    }
}

/// Makes the node the current camera (and no other).
fn make_current(c: &mut Call<'_>, node: NodeHandle, current: bool) -> CallResult {
    read::<Camera3D, _>(c.tree, node, "Camera3D", |_| ())?;
    if current {
        if let Some(mut cameras) = c.tree.world().storage_mut::<Camera3D>() {
            for (_, camera) in cameras.iter_mut() {
                camera.current = false;
            }
        }
    }
    write::<Camera3D>(c.tree, node, "Camera3D", |camera| camera.current = current)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every listed property can be read (on the right node), and nothing else.
    #[test]
    fn listed_properties_are_handled() {
        let tree = SceneTree::new();
        let root = tree.root();
        for &path in PROPERTIES {
            if let Err(CallError::BadArguments(message)) = get(&tree, root, path) {
                panic!("`{path}` is listed but not readable: {message}");
            }
        }
        assert!(matches!(
            get(&tree, root, "nope.nothing"),
            Err(CallError::BadArguments(_))
        ));
    }
}
