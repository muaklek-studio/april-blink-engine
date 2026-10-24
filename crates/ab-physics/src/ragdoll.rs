//! Ragdolls: rigid bodies joined by [`Joint3D`]s that either follow an animated skeleton
//! (inactive: kinematic hit boxes) or take over and drive it (active: limp physics).
//!
//! A [`Ragdoll`] node owns its parts: every rigid or kinematic [`PhysicsBody3D`] below it.
//! Parts with a [`RagdollBone`] are tied to a bone node of a skinned model. Build one from a
//! skeleton with [`PhysicsNodes::create_skeleton_ragdoll`](crate::PhysicsNodes), or by hand
//! (bodies + joints under a `Ragdoll` node, as scene files do).

use crate::components::{BodyKind, PhysicsBody3D};
use ab_core::{Affine3A, Transform3D, Vec3};
use ab_ecs::{Entity, World};
use ab_scene::{Children, NodeHandle, NodeInfo, Parent, Transform};

/// Root of a ragdoll. `active` switches every part between kinematic (following its bone,
/// or frozen in place) and rigid (simulated).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ragdoll {
    pub active: bool,
}

impl Ragdoll {
    /// Limp from the start.
    pub fn active() -> Self {
        Self { active: true }
    }

    /// Kinematic until activated (follows the animation, works as hit boxes).
    pub fn inactive() -> Self {
        Self { active: false }
    }
}

/// On a ragdoll part: the bone node it moves when active and follows when inactive.
/// `offset` is the bone's transform in the part's frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RagdollBone {
    pub bone: NodeHandle,
    pub offset: Affine3A,
}

/// Settings of [`PhysicsNodes::create_skeleton_ragdoll`](crate::PhysicsNodes).
#[derive(Debug, Clone, PartialEq)]
pub struct SkeletonRagdoll {
    /// The bone nodes (a skin's joints). Bones shorter than `min_bone_length` get no body of
    /// their own and follow their parent part.
    pub bones: Vec<NodeHandle>,
    /// Capsule radius as a fraction of the bone's length.
    pub radius: f32,
    /// Smallest and largest capsule radius in metres.
    pub radius_range: [f32; 2],
    /// Total mass in kg, spread by volume.
    pub total_mass: f32,
    pub min_bone_length: f32,
    /// Joint limits in degrees.
    pub swing_degrees: f32,
    pub twist_degrees: f32,
    pub collision_layer: u32,
    pub collision_mask: u32,
    /// Start limp instead of following the animation.
    pub active: bool,
}

impl SkeletonRagdoll {
    pub fn new(bones: Vec<NodeHandle>) -> Self {
        Self {
            bones,
            radius: 0.25,
            radius_range: [0.03, 0.3],
            total_mass: 60.0,
            min_bone_length: 0.04,
            swing_degrees: 50.0,
            twist_degrees: 30.0,
            collision_layer: 1,
            collision_mask: u32::MAX,
            active: false,
        }
    }
}

