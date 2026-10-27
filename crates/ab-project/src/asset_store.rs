//! A project's binary assets in git: images, models, sounds and fonts are kept as
//! compressed files in `packed-assets/`, one per asset, stored with Git LFS; the files
//! themselves are unpacked next to the scenes and left out of git. Text assets (scenes,
//! materials, clips, locales) stay ordinary files, so git can still show and merge their
//! changes.
//!
//! ```text
//! my-game/
//!   assets/models/crate.glb              unpacked: what the editor and tools use (not in git)
//!   assets/scenes/level.abscene          text: in git as it is
//!   packed-assets/models/crate.glb.abp   the model, Deflate-compressed (Git LFS)
//!   packed-assets/textures/wall.png.abp  the image as it is: PNG is compressed already
//!   .ablink/asset-sync.txt               what was last in step (not in git)
//! ```
//!
//! One file per asset: an edit uploads that asset again, not its whole folder, and Git LFS
//! keeps only the versions still in use on each machine. Each is an `.abp` pack holding one
//! file with its checksum, compressed only when that saves at least 5 % (so PNG, JPEG, Ogg
//! and MP3 are stored as they are). Git LFS keeps what it is given as it is: compressing
//! first is what saves space and transfer there. A project can do without Git LFS
//! (`asset_lfs = false` in `game.toml`); the packs are then ordinary files in git.
//!
//! [`sync`] brings both sides in step, the way git itself would with a merge:
//!
//! * a pack changed and the file did not (a teammate's update after `git pull`): the file
//!   is unpacked; a file the packs no longer hold (deleted by a teammate) is removed;
//! * a file changed and its pack did not (an edit): its pack is written again;
//! * both changed (a conflict): the file is kept and the packed version is saved in
//!   `.ablink/conflicts/` so nothing is lost.
//!
//! Packs are written only when what they hold changes, and the same file always gives the
//! same bytes, so git sees a change only for the assets that really changed. Packs of an
//! older layout (one per folder) are replaced by per-asset packs on the next sync.

use crate::Project;
use ab_vfs::{Compression, Pack, PackWriter};
use anyhow::{Context as _, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Folder of the project holding the packed assets (in git).
pub const PACK_DIR: &str = "packed-assets";

/// Folder of the project for what the editor makes and keeps for itself: caches, logs,
/// backups, the sync state. Never in git.
pub const DATA_DIR: &str = ".ablink";

/// What Git LFS leaves in place of a file it has not downloaded.
const LFS_POINTER: &[u8] = b"version https://git-lfs.github.com/spec/";
const STATE_FILE: &str = "asset-sync.txt";
const STATE_HEADER: &str = "# April Blink asset sync v1: hash size modified path";
const CONFLICTS_DIR: &str = "conflicts";
/// Under the data folder: asset files a sync removed or replaced, kept in case.
const TRASH_DIR: &str = "trash";
/// Under the data folder: held during a sync, so the editor and an export never sync at once.
const LOCK_FILE: &str = "asset-sync.lock";

/// Extensions (lowercase) of the assets kept packed rather than as files in git.
pub const PACKED_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "tga", "bmp", "webp", "hdr", "exr", "ktx2", "dds", "glb", "bin", "fbx",
    "obj", "wav", "ogg", "mp3", "flac", "ttf", "otf",
];

/// Whether `path` is an asset kept packed.
pub fn is_packed(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, extension)| {
        PACKED_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
    })
}

/// What [`sync`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Files unpacked: new from a pack, or updated by one.
    pub unpacked: Vec<String>,
    /// Files removed because the packs no longer hold them.
    pub removed: Vec<String>,
    /// Assets packed (new or changed here).
    pub packed: Vec<String>,
    /// Packs removed: their assets are gone (or were packed in an older layout).
    pub dropped: Vec<String>,
    /// Files changed both here and in a pack: kept, the packed version saved aside.
    pub conflicts: Vec<String>,
    /// Packed assets in the project.
    pub files: usize,
    /// Their size, and the size of all packs.
    pub original_bytes: u64,
    pub pack_bytes: u64,
}

impl SyncReport {
    /// Whether anything changed on disk.
    pub fn changed(&self) -> bool {
        !(self.unpacked.is_empty()
            && self.removed.is_empty()
            && self.packed.is_empty()
            && self.dropped.is_empty())
    }

