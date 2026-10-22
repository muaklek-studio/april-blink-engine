use crate::components::{Children, Transform};
use crate::handle::NodeHandle;
use ab_core::Affine3A;
use ab_ecs::World;

/// Recomputes cached global transforms of the tree below `root`.
///
/// Top-down: a node is recomputed when it is dirty or any `Node3D` ancestor was recomputed
/// this pass. A plain `Node` breaks inheritance: its `Node3D` children are positioned
/// relative to the world origin, as in Godot.
pub fn propagate_transforms(world: &World, root: NodeHandle) {
    let Some(mut transforms) = world.storage_mut::<Transform>() else {
        return;
    };
    let Some(children) = world.storage::<Children>() else {
        return;
    };

    // (node, parent global, parent changed this pass)
    let mut stack: Vec<(NodeHandle, Affine3A, bool)> = vec![(root, Affine3A::IDENTITY, false)];
    while let Some((node, parent_global, parent_changed)) = stack.pop() {
        let (global, changed) = match transforms.get_mut(node.entity()) {
            Some(t) => {
                let changed = parent_changed || t.dirty;
                if changed {
                    t.global = parent_global * t.local.to_affine();
                    t.dirty = false;
                }
                (t.global, changed)
            }
            None => (Affine3A::IDENTITY, false),
        };
        if let Some(Children(kids)) = children.get(node.entity()) {
            stack.extend(kids.iter().rev().map(|&kid| (kid, global, changed)));
        }
    }
}
