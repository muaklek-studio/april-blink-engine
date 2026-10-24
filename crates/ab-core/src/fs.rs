//! Crash-safe file writes.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Writes `contents` to `path` so that a crash or power cut leaves either the old file or
/// the new one, never a half-written mix: the data goes to a temporary file next to `path`,
/// is flushed to disk, then renamed over `path`.
pub fn atomic_write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    let temp = temp_path(path)?;
    let written = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(contents.as_ref())?;
        file.sync_all()
    })();
    let result = written.and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    // The rename itself reaches the disk once the folder is synced (Unix; a power cut could
    // otherwise bring the old file back).
    #[cfg(unix)]
    if result.is_ok() {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
        }
    }
    result
}

/// Copies the current `path` (if any) to `backup`, then [`atomic_write`]s `contents` to it.
/// The backup keeps the previous version, for when the new one turns out to be wrong.
pub fn write_with_backup(
    path: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
    backup: impl AsRef<Path>,
) -> io::Result<()> {
    let (path, backup) = (path.as_ref(), backup.as_ref());
    if path.is_file() {
        if let Some(dir) = backup.parent() {
            std::fs::create_dir_all(dir)?;
        }
        atomic_write(backup, std::fs::read(path)?)?;
    }
    atomic_write(path, contents)
}

/// `dir/.name.<pid>.tmp`: same folder (so the rename stays on one file system), hidden from
/// asset scans.
fn temp_path(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a file path", path.display()),
        )
    })?;
    // One temporary per process: two writers to the same file never share it.
    let mut temp = std::ffi::OsString::from(".");
    temp.push(name);
    temp.push(format!(".{}.tmp", std::process::id()));
    Ok(path.with_file_name(temp))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ab-fs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_and_replaces_without_leftovers() {
        let dir = temp_dir("atomic");
        let file = dir.join("level.abscene");
        atomic_write(&file, "first").unwrap();
        atomic_write(&file, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "second");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["level.abscene"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_write_keeps_the_old_file() {
        let dir = temp_dir("fail");
        let file = dir.join("scene.toml");
        std::fs::write(&file, "old").unwrap();
        // The temporary file cannot be created where a folder already has its name.
        std::fs::create_dir(temp_path(&file).unwrap()).unwrap();
        assert!(atomic_write(&file, "new").is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "old");
        assert!(atomic_write(Path::new("/"), "x").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn backup_keeps_the_previous_version() {
        let dir = temp_dir("backup");
        let file = dir.join("a.toml");
        let backup = dir.join("backups/a.toml.bak");
        write_with_backup(&file, "v1", &backup).unwrap();
        assert!(!backup.exists(), "nothing to back up yet");
        write_with_backup(&file, "v2", &backup).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "v1");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v2");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
