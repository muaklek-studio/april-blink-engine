//! Game data packs (`.abpack`, or appended to an exported game).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    if let Ok(pack) = ab_vfs::Pack::from_bytes(bytes.to_vec()) {
        let paths: Vec<String> = pack.entries().iter().map(|e| e.path.clone()).collect();
        for path in paths {
            let _ = pack.read(&path);
        }
    }
});
