//! Exporting games: shippable copies for players, in a folder per platform.
//!
//! For each platform (this machine's by default; also Windows from Linux with `cargo xwin`):
//!
//! 1. builds the game:
//!    * a project ([`Game::Project`]): its script library in release mode, run by the
//!      **player** (`ab-player`, built with the `shipping` feature: no debug views, developer
//!      options ignored), renamed after the game. The player comes from next to the editor
//!      (`players/<platform>/`), from `AB_PLAYERS`, or is built from the engine's sources;
//!    * an engine game ([`Game::Engine`], Night Relics and the demo): its executable with the
//!      `dist` profile and `ab-engine/shipping`.
//!
//!    Windows builds link the C runtime statically, so players need no Visual C++
//!    redistributable;
//! 2. packs the asset directory into `<name>.abp` next to the executable, or, with
//!    `single_file`, appends the pack to the executable;
//! 3. with `extras`, adds a README and the licenses as files (they always ship inside the
//!    pack, and the game shows them with `--licenses`);
//! 4. verifies: the pack is read back in full, the exported game is run headless from an empty
//!    directory (so an export that still depends on the source tree fails here instead of on a
//!    player's PC) and Windows binaries are checked for DLLs players may not have;
//! 5. with `archive`, zips it (Windows) or makes a `.tar.gz` (Linux), ready to upload.
//!
//! Progress goes to a log callback, one line at a time: `=== <title> <version> for
//! <platform> ===` starts a platform, `exported <title> to <folder>` and `archive: <path>`
//! report results.

use crate::{archive, engine_root, process, Platform, Profile, Project};
use ab_vfs::{append_to_executable, Pack, PackWriter};
use anyhow::{bail, Context as _, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where export progress goes, a line at a time.
pub type Log<'a> = &'a mut dyn FnMut(&str);

macro_rules! say {
    ($log:expr, $($arg:tt)*) => {
        ($log)(&format!($($arg)*))
    };
}

/// The built-in font's license; it travels inside every pack.
const FONT_LICENSE: &str = include_str!("../../ab-ui/fonts/OFL.txt");

/// What to export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Game {
    /// A project: script library + player.
    Project(Project),
    /// A game executable of the engine's workspace.
    Engine {
        /// The engine workspace.
        workspace: PathBuf,
        package: String,
        binary: String,
        assets: PathBuf,
        /// `game.toml` and the input map, as (name in the project, file).
        project_files: Vec<(String, PathBuf)>,
        title: String,
        version: String,
    },
}

impl Game {
    /// Night Relics, from the engine's sources.
    pub fn slice(workspace: &Path) -> Self {
        Self::engine(
            workspace,
            "ab-slice",
            "night-relics",
            "Night Relics",
            Some("crates/ab-slice"),
        )
    }

    /// The tech demo, from the engine's sources.
    pub fn demo(workspace: &Path) -> Self {
        Self::engine(workspace, "ab-app", "ab-app", "April Blink Demo", None)
    }

    /// `project`: the folder of the game's `game.toml`, relative to the workspace (its
    /// assets are the folder it names); else the workspace's `assets`, without project files.
    fn engine(
        workspace: &Path,
        package: &str,
        binary: &str,
        title: &str,
        project: Option<&str>,
    ) -> Self {
        let project = project.and_then(|dir| Project::open(&workspace.join(dir)).ok());
        let version = workspace_version(workspace).unwrap_or_else(|| "0.1.0".to_owned());
        Self::Engine {
            workspace: workspace.to_path_buf(),
            package: package.to_owned(),
            binary: binary.to_owned(),
            assets: project
                .as_ref()
                .map_or_else(|| workspace.join("assets"), |p| p.assets.clone()),
            project_files: project
                .as_ref()
                .map(Project::project_files)
                .unwrap_or_default(),
            title: project.map_or_else(|| title.to_owned(), |p| p.title),
            version,
        }
    }

