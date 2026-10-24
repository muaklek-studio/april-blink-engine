//! File sources and the overlay [`Vfs`].

use crate::pack::Pack;
use crate::path::normalize;
use crate::{VfsError, VfsResult};
use std::path::{Path, PathBuf};

/// Somewhere files can be read from. Paths are virtual: `/`-separated, relative.
pub trait FileSource: Send + Sync + 'static {
    fn exists(&self, path: &str) -> bool;
    fn read(&self, path: &str) -> VfsResult<Vec<u8>>;
    /// Every file, sorted.
    fn list(&self) -> Vec<String>;
    /// For logs: `dir:assets`, `pack:game.abp`.
    fn describe(&self) -> String;
}

/// A directory on disk.
#[derive(Debug, Clone)]
pub struct DirSource {
    root: PathBuf,
}

impl DirSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn resolve(&self, path: &str) -> VfsResult<PathBuf> {
        let normalized = normalize(path).ok_or_else(|| VfsError::InvalidPath(path.to_owned()))?;
        // `C:/x` (a drive prefix on Windows) is normalized but would replace the root.
        let confined = Path::new(&normalized)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
        if !confined {
            return Err(VfsError::InvalidPath(path.to_owned()));
        }
        Ok(self.root.join(normalized))
    }
}

impl FileSource for DirSource {
    fn exists(&self, path: &str) -> bool {
        self.resolve(path).is_ok_and(|p| p.is_file())
    }

    fn read(&self, path: &str) -> VfsResult<Vec<u8>> {
        let full = self.resolve(path)?;
        std::fs::read(&full).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => VfsError::NotFound(path.to_owned()),
            _ => VfsError::Io(e),
        })
    }

    fn list(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(relative) = path.strip_prefix(&self.root) {
                    out.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        out.sort();
        out
    }

    fn describe(&self) -> String {
        format!("dir:{}", self.root.display())
    }
}

/// An `.abp` pack.
#[derive(Debug)]
pub struct PackSource {
    pack: Pack,
    name: String,
}

impl PackSource {
    pub fn new(pack: Pack) -> Self {
        Self {
            pack,
            name: "memory".into(),
        }
    }

    /// Opens a pack file (or a program with a pack appended).
    pub fn open(path: &Path) -> VfsResult<Self> {
        Ok(Self {
            pack: Pack::open(path)?,
            name: path.display().to_string(),
        })
    }

    pub fn pack(&self) -> &Pack {
        &self.pack
    }
}

impl FileSource for PackSource {
    fn exists(&self, path: &str) -> bool {
        self.pack.contains(path)
    }

    fn read(&self, path: &str) -> VfsResult<Vec<u8>> {
        self.pack.read(path)
    }

    fn list(&self) -> Vec<String> {
        self.pack.entries().iter().map(|e| e.path.clone()).collect()
    }

    fn describe(&self) -> String {
        format!("pack:{} ({} files)", self.name, self.pack.len())
    }
}

/// Sources stacked: reads go to the most recently mounted source that has the file.
#[derive(Default)]
pub struct Vfs {
    sources: Vec<Box<dyn FileSource>>,
}

impl std::fmt::Debug for Vfs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.sources.iter().map(|s| s.describe()))
            .finish()
    }
}

impl Vfs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mounts a source above the existing ones.
    pub fn mount(&mut self, source: impl FileSource) -> &mut Self {
        self.sources.push(Box::new(source));
        self
    }

    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// The mounted sources, top first, for logs.
    pub fn describe(&self) -> Vec<String> {
        self.sources.iter().rev().map(|s| s.describe()).collect()
    }
}

impl FileSource for Vfs {
    fn exists(&self, path: &str) -> bool {
        self.sources.iter().any(|s| s.exists(path))
    }

    fn read(&self, path: &str) -> VfsResult<Vec<u8>> {
        for source in self.sources.iter().rev() {
            if source.exists(path) {
                return source.read(path);
            }
        }
        Err(VfsError::NotFound(path.to_owned()))
    }

    fn list(&self) -> Vec<String> {
        let mut all: Vec<String> = self.sources.iter().flat_map(|s| s.list()).collect();
        all.sort();
        all.dedup();
        all
    }

    fn describe(&self) -> String {
        self.describe().join(" > ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PackWriter;

    #[test]
    fn later_mounts_override_and_paths_are_confined() {
        let dir = std::env::temp_dir().join(format!("ab-vfs-src-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("scenes")).unwrap();
        std::fs::write(dir.join("scenes/main.abscene"), b"from disk").unwrap();
        std::fs::write(dir.join("only-on-disk.txt"), b"disk").unwrap();

        let mut writer = PackWriter::new();
        writer.add("scenes/main.abscene", b"from pack".to_vec());
        writer.add("only-in-pack.txt", b"pack".to_vec());
        let mut bytes = Vec::new();
        writer.write_to(&mut bytes).unwrap();

        let mut vfs = Vfs::new();
        vfs.mount(PackSource::new(crate::Pack::from_bytes(bytes).unwrap()));
        vfs.mount(DirSource::new(&dir));
        assert_eq!(
            vfs.read("scenes/main.abscene").unwrap(),
            b"from disk",
            "top wins"
        );
        assert_eq!(vfs.read("only-in-pack.txt").unwrap(), b"pack");
        assert_eq!(vfs.read("only-on-disk.txt").unwrap(), b"disk");
        assert!(matches!(vfs.read("nope"), Err(VfsError::NotFound(_))));
        assert_eq!(
            vfs.list(),
            [
                "only-in-pack.txt",
                "only-on-disk.txt",
                "scenes/main.abscene"
            ]
        );
        let disk = DirSource::new(dir.join("scenes"));
        assert!(matches!(
            disk.read("../only-on-disk.txt"),
            Err(VfsError::InvalidPath(_))
        ));
        assert_eq!(vfs.describe().len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
