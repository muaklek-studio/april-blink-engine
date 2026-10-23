//! Finding the engine's source tree.
//!
//! Development builds use the engine checkout for the script SDK and to build players.
//! Where it is comes from, in order:
//!
//! 1. `AB_ENGINE_ROOT`: another checkout, or empty to ignore the sources (as on a game
//!    maker's machine). It is explicit, so an invalid value is reported, not skipped.
//! 2. The folder chosen with [`set_engine_root`]: the editor remembers the last valid one,
//!    or the user picked it.
//! 3. The checkout the program was built from, if it is still there.
//! 4. A checkout around the running program or the current folder (the editor started from
//!    `target/debug` of a moved checkout).
//!
//! Every candidate is validated with [`is_engine_root`]; an invalid folder is never used.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// The folder set by [`set_engine_root`].
static CHOSEN: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Where the engine sources were found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineSource {
    /// `AB_ENGINE_ROOT`.
    Environment,
    /// [`set_engine_root`]: remembered from an earlier run, or picked by the user.
    Chosen,
    /// The checkout this program was built from.
    Build,
    /// Found next to the running program or the current folder.
    Nearby,
}

/// The engine's source tree, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineLookup {
    Found {
        root: PathBuf,
        source: EngineSource,
    },
    /// `AB_ENGINE_ROOT` is empty: the sources are ignored on purpose.
    Disabled,
    /// `AB_ENGINE_ROOT` names a folder that is not an engine checkout.
    InvalidEnvironment(PathBuf),
    NotFound,
}

impl EngineLookup {
    pub fn root(&self) -> Option<&Path> {
        match self {
            Self::Found { root, .. } => Some(root),
            _ => None,
        }
    }
}

/// Whether `dir` is an April Blink engine checkout: a cargo workspace with the engine and
/// the script SDK.
pub fn is_engine_root(dir: &Path) -> bool {
    let workspace = std::fs::read_to_string(dir.join("Cargo.toml"))
        .is_ok_and(|text| text.contains("[workspace]") && text.contains("ab-engine"));
    workspace
        && dir.join("crates/ab-engine/Cargo.toml").is_file()
        && dir.join("crates/ab-script/Cargo.toml").is_file()
}

/// Uses `dir` for the engine sources from now on (below `AB_ENGINE_ROOT`). Refused, and
/// nothing changes, when it is not an engine checkout. `None` forgets the choice.
pub fn set_engine_root(dir: Option<&Path>) -> Result<(), String> {
    let chosen = match dir {
        Some(dir) if is_engine_root(dir) => Some(canonical(dir)),
        Some(dir) => {
            return Err(format!(
                "{} is not an April Blink engine checkout (it needs Cargo.toml, \
                 crates/ab-engine and crates/ab-script)",
                dir.display()
            ))
        }
        None => None,
    };
    *CHOSEN.write().unwrap_or_else(|e| e.into_inner()) = chosen;
    Ok(())
}

/// Looks for the engine sources (see the module docs).
pub fn locate_engine() -> EngineLookup {
    let exe = std::env::current_exe().ok();
    let cwd = std::env::current_dir().ok();
    let chosen = CHOSEN.read().unwrap_or_else(|e| e.into_inner()).clone();
    locate(
        std::env::var_os("AB_ENGINE_ROOT").map(PathBuf::from),
        chosen.as_deref(),
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        exe.as_deref().and_then(Path::parent),
        cwd.as_deref(),
    )
}

fn locate(
    environment: Option<PathBuf>,
    chosen: Option<&Path>,
    build: &Path,
    exe_dir: Option<&Path>,
    cwd: Option<&Path>,
) -> EngineLookup {
    match environment {
        Some(dir) if dir.as_os_str().is_empty() => return EngineLookup::Disabled,
        Some(dir) if is_engine_root(&dir) => {
            return EngineLookup::Found {
                root: canonical(&dir),
                source: EngineSource::Environment,
            }
        }
        Some(dir) => return EngineLookup::InvalidEnvironment(dir),
        None => {}
    }
    let found = |root: &Path, source| EngineLookup::Found {
        root: canonical(root),
        source,
    };
    if let Some(dir) = chosen.filter(|d| is_engine_root(d)) {
        return found(dir, EngineSource::Chosen);
    }
    if is_engine_root(build) {
        return found(build, EngineSource::Build);
    }
    // `target/debug/ab-editor` of a checkout, or started from somewhere inside one.
    let nearby = [exe_dir, cwd]
        .into_iter()
        .flatten()
        .flat_map(|dir| dir.ancestors().take(6))
        .find(|dir| is_engine_root(dir));
    match nearby {
        Some(dir) => found(dir, EngineSource::Nearby),
        None => EngineLookup::NotFound,
    }
}

/// The engine's source tree, if it can be found (see [`locate_engine`]): new projects then
/// use its SDK, and players can be built from it.
pub fn engine_root() -> Option<PathBuf> {
    match locate_engine() {
        EngineLookup::Found { root, .. } => Some(root),
        _ => None,
    }
}

fn canonical(dir: &Path) -> PathBuf {
    crate::path::canonicalize_lossy(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout(dir: &Path) {
        for krate in ["ab-engine", "ab-script"] {
            std::fs::create_dir_all(dir.join("crates").join(krate)).unwrap();
            std::fs::write(dir.join("crates").join(krate).join("Cargo.toml"), "").unwrap();
        }
        std::fs::write(
            dir.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/ab-engine\"]\n",
        )
        .unwrap();
    }

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ab-engine-root-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn this_checkout_is_an_engine_root() {
        let here = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        assert!(is_engine_root(here));
        assert!(!is_engine_root(&here.join("crates")));
        assert!(!is_engine_root(Path::new("/nonexistent/engine")));
    }

    #[test]
    fn candidates_in_order() {
        let base = temp("order");
        let (a, b, gone) = (base.join("a"), base.join("b"), base.join("gone"));
        checkout(&a);
        checkout(&b);
        let found = |root: &Path, source| EngineLookup::Found {
            root: canonical(root),
            source,
        };

        // The environment wins, and an invalid one is reported, not skipped.
        assert_eq!(
            locate(Some(a.clone()), Some(&b), &b, None, None),
            found(&a, EngineSource::Environment)
        );
        assert_eq!(
            locate(Some(gone.clone()), Some(&b), &b, None, None),
            EngineLookup::InvalidEnvironment(gone.clone())
        );
        assert_eq!(
            locate(Some(PathBuf::new()), Some(&b), &b, None, None),
            EngineLookup::Disabled
        );
        // The remembered folder, then the build's.
        assert_eq!(
            locate(None, Some(&b), &a, None, None),
            found(&b, EngineSource::Chosen)
        );
        // A remembered folder that moved away falls back to the build's checkout.
        assert_eq!(
            locate(None, Some(&gone), &a, None, None),
            found(&a, EngineSource::Build)
        );
        // The build's checkout moved: found around the program.
        let exe_dir = b.join("target/debug");
        std::fs::create_dir_all(&exe_dir).unwrap();
        assert_eq!(
            locate(None, Some(&gone), &gone, Some(&exe_dir), None),
            found(&b, EngineSource::Nearby)
        );
        assert_eq!(
            locate(None, None, &gone, Some(&base), Some(&base)),
            EngineLookup::NotFound
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn invalid_choices_are_refused() {
        let base = temp("refused");
        let err = set_engine_root(Some(&base)).unwrap_err();
        assert!(err.contains("not an April Blink engine checkout"), "{err}");
        let _ = std::fs::remove_dir_all(&base);
    }
}
