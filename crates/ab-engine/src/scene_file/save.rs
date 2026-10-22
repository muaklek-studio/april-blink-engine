//! Describing a subtree as a [`SceneFile`].

use super::format::*;
use super::{SceneInstance, SceneOrigin};
use ab_audio::AudioPlayer;
use ab_core::{EulerRot, Transform3D, Vec3};
use ab_physics::{
    BodyKind, CharacterBody3D, CollisionShape3D, Joint3D, JointKind, PhysicsBody3D, Ragdoll, Shape,
};
use ab_rendering::{Camera3D, DirectionalLight3D, MeshInstance3D, OmniLight3D, SpotLight3D};
use ab_scene::{Groups, NodeHandle, SceneTree, Timer};
use tracing::warn;

/// Describes `root` and its subtree. Instanced models and scenes are written as
/// `instance = path` without their contents. Nodes whose data cannot be described (UI,
/// meshes created in code, triangle-mesh shapes) are written as plain `Node` / `Node3D`
/// with a warning. Write the result with [`SceneFile::to_toml`].
pub fn save_scene(tree: &SceneTree, root: NodeHandle) -> SceneFile {
    let mut scene = SceneFile::default();
    // (node, parent path, own path)
    let mut stack = vec![(root, None::<String>, ".".to_owned())];
    while let Some((node, parent, path)) = stack.pop() {
        let Some(name) = tree.name(node).map(|n| n.to_owned()) else {
            continue;
        };
        let mut desc = NodeDesc {
            name: name.clone(),
            parent,
            ..Default::default()
        };
        common(tree, node, &mut desc);
        let instance = tree.component::<SceneInstance>(node).map(|i| i.0.clone());
        match instance {
            Some(instance) => desc.instance = Some(instance),
            None => {
                describe(tree, node, &mut desc);
                for child in tree.children(node).into_iter().rev() {
                    let Some(child_name) = tree.name(child).map(|n| n.to_owned()) else {
                        continue;
                    };
                    let child_path = if path == "." {
                        child_name
                    } else {
                        format!("{path}/{child_name}")
                    };
                    stack.push((child, Some(path.clone()), child_path));
                }
            }
        }
        scene.nodes.push(desc);
    }
    scene
}

fn common(tree: &SceneTree, node: NodeHandle, desc: &mut NodeDesc) {
    if let Some(Transform3D {
        translation,
        rotation,
        scale,
    }) = tree.transform(node)
    {
        if translation != Vec3::ZERO {
            desc.position = Some(round3(translation.to_array()));
        }
        let (y, x, z) = rotation.to_euler(EulerRot::YXZ);
        // Thousandths of a degree: loading and saving again gives the same numbers.
        let degrees = [x, y, z].map(|r| {
            let d = (r.to_degrees() * 1e3).round() / 1e3;
            if d == 0.0 {
                0.0
            } else {
                d
            }
        });
        if degrees != [0.0; 3] {
            desc.rotation = Some(degrees);
        }
        if scale != Vec3::ONE {
            desc.scale = Some(round3(scale.to_array()));
        }
    }
    if !tree.is_visible(node) {
        desc.visible = Some(false);
    }
    if let Some(groups) = tree.component::<Groups>(node) {
        desc.groups = groups.0.clone();
    }
    desc.set_script_list(tree.script_names(node));
}

