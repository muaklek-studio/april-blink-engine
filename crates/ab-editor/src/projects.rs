//! Projects: what the project manager lists, creates and opens.
//!
//! A project is its own cargo workspace with a script library built on the `ab-script` SDK
//! (see [`ab_project`]), anywhere on disk. The editor loads the library through the script
//! ABI, so it needs neither the engine's sources nor the same Rust toolchain.

use ab_engine::save::SaveStore;
pub use ab_project::{create, engine_root, BuildState, NewProject, Profile, Project, ScriptBuild};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The default place for new projects: `April Blink Projects` in the user's home folder.
pub fn projects_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join("April Blink Projects"))
}

/// The project data folder (`.ablink`) of the project whose assets are `assets`; `None` for
/// an asset folder outside a project.
pub fn data_dir(assets: &Path) -> Option<PathBuf> {
    Project::open(assets).ok().map(|project| project.data_dir())
}

/// Removes the oldest files of `dir` (by last change) until it holds at most `max_bytes`.
/// Returns the bytes freed.
pub fn trim_cache(dir: &Path, max_bytes: u64) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let metadata = entry.metadata().ok().filter(|m| m.is_file())?;
            Some((
                metadata.modified().unwrap_or(UNIX_EPOCH),
                metadata.len(),
                entry.path(),
            ))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    files.sort();
    let mut freed = 0;
    for (_, size, path) in files {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= size;
            freed += size;
        }
    }
    freed
}

// =============================================================================================
// Recent projects
// =============================================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: PathBuf,
    pub title: String,
    /// Seconds since the Unix epoch.
    pub last_opened: u64,
}

/// The projects the manager lists, most recently opened first. Stored with the editor's
/// settings (`SaveStore` "April Blink Editor", slot `projects`).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RecentProjects {
    pub projects: Vec<RecentProject>,
    /// Where the last new project was created; the next one starts there.
    #[serde(default)]
    pub last_location: Option<PathBuf>,
}

const RECENT_SLOT: &str = "projects";

impl RecentProjects {
    pub fn store() -> SaveStore {
        SaveStore::for_game("April Blink Editor")
    }

    pub fn load(store: &SaveStore) -> Result<Self> {
        Ok(store.load(RECENT_SLOT)?.unwrap_or_default())
    }

    pub fn save(&self, store: &SaveStore) -> Result<()> {
        store.save(RECENT_SLOT, self)?;
        Ok(())
    }

    /// Puts `project` first.
    pub fn touch(&mut self, project: &Project) {
        self.projects.retain(|p| p.path != project.root);
        self.projects.insert(
            0,
            RecentProject {
                path: project.root.clone(),
                title: project.title.clone(),
                last_opened: now(),
            },
        );
    }

    /// Adds `project` at the end, as never opened, unless it is listed already.
    pub fn add(&mut self, project: &Project) {
        if !self.contains(&project.root) {
            self.projects.push(RecentProject {
                path: project.root.clone(),
                title: project.title.clone(),
                last_opened: 0,
            });
        }
    }

    pub fn remove(&mut self, path: &Path) {
        self.projects.retain(|p| p.path != path);
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.projects.iter().any(|p| p.path == path)
    }
}

/// Projects in the default projects folder ([`projects_dir`]) that are not in the list yet
/// (to import).
pub fn unlisted_projects(recent: &RecentProjects) -> Vec<Project> {
    let Some(dir) = projects_dir() else {
        return Vec::new();
    };
    let mut found: Vec<Project> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| Project::open(&entry.path()).ok())
        .filter(|p| !recent.contains(&p.root))
        .collect();
    found.sort_by(|a, b| a.title.cmp(&b.title));
    found
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// "just now", "5 minutes ago", "3 days ago".
pub fn ago(then: u64, now: u64) -> String {
    if then == 0 {
        return "never opened".to_owned();
    }
    let seconds = now.saturating_sub(then);
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => plural(seconds / 60, "minute"),
        3600..86_400 => plural(seconds / 3600, "hour"),
        86_400..2_592_000 => plural(seconds / 86_400, "day"),
        _ => plural(seconds / 2_592_000, "month"),
    }
}

/// Newest modification time of the files under `dir`.
pub fn newest_change(dir: &Path) -> Option<SystemTime> {
    let mut newest = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                newest = newest.max(Some(modified));
            }
        }
    }
    newest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_project_settings_are_what_the_engine_reads() {
        let text = std::str::from_utf8(ab_project::template_file("game.toml").unwrap()).unwrap();
        let settings = ab_engine::game::GameSettings::from_toml(text)
            .expect("every key of the template's game.toml is one the engine knows");
        assert_eq!(settings.asset_lfs, Some(true));
    }

    #[test]
    fn trimming_a_cache_drops_the_oldest_files_first() {
        let dir = std::env::temp_dir().join(format!("ab-trim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, age) in [("old", 300), ("mid", 200), ("new", 100)] {
            let file = dir.join(name);
            std::fs::write(&file, [0u8; 100]).unwrap();
            let time = SystemTime::now() - std::time::Duration::from_secs(age);
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(time)
                .unwrap();
        }
        assert_eq!(trim_cache(&dir, 250), 100);
        assert!(!dir.join("old").exists() && dir.join("mid").exists());
        assert_eq!(trim_cache(&dir, 1000), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ab-editor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn recent_list_keeps_order_and_persists() {
        let dir = temp("recent");
        let store = SaveStore::in_dir(&dir);
        let project = |name: &str| Project {
            root: dir.join(name),
            title: name.to_owned(),
            assets: dir.join(name).join("assets"),
            main_scene: None,
            input: None,
            config: None,
            audio: None,
            package: None,
            lfs: true,
            export: Default::default(),
        };
        let mut recent = RecentProjects::load(&store).unwrap();
        assert!(recent.projects.is_empty());
        recent.touch(&project("a"));
        recent.touch(&project("b"));
        recent.touch(&project("a"));
        recent.save(&store).unwrap();
        let mut loaded = RecentProjects::load(&store).unwrap();
        let titles: Vec<_> = loaded.projects.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, ["a", "b"]);
        loaded.remove(&dir.join("a"));
        assert!(!loaded.contains(&dir.join("a")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn relative_times() {
        assert_eq!(ago(0, 110), "never opened");
        assert_eq!(ago(100, 110), "just now");
        assert_eq!(ago(1, 61), "1 minute ago");
        assert_eq!(ago(1, 7201), "2 hours ago");
        assert_eq!(ago(1, 3 * 86_400 + 1), "3 days ago");
    }
}
