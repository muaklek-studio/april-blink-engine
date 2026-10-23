//! Building nodes from a [`SceneFile`].

use super::format::*;
use super::{SceneInstance, SceneOrigin};
use ab_animation::AnimationPlayer;
use ab_asset::{spawn_gltf, AssetError, AssetServer};
use ab_audio::{Attenuation, AudioNodes, AudioPlayer};
use ab_core::{EulerRot, Quat, Transform3D, Vec3};
use ab_effects::{ColorRamp, EffectNodes, ParticleEmitter, ParticleSettings};
use ab_physics::{
    BodyKind, CharacterBody3D, Joint3D, JointKind, PhysicsBody3D, PhysicsNodes, Ragdoll, Shape,
};
use ab_rendering::{
    Camera3D, ColorSpace, DirectionalLight3D, Environment, Material, MaterialHandle, Mesh,
    MeshHandle, MeshInstance3D, OmniLight3D, RenderAssets, RenderNodes, SpotLight3D,
};
use ab_scene::{NodeHandle, SceneError, SceneTree, Timer};
use std::collections::HashMap;

/// Most segments, rings or subdivisions of a procedural mesh in a scene file.
const MAX_MESH_DETAIL: u32 = 512;
/// Instances nested deeper than this are refused (a scene instancing itself).
const MAX_DEPTH: u32 = 16;

#[derive(Debug, thiserror::Error)]
pub enum SceneFileError {
    #[error("cannot read scene `{path}`: {source}")]
    Asset {
        path: String,
        #[source]
        source: AssetError,
    },
    #[error("invalid scene `{path}`: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("scene `{path}`, node `{node}`: {reason}")]
    Node {
        path: String,
        node: String,
        reason: String,
    },
}

/// ECS resource: meshes and materials created for scene files, shared by every node that
/// describes the same one (loading a level twice does not duplicate GPU data).
#[derive(Debug, Default)]
pub struct SceneAssetCache {
    meshes: HashMap<String, MeshHandle>,
    materials: HashMap<String, MaterialHandle>,
}

/// Loads the scene file at `path` (through the [`AssetServer`]) and builds it as a detached
/// subtree; add the returned root to the tree. Applies its `[environment]`.
pub fn load_scene(tree: &mut SceneTree, path: &str) -> Result<NodeHandle, SceneFileError> {
    load_nested(tree, path, 0, true)
}

fn load_nested(
    tree: &mut SceneTree,
    path: &str,
    depth: u32,
    with_environment: bool,
) -> Result<NodeHandle, SceneFileError> {
    let bytes = tree
        .world()
        .resource::<AssetServer>()
        .ok_or_else(|| SceneFileError::Asset {
            path: path.to_owned(),
            source: AssetError::MissingResource("AssetServer"),
        })?
        .read(path)
        .map_err(|source| SceneFileError::Asset {
            path: path.to_owned(),
            source,
        })?;
    let text = String::from_utf8_lossy(&bytes);
    let scene = SceneFile::from_toml(&text).map_err(|source| SceneFileError::Parse {
        path: path.to_owned(),
        source,
    })?;
    if with_environment {
        if let Some(environment) = &scene.environment {
            apply_environment(tree, environment).map_err(|reason| SceneFileError::Node {
                path: path.to_owned(),
                node: "[environment]".into(),
                reason,
            })?;
        }
    }
    instantiate_at_depth(tree, &scene, path, depth)
}

/// A prefab scene with an instance's overrides on top (see [`super::prefab`]).
fn load_prefab(
    tree: &mut SceneTree,
    path: &str,
    depth: u32,
    overrides: &[toml::Table],
) -> Result<NodeHandle, SceneFileError> {
    let bytes = tree
        .world()
        .resource::<AssetServer>()
        .ok_or_else(|| SceneFileError::Asset {
            path: path.to_owned(),
            source: AssetError::MissingResource("AssetServer"),
        })?
        .read(path)
        .map_err(|source| SceneFileError::Asset {
            path: path.to_owned(),
            source,
        })?;
    let mut scene = SceneFile::from_toml(&String::from_utf8_lossy(&bytes)).map_err(|source| {
        SceneFileError::Parse {
            path: path.to_owned(),
            source,
        }
    })?;
    let unmatched = super::prefab::apply_overrides(&mut scene, overrides);
    for (_, reason) in &unmatched {
        tracing::warn!("instance of {path}: override not applied: {reason}");
    }
    let node = instantiate_at_depth(tree, &scene, path, depth)?;
    if !unmatched.is_empty() {
        let kept = unmatched.into_iter().map(|(o, _)| o).collect();
        tree.insert_component(node, super::prefab::UnmatchedOverrides(kept))
            .map_err(|e| SceneFileError::Node {
                path: path.to_owned(),
                node: String::new(),
                reason: e.to_string(),
            })?;
    }
    Ok(node)
}

