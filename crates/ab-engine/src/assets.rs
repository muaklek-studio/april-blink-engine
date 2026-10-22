//! Where the game's assets come from, in development and once shipped.
//!
//! [`mount_assets`] stacks, lowest first:
//!
//! 1. a pack appended to the game's executable (single-file builds, `cargo xtask export
//!    --single-file`);
//! 2. a pack next to the executable: `<executable name>.abp`, else `assets.abp`;
//! 3. the asset directory ([`EngineConfig::asset_root`](crate::EngineConfig), looked up in the
//!    working directory, then next to the executable), so during development files are read
//!    from disk, and in a shipped game a folder of loose files can patch or mod the pack.
//!
//! `AB_ASSETS=<directory or pack>` replaces all of this (testing a build against other data).
//!
//! The project's own files (`game.toml`, the input map) are not assets: during development
//! they are read from the project folder ([`read_project_file`]), and exports pack them under
//! [`PROJECT_DIR`], out of the way of the game's assets.

use ab_vfs::{DirSource, FileSource, Pack, PackSource, Vfs};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Name of the pack looked for next to the executable when none is named after it.
pub const DEFAULT_PACK: &str = "assets.abp";

/// Builds the asset file system for a game whose executable is `executable` (usually
/// `std::env::current_exe()`) and whose asset directory is `asset_root`.
pub fn mount_assets(asset_root: &Path, executable: Option<&Path>) -> Vfs {
    let vfs = mount(asset_root, executable);
    info!(sources = ?vfs.describe(), "assets mounted");
    vfs
}

/// [`mount_assets`] without the log line (reading settings before the engine starts).
pub(crate) fn mount(asset_root: &Path, executable: Option<&Path>) -> Vfs {
    if let Some(over) = std::env::var_os("AB_ASSETS").map(PathBuf::from) {
        let mut vfs = Vfs::new();
        mount_path(&mut vfs, &over);
        return vfs;
    }
    let mut vfs = discover(asset_root, executable);
    if vfs.source_count() == 0 {
        warn!(
            path = %asset_root.display(),
            "no asset directory or pack found; asset loads will fail"
        );
        vfs.mount(DirSource::new(asset_root));
    }
    vfs
}

/// The packs and asset directory that exist, lowest priority first.
fn discover(asset_root: &Path, executable: Option<&Path>) -> Vfs {
    let mut vfs = Vfs::new();
    mount_packs(&mut vfs, executable);
    let directory = if asset_root.is_absolute() || asset_root.is_dir() {
        Some(asset_root.to_path_buf()).filter(|d| d.is_dir())
    } else {
        executable
            .and_then(|exe| exe.parent())
            .map(|dir| dir.join(asset_root))
            .filter(|d| d.is_dir())
    };
    if let Some(dir) = directory {
        vfs.mount(DirSource::new(dir));
    }
    vfs
}

/// The packs of the game whose executable is `executable`: appended to it, then next to it.
fn mount_packs(vfs: &mut Vfs, executable: Option<&Path>) {
    if let Some(exe) = executable {
        if Pack::is_pack(exe) {
            mount_path(vfs, exe);
        }
        if let Some(dir) = exe.parent() {
            let named = exe
                .file_stem()
                .map(|stem| dir.join(stem).with_extension(ab_vfs::PACK_EXTENSION));
            let pack = named
                .filter(|p| p.is_file())
                .or_else(|| Some(dir.join(DEFAULT_PACK)).filter(|p| p.is_file()));
            if let Some(pack) = pack {
                mount_path(vfs, &pack);
            }
        }
    }
}

fn mount_path(vfs: &mut Vfs, path: &Path) {
    if path.is_dir() {
        vfs.mount(DirSource::new(path));
        return;
    }
    match PackSource::open(path) {
        Ok(pack) => {
            vfs.mount(pack);
        }
        Err(err) => warn!(path = %path.display(), "cannot open asset pack: {err}"),
    }
}

pub use ab_vfs::PROJECT_DIR;

/// Reads the project file `name` (`game.toml`, `input.toml`): from the game's pack
/// (`.project/<name>`), else from `project_dir` on disk (looked up like the asset directory:
/// as given, then next to the executable).
pub fn read_project_file(
    project_dir: &Path,
    executable: Option<&Path>,
    name: &str,
) -> anyhow::Result<Vec<u8>> {
    let packed = format!("{PROJECT_DIR}/{name}");
    let vfs = project_packs(executable);
    if vfs.exists(&packed) {
        return Ok(vfs.read(&packed)?);
    }
    let candidates = std::iter::once(project_dir.join(name)).chain(
        (!project_dir.is_absolute())
            .then(|| executable.and_then(Path::parent))
            .flatten()
            .map(|dir| dir.join(project_dir).join(name)),
    );
    for path in candidates {
        if path.is_file() {
            return std::fs::read(&path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()));
        }
    }
    anyhow::bail!(
        "{name} not found in {} or in the game's pack",
        project_dir.display()
    )
}

