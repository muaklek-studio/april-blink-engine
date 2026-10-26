//! Inverse kinematics: turning a chain of bones so its tip reaches a target.
//!
//! A node (usually a character's model) lists its chains in `ik`:
//!
//! ```toml
//! rig = "rigs/knight.abrig"
//! ik = [
//!     { tip = "left_foot", ground = true },
//!     { tip = "right_foot", ground = true },
//!     { tip = "right_hand", target = "../Lever/Handle", pole = "../ElbowHint", align = true },
//! ]
//! ```
//!
//! * `tip` is a bone name or a humanoid part (`left_hand`); `bones` bones above it turn (2:
//!   upper and lower arm, thigh and shin), or `chain` names one of the rig's chains.
//! * `target` is a node to reach (a path from this node); `ground` puts the tip on the
//!   ground below where the animation has it (foot placement on slopes and steps).
//! * `pole` is a node the middle joint bends towards (the knee forwards, the elbow out).
//! * `solver`: `two_bone` (exact, for limbs; the default for 2 bones) or `fabrik` (any
//!   length: tails, tentacles, spines).
//! * `weight` mixes the solved pose with the animated one (0..1); `align` turns the tip to
//!   the target's rotation.
//!
//! The rig's joint limits (see [`crate::rig`]) are kept. Chains are solved after the scripts
//! and animations of the frame, on what they posed. [`IkDebug`] draws the chains, their
//! targets and poles.

use crate::rig::{JointLimit, Rig};
use ab_core::{Affine3A, Color, Quat, Vec3};
use ab_physics::{PhysicsWorld, QueryOptions};
use ab_rendering::DebugDraw;
use ab_scene::{NodeHandle, SceneTree};
use serde::{Deserialize, Serialize};

/// How a chain is solved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Solver {
    /// Exact for two bones (the default for 2 bones).
    #[default]
    TwoBone,
    /// Iterative, any number of bones.
    Fabrik,
}

/// One entry of a node's `ik = [...]` (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct IkDesc {
    /// The last bone: a bone name or a humanoid part.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tip: String,
    /// A chain of the node's rig, instead of `tip` and `bones`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain: Option<String>,
    /// Bones above the tip that turn.
    #[serde(skip_serializing_if = "is_two")]
    pub bones: usize,
    /// A node to reach, by path from this node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Put the tip on the ground below its animated position.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ground: bool,
    /// A node the middle joint bends towards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pole: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solver: Option<Solver>,
    #[serde(skip_serializing_if = "is_one")]
    pub weight: f32,
    /// Turn the tip to the target's rotation.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub align: bool,
}

fn is_two(v: &usize) -> bool {
    *v == 2
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

impl Default for IkDesc {
    fn default() -> Self {
        Self {
            tip: String::new(),
            chain: None,
            bones: 2,
            target: None,
            ground: false,
            pole: None,
            solver: None,
            weight: 1.0,
            align: false,
        }
    }
}

/// Draw the IK chains (red: the bones, yellow: targets, blue: poles).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IkDebug(pub bool);

/// A chain's bones, root first, found once.
#[derive(Debug, Clone, PartialEq)]
struct Resolved {
    joints: Vec<NodeHandle>,
    limits: Vec<Option<JointLimit>>,
    /// Each joint's local rotation at rest (when found).
    rest: Vec<Quat>,
    /// The tip's height above the node at rest (foot placement keeps it over the ground).
    tip_height: f32,
    /// The joints' local rotations before IK last frame, and as IK left them: a joint still
    /// as IK left it (nothing animated it since) goes back to its pose before IK, so the
    /// chain is solved from the animation, not from last frame's solution.
    animated: Vec<Quat>,
    written: Vec<Quat>,
}

/// A node's IK chains (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct IkChains {
    pub chains: Vec<IkDesc>,
    resolved: Vec<Option<Resolved>>,
    /// Why chains could not be set up, once each.
    pub errors: Vec<String>,
    /// Off: the animation only.
    pub enabled: bool,
}

impl IkChains {
    pub fn new(chains: Vec<IkDesc>) -> Self {
        Self {
            resolved: vec![None; chains.len()],
            chains,
            errors: Vec::new(),
            enabled: true,
        }
    }
}

