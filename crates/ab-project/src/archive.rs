//! Release archives of an exported folder: `.zip` for Windows players, `.tar.gz` for Linux
//! (keeps the executable bit). Both are deterministic: sorted paths, fixed timestamps.

use anyhow::{bail, Context as _, Result};
use flate2::write::{DeflateEncoder, GzEncoder};
use flate2::Compression;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Every file under `dir`, sorted, as (path relative to `dir` with `/`, absolute path).
fn files(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(dir)?
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((relative, path));
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).map_or(0o644, |m| m.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn mode(path: &Path) -> u32 {
    let exe = path
        .extension()
        .is_none_or(|e| e.eq_ignore_ascii_case("exe"));
    if exe {
        0o755
    } else {
        0o644
    }
}

/// Packs `dir` into `<archive>.tar.gz`, everything inside a top folder named `root`.
pub fn tar_gz(dir: &Path, root: &str, archive: &Path) -> Result<()> {
    let file = std::fs::File::create(archive)
        .with_context(|| format!("creating {}", archive.display()))?;
    let mut tar = tar::Builder::new(GzEncoder::new(file, Compression::best()));
    tar.mode(tar::HeaderMode::Deterministic);
    for (relative, path) in files(dir)? {
        let data = std::fs::read(&path)?;
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(mode(&path));
        header.set_mtime(0);
        header.set_entry_type(tar::EntryType::Regular);
        tar.append_data(&mut header, format!("{root}/{relative}"), data.as_slice())?;
    }
    tar.into_inner()?.finish()?.sync_all()?;
    Ok(())
}

/// Packs `dir` into a `.zip`, everything inside a top folder named `root`. Deflated, UTF-8
/// names, Unix permissions kept (so a Linux build zipped this way still runs).
pub fn zip(dir: &Path, root: &str, archive: &Path) -> Result<()> {
    // 1980-01-01 00:00, the earliest DOS date: reproducible archives.
    const DOS_TIME: u16 = 0;
    const DOS_DATE: u16 = (1 << 5) | 1;
    const UTF8_NAMES: u16 = 1 << 11;
    const DEFLATE: u16 = 8;
    const VERSION: u16 = 20;

    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    let files = files(dir)?;
    for (relative, path) in &files {
        let data = std::fs::read(path)?;
        let name = format!("{root}/{relative}");
        let crc = crc32fast::hash(&data);
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&data)?;
        let compressed = encoder.finish()?;
        let offset = out.len();
        if offset > u32::MAX as usize || data.len() > u32::MAX as usize {
            bail!("{name}: archives over 4 GB are not supported");
        }
        let fields = |buf: &mut Vec<u8>| {
            buf.extend_from_slice(&VERSION.to_le_bytes());
            buf.extend_from_slice(&UTF8_NAMES.to_le_bytes());
            buf.extend_from_slice(&DEFLATE.to_le_bytes());
            buf.extend_from_slice(&DOS_TIME.to_le_bytes());
            buf.extend_from_slice(&DOS_DATE.to_le_bytes());
            buf.extend_from_slice(&crc.to_le_bytes());
            buf.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&(name.len() as u16).to_le_bytes());
            buf.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        };
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        fields(&mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&compressed);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        // Made by: Unix (3), so the external attributes carry permissions.
        central.extend_from_slice(&((3u16 << 8) | VERSION).to_le_bytes());
        fields(&mut central);
        central.extend_from_slice(&0u16.to_le_bytes()); // comment length
        central.extend_from_slice(&0u16.to_le_bytes()); // disk number
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        let external = (0o100_000 | mode(path)) << 16; // regular file + permissions
        central.extend_from_slice(&external.to_le_bytes());
        central.extend_from_slice(&(offset as u32).to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    if files.len() > u16::MAX as usize {
        bail!("too many files for a zip archive");
    }
    let central_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    std::fs::write(archive, out).with_context(|| format!("writing {}", archive.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn sample() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ab-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("licenses")).unwrap();
        std::fs::write(dir.join("game.abp"), vec![7u8; 10_000]).unwrap();
        std::fs::write(dir.join("README.txt"), "สวัสดี hello\n".repeat(20)).unwrap();
        std::fs::write(dir.join("licenses/OFL.txt"), "license").unwrap();
        dir
    }

    fn tool(name: &str) -> bool {
        Command::new(name).arg("--help").output().is_ok()
    }

    #[test]
    fn zip_and_tar_gz_are_deterministic_and_readable() {
        let dir = sample();
        let out = dir.with_extension("out");
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        let (a, b) = (out.join("a.zip"), out.join("b.zip"));
        zip(&dir, "game", &a).unwrap();
        zip(&dir, "game", &b).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        let tgz = out.join("game.tar.gz");
        tar_gz(&dir, "game", &tgz).unwrap();

        // Read them back with the system tools where available.
        if tool("unzip") {
            let test = Command::new("unzip").arg("-t").arg(&a).output().unwrap();
            let log = String::from_utf8_lossy(&test.stdout);
            assert!(test.status.success(), "{log}");
            assert!(log.contains("game/licenses/OFL.txt"), "{log}");
        }
        if tool("tar") {
            let list = Command::new("tar").arg("-tzf").arg(&tgz).output().unwrap();
            let names = String::from_utf8_lossy(&list.stdout);
            assert!(list.status.success());
            assert_eq!(
                names.lines().collect::<Vec<_>>(),
                ["game/README.txt", "game/game.abp", "game/licenses/OFL.txt"]
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&out);
    }
}