    /// File name stem of the exported executable and its pack.
    pub fn name(&self) -> String {
        match self {
            Self::Project(project) => project
                .package
                .clone()
                .unwrap_or_else(|| crate::folder_name(&project.title)),
            Self::Engine { binary, .. } => binary.clone(),
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Self::Project(project) => &project.title,
            Self::Engine { title, .. } => title,
        }
    }

    pub fn version(&self) -> String {
        match self {
            Self::Project(project) => project.version(),
            Self::Engine { version, .. } => version.clone(),
        }
    }

    fn assets(&self) -> &Path {
        match self {
            Self::Project(project) => &project.assets,
            Self::Engine { assets, .. } => assets,
        }
    }

    fn project_files(&self) -> Vec<(String, PathBuf)> {
        match self {
            Self::Project(project) => project.project_files(),
            Self::Engine { project_files, .. } => project_files.clone(),
        }
    }
}

/// The engine workspace's `[workspace.package] version`.
fn workspace_version(workspace: &Path) -> Option<String> {
    let text = std::fs::read_to_string(workspace.join("Cargo.toml")).ok()?;
    let table: toml::Table = text.parse().ok()?;
    table
        .get("workspace")?
        .get("package")?
        .get("version")?
        .as_str()
        .map(str::to_owned)
}

/// One build to make: a platform, and the explicit target triple if not the host's default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub platform: Platform,
    pub triple: Option<String>,
}

impl Build {
    /// A build for `platform`, cross-compiled when it is not this machine's.
    pub fn for_platform(platform: Platform) -> Self {
        Self {
            platform,
            triple: (platform != Platform::host()).then(|| platform.triple().to_owned()),
        }
    }
}

/// How to export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The pack appended to the executable instead of next to it.
    pub single_file: bool,
    /// README.txt and licenses/ next to the executable.
    pub extras: bool,
    pub verify: bool,
    pub archive: bool,
    /// Folder the exports go in.
    pub out: PathBuf,
    pub builds: Vec<Build>,
    /// `<out>/<name>` (false: one platform) or `<out>/<name>-<platform>`.
    pub suffix_platform: bool,
}

impl Options {
    /// This machine's platform, into `out`, verified.
    pub fn new(out: impl Into<PathBuf>) -> Self {
        Self {
            single_file: false,
            extras: false,
            verify: true,
            archive: false,
            out: out.into(),
            builds: vec![Build::for_platform(Platform::host())],
            suffix_platform: false,
        }
    }

    /// Exports for `platforms`, each in `<out>/<name>-<platform>`.
    pub fn with_platforms(mut self, platforms: &[Platform]) -> Self {
        self.builds = platforms.iter().map(|&p| Build::for_platform(p)).collect();
        self.suffix_platform = true;
        self
    }
}

/// Command-line usage of an export (`cargo xtask export`, `ab-editor --export`).
pub const USAGE: &str = "export <slice|demo|project folder> [--platform linux|windows|all] \
                         [--archive] [--single-file] [--extras] [--target <triple>] [--out <dir>] \
                         [--no-verify]";

/// Reads export arguments: the game (a name or a folder) and the options. Exports go in
/// `default_out` unless `--out` says otherwise.
pub fn parse_args(args: &[&str], default_out: &Path) -> Result<(String, Options)> {
    let mut game = None;
    let mut options = Options::new(default_out);
    let mut platforms = None;
    let mut triple: Option<String> = None;
    let mut args = args.iter();
    while let Some(&arg) = args.next() {
        match arg {
            "--single-file" => options.single_file = true,
            "--extras" => options.extras = true,
            "--no-verify" => options.verify = false,
            "--archive" | "--zip" => options.archive = true,
            "--out" => options.out = PathBuf::from(args.next().context("--out needs a directory")?),
            "--target" => {
                triple = Some(args.next().context("--target needs a triple")?.to_string());
            }
            "--platform" | "-p" => {
                platforms = Some(Platform::parse(
                    args.next()
                        .context("--platform needs linux, windows or all")?,
                )?);
            }
            other if other.starts_with("--") => bail!("unknown option `{other}`\nusage: {USAGE}"),
            other => game = Some(other.to_owned()),
        }
    }
    let Some(game) = game else {
        bail!("usage: {USAGE}");
    };
    match (platforms, triple) {
        (Some(_), Some(_)) => bail!("use either --platform or --target, not both"),
        (None, Some(triple)) => {
            options.builds = vec![Build {
                platform: Platform::of_triple(&triple),
                triple: Some(triple),
            }];
        }
        (Some(platforms), None) => options = options.with_platforms(&platforms),
        (None, None) => {}
    }
    Ok((game, options))
}

