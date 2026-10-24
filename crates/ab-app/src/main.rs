//! Bootstrap application: a small atmospheric scene that exercises the engine end to end.
//!
//! ```text
//! cargo run -p ab-app                                 # windowed
//! cargo run -p ab-app -- --headless 600               # 600 frames, no window, no GPU
//! cargo run -p ab-app -- --screenshot shot.png        # render one frame offscreen to PNG
//! cargo xtask hot                                     # windowed, scripts hot reloaded
//! ```
//!
//! Controls (rebindable in `input.toml`, next to this crate's `Cargo.toml`): WASD / left
//! stick to move, Space and Q / bumpers for up and down, Shift to sprint, hold the right mouse
//! button or press Tab to look around (right stick on a gamepad), left click / E / right
//! trigger to shove a crate, Esc to release the mouse or quit.

// Release builds are windowed programs on Windows (no console); the log and crash reports go
// to the game's data folder instead (see `ab_engine::crash`).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod demo;
mod hud;

use ab_engine::core::init_logging;
use ab_engine::prelude::*;
use ab_engine::script_host::ScriptLibrary;
use anyhow::{bail, Context as _};
use std::path::PathBuf;
use tracing::{info, warn};

const USAGE: &str = "usage: ab-app [--headless <frames>] [--screenshot <file.png> [--frames <n>]] [--hot [<script library>]] [--overlay]";

enum Mode {
    Windowed,
    Headless { frames: u64 },
    Screenshot { path: PathBuf, frames: u64 },
}

struct Options {
    mode: Mode,
    /// Script library to hot reload.
    hot: Option<PathBuf>,
    /// Start with the debug overlay shown (F3 toggles it).
    overlay: bool,
}

fn parse_args() -> anyhow::Result<Option<Options>> {
    parse_args_from(std::env::args().skip(1))
}

/// Parses the arguments after the program name; `None` when only the usage was asked for.
fn parse_args_from(args: impl IntoIterator<Item = String>) -> anyhow::Result<Option<Options>> {
    let mut args = args.into_iter().peekable();
    let mut mode = Mode::Windowed;
    let mut frames = None;
    let mut hot = None;
    let mut overlay = false;
    while let Some(arg) = args.next() {
        if arg == "--overlay" {
            overlay = true;
            continue;
        }
        if arg == "--hot" {
            hot = Some(match args.next_if(|next| !next.starts_with("--")) {
                Some(path) => PathBuf::from(path),
                None => default_script_library()?,
            });
            continue;
        }
        let mut value = |name: &str| args.next().with_context(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--headless" => {
                let n = value("--headless")?
                    .parse()
                    .context("invalid frame count")?;
                mode = Mode::Headless { frames: n };
            }
            "--screenshot" => {
                mode = Mode::Screenshot {
                    path: value("--screenshot")?.into(),
                    frames: 0,
                };
            }
            "--frames" => frames = Some(value("--frames")?.parse().context("invalid frame count")?),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            other => bail!("unknown argument `{other}`\n{USAGE}"),
        }
    }
    if let Mode::Screenshot { frames: n, .. } = &mut mode {
        *n = frames.unwrap_or(60);
    }
    Ok(Some(Options { mode, hot, overlay }))
}

