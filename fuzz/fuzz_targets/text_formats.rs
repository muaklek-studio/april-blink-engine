//! The other project files: input maps, game settings and config, animation clips,
//! translations.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    let _ = ab_input::InputMap::from_toml(text);
    let _ = ab_engine::GameSettings::from_toml(text);
    let _ = ab_engine::game_config::GameConfig::from_toml(text);
    let _ = ab_engine::animation_file::ClipFile::from_toml(text);
    let _ = ab_locale::Language::from_toml(text);
});
