//! Reading a project.

use crate::Platform;
use anyhow::{bail, Context as _, Result};
use std::path::{Path, PathBuf};

/// How a script library is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Profile {
    /// While editing: compiles fast.
    Debug,
    /// For exported games.
    Release,
}

impl Profile {
    /// Cargo's output folder for it.
    pub fn dir(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }
}

/// The project's settings file, in the project folder.
pub const GAME_FILE: &str = "game.toml";

/// A game project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// The project folder.
    pub root: PathBuf,
    pub title: String,
    /// Asset directory (`root/assets`, or the folder `game.toml` names).
    pub assets: PathBuf,
    /// Scene opened first, relative to the assets.
    pub main_scene: Option<String>,
    /// The input map, relative to the project folder (`input.toml`).
    pub input: Option<String>,
    /// The game's named values, relative to the project folder (`config.toml`).
    pub config: Option<String>,
    /// Cargo package of the scripts (`None` for a project without scripts).
    pub package: Option<String>,
    /// Its packed assets go to Git LFS (`asset_lfs` in `game.toml`, on unless `false`; see
    /// [`crate::asset_store`]).
    pub lfs: bool,
}

impl Project {
    /// Reads the project in `path` (the project folder, or its `assets` folder).
    pub fn open(path: &Path) -> Result<Self> {
        let mut root = crate::path::canonicalize(path)
            .with_context(|| format!("{} does not exist", path.display()))?;
        let is_project = |dir: &Path| {
            dir.join(GAME_FILE).is_file()
                && !(dir.file_name().is_some_and(|n| n == "assets")
                    && dir.parent().is_some_and(|p| p.join("Cargo.toml").is_file()))
        };
        if !is_project(&root) {
            if let Some(parent) = root.parent().filter(|p| is_project(p)) {
                root = parent.to_path_buf();
            }
        }
        let settings_path = root.join(GAME_FILE);
        if !settings_path.is_file() {
            bail!(
                "{} is not an April Blink project (no {GAME_FILE})",
                root.display()
            );
        }
        let settings = read_settings(&settings_path)?;
        let text = |key: &str| {
            settings
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let package = read_manifest(&root.join("Cargo.toml"))
            .ok()
            .and_then(|manifest| package_field(&manifest, "name"));
        let folder = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self {
            assets: root.join(text("assets").unwrap_or_else(|| "assets".to_owned())),
            title: text("title").unwrap_or(folder),
            main_scene: text("main_scene"),
            input: text("input"),
            config: text("config"),
            lfs: settings
                .get("asset_lfs")
                .and_then(toml::Value::as_bool)
                .unwrap_or(true),
            package,
            root,
        })
    }

    /// The project's own files, packed apart from the assets in exports: `game.toml` and the
    /// input map, as (path relative to the project folder, file).
    pub fn project_files(&self) -> Vec<(String, PathBuf)> {
        std::iter::once(GAME_FILE.to_owned())
            .chain(self.input.clone())
            .chain(self.config.clone())
            .map(|name| (name.replace('\\', "/"), self.root.join(&name)))
            .collect()
    }

    /// The crate name of the scripts (`my-game` → `my_game`).
    pub fn crate_name(&self) -> Option<String> {
        Some(self.package.as_ref()?.replace('-', "_"))
    }

    /// Cargo's target directory for the project.
    pub fn target_dir(&self) -> PathBuf {
        std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .unwrap_or_else(|| self.root.join("target"))
    }

    /// The script library cargo builds for `profile` (and `triple`, when cross-compiling),
    /// built or not.
    pub fn library(&self, profile: Profile, triple: Option<&str>) -> Option<PathBuf> {
        let platform = triple.map_or_else(Platform::host, Platform::of_triple);
        let mut dir = self.target_dir();
        if let Some(triple) = triple {
            dir.push(triple);
        }
        Some(
            dir.join(profile.dir())
                .join(platform.library(&self.crate_name()?)),
        )
    }

    /// The library the editor loads while editing.
    pub fn editor_library(&self) -> Option<PathBuf> {
        self.library(Profile::Debug, None)
    }

    /// Script sources, watched for changes while editing.
    pub fn sources(&self) -> PathBuf {
        self.root.join("src")
    }

    /// The version in `Cargo.toml` (`0.1.0` if it has none).
    pub fn version(&self) -> String {
        read_manifest(&self.root.join("Cargo.toml"))
            .ok()
            .and_then(|manifest| package_field(&manifest, "version"))
            .unwrap_or_else(|| "0.1.0".to_owned())
    }
}

fn read_settings(path: &Path) -> Result<toml::Table> {
    std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?
        .parse()
        .with_context(|| format!("invalid {}", path.display()))
}

fn read_manifest(path: &Path) -> Result<toml::Table> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(text.parse()?)
}

fn package_field(manifest: &toml::Table, field: &str) -> Option<String> {
    manifest
        .get("package")?
        .get(field)?
        .as_str()
        .map(str::to_owned)
}
