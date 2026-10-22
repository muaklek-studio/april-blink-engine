use ab_asset::{AssetError, AssetServer, MemorySource};
use std::time::Duration;

/// 16-bit mono PCM WAV.
fn wav(sample_rate: u32, frames: usize) -> Vec<u8> {
    let data_len = (frames * 2) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    for field in [
        16u32.to_le_bytes().as_slice(),
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
    ] {
        out.extend_from_slice(field);
    }
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
fn loads_and_caches_audio() {
    let mut source = MemorySource::new();
    source
        .insert("sfx/blip.wav", wav(4000, 1000))
        .insert("sfx/broken.wav", b"RIFF nonsense".to_vec());
    let mut server = AssetServer::new(source);

    assert!(!server.is_loaded("sfx/blip.wav"));
    let blip = server.load_audio("sfx/blip.wav").unwrap();
    assert_eq!(blip.duration(), Duration::from_millis(250));
    assert!(server.is_loaded("./sfx/blip.wav"));
    assert_eq!(
        server.load_audio("sfx//blip.wav").unwrap().duration(),
        blip.duration()
    );

    assert!(matches!(
        server.load_audio("sfx/broken.wav"),
        Err(AssetError::InvalidData { .. })
    ));
    assert!(matches!(
        server.load_audio("sfx/missing.wav"),
        Err(AssetError::Io { .. })
    ));
}
