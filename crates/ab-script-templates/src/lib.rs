//! The starter scripts the editor offers when making a new script: movement, jumping,
//! dashing, first and third person controllers, animated characters, melee combat with
//! combos, health, guns with reloading, a crosshair, enemies, auto aim and a car.
//!
//! Each template is one self-contained file that compiles in any game's script crate (it
//! only uses `ab_script` and `serde`), so the editor copies it into the game as it is, with
//! the type renamed. Building them here, against the SDK, keeps them compiling and lint-free
//! as the engine changes; `tests/templates.rs` runs them in an engine.
//!
//! Templates work together through signals and groups rather than each other's types (a
//! game can rename them):
//!
//! * `damage` (amount) on a node hurts it when it has `Health`; it then emits `damaged`,
//!   and `died` at 0. Nodes that can be hurt are in the `damageable` group.
//! * The player controllers join the `player` group; `EnemyAi` joins `enemies`.
//! * `Gun` emits `fired` and `hit`, which `Crosshair` shows; `AutoAim` emits `aim_assist`,
//!   which the controllers add to their look.
//! * `Dash` puts its node in `movement_busy` (the `Movement` script then lets it fly) and
//!   `invulnerable` (`Health` then ignores damage).

pub mod animated_character;
pub mod auto_aim;
pub mod car;
pub mod crosshair;
pub mod dash;
pub mod enemy;
pub mod first_person;
pub mod gun;
pub mod health;
pub mod jump;
pub mod melee_combat;
pub mod movement;
pub mod third_person;

use ab_script::Registry;

/// Registers every template under its type name.
pub fn register(registry: &mut Registry) {
    registry
        .register::<animated_character::AnimatedCharacter>("AnimatedCharacter")
        .register::<auto_aim::AutoAim>("AutoAim")
        .register::<car::Car>("Car")
        .register::<crosshair::Crosshair>("Crosshair")
        .register::<dash::Dash>("Dash")
        .register::<enemy::EnemyAi>("EnemyAi")
        .register::<first_person::FirstPersonController>("FirstPersonController")
        .register::<gun::Gun>("Gun")
        .register::<health::Health>("Health")
        .register::<jump::Jump>("Jump")
        .register::<melee_combat::MeleeCombat>("MeleeCombat")
        .register::<movement::Movement>("Movement")
        .register::<third_person::ThirdPersonController>("ThirdPersonController");
}
