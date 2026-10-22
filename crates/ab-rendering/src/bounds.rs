//! World-space bounding boxes of what is drawn, skinned meshes included.
//!
//! A static mesh's box is its mesh-space box moved by the node's transform. A skinned mesh
//! bends with its joints, so its bind-pose box says little once it is posed; instead each
//! joint keeps the box of the vertices it moves (in mesh space), and the posed mesh lies inside
//! the union of those boxes moved by the joints' skinning matrices: every skinned vertex is a
//! weighted average of its joints' transforms of it, and each of those lies in that joint's
//! moved box.

use crate::assets::RenderAssets;
use crate::components::{MeshInstance3D, Skin};
use crate::mesh::{Aabb, Mesh};
use ab_core::{Affine3A, Mat4};
use ab_ecs::{Entity, SparseSet, World};
use ab_scene::{NodeHandle, NodeInfo, SceneTree, Transform};

/// Per joint of a skinned mesh (the skin's joint index), the mesh-space box of the vertices it
/// moves; `None` for joints that move none. Empty for a mesh without skin.
pub(crate) fn joint_bounds(mesh: &Mesh) -> Vec<Option<Aabb>> {
    let Some(skin) = &mesh.skin else {
        return Vec::new();
    };
    let count = skin
        .iter()
        .flat_map(|w| w.joints)
        .max()
        .map_or(0, |j| usize::from(j) + 1);
    let mut bounds: Vec<Option<Aabb>> = vec![None; count];
    for (vertex, weights) in mesh.vertices.iter().zip(skin) {
        let p = vertex.position.into();
        for (&joint, &weight) in weights.joints.iter().zip(&weights.weights) {
            if weight <= 0.0 {
                continue;
            }
            let point = Aabb { min: p, max: p };
            let slot = &mut bounds[usize::from(joint)];
            *slot = Some(slot.map_or(point, |b| b.union(&point)));
        }
    }
    bounds
}

/// World bounds of a skinned mesh posed by `palette` (`joint global × inverse bind`, one per
/// joint of the skin).
pub fn posed_bounds(joint_bounds: &[Option<Aabb>], palette: &[Mat4]) -> Option<Aabb> {
    joint_bounds
        .iter()
        .zip(palette)
        .filter_map(|(bounds, matrix)| bounds.map(|b| b.transformed(&Affine3A::from_mat4(*matrix))))
        .reduce(|a, b| a.union(&b))
}

/// What [`for_each_mesh_bounds`] reports for one mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshBounds {
    pub entity: Entity,
    pub bounds: Aabb,
    pub skinned: bool,
}

/// World bounds of the mesh on `entity` as it is drawn now (posed when skinned).
pub fn entity_mesh_bounds(world: &World, entity: Entity) -> Option<MeshBounds> {
    let meshes = world.storage::<MeshInstance3D>()?;
    let transforms = world.storage::<Transform>()?;
    let assets = world.resource::<RenderAssets>()?;
    let skins = world.storage::<Skin>();
    let instance = meshes.get(entity)?;
    mesh_bounds_in(
        &assets,
        &transforms,
        skins.as_ref().and_then(|s| s.get(entity)),
        instance,
        entity,
    )
}

/// Calls `f` with the bounds of every mesh in the tree whose node is visible (its own flag;
/// hidden parents are not checked).
pub fn for_each_mesh_bounds(world: &World, mut f: impl FnMut(MeshBounds)) {
    let (Some(meshes), Some(transforms), Some(assets), Some(infos)) = (
        world.storage::<MeshInstance3D>(),
        world.storage::<Transform>(),
        world.resource::<RenderAssets>(),
        world.storage::<NodeInfo>(),
    ) else {
        return;
    };
    let skins = world.storage::<Skin>();
    for (entity, instance) in meshes.iter() {
        if !infos.get(entity).is_some_and(|i| i.in_tree && i.visible) {
            continue;
        }
        let skin = skins.as_ref().and_then(|s| s.get(entity));
        if let Some(bounds) = mesh_bounds_in(&assets, &transforms, skin, instance, entity) {
            f(bounds);
        }
    }
}

fn mesh_bounds_in(
    assets: &RenderAssets,
    transforms: &SparseSet<Transform>,
    skin: Option<&Skin>,
    instance: &MeshInstance3D,
    entity: Entity,
) -> Option<MeshBounds> {
    if let Some(skin) = skin {
        let joints = assets.mesh_joint_bounds(instance.mesh);
        if !joints.is_empty() {
            let palette: Vec<Mat4> = skin
                .joints
                .iter()
                .enumerate()
                .map(|(i, joint)| {
                    let global = transforms
                        .get(joint.entity())
                        .map_or(Mat4::IDENTITY, |t| Mat4::from(t.global));
                    global
                        * skin
                            .inverse_bind_matrices
                            .get(i)
                            .copied()
                            .unwrap_or(Mat4::IDENTITY)
                })
                .collect();
            return posed_bounds(joints, &palette).map(|bounds| MeshBounds {
                entity,
                bounds,
                skinned: true,
            });
        }
    }
    let global = transforms.get(entity)?.global;
    Some(MeshBounds {
        entity,
        bounds: assets.mesh_aabb(instance.mesh)?.transformed(&global),
        skinned: false,
    })
}