// =============================================================================================
// Solvers (positions)
// =============================================================================================

/// Where the three joints of a two-bone chain go for the tip to reach `target`, bending
/// towards `pole` (or the way it bends now). Out of reach: the chain points straight at it.
pub fn solve_two_bone(joints: [Vec3; 3], target: Vec3, pole: Option<Vec3>) -> [Vec3; 3] {
    let [root, mid, tip] = joints;
    let upper = mid.distance(root);
    let lower = tip.distance(mid);
    let to_target = target - root;
    let Some(dir) = to_target.try_normalize() else {
        return joints;
    };
    let reach = to_target
        .length()
        .clamp((upper - lower).abs() + 1e-4, upper + lower - 1e-4);
    let bend = pole.map_or(mid - root, |p| p - root);
    let side = (bend - dir * bend.dot(dir))
        .try_normalize()
        .unwrap_or_else(|| dir.any_orthonormal_vector());
    let cos =
        ((upper * upper + reach * reach - lower * lower) / (2.0 * upper * reach)).clamp(-1.0, 1.0);
    let sin = (1.0 - cos * cos).max(0.0).sqrt();
    let new_mid = root + dir * (upper * cos) + side * (upper * sin);
    [root, new_mid, root + dir * reach]
}

/// FABRIK: moves `joints` (root first, root kept) so the last reaches `target`. Out of
/// reach: stretched towards it.
pub fn solve_fabrik(joints: &mut [Vec3], target: Vec3, iterations: usize, tolerance: f32) {
    let n = joints.len();
    if n < 2 {
        return;
    }
    let lengths: Vec<f32> = joints.windows(2).map(|w| w[0].distance(w[1])).collect();
    let root = joints[0];
    let total: f32 = lengths.iter().sum();
    if root.distance(target) >= total {
        let dir = (target - root).normalize_or(Vec3::Y);
        for i in 1..n {
            joints[i] = joints[i - 1] + dir * lengths[i - 1];
        }
        return;
    }
    for _ in 0..iterations {
        if joints[n - 1].distance(target) <= tolerance {
            break;
        }
        joints[n - 1] = target;
        for i in (0..n - 1).rev() {
            let dir = (joints[i] - joints[i + 1]).normalize_or(Vec3::Y);
            joints[i] = joints[i + 1] + dir * lengths[i];
        }
        joints[0] = root;
        for i in 1..n {
            let dir = (joints[i] - joints[i - 1]).normalize_or(Vec3::Y);
            joints[i] = joints[i - 1] + dir * lengths[i - 1];
        }
    }
}

// =============================================================================================
// Running chains on a tree
// =============================================================================================

/// `count` bones above `tip` (and `tip`), root first.
fn chain_above(tree: &SceneTree, tip: NodeHandle, count: usize) -> Option<Vec<NodeHandle>> {
    let mut joints = vec![tip];
    let mut at = tip;
    for _ in 0..count {
        at = tree.parent(at)?;
        joints.push(at);
    }
    joints.reverse();
    Some(joints)
}

fn resolve(tree: &SceneTree, node: NodeHandle, desc: &IkDesc) -> Result<Resolved, String> {
    let rig = tree
        .component::<Rig>(node)
        .map(|r| r.clone())
        .unwrap_or_default();
    let (tip, bones) = match &desc.chain {
        Some(name) => {
            let chain = rig
                .desc
                .chains
                .iter()
                .find(|c| c.name == *name)
                .ok_or_else(|| format!("ik: the rig has no chain `{name}`"))?;
            (chain.tip.clone(), chain.bones)
        }
        None => (desc.tip.clone(), desc.bones),
    };
    if tip.is_empty() {
        return Err("ik: give a `tip` bone or a `chain`".into());
    }
    let tip_node = rig
        .bone(tree, node, &tip)
        .ok_or_else(|| format!("ik: no bone `{tip}`"))?;
    let joints = chain_above(tree, tip_node, bones.max(1))
        .ok_or_else(|| format!("ik: `{tip}` has fewer than {bones} bones above it"))?;
    let limits = joints.iter().map(|&j| rig.limit(tree, node, j)).collect();
    let rest: Vec<Quat> = joints
        .iter()
        .map(|&j| tree.rotation(j).unwrap_or(Quat::IDENTITY))
        .collect();
    let base = crate::procgen::world_of(tree, node).translation;
    let tip_height = crate::procgen::world_of(tree, tip_node).translation.y - base.y;
    Ok(Resolved {
        animated: rest.clone(),
        written: Vec::new(),
        joints,
        limits,
        rest,
        tip_height,
    })
}