/// The game an export argument names: `slice` or `demo` (from the engine's sources), or a
/// project folder.
pub fn resolve_game(name: &str) -> Result<Game> {
    match name {
        "slice" | "night-relics" | "demo" | "ab-app" => {
            let engine = engine_root()
                .context("Night Relics and the demo export from the engine's sources only")?;
            Ok(if name.starts_with('d') || name == "ab-app" {
                Game::demo(&engine)
            } else {
                Game::slice(&engine)
            })
        }
        path => Ok(Game::Project(Project::open(Path::new(path)).with_context(
            || format!("`{path}` is neither slice, demo nor a project folder"),
        )?)),
    }
}

/// What an export made for one platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exported {
    pub platform: Platform,
    pub folder: PathBuf,
    pub archive: Option<PathBuf>,
}

/// Exports `game`; see the [module docs](self).
pub fn export(game: &Game, options: &Options, log: Log<'_>) -> Result<Vec<Exported>> {
    let assets = game.assets();
    if !assets.is_dir() {
        bail!("asset directory {} not found", assets.display());
    }
    if let Game::Project(project) = game {
        if project.package.is_none() {
            bail!(
                "{} has no Cargo.toml (no scripts to build)",
                project.root.display()
            );
        }
    }
    let version = game.version();
    // Check every toolchain first: a missing one should not fail after a long build.
    for build in &options.builds {
        check_toolchain(build)?;
    }
    let mut pack = PackWriter::new();
    pack.add_directory(assets)
        .with_context(|| format!("packing {}", assets.display()))?;
    // The project's own files go apart from the assets, where the engine looks for them.
    for (name, file) in game.project_files() {
        let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
        pack.add(&format!("{}/{name}", ab_vfs::PROJECT_DIR), bytes);
    }
    pack.add(
        "licenses/Prompt-font-OFL.txt",
        FONT_LICENSE.as_bytes().to_vec(),
    );

    let mut results = Vec::new();
    for build in &options.builds {
        say!(
            log,
            "=== {} {version} for {} ===",
            game.title(),
            build.platform.name()
        );
        results.push(export_one(game, &version, build, options, &pack, log)?);
    }
    Ok(results)
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

fn export_one(
    game: &Game,
    version: &str,
    build: &Build,
    options: &Options,
    pack: &PackWriter,
    log: Log<'_>,
) -> Result<Exported> {
    let platform = build.platform;
    let name = game.name();
    let built = compile(game, build, log)?;
    let exe_name = platform.executable(&name);

    let folder = if options.suffix_platform {
        format!("{name}-{}", platform.name())
    } else {
        name.clone()
    };
    let dir = options.out.join(&folder);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("clearing {}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir)?;
    let exe = dir.join(&exe_name);
    let (stats, pack_path) = if options.single_file {
        (
            append_to_executable(&built.executable, pack, &exe)?,
            exe.clone(),
        )
    } else {
        std::fs::copy(&built.executable, &exe)
            .with_context(|| format!("copying {}", built.executable.display()))?;
        let pack_path = dir.join(format!("{name}.{}", ab_vfs::PACK_EXTENSION));
        (pack.write_file(&pack_path)?, pack_path)
    };
    let library = match &built.library {
        Some(library) => {
            let shipped = dir.join(platform.shipped_library(&name));
            std::fs::copy(library, &shipped)
                .with_context(|| format!("copying {}", library.display()))?;
            Some(shipped)
        }
        None => None,
    };
    if options.extras {
        write_extras(&dir, game.title(), version, platform, &exe_name)?;
    }
    say!(
        log,
        "packed {} files ({} compressed): {:.1} MB of assets → {:.1} MB",
        stats.files,
        stats.compressed_files,
        stats.original_bytes as f64 / 1e6,
        stats.pack_bytes as f64 / 1e6,
    );

    if options.verify {
        if platform == Platform::Windows {
            check_windows_exe(&exe, log)?;
            if let Some(library) = &library {
                check_windows_exe(library, log)?;
            }
        }
        if build.triple.is_none() {
            verify_export(&exe, &pack_path, log)?;
        } else {
            // Cannot run another platform's program here; the pack can still be checked.
            Pack::open(&pack_path)?
                .verify()
                .context("the pack is damaged")?;
            say!(
                log,
                "verify: pack OK (run the game once on {} to finish checking)",
                platform.name()
            );
        }
    }
    say!(log, "exported {} to {}", game.title(), dir.display());
    let mut files: Vec<_> = std::fs::read_dir(&dir)?.flatten().collect();
    files.sort_by_key(|e| e.file_name());
    for entry in files {
        say!(
            log,
            "  {:<32} {:>8.1} MB",
            entry.file_name().to_string_lossy(),
            file_size(&entry.path()) as f64 / 1e6
        );
    }

    let archive = if options.archive {
        let base = format!("{name}-{version}-{}", platform.name());
        let path = if platform == Platform::Windows {
            let path = options.out.join(format!("{base}.zip"));
            archive::zip(&dir, &folder, &path)?;
            path
        } else {
            let path = options.out.join(format!("{base}.tar.gz"));
            archive::tar_gz(&dir, &folder, &path)?;
            path
        };
        say!(log, "archive: {}", path.display());
        Some(path)
    } else {
        None
    };
    Ok(Exported {
        platform,
        folder: dir,
        archive,
    })
}

fn cargo_command() -> Command {
    process::command(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()))
}

fn succeeds(command: &mut Command) -> bool {
    command.output().is_ok_and(|output| output.status.success())
}

/// How a build is compiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Toolchain {
    /// Plain `cargo build`.
    Cargo,
    /// `cargo xwin build`: the MSVC target from Linux, with Microsoft's SDK downloaded once.
    Xwin,
}

