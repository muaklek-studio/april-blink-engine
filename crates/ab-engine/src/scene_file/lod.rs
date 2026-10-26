//! Levels of detail in scene files (see [`ab_rendering::MeshLod`]).
//!
//! * `lod = { levels = [[0.3, 0.5], [0.1, 0.2]], cull = 0.01 }` on any node: every mesh in
//!   it gets simpler versions, here half its triangles below 30% of the screen's height and
//!   a fifth below 10%, and is not drawn below 1%. The meshes are made on load (and shared).
//! * An `HLOD` node, `hlod = { distance = 80.0, detail = 0.3 }`: its meshes are merged, one
//!   mesh per material, simplified to `detail`, drawn instead of them beyond `distance`
//!   metres. The merged meshes are made on load (marked [`Generated`]), never saved. A group
//!   is for things that stay put: the switch distance is from where it was built.

use super::format::NodeDesc;
use super::Generated;
use ab_core::Affine3A;
use ab_rendering::{
    merge, simplify, Aabb, HlodGroup, HlodProxy, LodLevel, MaterialHandle, MeshHandle,
    MeshInstance3D, MeshLod, RenderAssets, RenderNodes as _, Skin, MAX_LOD_LEVELS,
};
use ab_scene::{NodeHandle, SceneTree};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The class of HLOD group nodes.
pub const HLOD: &str = "HLOD";

/// `lod = { levels = [[screen size, detail], ...], cull = ... }` (see the module docs).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LodDesc {
    /// `[screen size, detail]`: below the size (fraction of the screen's height), a version
    /// with `detail` (0..1) of the triangles.
    pub levels: Vec<[f32; 2]>,
    /// Not drawn below this screen size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cull: Option<f32>,
}

