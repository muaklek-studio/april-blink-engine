//! The `.abp` pack format (version 1, little-endian):
//!
//! ```text
//! header   64 bytes   "ABPK", version u32, flags u32, file count u32,
//!                     TOC offset u64, TOC size u64, TOC checksum u64, reserved
//! data                each file's bytes, stored or LZ4-compressed
//! TOC                 per file: path length u16, path (UTF-8), offset u64, stored size u64,
//!                     size u64, compression u8, checksum u64 (of the original bytes)
//! trailer  16 bytes   pack size u64 (all of the above + trailer), "ABPKTAIL"
//! ```
//!
//! Offsets are relative to the start of the pack, and the trailer is at the very end, so the
//! same pack works as a standalone file or appended to an executable. Checksums are FNV-1a
//! 64 (corruption detection, not security).

use crate::path::normalize;
use crate::{VfsError, VfsResult};
use ab_core::utils::hash::fnv1a64;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

/// File extension of packs.
pub const PACK_EXTENSION: &str = "abp";

/// Folder of a game's pack holding the project's own files (`game.toml`, the input map),
/// apart from its assets.
pub const PROJECT_DIR: &str = ".project";

const MAGIC: &[u8; 4] = b"ABPK";
const TAIL_MAGIC: &[u8; 8] = b"ABPKTAIL";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 64;
const TRAILER_SIZE: usize = 16;
/// Smallest table of contents entry: path length, empty path, offset, sizes, compression,
/// checksum.
const MIN_ENTRY_SIZE: usize = 2 + 8 + 8 + 8 + 1 + 8;
/// LZ4 never expands data by more than this factor, so larger claimed sizes are damage.
const LZ4_MAX_RATIO: u64 = 255;

/// Extensions of formats that are already compressed: stored as they are.
const PRECOMPRESSED: &[&str] = &["png", "jpg", "jpeg", "ogg", "mp3", "flac", "zip", "abp"];

/// How a file is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None = 0,
    Lz4 = 1,
}

/// One file of a pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackEntry {
    pub path: String,
    offset: u64,
    /// Bytes in the pack.
    pub stored_size: u64,
    /// Bytes once read.
    pub size: u64,
    pub compression: Compression,
    checksum: u64,
}

/// Totals of a written pack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PackStats {
    pub files: usize,
    /// Sum of the original file sizes.
    pub original_bytes: u64,
    /// Size of the pack.
    pub pack_bytes: u64,
    pub compressed_files: usize,
}

/// Builds a pack in memory, then writes it.
#[derive(Debug, Default)]
pub struct PackWriter {
    files: Vec<(String, Vec<u8>, bool)>,
}