/// Type and type-specific table.
fn describe(tree: &SceneTree, node: NodeHandle, desc: &mut NodeDesc) {
    let class = tree.class_name(node).unwrap_or("Node");
    let plain = if tree.is_node3d(node) {
        "Node3D"
    } else {
        "Node"
    };
    let origin = tree.component::<SceneOrigin>(node).map(|o| o.clone());
    let kind: &str = match class {
        "Node" | "Node3D" | "AudioListener3D" | "CameraShake" => class,
        "Ragdoll" => {
            let active = tree.component::<Ragdoll>(node).is_some_and(|r| r.active);
            desc.ragdoll = Some(RagdollDesc {
                active: active.then_some(true),
            });
            class
        }
        "Joint3D" => match tree.component::<Joint3D>(node).map(|j| j.clone()) {
            Some(joint) => {
                let path = |target: Option<NodeHandle>| {
                    target
                        .and_then(|t| relative_path(tree, node, t))
                        .unwrap_or_default()
                };
                let round = |radians: f32| (radians.to_degrees() * 1e3).round() / 1e3;
                let mut d = JointDesc {
                    body_a: path(joint.body_a),
                    body_b: path(joint.body_b),
                    collide: joint.collide_connected.then_some(true),
                    ..Default::default()
                };
                match joint.kind {
                    JointKind::Pin => {}
                    JointKind::Fixed => d.kind = Some("fixed".into()),
                    JointKind::Hinge { limits } => {
                        d.kind = Some("hinge".into());
                        d.limits = limits.map(|l| l.map(round));
                    }
                    JointKind::ConeTwist { swing, twist } => {
                        d.kind = Some("cone".into());
                        d.swing = Some(round(swing));
                        d.twist = Some(round(twist));
                    }
                }
                desc.joint = Some(d);
                class
            }
            None => unsupported(tree, node, class, plain),
        },
        "Particles3D" => match origin.as_ref().and_then(|o| o.particles.clone()) {
            Some(particles) => {
                desc.particles = Some(particles);
                class
            }
            None => unsupported(tree, node, class, plain),
        },
        "MeshInstance3D" => match origin.as_ref().and_then(|o| o.mesh.clone()) {
            Some(mesh) => {
                desc.mesh = Some(mesh);
                desc.material = origin.as_ref().and_then(|o| o.material.clone());
                let shadows = tree
                    .component::<MeshInstance3D>(node)
                    .is_none_or(|m| m.cast_shadows);
                desc.cast_shadows = (!shadows).then_some(false);
                class
            }
            None => unsupported(tree, node, class, plain),
        },
        "Camera3D" => {
            if let Some(c) = tree.component::<Camera3D>(node) {
                desc.camera = Some(CameraDesc {
                    fov: Some(c.fov_y_degrees),
                    near: Some(c.near),
                    far: Some(c.far),
                    current: c.current.then_some(true),
                });
            }
            class
        }
        "DirectionalLight3D" => {
            if let Some(l) = tree.component::<DirectionalLight3D>(node) {
                desc.light = Some(LightDesc {
                    color: Some(ColorDesc::from_color(l.color)),
                    energy: Some(l.energy),
                    shadows: l.shadow_enabled.then_some(true),
                    ..Default::default()
                });
            }
            class
        }
        "OmniLight3D" => {
            if let Some(l) = tree.component::<OmniLight3D>(node) {
                desc.light = Some(LightDesc {
                    color: Some(ColorDesc::from_color(l.color)),
                    energy: Some(l.energy),
                    range: Some(l.range),
                    ..Default::default()
                });
            }
            class
        }
        "SpotLight3D" => {
            if let Some(l) = tree.component::<SpotLight3D>(node) {
                desc.light = Some(LightDesc {
                    color: Some(ColorDesc::from_color(l.color)),
                    energy: Some(l.energy),
                    range: Some(l.range),
                    angle: Some(l.angle_degrees),
                    softness: Some(l.edge_softness),
                    shadows: l.shadow_enabled.then_some(true),
                });
            }
            class
        }
        "StaticBody3D" | "RigidBody3D" | "AnimatableBody3D" | "Area3D" | "CharacterBody3D" => {
            if let Some(b) = tree.component::<PhysicsBody3D>(node) {
                // All defaults: nothing to write (and no `body = {}` in the file).
                desc.body = Some(body(&b)).filter(|d| *d != BodyDesc::default());
                if b.kind == BodyKind::Character {
                    if let Some(c) = tree.component::<CharacterBody3D>(node) {
                        desc.character = Some(CharacterDesc {
                            floor_max_angle: Some(round(c.floor_max_angle.to_degrees())),
                            floor_snap_length: Some(c.floor_snap_length),
                            max_step_height: Some(c.max_step_height),
                            push_rigid_bodies: Some(c.push_rigid_bodies),
                        });
                    }
                }
            }
            class
        }
        "CollisionShape3D" => {
            let shape = tree
                .component::<CollisionShape3D>(node)
                .and_then(|s| match s.shape() {
                    Shape::Box { size } => Some(ShapeDesc::Box(size.to_array())),
                    Shape::Sphere { radius } => Some(ShapeDesc::Sphere(*radius)),
                    Shape::Capsule { radius, height } => {
                        Some(ShapeDesc::Capsule([*radius, *height]))
                    }
                    Shape::Cylinder { radius, height } => {
                        Some(ShapeDesc::Cylinder([*radius, *height]))
                    }
                    Shape::ConvexHull { .. } | Shape::TriMesh { .. } => None,
                });
            match shape {
                Some(shape) => {
                    desc.shape = Some(shape);
                    class
                }
                None => unsupported(tree, node, class, plain),
            }
        }
        "AudioStreamPlayer" | "AudioStreamPlayer3D" => {
            match (
                origin.and_then(|o| o.audio_stream),
                tree.component::<AudioPlayer>(node),
            ) {
                (Some(stream), Some(p)) => {
                    let attenuation = p.positional;
                    desc.audio = Some(AudioDesc {
                        stream,
                        volume_db: Some(p.volume_db),
                        pitch: Some(p.pitch_scale),
                        looping: Some(p.looping),
                        autoplay: Some(p.autoplay),
                        min_distance: attenuation.map(|a| a.min_distance),
                        max_distance: attenuation.map(|a| a.max_distance),
                    });
                    class
                }
                _ => unsupported(tree, node, class, plain),
            }
        }
        "Timer" => {
            if let Some(t) = tree.component::<Timer>(node) {
                desc.timer = Some(TimerDesc {
                    wait_time: t.wait_time,
                    one_shot: Some(t.one_shot),
                    autostart: Some(t.autostart),
                });
            }
            class
        }
        _ => unsupported(tree, node, class, plain),
    };
    desc.kind = Some(kind.to_owned());
}

