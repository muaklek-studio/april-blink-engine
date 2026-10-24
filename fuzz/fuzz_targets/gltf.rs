//! Models (`.glb`): a broken file is an error, never a panic.
#![no_main]

use ab_animation::AnimationLibrary;
use ab_asset::{AssetServer, MemorySource};
use ab_rendering::RenderAssets;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let mut source = MemorySource::new();
    source.insert("model.glb", bytes);
    let mut server = AssetServer::new(source);
    let _ = server.load_gltf(
        &mut RenderAssets::new(),
        &mut AnimationLibrary::new(),
        "model.glb",
    );
});
