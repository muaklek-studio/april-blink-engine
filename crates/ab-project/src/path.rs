//! Paths as the editor, saved settings and generated files should see them.
//!
//! `std::fs::canonicalize` on Windows returns a *verbatim* path (`\\?\D:\Games\…`, or
//! `\\?\UNC\server\share\…`). File APIs accept those, but they look wrong in the interface
//! and in saved settings, and turning their separators into `/` gives `//?/D:/…`, which
//! other programs (cargo, editors) may not understand. The helpers here make paths absolute
//! without that prefix.

use std::path::{Path, PathBuf};

/// [`std::fs::canonicalize`], without Windows' verbatim prefix.
pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path).map(|path| without_verbatim(&path))
}

/// [`canonicalize`], or `path` itself when it cannot be canonicalized (usually because it
/// does not exist).
pub fn canonicalize_lossy(path: &Path) -> PathBuf {
    canonicalize(path).unwrap_or_else(|_| without_verbatim(path))
}

/// `path` without Windows' verbatim prefix: `\\?\D:\Games` → `D:\Games` and
/// `\\?\UNC\server\share` → `\\server\share`. Everywhere else (and for any other prefix,
/// such as a volume GUID) it is returned unchanged.
pub fn without_verbatim(path: &Path) -> PathBuf {
    // Keep the prefix when dropping it could exceed the old path limit: such a path needs
    // verbatim form to work at all.
    const MAX_PATH: usize = 260;
    let text = path.as_os_str().to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        let plain = format!(r"\\{rest}");
        if plain.len() < MAX_PATH {
            return PathBuf::from(plain);
        }
        return path.to_path_buf();
    }
    // Only drive paths have a plain form; a volume GUID (`\\?\Volume{…}\`) needs its prefix.
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        let bytes = rest.as_bytes();
        let drive = bytes.first().is_some_and(u8::is_ascii_alphabetic)
            && bytes.get(1) == Some(&b':')
            && bytes.get(2).is_some_and(|b| *b == b'\\' || *b == b'/');
        if drive && rest.len() < MAX_PATH {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

/// A path for a generated file (`Cargo.toml` and the like): absolute, without Windows'
/// verbatim prefix, with `/` separators so it works on every platform.
pub fn slash(path: &Path) -> String {
    let path = without_verbatim(path);
    let text = path.to_string_lossy();
    // A long path keeps its verbatim prefix (see [`without_verbatim`]): its backslashes must
    // stay, since `//?/` would not name the same place.
    if text.starts_with(r"\\?\") {
        text.into_owned()
    } else {
        text.replace('\\', "/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_paths_have_no_windows_prefix() {
        let dir = std::env::temp_dir().join(format!("ab-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let canonical = canonicalize(&dir).unwrap();
        assert!(canonical.is_absolute());
        let text = canonical.to_string_lossy();
        assert!(
            !text.starts_with(r"\\?\"),
            "{text} keeps the verbatim prefix"
        );
        assert!(!slash(&canonical).contains('\\'), "{}", slash(&canonical));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn verbatim_prefixes_are_removed() {
        assert_eq!(
            without_verbatim(Path::new(r"\\?\D:\Games\My Game")),
            PathBuf::from(r"D:\Games\My Game")
        );
        assert_eq!(
            without_verbatim(Path::new(r"\\?\UNC\server\share\game")),
            PathBuf::from(r"\\server\share\game")
        );
        assert_eq!(
            slash(Path::new(r"\\?\D:\Games\My Game")),
            "D:/Games/My Game"
        );
        assert_eq!(
            without_verbatim(Path::new("relative/game")),
            PathBuf::from("relative/game")
        );
        // A volume GUID path has no plain form.
        let volume = Path::new(r"\\?\Volume{0000}\game");
        assert_eq!(without_verbatim(volume), volume);
        // A path too long for the plain form keeps its prefix, and its backslashes.
        let long = format!(r"\\?\D:\{}", "a".repeat(300));
        assert_eq!(slash(Path::new(&long)), long);
    }
}
