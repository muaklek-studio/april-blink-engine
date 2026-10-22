//! Night Relics.
//!
//! ```text
//! cargo run -p ab-slice                                   # play
//! cargo run -p ab-slice -- --headless 600                 # run without a window
//! cargo run -p ab-slice -- --screenshot shot.png [--frames n]
//! cargo xtask hot slice                                   # play with hot-reloaded scripts
//! cargo xtask export slice                                # build a shippable copy in dist/
//! ```
//!
//! Everything else (window, scenes, input) comes from `game.toml` and `input.toml` in this
//! crate; the scenes and models are in the engine's `assets/slice`.

// Release builds are windowed programs on Windows (no console); the log and crash reports go
// to the game's data folder instead (see `ab_engine::crash`).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    ab_engine::run_game(
        ab_engine::Game::with_scripts("game.toml", ab_slice::ab_script_library)
            .project(env!("CARGO_MANIFEST_DIR"))
            .hot_library("ab_slice"),
    )
}
