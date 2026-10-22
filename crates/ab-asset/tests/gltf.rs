//! Loads the generated sample assets in `assets/models` (see scripts/gen_sample_assets.py).

use ab_animation::{apply_animations, AnimationLibrary, AnimationPlayer};
use ab_asset::{spawn_gltf, AssetError, AssetServer, MemorySource};
use ab_core::{Quat, Vec3};
use ab_rendering::{extract, ColorSpace, MeshInstance3D, RenderAssets, RenderList, Skin};
use ab_scene::SceneTree;
use std::path::PathBuf;

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

fn tree() -> SceneTree {
    let mut tree = SceneTree::new();
    let world = tree.world_mut();
    world.insert_resource(AssetServer::from_directory(assets_dir()));
    world.insert_resource(RenderAssets::new());
    world.insert_resource(AnimationLibrary::new());
    tree
}

#[test]
fn loads_skinned_animated_gltf_with_external_texture() {
    let mut server = AssetServer::from_directory(assets_dir());
    let mut render = RenderAssets::new();
    let mut library = AnimationLibrary::new();
    let scene = server
        .load_gltf(&mut render, &mut library, "models/totem.gltf")
        .unwrap();

    assert_eq!(scene.name(), "totem");
    assert_eq!(scene.nodes.len(), 5);
    assert_eq!(scene.roots, [0]);
    assert_eq!(scene.skins.len(), 1);
    assert_eq!(scene.skins[0].joints, [1, 2, 3]);
    let ibm = scene.skins[0].inverse_bind_matrices[2];
    assert!(ibm
        .transform_point3(Vec3::new(0.0, 2.0, 0.0))
        .abs_diff_eq(Vec3::ZERO, 1e-6));

    let primitive = scene.meshes[0].primitives[0];
    assert!(primitive.skinned);
    let mesh = render.mesh(primitive.mesh).unwrap();
    assert_eq!(mesh.vertices.len(), 72);
    assert!(
        mesh.vertices.iter().all(|v| v.tangent[3].abs() == 1.0),
        "tangents computed"
    );

    let material = render.material(primitive.material).unwrap();
    let albedo = render.texture(material.albedo_texture.unwrap()).unwrap();
    assert_eq!((albedo.width, albedo.height), (128, 192));
    assert_eq!(albedo.color_space, ColorSpace::Srgb);

    let names: Vec<&str> = scene.animations.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["Sway", "Spin"]);
    let sway = library.get(scene.animations[0].1).unwrap();
    assert_eq!(sway.duration, 2.0);
    assert_eq!(sway.channels.len(), 2);
}

#[test]
fn loads_glb_with_embedded_textures() {
    let mut server = AssetServer::from_directory(assets_dir());
    let mut render = RenderAssets::new();
    let mut library = AnimationLibrary::new();
    let scene = server
        .load_gltf(&mut render, &mut library, "models/crate.glb")
        .unwrap();
    let material = render.material(scene.materials[0]).unwrap();
    let normal = render.texture(material.normal_texture.unwrap()).unwrap();
    assert_eq!(normal.color_space, ColorSpace::Linear);
    assert_eq!(scene.textures.len(), 2);
    assert!(scene.animations.is_empty());
}

#[test]
fn loading_twice_reuses_resources() {
    let mut server = AssetServer::from_directory(assets_dir());
    let mut render = RenderAssets::new();
    let mut library = AnimationLibrary::new();
    let a = server
        .load_gltf(&mut render, &mut library, "models/crate.glb")
        .unwrap();
    let meshes = render.mesh_count();
    let b = server
        .load_gltf(&mut render, &mut library, "./models//crate.glb")
        .unwrap();
    assert!(
        std::sync::Arc::ptr_eq(&a, &b),
        "normalised paths share the cache entry"
    );
    assert_eq!(render.mesh_count(), meshes);
    assert!(server.is_loaded("models/crate.glb"));

    let t1 = server
        .load_texture(&mut render, "models/totem_albedo.png", ColorSpace::Srgb)
        .unwrap();
    let t2 = server
        .load_texture(&mut render, "models/totem_albedo.png", ColorSpace::Srgb)
        .unwrap();
    let t3 = server
        .load_texture(&mut render, "models/totem_albedo.png", ColorSpace::Linear)
        .unwrap();
    assert_eq!(t1, t2);
    assert_ne!(t1, t3, "color space is part of the cache key");
}

