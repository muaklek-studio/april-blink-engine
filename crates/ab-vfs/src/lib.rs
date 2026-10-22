//! Virtual file system: where a game's assets come from.
//!
//! * [`DirSource`]: a directory on disk (development).
//! * [`PackSource`]: an `.abp` pack ([`Pack`]), either a file next to the game or appended to
//!   the executable itself (single-file distribution).
//! * [`Vfs`]: several sources stacked; later mounts win, so a mod or patch directory can
//!   override packed files.
//!
//! ```
//! use ab_vfs::{FileSource, Pack, PackSource, PackWriter, Vfs};
//!
//! let mut writer = PackWriter::new();
//! writer.add("scenes/main.toml", b"[[node]]\nname = \"Main\"\n".to_vec());
//! let mut bytes = Vec::new();
//! writer.write_to(&mut bytes).unwrap();
//!
//! let mut vfs = Vfs::new();
//! vfs.mount(PackSource::new(Pack::from_bytes(bytes).unwrap()));
//! assert!(vfs.exists("scenes/main.toml"));
//! assert_eq!(vfs.read("scenes/main.toml").unwrap()[..8], *b"[[node]]");
//! ```

mod pack;
mod path;
mod source;

pub use pack::{
    append_to_executable, Compression, Pack, PackEntry, PackStats, PackWriter, PACK_EXTENSION,
    PROJECT_DIR,
};
pub use path::normalize;
pub use source::{DirSource, FileSource, PackSource, Vfs};

/// Errors of the file system and packs.
#[derive(Debug, thiserror::Error)]
pub enum VfsError {
    #[error("`{0}` not found")]
    NotFound(String),
    #[error("invalid path `{0}` (empty or escaping the root)")]
    InvalidPath(String),
    #[error("not an April Blink pack: {0}")]
    NotAPack(String),
    #[error("corrupt pack: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type VfsResult<T> = Result<T, VfsError>;