fn rotation_of(world: &Affine3A) -> Quat {
    world.to_scale_rotation_translation().1
}

/// Turns the chain's joints so they lie on `positions` (as far as `weight`), keeping the
/// limits; writes their local rotations. `worlds`: the joints' world transforms now.
fn pose_chain(
    tree: &mut SceneTree,
    chain: &Resolved,
    worlds: &mut [Affine3A],
    positions: &[Vec3],
    weight: f32,
    tip_rotation: Option<Quat>,
) {
    let n = chain.joints.len();
    let parent_world = |tree: &SceneTree, i: usize, worlds: &[Affine3A]| -> Quat {
        if i == 0 {
            tree.parent(chain.joints[0]).map_or(Quat::IDENTITY, |p| {
                rotation_of(&crate::procgen::world_of(tree, p))
            })
        } else {
            rotation_of(&worlds[i - 1])
        }
    };
    for i in 0..n - 1 {
        let from = worlds[i + 1].translation - worlds[i].translation;
        let to = positions[i + 1] - Vec3::from(worlds[i].translation);
        let (Some(from), Some(to)) = (Vec3::from(from).try_normalize(), to.try_normalize()) else {
            continue;
        };
        let turn = Quat::IDENTITY.slerp(Quat::from_rotation_arc(from, to), weight.clamp(0.0, 1.0));
        let parent = parent_world(tree, i, worlds);
        let mut world_rotation = turn * rotation_of(&worlds[i]);
        let mut local = parent.inverse() * world_rotation;
        if let Some(limit) = chain.limits[i] {
            let child = tree
                .transform(chain.joints[i + 1])
                .map_or(Vec3::NEG_Y, |t| t.translation);
            local = limit.apply(local, chain.rest[i], child);
            world_rotation = parent * local;
        }
        let _ = tree.set_rotation(chain.joints[i], local.normalize());
        // The joints below follow.
        let applied = world_rotation * rotation_of(&worlds[i]).inverse();
        let pivot = Vec3::from(worlds[i].translation);
        worlds[i] = Affine3A::from_scale_rotation_translation(
            worlds[i].to_scale_rotation_translation().0,
            world_rotation,
            pivot,
        );
        for w in worlds.iter_mut().skip(i + 1) {
            let (scale, rotation, translation) = w.to_scale_rotation_translation();
            *w = Affine3A::from_scale_rotation_translation(
                scale,
                applied * rotation,
                pivot + applied * (translation - pivot),
            );
        }
    }
    if let Some(rotation) = tip_rotation {
        let parent = parent_world(tree, n - 1, worlds);
        let current = rotation_of(&worlds[n - 1]);
        let wanted = current.slerp(rotation, weight.clamp(0.0, 1.0));
        let _ = tree.set_rotation(chain.joints[n - 1], (parent.inverse() * wanted).normalize());
    }
}

