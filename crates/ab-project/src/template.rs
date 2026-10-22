//! New projects, from the template in `template/`.

use crate::{engine_root, Project};
use anyhow::Result;
use std::path::PathBuf;

include!(concat!(env!("OUT_DIR"), "/template.rs"));

/// A new scene: a sun, a floor and a camera (the template's main scene).
pub const NEW_SCENE: &str = include_str!("../template/assets/scenes/main.toml");

/// `My Cool Game!` → `my-cool-game`.
pub fn folder_name(title: &str) -> String {
    let mut name = String::new();
    for c in title.trim().chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c.to_ascii_lowercase());
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
    }
    let name = name.trim_end_matches('-');
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        format!("game-{name}")
    } else {
        name.to_owned()
    }
}

/// Where a new project goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProject {
    pub title: String,
    /// The chosen folder.
    pub location: PathBuf,
    /// Create `location/<folder name>` (true) or fill `location` itself, which must be empty.
    pub new_folder: bool,
}

/// Package names cargo or the engine already use.
const RESERVED: &[&str] = &[
    "ab-script",
    "ab-script-api",
    "serde",
    "test",
    "build",
    "xtask",
];

impl NewProject {
    /// The project's package name (also its folder name with [`Self::new_folder`]).
    pub fn package(&self) -> String {
        folder_name(&self.title)
    }

    /// The folder the project is created in.
    pub fn root(&self) -> PathBuf {
        if self.new_folder {
            self.location.join(self.package())
        } else {
            self.location.clone()
        }
    }

    /// Checks the title and the location; returns the package name.
    pub fn validate(&self) -> Result<String, String> {
        let title = self.title.trim();
        if title.is_empty() {
            return Err("Enter a name for the project.".into());
        }
        if title.chars().count() > 48 {
            return Err("The name is too long (48 characters at most).".into());
        }
        if title.contains(['"', '\\', '`']) || title.chars().any(char::is_control) {
            return Err("The name cannot contain quotes, backslashes or backticks.".into());
        }
        let name = self.package();
        if name.len() < 2 {
            return Err("Use at least two letters or digits (Latin) in the name.".into());
        }
        if name.starts_with("ab-") || RESERVED.contains(&name.as_str()) {
            return Err(format!("`{name}` is reserved; choose another name."));
        }
        if !self.location.is_absolute() {
            return Err("Choose the folder to create the project in.".into());
        }
        let root = self.root();
        if self.new_folder {
            if root.exists() {
                return Err(format!("A folder `{name}` already exists there."));
            }
        } else if std::fs::read_dir(&root).is_ok_and(|mut entries| entries.next().is_some()) {
            return Err(
                "The folder is not empty: choose an empty one, or create a new folder.".into(),
            );
        }
        Ok(name)
    }
}

/// Where the SDK's sources are: the engine's copy in development builds (so new projects
/// build against the engine they run in), else the copy shipped with the editor
/// (`sdk/ab-script` next to the program, made by `cargo xtask package-editor`).
pub fn sdk_dir() -> Option<PathBuf> {
    let shipped = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("sdk/ab-script")));
    engine_root()
        .map(|root| root.join("crates/ab-script"))
        .into_iter()
        .chain(shipped)
        .find(|dir| dir.join("Cargo.toml").is_file())
}

/// How the template depends on the SDK: its local sources ([`sdk_dir`]), else the
/// published crate.
fn sdk_dependency() -> String {
    match sdk_dir() {
        Some(path) => {
            let path = toml::Value::String(path.to_string_lossy().replace('\\', "/"));
            format!("{{ path = {path} }}")
        }
        None => format!("\"{}\"", env!("CARGO_PKG_VERSION")),
    }
}

/// Replaces the template's placeholders.
fn fill(text: &str, package: &str, title: &str) -> String {
    text.replace("{{package}}", package)
        .replace("{{crate}}", &package.replace('-', "_"))
        .replace("{{title}}", title)
        .replace("{{ab_script}}", &sdk_dependency())
}

/// Creates a project from the template: a floor, a light and a camera, an input map, and a
/// script crate with nothing registered yet. On failure the location is left as it was.
pub fn create(new: &NewProject) -> Result<Project> {
    let name = new.validate().map_err(anyhow::Error::msg)?;
    let root = new.root();
    let created_root = !root.exists();
    let result = (|| -> Result<Project> {
        for (relative, bytes) in TEMPLATE {
            let dest = root.join(relative);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            match std::str::from_utf8(bytes) {
                Ok(text) => std::fs::write(&dest, fill(text, &name, new.title.trim()))?,
                Err(_) => std::fs::write(&dest, bytes)?,
            }
        }
        Project::open(&root)
    })();
    result.map_err(|err| {
        if created_root {
            let _ = std::fs::remove_dir_all(&root);
        } else {
            let mut tops: Vec<&str> = TEMPLATE
                .iter()
                .filter_map(|(relative, _)| relative.split('/').next())
                .collect();
            tops.dedup();
            for top in tops {
                let path = root.join(top);
                let _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
            }
        }
        err.context(format!("creating {}", root.display()))
    })
}