/// Every node below `root`, parents first.
pub(crate) fn descendants(world: &World, root: Entity) -> Vec<Entity> {
    let children = |entity: Entity| -> Vec<Entity> {
        world
            .get::<Children>(entity)
            .map(|c| c.0.iter().rev().map(|n| n.entity()).collect())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    let mut stack = children(root);
    while let Some(entity) = stack.pop() {
        out.push(entity);
        stack.extend(children(entity));
    }
    out
}

/// Before the step: applies each ragdoll's mode to its parts and moves inactive parts to
/// their bones. Returns whether any part was moved (their children's globals are then stale).
pub(crate) fn before_step(world: &World) -> bool {
    let ragdolls: Vec<(Entity, bool)> =
        match (world.storage::<Ragdoll>(), world.storage::<NodeInfo>()) {
            (Some(ragdolls), Some(infos)) => ragdolls
                .iter()
                .filter(|(e, _)| infos.get(*e).is_some_and(|i| i.in_tree))
                .map(|(e, r)| (e, r.active))
                .collect(),
            _ => return false,
        };
    let mut followed = false;
    for (root, active) in ragdolls {
        let wanted = if active {
            BodyKind::Rigid
        } else {
            BodyKind::Kinematic
        };
        for part in descendants(world, root) {
            let Some(mut body) = world.get_mut::<PhysicsBody3D>(part) else {
                continue;
            };
            if matches!(body.kind, BodyKind::Rigid | BodyKind::Kinematic) && body.kind != wanted {
                body.kind = wanted;
            }
            drop(body);
            if !active {
                followed |= follow_bone(world, part);
            }
        }
    }
    followed
}

/// After the step: active parts move their bones, parents first.
pub(crate) fn after_step(world: &World) {
    let roots: Vec<Entity> = match (world.storage::<Ragdoll>(), world.storage::<NodeInfo>()) {
        (Some(ragdolls), Some(infos)) => ragdolls
            .iter()
            .filter(|(e, r)| r.active && infos.get(*e).is_some_and(|i| i.in_tree))
            .map(|(e, _)| e)
            .collect(),
        _ => return,
    };
    for root in roots {
        let mut parts: Vec<(usize, Entity, RagdollBone)> = descendants(world, root)
            .into_iter()
            .filter_map(|part| {
                let bone = *world.get::<RagdollBone>(part)?;
                Some((depth(world, bone.bone.entity()), part, bone))
            })
            .collect();
        parts.sort_by_key(|(depth, ..)| *depth);
        for (_, part, bone) in parts {
            let Some(part_global) = world.get::<Transform>(part).map(|t| t.global) else {
                continue;
            };
            set_global(world, bone.bone.entity(), part_global * bone.offset);
            // Bones below without a body of their own (a spine bone between belly and
            // chest, a clavicle) must move now: deeper parts set their bones relative to
            // them.
            refresh_descendants(world, bone.bone.entity());
        }
    }
}

/// Puts an inactive part where its bone is. Returns whether it has a bone to follow.
fn follow_bone(world: &World, part: Entity) -> bool {
    let Some(bone) = world.get::<RagdollBone>(part).map(|b| *b) else {
        return false;
    };
    let Some(bone_global) = world.get::<Transform>(bone.bone.entity()).map(|t| t.global) else {
        return false;
    };
    set_global(world, part, bone_global * bone.offset.inverse());
    true
}

/// Recomputes the globals of every node below `root` from its current global, as transform
/// propagation would (a parent without a `Transform` breaks inheritance).
fn refresh_descendants(world: &World, root: Entity) {
    for entity in descendants(world, root) {
        let parent_global = world
            .get::<Parent>(entity)
            .and_then(|p| world.get::<Transform>(p.0.entity()).map(|t| t.global))
            .unwrap_or(Affine3A::IDENTITY);
        if let Some(mut transform) = world.get_mut::<Transform>(entity) {
            transform.global = parent_global * transform.local.to_affine();
        }
    }
}

fn depth(world: &World, mut entity: Entity) -> usize {
    let mut depth = 0;
    while let Some(parent) = world.get::<Parent>(entity) {
        entity = parent.0.entity();
        depth += 1;
    }
    depth
}

/// Sets a node's global transform directly in its `Transform` (local derived from the
/// parent's current global).
pub(crate) fn set_global(world: &World, entity: Entity, global: Affine3A) {
    let parent_global = world
        .get::<Parent>(entity)
        .and_then(|p| world.get::<Transform>(p.0.entity()).map(|t| t.global))
        .unwrap_or(Affine3A::IDENTITY);
    if let Some(mut transform) = world.get_mut::<Transform>(entity) {
        transform.local = Transform3D::from_affine(&(parent_global.inverse() * global));
        transform.global = parent_global * transform.local.to_affine();
        transform.dirty = true;
    }
}

/// One part planned from a bone: the capsule from the bone's head to its tail.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BonePart {
    pub bone: NodeHandle,
    pub head: Vec3,
    pub tail: Vec3,
}

impl BonePart {
    pub fn length(&self) -> f32 {
        self.head.distance(self.tail)
    }
}

/// Plans one capsule per bone: head at the bone, tail at the average of its child bones, or
/// (for end bones) continuing the bone's own +Y axis by `fallback` metres.
pub(crate) fn plan_bones(
    globals: &[(NodeHandle, Affine3A)],
    parents: &[Option<usize>],
    fallback: f32,
) -> Vec<BonePart> {
    globals
        .iter()
        .enumerate()
        .map(|(i, (bone, global))| {
            let head: Vec3 = global.translation.into();
            let children: Vec<Vec3> = parents
                .iter()
                .enumerate()
                .filter(|(_, p)| **p == Some(i))
                .map(|(c, _)| globals[c].1.translation.into())
                .collect();
            let tail = if children.is_empty() {
                let up = Vec3::from(global.matrix3.y_axis).normalize_or(Vec3::Y);
                head + up * fallback
            } else {
                children.iter().copied().sum::<Vec3>() / children.len() as f32
            };
            BonePart {
                bone: *bone,
                head,
                tail,
            }
        })
        .collect()
}
