//! Save games and settings: any `serde` type, one JSON file per slot, in the player's data
//! folder.
//!
//! ```no_run
//! use ab_engine::save::SaveStore;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize, Default)]
//! struct Progress {
//!     level: u32,
//!     coins: u32,
//! }
//!
//! # fn demo(tree: &ab_engine::scene::SceneTree) -> Result<(), ab_engine::save::SaveError> {
//! let saves = tree.world().resource::<SaveStore>().unwrap();
//! saves.save("slot1", &Progress { level: 3, coins: 120 })?;
//! let progress: Progress = saves.load("slot1")?.unwrap_or_default();
//! # Ok(())
//! # }
//! ```
//!
//! [`run_game`](crate::run_game) inserts a `SaveStore` for the game's `save_name` (or title).
//! Folders: `%APPDATA%\<game>` (Windows), `~/Library/Application Support/<game>` (macOS),
//! `$XDG_DATA_HOME/<game>` or `~/.local/share/<game>` (Linux); `AB_SAVE_DIR` overrides.

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("invalid save slot `{0}` (use letters, digits, `-` and `_`)")]
    InvalidSlot(String),
    #[error("save file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("save slot `{slot}` has unreadable data: {source}")]
    Format {
        slot: String,
        #[source]
        source: serde_json::Error,
    },
}

/// ECS resource: reads and writes save slots in one folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveStore {
    dir: PathBuf,
}

impl SaveStore {
    /// The data folder of the game called `name` (see the module docs).
    pub fn for_game(name: &str) -> Self {
        Self::in_dir(user_data_dir().join(folder_name(name)))
    }

    /// Saves in `dir` (tests, portable builds).
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, slot: &str) -> Result<PathBuf, SaveError> {
        let valid = !slot.is_empty()
            && slot.len() <= 64
            && slot
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid {
            return Err(SaveError::InvalidSlot(slot.to_owned()));
        }
        Ok(self.dir.join(format!("{slot}.json")))
    }

    /// Writes `value` to `slot`. The previous save stays intact until the new one is
    /// completely written (no half-written saves after a crash or power cut).
    pub fn save<T: Serialize>(&self, slot: &str, value: &T) -> Result<(), SaveError> {
        let path = self.path(slot)?;
        let io = |source| SaveError::Io {
            path: path.clone(),
            source,
        };
        let text = serde_json::to_vec_pretty(value).map_err(|source| SaveError::Format {
            slot: slot.to_owned(),
            source,
        })?;
        std::fs::create_dir_all(&self.dir).map_err(io)?;
        let temp = path.with_extension("json.tmp");
        // Flushed to disk before it replaces the old save, so a power cut leaves either the
        // old or the new file, never an empty one.
        let mut file = std::fs::File::create(&temp).map_err(io)?;
        std::io::Write::write_all(&mut file, &text).map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        std::fs::rename(&temp, &path).map_err(io)?;
        Ok(())
    }

    /// The value in `slot`, `None` if nothing was saved there.
    pub fn load<T: DeserializeOwned>(&self, slot: &str) -> Result<Option<T>, SaveError> {
        let path = self.path(slot)?;
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(SaveError::Io { path, source }),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| SaveError::Format {
                slot: slot.to_owned(),
                source,
            })
    }

    pub fn exists(&self, slot: &str) -> bool {
        self.path(slot).is_ok_and(|p| p.is_file())
    }

    /// Removes a slot. Returns whether there was one.
    pub fn delete(&self, slot: &str) -> Result<bool, SaveError> {
        let path = self.path(slot)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(SaveError::Io { path, source }),
        }
    }

    /// The saved slots, sorted.
    pub fn slots(&self) -> Vec<String> {
        let mut slots: Vec<String> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".json").map(str::to_owned)
            })
            .collect();
        slots.sort();
        slots
    }
}

/// A folder name from a game title: letters, digits, spaces, `-`, `_` kept.
fn folder_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "April Blink Game".into()
    } else {
        trimmed.into()
    }
}

/// The per-user data folder of this platform.
pub fn user_data_dir() -> PathBuf {
    let env = |key: &str| {
        std::env::var_os(key)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = env("AB_SAVE_DIR") {
        return dir;
    }
    if cfg!(windows) {
        if let Some(dir) = env("APPDATA") {
            return dir;
        }
    } else if cfg!(target_os = "macos") {
        if let Some(home) = env("HOME") {
            return home.join("Library/Application Support");
        }
    } else {
        if let Some(dir) = env("XDG_DATA_HOME") {
            return dir;
        }
        if let Some(home) = env("HOME") {
            return home.join(".local/share");
        }
    }
    PathBuf::from("saves")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Progress {
        level: u32,
        name: String,
    }

    #[test]
    fn save_load_list_delete() {
        let dir = std::env::temp_dir().join(format!("ab-saves-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let saves = SaveStore::in_dir(&dir);
        assert_eq!(saves.load::<Progress>("slot1").unwrap(), None);
        let progress = Progress {
            level: 3,
            name: "ผู้เล่น".into(),
        };
        saves.save("slot1", &progress).unwrap();
        saves.save("settings", &42u32).unwrap();
        assert_eq!(saves.load::<Progress>("slot1").unwrap(), Some(progress));
        assert_eq!(saves.slots(), ["settings", "slot1"]);
        assert!(saves.exists("slot1"));
        assert!(matches!(
            saves.load::<Progress>("settings"),
            Err(SaveError::Format { .. })
        ));
        assert!(matches!(
            saves.save("../evil", &1),
            Err(SaveError::InvalidSlot(_))
        ));
        assert!(saves.delete("slot1").unwrap());
        assert!(!saves.delete("slot1").unwrap());
        assert!(!dir.join("slot1.json.tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn folder_names_are_safe() {
        assert_eq!(folder_name("Night Relics"), "Night Relics");
        assert_eq!(folder_name("a/b:c"), "a_b_c");
        assert_eq!(folder_name("  "), "April Blink Game");
        assert_eq!(SaveStore::for_game("X").dir().file_name().unwrap(), "X");
    }
}