    /// One line for a status bar.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.unpacked.is_empty() {
            parts.push(format!("{} unpacked", self.unpacked.len()));
        }
        if !self.removed.is_empty() {
            parts.push(format!("{} removed", self.removed.len()));
        }
        if !self.packed.is_empty() {
            parts.push(format!("{} packed", self.packed.len()));
        }
        if !self.conflicts.is_empty() {
            parts.push(format!("{} conflict(s)", self.conflicts.len()));
        }
        if parts.is_empty() {
            parts.push("up to date".into());
        }
        let saved = self.original_bytes.saturating_sub(self.pack_bytes);
        format!(
            "Packed assets: {} ({} files, {} in git, {} saved)",
            parts.join(", "),
            self.files,
            human_bytes(self.pack_bytes),
            human_bytes(saved),
        )
    }
}

fn human_bytes(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / f64::from(1 << 30)),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / f64::from(1 << 20)),
        b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

/// A packed asset as last seen on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Record {
    /// FNV-1a 64 of the bytes (the packs' checksum).
    hash: u64,
    size: u64,
    /// Nanoseconds since 1970: with the size, says whether the hash must be computed again.
    modified: u128,
}

/// The packed assets as they were when the files and the packs were last in step.
#[derive(Debug, Default, PartialEq, Eq)]
struct State {
    files: BTreeMap<String, Record>,
}

impl State {
    fn read(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let files = text
            .lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| {
                let mut parts = line.splitn(4, ' ');
                let hash = u64::from_str_radix(parts.next()?, 16).ok()?;
                let size = parts.next()?.parse().ok()?;
                let modified = parts.next()?.parse().ok()?;
                let path = parts.next()?.to_owned();
                Some((
                    path,
                    Record {
                        hash,
                        size,
                        modified,
                    },
                ))
            })
            .collect();
        Self { files }
    }

    fn write(&self, path: &Path) -> Result<()> {
        let mut text = String::from(STATE_HEADER);
        text.push('\n');
        for (file, r) in &self.files {
            text.push_str(&format!(
                "{:016x} {} {} {file}\n",
                r.hash, r.size, r.modified
            ));
        }
        write_replacing(path, text.as_bytes())
    }
}

/// The pack of the asset `path` (relative to the assets): `<path>.abp` in the pack folder.
fn pack_file(pack_dir: &Path, path: &str) -> PathBuf {
    pack_dir.join(format!("{path}.{}", ab_vfs::PACK_EXTENSION))
}

fn modified_nanos(metadata: &std::fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

/// Every packed asset under `assets` (hidden files and folders skipped), hashed: a file
/// whose size and time match `known` keeps its known hash.
fn scan(assets: &Path, known: &State) -> Result<BTreeMap<String, Record>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![assets.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if dir == assets && err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(files)
            }
            Err(err) => return Err(err).with_context(|| format!("reading {}", dir.display())),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = relative(assets, &path);
            if !kind.is_file() || !is_packed(&relative) {
                continue;
            }
            let metadata = entry.metadata()?;
            let (size, modified) = (metadata.len(), modified_nanos(&metadata));
            let hash = match known.files.get(&relative) {
                Some(r) if r.size == size && r.modified == modified && modified != 0 => r.hash,
                _ => hash_file(&path)?,
            };
            files.insert(
                relative,
                Record {
                    hash,
                    size,
                    modified,
                },
            );
        }
    }
    Ok(files)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn hash_file(path: &Path) -> Result<u64> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(ab_vfs::fnv1a64(&bytes))
}

/// Every pack under `pack_dir`, and the Git LFS pointers left in place of packs not
/// downloaded.
fn pack_files(pack_dir: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let (mut packs, mut pointers) = (Vec::new(), Vec::new());
    let mut stack = vec![pack_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if dir == pack_dir && err.kind() == std::io::ErrorKind::NotFound => continue,
            // A folder skipped would count its packs as deleted, and their assets with them.
            Err(err) => return Err(err).with_context(|| format!("reading {}", dir.display())),
        };
        for entry in entries {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e == ab_vfs::PACK_EXTENSION)
            {
                if is_lfs_pointer(&path) {
                    pointers.push(path);
                } else {
                    packs.push(path);
                }
            }
        }
    }
    packs.sort();
    pointers.sort();
    Ok((packs, pointers))
}

