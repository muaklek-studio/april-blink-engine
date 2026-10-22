# ab-script

Game scripts for [April Blink Engine](https://github.com/thanaphoom/april-blink-engine), in Rust.

A project's scripts are a `cdylib` crate depending on this SDK. The editor and exported games
load it through a stable C ABI (`ab-script-api`), so:

- the editor and the player never need to be rebuilt with your code, and your code never needs
  the engine's sources;
- a library keeps working with newer engines: every engine feature is a binding called by
  name, and adding one never changes the ABI. A binding an older engine lacks fails with a
  clear error instead of crashing;
- scripts are hot reloaded while editing: save, and the editor rebuilds the library and swaps
  it in, keeping each script's state (`serde_state!()`).

## A script

```rust
use ab_script::prelude::*;

/// Spins its node around Y.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Spinner {
    speed: f32,
}

impl Script for Spinner {
    serde_state!();

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        let _ = tree.rotate(node, Vec3::Y, self.speed * delta);
    }
}

fn register(registry: &mut Registry) {
    registry.register::<Spinner>("Spinner");
}

ab_script::export!(register);
```

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
ab-script = "0.1"
serde = { version = "1", features = ["derive"] }
```

Scenes attach scripts by their registered name (`script = "Spinner"`). New projects made by
the editor (or `cargo xtask new-game`) are set up like this already; scripts made in the editor
(Assets → Create → Script) are registered for you.

## What scripts can do

Each callback (`enter_tree`, `ready`, `process`, `physics_process`, `exit_tree`, `on_signal`,
`reloaded`) receives a `Tree`: nodes and transforms, groups, signals, input, physics bodies,
characters and queries, UI controls, cameras, lights, particles, tweens, timers, audio,
navigation, saves, and logging (`info!`, `warn!`, ...). Errors from the engine come back as
`Result`s; a script that panics is disabled, and the game keeps running.

## Testing scripts

`script_type::<T>()` gives the registered type, for tests that run scripts in an engine.
