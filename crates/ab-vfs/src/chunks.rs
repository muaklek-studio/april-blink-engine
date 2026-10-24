//! Chunked assets: a game's files split into several packs plus an index, all in one folder
//! (`assets/` next to an exported game):
//!
//! ```text
//! assets/
//!   index.abm              the manifest: every chunk, and which chunk holds each file
//!   3fa2c1d09b7e5a44.abp   one pack per top-level folder of the assets, named after
//!   9c01e2f3a4b5c6d7.abp   the FNV-1a 64 hash of its bytes
//! ```
//!
//! Files go to the chunk of their top-level folder (`models/crate.glb` → `models`, files at
//! the root → `.`), so editing a file changes only its chunk. Packs are deterministic and
//! named after their content, so an unchanged chunk keeps its file from one build to the
//! next: a patch is the chunks [`PackWriter::write_chunks`] reports as written, plus
//! `index.abm`. The index is written last and replaced in one step, so a game never sees an
//! index naming chunks that are not there yet. Hashed names hide what each chunk holds from
//! a glance at the folder; they do not protect it (`ab-pack list` shows every file).
//!
//! Only the index is read when the folder is mounted ([`ChunkedSource`]); a chunk is opened
//! the first time one of its files is read.
//!
//! The index (version 1, little-endian):
//!
//! ```text
//! header  32 bytes  "ABMF", version u32, chunk count u32, file count u32,
//!                   body size u64, body checksum u64
//! body              per chunk: name length u16, name (UTF-8), hash u64, size u64
//!                   per file: path length u16, path (UTF-8), chunk index u32
//! ```

use crate::pack::{u16_at, u32_at, u64_at, write_files, Pack, PackStats, PackWriter};
use crate::path::normalize;
use crate::source::FileSource;
use crate::{VfsError, VfsResult, PACK_EXTENSION};
use ab_core::utils::hash::fnv1a64;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// File name of the index in a chunk folder.
pub const MANIFEST_FILE: &str = "index.abm";

/// Folder next to an exported game holding its chunks.
pub const CHUNK_DIR: &str = "assets";

const MAGIC: &[u8; 4] = b"ABMF";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 32;
/// Smallest chunk entry: name length, empty name, hash, size.
const MIN_CHUNK_ENTRY: usize = 2 + 8 + 8;
/// Smallest file entry: path length, empty path, chunk index.
const MIN_FILE_ENTRY: usize = 2 + 4;
/// Chunk of the files at the root of the assets.
const ROOT_CHUNK: &str = ".";

/// The chunk a file goes to: its top-level folder.
pub fn chunk_of(path: &str) -> &str {
    path.split_once('/').map_or(ROOT_CHUNK, |(top, _)| top)
}

/// Whether `dir` is a chunk folder (holds an index).
pub fn is_chunked(dir: &Path) -> bool {
    dir.join(MANIFEST_FILE).is_file()
}

/// File name of the chunk whose bytes hash to `hash`.
fn chunk_file_name(hash: u64) -> String {
    format!("{hash:016x}.{PACK_EXTENSION}")
}

/// Whether `name` looks like a chunk file (16 hex digits and the pack extension).
fn is_chunk_file_name(name: &str) -> bool {
    name.strip_suffix(PACK_EXTENSION)
        .and_then(|stem| stem.strip_suffix('.'))
        .is_some_and(|hex| hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// One chunk of an index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkInfo {
    /// The top-level folder it holds (`.` for root files).
    pub name: String,
    /// FNV-1a 64 of the chunk's bytes; also its file name.
    pub hash: u64,
    /// Bytes of the chunk file.
    pub size: u64,
}

impl ChunkInfo {
    /// `<hash>.abp`.
    pub fn file_name(&self) -> String {
        chunk_file_name(self.hash)
    }
}

/// The index of a chunk folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    chunks: Vec<ChunkInfo>,
    /// Path → chunk index.
    files: BTreeMap<String, u32>,
}