fn unsupported<'a>(tree: &SceneTree, node: NodeHandle, class: &str, plain: &'a str) -> &'a str {
    warn!(
        node = tree.path_of(node).unwrap_or_else(|| node.to_string()),
        class, "cannot describe this node in a scene file; saved as {plain}"
    );
    plain
}

fn body(b: &PhysicsBody3D) -> BodyDesc {
    let default = PhysicsBody3D::new(b.kind);
    let changed = |v: f32, d: f32| (v != d).then_some(v);
    BodyDesc {
        mass: changed(b.mass, default.mass),
        gravity_scale: changed(b.gravity_scale, default.gravity_scale),
        friction: changed(b.friction, default.friction),
        bounce: changed(b.bounce, default.bounce),
        linear_damping: changed(b.linear_damping, default.linear_damping),
        angular_damping: changed(b.angular_damping, default.angular_damping),
        lock_rotation: (b.lock_rotation != default.lock_rotation).then_some(b.lock_rotation),
        continuous_cd: (b.continuous_cd != default.continuous_cd).then_some(b.continuous_cd),
        can_sleep: (b.can_sleep != default.can_sleep).then_some(b.can_sleep),
        layer: (b.collision_layer != default.collision_layer).then_some(b.collision_layer),
        mask: (b.collision_mask != default.collision_mask).then_some(b.collision_mask),
    }
}

/// Six significant decimals: keeps files readable and stable across save/load cycles.
fn round(v: f32) -> f32 {
    let r = (v * 1e5).round() / 1e5;
    if r == 0.0 {
        0.0 // no "-0.0"
    } else {
        r
    }
}

fn round3(v: [f32; 3]) -> [f32; 3] {
    v.map(round)
}

/// Path from `from` to `to` (`"../Torso"`), through their nearest common ancestor.
pub fn relative_path(tree: &SceneTree, from: NodeHandle, to: NodeHandle) -> Option<String> {
    let chain = |node: NodeHandle| {
        let mut chain: Vec<NodeHandle> =
            std::iter::successors(Some(node), |&n| tree.parent(n)).collect();
        chain.reverse();
        chain
    };
    let (up, down) = (chain(from), chain(to));
    let common = up.iter().zip(&down).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return None;
    }
    let mut parts: Vec<String> = vec!["..".to_owned(); up.len() - common];
    for &node in &down[common..] {
        parts.push(tree.name(node)?.to_owned());
    }
    Some(if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    })
}
