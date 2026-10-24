//! Scene and prefab files: any text parses or errors, and what parses saves and parses back.
#![no_main]

use ab_engine::scene_file::SceneFile;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    if let Ok(scene) = SceneFile::from_toml(text) {
        let saved = scene.to_toml();
        let again = SceneFile::from_toml(&saved).expect("a saved scene parses");
        // NaN never equals itself: compare the text instead.
        assert_eq!(again.to_toml(), saved);
    }
});
