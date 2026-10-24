//! `Generator` nodes: a procedural generator and its parameters (see [`crate::procgen`]).
//! What it makes is built under the node, marked [`Generated`]: rebuilt on load, not saved.

use super::format::{GeneratorDesc, NodeDesc};
use super::{instantiate_scene, material_handle, Generated};
use crate::procgen::{self, Context, Output};
use ab_core::Vec3;
use ab_physics::{BodyKind, PhysicsBody3D, PhysicsNodes as _, Shape};
use ab_rendering::{MeshInstance3D, RenderAssets, RenderNodes as _};
use ab_scene::{NodeHandle, SceneTree};

/// The class of generator nodes.
pub const GENERATOR: &str = "Generator";

/// A generator node's description; built or not.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratorNode {
    pub desc: GeneratorDesc,
    /// Why the last build failed.
    pub error: Option<String>,
}

pub(super) fn create(
    tree: &mut SceneTree,
    name: &str,
    desc: &NodeDesc,
) -> Result<NodeHandle, String> {
    let g = desc
        .generator
        .clone()
        .ok_or("Generator needs `generator = { kind = ... }`")?;
    if procgen::generator(&g.kind).is_none() {
        let kinds: Vec<&str> = procgen::GENERATORS.iter().map(|g| g.kind()).collect();
        return Err(format!(
            "unknown generator `{}` ({})",
            g.kind,
            kinds.join(", ")
        ));
    }
    let node = tree.create_node3d(name);
    tree.set_class_name(node, GENERATOR)
        .map_err(|e| e.to_string())?;
    tree.insert_component(
        node,
        GeneratorNode {
            desc: g,
            error: None,
        },
    )
    .map_err(|e| e.to_string())?;
    // Built once the whole scene exists (`build_all`): it may stand on other parts of it.
    Ok(node)
}

/// Builds every generator under `root` (and in what they make): those that look at the
/// ground last. A failure is kept on the node (and logged); the rest still build.
pub fn build_all(tree: &mut SceneTree, root: NodeHandle) {
    for stage in 0..=1 {
        // Generators can make generators (a town makes buildings): until none is left.
        for _ in 0..4 {
            let pending: Vec<NodeHandle> = tree
                .subtree(root)
                .into_iter()
                .filter(|&n| {
                    tree.component::<GeneratorNode>(n).is_some_and(|g| {
                        procgen::generator(&g.desc.kind).is_some_and(|k| k.stage() == stage)
                    }) && !tree
                        .children(n)
                        .iter()
                        .any(|&c| tree.component::<Generated>(c).is_some())
                })
                .collect();
            if pending.is_empty() {
                break;
            }
            for node in pending {
                if let Err(err) = build(tree, node) {
                    tracing::warn!(
                        "generator {}: {err}",
                        tree.path_of(node).unwrap_or_default()
                    );
                }
            }
        }
    }
}

/// Builds again the generators under `root` that look at the ground (now that `root` is
/// in the scene, after being made on its own).
pub fn rebuild_grounded(tree: &mut SceneTree, root: NodeHandle) {
    let grounded: Vec<NodeHandle> = tree
        .subtree(root)
        .into_iter()
        .filter(|&n| {
            tree.component::<GeneratorNode>(n)
                .is_some_and(|g| procgen::generator(&g.desc.kind).is_some_and(|k| k.stage() > 0))
        })
        .collect();
    for node in grounded {
        if let Err(err) = build(tree, node) {
            tracing::warn!("generator: {err}");
        }
    }
}

/// Generators building inside generators' pieces, at most (a piece that scatters itself,
/// or A → B → A, would otherwise recurse until the stack overflows).
const MAX_NESTING: u32 = 8;

thread_local! {
    /// Generators being built on this thread, one inside another.
    static NESTING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Replaces what `node` made with what it makes now.
pub fn build(tree: &mut SceneTree, node: NodeHandle) -> Result<(), String> {
    struct Nested;
    impl Drop for Nested {
        fn drop(&mut self) {
            NESTING.with(|n| n.set(n.get().saturating_sub(1)));
        }
    }
    let depth = NESTING.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let _nested = Nested;
    if depth > MAX_NESTING {
        let err =
            format!("generators nested deeper than {MAX_NESTING} (a piece generating itself?)");
        if let Some(mut state) = tree.component_mut::<GeneratorNode>(node) {
            state.error = Some(err.clone());
        }
        return Err(err);
    }
    for child in tree.children(node) {
        if tree.component::<Generated>(child).is_some() {
            tree.free_immediately(child);
        }
    }
    let Some(g) = tree
        .component::<GeneratorNode>(node)
        .map(|g| g.desc.clone())
    else {
        return Ok(());
    };
    let generator =
        procgen::generator(&g.kind).ok_or_else(|| format!("unknown generator `{}`", g.kind))?;
    let outputs = {
        let mut cx = Context::new(tree, node, g.seed, &g.params, generator.params());
        generator.generate(&mut cx)
    };
    let result = outputs.and_then(|outputs| place(tree, node, outputs));
    if let Some(mut state) = tree.component_mut::<GeneratorNode>(node) {
        state.error = result.as_ref().err().cloned();
    }
    result
}

fn place(tree: &mut SceneTree, node: NodeHandle, outputs: Vec<Output>) -> Result<(), String> {
    let built = tree.create_node3d("Built");
    tree.insert_component(built, Generated)
        .map_err(|e| e.to_string())?;
    tree.add_child(node, built).map_err(|e| e.to_string())?;
    for output in outputs {
        match output {
            Output::Scene { scene, at } => {
                let made =
                    instantiate_scene(tree, &scene, "generator").map_err(|e| e.to_string())?;
                let _ = tree.set_transform(made, at);
                tree.add_child(built, made).map_err(|e| e.to_string())?;
            }
            Output::Mesh {
                name,
                mut mesh,
                material,
                collide,
            } => {
                let shape = collide.then(|| {
                    Shape::trimesh(
                        mesh.vertices
                            .iter()
                            .map(|v| Vec3::from(v.position))
                            .collect::<Vec<_>>(),
                        mesh.indices.as_chunks::<3>().0.to_vec(),
                    )
                });
                mesh.compute_tangents();
                let handle = tree
                    .world()
                    .resource_mut::<RenderAssets>()
                    .ok_or("the RenderAssets resource is missing")?
                    .add_mesh(mesh)
                    .map_err(|e| e.to_string())?;
                let material = material_handle(tree, &material)?;
                let holder = match shape {
                    Some(shape) => {
                        let body =
                            tree.create_physics_body3d(&name, PhysicsBody3D::new(BodyKind::Static));
                        let collider = tree.create_collision_shape3d("Shape", shape);
                        tree.add_child(body, collider).map_err(|e| e.to_string())?;
                        body
                    }
                    None => tree.create_node3d(&name),
                };
                let visual =
                    tree.create_mesh_instance3d("Mesh", MeshInstance3D::new(handle, material));
                tree.add_child(holder, visual).map_err(|e| e.to_string())?;
                tree.add_child(built, holder).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

pub(super) fn describe(tree: &SceneTree, node: NodeHandle, desc: &mut NodeDesc) {
    desc.generator = tree
        .component::<GeneratorNode>(node)
        .map(|g| g.desc.clone());
}
