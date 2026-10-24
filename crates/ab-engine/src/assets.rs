//! Where the game's assets come from, in development and once shipped.
//!
//! [`mount_assets`] stacks, lowest first:
//!
//! 1. a pack appended to the game's executable (single-file builds, `cargo xtask export
//!    --single-file`);
//! 2. the chunks of an exported game: [`CHUNK_DIR`] next to the executable, holding an index
//!    ([`ab_vfs::ChunkedSource`]; patched by replacing chunks and the index);
//! 3. the asset directory ([`EngineConfig::asset_root`](crate::EngineConfig), looked up in the
//!    working directory, then next to the executable), so during development files are read
//!    from disk. A directory holding an index is mounted as chunks.
//!
//! `AB_ASSETS=<directory, chunk folder or single-file game>` replaces all of this (testing a
//! build against other data).
//!
//! The project's own files (`game.toml`, the input map) are not assets: during development
//! they are read from the project folder ([`read_project_file`]), and exports pack them under
//! [`PROJECT_DIR`], out of the way of the game's assets.

use ab_vfs::{is_chunked, ChunkedSource, DirSource, FileSource, Pack, PackSource, Vfs, CHUNK_DIR};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

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
    let chunks = exported_chunks(executable);
    let directory = if asset_root.is_absolute() || asset_root.is_dir() {
        Some(asset_root.to_path_buf()).filter(|d| d.is_dir())
    } else {
        executable
            .and_then(|exe| exe.parent())
            .map(|dir| dir.join(asset_root))
            .filter(|d| d.is_dir())
    };
    // The exported game's chunk folder is usually the asset directory too: mounted once.
    let same = |dir: &Path| {
        chunks.as_deref().is_some_and(|chunks| {
            std::fs::canonicalize(chunks).ok() == std::fs::canonicalize(dir).ok()
        })
    };
    if let Some(dir) = directory.filter(|dir| !same(dir)) {
        mount_path(&mut vfs, &dir);
    }
    vfs
}

/// [`CHUNK_DIR`] next to `executable`, when it holds an index.
fn exported_chunks(executable: Option<&Path>) -> Option<PathBuf> {
    executable?
        .parent()
        .map(|dir| dir.join(CHUNK_DIR))
        .filter(|dir| is_chunked(dir))
}

/// The packs of the game whose executable is `executable`: appended to it, then its chunks.
fn mount_packs(vfs: &mut Vfs, executable: Option<&Path>) {
    if let Some(exe) = executable.filter(|exe| Pack::is_pack(exe)) {
        mount_path(vfs, exe);
    }
    if let Some(chunks) = exported_chunks(executable) {
        mount_path(vfs, &chunks);
    }
}

fn mount_path(vfs: &mut Vfs, path: &Path) {
    if is_chunked(path) {
        match ChunkedSource::open(path) {
            Ok(chunks) => {
                vfs.mount(chunks);
            }
            Err(err) => warn!(path = %path.display(), "cannot read asset chunk index: {err}"),
        }
        return;
    }
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

/// Reads the project file `name` (`game.toml`, `input.toml`): from the game's pack or chunks
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

/// The packs project files can come from (`AB_ASSETS`, when it names a single-file game or
/// a chunk folder, instead).
fn project_packs(executable: Option<&Path>) -> Vfs {
    let mut vfs = Vfs::new();
    match std::env::var_os("AB_ASSETS").map(PathBuf::from) {
        Some(over) if over.is_file() || is_chunked(&over) => mount_path(&mut vfs, &over),
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

    /// `dir/game`: a program with `pack` appended.
    fn single_file_game(dir: &Path, pack: &PackWriter) -> PathBuf {
        let plain = dir.join("plain");
        std::fs::write(&plain, b"program").unwrap();
        let exe = dir.join("game");
        ab_vfs::append_to_executable(&plain, pack, &exe).unwrap();
        exe
    }

    #[test]
    fn a_single_file_game_finds_the_pack_inside_itself() {
        let dir = temp("single");
        let exe = single_file_game(&dir, &pack_with("a.txt", "inside"));
        let vfs = mount_assets(Path::new("missing-assets"), Some(&exe));
        assert_eq!(vfs.read("a.txt").unwrap(), b"inside");
        assert_eq!(vfs.source_count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn project_files_come_from_the_pack_else_the_project_folder() {
        let dir = temp("project-files");
        let mut pack = pack_with("game.toml", "an asset");
        pack.add(".project/game.toml", b"packed".to_vec());
        let exe = single_file_game(&dir, &pack);
        let plain = dir.join("plain");
        let project = dir.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("game.toml"), b"on disk").unwrap();
        // Development (a program without a pack): the project folder, as given or next to
        // the executable.
        assert_eq!(
            read_project_file(&project, Some(&plain), "game.toml").unwrap(),
            b"on disk"
        );
        assert_eq!(
            read_project_file(Path::new("project"), Some(&plain), "game.toml").unwrap(),
            b"on disk"
        );
        assert!(read_project_file(&project, Some(&plain), "input.toml").is_err());
        assert!(!is_packed(Some(&plain), "game.toml"));
        // Shipped: the pack's project folder wins, and plain assets are not project files.
        assert_eq!(
            read_project_file(&project, Some(&exe), "game.toml").unwrap(),
            b"packed"
        );
        assert!(is_packed(Some(&exe), "game.toml"));
        assert!(!is_packed(Some(&exe), "input.toml"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_exported_game_reads_its_chunks() {
        let dir = temp("chunked");
        let exe = dir.join("game");
        std::fs::write(&exe, b"program").unwrap();
        let mut pack = pack_with("models/a.txt", "packed");
        pack.add(".project/game.toml", b"packed".to_vec());
        pack.write_chunks(&dir.join(CHUNK_DIR)).unwrap();
        // Found next to the game whatever the working directory, and mounted once even
        // though it is also the asset directory.
        for root in [PathBuf::from("missing-assets"), dir.join(CHUNK_DIR)] {
            let vfs = mount_assets(&root, Some(&exe));
            assert_eq!(vfs.source_count(), 1, "{:?}", vfs.describe());
            assert_eq!(vfs.read("models/a.txt").unwrap(), b"packed");
            assert!(vfs.describe()[0].starts_with("chunks:"));
            assert!(
                !vfs.exists(ab_vfs::MANIFEST_FILE),
                "chunk files are not assets"
            );
        }
        assert!(is_packed(Some(&exe), "game.toml"));
        assert_eq!(
            read_project_file(Path::new("missing"), Some(&exe), "game.toml").unwrap(),
            b"packed"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn development_reads_the_directory() {
        let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let vfs = mount_assets(&root, None);
        assert!(vfs.exists("slice/level.abscene"));
        assert!(list_assets(&vfs).iter().any(|p| p == "models/crate.glb"));
    }
}
