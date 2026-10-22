use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Where asset bytes come from. Paths are relative, `/`-separated and already normalised
/// (see [`normalize_path`]).
pub trait AssetSource: Send + Sync + 'static {
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;
    fn exists(&self, path: &str) -> bool;
}

/// Normalises an asset path: `\` → `/`, removes `.` and empty segments, resolves `..`.
/// Returns `None` if `..` would escape the asset root.
pub fn normalize_path(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    Some(parts.join("/"))
}

/// Resolves `relative` against the directory containing `base` (both asset paths).
pub(crate) fn resolve_relative(base: &str, relative: &str) -> Option<String> {
    let dir = base.rsplit_once('/').map_or("", |(dir, _)| dir);
    // glTF URIs are percent-encoded; decode the common space escape.
    let relative = relative.replace("%20", " ");
    normalize_path(&format!("{dir}/{relative}"))
}

/// Reads assets from a directory on disk.
#[derive(Debug, Clone)]
pub struct FileSystemSource {
    root: PathBuf,
}

impl FileSystemSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn full_path(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }
}

impl AssetSource for FileSystemSource {
    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        std::fs::read(self.full_path(path))
    }

    fn exists(&self, path: &str) -> bool {
        self.full_path(path).is_file()
    }
}

/// The engine's virtual file system (directories and `.abp` packs) as an asset source.
impl AssetSource for ab_vfs::Vfs {
    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        ab_vfs::FileSource::read(self, path).map_err(|e| match e {
            ab_vfs::VfsError::NotFound(_) => io::Error::new(io::ErrorKind::NotFound, e.to_string()),
            ab_vfs::VfsError::Io(e) => e,
            other => io::Error::new(io::ErrorKind::InvalidData, other.to_string()),
        })
    }

    fn exists(&self, path: &str) -> bool {
        ab_vfs::FileSource::exists(self, path)
    }
}

/// In-memory assets (tests, procedurally generated content).
#[derive(Default, Clone)]
pub struct MemorySource {
    files: HashMap<String, Vec<u8>>,
}

impl MemorySource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file; the path is normalised.
    pub fn insert(&mut self, path: &str, bytes: impl Into<Vec<u8>>) -> &mut Self {
        let path = normalize_path(path).unwrap_or_else(|| path.to_owned());
        self.files.insert(path, bytes.into());
        self
    }
}

impl AssetSource for MemorySource {
    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "not in memory source"))
    }

    fn exists(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }
}

impl fmt::Debug for MemorySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut paths: Vec<_> = self.files.keys().collect();
        paths.sort();
        f.debug_struct("MemorySource")
            .field("files", &paths)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize_path("a/./b//c").as_deref(), Some("a/b/c"));
        assert_eq!(normalize_path("a\\b\\..\\c").as_deref(), Some("a/c"));
        assert_eq!(
            normalize_path("/models/x.glb").as_deref(),
            Some("models/x.glb")
        );
        assert_eq!(normalize_path("../secret"), None);
    }

    #[test]
    fn resolves_relative_uris() {
        assert_eq!(
            resolve_relative("models/hero/hero.gltf", "textures/skin%20d.png").as_deref(),
            Some("models/hero/textures/skin d.png")
        );
        assert_eq!(
            resolve_relative("hero.gltf", "../x.png"),
            None,
            "cannot escape the root"
        );
    }

    #[test]
    fn memory_source_round_trip() {
        let mut source = MemorySource::new();
        source.insert("./a/b.bin", [1, 2, 3]);
        assert!(source.exists("a/b.bin"));
        assert_eq!(source.read("a/b.bin").unwrap(), [1, 2, 3]);
        assert!(source.read("missing").is_err());
    }
}