/// The template's file `relative` (tests, tools).
pub fn template_file(relative: &str) -> Option<&'static [u8]> {
    TEMPLATE
        .iter()
        .find(|(path, _)| *path == relative)
        .map(|(_, bytes)| *bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ab-project-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn new(title: &str, location: &Path) -> NewProject {
        NewProject {
            title: title.into(),
            location: location.to_path_buf(),
            new_folder: true,
        }
    }

    #[test]
    fn folder_names() {
        assert_eq!(folder_name("My Cool Game!"), "my-cool-game");
        assert_eq!(folder_name("  Tank  3D "), "tank-3d");
        assert_eq!(folder_name("3D Racer"), "game-3d-racer");
        assert_eq!(folder_name("เกมผี"), "");
    }

    #[test]
    fn validation() {
        let dir = temp("validate");
        let check = |title: &str| new(title, &dir).validate();
        assert!(check("").is_err());
        assert!(check("เกมผี").is_err());
        assert!(check("AB Engine").is_err());
        assert!(check("Serde").is_err());
        assert!(check("Say \"hi\"").is_err());
        assert_eq!(check("Ghost House").unwrap(), "ghost-house");
        assert!(new("Ghost House", Path::new("relative"))
            .validate()
            .is_err());
        std::fs::create_dir_all(dir.join("ghost-house")).unwrap();
        assert!(check("Ghost House").is_err(), "the folder exists");
        let in_place = |location: &Path| NewProject {
            new_folder: false,
            ..new("Ghost House", location)
        };
        assert_eq!(
            in_place(&dir.join("ghost-house")).root(),
            dir.join("ghost-house")
        );
        assert!(in_place(&dir.join("ghost-house")).validate().is_ok());
        assert!(in_place(&dir).validate().is_err(), "not empty");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn creates_a_standalone_project() {
        let dir = temp("create");
        let project = create(&new("Blank Slate", &dir)).unwrap();
        assert_eq!(project.title, "Blank Slate");
        assert_eq!(project.package.as_deref(), Some("blank-slate"));
        assert_eq!(project.main_scene.as_deref(), Some("scenes/main.toml"));
        assert!(
            !project.assets.join("game.toml").exists(),
            "project files stay out of the assets"
        );
        assert_eq!(project.input.as_deref(), Some("input.toml"));
        assert_eq!(project.assets, project.root.join("assets"));
        for file in [
            "Cargo.toml",
            "game.toml",
            "src/lib.rs",
            "src/scripts/mod.rs",
            ".gitignore",
        ] {
            let text = std::fs::read_to_string(project.root.join(file)).unwrap();
            assert!(!text.contains("{{"), "{file} keeps a placeholder");
        }
        let manifest: toml::Table = std::fs::read_to_string(project.root.join("Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(manifest.contains_key("workspace"), "its own workspace");
        let sdk = &manifest["dependencies"]["ab-script"];
        let sdk_path = sdk.get("path").and_then(|p| p.as_str()).unwrap();
        assert!(
            Path::new(sdk_path).join("Cargo.toml").is_file(),
            "the engine's SDK in development: {sdk}"
        );
        let library = project.editor_library().unwrap();
        assert!(library
            .to_string_lossy()
            .ends_with(&crate::Platform::host().library("blank_slate")));
        assert_eq!(project.version(), "0.1.0");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn creates_into_an_empty_chosen_folder_and_cleans_up_failures() {
        let dir = temp("in-place");
        let folder = dir.join("Chosen");
        std::fs::create_dir_all(&folder).unwrap();
        let in_place = NewProject {
            new_folder: false,
            ..new("Inside", &folder)
        };
        let project = create(&in_place).unwrap();
        assert_eq!(project.root, folder.canonicalize().unwrap());
        assert!(create(&in_place).is_err(), "not empty any more");
        assert!(folder.join("game.toml").is_file(), "left as it was");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn opens_from_the_assets_folder_and_rejects_others() {
        let dir = temp("open");
        let project = create(&new("Coin Rush", &dir)).unwrap();
        let again = Project::open(&project.root.join("assets")).unwrap();
        assert_eq!(again.root, project.root);
        assert!(Project::open(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn projects_of_the_first_layout_move_their_files_out_of_the_assets() {
        let dir = temp("old-layout");
        let project = create(&new("Old Times", &dir)).unwrap();
        let root = project.root.clone();
        for name in ["game.toml", "input.toml"] {
            std::fs::rename(root.join(name), root.join("assets").join(name)).unwrap();
        }
        // Opened from its assets folder, as the editor used to.
        let moved = Project::open(&root.join("assets")).unwrap();
        assert_eq!(moved, project);
        assert!(root.join("game.toml").is_file() && root.join("input.toml").is_file());
        assert!(!root.join("assets/game.toml").exists());
        assert!(!root.join("assets/input.toml").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn project_files_are_the_settings_and_the_input_map() {
        let dir = temp("project-files");
        let project = create(&new("Packed", &dir)).unwrap();
        let names: Vec<_> = project
            .project_files()
            .into_iter()
            .map(|(name, file)| {
                assert!(file.is_file());
                name
            })
            .collect();
        assert_eq!(names, ["game.toml", "input.toml"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_new_scene_is_the_template_scene() {
        assert_eq!(
            template_file("assets/scenes/main.toml"),
            Some(NEW_SCENE.as_bytes())
        );
    }
}
