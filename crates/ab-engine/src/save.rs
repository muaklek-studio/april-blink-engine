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
//! When the saved type changes over the game's life, save with a version and upgrade older
//! saves as they load (a save from before versions were used is version 0):
//!
//! ```no_run
//! # use ab_engine::save::SaveStore;
//! # #[derive(serde::Serialize, serde::Deserialize, Default)]
//! # struct Progress { level: u32, coins: u32 }
//! # fn demo(saves: &SaveStore) -> Result<(), ab_engine::save::SaveError> {
//! saves.save_versioned("slot1", 2, &Progress { level: 3, coins: 120 })?;
//! let progress: Option<Progress> = saves.load_versioned("slot1", 2, |from, data| {
//!     match from {
//!         0 => data["coins"] = 0.into(),       // version 1 added coins
//!         1 => data["level"] = data["stage"].take(), // version 2 renamed stage
//!         _ => {}
//!     }
//!     Ok(())
//! })?;
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
    #[error("save slot `{slot}` is version {found}, newer than this game's {supported}")]
    Newer {
        slot: String,
        found: u32,
        supported: u32,
    },
    #[error("save slot `{slot}` cannot be upgraded from version {from}: {reason}")]
    Upgrade {
        slot: String,
        from: u32,
        reason: String,
    },
}

/// The key that marks a versioned save (`{ "ab_save_version": 2, "data": … }`).
const VERSION_KEY: &str = "ab_save_version";

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
        // Same rules on every platform, so saves copied between PCs stay valid.
        let valid = !slot.is_empty()
            && slot.len() <= 64
            && slot
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            && !is_reserved_name(slot);
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
        ab_core::fs::atomic_write(&path, text).map_err(io)
    }

    /// The value in `slot`, `None` if nothing was saved there.
    pub fn load<T: DeserializeOwned>(&self, slot: &str) -> Result<Option<T>, SaveError> {
        let Some(bytes) = self.read(slot)? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| SaveError::Format {
                slot: slot.to_owned(),
                source,
            })
    }

    /// Like [`SaveStore::save`], marked with `version`: the version of `T`'s layout, for
    /// [`SaveStore::load_versioned`] to upgrade saves of older versions.
    pub fn save_versioned<T: Serialize>(
        &self,
        slot: &str,
        version: u32,
        value: &T,
    ) -> Result<(), SaveError> {
        let data = serde_json::to_value(value).map_err(|source| SaveError::Format {
            slot: slot.to_owned(),
            source,
        })?;
        self.save(
            slot,
            &serde_json::json!({ VERSION_KEY: version, "data": data }),
        )
    }

    /// The value in `slot` as version `version` of `T`. An older save is upgraded first:
    /// `upgrade(v, data)` turns version `v` data into version `v + 1`, for each version in
    /// between. Saves made with [`SaveStore::save`] are version 0; a save newer than
    /// `version` is an error ([`SaveError::Newer`]).
    pub fn load_versioned<T: DeserializeOwned>(
        &self,
        slot: &str,
        version: u32,
        mut upgrade: impl FnMut(u32, &mut serde_json::Value) -> Result<(), String>,
    ) -> Result<Option<T>, SaveError> {
        let Some(value) = self.load::<serde_json::Value>(slot)? else {
            return Ok(None);
        };
        let (found, mut data) = split_version(value);
        if found > version {
            return Err(SaveError::Newer {
                slot: slot.to_owned(),
                found,
                supported: version,
            });
        }
        for from in found..version {
            upgrade(from, &mut data).map_err(|reason| SaveError::Upgrade {
                slot: slot.to_owned(),
                from,
                reason,
            })?;
        }
        serde_json::from_value(data)
            .map(Some)
            .map_err(|source| SaveError::Format {
                slot: slot.to_owned(),
                source,
            })
    }

    /// The bytes of `slot`, `None` if nothing was saved there.
    fn read(&self, slot: &str) -> Result<Option<Vec<u8>>, SaveError> {
        let path = self.path(slot)?;
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(SaveError::Io { path, source }),
        }
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