fn toolchain(build: &Build) -> Toolchain {
    match &build.triple {
        Some(triple) if triple.contains("msvc") && !cfg!(windows) => Toolchain::Xwin,
        _ => Toolchain::Cargo,
    }
}

/// Fails early, with what to install, when a build cannot work on this machine.
pub fn check_toolchain(build: &Build) -> Result<()> {
    let Some(triple) = &build.triple else {
        return Ok(());
    };
    if Platform::of_triple(triple) == Platform::Linux && cfg!(windows) {
        bail!("Linux builds cannot be made on Windows: export on Linux (WSL works) or in CI");
    }
    let installed = process::command("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|l| l.trim() == triple)
        })
        .unwrap_or(true); // no rustup: let cargo report it
    if !installed {
        bail!("the Rust target {triple} is not installed: run `rustup target add {triple}`");
    }
    if toolchain(build) == Toolchain::Xwin && !succeeds(cargo_command().args(["xwin", "--version"]))
    {
        bail!(
            "Windows builds from Linux need cargo-xwin (it downloads the Windows SDK once): \
             run `cargo install --locked cargo-xwin`, plus `clang` and `lld` from your \
             distribution (Ubuntu: `sudo apt install clang lld`)"
        );
    }
    Ok(())
}

/// A cargo build command for `build`: the toolchain, the target, a static C runtime on
/// Windows (the game starts on PCs without the Visual C++ redistributable).
fn build_command(build: &Build) -> Command {
    let mut command = cargo_command();
    let toolchain = toolchain(build);
    if toolchain == Toolchain::Xwin {
        command.arg("xwin");
    }
    command.arg("build");
    if let Some(triple) = &build.triple {
        command.args(["--target", triple]);
    }
    if build.platform == Platform::Windows {
        let triple = build
            .triple
            .as_deref()
            .unwrap_or(Platform::Windows.triple());
        let var = format!(
            "CARGO_TARGET_{}_RUSTFLAGS",
            triple.to_uppercase().replace('-', "_")
        );
        let mut flags = std::env::var(&var).unwrap_or_default();
        if !flags.contains("crt-static") {
            flags = format!("{flags} -C target-feature=+crt-static")
                .trim()
                .to_owned();
        }
        if toolchain == Toolchain::Xwin {
            // Microsoft's static runtime references .pdb files that are not shipped: harmless
            // LNK4099 warnings, one per object file.
            flags.push_str(" -C link-arg=/IGNORE:4099");
        }
        command.env(var, flags);
    }
    command
}

