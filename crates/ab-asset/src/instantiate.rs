use crate::error::AssetError;
use crate::gltf_loader::GltfScene;
use crate::server::AssetServer;
use ab_animation::{AnimationLibrary, AnimationPlayer};
use ab_ecs::Entity;
use ab_rendering::{class, MeshInstance3D, RenderAssets, Skin};
use ab_scene::{NodeHandle, SceneTree, Transform};

/// Class name of the node that carries an imported scene's [`AnimationPlayer`].
pub const ANIMATION_PLAYER_CLASS: &str = "AnimationPlayer";

/// Builds a detached node tree from a loaded glTF scene and returns its root (a `Node3D`
/// named after the file). Attach it with `add_child`.
///
/// Every glTF node becomes a `Node3D`; a mesh with one primitive is drawn by the node
/// itself, extra primitives become `<name>_<n>` children. If the file has animations, an
/// `AnimationPlayer` child node holds a player with every clip (not playing).
pub fn instantiate_gltf(tree: &mut SceneTree, scene: &GltfScene) -> Result<NodeHandle, AssetError> {
    let root = tree.create_node3d(scene.name());

    // Only nodes reachable from the scene roots are created; other indices (unused nodes)
    // map to a handle that never resolves.
    let mut handles = vec![NodeHandle::from_entity(Entity::PLACEHOLDER); scene.nodes.len()];
    let mut stack: Vec<(usize, NodeHandle)> =
        scene.roots.iter().rev().map(|&i| (i, root)).collect();
    let mut visited = vec![false; scene.nodes.len()];
    while let Some((index, parent)) = stack.pop() {
        let Some(node) = scene.nodes.get(index) else {
            continue;
        };
        if std::mem::replace(&mut visited[index], true) {
            continue; // malformed file: node with two parents
        }
        let handle = tree.create_node3d(&node.name);
        tree.set_transform(handle, node.transform).expect("Node3D");
        tree.add_child(parent, handle).expect("fresh detached node");
        handles[index] = handle;
        stack.extend(node.children.iter().rev().map(|&c| (c, handle)));
    }

    for (index, node) in scene.nodes.iter().enumerate() {
        let handle = handles[index];
        let Some(mesh) = node.mesh.and_then(|m| scene.meshes.get(m)) else {
            continue;
        };
        if !tree.is_valid(handle) {
            continue;
        }
        let skin = node.skin.and_then(|s| scene.skins.get(s)).map(|skin| Skin {
            joints: skin
                .joints
                .iter()
                .map(|&j| handles.get(j).copied().unwrap_or(handles[index]))
                .collect(),
            inverse_bind_matrices: skin.inverse_bind_matrices.clone(),
        });
        for (i, primitive) in mesh.primitives.iter().enumerate() {
            let target = if i == 0 {
                tree.set_class_name(handle, class::MESH_INSTANCE_3D)
                    .expect("valid node");
                handle
            } else {
                let child = tree.spawn_node(
                    &format!("{}_{i}", node.name),
                    class::MESH_INSTANCE_3D,
                    (Transform::default(),),
                );
                tree.add_child(handle, child).expect("fresh detached node");
                child
            };
            let instance = MeshInstance3D::new(primitive.mesh, primitive.material);
            tree.insert_component(target, instance).expect("valid node");
            if let (Some(skin), true) = (&skin, primitive.skinned) {
                tree.insert_component(target, skin.clone())
                    .expect("valid node");
            }
        }
    }

    if !scene.animations.is_empty() {
        let mut player = AnimationPlayer::new(handles);
        player.clips = scene.animations.clone();
        let node = tree.spawn_node("AnimationPlayer", ANIMATION_PLAYER_CLASS, (player,));
        tree.add_child(root, node).expect("fresh detached node");
    }
    Ok(root)
}

/// Loads (or reuses) a glTF through the world's [`AssetServer`] and instantiates it.
///
/// Needs the `AssetServer` and `RenderAssets` resources; an `AnimationLibrary` is created if
/// missing.
pub fn spawn_gltf(tree: &mut SceneTree, path: &str) -> Result<NodeHandle, AssetError> {
    let world = tree.world_mut();
    let mut server = world
        .remove_resource::<AssetServer>()
        .ok_or(AssetError::MissingResource("AssetServer"))?;
    let Some(mut render) = world.remove_resource::<RenderAssets>() else {
        world.insert_resource(server);
        return Err(AssetError::MissingResource("RenderAssets"));
    };
    let mut library = world
        .remove_resource::<AnimationLibrary>()
        .unwrap_or_default();

    let result = server.load_gltf(&mut render, &mut library, path);

    world.insert_resource(server);
    world.insert_resource(render);
    world.insert_resource(library);
    let scene = result?;
    instantiate_gltf(tree, &scene)
}