/// Builds `scene` as a detached subtree (without applying its environment). `origin` names
/// the scene in errors.
pub fn instantiate_scene(
    tree: &mut SceneTree,
    scene: &SceneFile,
    origin: &str,
) -> Result<NodeHandle, SceneFileError> {
    instantiate_at_depth(tree, scene, origin, 0)
}

fn instantiate_at_depth(
    tree: &mut SceneTree,
    scene: &SceneFile,
    origin: &str,
    depth: u32,
) -> Result<NodeHandle, SceneFileError> {
    let error = |node: &str, reason: String| SceneFileError::Node {
        path: origin.to_owned(),
        node: node.to_owned(),
        reason,
    };
    if scene.nodes.is_empty() {
        return Err(error("", "the scene has no nodes".into()));
    }
    let mut created: Vec<NodeHandle> = Vec::new();
    let mut paths: HashMap<String, NodeHandle> = HashMap::new();
    let mut root = None;
    let result = (|| {
        for (index, desc) in scene.nodes.iter().enumerate() {
            let name = if desc.name.trim().is_empty() {
                return Err(error("", format!("node #{} has no name", index + 1)));
            } else {
                desc.name.as_str()
            };
            let node = create(tree, desc, depth).map_err(|reason| error(name, reason))?;
            created.push(node);
            configure(tree, node, desc).map_err(|reason| error(name, reason))?;
            match (index, desc.parent.as_deref()) {
                (0, None) => {
                    root = Some(node);
                    paths.insert(".".into(), node);
                }
                (0, Some(_)) => {
                    return Err(error(
                        name,
                        "the first node is the root and has no parent".into(),
                    ))
                }
                (_, None) => {
                    return Err(error(name, "missing `parent` (\".\" for the root)".into()))
                }
                (_, Some(parent_path)) => {
                    let parent = *paths.get(parent_path).ok_or_else(|| {
                        error(
                            name,
                            format!("parent `{parent_path}` not found (parents must come first)"),
                        )
                    })?;
                    tree.add_child(parent, node)
                        .map_err(|e| error(name, e.to_string()))?;
                    // Duplicate sibling names get a suffix; register the real one.
                    let actual = tree.name(node).map(|n| n.to_owned()).unwrap_or_default();
                    let path = if parent_path == "." {
                        actual
                    } else {
                        format!("{parent_path}/{actual}")
                    };
                    paths.insert(path, node);
                }
            }
        }
        // Joints name their bodies by path: resolve once the hierarchy exists.
        for (desc, &node) in scene.nodes.iter().zip(&created) {
            let Some(joint) = &desc.joint else {
                continue;
            };
            // An empty path leaves that side unconnected (set it later).
            let find = |path: &str| -> Result<Option<NodeHandle>, SceneFileError> {
                if path.trim().is_empty() {
                    return Ok(None);
                }
                tree.get_node(node, path)
                    .map(Some)
                    .ok_or_else(|| error(&desc.name, format!("joint body `{path}` not found")))
            };
            let (a, b) = (find(&joint.body_a)?, find(&joint.body_b)?);
            if let Some(mut component) = tree.component_mut::<Joint3D>(node) {
                component.body_a = a;
                component.body_b = b;
            }
        }
        let root = root.expect("the first node is the root");
        for (desc, &node) in scene.nodes.iter().zip(&created) {
            if let Some(animation) = &desc.animation {
                load_animations(tree, node, animation)
                    .map_err(|reason| error(&desc.name, reason))?;
            }
        }
        // Humanoid ragdolls need their characters' skeletons: built last.
        for (node, err) in crate::ragdoll::build_rigs(tree, root) {
            let name = tree.name(node).map(|n| n.to_owned()).unwrap_or_default();
            tracing::warn!("{origin}: ragdoll `{name}`: {err}");
        }
        // Generators, once the whole scene is there (a scene instanced inside another is
        // built with it).
        if depth == 0 {
            super::generator::build_all(tree, root);
        }
        Ok(root)
    })();
    if result.is_err() {
        // Free what was built: the first node holds everything attached so far.
        if let Some(&first) = created.first() {
            let _ = tree.queue_free(first);
            for node in created {
                if tree.parent(node).is_none() && tree.is_valid(node) {
                    let _ = tree.queue_free(node);
                }
            }
            tree.flush_deletions();
        }
    }
    result
}

