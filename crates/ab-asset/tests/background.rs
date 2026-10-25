//! Background loading (`request_*` + `poll`) and streaming audio.

use ab_animation::AnimationLibrary;
use ab_asset::{AssetServer, MemorySource, STREAMING_AUDIO_MIN_BYTES, STREAMING_AUDIO_MIN_SECONDS};
use ab_rendering::{ColorSpace, RenderAssets};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// Polls until nothing is pending (or fails after a few seconds).
fn wait(server: &mut AssetServer, render: &mut RenderAssets, library: &mut AnimationLibrary) {
    let start = Instant::now();
    while server.pending_loads() > 0 {
        server.poll(render, library);
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "background loads hang"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// 16-bit mono PCM WAV of silence.
fn wav(sample_rate: u32, frames: usize) -> Vec<u8> {
    let data_len = (frames * 2) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.resize(out.len() + frames * 2, 0);
    out
}

#[test]
fn requested_assets_load_in_the_background() {
    let mut server = AssetServer::from_directory(assets_dir());
    let mut render = RenderAssets::new();
    let mut library = AnimationLibrary::new();

    server.request_gltf("models/totem.gltf").unwrap();
    server.request_gltf("./models/totem.gltf").unwrap();
    server
        .request_texture("models/totem_albedo.png", ColorSpace::Srgb)
        .unwrap();
    server.request_audio("audio/thud.wav").unwrap();
    assert_eq!(
        server.pending_loads(),
        3,
        "a repeated request is not queued twice"
    );
    assert!(server.is_pending("models/totem.gltf"));

    wait(&mut server, &mut render, &mut library);
    assert!(server.is_loaded("models/totem.gltf"));
    assert!(server.is_loaded("audio/thud.wav"));

    // Ready: loading returns the cached resources without adding new ones.
    let textures = render.texture_count();
    let scene = server
        .load_gltf(&mut render, &mut library, "models/totem.gltf")
        .unwrap();
    server
        .load_texture(&mut render, "models/totem_albedo.png", ColorSpace::Srgb)
        .unwrap();
    assert_eq!(render.texture_count(), textures);
    assert_eq!(scene.nodes.len(), 5);
    assert_eq!(scene.animations.len(), 2);

    // Requests for what is loaded do nothing.
    server.request_gltf("models/totem.gltf").unwrap();
    assert_eq!(server.pending_loads(), 0);
}

#[test]
fn background_failures_are_reported_by_load() {
    let mut source = MemorySource::new();
    source.insert("models/broken.glb", b"glTF nonsense".to_vec());
    let mut server = AssetServer::new(source);
    let mut render = RenderAssets::new();
    let mut library = AnimationLibrary::new();

    server.request_gltf("models/broken.glb").unwrap();
    server.request_audio("missing.ogg").unwrap();
    assert!(server.request_gltf("../outside.glb").is_err());
    wait(&mut server, &mut render, &mut library);

    assert!(server
        .load_gltf(&mut render, &mut library, "models/broken.glb")
        .is_err());
    assert!(server.load_audio("missing.ogg").is_err());
}

#[test]
fn evicting_drops_a_load_in_flight() {
    let mut server = AssetServer::from_directory(assets_dir());
    let mut render = RenderAssets::new();
    let mut library = AnimationLibrary::new();

    server.request_audio("audio/hum.wav").unwrap();
    server.evict("audio/hum.wav");
    assert_eq!(server.pending_loads(), 0);
    std::thread::sleep(Duration::from_millis(50));
    server.poll(&mut render, &mut library);
    assert!(
        !server.is_loaded("audio/hum.wav"),
        "the stale result is ignored"
    );
}

#[test]
fn large_audio_files_stream() {
    let frames = STREAMING_AUDIO_MIN_BYTES / 2;
    let mut source = MemorySource::new();
    source
        .insert("music/long.wav", wav(44_100, frames))
        .insert("sfx/short.wav", wav(8000, 800));
    let mut server = AssetServer::new(source);

    let long = server.load_audio("music/long.wav").unwrap();
    assert!(long.is_streaming());
    assert_eq!(long.sample_rate(), 44_100);
    assert!(!server.load_audio("sfx/short.wav").unwrap().is_streaming());
}

#[test]
fn large_but_short_audio_files_decode() {
    // Over the size threshold, but a few seconds of high sample rate: an effect that plays
    // often, kept decoded so it starts at once.
    let frames = STREAMING_AUDIO_MIN_BYTES / 2 + 1000;
    let mut source = MemorySource::new();
    source.insert("sfx/big.wav", wav(192_000, frames));
    let mut server = AssetServer::new(source);
    let big = server.load_audio("sfx/big.wav").unwrap();
    assert!(big.duration().as_secs_f32() < STREAMING_AUDIO_MIN_SECONDS);
    assert!(!big.is_streaming());
}

#[test]
fn status_follows_a_request_through_eviction_and_failure() {
    use ab_asset::{AssetRequest, LoadStatus};
    let mut source = MemorySource::new();
    source
        .insert("sfx/blip.wav", wav(8000, 800))
        .insert("sfx/broken.wav", b"RIFF nonsense".to_vec());
    let mut server = AssetServer::new(source);
    let mut render = RenderAssets::new();
    let mut library = AnimationLibrary::new();
    let blip = AssetRequest::Audio("./sfx/blip.wav".into());
    let broken = AssetRequest::Audio("sfx/broken.wav".into());

    assert_eq!(server.status(&blip), LoadStatus::NotLoaded);
    server.request(&blip).unwrap();
    server.request(&broken).unwrap();
    assert_eq!(server.status(&blip), LoadStatus::Loading);
    wait(&mut server, &mut render, &mut library);
    assert_eq!(server.status(&blip), LoadStatus::Loaded);
    assert_eq!(server.status(&broken), LoadStatus::Failed);

    // Its file changed: no longer cached.
    server.evict("sfx/blip.wav");
    assert_eq!(server.status(&blip), LoadStatus::NotLoaded);
    // A failed one is tried again when requested again.
    server.request(&broken).unwrap();
    assert_eq!(server.status(&broken), LoadStatus::Loading);
    wait(&mut server, &mut render, &mut library);
    assert_eq!(server.status(&broken), LoadStatus::Failed);
    assert_eq!(
        server.status(&AssetRequest::Gltf("../outside.glb".into())),
        LoadStatus::Failed
    );
}