/// Whether `path` is a Git LFS pointer (a small text file naming the real content).
fn is_lfs_pointer(path: &Path) -> bool {
    use std::io::Read as _;
    let mut start = [0u8; LFS_POINTER.len()];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut start))
        .is_ok_and(|()| start == LFS_POINTER)
}

/// What the packs hold: path → (hash, the pack).
fn packed(packs: &[(PathBuf, Pack)]) -> BTreeMap<String, (u64, usize)> {
    let mut files = BTreeMap::new();
    for (index, (_, pack)) in packs.iter().enumerate() {
        for entry in pack.entries() {
            files.insert(entry.path.clone(), (entry.checksum(), index));
        }
    }
    files
}

/// Writes `path` through a temporary file, so it is the old file or the new one, never
/// half of either.
/// Whether `path` (from a pack) stays inside the folder it is joined to: plain names only,
/// no `..`, roots or drive prefixes (`C:` would replace the base path on Windows).
fn is_safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(':')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// Moves asset `path` into the data folder's trash (replacing an older copy there): a
/// sync never destroys a file it cannot get back.
fn trash(data: &Path, assets: &Path, path: &str) -> Result<()> {
    let file = assets.join(path);
    let kept = data.join(TRASH_DIR).join(path);
    if let Some(parent) = kept.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(&kept);
    if std::fs::rename(&file, &kept).is_err() {
        // Another volume: copy, then remove.
        std::fs::copy(&file, &kept).with_context(|| format!("keeping {}", file.display()))?;
        std::fs::remove_file(&file).with_context(|| format!("removing {}", file.display()))?;
    }
    Ok(())
}