impl PackWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file, compressed when that makes it smaller (and its format is not already
    /// compressed). A file added twice keeps the last version.
    pub fn add(&mut self, path: &str, bytes: Vec<u8>) -> &mut Self {
        let extension = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        let compress = !extension.is_some_and(|e| PRECOMPRESSED.contains(&e.as_str()));
        self.add_with(path, bytes, compress)
    }

    /// Adds a file, never compressed.
    pub fn add_stored(&mut self, path: &str, bytes: Vec<u8>) -> &mut Self {
        self.add_with(path, bytes, false)
    }

    fn add_with(&mut self, path: &str, bytes: Vec<u8>, compress: bool) -> &mut Self {
        let Some(path) = normalize(path) else {
            return self;
        };
        self.files.retain(|(p, ..)| *p != path);
        self.files.push((path, bytes, compress));
        self
    }

    /// Adds every file under `root` (hidden files and folders skipped), with paths relative
    /// to it. Returns how many were added.
    pub fn add_directory(&mut self, root: &Path) -> VfsResult<usize> {
        let mut files = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                let hidden = path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'));
                if hidden {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else if path.is_file() {
                    files.push(path);
                }
            }
        }
        // Deterministic packs: same input, same bytes.
        files.sort();
        let count = files.len();
        for file in files {
            let relative = file
                .strip_prefix(root)
                .map_err(|_| VfsError::InvalidPath(file.display().to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            self.add(&relative, std::fs::read(&file)?);
        }
        Ok(count)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Writes the pack (header, data, table of contents, trailer).
    pub fn write_to(&self, out: &mut impl Write) -> VfsResult<PackStats> {
        let mut files: Vec<&(String, Vec<u8>, bool)> = self.files.iter().collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut stats = PackStats {
            files: files.len(),
            ..Default::default()
        };
        let mut data = Vec::new();
        let mut toc = Vec::new();
        for (path, bytes, compress) in files {
            let packed = compress
                .then(|| lz4_flex::block::compress(bytes))
                // Keep compression only when it saves at least 5 %.
                .filter(|packed| packed.len() * 20 < bytes.len() * 19);
            let (stored, compression) = match &packed {
                Some(packed) => (packed.as_slice(), Compression::Lz4),
                None => (bytes.as_slice(), Compression::None),
            };
            if compression == Compression::Lz4 {
                stats.compressed_files += 1;
            }
            stats.original_bytes += bytes.len() as u64;
            let offset = (HEADER_SIZE + data.len()) as u64;
            data.extend_from_slice(stored);
            let path_bytes = path.as_bytes();
            let path_len =
                u16::try_from(path_bytes.len()).map_err(|_| VfsError::InvalidPath(path.clone()))?;
            toc.extend_from_slice(&path_len.to_le_bytes());
            toc.extend_from_slice(path_bytes);
            toc.extend_from_slice(&offset.to_le_bytes());
            toc.extend_from_slice(&(stored.len() as u64).to_le_bytes());
            toc.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            toc.push(compression as u8);
            toc.extend_from_slice(&fnv1a64(bytes).to_le_bytes());
        }
        let toc_offset = (HEADER_SIZE + data.len()) as u64;
        let mut header = Vec::with_capacity(HEADER_SIZE);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&VERSION.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        header.extend_from_slice(&(stats.files as u32).to_le_bytes());
        header.extend_from_slice(&toc_offset.to_le_bytes());
        header.extend_from_slice(&(toc.len() as u64).to_le_bytes());
        header.extend_from_slice(&fnv1a64(&toc).to_le_bytes());
        header.resize(HEADER_SIZE, 0);
        let pack_size = (HEADER_SIZE + data.len() + toc.len() + TRAILER_SIZE) as u64;
        out.write_all(&header)?;
        out.write_all(&data)?;
        out.write_all(&toc)?;
        out.write_all(&pack_size.to_le_bytes())?;
        out.write_all(TAIL_MAGIC)?;
        stats.pack_bytes = pack_size;
        Ok(stats)
    }

    /// Writes the pack to a file.
    pub fn write_file(&self, path: &Path) -> VfsResult<PackStats> {
        let mut file = std::io::BufWriter::new(File::create(path)?);
        let stats = self.write_to(&mut file)?;
        file.flush()?;
        Ok(stats)
    }
}

/// Writes `executable` followed by the pack to `output`: one file that finds its own data
/// ([`Pack::open`] on it reads the pack from its end).
pub fn append_to_executable(
    executable: &Path,
    pack: &PackWriter,
    output: &Path,
) -> VfsResult<PackStats> {
    let program = std::fs::read(executable)?;
    let mut file = std::io::BufWriter::new(File::create(output)?);
    file.write_all(&program)?;
    let stats = pack.write_to(&mut file)?;
    file.flush()?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(executable)?.permissions().mode();
        std::fs::set_permissions(output, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(stats)
}

enum Backing {
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl Backing {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Mapped(map) => map,
            Self::Owned(bytes) => bytes,
        }
    }
}

/// A pack opened for reading (memory-mapped).
pub struct Pack {
    backing: Backing,
    /// Where the pack starts in the backing bytes (non-zero when appended to a program).
    start: usize,
    entries: HashMap<String, PackEntry>,
}

impl std::fmt::Debug for Pack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pack")
            .field("files", &self.entries.len())
            .field("start", &self.start)
            .finish()
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

impl Pack {
    /// Opens a pack file, or the pack appended to a program.
    pub fn open(path: &Path) -> VfsResult<Self> {
        let file = File::open(path)?;
        // SAFETY: the pack is mapped read-only; like any memory map it must not be truncated
        // by another process while the game runs (packs are read-only game data).
        let map = unsafe { memmap2::Mmap::map(&file)? };
        Self::parse(Backing::Mapped(map)).map_err(|e| match e {
            VfsError::NotAPack(_) => VfsError::NotAPack(path.display().to_string()),
            other => other,
        })
    }

    /// Whether `path` ends with a pack (a pack file, or a program with one appended).
    pub fn is_pack(path: &Path) -> bool {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut file) = File::open(path) else {
            return false;
        };
        let mut tail = [0u8; 8];
        file.seek(SeekFrom::End(-8)).is_ok()
            && file.read_exact(&mut tail).is_ok()
            && &tail == TAIL_MAGIC
    }

    /// Reads a pack held in memory.
    pub fn from_bytes(bytes: Vec<u8>) -> VfsResult<Self> {
        Self::parse(Backing::Owned(bytes))
    }

    fn parse(backing: Backing) -> VfsResult<Self> {
        let corrupt = |what: &str| VfsError::Corrupt(what.to_owned());
        let bytes = backing.bytes();
        if bytes.len() < HEADER_SIZE + TRAILER_SIZE || &bytes[bytes.len() - 8..] != TAIL_MAGIC {
            return Err(VfsError::NotAPack("no pack trailer".into()));
        }
        let pack_size =
            u64_at(bytes, bytes.len() - TRAILER_SIZE).ok_or_else(|| corrupt("trailer"))? as usize;
        if pack_size < HEADER_SIZE + TRAILER_SIZE {
            return Err(corrupt("pack size"));
        }
        let start = bytes
            .len()
            .checked_sub(pack_size)
            .ok_or_else(|| corrupt("pack size exceeds the file"))?;
        let pack = &bytes[start..];
        if pack.get(..4) != Some(MAGIC.as_slice()) {
            return Err(VfsError::NotAPack("bad magic".into()));
        }
        let version = u32_at(pack, 4).ok_or_else(|| corrupt("header"))?;
        if version != VERSION {
            return Err(VfsError::Corrupt(format!(
                "pack version {version} (this engine reads {VERSION})"
            )));
        }
        let count = u32_at(pack, 12).ok_or_else(|| corrupt("header"))? as usize;
        let toc_offset = u64_at(pack, 16).ok_or_else(|| corrupt("header"))? as usize;
        let toc_size = u64_at(pack, 24).ok_or_else(|| corrupt("header"))? as usize;
        let checksum = u64_at(pack, 32).ok_or_else(|| corrupt("header"))?;
        let toc = pack
            .get(toc_offset..toc_offset.saturating_add(toc_size))
            .ok_or_else(|| corrupt("table of contents out of bounds"))?;
        if fnv1a64(toc) != checksum {
            return Err(corrupt("table of contents checksum mismatch"));
        }
        // The header is not covered by the checksum: a damaged count must not reserve
        // gigabytes before the entries prove it wrong.
        if count > toc_size / MIN_ENTRY_SIZE {
            return Err(corrupt("file count"));
        }
        let mut entries = HashMap::with_capacity(count);
        let mut at = 0;
        for _ in 0..count {
            let entry = (|| {
                let len = u16_at(toc, at)? as usize;
                let path = std::str::from_utf8(toc.get(at + 2..at + 2 + len)?).ok()?;
                let field = at + 2 + len;
                let entry = PackEntry {
                    path: path.to_owned(),
                    offset: u64_at(toc, field)?,
                    stored_size: u64_at(toc, field + 8)?,
                    size: u64_at(toc, field + 16)?,
                    compression: match toc.get(field + 24)? {
                        0 => Compression::None,
                        1 => Compression::Lz4,
                        _ => return None,
                    },
                    checksum: u64_at(toc, field + 25)?,
                };
                at = field + 33;
                Some(entry)
            })()
            .ok_or_else(|| corrupt("table of contents entry"))?;
            let end = entry.offset.checked_add(entry.stored_size);
            if end.is_none_or(|end| end > toc_offset as u64) {
                return Err(corrupt("file data out of bounds"));
            }
            let size_ok = match entry.compression {
                Compression::None => entry.size == entry.stored_size,
                Compression::Lz4 => {
                    entry.size <= entry.stored_size.saturating_mul(LZ4_MAX_RATIO) + 64
                }
            };
            if !size_ok {
                return Err(corrupt("file size"));
            }
            entries.insert(entry.path.clone(), entry);
        }
        Ok(Self {
            backing,
            start,
            entries,
        })
    }

    fn data(&self) -> &[u8] {
        &self.backing.bytes()[self.start..]
    }

    pub fn contains(&self, path: &str) -> bool {
        normalize(path).is_some_and(|p| self.entries.contains_key(&p))
    }

    pub fn entry(&self, path: &str) -> Option<&PackEntry> {
        self.entries.get(&normalize(path)?)
    }

    /// Every file, sorted by path.
    pub fn entries(&self) -> Vec<&PackEntry> {
        let mut entries: Vec<&PackEntry> = self.entries.values().collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A file's bytes, decompressed and checked.
    pub fn read(&self, path: &str) -> VfsResult<Vec<u8>> {
        let entry = self
            .entry(path)
            .ok_or_else(|| VfsError::NotFound(path.to_owned()))?;
        let stored =
            &self.data()[entry.offset as usize..(entry.offset + entry.stored_size) as usize];
        let bytes = match entry.compression {
            Compression::None => stored.to_vec(),
            Compression::Lz4 => lz4_flex::block::decompress(stored, entry.size as usize)
                .map_err(|e| VfsError::Corrupt(format!("{}: {e}", entry.path)))?,
        };
        if bytes.len() as u64 != entry.size || fnv1a64(&bytes) != entry.checksum {
            return Err(VfsError::Corrupt(format!(
                "{}: checksum mismatch",
                entry.path
            )));
        }
        Ok(bytes)
    }

    /// Reads every file, reporting the first damaged one.
    pub fn verify(&self) -> VfsResult<()> {
        for entry in self.entries() {
            self.read(&entry.path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PackWriter {
        let mut writer = PackWriter::new();
        writer
            .add(
                "scenes/level.toml",
                "[[node]]\nname = \"A\"\n".repeat(50).into_bytes(),
            )
            .add("textures/wall.png", vec![7u8; 300])
            .add("audio\\hit.wav", (0..=255u8).collect())
            .add("empty.txt", Vec::new());
        writer
    }

    fn written(writer: &PackWriter) -> (Vec<u8>, PackStats) {
        let mut bytes = Vec::new();
        let stats = writer.write_to(&mut bytes).unwrap();
        (bytes, stats)
    }

    #[test]
    fn round_trip_with_compression() {
        let (bytes, stats) = written(&sample());
        assert_eq!(stats.files, 4);
        assert_eq!(stats.pack_bytes as usize, bytes.len());
        assert_eq!(
            stats.compressed_files, 1,
            "only the repetitive text shrinks enough"
        );
        let pack = Pack::from_bytes(bytes).unwrap();
        assert_eq!(pack.len(), 4);
        let level = pack.entry("scenes/level.toml").unwrap();
        assert_eq!(level.compression, Compression::Lz4);
        assert!(level.stored_size < level.size);
        assert_eq!(
            pack.entry("textures/wall.png").unwrap().compression,
            Compression::None,
            "already-compressed formats are stored"
        );
        assert_eq!(
            pack.read("scenes/level.toml").unwrap(),
            "[[node]]\nname = \"A\"\n".repeat(50).into_bytes()
        );
        assert_eq!(
            pack.read("./audio/hit.wav").unwrap().len(),
            256,
            "normalized"
        );
        assert!(pack.read("empty.txt").unwrap().is_empty());
        assert!(matches!(pack.read("missing"), Err(VfsError::NotFound(_))));
        pack.verify().unwrap();
        let paths: Vec<&str> = pack.entries().iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "audio/hit.wav",
                "empty.txt",
                "scenes/level.toml",
                "textures/wall.png"
            ]
        );
    }

    #[test]
    fn deterministic_and_detects_damage() {
        let (a, _) = written(&sample());
        let (b, _) = written(&sample());
        assert_eq!(a, b, "same input, same bytes");

        let mut damaged = a.clone();
        let data_byte = HEADER_SIZE + 10;
        damaged[data_byte] ^= 0xff;
        let pack = Pack::from_bytes(damaged).unwrap();
        assert!(matches!(pack.verify(), Err(VfsError::Corrupt(_))));

        let mut toc_damaged = a.clone();
        let toc_byte = toc_damaged.len() - TRAILER_SIZE - 3;
        toc_damaged[toc_byte] ^= 0xff;
        assert!(matches!(
            Pack::from_bytes(toc_damaged),
            Err(VfsError::Corrupt(_))
        ));

        assert!(matches!(
            Pack::from_bytes(b"hello".to_vec()),
            Err(VfsError::NotAPack(_))
        ));
    }

    /// Opens and reads everything; damage must surface as errors, never as panics or
    /// giant allocations.
    fn open_and_read_all(bytes: Vec<u8>) {
        if let Ok(pack) = Pack::from_bytes(bytes) {
            let paths: Vec<String> = pack.entries().iter().map(|e| e.path.clone()).collect();
            for path in paths {
                let _ = pack.read(&path);
            }
        }
    }

    #[test]
    fn any_damage_is_an_error_not_a_crash() {
        let (bytes, _) = written(&sample());
        for i in 0..bytes.len() {
            for flip in [0x01, 0x80, 0xff] {
                let mut damaged = bytes.clone();
                damaged[i] ^= flip;
                open_and_read_all(damaged);
            }
        }
        for len in 0..bytes.len() {
            open_and_read_all(bytes[..len].to_vec());
            open_and_read_all(bytes[bytes.len() - len..].to_vec());
        }
    }

    #[test]
    fn rejects_impossible_header_values() {
        let (bytes, _) = written(&sample());
        let set_u64 = |bytes: &mut Vec<u8>, at: usize, v: u64| {
            bytes[at..at + 8].copy_from_slice(&v.to_le_bytes());
        };
        // Trailer pack size smaller than a header.
        let mut tiny = bytes.clone();
        let trailer = tiny.len() - TRAILER_SIZE;
        set_u64(&mut tiny, trailer, 2);
        assert!(matches!(Pack::from_bytes(tiny), Err(VfsError::Corrupt(_))));
        // A file count the table of contents cannot hold (the header has no checksum).
        let mut huge = bytes.clone();
        huge[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(Pack::from_bytes(huge), Err(VfsError::Corrupt(_))));
    }

    #[test]
    fn found_at_the_end_of_a_program() {
        let (pack, _) = written(&sample());
        let mut program = b"\x7fELF pretend program bytes".repeat(100);
        program.extend_from_slice(&pack);
        let found = Pack::from_bytes(program).unwrap();
        assert_eq!(found.read("audio/hit.wav").unwrap().len(), 256);

        let dir = std::env::temp_dir().join(format!("ab-vfs-pack-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("game");
        std::fs::write(&exe, b"program").unwrap();
        assert!(!Pack::is_pack(&exe));
        let out = dir.join("game-packed");
        append_to_executable(&exe, &sample(), &out).unwrap();
        assert!(Pack::is_pack(&out));
        let pack = Pack::open(&out).unwrap();
        assert_eq!(pack.len(), 4);
        assert!(std::fs::read(&out).unwrap().starts_with(b"program"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn packs_a_directory() {
        let dir = std::env::temp_dir().join(format!("ab-vfs-dir-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("a.txt"), b"a").unwrap();
        std::fs::write(dir.join("sub/b.txt"), b"b").unwrap();
        std::fs::write(dir.join(".git/config"), b"x").unwrap();
        let mut writer = PackWriter::new();
        assert_eq!(writer.add_directory(&dir).unwrap(), 2, "hidden skipped");
        let (bytes, _) = written(&writer);
        let pack = Pack::from_bytes(bytes).unwrap();
        assert_eq!(pack.read("sub/b.txt").unwrap(), b"b");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