/// The version and data of a save: `(0, value)` for one saved without a version.
fn split_version(value: serde_json::Value) -> (u32, serde_json::Value) {
    if let serde_json::Value::Object(mut map) = value {
        let version = map.get(VERSION_KEY).and_then(|v| v.as_u64());
        if let (Some(version), 2, true) = (version, map.len(), map.contains_key("data")) {
            let version = u32::try_from(version).unwrap_or(u32::MAX);
            return (version, map.remove("data").unwrap_or_default());
        }
        return (0, serde_json::Value::Object(map));
    }
    (0, value)
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
    } else if is_reserved_name(trimmed) {
        format!("{trimmed}_")
    } else {
        trimmed.into()
    }
}

/// Names Windows keeps for devices (`CON`, `nul.txt`, `COM1`, …), whatever the case or
/// extension: a file or folder named so cannot be made there.
fn is_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            // By characters: a title in Cyrillic or Greek has no byte boundary at 3.
            let mut chars = upper.chars();
            let prefix: String = chars.by_ref().take(3).collect();
            let digit: Vec<char> = chars.collect();
            matches!(prefix.as_str(), "COM" | "LPT") && matches!(digit.as_slice(), ['1'..='9'])
        }
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
        assert!(!dir
            .join(format!(".slot1.json.{}.tmp", std::process::id()))
            .exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn versioned_saves_upgrade_and_refuse_newer() {
        let dir = std::env::temp_dir().join(format!("ab-saves-versioned-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let saves = SaveStore::in_dir(&dir);
        // Version 1 called the level `stage`; version 2 renamed it and added `name`.
        let upgrade = |from: u32, data: &mut serde_json::Value| {
            match from {
                0 => return Err("too old".to_owned()),
                1 => {
                    data["level"] = data["stage"].take();
                    data["name"] = "?".into();
                    data.as_object_mut().unwrap().remove("stage");
                }
                _ => {}
            }
            Ok(())
        };
        saves
            .save_versioned("old", 1, &serde_json::json!({ "stage": 4 }))
            .unwrap();
        let progress: Progress = saves.load_versioned("old", 2, upgrade).unwrap().unwrap();
        assert_eq!(
            progress,
            Progress {
                level: 4,
                name: "?".into()
            }
        );

        let current = Progress {
            level: 7,
            name: "ก".into(),
        };
        saves.save_versioned("now", 2, &current).unwrap();
        assert_eq!(
            saves.load_versioned("now", 2, upgrade).unwrap(),
            Some(current)
        );
        assert!(matches!(
            saves.load_versioned::<Progress>("now", 1, upgrade),
            Err(SaveError::Newer {
                found: 2,
                supported: 1,
                ..
            })
        ));

        // A save from before versions is version 0, and goes through every upgrade.
        saves
            .save("legacy", &serde_json::json!({ "stage": 1 }))
            .unwrap();
        assert!(matches!(
            saves.load_versioned::<Progress>("legacy", 2, upgrade),
            Err(SaveError::Upgrade { from: 0, .. })
        ));
        assert_eq!(
            saves
                .load_versioned::<Progress>("none", 2, upgrade)
                .unwrap(),
            None
        );
        assert_eq!(
            split_version(serde_json::json!(5)),
            (0, serde_json::json!(5))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn folder_names_are_safe() {
        assert_eq!(folder_name("Night Relics"), "Night Relics");
        assert_eq!(folder_name("a/b:c"), "a_b_c");
        assert_eq!(folder_name("  "), "April Blink Game");
        assert_eq!(folder_name("CON"), "CON_");
        assert_eq!(folder_name("Aux"), "Aux_");
        assert_eq!(folder_name("Console"), "Console");
        assert_eq!(SaveStore::for_game("X").dir().file_name().unwrap(), "X");
    }

    /// Windows device names are refused as slots on every platform (saves stay portable).
    #[test]
    fn reserved_slot_names_are_refused() {
        let store = SaveStore::in_dir(std::env::temp_dir().join("ab-reserved-slots"));
        for slot in [
            "con", "CON", "Nul", "prn", "aux", "com1", "COM9", "lpt3", "LPT9",
        ] {
            assert!(
                matches!(store.save(slot, &1), Err(SaveError::InvalidSlot(_))),
                "{slot}"
            );
            assert!(!store.exists(slot));
        }
        for name in ["nul.json", "CON.txt", "aux ", "com1.tar.gz"] {
            assert!(is_reserved_name(name), "{name}");
        }
        for name in [
            "console",
            "com",
            "com0",
            "com10",
            "lpt",
            "null",
            "slot1",
            "Игра",
            "ÄÖÜ",
            "เกม",
        ] {
            assert!(!is_reserved_name(name), "{name}");
        }
    }
}