fn run(mut command: Command, dir: &Path, what: &str, log: Log<'_>) -> Result<()> {
    let shown = format!("{command:?}").replace('"', "");
    say!(log, "$ {shown}");
    let output = command
        .current_dir(dir)
        .output()
        .context("failed to run cargo: is Rust installed (https://rustup.rs)?")?;
    for line in String::from_utf8_lossy(&output.stderr).lines() {
        log(line);
    }
    if !output.status.success() {
        bail!("building {what} failed ({})", output.status);
    }
    Ok(())
}

/// The built files of one platform.
struct Built {
    executable: PathBuf,
    /// A project's script library.
    library: Option<PathBuf>,
}

fn compile(game: &Game, build: &Build, log: Log<'_>) -> Result<Built> {
    match game {
        Game::Engine {
            workspace,
            package,
            binary,
            ..
        } => {
            // Players get no debug views or developer options.
            let executable = engine_binary(
                workspace,
                build,
                package,
                binary,
                &["ab-engine/shipping"],
                log,
            )?;
            Ok(Built {
                executable,
                library: None,
            })
        }
        Game::Project(project) => {
            let mut command = build_command(build);
            command.args(["--lib", "--release"]);
            run(command, &project.root, "the scripts", log)?;
            let library = project
                .library(Profile::Release, build.triple.as_deref())
                .context("the project has no Cargo.toml")?;
            if !library.is_file() {
                bail!(
                    "cargo built no {} (is `crate-type = [\"cdylib\"]` in Cargo.toml?)",
                    library.display()
                );
            }
            Ok(Built {
                executable: player(build, log)?,
                library: Some(library),
            })
        }
    }
}

/// Where a workspace's `dist` build of `file` lands.
fn engine_output(workspace: &Path, build: &Build, file: &str) -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join("target"));
    match &build.triple {
        Some(triple) => target.join(triple).join("dist").join(file),
        None => target.join("dist").join(file),
    }
}