/// `ab_demo_scripts` built as a dynamic library, next to this executable (where cargo puts
/// it).
fn default_script_library() -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().context("executable has no directory")?;
    Ok(dir.join(format!(
        "{}ab_demo_scripts{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    )))
}

/// Registers the demo's scripts, builds the scene and optionally starts hot reloading.
fn setup(hot: Option<PathBuf>, overlay: bool) -> impl FnOnce(&mut Engine) -> anyhow::Result<()> {
    move |engine: &mut Engine| {
        if let Some(mut debug) = engine.tree().world().resource_mut::<DebugOverlay>() {
            debug.visible = overlay;
        }
        // The demo's scripts are linked in; they still run through the script ABI, exactly
        // like a library file (which `--hot` loads on top).
        let scripts =
            ScriptLibrary::from_entry(ab_demo_scripts::ab_script_library, "demo scripts")?;
        if let Some(mut registry) = engine.tree().world().resource_mut::<ScriptRegistry>() {
            scripts.register(&mut registry);
        }
        demo::build_scene(engine)?;
        if let Some(library) = hot {
            // Keeps watching even if this first load fails (e.g. not built yet).
            if let Err(err) = engine.enable_hot_reload(&library) {
                warn!(
                    "hot reload: {err:#}; watching {} for a new build",
                    library.display()
                );
            }
        }
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    init_logging();
    let logs = ab_engine::save::SaveStore::for_game("April Blink Demo")
        .dir()
        .join("logs");
    let logs = ab_engine::core::log_to_directory(&logs).ok().map(|_| logs);
    ab_engine::crash::run_reporting_crashes("April Blink Demo", logs.as_deref(), run)
}

fn run() -> anyhow::Result<()> {
    let Some(Options { mode, hot, overlay }) = parse_args()? else {
        return Ok(());
    };
    let config = EngineConfig::default();

    match mode {
        Mode::Windowed => ab_engine::run(config, setup(hot, overlay)),
        Mode::Headless { frames } => {
            let engine = ab_engine::run_headless(config, frames, 1.0 / 60.0, setup(hot, overlay))?;
            let time = engine.time();
            info!(
                frames = time.frame_count,
                physics_steps = time.physics_frame_count,
                nodes = engine.tree().node_count(),
                "headless run finished"
            );
            Ok(())
        }
        Mode::Screenshot { path, frames } => {
            let (width, height) = (1280, 720);
            let engine =
                ab_engine::run_headless(config.clone(), frames, 1.0 / 60.0, setup(hot, overlay))?;
            let pixels = ab_engine::render_offscreen(&engine, &config.renderer, width, height)?;
            let bytes: Vec<u8> = pixels.into_iter().flatten().collect();
            image::save_buffer(
                &path,
                &bytes,
                width,
                height,
                image::ExtendedColorType::Rgba8,
            )
            .with_context(|| format!("failed to write {}", path.display()))?;
            info!(path = %path.display(), width, height, "screenshot saved");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> anyhow::Result<Option<Options>> {
        parse_args_from(args.iter().map(|&a| a.to_owned()))
    }

    fn options(args: &[&str]) -> Options {
        parse(args).unwrap().expect("options")
    }

    #[test]
    fn no_arguments_open_a_window() {
        let options = options(&[]);
        assert!(matches!(options.mode, Mode::Windowed));
        assert!(options.hot.is_none());
        assert!(!options.overlay);
    }

    #[test]
    fn headless_takes_a_frame_count() {
        let options = options(&["--headless", "600", "--overlay"]);
        assert!(matches!(options.mode, Mode::Headless { frames: 600 }));
        assert!(options.overlay);
    }

    #[test]
    fn screenshots_default_to_60_frames() {
        let Mode::Screenshot { path, frames } = options(&["--screenshot", "shot.png"]).mode else {
            panic!("not a screenshot");
        };
        assert_eq!(path, PathBuf::from("shot.png"));
        assert_eq!(frames, 60);
    }

    #[test]
    fn frames_apply_to_screenshots_in_any_order() {
        for args in [
            ["--screenshot", "a.png", "--frames", "5"],
            ["--frames", "5", "--screenshot", "a.png"],
        ] {
            assert!(matches!(
                options(&args).mode,
                Mode::Screenshot { frames: 5, .. }
            ));
        }
    }

    #[test]
    fn the_last_mode_wins() {
        let options = options(&["--screenshot", "a.png", "--headless", "3"]);
        assert!(matches!(options.mode, Mode::Headless { frames: 3 }));
    }

    #[test]
    fn hot_takes_an_optional_library() {
        let explicit = options(&["--hot", "libscripts.so", "--overlay"]);
        assert_eq!(explicit.hot, Some(PathBuf::from("libscripts.so")));
        assert!(explicit.overlay);

        // Followed by another option: the demo scripts next to the executable.
        let default = options(&["--hot", "--overlay"]);
        let library = default.hot.expect("default library");
        assert!(library
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("ab_demo_scripts"));
        assert!(default.overlay);
        assert!(options(&["--hot"]).hot.is_some());
    }

    #[test]
    fn help_asks_for_no_run() {
        assert!(parse(&["--help"]).unwrap().is_none());
        assert!(parse(&["--overlay", "-h"]).unwrap().is_none());
    }

    #[test]
    fn bad_arguments_are_errors() {
        for args in [
            &["--bogus"][..],
            &["--headless"],
            &["--headless", "many"],
            &["--headless", "-1"],
            &["--screenshot"],
            &["--frames", "1.5"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
        let err = parse(&["--bogus"]).err().unwrap().to_string();
        assert!(err.contains("--bogus") && err.contains("usage"), "{err}");
    }
}