/// Solves every node's IK chains on the pose of this frame. The engine calls it after the
/// scripts and animations.
pub fn update(tree: &mut SceneTree) {
    let nodes: Vec<NodeHandle> = tree
        .world()
        .query::<(ab_ecs::Entity, &IkChains)>()
        .iter()
        .map(|(e, _)| NodeHandle::from_entity(e))
        .collect();
    let debug = tree.world().resource::<IkDebug>().is_some_and(|d| d.0);
    for node in nodes {
        if !tree.is_valid(node) || !tree.is_inside_tree(node) {
            continue;
        }
        let Some(mut ik) = tree.component::<IkChains>(node).map(|c| c.clone()) else {
            continue;
        };
        if !ik.enabled {
            continue;
        }
        let mut changed = false;
        for (index, desc) in ik.chains.clone().iter().enumerate() {
            changed = true;
            if ik.resolved[index]
                .as_ref()
                .is_some_and(|r| r.joints.iter().any(|&j| !tree.is_valid(j)))
            {
                ik.resolved[index] = None;
            }
            if ik.resolved[index].is_none() {
                match resolve(tree, node, desc) {
                    Ok(r) => ik.resolved[index] = Some(r),
                    Err(err) => {
                        if !ik.errors.contains(&err) {
                            tracing::warn!("{}: {err}", tree.name(node).as_deref().unwrap_or("?"));
                            ik.errors.push(err);
                        }
                        changed = true;
                        continue;
                    }
                }
                changed = true;
            }
            let chain = ik.resolved[index].as_mut().expect("resolved above");
            // Back to the animated pose where nothing moved the joints since last frame.
            for (i, &joint) in chain.joints.iter().enumerate() {
                let now = tree.rotation(joint).unwrap_or(Quat::IDENTITY);
                let untouched = chain
                    .written
                    .get(i)
                    .is_some_and(|w| w.abs_diff_eq(now, 1e-6));
                if untouched {
                    let _ = tree.set_rotation(joint, chain.animated[i]);
                } else {
                    chain.animated[i] = now;
                }
            }
            solve_one(tree, node, desc, chain, debug);
            chain.written = chain
                .joints
                .iter()
                .map(|&j| tree.rotation(j).unwrap_or(Quat::IDENTITY))
                .collect();
        }
        if changed {
            let _ = tree.insert_component(node, ik);
        }
    }
}

fn solve_one(tree: &mut SceneTree, node: NodeHandle, desc: &IkDesc, chain: &Resolved, debug: bool) {
    if desc.weight <= 0.0 || chain.joints.len() < 2 {
        return;
    }
    let mut worlds: Vec<Affine3A> = chain
        .joints
        .iter()
        .map(|&j| crate::procgen::world_of(tree, j))
        .collect();
    let positions: Vec<Vec3> = worlds.iter().map(|w| Vec3::from(w.translation)).collect();
    let tip_now = *positions.last().expect("two joints at least");
    let target_node = desc.target.as_deref().and_then(|p| tree.get_node(node, p));
    let mut tip_rotation = None;
    let mut target = target_node.map(|t| {
        let world = crate::procgen::world_of(tree, t);
        if desc.align {
            tip_rotation = Some(rotation_of(&world));
        }
        Vec3::from(world.translation)
    });
    if desc.ground {
        target = ground_target(tree, node, target.unwrap_or(tip_now), chain.tip_height).or(target);
    }
    let Some(target) = target else {
        return;
    };
    let pole = desc
        .pole
        .as_deref()
        .and_then(|p| tree.get_node(node, p))
        .map(|p| Vec3::from(crate::procgen::world_of(tree, p).translation));
    let solver = desc.solver.unwrap_or(if positions.len() == 3 {
        Solver::TwoBone
    } else {
        Solver::Fabrik
    });
    let solved = match solver {
        Solver::TwoBone if positions.len() == 3 => {
            solve_two_bone([positions[0], positions[1], positions[2]], target, pole).to_vec()
        }
        _ => {
            let mut p = positions.clone();
            solve_fabrik(&mut p, target, 16, 1e-3);
            p
        }
    };
    pose_chain(tree, chain, &mut worlds, &solved, desc.weight, tip_rotation);
    if debug {
        if let Some(mut draw) = tree.world().resource_mut::<DebugDraw>() {
            let mut brush = draw.on_top();
            for w in solved.windows(2) {
                brush.line(w[0], w[1], Color::rgb(1.0, 0.3, 0.25));
            }
            brush.cross(target, 0.08, Color::rgb(1.0, 0.85, 0.2));
            if let Some(pole) = pole {
                brush.line(solved[1], pole, Color::rgb(0.3, 0.6, 1.0));
                brush.cross(pole, 0.06, Color::rgb(0.3, 0.6, 1.0));
            }
        }
    }
}

/// How far above and below its animated position a foot looks for the ground (metres).
const GROUND_ABOVE: f32 = 0.5;
const GROUND_BELOW: f32 = 0.7;