/// Bounding boxes of nodes, on [`SceneTree`].
pub trait Bounds {
    /// World bounds of the mesh on `node` as it is drawn now (posed when skinned); `None`
    /// when the node has no mesh.
    fn mesh_bounds(&self, node: NodeHandle) -> Option<Aabb>;

    /// World bounds of every mesh in `node`'s subtree, hidden ones included.
    fn subtree_bounds(&self, node: NodeHandle) -> Option<Aabb>;
}

impl Bounds for SceneTree {
    fn mesh_bounds(&self, node: NodeHandle) -> Option<Aabb> {
        entity_mesh_bounds(self.world(), node.entity()).map(|b| b.bounds)
    }

    fn subtree_bounds(&self, node: NodeHandle) -> Option<Aabb> {
        self.subtree(node)
            .into_iter()
            .filter_map(|n| self.mesh_bounds(n))
            .reduce(|a, b| a.union(&b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::Material;
    use crate::mesh::SkinWeights;
    use crate::nodes::RenderNodes;
    use ab_core::{Transform3D, Vec3};

    /// A 2 m cube whose bottom half follows joint 0 and top half joint 1.
    fn two_joint_cube() -> Mesh {
        let mut mesh = Mesh::cube(2.0);
        let skin = mesh
            .vertices
            .iter()
            .map(|v| SkinWeights::single(u16::from(v.position[1] > 0.0)))
            .collect();
        mesh.skin = Some(skin);
        mesh
    }

    #[test]
    fn joint_bounds_cover_the_vertices_of_each_joint() {
        let bounds = joint_bounds(&two_joint_cube());
        assert_eq!(bounds.len(), 2);
        let (bottom, top) = (bounds[0].unwrap(), bounds[1].unwrap());
        assert_eq!(bottom.max.y, -1.0);
        assert_eq!(top.min.y, 1.0);
        assert!(joint_bounds(&Mesh::cube(2.0)).is_empty());
    }

    #[test]
    fn posed_bounds_follow_the_joints() {
        let bounds = joint_bounds(&two_joint_cube());
        let lifted = Mat4::from_translation(Vec3::new(0.0, 5.0, 0.0));
        let posed = posed_bounds(&bounds, &[Mat4::IDENTITY, lifted]).unwrap();
        assert_eq!(posed.min.y, -1.0);
        assert_eq!(posed.max.y, 6.0, "the top joint carries its vertices up");
    }

    #[test]
    fn tree_bounds_for_static_and_skinned_meshes() {
        let mut tree = SceneTree::new();
        let mut assets = RenderAssets::new();
        let cube = assets.add_mesh(Mesh::cube(2.0)).unwrap();
        let skinned = assets.add_mesh(two_joint_cube()).unwrap();
        let material = assets.add_material(Material::default());
        tree.world_mut().insert_resource(assets);
        let root = tree.root();

        let box_node = tree.create_mesh_instance3d("Box", MeshInstance3D::new(cube, material));
        tree.add_child(root, box_node).unwrap();
        tree.set_transform(
            box_node,
            Transform3D::from_translation(Vec3::new(10.0, 0.0, 0.0)),
        )
        .unwrap();

        let bottom = tree.create_node3d("Bottom");
        let top = tree.create_node3d("Top");
        tree.add_child(root, bottom).unwrap();
        tree.add_child(root, top).unwrap();
        tree.set_transform(top, Transform3D::from_translation(Vec3::new(0.0, 3.0, 0.0)))
            .unwrap();
        let body = tree.create_mesh_instance3d("Body", MeshInstance3D::new(skinned, material));
        tree.add_child(root, body).unwrap();
        let skin = Skin {
            joints: vec![bottom, top],
            inverse_bind_matrices: vec![Mat4::IDENTITY; 2],
        };
        tree.insert_component(body, skin).unwrap();
        tree.update_transforms();

        let b = tree.mesh_bounds(box_node).unwrap();
        assert_eq!((b.min.x, b.max.x), (9.0, 11.0));
        let s = tree.mesh_bounds(body).unwrap();
        assert_eq!((s.min.y, s.max.y), (-1.0, 4.0));
        let all = tree.subtree_bounds(root).unwrap();
        assert_eq!((all.min.x, all.max.x, all.max.y), (-1.0, 11.0, 4.0));
        assert_eq!(tree.mesh_bounds(top), None);

        let mut seen = Vec::new();
        for_each_mesh_bounds(tree.world(), |b| seen.push((b.entity, b.skinned)));
        seen.sort_by_key(|(e, _)| *e);
        let mut expected = vec![(box_node.entity(), false), (body.entity(), true)];
        expected.sort_by_key(|(e, _)| *e);
        assert_eq!(seen, expected);
        tree.set_visible(box_node, false).unwrap();
        let mut count = 0;
        for_each_mesh_bounds(tree.world(), |_| count += 1);
        assert_eq!(count, 1, "hidden meshes are skipped");
    }
}
