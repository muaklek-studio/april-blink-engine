//! Asset loading and caching.
//!
//! * [`AssetSource`] abstracts where bytes come from: [`FileSystemSource`] in development,
//!   the packed VFS in release builds (Phase 12), [`MemorySource`] in tests.
//! * [`AssetServer`] (an ECS resource) loads images, audio and glTF 2.0 files (`.gltf` with
//!   external or embedded buffers, and `.glb`) and caches them by normalised path, so loading
//!   the same file twice returns the same GPU resources.
//! * A loaded [`GltfScene`] is a template; [`instantiate_gltf`] builds a node tree from it
//!   (meshes, materials, textures, skins and an `AnimationPlayer` with every clip).
//!
//! ```no_run
//! # fn demo(tree: &mut ab_scene::SceneTree) -> Result<(), ab_asset::AssetError> {
//! let robot = ab_asset::spawn_gltf(tree, "models/robot.glb")?;
//! tree.add_child(tree.root(), robot).expect("valid nodes");
//! # Ok(()) }
//! ```

mod base64;
mod error;
mod gltf_loader;
mod image_loader;
mod instantiate;
mod server;
mod source;

pub use error::AssetError;
pub use gltf_loader::{GltfMesh, GltfNode, GltfPrimitive, GltfScene, GltfSkin};
pub use image_loader::decode_image;
pub use instantiate::{instantiate_gltf, spawn_gltf, ANIMATION_PLAYER_CLASS};
pub use server::AssetServer;
pub use source::{normalize_path, AssetSource, FileSystemSource, MemorySource};
