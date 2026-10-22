//! Ragdolls made easy.
//!
//! * [`humanoid_ragdoll`] / [`spawn_humanoid_ragdoll`]: a ready-made jointed mannequin (a
//!   scene you can also save, tweak in the editor and instance).
//! * [`humanoid_ragdoll_for_model`]: a person-shaped ragdoll for any humanoid character
//!   (Mixamo, Unreal, Unity, Blender, VRoid... rigs: the bones are found by name or by the
//!   skeleton's shape), following its animation as hit boxes until [`go_limp`]. Scene files
//!   do the same with `ragdoll = { skeleton = "../Knight" }` ([`RagdollRig`]).
//! * [`ragdoll_for_model`]: a capsule per bone, for skeletons that are not humanoid.
//! * [`go_limp`]: stop the character's animation, let physics take over, push it.
//!
//! ```no_run
//! use ab_engine::prelude::*;
//! use ab_engine::ragdoll;
//!
//! fn on_death(tree: &mut SceneTree, character_model: NodeHandle, hit_from: Vec3) {
//!     // Usually built once when the character spawns.
//!     let doll = ragdoll::ragdoll_for_model(tree, character_model, SkeletonRagdoll::new(vec![]))
//!         .expect("a skinned model");
//!     let at = tree.global_position(character_model).unwrap() + Vec3::Y;
//!     ragdoll::go_limp(tree, doll, Some(((at - hit_from).normalize() * 40.0, at)));
//! }
//! ```

use crate::scene_file::{instantiate_scene, NodeDesc, SceneFile, SceneFileError};
use ab_animation::AnimationPlayer;
use ab_core::{Color, Vec3};
use ab_physics::{
    fill_humanoid_ragdoll, HumanBone, HumanoidMap, HumanoidRagdollSettings, PhysicsError,
    PhysicsNodes, Ragdoll, SkeletonRagdoll,
};
use ab_rendering::Skin;
use ab_scene::{NodeHandle, SceneTree};
use std::collections::BTreeMap;

/// Options of [`humanoid_ragdoll`].
#[derive(Debug, Clone, PartialEq)]
pub struct HumanoidRagdoll {
    pub name: String,
    /// Standing height in metres (proportions scale with it).
    pub height: f32,
    /// Total mass in kg.
    pub mass: f32,
    pub color: Color,
    /// Limp from the start; otherwise it stands frozen until activated.
    pub active: bool,
}

impl Default for HumanoidRagdoll {
    fn default() -> Self {
        Self {
            name: "Ragdoll".into(),
            height: 1.8,
            mass: 70.0,
            color: Color::from_srgb8(196, 120, 72, 255),
            active: true,
        }
    }
}

enum PartShape {
    Box([f32; 3]),
    Sphere(f32),
}

struct Part {
    name: String,
    shape: PartShape,
    center: [f32; 3],
    /// Share of the total mass.
    mass: f32,
}

struct JointSpec {
    name: String,
    a: String,
    b: String,
    anchor: [f32; 3],
    /// Euler degrees putting the joint's X axis along the limb.
    rotation: [f32; 3],
    /// `kind` table entries.
    joint: &'static str,
}

const UP: [f32; 3] = [0.0, 0.0, 90.0];
const DOWN: [f32; 3] = [0.0, 0.0, -90.0];
const SIDE: [f32; 3] = [0.0, 0.0, 0.0];

fn parts() -> Vec<Part> {
    let b = PartShape::Box;
    let mut parts = vec![
        Part {
            name: "Pelvis".into(),
            shape: b([0.32, 0.2, 0.2]),
            center: [0.0, 0.95, 0.0],
            mass: 0.15,
        },
        Part {
            name: "Chest".into(),
            shape: b([0.38, 0.45, 0.22]),
            center: [0.0, 1.3, 0.0],
            mass: 0.25,
        },
        Part {
            name: "Head".into(),
            shape: PartShape::Sphere(0.12),
            center: [0.0, 1.66, 0.0],
            mass: 0.08,
        },
    ];
    for (side, x) in [("L", -1.0), ("R", 1.0)] {
        parts.extend([
            Part {
                name: format!("UpperArm{side}"),
                shape: b([0.1, 0.3, 0.1]),
                center: [0.25 * x, 1.33, 0.0],
                mass: 0.03,
            },
            Part {
                name: format!("LowerArm{side}"),
                shape: b([0.09, 0.3, 0.09]),
                center: [0.25 * x, 1.03, 0.0],
                mass: 0.02,
            },
            Part {
                name: format!("Thigh{side}"),
                shape: b([0.14, 0.4, 0.14]),
                center: [0.1 * x, 0.65, 0.0],
                mass: 0.1,
            },
            Part {
                name: format!("Shin{side}"),
                shape: b([0.11, 0.42, 0.11]),
                center: [0.1 * x, 0.24, 0.0],
                mass: 0.05,
            },
        ]);
    }
    parts
}