fn write_replacing(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    // One temporary per process: two writers never share it.
    name.push(format!(".{}.partial", std::process::id()));
    let temporary = path.with_file_name(name);
    std::fs::write(&temporary, bytes)
        .with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

impl Project {
    /// The project's own data folder (`.ablink`): caches, logs, backups. Not in git.
    pub fn data_dir(&self) -> PathBuf {
        self.root.join(DATA_DIR)
    }

    /// Where the packed assets are kept (`packed-assets`).
    pub fn pack_dir(&self) -> PathBuf {
        self.root.join(PACK_DIR)
    }
}

/// One sync at a time in a process (the editor's timer, a commit, an export).
static SYNC: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Brings the project's packed assets and its files in step (see the [module docs](self)).
pub fn sync(project: &Project) -> Result<SyncReport> {
    let _one_at_a_time = SYNC.lock().unwrap_or_else(|e| e.into_inner());
    let assets = &project.assets;
    let pack_dir = project.pack_dir();
    let data = project.data_dir();
    // And one at a time between processes (`ab-editor --export` syncs too).
    std::fs::create_dir_all(&data).with_context(|| format!("creating {}", data.display()))?;
    let lock = std::fs::File::create(data.join(LOCK_FILE))
        .with_context(|| format!("creating {}", data.join(LOCK_FILE).display()))?;
    lock.lock().context("waiting for another asset sync")?;
    let state_path = data.join(STATE_FILE);
    let base = State::read(&state_path);
    let mut report = SyncReport::default();
    if !assets.is_dir() && !base.files.is_empty() {
        // A renamed or mistyped assets folder is not "every asset deleted".
        anyhow::bail!(
            "the assets folder {} is missing: {} assets were there at the last sync",
            assets.display(),
            base.files.len()
        );
    }

    let (mut files, mut pointers) = pack_files(&pack_dir)?;
    if !pointers.is_empty() && lfs_available() {
        // A clone or pull without Git LFS's download: fetch the real packs.
        lfs_pull(&project.root)?;
        (files, pointers) = pack_files(&pack_dir)?;
    }
    if let Some(first) = pointers.first() {
        anyhow::bail!(
            "{} packed assets are Git LFS pointers, not downloaded ({}…): {}",
            pointers.len(),
            relative(&pack_dir, first),
            if lfs_available() {
                "run `git lfs pull` in the project"
            } else {
                LFS_INSTALL
            }
        );
    }
    let packs: Vec<(PathBuf, Pack)> = files
        .into_iter()
        .map(|path| {
            Pack::open(&path)
                .map(|pack| (path.clone(), pack))
                .with_context(|| format!("reading {}", path.display()))
        })
        .collect::<Result<_>>()?;
    let in_packs = packed(&packs);
    let mut local = scan(assets, &base)?;

    // From the packs: what changed there and not here.
    for (path, &(hash, index)) in &in_packs {
        if !is_safe_relative(path) {
            anyhow::bail!("a packed asset has an unsafe path: {path}");
        }
        let here = local.get(path).map(|r| r.hash);
        let before = base.files.get(path).map(|r| r.hash);
        let take = match (here, before) {
            (Some(here), _) if here == hash => false,
            // Unchanged here, updated in the pack.
            (Some(here), Some(before)) if here == before => true,
            // Changed here only: packed below.
            (Some(_), Some(before)) if before == hash => false,
            // Changed on both sides (or never in step): keep ours, save theirs.
            (Some(_), _) => {
                let bytes = packs[index].1.read(path)?;
                write_replacing(&data.join(CONFLICTS_DIR).join(path), &bytes)?;
                report.conflicts.push(path.clone());
                false
            }
            // Deleted here: the pack loses it below.
            (None, Some(before)) if before == hash => false,
            // New in the pack (or changed there after a delete here).
            (None, _) => true,
        };
        if take {
            let bytes = packs[index].1.read(path)?;
            let file = assets.join(path);
            if file.exists() {
                trash(&data, assets, path)?;
            }
            write_replacing(&file, &bytes)?;
            let metadata = std::fs::metadata(&file)?;
            local.insert(
                path.clone(),
                Record {
                    hash,
                    size: metadata.len(),
                    modified: modified_nanos(&metadata),
                },
            );
            report.unpacked.push(path.clone());
        }
    }
    // Gone from the packs (deleted by a teammate) and unchanged here: removed here too.
    for (path, before) in &base.files {
        if in_packs.contains_key(path) {
            continue;
        }
        if local.get(path).is_some_and(|r| r.hash == before.hash) {
            trash(&data, assets, path)?;
            local.remove(path);
            report.removed.push(path.clone());
        }
    }

    // To the packs: every asset whose pack does not hold it as it is now.
    let mut kept: BTreeSet<PathBuf> = BTreeSet::new();
    // Written once the open packs are dropped (Windows cannot replace a mapped file).
    let mut writes: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    let current: BTreeMap<&Path, Vec<(&str, u64)>> = packs
        .iter()
        .map(|(file, pack)| {
            let entries = pack
                .entries()
                .into_iter()
                .map(|e| (e.path.as_str(), e.checksum()))
                .collect();
            (file.as_path(), entries)
        })
        .collect();
    for (path, record) in &local {
        let file = pack_file(&pack_dir, path);
        if current.get(file.as_path()) == Some(&vec![(path.as_str(), record.hash)]) {
            kept.insert(file);
            continue;
        }
        let bytes = std::fs::read(assets.join(path))
            .with_context(|| format!("reading {}", assets.join(path).display()))?;
        let mut writer = PackWriter::new().with_compression(Compression::Deflate);
        writer.add(path, bytes);
        let mut packed = Vec::new();
        writer
            .write_to(&mut packed)
            .with_context(|| format!("packing {path}"))?;
        writes.push((file.clone(), packed));
        kept.insert(file);
        report.packed.push(path.clone());
    }
    // Packs of assets gone, or of the older one-per-folder layout. Opened packs are dropped
    // first (Windows cannot remove a mapped file).
    let stale: Vec<PathBuf> = packs
        .iter()
        .map(|(path, _)| path.clone())
        .filter(|path| !kept.contains(path))
        .collect();
    drop(current);
    drop(packs);
    for (file, packed) in writes {
        write_replacing(&file, &packed)?;
    }
    for path in stale {
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        report.dropped.push(relative(&pack_dir, &path));
        remove_empty_parents(&path, &pack_dir);
    }

    report.files = local.len();
    report.original_bytes = local.values().map(|r| r.size).sum();
    report.pack_bytes = kept
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|m| m.len())
        .sum();
    State { files: local }.write(&state_path)?;
    Ok(report)
}

/// What to tell someone without Git LFS.
pub const LFS_INSTALL: &str = "install Git LFS (https://git-lfs.com; on macOS `brew install \
     git-lfs`), then run `git lfs install` once";

/// Runs `git -C dir args…`; its standard output, or its error message.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = crate::process::command("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .context("cannot run git")?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git {}: {}", args.join(" "), error.trim())
    }
}

