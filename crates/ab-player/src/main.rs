//! The April Blink game player: runs a game made with the editor from its assets and its
//! script library. Exports ship it renamed after the game:
//!
//! ```text
//! MyGame/
//!   my-game(.exe)                     this player
//!   my-game.abp                       the assets, and game.toml and the input map
//!   libmy-game_scripts.so             the scripts (my-game_scripts.dll on Windows)
//! ```
//!
//! During development it runs a project in place:
//!
//! ```text
//! ab-player --project <project> --scripts <project>/target/debug/libmy_game.so
//! ```
//!
//! Every game option works too (`--headless <frames>`, `--fullscreen`, `--hot`, …; see
//! `ab_engine::game`).

// Release builds are windowed programs on Windows (no console); the log and crash reports go
// to the game's data folder instead (see `ab_engine::crash`).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ab_project::{Platform, GAME_FILE};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let project = take_option(&mut args, "--project");
    let assets = take_option(&mut args, "--assets");
    let scripts = take_option(&mut args, "--scripts")
        .map(PathBuf::from)
        .or_else(shipped_library);
    let mut game = match scripts {
        Some(library) => {
            let hot = library.clone();
            ab_engine::Game::with_script_file(GAME_FILE, library).hot_library_path(hot)
        }
        // A game without scripts (scenes only).
        None => ab_engine::Game::new(GAME_FILE, |_| {}),
    };
    if let Some(project) = project {
        game = game.project(project);
    }
    if let Some(assets) = assets {
        game = game.assets(assets);
    }
    game.run_with_args(args)
}

/// Removes `--name <value>` from `args`, returning the value. Shipped games take no such
/// options (a player cannot be pointed at other code).
fn take_option(args: &mut Vec<String>, name: &str) -> Option<String> {
    let at = args.iter().position(|a| a == name)?;
    args.remove(at);
    let value = (at < args.len()).then(|| args.remove(at));
    if ab_engine::SHIPPING {
        eprintln!("{name} is a developer option; this build ignores it");
        return None;
    }
    value
}

/// The script library shipped next to the player, named after it.
fn shipped_library() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let stem = exe.file_stem()?.to_string_lossy().into_owned();
    let library = exe.parent()?.join(Platform::host().shipped_library(&stem));
    library.is_file().then_some(library)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_taken_out_of_the_arguments() {
        let mut args: Vec<String> = ["--headless", "5", "--assets", "/g/assets", "--overlay"]
            .map(String::from)
            .into();
        let assets = take_option(&mut args, "--assets");
        assert_eq!(args, ["--headless", "5", "--overlay"]);
        assert_eq!(assets.is_some(), !ab_engine::SHIPPING);
        assert_eq!(take_option(&mut args, "--scripts"), None);
    }
}
