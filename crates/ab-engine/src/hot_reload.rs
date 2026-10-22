//! Hot reload of a game's script library while the game (or the editor) runs.
//!
//! A script library is a crate built with the `ab-script` SDK as a `cdylib` (see
//! [`crate::script_host`]). [`Engine::enable_hot_reload`] watches its file: when a rebuild
//! lands, the new version is loaded and every script attached by name is swapped for its
//! new version, keeping its state through `save_state` / `load_state` (the SDK's
//! `serde_state!`). Scripts then get `reloaded` instead of `ready`.
//!
//! Old versions stay loaded (never unloaded), so anything still pointing into them keeps
//! working with the old code until replaced.
//!
//! [`Engine::enable_hot_reload`]: crate::Engine::enable_hot_reload

use crate::script_host::ScriptLibrary;
use ab_scene::{SceneTree, ScriptRegistry};
use anyhow::Context as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use tracing::info;

/// How often the library file is checked.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// How long the file must stay unchanged before it is loaded (the linker may still be
/// writing it).
const SETTLE_TIME: Duration = Duration::from_millis(300);

/// Watches a script library and reloads it into a scene tree when it changes.
#[derive(Debug)]
pub(crate) struct HotReload {
    library: PathBuf,
    loaded: Option<(SystemTime, u64)>,
    pending: Option<((SystemTime, u64), Instant)>,
    last_poll: Option<Instant>,
    generation: u32,
}

impl HotReload {
    pub(crate) fn new(library: PathBuf) -> Self {
        Self {
            library,
            loaded: None,
            pending: None,
            last_poll: None,
            generation: 0,
        }
    }

    pub(crate) fn library(&self) -> &Path {
        &self.library
    }

    /// Whether a changed library is ready to load (it changed and then stayed unchanged for
    /// a moment).
    pub(crate) fn poll(&mut self) -> bool {
        let now = Instant::now();
        if self
            .last_poll
            .is_some_and(|last| now.duration_since(last) < POLL_INTERVAL)
        {
            return false;
        }
        self.last_poll = Some(now);
        let Ok(stamp) = file_stamp(&self.library) else {
            return false;
        };
        if Some(stamp) == self.loaded {
            self.pending = None;
            return false;
        }
        match self.pending {
            Some((pending, since)) if pending == stamp => now.duration_since(since) >= SETTLE_TIME,
            _ => {
                self.pending = Some((stamp, now));
                false
            }
        }
    }

    /// Loads the current library and swaps the tree's named scripts for its versions.
    /// Returns how many scripts were replaced.
    pub(crate) fn load(&mut self, tree: &mut SceneTree) -> anyhow::Result<usize> {
        let fresh = self.load_registry()?;
        let names = fresh.len();
        tree.world()
            .resource_mut::<ScriptRegistry>()
            .context("the ScriptRegistry resource is missing")?
            .extend(fresh);
        let replaced = tree.reload_scripts();
        info!(
            library = %self.library.display(),
            version = self.generation,
            registered = names,
            replaced,
            "script library loaded"
        );
        Ok(replaced)
    }

    /// Loads the current library and returns the scripts it registers, without touching any
    /// scene (an editor fills its Play-mode registry this way while the scene is edited).
    pub(crate) fn load_registry(&mut self) -> anyhow::Result<ScriptRegistry> {
        let stamp = file_stamp(&self.library)
            .with_context(|| format!("cannot read {}", self.library.display()))?;
        // Remembered before loading: a broken build is not retried until it changes.
        self.loaded = Some(stamp);
        self.pending = None;
        let library = ScriptLibrary::load(&self.library)?;
        self.generation += 1;
        Ok(library.registry())
    }
}

fn file_stamp(path: &Path) -> std::io::Result<(SystemTime, u64)> {
    let metadata = std::fs::metadata(path)?;
    Ok((metadata.modified()?, metadata.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_waits_for_the_file_to_settle() {
        let dir = std::env::temp_dir().join(format!("ab-hot-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("lib.bin");
        let mut hot = HotReload::new(file.clone());
        assert!(!hot.poll(), "missing file");
        std::fs::write(&file, b"v1").unwrap();
        hot.last_poll = None;
        assert!(!hot.poll(), "just changed");
        std::thread::sleep(SETTLE_TIME + Duration::from_millis(20));
        hot.last_poll = None;
        assert!(hot.poll(), "settled");
        hot.loaded = file_stamp(&file).ok();
        hot.last_poll = None;
        assert!(!hot.poll(), "already loaded");
        // Not a library: loading fails cleanly.
        let mut tree = SceneTree::new();
        assert!(hot.load(&mut tree).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