/// Whether Git LFS is installed (`git lfs` runs).
pub fn lfs_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        crate::process::command("git")
            .args(["lfs", "version"])
            .stdin(std::process::Stdio::null())
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// Whether `root` is inside a git repository.
fn in_repository(root: &Path) -> bool {
    git(root, &["rev-parse", "--git-dir"]).is_ok()
}

/// Makes the repository of `root` use Git LFS (its filters and push hook), so the packs
/// are stored there. Nothing to do outside a repository or without Git LFS (returns false).
pub fn lfs_setup(root: &Path) -> Result<bool> {
    if !lfs_available() || !in_repository(root) {
        return Ok(false);
    }
    git(root, &["lfs", "install", "--local"])?;
    Ok(true)
}

/// Downloads the Git LFS content of the repository of `root` (the real packs).
fn lfs_pull(root: &Path) -> Result<()> {
    if !lfs_setup(root)? {
        return Ok(());
    }
    git(root, &["lfs", "pull"]).map(drop)
}

/// Gets the project ready to commit its packed assets: refuses when they should go to Git
/// LFS and it is not installed (they would land in git itself), and sets the repository
/// up for it otherwise.
pub fn prepare_commit(project: &Project) -> Result<()> {
    if !project.lfs {
        return Ok(());
    }
    if !lfs_available() {
        anyhow::bail!(
            "the packed assets go to Git LFS, which is not installed: {LFS_INSTALL} \
             (or set `asset_lfs = false` in game.toml to keep them in git itself)"
        );
    }
    lfs_setup(&project.root).map(drop)
}

/// Removes the folders between `file` and `root` left empty.
fn remove_empty_parents(file: &Path, root: &Path) {
    let mut dir = file.parent();
    while let Some(current) = dir.filter(|d| *d != root && d.starts_with(root)) {
        if std::fs::remove_dir(current).is_err() {
            break;
        }
        dir = current.parent();
    }
}

const BLOCK_START: &str = "# >>> April Blink (kept up to date by the editor) >>>";
const BLOCK_END: &str = "# <<< April Blink <<<";