/// Loads an `AnimationPlayer`'s clip files and points it at the nodes they animate (paths
/// from the player's parent).
pub fn load_animations(
    tree: &mut SceneTree,
    player: NodeHandle,
    desc: &AnimationDesc,
) -> Result<(), String> {
    use crate::animation_file::ClipFile;
    let mut files = Vec::new();
    for path in &desc.clips {
        let bytes = tree
            .world()
            .resource::<AssetServer>()
            .ok_or("the AssetServer resource is missing")?
            .read(path)
            .map_err(|e| format!("animation `{path}`: {e}"))?;
        let text = String::from_utf8_lossy(&bytes);
        files.push(ClipFile::from_toml(&text).map_err(|e| format!("`{path}`: {e}"))?);
    }
    install_clips(tree, player, &files, desc)
}

/// Gives the `AnimationPlayer` node `player` these clips (as [`load_animations`] does from
/// files): editors applying clips being edited.
pub fn install_clips(
    tree: &mut SceneTree,
    player: NodeHandle,
    files: &[crate::animation_file::ClipFile],
    desc: &AnimationDesc,
) -> Result<(), String> {
    let mut targets: Vec<String> = Vec::new();
    let mut clips = Vec::new();
    for file in files {
        let (mut clip, local) = file.build();
        // One target list for all the player's clips.
        for channel in &mut clip.channels {
            let path = &local[channel.target];
            channel.target = match targets.iter().position(|t| t == path) {
                Some(i) => i,
                None => {
                    targets.push(path.clone());
                    targets.len() - 1
                }
            };
        }
        clips.push(clip);
    }
    // Not placed yet (an editor rebuilding the node): set up once it is.
    let Some(base) = tree.parent(player) else {
        return Ok(());
    };
    let handles: Vec<NodeHandle> = targets
        .iter()
        .map(|path| {
            let node = if path == "." {
                Some(base)
            } else {
                tree.get_node(base, path)
            };
            node.unwrap_or_else(|| {
                tracing::warn!("animation target `{path}` not found");
                // Writes to a node without a transform are ignored.
                player
            })
        })
        .collect();
    let library = tree
        .world_mut()
        .resource_or_insert_with(ab_animation::AnimationLibrary::new);
    let named: Vec<(String, ab_animation::ClipHandle)> = clips
        .into_iter()
        .map(|clip| (clip.name.clone(), library.add(clip)))
        .collect();
    let mut player_component = tree
        .component_mut::<AnimationPlayer>(player)
        .ok_or("not an AnimationPlayer")?;
    player_component.targets = handles;
    player_component.clips = named;
    player_component.looping = desc.looping.unwrap_or(true);
    player_component.speed = desc.speed.unwrap_or(1.0);
    if let Some(name) = &desc.autoplay {
        if !player_component.play_named(name) {
            return Err(format!("no clip named `{name}` to autoplay"));
        }
    }
    Ok(())
}

