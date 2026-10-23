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
        let mut recent: Self = store.load(RECENT_SLOT)?.unwrap_or_default();
        // Settings written on Windows used to keep canonical paths with the `\\?\` prefix.
        for project in &mut recent.projects {
            project.path = ab_project::path::without_verbatim(&project.path);
        }
        if let Some(dir) = &mut recent.last_location {
            *dir = ab_project::path::without_verbatim(dir);
        }
        Ok(recent)
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
            package: None,
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
    fn saved_windows_verbatim_paths_load_without_the_prefix() {
        let dir = temp("verbatim");
        let store = SaveStore::in_dir(&dir);
        let saved = RecentProjects {
            projects: vec![RecentProject {
                path: PathBuf::from(r"\\?\D:\Games\My Game"),
                title: "My Game".to_owned(),
                last_opened: 0,
            }],
            last_location: Some(PathBuf::from(r"\\?\D:\Games")),
        };
        saved.save(&store).unwrap();
        let loaded = RecentProjects::load(&store).unwrap();
        assert_eq!(loaded.projects[0].path, Path::new(r"D:\Games\My Game"));
        assert_eq!(
            loaded.last_location.as_deref(),
            Some(Path::new(r"D:\Games"))
        );
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