/// A gitignore pattern matching `extension` in any letter case (`*.[pP][nN][gG]`).
fn any_case(extension: &str) -> String {
    extension
        .chars()
        .map(|c| {
            if c.is_ascii_alphabetic() {
                format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase())
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// The project's lines of `.gitignore`: build output, the editor's data, and the unpacked
/// binary assets (their packs are what git keeps).
pub fn gitignore_block(project: &Project) -> String {
    let assets = relative(&project.root, &project.assets);
    let mut lines = vec![
        BLOCK_START.to_owned(),
        "# Build output, and the editor's caches, logs and backups.".to_owned(),
        "/target/".to_owned(),
        "/dist/".to_owned(),
        format!("/{DATA_DIR}/"),
        "*.partial".to_owned(),
        ".DS_Store".to_owned(),
        "Thumbs.db".to_owned(),
        format!("# Binary assets: git keeps them packed in {PACK_DIR}/, the editor unpacks them."),
    ];
    for extension in PACKED_EXTENSIONS {
        lines.push(format!("/{assets}/**/*.{}", any_case(extension)));
    }
    lines.push(BLOCK_END.to_owned());
    lines.join("\n") + "\n"
}

/// The project's lines of `.gitattributes`: packs go to Git LFS (or, without it, are
/// binary: no diffs, no merges, no delta search on compressed data), text assets keep one
/// line ending.
pub fn gitattributes_block(project: &Project) -> String {
    let packs = if project.lfs {
        "/packed-assets/** filter=lfs diff=lfs merge=lfs -text"
    } else {
        "/packed-assets/** binary -delta"
    };
    [
        BLOCK_START,
        packs,
        "*.abscene text eol=lf",
        "*.toml text eol=lf",
        "*.gltf text eol=lf",
        BLOCK_END,
    ]
    .join("\n")
        + "\n"
}

/// `text` with its April Blink block replaced by `block` (added at the end when missing).
fn with_block(text: &str, block: &str) -> String {
    let start = text.find(BLOCK_START);
    let end = text
        .find(BLOCK_END)
        .map(|at| at + BLOCK_END.len())
        .map(|at| at + usize::from(text[at..].starts_with('\n')));
    match (start, end) {
        (Some(start), Some(end)) if start < end => {
            format!("{}{block}{}", &text[..start], &text[end..])
        }
        _ if text.is_empty() => block.to_owned(),
        _ if text.ends_with('\n') => format!("{text}\n{block}"),
        _ => format!("{text}\n\n{block}"),
    }
}

/// Writes or updates the April Blink blocks of the project's `.gitignore` and
/// `.gitattributes`, keeping the user's own lines. Returns whether a file changed.
pub fn ensure_git_files(project: &Project) -> Result<bool> {
    let mut changed = false;
    for (name, block) in [
        (".gitignore", gitignore_block(project)),
        (".gitattributes", gitattributes_block(project)),
    ] {
        let path = project.root.join(name);
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        let new = with_block(&old, &block);
        if new != old {
            std::fs::write(&path, new).with_context(|| format!("writing {}", path.display()))?;
            changed = true;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> Project {
        let root = std::env::temp_dir().join(format!("ab-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets")).unwrap();
        Project {
            assets: root.join("assets"),
            root,
            title: name.into(),
            main_scene: None,
            input: None,
            config: None,
            audio: None,
            package: None,
            lfs: true,
            export: Default::default(),
        }
    }

    fn write(project: &Project, path: &str, bytes: &[u8]) {
        let file = project.assets.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    }

    fn read(project: &Project, path: &str) -> Option<Vec<u8>> {
        std::fs::read(project.assets.join(path)).ok()
    }

    /// A teammate's clone: the packs (as git would bring them), no files, no state.
    fn clone_of(project: &Project, name: &str) -> Project {
        let other = self::project(name);
        copy_dir(&project.pack_dir(), &other.pack_dir());
        other
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let path = entry.unwrap().path();
            let dest = to.join(path.file_name().unwrap());
            if path.is_dir() {
                copy_dir(&path, &dest);
            } else {
                std::fs::copy(&path, &dest).unwrap();
            }
        }
    }

    /// `git pull`: the packs become `from`'s.
    fn pull(into: &Project, from: &Project) {
        let _ = std::fs::remove_dir_all(into.pack_dir());
        copy_dir(&from.pack_dir(), &into.pack_dir());
    }

    #[test]
    fn packs_one_file_per_asset_and_only_binary_ones() {
        let p = project("layout");
        write(&p, "models/crate.glb", &[7; 4000]);
        write(&p, "models/props/barrel.png", b"png bytes");
        write(&p, "scenes/level.abscene", b"[[node]]");
        let report = sync(&p).unwrap();
        assert_eq!(
            report.packed,
            ["models/crate.glb", "models/props/barrel.png"]
        );
        assert_eq!(report.files, 2, "the scene stays a text file in git");
        let crate_pack = Pack::open(&p.pack_dir().join("models/crate.glb.abp")).unwrap();
        assert_eq!(
            crate_pack.entry("models/crate.glb").unwrap().compression,
            Compression::Deflate
        );
        let png = Pack::open(&p.pack_dir().join("models/props/barrel.png.abp")).unwrap();
        assert_eq!(
            png.entry("models/props/barrel.png").unwrap().compression,
            Compression::None,
            "compressed already: stored as it is"
        );
        assert!(report.pack_bytes < report.original_bytes, "{report:?}");
        // Nothing changed: nothing written, so git sees no change.
        let again = sync(&p).unwrap();
        assert!(!again.changed(), "{again:?}");
        // An edit rewrites that asset's pack only.
        let before = std::fs::read(p.pack_dir().join("models/props/barrel.png.abp")).unwrap();
        write(&p, "models/crate.glb", &[8; 4000]);
        let edit = sync(&p).unwrap();
        assert_eq!(edit.packed, ["models/crate.glb"]);
        let after = std::fs::read(p.pack_dir().join("models/props/barrel.png.abp")).unwrap();
        assert_eq!(before, after);
        std::fs::remove_dir_all(&p.root).unwrap();
    }

    #[test]
    fn folder_packs_of_the_older_layout_become_per_asset_packs() {
        let p = project("migrate");
        let mut old = PackWriter::new().with_compression(Compression::Deflate);
        old.add("models/crate.glb", b"crate".to_vec());
        old.add("models/tree.glb", b"tree".to_vec());
        let mut bytes = Vec::new();
        old.write_to(&mut bytes).unwrap();
        std::fs::create_dir_all(p.pack_dir()).unwrap();
        std::fs::write(p.pack_dir().join("models.abp"), bytes).unwrap();
        let report = sync(&p).unwrap();
        assert_eq!(report.unpacked.len(), 2);
        assert_eq!(report.dropped, ["models.abp"]);
        assert!(p.pack_dir().join("models/crate.glb.abp").is_file());
        assert!(p.pack_dir().join("models/tree.glb.abp").is_file());
        assert_eq!(read(&p, "models/tree.glb").unwrap(), b"tree");
        std::fs::remove_dir_all(&p.root).unwrap();
    }

    #[test]
    fn lfs_pointers_are_reported_not_mistaken_for_damage() {
        let p = project("pointers");
        let pointer = p.pack_dir().join("models/crate.glb.abp");
        std::fs::create_dir_all(pointer.parent().unwrap()).unwrap();
        std::fs::write(
            &pointer,
            "version https://git-lfs.github.com/spec/v1\noid sha256:00\nsize 5\n",
        )
        .unwrap();
        assert!(is_lfs_pointer(&pointer));
        // Not a repository: nothing to pull from, so it says what to do.
        let err = format!("{:#}", sync(&p).unwrap_err());
        assert!(err.contains("Git LFS pointers"), "{err}");
        std::fs::remove_dir_all(&p.root).unwrap();
    }

    #[test]
    fn a_fresh_clone_unpacks_everything_and_edits_travel_both_ways() {
        let mine = project("mine");
        write(&mine, "models/crate.glb", b"crate v1");
        write(&mine, "sounds/hit.wav", b"hit v1");
        sync(&mine).unwrap();

        let theirs = clone_of(&mine, "theirs");
        let report = sync(&theirs).unwrap();
        assert_eq!(report.unpacked.len(), 2);
        assert_eq!(read(&theirs, "models/crate.glb").unwrap(), b"crate v1");
        assert!(!report.changed() || report.packed.is_empty(), "{report:?}");

        // They change the crate and delete the sound; I pull.
        write(&theirs, "models/crate.glb", b"crate v2");
        std::fs::remove_file(theirs.assets.join("sounds/hit.wav")).unwrap();
        let report = sync(&theirs).unwrap();
        assert_eq!(report.packed, ["models/crate.glb"]);
        assert_eq!(report.dropped, ["sounds/hit.wav.abp"]);
        pull(&mine, &theirs);
        let report = sync(&mine).unwrap();
        assert_eq!(report.unpacked, ["models/crate.glb"]);
        assert_eq!(report.removed, ["sounds/hit.wav"]);
        assert_eq!(read(&mine, "models/crate.glb").unwrap(), b"crate v2");
        assert!(read(&mine, "sounds/hit.wav").is_none());
        assert!(report.packed.is_empty(), "already in step: {report:?}");
        for p in [mine, theirs] {
            std::fs::remove_dir_all(&p.root).unwrap();
        }
    }

    #[test]
    fn removed_and_replaced_files_are_kept_in_the_trash() {
        let mine = project("trash-mine");
        write(&mine, "models/crate.glb", b"crate v1");
        write(&mine, "sounds/hit.wav", b"hit v1");
        sync(&mine).unwrap();
        // The packs go back to having the sound only (a discard, or a teammate's delete).
        std::fs::remove_file(mine.pack_dir().join("models/crate.glb.abp")).unwrap();
        let report = sync(&mine).unwrap();
        assert_eq!(report.removed, ["models/crate.glb"]);
        assert_eq!(
            std::fs::read(mine.data_dir().join("trash/models/crate.glb")).unwrap(),
            b"crate v1"
        );
        std::fs::remove_dir_all(&mine.root).unwrap();
    }

    #[test]
    fn a_missing_assets_folder_is_an_error_not_a_mass_delete() {
        let mine = project("missing-assets");
        write(&mine, "models/crate.glb", b"crate v1");
        sync(&mine).unwrap();
        std::fs::rename(&mine.assets, mine.root.join("assets-moved")).unwrap();
        assert!(sync(&mine).is_err());
        assert!(mine.pack_dir().join("models/crate.glb.abp").is_file());
        std::fs::remove_dir_all(&mine.root).unwrap();
    }

    #[test]
    fn packed_paths_stay_inside_the_assets() {
        assert!(is_safe_relative("models/crate.glb"));
        for bad in [
            "",
            "../x.png",
            "/etc/x.png",
            "C:/x.png",
            "C:x.png",
            "a/../../b.png",
        ] {
            assert!(!is_safe_relative(bad), "{bad}");
        }
    }

    #[test]
    fn local_edits_are_never_overwritten() {
        let mine = project("edit-mine");
        write(&mine, "models/crate.glb", b"crate v1");
        write(&mine, "models/tree.glb", b"tree v1");
        sync(&mine).unwrap();
        let theirs = clone_of(&mine, "edit-theirs");
        sync(&theirs).unwrap();
        write(&theirs, "models/crate.glb", b"crate theirs");
        write(&theirs, "models/tree.glb", b"tree theirs");
        sync(&theirs).unwrap();

        // I edited the crate too (not synced yet), then pull.
        write(&mine, "models/crate.glb", b"crate mine");
        pull(&mine, &theirs);
        let report = sync(&mine).unwrap();
        assert_eq!(report.conflicts, ["models/crate.glb"]);
        assert_eq!(read(&mine, "models/crate.glb").unwrap(), b"crate mine");
        assert_eq!(
            std::fs::read(mine.data_dir().join("conflicts/models/crate.glb")).unwrap(),
            b"crate theirs",
            "their version is kept aside"
        );
        assert_eq!(read(&mine, "models/tree.glb").unwrap(), b"tree theirs");
        // My version goes into the pack.
        let pack = Pack::open(&mine.pack_dir().join("models/crate.glb.abp")).unwrap();
        assert_eq!(pack.read("models/crate.glb").unwrap(), b"crate mine");
        for p in [mine, theirs] {
            std::fs::remove_dir_all(&p.root).unwrap();
        }
    }

    #[test]
    fn git_files_keep_the_users_lines() {
        let p = project("git-files");
        std::fs::write(p.root.join(".gitignore"), "/notes.txt\n").unwrap();
        assert!(ensure_git_files(&p).unwrap());
        assert!(!ensure_git_files(&p).unwrap(), "already up to date");
        let ignore = std::fs::read_to_string(p.root.join(".gitignore")).unwrap();
        assert!(ignore.starts_with("/notes.txt\n"));
        assert!(ignore.contains("/.ablink/"));
        assert!(ignore.contains("/assets/**/*.[pP][nN][gG]"));
        assert!(!ignore.contains(PACK_DIR) || !ignore.contains(&format!("/{PACK_DIR}/\n")));
        assert_eq!(ignore.matches(BLOCK_START).count(), 1);
        // An older block is replaced, not added to.
        let old = format!("/notes.txt\n\n{BLOCK_START}\n/old\n{BLOCK_END}\n/after\n");
        std::fs::write(p.root.join(".gitignore"), old).unwrap();
        ensure_git_files(&p).unwrap();
        let ignore = std::fs::read_to_string(p.root.join(".gitignore")).unwrap();
        assert!(!ignore.contains("/old\n") && ignore.ends_with("/after\n"));
        let attributes = std::fs::read_to_string(p.root.join(".gitattributes")).unwrap();
        assert!(attributes.contains("/packed-assets/** filter=lfs diff=lfs merge=lfs -text"));
        let plain = Project {
            lfs: false,
            export: Default::default(),
            ..p.clone()
        };
        assert!(gitattributes_block(&plain).contains("/packed-assets/** binary -delta"));
        std::fs::remove_dir_all(&p.root).unwrap();
    }

    #[test]
    fn commits_need_git_lfs_only_when_the_project_uses_it() {
        let p = project("commit");
        let plain = Project {
            lfs: false,
            export: Default::default(),
            ..p.clone()
        };
        assert!(prepare_commit(&plain).is_ok());
        // Not a repository: with Git LFS there is nothing to set up, without it a refusal.
        match prepare_commit(&p) {
            Ok(()) => assert!(lfs_available()),
            Err(err) => {
                assert!(!lfs_available());
                assert!(format!("{err}").contains("Git LFS"), "{err}");
            }
        }
        std::fs::remove_dir_all(&p.root).unwrap();
    }

    #[test]
    fn packed_extensions_ignore_case() {
        assert!(is_packed("a/B.PNG") && is_packed("x.glb") && !is_packed("level.abscene"));
        assert_eq!(any_case("ktx2"), "[kK][tT][xX]2");
    }
}