fn joints() -> Vec<JointSpec> {
    let mut joints = vec![
        JointSpec {
            name: "Spine".into(),
            a: "Pelvis".into(),
            b: "Chest".into(),
            anchor: [0.0, 1.065, 0.0],
            rotation: UP,
            joint: "kind = \"cone\", swing = 25.0, twist = 20.0",
        },
        JointSpec {
            name: "Neck".into(),
            a: "Chest".into(),
            b: "Head".into(),
            anchor: [0.0, 1.54, 0.0],
            rotation: UP,
            joint: "kind = \"cone\", swing = 35.0, twist = 40.0",
        },
    ];
    for (side, x) in [("L", -1.0), ("R", 1.0)] {
        joints.extend([
            JointSpec {
                name: format!("Shoulder{side}"),
                a: "Chest".into(),
                b: format!("UpperArm{side}"),
                anchor: [0.25 * x, 1.47, 0.0],
                rotation: DOWN,
                joint: "kind = \"cone\", swing = 80.0, twist = 45.0",
            },
            JointSpec {
                name: format!("Elbow{side}"),
                a: format!("UpperArm{side}"),
                b: format!("LowerArm{side}"),
                anchor: [0.25 * x, 1.18, 0.0],
                rotation: SIDE,
                joint: "kind = \"hinge\", limits = [0.0, 145.0]",
            },
            JointSpec {
                name: format!("Hip{side}"),
                a: "Pelvis".into(),
                b: format!("Thigh{side}"),
                anchor: [0.1 * x, 0.85, 0.0],
                rotation: DOWN,
                joint: "kind = \"cone\", swing = 60.0, twist = 20.0",
            },
            JointSpec {
                name: format!("Knee{side}"),
                a: format!("Thigh{side}"),
                b: format!("Shin{side}"),
                anchor: [0.1 * x, 0.45, 0.0],
                rotation: SIDE,
                joint: "kind = \"hinge\", limits = [-145.0, 0.0]",
            },
        ]);
    }
    joints
}

/// A jointed mannequin (11 bodies, 10 joints) standing on its feet at the origin, as a
/// scene: instance it with [`spawn_humanoid_ragdoll`], save it, or edit it.
pub fn humanoid_ragdoll(options: &HumanoidRagdoll) -> SceneFile {
    let k = options.height / 1.8;
    let scaled = |v: [f32; 3]| v.map(|c| c * k);
    let color = crate::scene_file::ColorDesc::from_color(options.color);
    let skin = crate::scene_file::ColorDesc::from_color(Color::from_srgb8(232, 190, 160, 255));
    let parts = parts();
    let total: f32 = parts.iter().map(|p| p.mass).sum();
    let mut toml = format!(
        "[[node]]\nname = \"{}\"\ntype = \"Ragdoll\"\nragdoll = {{ active = {} }}\n",
        options.name, options.active
    );
    for part in &parts {
        let mass = options.mass * part.mass / total;
        let [x, y, z] = scaled(part.center);
        toml.push_str(&format!(
            "\n[[node]]\nname = \"{}\"\nparent = \".\"\ntype = \"RigidBody3D\"\nposition = [{x}, {y}, {z}]\nbody = {{ mass = {mass}, friction = 0.8, angular_damping = 0.5 }}\n",
            part.name
        ));
        let (shape, mesh, scale, look) = match part.shape {
            PartShape::Box(size) => {
                let [sx, sy, sz] = scaled(size);
                (
                    format!("{{ box = [{sx}, {sy}, {sz}] }}"),
                    "{ primitive = \"cube\" }".to_owned(),
                    format!("scale = [{sx}, {sy}, {sz}]\n"),
                    &color,
                )
            }
            PartShape::Sphere(radius) => {
                let r = radius * k;
                (
                    format!("{{ sphere = {r} }}"),
                    format!("{{ primitive = \"sphere\", radius = {r} }}"),
                    String::new(),
                    &skin,
                )
            }
        };
        let look = match look {
            crate::scene_file::ColorDesc::Hex(hex) => hex.clone(),
            _ => "#ffffff".into(),
        };
        toml.push_str(&format!(
            "\n[[node]]\nname = \"Mesh\"\nparent = \"{0}\"\ntype = \"MeshInstance3D\"\n{scale}mesh = {mesh}\nmaterial = {{ color = \"{look}\", roughness = 0.7 }}\n\n[[node]]\nname = \"Shape\"\nparent = \"{0}\"\ntype = \"CollisionShape3D\"\nshape = {shape}\n",
            part.name
        ));
    }
    for joint in joints() {
        let [x, y, z] = scaled(joint.anchor);
        let [rx, ry, rz] = joint.rotation;
        toml.push_str(&format!(
            "\n[[node]]\nname = \"{}\"\nparent = \".\"\ntype = \"Joint3D\"\nposition = [{x}, {y}, {z}]\nrotation = [{rx}, {ry}, {rz}]\njoint = {{ {}, body_a = \"../{}\", body_b = \"../{}\" }}\n",
            joint.name, joint.joint, joint.a, joint.b
        ));
    }
    SceneFile::from_toml(&toml).expect("the humanoid ragdoll template parses")
}