impl Manifest {
    /// Reads `dir`'s index.
    pub fn read(dir: &Path) -> VfsResult<Self> {
        let path = dir.join(MANIFEST_FILE);
        Self::from_bytes(&std::fs::read(&path)?).map_err(|e| match e {
            VfsError::NotAPack(_) => VfsError::NotAPack(path.display().to_string()),
            other => other,
        })
    }

    pub fn chunks(&self) -> &[ChunkInfo] {
        &self.chunks
    }

    /// Every file and its chunk, sorted by path.
    pub fn files(&self) -> impl Iterator<Item = (&str, &ChunkInfo)> {
        self.files
            .iter()
            .map(|(path, &chunk)| (path.as_str(), &self.chunks[chunk as usize]))
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Index of the chunk holding `path`.
    pub fn chunk_index(&self, path: &str) -> Option<usize> {
        self.files
            .get(&normalize(path)?)
            .map(|&chunk| chunk as usize)
    }

    /// Identifies this set of chunks: two builds with the same files have the same id (a
    /// patcher compares it to know whether a game is up to date).
    pub fn id(&self) -> u64 {
        fnv1a64(&self.body())
    }

    fn body(&self) -> Vec<u8> {
        let mut body = Vec::new();
        for chunk in &self.chunks {
            body.extend_from_slice(&(chunk.name.len() as u16).to_le_bytes());
            body.extend_from_slice(chunk.name.as_bytes());
            body.extend_from_slice(&chunk.hash.to_le_bytes());
            body.extend_from_slice(&chunk.size.to_le_bytes());
        }
        for (path, chunk) in &self.files {
            body.extend_from_slice(&(path.len() as u16).to_le_bytes());
            body.extend_from_slice(path.as_bytes());
            body.extend_from_slice(&chunk.to_le_bytes());
        }
        body
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let body = self.body();
        let mut out = Vec::with_capacity(HEADER_SIZE + body.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&(self.chunks.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.files.len() as u32).to_le_bytes());
        out.extend_from_slice(&(body.len() as u64).to_le_bytes());
        out.extend_from_slice(&fnv1a64(&body).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> VfsResult<Self> {
        let corrupt = |what: &str| VfsError::Corrupt(format!("chunk index: {what}"));
        if bytes.get(..4) != Some(MAGIC.as_slice()) {
            return Err(VfsError::NotAPack("not a chunk index".into()));
        }
        let version = u32_at(bytes, 4).ok_or_else(|| corrupt("header"))?;
        if version != VERSION {
            return Err(VfsError::Corrupt(format!(
                "chunk index version {version} (this engine reads {VERSION})"
            )));
        }
        let chunk_count = u32_at(bytes, 8).ok_or_else(|| corrupt("header"))? as usize;
        let file_count = u32_at(bytes, 12).ok_or_else(|| corrupt("header"))? as usize;
        let body_size = u64_at(bytes, 16).ok_or_else(|| corrupt("header"))?;
        let checksum = u64_at(bytes, 24).ok_or_else(|| corrupt("header"))?;
        let body = &bytes[HEADER_SIZE.min(bytes.len())..];
        if body.len() as u64 != body_size {
            return Err(corrupt("size"));
        }
        if fnv1a64(body) != checksum {
            return Err(corrupt("checksum mismatch"));
        }
        if chunk_count > body.len() / MIN_CHUNK_ENTRY || file_count > body.len() / MIN_FILE_ENTRY {
            return Err(corrupt("counts"));
        }
        let mut at = 0;
        let text = |at: &mut usize| -> Option<String> {
            let len = u16_at(body, *at)? as usize;
            let text = std::str::from_utf8(body.get(*at + 2..*at + 2 + len)?).ok()?;
            *at += 2 + len;
            Some(text.to_owned())
        };
        let mut chunks = Vec::with_capacity(chunk_count);
        for _ in 0..chunk_count {
            let chunk = (|| {
                let name = text(&mut at)?;
                let chunk = ChunkInfo {
                    name,
                    hash: u64_at(body, at)?,
                    size: u64_at(body, at + 8)?,
                };
                at += 16;
                Some(chunk)
            })()
            .ok_or_else(|| corrupt("chunk entry"))?;
            chunks.push(chunk);
        }
        let mut files = BTreeMap::new();
        for _ in 0..file_count {
            let (path, chunk) = (|| {
                let path = text(&mut at)?;
                let chunk = u32_at(body, at)?;
                at += 4;
                Some((path, chunk))
            })()
            .ok_or_else(|| corrupt("file entry"))?;
            // Same rule as packs: only normalized paths can be looked up or extracted safely.
            if normalize(&path).as_deref() != Some(path.as_str()) {
                return Err(corrupt(&format!("invalid file path `{path}`")));
            }
            if chunk as usize >= chunks.len() {
                return Err(corrupt(&format!("`{path}` in a chunk that does not exist")));
            }
            if files.insert(path, chunk).is_some() {
                return Err(corrupt("a file listed twice"));
            }
        }
        if at != body.len() {
            return Err(corrupt("trailing bytes"));
        }
        Ok(Self { chunks, files })
    }
}

/// What [`PackWriter::write_chunks`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChunkStats {
    /// All chunks together (`pack_bytes`: the chunk files, without the index).
    pub totals: PackStats,
    pub chunks: usize,
    /// File names of the chunks written by this call: new or changed content. The others
    /// were already in the folder. These and the index are what a patch ships.
    pub written: Vec<String>,
    /// Chunk files the new index no longer names, removed.
    pub removed: usize,
}

impl PackWriter {
    /// Writes the files as chunks and their index in `dir` (created if needed); see the
    /// [module docs](self). Chunks already in `dir` are kept, the index is replaced, and
    /// chunk files it no longer names are removed.
    pub fn write_chunks(&self, dir: &Path) -> VfsResult<ChunkStats> {
        let mut groups: BTreeMap<&str, Vec<&crate::pack::PendingFile>> = BTreeMap::new();
        for file in self.pending() {
            groups.entry(chunk_of(&file.0)).or_default().push(file);
        }
        std::fs::create_dir_all(dir)?;
        let mut stats = ChunkStats {
            chunks: groups.len(),
            ..Default::default()
        };
        let mut manifest = Manifest::default();
        for (index, (name, files)) in groups.into_iter().enumerate() {
            for file in &files {
                manifest.files.insert(file.0.clone(), index as u32);
            }
            let mut bytes = Vec::new();
            let pack = write_files(files, &mut bytes)?;
            stats.totals.files += pack.files;
            stats.totals.original_bytes += pack.original_bytes;
            stats.totals.pack_bytes += pack.pack_bytes;
            stats.totals.compressed_files += pack.compressed_files;
            let chunk = ChunkInfo {
                name: name.to_owned(),
                hash: fnv1a64(&bytes),
                size: bytes.len() as u64,
            };
            let path = dir.join(chunk.file_name());
            // Named after its content: a file of that name is this chunk, unless damaged.
            if !std::fs::read(&path).is_ok_and(|old| old == bytes) {
                write_replacing(&path, &bytes)?;
                stats.written.push(chunk.file_name());
            }
            manifest.chunks.push(chunk);
        }
        write_replacing(&dir.join(MANIFEST_FILE), &manifest.to_bytes())?;
        stats.removed = remove_stale_chunks(dir, &manifest);
        Ok(stats)
    }
}

/// Writes `path` through a temporary file, so readers see the old file or the new one,
/// never half of it.
fn write_replacing(path: &Path, bytes: &[u8]) -> VfsResult<()> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// Removes the chunk files of `dir` that `manifest` does not name; returns how many. A file
/// that cannot be removed (in use by a running game on Windows) is left for next time.
fn remove_stale_chunks(dir: &Path, manifest: &Manifest) -> usize {
    let named: HashSet<String> = manifest.chunks.iter().map(ChunkInfo::file_name).collect();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            is_chunk_file_name(&name) && !named.contains(&name)
        })
        .filter(|entry| std::fs::remove_file(entry.path()).is_ok())
        .count()
}

