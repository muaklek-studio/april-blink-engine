//! Game data packs (chunks, or appended to an exported game) and chunk indexes (`index.abm`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    if let Ok(manifest) = ab_vfs::Manifest::from_bytes(bytes) {
        for (path, chunk) in manifest.files() {
            let _ = (path, chunk.file_name());
        }
    }
    if let Ok(pack) = ab_vfs::Pack::from_bytes(bytes.to_vec()) {
        let paths: Vec<String> = pack.entries().iter().map(|e| e.path.clone()).collect();
        for path in paths {
            let _ = pack.read(&path);
        }
    }
});
