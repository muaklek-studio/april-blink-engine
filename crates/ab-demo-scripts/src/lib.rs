//! Gameplay scripts of the demo, written with the `ab-script` SDK. Everything here can be hot
//! reloaded: run `cargo xtask hot`, edit a script, save, and the running demo picks up the
//! change.
//!
//! Scripts read their configuration from the scene in `ready` (node paths, groups, component
//! values), so the scene code only attaches them by name.

mod camera;
mod hud;
mod impacts;
mod props;

pub use camera::FlyCamera;
pub use hud::HudScript;
pub use impacts::ImpactSounds;
pub use props::{Flicker, Spinner, Stats};

use ab_script::Registry;

/// Group of the crates the HUD counts and drops.
pub const CRATES_GROUP: &str = "crates";

/// Registers every script of the demo by name.
pub fn register(registry: &mut Registry) {
    registry
        .register::<FlyCamera>("FlyCamera")
        .register::<HudScript>("HudScript")
        .register::<ImpactSounds>("ImpactSounds")
        .register::<Spinner>("Spinner")
        .register::<Flicker>("Flicker")
        .register::<Stats>("Stats");
}

ab_script::export!(register);
