//! The project's asset directory, indexed for the asset browser and menus.

use ab_engine::core::jobs::{self, Task};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssetKind {
    Folder,
    Scene,
    Model,
    Material,
    /// A custom surface shader (`.wgsl`).
    Shader,
    /// An animation state machine (`.abstate`).
    StateMachine,
    /// A rig: humanoid parts, joint limits, IK chains (`.abrig`).
    Rig,
    Image,
    Audio,
    /// A sound event: clips, variation, bus and 3D settings (`.absound`).
    SoundEvent,
    Font,
    Config,
    Other,
}

impl AssetKind {
    /// The kind of a file by its name.
    pub fn of(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("glb" | "gltf") => Self::Model,
            Some("png" | "jpg" | "jpeg") => Self::Image,
            Some("wav" | "ogg" | "mp3" | "flac") => Self::Audio,
            Some(ab_engine::audio::SOUND_EVENT_EXTENSION) => Self::SoundEvent,
            Some("ttf" | "otf") => Self::Font,
            Some(ab_engine::scene_file::SCENE_EXTENSION) => Self::Scene,
            Some(ab_engine::scene_file::MATERIAL_EXTENSION) => Self::Material,
            Some("wgsl") => Self::Shader,
            Some(ab_engine::state_machine::STATE_MACHINE_EXTENSION) => Self::StateMachine,
            Some(ab_engine::rig::RIG_EXTENSION) => Self::Rig,
            Some("toml" | "json") => Self::Config,
            _ => Self::Other,
        }
    }

    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as icon;
        match self {
            Self::Folder => icon::FOLDER_SIMPLE,
            Self::Scene => icon::FILM_SLATE,
            Self::Model => icon::CUBE,
            Self::Material => icon::DROP_HALF,
            Self::Shader => icon::SPARKLE,
            Self::StateMachine => icon::GRAPH,
            Self::Rig => icon::PERSON_SIMPLE,
            Self::Image => icon::IMAGE,
            Self::Audio => icon::WAVEFORM,
            Self::SoundEvent => icon::SPEAKER_HIGH,
            Self::Font => icon::TEXT_AA,
            Self::Config => icon::GEAR_SIX,
            Self::Other => icon::FILE,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AssetEntry {
    /// Relative to the project root, with `/` separators.
    pub path: String,
    pub name: String,
    pub kind: AssetKind,
}

/// Every file and folder under the project root, refreshed periodically. Refreshes scan
/// the disk on a background thread, so a large project never stalls the editor.
#[derive(Debug)]
pub struct AssetIndex {
    pub root: PathBuf,
    pub entries: Vec<AssetEntry>,
    scanned: Option<Instant>,
    scan: Option<Task<Vec<AssetEntry>>>,
}

impl AssetIndex {
    pub fn new(root: PathBuf) -> Self {
        let mut index = Self {
            root,
            entries: Vec::new(),
            scanned: None,
            scan: None,
        };
        index.rescan();
        index
    }

    /// Picks up a finished background scan, and starts one when the last scan is older
    /// than a couple of seconds.
    pub fn refresh(&mut self) {
        self.poll();
        if self.scan.is_none()
            && self
                .scanned
                .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
        {
            self.request_rescan();
        }
    }

    /// Starts a background scan now (unless one is running); [`AssetIndex::refresh`]
    /// applies it.
    pub fn request_rescan(&mut self) {
        if self.scan.is_none() {
            let root = self.root.clone();
            self.scan = Some(jobs::spawn_background("ab-asset-scan", move || scan(&root)));
        }
    }

    /// A background scan is running.
    pub fn is_scanning(&self) -> bool {
        self.scan.is_some()
    }

    fn poll(&mut self) {
        let Some(task) = &mut self.scan else {
            return;
        };
        if let Some(entries) = task.try_take() {
            self.entries = entries;
            self.scanned = Some(Instant::now());
            self.scan = None;
        }
    }

    /// Scans right away, on this thread.
    pub fn rescan(&mut self) {
        self.scan = None;
        self.entries = scan(&self.root);
        self.scanned = Some(Instant::now());
    }

    /// Every file and folder found.
    pub fn all(&self) -> impl Iterator<Item = &AssetEntry> {
        self.entries.iter()
    }

    pub fn of_kind(&self, kind: AssetKind) -> impl Iterator<Item = &AssetEntry> {
        self.entries.iter().filter(move |e| e.kind == kind)
    }
}

/// Every file and folder under `root` (hidden ones skipped), folders first.
fn scan(root: &Path) -> Vec<AssetEntry> {
    let mut entries = Vec::new();
    walk(root, root, &mut entries);
    entries
}

fn walk(root: &Path, dir: &Path, entries: &mut Vec<AssetEntry>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<_> = read.flatten().map(|e| e.path()).collect();
    // Folders first, then by name.
    items.sort_by_key(|p| (!p.is_dir(), p.file_name().map(|n| n.to_ascii_lowercase())));
    for path in items {
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        let rel = relative.to_string_lossy().replace('\\', "/");
        if path.is_dir() {
            entries.push(AssetEntry {
                path: rel,
                name,
                kind: AssetKind::Folder,
            });
            // A linked folder is listed, not entered: a link back up would never end.
            if !path.is_symlink() {
                walk(root, &path, entries);
            }
        } else {
            let kind = AssetKind::of(&path);
            entries.push(AssetEntry {
                path: rel,
                name,
                kind,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_the_sample_assets() {
        let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let index = AssetIndex::new(root);
        let find = |path: &str| {
            index
                .entries
                .iter()
                .find(|e| e.path == path)
                .map(|e| e.kind)
        };
        assert_eq!(find("models"), Some(AssetKind::Folder));
        assert_eq!(find("models/crate.glb"), Some(AssetKind::Model));
        assert_eq!(find("audio/thud.wav"), Some(AssetKind::Audio));
        assert_eq!(find("slice/level.abscene"), Some(AssetKind::Scene));
        assert_eq!(find("input.toml"), None, "project files are not assets");
        assert!(index.of_kind(AssetKind::Scene).count() >= 1);
    }

    #[test]
    fn background_rescans_pick_up_new_files() {
        let dir = std::env::temp_dir().join(format!("ab-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut index = AssetIndex::new(dir.clone());
        assert!(index.entries.is_empty());
        std::fs::write(dir.join("level.abscene"), "[[node]]\nname = \"Root\"\n").unwrap();
        index.request_rescan();
        assert!(index.is_scanning());
        let start = Instant::now();
        while index.is_scanning() {
            assert!(start.elapsed() < Duration::from_secs(5), "scan hung");
            std::thread::sleep(Duration::from_millis(2));
            index.refresh();
        }
        assert_eq!(index.of_kind(AssetKind::Scene).count(), 1);
        // Scenes by extension, without looking inside.
        std::fs::write(dir.join("empty.abscene"), "").unwrap();
        std::fs::write(dir.join("clip.toml"), "[[track]]\n").unwrap();
        index.rescan();
        let mut scenes: Vec<_> = index
            .of_kind(AssetKind::Scene)
            .map(|a| a.path.clone())
            .collect();
        scenes.sort();
        assert_eq!(scenes, ["empty.abscene", "level.abscene"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