/// Whether the game's pack holds the project file `name` (a shipped game).
pub fn is_packed(executable: Option<&Path>, name: &str) -> bool {
    project_packs(executable).exists(&format!("{PROJECT_DIR}/{name}"))
}

/// The packs project files can come from (`AB_ASSETS`, when it names a pack, instead).
fn project_packs(executable: Option<&Path>) -> Vfs {
    let mut vfs = Vfs::new();
    match std::env::var_os("AB_ASSETS").map(PathBuf::from) {
        Some(over) if over.is_file() => mount_path(&mut vfs, &over),
        Some(_) => {}
        None => mount_packs(&mut vfs, executable),
    }
    vfs
}

/// The files the game can load, for tools and debug menus.
pub fn list_assets(vfs: &Vfs) -> Vec<String> {
    vfs.list()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_vfs::PackWriter;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ab-engine-assets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn pack_with(file: &str, text: &str) -> PackWriter {
        let mut writer = PackWriter::new();
        writer.add(file, text.as_bytes().to_vec());
        writer
    }

    #[test]
    fn a_shipped_game_reads_its_pack_and_loose_files_patch_it() {
        let dir = temp("shipped");
        let exe = dir.join("game");
        std::fs::write(&exe, b"program").unwrap();
        pack_with("main.toml", "packed")
            .write_file(&dir.join("game.abp"))
            .unwrap();
        // No asset folder: the pack alone.
        let vfs = mount_assets(Path::new("missing-assets"), Some(&exe));
        assert_eq!(vfs.read("main.toml").unwrap(), b"packed");
        assert_eq!(vfs.source_count(), 1);

        // A folder next to the game overrides single files.
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("assets/main.toml"), b"patched").unwrap();
        let vfs = mount_assets(&dir.join("assets"), Some(&exe));
        assert_eq!(vfs.read("main.toml").unwrap(), b"patched");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_single_file_game_finds_the_pack_inside_itself() {
        let dir = temp("single");
        let plain = dir.join("plain");
        std::fs::write(&plain, b"program").unwrap();
        let exe = dir.join("game");
        ab_vfs::append_to_executable(&plain, &pack_with("a.txt", "inside"), &exe).unwrap();
        // A default-named pack is found too, below the appended one.
        pack_with("b.txt", "beside")
            .write_file(&dir.join(DEFAULT_PACK))
            .unwrap();
        let vfs = mount_assets(Path::new("missing-assets"), Some(&exe));
        assert_eq!(vfs.read("a.txt").unwrap(), b"inside");
        assert_eq!(vfs.read("b.txt").unwrap(), b"beside");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn project_files_come_from_the_pack_else_the_project_folder() {
        let dir = temp("project-files");
        let exe = dir.join("game");
        std::fs::write(&exe, b"program").unwrap();
        let project = dir.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("game.toml"), b"on disk").unwrap();
        // Development: the project folder, as given or next to the executable.
        assert_eq!(
            read_project_file(&project, Some(&exe), "game.toml").unwrap(),
            b"on disk"
        );
        assert_eq!(
            read_project_file(Path::new("project"), Some(&exe), "game.toml").unwrap(),
            b"on disk"
        );
        assert!(read_project_file(&project, Some(&exe), "input.toml").is_err());
        // Shipped: the pack's project folder wins, and plain assets are not project files.
        let mut pack = pack_with("game.toml", "an asset");
        pack.add(".project/game.toml", b"packed".to_vec());
        pack.write_file(&dir.join("game.abp")).unwrap();
        assert_eq!(
            read_project_file(&project, Some(&exe), "game.toml").unwrap(),
            b"packed"
        );
        assert!(is_packed(Some(&exe), "game.toml"));
        assert!(!is_packed(Some(&exe), "input.toml"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn development_reads_the_directory() {
        let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let vfs = mount_assets(&root, None);
        assert!(vfs.exists("slice/level.toml"));
        assert!(list_assets(&vfs).iter().any(|p| p == "models/crate.glb"));
    }
}