/// Builds [`humanoid_ragdoll`] as a detached subtree; position it and add it to the tree.
pub fn spawn_humanoid_ragdoll(
    tree: &mut SceneTree,
    options: &HumanoidRagdoll,
) -> Result<NodeHandle, SceneFileError> {
    instantiate_scene(tree, &humanoid_ragdoll(options), "humanoid ragdoll")
}

/// Builds a ragdoll for the skinned model under `model` (bones from its first `Skin`, unless
/// `settings.bones` lists them) and adds it to the model. It follows the animation, so its
/// bodies work as hit boxes, until [`go_limp`].
pub fn ragdoll_for_model(
    tree: &mut SceneTree,
    model: NodeHandle,
    mut settings: SkeletonRagdoll,
) -> Result<NodeHandle, PhysicsError> {
    if settings.bones.is_empty() {
        settings.bones = tree
            .subtree(model)
            .into_iter()
            .find_map(|n| tree.component::<Skin>(n).map(|s| s.joints.clone()))
            .unwrap_or_default();
    }
    tree.create_skeleton_ragdoll(model, "Ragdoll", &settings)
}

/// Makes a ragdoll limp: stops the animation players of the model it belongs to (its
/// parent's subtree), activates it, and optionally applies `(impulse, at)` to the part
/// nearest `at`.
pub fn go_limp(
    tree: &mut SceneTree,
    ragdoll: NodeHandle,
    impulse: Option<(Vec3, Vec3)>,
) -> Result<(), PhysicsError> {
    let model = tree.parent(ragdoll).unwrap_or(ragdoll);
    for node in tree.subtree(model) {
        if let Some(mut player) = tree.component_mut::<AnimationPlayer>(node) {
            player.stop();
        }
    }
    tree.set_ragdoll_active(ragdoll, true)?;
    if let Some((impulse, at)) = impulse {
        tree.apply_ragdoll_impulse(ragdoll, impulse, at);
    }
    Ok(())
}

/// A [`NodeDesc`] for an empty `Ragdoll` node (editor "create" menus).
pub fn ragdoll_node(name: &str, active: bool) -> NodeDesc {
    NodeDesc {
        name: name.into(),
        kind: Some("Ragdoll".into()),
        ragdoll: Some(crate::scene_file::RagdollDesc {
            active: Some(active),
            ..Default::default()
        }),
        ..Default::default()
    }
}

// =============================================================================================
// Humanoid ragdolls for characters
// =============================================================================================

/// On a `Ragdoll` node: build a humanoid ragdoll for the character at `skeleton` (a path from
/// this node). Its bodies are made when the scene loads and are not saved; the scene file
/// keeps only this.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RagdollRig {
    /// Path from the `Ragdoll` node to the character (its model instance).
    pub skeleton: String,
    /// Bones set by hand, by [`HumanBone::key`] (`left_upper_arm`): the bone node's name, or
    /// `""` for none. The others are found automatically.
    pub bones: BTreeMap<String, String>,
    /// Total mass in kg (default 70).
    pub mass: Option<f32>,
    /// Body thickness, 1 = average build.
    pub thickness: Option<f32>,
}

