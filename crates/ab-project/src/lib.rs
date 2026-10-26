//! April Blink game projects, shared by the editor and `cargo xtask`.
//!
//! A project is a folder with its own cargo workspace:
//!
//! ```text
//! my-game/
//!   Cargo.toml            a cdylib depending on the `ab-script` SDK
//!   src/lib.rs            registers the scripts (`ab_script::export!`)
//!   src/scripts/          scripts made in the editor, one file each
//!   game.toml             title, main scene, window, input map
//!   input.toml            the input map: actions and their keys and buttons
//!   assets/…              scenes, models, sounds: only the game's content
//!   packed-assets/…       the binary assets, compressed, as git keeps them
//!   .ablink/              the editor's caches, logs and backups (not in git)
//! ```
//!
//! Its scripts build with any Rust toolchain into a library that the editor and the player
//! load through the script ABI, so neither needs the engine's sources. This crate:
//!
//! * creates projects from the template ([`create`]),
//! * reads them ([`Project`]),
//! * builds their script library in the background ([`ScriptBuild`]),
//! * exports games for players ([`export`]): the player executable, the assets packed into
//!   one file and the script library, checked by running the result.

pub mod archive;
pub mod asset_store;
mod engine;
pub mod export;
pub mod export_settings;
pub mod path;
mod platform;
pub mod process;
mod project;
pub mod script_templates;
mod scripts;
mod template;

pub use engine::{
    engine_root, is_engine_root, locate_engine, set_engine_root, EngineLookup, EngineSource,
};
pub use platform::Platform;
pub use project::{Profile, Project, GAME_FILE};
pub use scripts::{BuildState, ScriptBuild};
pub use template::{create, folder_name, sdk_dir, template_file, NewProject, NEW_SCENE};