/// Where a tip at `at` stands on the ground below it, as high over it as at rest.
fn ground_target(tree: &SceneTree, node: NodeHandle, at: Vec3, tip_height: f32) -> Option<Vec3> {
    let physics = tree.world().resource::<PhysicsWorld>()?;
    // The character's own bodies are not ground.
    let mut exclude: Vec<NodeHandle> =
        std::iter::successors(Some(node), |&n| tree.parent(n)).collect();
    exclude.extend(tree.subtree(node));
    let options = QueryOptions {
        exclude,
        ..QueryOptions::new()
    };
    let base = crate::procgen::world_of(tree, node).translation.y;
    // Only the foot's lift above its rest height is animation (a step), the rest is the floor.
    let lift = (at.y - base - tip_height).max(0.0);
    let from = Vec3::new(at.x, at.y + GROUND_ABOVE, at.z);
    let hit = physics.cast_ray(from, Vec3::NEG_Y, GROUND_ABOVE + GROUND_BELOW, &options)?;
    Some(Vec3::new(
        at.x,
        hit.position.y + tip_height.max(0.0) + lift,
        at.z,
    ))
}

/// Sets up `node`'s `ik` and `rig` from its description (scene loading).
pub(crate) fn configure(
    tree: &mut SceneTree,
    node: NodeHandle,
    rig: Option<&str>,
    ik: &[IkDesc],
) -> Result<(), String> {
    if let Some(path) = rig.filter(|p| !p.trim().is_empty()) {
        let bytes = tree
            .world()
            .resource::<ab_asset::AssetServer>()
            .ok_or("the AssetServer resource is missing")?
            .read(path)
            .map_err(|e| format!("rig {path}: {e}"))?;
        let desc = crate::rig::RigFile::from_toml(&String::from_utf8_lossy(&bytes))
            .map_err(|e| format!("rig {path}: {e}"))?;
        tree.insert_component(
            node,
            Rig {
                file: Some(path.to_owned()),
                desc,
            },
        )
        .map_err(|e| e.to_string())?;
    }
    if !ik.is_empty() {
        for desc in ik {
            if !(0.0..=1.0).contains(&desc.weight) {
                return Err("ik.weight is 0..1".into());
            }
            if desc.bones == 0 || desc.bones > 32 {
                return Err("ik.bones is 1..32".into());
            }
        }
        let mut chains = IkChains::new(ik.to_vec());
        // Found now when the bones are there (a model instance): their rest rotations, which
        // limits are measured from, are the model's own pose, not an animated one. Others
        // are found on the first frame.
        for (slot, desc) in chains.resolved.iter_mut().zip(ik) {
            *slot = resolve(tree, node, desc).ok();
        }
        tree.insert_component(node, chains)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_bones_reach_bend_towards_the_pole_and_keep_their_lengths() {
        let joints = [
            Vec3::ZERO,
            Vec3::new(0.0, -1.0, 0.1),
            Vec3::new(0.0, -2.0, 0.0),
        ];
        let target = Vec3::new(0.5, -1.5, 0.0);
        let [root, mid, tip] = solve_two_bone(joints, target, Some(Vec3::new(0.0, -1.0, 5.0)));
        assert!(tip.distance(target) < 1e-3);
        assert!((mid.distance(root) - joints[1].distance(joints[0])).abs() < 1e-3);
        assert!((tip.distance(mid) - joints[2].distance(joints[1])).abs() < 1e-3);
        assert!(mid.z > 0.5, "bent towards the pole: {mid}");
        // Out of reach: straight towards the target.
        let [_, mid, tip] = solve_two_bone(joints, Vec3::new(10.0, 0.0, 0.0), None);
        assert!(mid.y.abs() < 0.05 && tip.x > 1.9, "{mid} {tip}");
    }

    #[test]
    fn fabrik_reaches_with_any_number_of_bones() {
        let mut joints: Vec<Vec3> = (0..5).map(|i| Vec3::new(0.0, i as f32, 0.0)).collect();
        let target = Vec3::new(2.0, 2.0, 0.5);
        solve_fabrik(&mut joints, target, 32, 1e-4);
        assert!(joints[4].distance(target) < 1e-2);
        assert_eq!(joints[0], Vec3::ZERO, "the root stays");
        for w in joints.windows(2) {
            assert!((w[0].distance(w[1]) - 1.0).abs() < 1e-3);
        }
    }
}