/// Creates the node for `desc` (detached).
fn create(tree: &mut SceneTree, desc: &NodeDesc, depth: u32) -> Result<NodeHandle, String> {
    let name = desc.name.as_str();
    if let Some(path) = &desc.instance {
        if desc.kind.is_some() {
            return Err("use either `type` or `instance`, not both".into());
        }
        if depth >= MAX_DEPTH {
            return Err(format!(
                "instances nested deeper than {MAX_DEPTH} (a cycle?)"
            ));
        }
        let loaded = if super::is_scene_path(path) {
            load_prefab(tree, path, depth + 1, &desc.overrides)
        } else {
            spawn_gltf(tree, path).map_err(|source| SceneFileError::Asset {
                path: path.clone(),
                source,
            })
        };
        let node = match loaded {
            Ok(node) => node,
            // The file is missing or unreadable: a placeholder keeps the instance (and
            // what it overrides) in the scene, to be saved back as it was.
            Err(err @ (SceneFileError::Asset { .. } | SceneFileError::Parse { .. })) => {
                tracing::warn!("{name}: instance {path} not loaded: {err}");
                let node = tree.create_node3d(name);
                tree.insert_component(
                    node,
                    super::prefab::MissingInstance {
                        error: err.to_string(),
                        overrides: desc.overrides.clone(),
                    },
                )
                .map_err(|e| e.to_string())?;
                node
            }
            Err(err) => return Err(err.to_string()),
        };
        tree.set_name(node, name).map_err(|e| e.to_string())?;
        tree.insert_component(node, SceneInstance(path.clone()))
            .map_err(|e| e.to_string())?;
        return Ok(node);
    }
    let kind = desc.kind.as_deref().unwrap_or("Node3D");
    if let Some(node) = super::ui::create(tree, kind, desc) {
        return node;
    }
    let node = match kind {
        "Node" => tree.create_node(name),
        "Node3D" => tree.create_node3d(name),
        "MeshInstance3D" => {
            let mesh_desc = desc.mesh.clone().ok_or("MeshInstance3D needs `mesh`")?;
            let material_desc = desc.material.clone().unwrap_or_default();
            let (mesh, material) = mesh_and_material(tree, &mesh_desc, &material_desc)?;
            let instance = MeshInstance3D {
                cast_shadows: desc.cast_shadows.unwrap_or(true),
                ..MeshInstance3D::new(mesh, material)
            };
            let node = tree.create_mesh_instance3d(name, instance);
            tree.insert_component(
                node,
                SceneOrigin {
                    mesh: Some(mesh_desc),
                    material: Some(material_desc),
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())?;
            node
        }
        "Camera3D" => {
            let c = desc.camera.clone().unwrap_or_default();
            let default = Camera3D::default();
            tree.create_camera3d(
                name,
                Camera3D {
                    fov_y_degrees: c.fov.unwrap_or(default.fov_y_degrees),
                    near: c.near.unwrap_or(default.near),
                    far: c.far.unwrap_or(default.far),
                    current: c.current.unwrap_or(false),
                },
            )
        }
        "DirectionalLight3D" => {
            let l = desc.light.clone().unwrap_or_default();
            tree.create_directional_light3d(
                name,
                DirectionalLight3D {
                    color: color(&l.color)?,
                    energy: l.energy.unwrap_or(1.0),
                    shadow_enabled: l.shadows.unwrap_or(false),
                },
            )
        }
        "OmniLight3D" => {
            let l = desc.light.clone().unwrap_or_default();
            let default = OmniLight3D::default();
            let energy = l.energy.unwrap_or(default.energy);
            let node = tree.create_omni_light3d(
                name,
                OmniLight3D {
                    color: color(&l.color)?,
                    energy,
                    range: l.range.unwrap_or(default.range),
                },
            );
            if let Some(seconds) = l.flash.filter(|s| *s > 0.0) {
                tree.insert_component(node, ab_effects::LightFlash::new(energy, seconds))
                    .map_err(|e| e.to_string())?;
            }
            node
        }
        "SpotLight3D" => {
            let l = desc.light.clone().unwrap_or_default();
            let default = SpotLight3D::default();
            tree.create_spot_light3d(
                name,
                SpotLight3D {
                    color: color(&l.color)?,
                    energy: l.energy.unwrap_or(default.energy),
                    range: l.range.unwrap_or(default.range),
                    angle_degrees: l.angle.unwrap_or(default.angle_degrees),
                    edge_softness: l.softness.unwrap_or(default.edge_softness),
                    shadow_enabled: l.shadows.unwrap_or(false),
                },
            )
        }
        "StaticBody3D" | "RigidBody3D" | "AnimatableBody3D" | "Area3D" => {
            let kind = match kind {
                "StaticBody3D" => BodyKind::Static,
                "RigidBody3D" => BodyKind::Rigid,
                "AnimatableBody3D" => BodyKind::Kinematic,
                _ => BodyKind::Area,
            };
            let body = body(PhysicsBody3D::new(kind), desc.body.as_ref());
            tree.create_physics_body3d(name, body)
        }
        "CharacterBody3D" => {
            let c = desc.character.clone().unwrap_or_default();
            let mut character = CharacterBody3D::default();
            if let Some(angle) = c.floor_max_angle {
                character.floor_max_angle = angle.to_radians();
            }
            character.floor_snap_length =
                c.floor_snap_length.unwrap_or(character.floor_snap_length);
            character.max_step_height = c.max_step_height.unwrap_or(character.max_step_height);
            character.push_rigid_bodies =
                c.push_rigid_bodies.unwrap_or(character.push_rigid_bodies);
            let node = tree.create_character_body3d(name, character);
            if let Some(desc) = &desc.body {
                if let Some(mut b) = tree.component_mut::<PhysicsBody3D>(node) {
                    *b = body(b.clone(), Some(desc));
                }
            }
            node
        }
        "CollisionShape3D" => {
            let shape = match desc.shape.clone().ok_or("CollisionShape3D needs `shape`")? {
                ShapeDesc::Box(size) => Shape::cuboid(Vec3::from(size)),
                ShapeDesc::Sphere(radius) => Shape::sphere(radius),
                ShapeDesc::Capsule([radius, height]) => Shape::capsule(radius, height),
                ShapeDesc::Cylinder([radius, height]) => Shape::cylinder(radius, height),
            };
            tree.create_collision_shape3d(name, shape)
        }
        "AudioStreamPlayer" | "AudioStreamPlayer3D" => {
            let a = desc
                .audio
                .clone()
                .ok_or("audio players need `audio = { stream = ... }`")?;
            let stream = tree
                .world()
                .resource_mut::<AssetServer>()
                .ok_or("the AssetServer resource is missing")?
                .load_audio(&a.stream)
                .map_err(|e| e.to_string())?;
            let mut player = AudioPlayer::new(stream);
            player.volume_db = a.volume_db.unwrap_or(0.0);
            player.pitch_scale = a.pitch.unwrap_or(1.0);
            player.looping = a.looping.unwrap_or(false);
            player.autoplay = a.autoplay.unwrap_or(false);
            let node = if kind == "AudioStreamPlayer3D" {
                let default = Attenuation::default();
                player.positional = Some(Attenuation {
                    min_distance: a.min_distance.unwrap_or(default.min_distance),
                    max_distance: a.max_distance.unwrap_or(default.max_distance),
                });
                tree.create_audio_stream_player3d(name, player)
            } else {
                tree.create_audio_stream_player(name, player)
            };
            tree.insert_component(
                node,
                SceneOrigin {
                    audio_stream: Some(a.stream),
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())?;
            node
        }
        "AudioListener3D" => tree.create_audio_listener3d(name),
        "Ragdoll" => {
            let r = desc.ragdoll.clone().unwrap_or_default();
            let node = tree.create_ragdoll(
                name,
                Ragdoll {
                    active: r.active.unwrap_or(false),
                },
            );
            if let Some(skeleton) = r.skeleton.filter(|s| !s.trim().is_empty()) {
                // Bodies are built once the whole scene exists (`build_rigs`).
                tree.insert_component(
                    node,
                    crate::ragdoll::RagdollRig {
                        skeleton,
                        bones: r.bones,
                        mass: r.mass,
                        thickness: r.thickness,
                    },
                )
                .map_err(|e| e.to_string())?;
            }
            node
        }
        "Joint3D" => {
            let j = desc
                .joint
                .as_ref()
                .ok_or("Joint3D needs `joint = { body_a = ..., body_b = ... }`")?;
            let kind = match j.kind.as_deref().unwrap_or("pin") {
                "pin" => JointKind::Pin,
                "fixed" => JointKind::Fixed,
                "hinge" => JointKind::Hinge {
                    limits: j.limits.map(|l| l.map(f32::to_radians)),
                },
                "cone" => JointKind::ConeTwist {
                    swing: j.swing.unwrap_or(45.0).to_radians(),
                    twist: j.twist.unwrap_or(30.0).to_radians(),
                },
                other => {
                    return Err(format!(
                        "unknown joint kind `{other}` (pin, hinge, cone or fixed)"
                    ))
                }
            };
            let mut joint = Joint3D::unconnected(kind);
            joint.collide_connected = j.collide.unwrap_or(false);
            tree.create_joint3d(name, joint)
        }
        "CameraShake" => tree.create_camera_shake(name),
        "AnimationPlayer" => {
            // Its clips address nodes by path: set up once the hierarchy exists.
            let node = tree.spawn_node(
                name,
                ab_asset::ANIMATION_PLAYER_CLASS,
                (AnimationPlayer::new(Vec::new()),),
            );
            tree.insert_component(
                node,
                SceneOrigin {
                    animation: Some(desc.animation.clone().unwrap_or_default()),
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())?;
            node
        }
        "Particles3D" => {
            let p = desc.particles.clone().unwrap_or_default();
            let emitter = particle_emitter(&p)?;
            let node = tree.create_particles3d(name, emitter);
            tree.insert_component(
                node,
                SceneOrigin {
                    particles: Some(p),
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())?;
            node
        }
        super::path::PATH_3D => super::path::create(tree, name, desc)?,
        super::generator::GENERATOR => super::generator::create(tree, name, desc)?,
        "Timer" => {
            let t = desc
                .timer
                .clone()
                .ok_or("Timer needs `timer = { wait_time = ... }`")?;
            let mut timer = Timer::new(t.wait_time);
            timer.one_shot = t.one_shot.unwrap_or(false);
            timer.autostart = t.autostart.unwrap_or(false);
            tree.create_timer_node(name, timer)
        }
        other => {
            return Err(format!(
                "unknown type `{other}` (supported: Node, Node3D, MeshInstance3D, Camera3D, \
                 DirectionalLight3D, OmniLight3D, SpotLight3D, StaticBody3D, RigidBody3D, \
                 AnimatableBody3D, CharacterBody3D, Area3D, CollisionShape3D, Joint3D, Ragdoll, \
                 AudioStreamPlayer, AudioStreamPlayer3D, AudioListener3D, Timer, \
                 Particles3D, CameraShake, AnimationPlayer, CanvasLayer, Control, Panel, Label, \
                 Button, ProgressBar, VBoxContainer, HBoxContainer, TextureRect)"
            ))
        }
    };
    Ok(node)
}

/// Transform, visibility, groups and script.
fn configure(tree: &mut SceneTree, node: NodeHandle, desc: &NodeDesc) -> Result<(), String> {
    let has_transform = desc.position.is_some() || desc.rotation.is_some() || desc.scale.is_some();
    if has_transform {
        if !tree.is_node3d(node) {
            return Err("position/rotation/scale need a 3D node type".into());
        }
        let current = tree.transform(node).unwrap_or(Transform3D::IDENTITY);
        let rotation = desc.rotation.map_or(current.rotation, |[x, y, z]| {
            Quat::from_euler(
                EulerRot::YXZ,
                y.to_radians(),
                x.to_radians(),
                z.to_radians(),
            )
        });
        tree.set_transform(
            node,
            Transform3D::from_trs(
                desc.position.map_or(current.translation, Vec3::from),
                rotation,
                desc.scale.map_or(current.scale, Vec3::from),
            ),
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(visible) = desc.visible {
        tree.set_visible(node, visible).map_err(|e| e.to_string())?;
    }
    for group in &desc.groups {
        tree.add_to_group(node, group).map_err(|e| e.to_string())?;
    }
    for script in desc.script_list() {
        tree.add_script_named(node, &script).map_err(|e| match e {
            SceneError::UnknownScript(name) => {
                format!("no script is registered as `{name}` (register it before loading)")
            }
            other => other.to_string(),
        })?;
    }
    Ok(())
}

fn body(mut body: PhysicsBody3D, desc: Option<&BodyDesc>) -> PhysicsBody3D {
    let Some(d) = desc else {
        return body;
    };
    body.mass = d.mass.unwrap_or(body.mass);
    body.gravity_scale = d.gravity_scale.unwrap_or(body.gravity_scale);
    body.friction = d.friction.unwrap_or(body.friction);
    body.bounce = d.bounce.unwrap_or(body.bounce);
    body.linear_damping = d.linear_damping.unwrap_or(body.linear_damping);
    body.angular_damping = d.angular_damping.unwrap_or(body.angular_damping);
    body.lock_rotation = d.lock_rotation.unwrap_or(body.lock_rotation);
    body.continuous_cd = d.continuous_cd.unwrap_or(body.continuous_cd);
    body.can_sleep = d.can_sleep.unwrap_or(body.can_sleep);
    body.collision_layer = d.layer.unwrap_or(body.collision_layer);
    body.collision_mask = d.mask.unwrap_or(body.collision_mask);
    body
}

fn color(desc: &Option<ColorDesc>) -> Result<ab_core::Color, String> {
    desc.as_ref()
        .map_or(Ok(ab_core::Color::WHITE), ColorDesc::to_color)
}

/// The emitter a `particles` table describes (editors changing an effect live).
pub fn particle_emitter(p: &ParticlesDesc) -> Result<ParticleEmitter, String> {
    let mut settings = match &p.preset {
        Some(preset) => ParticleSettings::preset(preset).ok_or_else(|| {
            format!(
                "unknown particle preset `{preset}` (one of: {})",
                ParticleSettings::PRESETS.join(", ")
            )
        })?,
        None => ParticleSettings::default(),
    };
    if let Some(amount) = p.amount {
        settings.amount = amount;
    }
    if let Some(lifetime) = p.lifetime {
        settings.lifetime = lifetime;
    }
    if let Some(v) = p.lifetime_random {
        settings.lifetime_random = v.clamp(0.0, 1.0);
    }
    if let Some(shape) = p.shape {
        settings.shape = shape;
    }
    if let Some(v) = p.direction {
        settings.direction = v.into();
    }
    if let Some(v) = p.spread {
        settings.spread = v.clamp(0.0, 180.0);
    }
    if let Some(v) = p.gravity {
        settings.gravity = v.into();
    }
    if let Some(v) = p.damping {
        settings.damping = v.max(0.0);
    }
    if let Some(v) = p.turbulence {
        settings.turbulence = v.max(0.0);
    }
    if let Some(v) = p.size_random {
        settings.size_random = v.clamp(0.0, 1.0);
    }
    if let Some(v) = p.spin {
        settings.spin = v;
    }
    if let Some(v) = p.hardness {
        settings.hardness = v.clamp(0.0, 1.0);
    }
    if let Some(v) = p.stretch {
        settings.stretch = v.max(0.0);
    }
    if let Some(size) = p.size {
        settings.size = size;
    }
    if let Some(speed) = p.speed {
        settings.speed = speed;
    }
    if p.color_start.is_some() || p.color_end.is_some() {
        let start = color(&p.color_start)?;
        let end = match &p.color_end {
            Some(c) => c.to_color()?,
            None => ab_core::Color::rgba(start.r, start.g, start.b, 0.0),
        };
        settings.color = ColorRamp::two(start, end);
    }
    if !p.colors.is_empty() {
        let mut stops = p
            .colors
            .iter()
            .map(|(t, c)| Ok((t.clamp(0.0, 1.0), c.to_color()?)))
            .collect::<Result<Vec<_>, String>>()?;
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        settings.color = ColorRamp(stops);
    }
    if let Some(emission) = p.emission {
        settings.emission = emission;
    }
    if let Some(additive) = p.additive {
        settings.additive = additive;
    }
    let mut emitter = ParticleEmitter::new(settings);
    emitter.one_shot = p.one_shot.unwrap_or(false);
    emitter.local_coords = p.local.unwrap_or(false);
    emitter.emitting = p.emitting.unwrap_or(true);
    Ok(emitter)
}

/// Mesh and material handles for descriptions, shared through the [`SceneAssetCache`]
/// (editors changing a node's look).
pub fn mesh_and_material(
    tree: &mut SceneTree,
    mesh: &MeshDesc,
    material: &MaterialDesc,
) -> Result<(MeshHandle, MaterialHandle), String> {
    let world = tree.world_mut();
    world.resource_or_insert_with(SceneAssetCache::default);
    let mesh = {
    let world = tree.world();
    let mut cache = world
        .resource_mut::<SceneAssetCache>()
        .expect("inserted above");
    let mut assets = world
        .resource_mut::<RenderAssets>()
        .ok_or("the RenderAssets resource is missing")?;
    let mesh_key = format!("{mesh:?}");
    let mesh = match cache.meshes.get(&mesh_key) {
        Some(&handle) => handle,
        None => {
            let built = match *mesh {
                MeshDesc::Cube { size } => Mesh::cube(size),
                MeshDesc::Sphere {
                    radius,
                    segments,
                    rings,
                    // Capped: a typo like `subdivisions = 100000` would otherwise build billions
                    // of vertices.
                } => Mesh::sphere(
                    radius,
                    segments.clamp(3, MAX_MESH_DETAIL),
                    rings.clamp(2, MAX_MESH_DETAIL),
                ),
                MeshDesc::Plane { size, subdivisions } => {
                    Mesh::plane(size, subdivisions.clamp(1, MAX_MESH_DETAIL))
                }
            };
            let handle = assets.add_mesh(built).map_err(|e| e.to_string())?;
            cache.meshes.insert(mesh_key, handle);
            handle
        }
    };
    mesh
    };
    let material = material_handle(tree, material)?;
    Ok((mesh, material))
}

/// The material `material` describes, made once and shared by every node that describes the
/// same one (its `file` and textures read in).
pub fn material_handle(tree: &mut SceneTree, material: &MaterialDesc) -> Result<MaterialHandle, String> {
    let world = tree.world_mut();
    world.resource_or_insert_with(SceneAssetCache::default);
    let world = tree.world();
    let mut cache = world
        .resource_mut::<SceneAssetCache>()
        .expect("inserted above");
    let mut assets = world
        .resource_mut::<RenderAssets>()
        .ok_or("the RenderAssets resource is missing")?;
    let mut server = world
        .resource_mut::<AssetServer>()
        .ok_or("the AssetServer resource is missing")?;
    let resolved = resolve_material(&server, material)?;
    let material_key = format!("{resolved:?}");
    let material = match cache.materials.get(&material_key) {
        Some(&handle) => handle,
        None => {
            let m = &resolved;
            let default = Material::default();
            let mut texture = |path: &Option<String>, space| -> Result<Option<_>, String> {
                path.as_deref()
                    .filter(|p| !p.is_empty())
                    .map(|p| {
                        server
                            .load_texture(&mut assets, p, space)
                            .map_err(|e| e.to_string())
                    })
                    .transpose()
            };
            let albedo_texture = texture(&m.albedo_texture, ColorSpace::Srgb)?;
            let normal_texture = texture(&m.normal_texture, ColorSpace::Linear)?;
            let metallic_roughness_texture =
                texture(&m.metallic_roughness_texture, ColorSpace::Linear)?;
            let emission_texture = texture(&m.emission_texture, ColorSpace::Srgb)?;
            let built = Material {
                albedo_texture,
                normal_texture,
                normal_scale: m.normal_scale.unwrap_or(default.normal_scale),
                metallic_roughness_texture,
                emission_texture,
                alpha_cutoff: m.alpha_cutoff.or(default.alpha_cutoff),
                albedo: color(&m.color)?,
                roughness: m.roughness.unwrap_or(default.roughness),
                metallic: m.metallic.unwrap_or(default.metallic),
                emission: m
                    .emission
                    .as_ref()
                    .map_or(Ok(default.emission), ColorDesc::to_color)?,
                emission_energy: m.emission_energy.unwrap_or(
                    if m.emission.is_some() || m.emission_texture.is_some() {
                        1.0
                    } else {
                        0.0
                    },
                ),
                unshaded: m.unshaded.unwrap_or(false),
                double_sided: m.double_sided.unwrap_or(false),
            };
            let handle = assets.add_material(built);
            cache.materials.insert(material_key, handle);
            handle
        }
    };
    Ok(material)
}

/// `material` with its `file` (an `.abmat`) read in under its own keys. A file may not name
/// another file.
pub fn resolve_material(
    assets: &AssetServer,
    material: &MaterialDesc,
) -> Result<MaterialDesc, String> {
    let Some(path) = material.file.as_deref().filter(|p| !p.is_empty()) else {
        return Ok(material.clone());
    };
    let bytes = assets
        .read(path)
        .map_err(|e| format!("material {path}: {e}"))?;
    let base = MaterialDesc::from_toml(&String::from_utf8_lossy(&bytes))
        .map_err(|e| format!("material {path}: {e}"))?;
    if base.file.is_some() {
        return Err(format!(
            "material {path}: a material file cannot name another (`file`)"
        ));
    }
    Ok(material.over(&base))
}

/// Applies an `[environment]` table to the `Environment` resource.
pub fn apply_environment(tree: &mut SceneTree, desc: &EnvironmentDesc) -> Result<(), String> {
    let world = tree.world_mut();
    let environment = world.resource_or_insert_with(Environment::default);
    if let Some(c) = &desc.background {
        environment.background = c.to_color()?;
    }
    if let Some(c) = &desc.ambient_color {
        environment.ambient_color = c.to_color()?;
    }
    if let Some(e) = desc.ambient_energy {
        environment.ambient_energy = e;
    }
    if let Some(c) = &desc.fog_color {
        environment.fog.color = c.to_color()?;
    }
    if let Some(d) = desc.fog_density {
        environment.fog.density = d;
        environment.fog.enabled = d > 0.0;
    }
    if let Some(h) = desc.fog_height {
        environment.fog.height = h;
    }
    if let Some(f) = desc.fog_height_falloff {
        environment.fog.height_falloff = f;
    }
    let finite = |key: &str, v: f32| {
        if v.is_finite() {
            Ok(v.max(0.0))
        } else {
            Err(format!("`{key}` must be a finite number"))
        }
    };
    let fog = &mut environment.fog;
    if let Some(v) = desc.fog_max_opacity {
        fog.max_opacity = finite("fog_max_opacity", v)?.min(1.0);
    }
    if let Some(v) = desc.fog_sun_scattering {
        fog.sun_scattering = finite("fog_sun_scattering", v)?;
    }
    if let Some(v) = desc.fog_sun_anisotropy {
        fog.sun_scattering_anisotropy = finite("fog_sun_anisotropy", v)?.min(0.95);
    }
    if let Some(v) = desc.fog_patchiness {
        fog.patchiness = finite("fog_patchiness", v)?.min(1.0);
    }
    if let Some(v) = desc.fog_patch_size {
        fog.patch_size = finite("fog_patch_size", v)?.max(0.5);
    }
    if let Some(wind) = desc.fog_wind {
        if !wind.iter().all(|v| v.is_finite()) {
            return Err("`fog_wind` must be finite numbers".into());
        }
        fog.wind = wind.into();
    }
    let shafts = &mut environment.light_shafts;
    if let Some(v) = desc.light_shafts {
        shafts.intensity = finite("light_shafts", v)?;
        shafts.enabled = shafts.intensity > 0.0;
    }
    if let Some(v) = desc.light_shafts_density {
        shafts.density = finite("light_shafts_density", v)?;
    }
    if let Some(v) = desc.light_shafts_anisotropy {
        shafts.anisotropy = finite("light_shafts_anisotropy", v)?.min(0.95);
    }
    if let Some(v) = desc.light_shafts_distance {
        shafts.max_distance = finite("light_shafts_distance", v)?.max(1.0);
    }
    let post = &mut environment.post;
    if let Some(e) = desc.exposure {
        post.exposure = e;
    }
    if let Some(v) = desc.contrast {
        post.contrast = finite("contrast", v)?;
    }
    if let Some(v) = desc.saturation {
        post.saturation = finite("saturation", v)?;
    }
    if let Some(c) = &desc.tint {
        post.tint = c.to_color()?;
    }
    if let Some(v) = desc.vignette {
        post.vignette_intensity = finite("vignette", v)?.min(1.0);
    }
    if let Some(v) = desc.vignette_smoothness {
        post.vignette_smoothness = finite("vignette_smoothness", v)?.min(1.0);
    }
    if let Some(v) = desc.bloom {
        post.bloom.intensity = finite("bloom", v)?;
    }
    if let Some(v) = desc.bloom_threshold {
        post.bloom.threshold = finite("bloom_threshold", v)?;
    }
    if let Some(v) = desc.chromatic_aberration {
        post.chromatic_aberration = finite("chromatic_aberration", v)?;
    }
    if let Some(v) = desc.grain {
        post.grain = finite("grain", v)?;
    }
    Ok(())
}
