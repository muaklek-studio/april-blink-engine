//! {{title}}: the game's scripts.
//!
//! Make scripts in the editor (Assets → Create → Script): each is a file in `src/scripts/`,
//! registered automatically. Scripts written elsewhere are registered here by the names
//! scene files use:
//!
//! ```ignore
//! registry.register::<Player>("Player");
//! ```

pub mod scripts;

use ab_script::Registry;

/// Registers the scripts by the names the scene files use.
pub fn register(registry: &mut Registry) {
    scripts::register(registry);
}

// The entry point the editor and the exported game load.
ab_script::export!(register);