/// The player for `build`'s platform: `$AB_PLAYERS/<platform>/`, next to this program in
/// `players/<platform>/`, or built from the engine's sources.
pub fn player(build: &Build, log: Log<'_>) -> Result<PathBuf> {
    let file = build.platform.executable("ab-player");
    let mut places: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("AB_PLAYERS") {
        places.push(PathBuf::from(dir).join(build.platform.name()).join(&file));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(PathBuf::from))
    {
        places.push(dir.join("players").join(build.platform.name()).join(&file));
    }
    if let Some(found) = places.iter().find(|p| p.is_file()) {
        say!(log, "player: {}", found.display());
        return Ok(found.clone());
    }
    let Some(engine) = engine_root() else {
        bail!(
            "no player for {} (looked in {}); reinstall the editor, or set AB_PLAYERS",
            build.platform.name(),
            places
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    };
    engine_binary(&engine, build, "ab-player", "ab-player", &["shipping"], log)
}

/// Builds the executable `binary` of `package` in the engine's `workspace` for `build`
/// (`dist` profile, with `features`); returns where it is.
pub fn engine_binary(
    workspace: &Path,
    build: &Build,
    package: &str,
    binary: &str,
    features: &[&str],
    log: Log<'_>,
) -> Result<PathBuf> {
    let mut command = build_command(build);
    command.args(["--profile", "dist", "-p", package, "--bin", binary]);
    if !features.is_empty() {
        command.args(["--features", &features.join(",")]);
    }
    run(command, workspace, package, log)?;
    Ok(engine_output(
        workspace,
        build,
        &build.platform.executable(binary),
    ))
}

fn write_extras(
    dir: &Path,
    title: &str,
    version: &str,
    platform: Platform,
    exe_name: &str,
) -> Result<()> {
    let licenses = dir.join("licenses");
    std::fs::create_dir_all(&licenses)?;
    std::fs::write(licenses.join("Prompt-font-OFL.txt"), FONT_LICENSE)?;
    let (run, data) = match platform {
        Platform::Windows => (
            format!("Double-click `{exe_name}`."),
            "%APPDATA%\\<game>  (paste it into the Explorer address bar)",
        ),
        Platform::Linux => (
            format!("Run `./{exe_name}` (or double-click it in your file manager)."),
            "~/.local/share/<game>",
        ),
    };
    let text = format!(
        "{title} {version}\n\n{run} Keep the other files next to it.\n\n\
         Keys: F11 fullscreen.\n\
         Options: --fullscreen, --windowed, --licenses.\n\n\
         Save games, the log (logs/latest.log) and crash reports are in your user data folder:\n\
         {data}\n\n\
         Made with April Blink Engine. The Prompt font is under the SIL Open Font License\n\
         (licenses/Prompt-font-OFL.txt).\n",
    );
    let text = if platform == Platform::Windows {
        text.replace('\n', "\r\n")
    } else {
        text
    };
    std::fs::write(dir.join("README.txt"), text)?;
    Ok(())
}

/// What [`check_windows_exe`] reads from a PE file.
#[derive(Debug, PartialEq, Eq)]
struct PeInfo {
    /// 2 = GUI (no console window), 3 = console.
    subsystem: u16,
    /// Lower-case names of the DLLs it imports.
    imports: Vec<String>,
}

fn pe_info(bytes: &[u8]) -> Option<PeInfo> {
    let u16_at = |at: usize| Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    if bytes.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32_at(0x3c)? as usize;
    if bytes.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let sections = u16_at(pe + 6)? as usize;
    let optional_size = u16_at(pe + 20)? as usize;
    let optional = pe + 24;
    if u16_at(optional)? != 0x20b {
        return None; // not PE32+
    }
    let subsystem = u16_at(optional + 68)?;
    // Data directory 1: imports (RVA, size), after 112 bytes of PE32+ optional header.
    let import_rva = u32_at(optional + 112 + 8)? as usize;
    let table = optional + optional_size;
    let to_offset = |rva: usize| -> Option<usize> {
        (0..sections).find_map(|i| {
            let s = table + i * 40;
            let size = u32_at(s + 8)?.max(u32_at(s + 16)?) as usize;
            let address = u32_at(s + 12)? as usize;
            let raw = u32_at(s + 20)? as usize;
            (rva >= address && rva < address + size).then(|| rva - address + raw)
        })
    };
    let mut imports = Vec::new();
    if import_rva != 0 {
        let mut at = to_offset(import_rva)?;
        // Import descriptors (20 bytes) until an all-zero one; the name RVA is at +12.
        for _ in 0..4096 {
            let name_rva = u32_at(at + 12)? as usize;
            if name_rva == 0 {
                break;
            }
            let start = to_offset(name_rva)?;
            let end = start + bytes.get(start..)?.iter().position(|&b| b == 0)?;
            imports.push(String::from_utf8_lossy(&bytes[start..end]).to_lowercase());
            at += 20;
        }
    }
    Some(PeInfo { subsystem, imports })
}

/// DLLs that are not part of every Windows install: a game importing them fails to start
/// with "VCRUNTIME140.dll was not found" on a fresh PC.
const REDISTRIBUTABLE_DLLS: &[&str] = &["vcruntime", "msvcp", "ucrtbase", "api-ms-win-crt"];

fn check_windows_exe(exe: &Path, log: Log<'_>) -> Result<()> {
    let bytes = std::fs::read(exe)?;
    let info = pe_info(&bytes)
        .with_context(|| format!("{} is not a 64-bit Windows binary", exe.display()))?;
    let missing: Vec<&String> = info
        .imports
        .iter()
        .filter(|dll| REDISTRIBUTABLE_DLLS.iter().any(|r| dll.starts_with(r)))
        .collect();
    if !missing.is_empty() {
        bail!(
            "{} needs the Visual C++ runtime ({missing:?}); it would not start on PCs without \
             it. Build with `-C target-feature=+crt-static`.",
            exe.display()
        );
    }
    let library = exe
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("dll"));
    if info.subsystem != 2 && !library {
        say!(
            log,
            "verify: note: {} opens a console window; add \
             `#![cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")]` to main.rs",
            exe.display()
        );
    }
    say!(
        log,
        "verify: {} OK ({}, needs only system DLLs: {})",
        exe.file_name().unwrap_or_default().to_string_lossy(),
        if library {
            "library"
        } else if info.subsystem == 2 {
            "no console"
        } else {
            "console"
        },
        info.imports.join(", ")
    );
    Ok(())
}

/// Checks the pack and runs the exported game headless from an empty directory.
fn verify_export(exe: &Path, pack_path: &Path, log: Log<'_>) -> Result<()> {
    let pack = Pack::open(pack_path).with_context(|| format!("opening {}", pack_path.display()))?;
    pack.verify().context("the pack is damaged")?;
    say!(log, "verify: pack OK ({} files)", pack.len());

    let sandbox = std::env::temp_dir().join(format!("ab-export-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sandbox);
    std::fs::create_dir_all(&sandbox)?;
    let output = process::command(exe)
        // `--overlay` proves a shipping build: it must be ignored, with a warning.
        .args(["--overlay", "--headless", "180"])
        .current_dir(&sandbox)
        .env_remove("AB_ASSETS")
        // Keep the check's log and saves out of the developer's own data folder.
        .env("AB_SAVE_DIR", sandbox.join("data"))
        .env("RUST_LOG", "info")
        .output()
        .with_context(|| format!("running {}", exe.display()))?;
    let output_log = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let log_file = std::fs::read_dir(sandbox.join("data"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|game| game.path().join("logs/latest.log").is_file());
    let _ = std::fs::remove_dir_all(&sandbox);
    let from_pack = output_log
        .lines()
        .any(|l| l.contains("assets mounted") && l.contains("pack:"));
    let errors: Vec<&str> = output_log.lines().filter(|l| l.contains("ERROR")).collect();
    let shipping = output_log.contains("is a developer option");
    if !shipping {
        log(&output_log);
        bail!("the exported game still has the developer options (not a shipping build)");
    }
    if !output.status.success() || !from_pack || !errors.is_empty() {
        log(&output_log);
        bail!(
            "the exported game did not run cleanly from its pack (exit: {}, assets from pack: {from_pack}, errors: {})",
            output.status,
            errors.len()
        );
    }
    let finished = output_log
        .lines()
        .find(|l| l.contains("headless run finished"))
        .map(|l| l.trim().to_owned())
        .unwrap_or_default();
    say!(
        log,
        "verify: the exported game runs from its pack outside the source tree"
    );
    say!(
        log,
        "verify: shipping build (debug views and developer options off)"
    );
    if !log_file {
        say!(
            log,
            "verify: note: the game wrote no log file (does main use ab_engine::run_game?)"
        );
    }
    if !finished.is_empty() {
        say!(log, "        {finished}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_and_games() {
        let options =
            Options::new("/tmp/out").with_platforms(&[Platform::Linux, Platform::Windows]);
        assert!(options.suffix_platform);
        let host = options
            .builds
            .iter()
            .find(|b| b.platform == Platform::host())
            .unwrap();
        assert!(host.triple.is_none(), "the host builds natively");
        assert_eq!(options.builds.len(), 2);
        assert!(Options::new("x").builds[0].triple.is_none());
        let engine = engine_root().unwrap();
        let slice = Game::slice(&engine);
        assert_eq!(
            (slice.name().as_str(), slice.title()),
            ("night-relics", "Night Relics")
        );
        assert_eq!(slice.version().split('.').count(), 3);
    }

    #[test]
    fn command_line() {
        let out = Path::new("/out");
        let (game, options) =
            parse_args(&["my-game", "--platform", "all", "--archive"], out).unwrap();
        assert_eq!(game, "my-game");
        assert!(options.archive && options.suffix_platform);
        assert_eq!(options.builds.len(), 2);
        let (_, options) = parse_args(&["slice", "--out", "x"], out).unwrap();
        assert!(!options.suffix_platform && options.builds[0].triple.is_none());
        assert_eq!(options.out, Path::new("x"));
        assert!(parse_args(&["slice", "--platform", "linux", "--target", "x"], out).is_err());
        assert!(parse_args(&[], out).is_err());
        assert!(parse_args(&["a", "--nope"], out).is_err());
        assert_eq!(resolve_game("slice").unwrap().name(), "night-relics");
        assert_eq!(resolve_game("demo").unwrap().name(), "ab-app");
        assert!(resolve_game("/no/such/project").is_err());
    }

    /// A minimal PE32+ image: one section holding an import table for two DLLs.
    fn fake_pe(subsystem: u16, dlls: &[&str]) -> Vec<u8> {
        let mut b = vec![0u8; 0x400];
        b[..2].copy_from_slice(b"MZ");
        b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        let put16 =
            |b: &mut Vec<u8>, at: usize, v: u16| b[at..at + 2].copy_from_slice(&v.to_le_bytes());
        let put32 =
            |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put16(&mut b, 0x86, 1); // one section
        put16(&mut b, 0x94, 240); // optional header size
        let optional = 0x98;
        put16(&mut b, optional, 0x20b);
        put16(&mut b, optional + 68, subsystem);
        put32(&mut b, optional + 120, 0x1000); // import table RVA
        let section = optional + 240;
        put32(&mut b, section + 8, 0x200); // virtual size
        put32(&mut b, section + 12, 0x1000); // virtual address
        put32(&mut b, section + 16, 0x200); // raw size
        put32(&mut b, section + 20, 0x200); // raw offset
        let names = 0x200 + 20 * (dlls.len() + 1);
        let mut name_at = names;
        for (i, dll) in dlls.iter().enumerate() {
            put32(
                &mut b,
                0x200 + i * 20 + 12,
                (0x1000 + name_at - 0x200) as u32,
            );
            b[name_at..name_at + dll.len()].copy_from_slice(dll.as_bytes());
            name_at += dll.len() + 1;
        }
        b
    }

    #[test]
    fn reads_subsystem_and_imports_of_windows_programs() {
        let info = pe_info(&fake_pe(2, &["KERNEL32.dll", "VCRUNTIME140.dll"])).unwrap();
        assert_eq!(info.subsystem, 2);
        assert_eq!(info.imports, ["kernel32.dll", "vcruntime140.dll"]);
        assert!(pe_info(b"\x7fELF").is_none());
        assert!(pe_info(&fake_pe(3, &[])).unwrap().imports.is_empty());
    }
}