/// The bones of the character under `model`: the joints of its first skin, or every 3D node
/// below it when it has no skin.
pub fn skeleton_bones(tree: &SceneTree, model: NodeHandle) -> Vec<NodeHandle> {
    let nodes = tree.subtree(model);
    nodes
        .iter()
        .find_map(|&n| tree.component::<Skin>(n).map(|s| s.joints.clone()))
        .unwrap_or_else(|| {
            nodes
                .into_iter()
                .filter(|&n| n != model && tree.is_node3d(n))
                .collect()
        })
}

/// The humanoid bones of the character under `model`: detected, then `overrides`
/// ([`RagdollRig::bones`]) applied.
pub fn humanoid_map(
    tree: &SceneTree,
    model: NodeHandle,
    overrides: &BTreeMap<String, String>,
) -> HumanoidMap {
    let bones = skeleton_bones(tree, model);
    let mut map = HumanoidMap::detect(tree, &bones);
    for (key, name) in overrides {
        let Some(slot) = HumanBone::from_key(key) else {
            tracing::warn!("ragdoll bones: unknown bone `{key}`");
            continue;
        };
        let node = bones
            .iter()
            .copied()
            .find(|&b| tree.name(b).is_some_and(|n| *n == **name));
        if node.is_none() && !name.is_empty() {
            tracing::warn!("ragdoll bones: no bone named `{name}` for {key}");
        }
        map.set(slot, node);
    }
    map
}

/// Builds a humanoid ragdoll for the character under `model` and adds it to the model. It
/// follows the animation (its bodies work as hit boxes) until [`go_limp`] or
/// `settings.active`.
pub fn humanoid_ragdoll_for_model(
    tree: &mut SceneTree,
    model: NodeHandle,
    settings: &HumanoidRagdollSettings,
) -> Result<NodeHandle, PhysicsError> {
    let map = humanoid_map(tree, model, &BTreeMap::new());
    ab_physics::create_humanoid_ragdoll(tree, model, "Ragdoll", &map, settings)
}

/// Builds the bodies of every [`RagdollRig`] in `root`'s subtree that has none yet. Scene
/// loading calls it; call it again after editing a rig (with its old bodies freed).
/// Returns the rigs that could not be built, with why.
pub fn build_rigs(tree: &mut SceneTree, root: NodeHandle) -> Vec<(NodeHandle, String)> {
    let rigs: Vec<NodeHandle> = tree
        .subtree(root)
        .into_iter()
        .filter(|&n| tree.component::<RagdollRig>(n).is_some() && tree.children(n).is_empty())
        .collect();
    if rigs.is_empty() {
        return Vec::new();
    }
    // The bodies are sized from the bones' global poses: bring them up to date, also for a
    // scene still being built off the tree.
    let top = std::iter::successors(Some(root), |&n| tree.parent(n))
        .last()
        .unwrap_or(root);
    ab_scene::propagate_transforms(tree.world(), top);
    let mut errors = Vec::new();
    for node in rigs {
        let rig = tree
            .component::<RagdollRig>(node)
            .map(|r| r.clone())
            .expect("filtered above");
        let active = tree.component::<Ragdoll>(node).is_some_and(|r| r.active);
        let Some(model) = tree.get_node(node, &rig.skeleton) else {
            // A rig built on its own (the editor rebuilding it) is built once placed.
            if tree.parent(node).is_some() {
                errors.push((node, format!("skeleton `{}` not found", rig.skeleton)));
            }
            continue;
        };
        let map = humanoid_map(tree, model, &rig.bones);
        let settings = HumanoidRagdollSettings {
            total_mass: rig.mass.unwrap_or(70.0).max(1.0),
            thickness: rig.thickness.unwrap_or(1.0),
            active,
            ..Default::default()
        };
        if let Err(err) = fill_humanoid_ragdoll(tree, node, &map, &settings) {
            // Half-built bodies would only get in the way.
            for child in tree.children(node) {
                let _ = tree.queue_free(child);
            }
            tree.flush_deletions();
            errors.push((node, err.to_string()));
        }
    }
    errors
}