#[test]
fn instantiates_node_tree_with_skin_and_player() {
    let mut tree = tree();
    let totem = spawn_gltf(&mut tree, "models/totem.gltf").unwrap();
    tree.add_child(tree.root(), totem).unwrap();

    assert_eq!(&*tree.name(totem).unwrap(), "totem");
    let joint2 = tree
        .get_node(totem, "Totem/Joint0/Joint1/Joint2")
        .expect("joint chain");
    let mesh = tree.get_node(totem, "Totem/TotemMesh").expect("mesh node");
    assert_eq!(tree.class_name(mesh), Some("MeshInstance3D"));
    assert!(tree.component::<MeshInstance3D>(mesh).is_some());
    let skin = tree.component::<Skin>(mesh).unwrap().clone();
    assert_eq!(skin.joints[2], joint2);

    let player_node = tree
        .get_node(totem, "AnimationPlayer")
        .expect("player node");
    let mut player = tree
        .component::<AnimationPlayer>(player_node)
        .unwrap()
        .clone();
    assert!(player.play_named("Sway"));
    *tree.component_mut::<AnimationPlayer>(player_node).unwrap() = player;

    // Quarter of the cycle: Joint1 and Joint2 are at their 0.3 rad peak.
    apply_animations(tree.world(), 0.5);
    let rotation = tree.rotation(joint2).unwrap();
    assert!(
        rotation.abs_diff_eq(Quat::from_rotation_z(0.3), 1e-5),
        "{rotation}"
    );

    // The renderer's joint palette follows the pose.
    tree.update_transforms();
    let mut list = RenderList::default();
    extract(tree.world(), 1.0, &mut list);
    let draw = list
        .draws
        .iter()
        .find(|d| d.joints.is_some())
        .expect("skinned draw");
    let joints = draw.joints.clone().unwrap();
    let palette = &list.joint_matrices[joints.start as usize..joints.end as usize];
    // The top of the bind pose (0, 3, 0) moves sideways once joints bend.
    let top = palette[2].transform_point3(Vec3::new(0.0, 3.0, 0.0));
    assert!(top.x < -0.5, "{top}");
}

#[test]
fn missing_files_and_resources_are_reported() {
    let mut tree = tree();
    let err = spawn_gltf(&mut tree, "models/nope.glb").unwrap_err();
    assert!(matches!(err, AssetError::Io { .. }), "{err}");
    assert!(
        tree.world().contains_resource::<AssetServer>(),
        "resources restored after errors"
    );

    let mut bare = SceneTree::new();
    assert!(matches!(
        spawn_gltf(&mut bare, "models/totem.gltf"),
        Err(AssetError::MissingResource("AssetServer"))
    ));
}

#[test]
fn rejects_corrupt_gltf() {
    let mut source = MemorySource::new();
    source.insert("bad.gltf", "{ not json");
    source.insert(
        "missing_buffer.gltf",
        r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4,"uri":"gone.bin"}]}"#,
    );
    let mut server = AssetServer::new(source);
    let mut render = RenderAssets::new();
    let mut library = AnimationLibrary::new();
    assert!(matches!(
        server.load_gltf(&mut render, &mut library, "bad.gltf"),
        Err(AssetError::Gltf { .. })
    ));
    assert!(matches!(
        server.load_gltf(&mut render, &mut library, "missing_buffer.gltf"),
        Err(AssetError::Io { .. })
    ));
    assert!(matches!(
        server.load_gltf(&mut render, &mut library, "../escape.gltf"),
        Err(AssetError::InvalidData { .. })
    ));
}