impl LodDesc {
    /// What cannot work, named.
    pub fn check(&self) -> Result<(), String> {
        if self.levels.len() > MAX_LOD_LEVELS {
            return Err(format!("lod: at most {MAX_LOD_LEVELS} levels"));
        }
        let mut last = (f32::INFINITY, 1.0);
        let mut sorted = self.levels.clone();
        sorted.sort_by(|a, b| b[0].total_cmp(&a[0]));
        for [size, detail] in sorted {
            if !(size > 0.0 && size.is_finite()) {
                return Err("lod: screen sizes are above 0".into());
            }
            if !(0.0..=1.0).contains(&detail) || detail == 0.0 {
                return Err("lod: detail is above 0 and at most 1".into());
            }
            if detail > last.1 {
                return Err(format!(
                    "lod: the level below {size} has more detail ({detail}) than the one above it ({})",
                    last.1
                ));
            }
            last = (size, detail);
        }
        if let Some(cull) = self.cull {
            if !(0.0..1.0).contains(&cull) {
                return Err("lod.cull is a screen size from 0 to 1".into());
            }
            if let Some(smallest) = self.levels.iter().map(|l| l[0]).reduce(f32::min) {
                if cull >= smallest {
                    return Err(format!(
                        "lod.cull ({cull}) hides the mesh before its last level ({smallest})"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// `hlod = { distance = ..., detail = ... }` of an `HLOD` node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HlodDesc {
    /// Metres from the camera to the group's centre where the merged meshes take over.
    pub distance: f32,
    /// 0..1: how much of the triangles the merged meshes keep.
    pub detail: f32,
}

impl Default for HlodDesc {
    fn default() -> Self {
        Self {
            distance: 80.0,
            detail: 0.3,
        }
    }
}

/// The `lod` a node was given.
#[derive(Debug, Clone, PartialEq)]
pub struct LodNode(pub LodDesc);

/// The `hlod` an `HLOD` node was given.
#[derive(Debug, Clone, PartialEq)]
pub struct HlodNode(pub HlodDesc);

/// Simplified meshes made so far, by mesh and detail (shared by every node using them).
#[derive(Debug, Default)]
struct LodMeshes {
    assets: u64,
    meshes: HashMap<(MeshHandle, u32), MeshHandle>,
}

fn simplified(tree: &SceneTree, mesh: MeshHandle, detail: f32) -> Option<MeshHandle> {
    let world = tree.world();
    let mut assets = world.resource_mut::<RenderAssets>()?;
    let mut cache = world.resource_mut::<LodMeshes>()?;
    if cache.assets != assets.id() {
        *cache = LodMeshes {
            assets: assets.id(),
            ..Default::default()
        };
    }
    let key = (mesh, detail.to_bits());
    if let Some(&made) = cache.meshes.get(&key) {
        return Some(made);
    }
    let simple = simplify(assets.mesh(mesh)?, detail);
    let made = assets.add_mesh(simple).ok()?;
    cache.meshes.insert(key, made);
    Some(made)
}

pub(super) fn configure(
    tree: &mut SceneTree,
    node: NodeHandle,
    desc: &NodeDesc,
) -> Result<(), String> {
    if let Some(lod) = &desc.lod {
        lod.check()?;
        tree.insert_component(node, LodNode(lod.clone()))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(super) fn create(
    tree: &mut SceneTree,
    name: &str,
    desc: &NodeDesc,
) -> Result<NodeHandle, String> {
    let h = desc.hlod.clone().unwrap_or_default();
    if !(h.distance > 0.0 && h.distance.is_finite()) {
        return Err("hlod.distance is above 0".into());
    }
    if !(h.detail > 0.0 && h.detail <= 1.0) {
        return Err("hlod.detail is above 0 and at most 1".into());
    }
    let node = tree.create_node3d(name);
    tree.set_class_name(node, HLOD).map_err(|e| e.to_string())?;
    tree.insert_component(node, HlodNode(h))
        .map_err(|e| e.to_string())?;
    Ok(node)
}

pub(super) fn describe(tree: &SceneTree, node: NodeHandle, desc: &mut NodeDesc) {
    desc.lod = tree.component::<LodNode>(node).map(|l| l.0.clone());
    desc.hlod = tree.component::<HlodNode>(node).map(|h| h.0.clone());
}

/// Gives the meshes of every `lod` node under `root` their levels, and builds every HLOD
/// group's merged meshes. Scene loading calls it once the scene exists; call it again
/// after changing them.
pub fn build_all(tree: &mut SceneTree, root: NodeHandle) {
    tree.world_mut().resource_or_insert_with(LodMeshes::default);
    let nodes = tree.subtree(root);
    for &node in &nodes {
        let Some(lod) = tree.component::<LodNode>(node).map(|l| l.0.clone()) else {
            continue;
        };
        for target in tree.subtree(node) {
            let Some(instance) = tree.component::<MeshInstance3D>(target).map(|m| *m) else {
                continue;
            };
            if tree.component::<Skin>(target).is_some()
                || tree.component::<HlodProxy>(target).is_some()
            {
                continue;
            }
            let levels = lod
                .levels
                .iter()
                .filter_map(|&[size, detail]| {
                    simplified(tree, instance.mesh, detail).map(|mesh| LodLevel {
                        screen_size: size,
                        mesh,
                    })
                })
                .collect();
            let made = MeshLod {
                levels,
                cull_below: lod.cull.unwrap_or(0.0),
            }
            .sorted();
            let _ = tree.insert_component(target, made);
        }
    }
    for node in nodes {
        if tree.component::<HlodNode>(node).is_some() {
            if let Err(err) = build_hlod(tree, node) {
                tracing::warn!("HLOD {}: {err}", tree.name(node).as_deref().unwrap_or("?"));
            }
        }
    }
}

/// (Re)builds an HLOD group's merged meshes. How many it made.
pub fn build_hlod(tree: &mut SceneTree, group: NodeHandle) -> Result<usize, String> {
    let desc = tree
        .component::<HlodNode>(group)
        .map(|h| h.0.clone())
        .ok_or("not an HLOD group")?;
    for child in tree.children(group) {
        if tree.component::<HlodProxy>(child).is_some() {
            tree.free_immediately(child);
        }
    }
    tree.update_transforms();
    let to_local = tree
        .global_transform(group)
        .unwrap_or(Affine3A::IDENTITY)
        .inverse();
    // The group's meshes by material, placed in its own space.
    let mut by_material: Vec<(MaterialHandle, Vec<(MeshHandle, Affine3A)>)> = Vec::new();
    for node in tree.subtree(group) {
        let Some(instance) = tree.component::<MeshInstance3D>(node).map(|m| *m) else {
            continue;
        };
        if tree.component::<Skin>(node).is_some() || tree.component::<HlodProxy>(node).is_some() {
            continue;
        }
        let placed = to_local * tree.global_transform(node).unwrap_or(Affine3A::IDENTITY);
        match by_material
            .iter_mut()
            .find(|(m, _)| *m == instance.material)
        {
            Some((_, parts)) => parts.push((instance.mesh, placed)),
            None => by_material.push((instance.material, vec![(instance.mesh, placed)])),
        }
    }
    let mut made = Vec::new();
    let mut bounds: Option<Aabb> = None;
    {
        let mut assets = tree
            .world()
            .resource_mut::<RenderAssets>()
            .ok_or("the RenderAssets resource is missing")?;
        for (material, parts) in &by_material {
            let meshes: Vec<_> = parts
                .iter()
                .filter_map(|(mesh, at)| assets.mesh(*mesh).map(|m| (m, *at)))
                .collect();
            let merged = simplify(&merge(&meshes), desc.detail);
            if merged.indices.is_empty() {
                continue;
            }
            if let Some(b) = merged.aabb() {
                bounds = Some(bounds.map_or(b, |a| a.union(&b)));
            }
            let mesh = assets.add_mesh(merged).map_err(|e| e.to_string())?;
            made.push((mesh, *material));
        }
    }
    for (i, (mesh, material)) in made.iter().enumerate() {
        let proxy = tree
            .create_mesh_instance3d(&format!("Proxy{i}"), MeshInstance3D::new(*mesh, *material));
        tree.insert_component(proxy, HlodProxy)
            .map_err(|e| e.to_string())?;
        tree.insert_component(proxy, Generated)
            .map_err(|e| e.to_string())?;
        tree.add_child(group, proxy).map_err(|e| e.to_string())?;
    }
    let center = tree
        .global_transform(group)
        .unwrap_or(Affine3A::IDENTITY)
        .transform_point3(bounds.map_or(ab_core::Vec3::ZERO, |b| b.center()));
    tree.insert_component(
        group,
        HlodGroup {
            distance: desc.distance,
            center,
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(made.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_descriptions_are_checked() {
        let ok = LodDesc {
            levels: vec![[0.3, 0.5], [0.1, 0.2]],
            cull: Some(0.01),
        };
        ok.check().unwrap();
        let more_detail_further = LodDesc {
            levels: vec![[0.3, 0.2], [0.1, 0.5]],
            cull: None,
        };
        assert!(more_detail_further
            .check()
            .unwrap_err()
            .contains("more detail"));
        let culls_early = LodDesc {
            levels: vec![[0.3, 0.5]],
            cull: Some(0.4),
        };
        assert!(culls_early
            .check()
            .unwrap_err()
            .contains("before its last level"));
        let zero = LodDesc {
            levels: vec![[0.3, 0.0]],
            cull: None,
        };
        assert!(zero.check().is_err());
    }
}