/// A chunk folder as a file source: the index is read up front, each chunk opened on first
/// use.
pub struct ChunkedSource {
    dir: PathBuf,
    manifest: Manifest,
    /// Each chunk once opened, or why it could not be.
    packs: Vec<OnceLock<Result<Pack, String>>>,
}

impl std::fmt::Debug for ChunkedSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkedSource")
            .field("dir", &self.dir)
            .field("chunks", &self.manifest.chunks.len())
            .field("files", &self.manifest.files.len())
            .finish()
    }
}

impl ChunkedSource {
    /// Reads the index of the chunk folder `dir`.
    pub fn open(dir: &Path) -> VfsResult<Self> {
        let manifest = Manifest::read(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            packs: manifest.chunks.iter().map(|_| OnceLock::new()).collect(),
            manifest,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The pack of chunk `index`, opened on first use.
    pub fn chunk(&self, index: usize) -> VfsResult<&Pack> {
        let info = self
            .manifest
            .chunks
            .get(index)
            .ok_or_else(|| VfsError::NotFound(format!("chunk {index}")))?;
        let opened = self.packs[index].get_or_init(|| {
            let path = self.dir.join(info.file_name());
            let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
            if size != info.size {
                return Err(format!("{size} bytes, the index says {}", info.size));
            }
            Pack::open(&path).map_err(|e| e.to_string())
        });
        opened.as_ref().map_err(|e| {
            VfsError::Corrupt(format!("chunk `{}` ({}): {e}", info.name, info.file_name()))
        })
    }

    /// Reads everything: each chunk's bytes against its name, then every file.
    pub fn verify(&self) -> VfsResult<()> {
        for (index, info) in self.manifest.chunks.iter().enumerate() {
            let bytes = std::fs::read(self.dir.join(info.file_name()))?;
            if fnv1a64(&bytes) != info.hash {
                return Err(VfsError::Corrupt(format!(
                    "chunk `{}` ({}): content does not match its name",
                    info.name,
                    info.file_name()
                )));
            }
            self.chunk(index)?.verify()?;
        }
        for (path, &chunk) in &self.manifest.files {
            if !self.chunk(chunk as usize)?.contains(path) {
                return Err(VfsError::Corrupt(format!(
                    "`{path}` is not in the chunk the index names"
                )));
            }
        }
        Ok(())
    }
}

impl FileSource for ChunkedSource {
    fn exists(&self, path: &str) -> bool {
        self.manifest.chunk_index(path).is_some()
    }

    fn read(&self, path: &str) -> VfsResult<Vec<u8>> {
        let chunk = self
            .manifest
            .chunk_index(path)
            .ok_or_else(|| VfsError::NotFound(path.to_owned()))?;
        self.chunk(chunk)?.read(path)
    }

    fn list(&self) -> Vec<String> {
        self.manifest.files.keys().cloned().collect()
    }

    fn describe(&self) -> String {
        format!(
            "chunks:{} ({} chunks, {} files)",
            self.dir.display(),
            self.manifest.chunks.len(),
            self.manifest.files.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ab-vfs-chunks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn sample() -> PackWriter {
        let mut writer = PackWriter::new();
        writer
            .add("models/crate.glb", vec![1u8; 500])
            .add("models/totem.gltf", b"{\"asset\":{}}".to_vec())
            .add("audio/hit.wav", (0..=255u8).collect())
            .add("main.abscene", b"[[node]]\n".to_vec())
            .add(".project/game.toml", b"title = \"T\"\n".to_vec());
        writer
    }

    fn chunk_files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| is_chunk_file_name(n))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn one_chunk_per_top_level_folder_named_by_hash() {
        let dir = temp("split");
        let stats = sample().write_chunks(&dir).unwrap();
        assert_eq!(stats.chunks, 4, ".project, audio, models and the root");
        assert_eq!(stats.totals.files, 5);
        assert_eq!(stats.written.len(), 4);
        assert_eq!(chunk_files(&dir).len(), 4);
        assert!(
            !dir.join("models.abp").exists(),
            "names do not show the folder"
        );

        let source = ChunkedSource::open(&dir).unwrap();
        source.verify().unwrap();
        assert_eq!(source.read("models/crate.glb").unwrap(), vec![1u8; 500]);
        assert_eq!(source.read("./main.abscene").unwrap(), b"[[node]]\n");
        assert!(source.exists(".project/game.toml"));
        assert!(matches!(source.read("nope"), Err(VfsError::NotFound(_))));
        assert_eq!(source.list().len(), 5);
        let names: Vec<&str> = source
            .manifest()
            .chunks()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, [".", ".project", "audio", "models"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_what_is_read_is_opened() {
        let dir = temp("lazy");
        sample().write_chunks(&dir).unwrap();
        let source = ChunkedSource::open(&dir).unwrap();
        let models = source.manifest().chunk_index("models/crate.glb").unwrap();
        // A missing chunk only fails the files in it.
        let audio =
            &source.manifest().chunks()[source.manifest().chunk_index("audio/hit.wav").unwrap()];
        std::fs::remove_file(dir.join(audio.file_name())).unwrap();
        assert!(source.read("models/crate.glb").is_ok());
        assert!(matches!(
            source.read("audio/hit.wav"),
            Err(VfsError::Corrupt(_))
        ));
        assert!(source.packs[models].get().is_some());
        assert!(source.verify().is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_change_rewrites_only_its_chunk() {
        let dir = temp("patch");
        let first = sample().write_chunks(&dir).unwrap();
        let id = Manifest::read(&dir).unwrap().id();

        // The same files again: nothing to write, same index.
        let again = sample().write_chunks(&dir).unwrap();
        assert!(again.written.is_empty());
        assert_eq!(Manifest::read(&dir).unwrap().id(), id);

        let mut changed = sample();
        changed.add("audio/hit.wav", vec![9u8; 64]);
        let patch = changed.write_chunks(&dir).unwrap();
        assert_eq!(patch.written.len(), 1, "only the audio chunk");
        assert_eq!(patch.removed, 1, "the old audio chunk");
        assert!(!first.written.contains(&patch.written[0]));
        assert_eq!(chunk_files(&dir).len(), 4);
        assert_ne!(Manifest::read(&dir).unwrap().id(), id);
        let source = ChunkedSource::open(&dir).unwrap();
        source.verify().unwrap();
        assert_eq!(source.read("audio/hit.wav").unwrap(), vec![9u8; 64]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_damaged_index_is_an_error_not_a_crash() {
        let dir = temp("damage");
        sample().write_chunks(&dir).unwrap();
        let bytes = std::fs::read(dir.join(MANIFEST_FILE)).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert!(Manifest::from_bytes(&bytes).is_ok());
        for i in 0..bytes.len() {
            for flip in [0x01, 0x80, 0xff] {
                let mut damaged = bytes.clone();
                damaged[i] ^= flip;
                assert!(Manifest::from_bytes(&damaged).is_err(), "byte {i}");
            }
        }
        for len in 0..bytes.len() {
            assert!(Manifest::from_bytes(&bytes[..len]).is_err());
        }
    }

    #[test]
    fn rejects_paths_escaping_the_root() {
        let mut manifest = Manifest::default();
        manifest.chunks.push(ChunkInfo {
            name: "x".into(),
            hash: 1,
            size: 1,
        });
        manifest.files.insert("../../.bashrc".into(), 0);
        assert!(matches!(
            Manifest::from_bytes(&manifest.to_bytes()),
            Err(VfsError::Corrupt(_))
        ));
    }
}
