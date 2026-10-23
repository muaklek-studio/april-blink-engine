//! **Night Relics**, the engine's vertical slice: a dark house at night, four relics to carry
//! to the altar, and a ghost that hunts you through the rooms. Survive until dawn; every
//! relic on the altar brings dawn 30 seconds closer, and makes the ghost faster.
//!
//! Everything gameplay lives in scripts written with the `ab-script` SDK and registered here,
//! so `cargo xtask hot slice` reloads them while playing. The level is
//! `assets/slice/level.abscene`; the game starts from this crate's `game.toml`.

mod flicker;
mod game;
mod ghost;
mod hud;
mod player;

pub use flicker::Flicker;
pub use game::{Game, Phase};
pub use ghost::Ghost;
pub use player::Player;

use ab_script::Registry;

/// Scene file of the level, relative to the asset directory.
pub const LEVEL: &str = "slice/level.abscene";
/// Emitted by the player node when a relic is put on the altar.
pub const RELIC_PLACED: &str = "relic_placed";
/// Groups used by the level.
pub const RELICS: &str = "relics";
pub const PLACED: &str = "placed";
pub const ALTAR: &str = "altar";
pub const ALTAR_SLOT: &str = "altar_slot";

/// Collision layers (bit masks).
pub mod layers {
    pub const LEVEL: u32 = 1;
    pub const RELICS: u32 = 2;
    pub const PLAYER: u32 = 4;
}

/// Registers the game's scripts.
pub fn register(registry: &mut Registry) {
    registry
        .register::<Game>("Game")
        .register::<Player>("Player")
        .register::<Ghost>("Ghost")
        .register::<Flicker>("Flicker");
}

ab_script::export!(register);
