//! Exporting games: shippable copies for players, in a folder per platform.
//!
//! For each platform (this machine's by default; also Windows from Linux with `cargo xwin`,
//! and macOS on a Mac):
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
//!    On macOS the game is an app bundle (`<title>.app`, see [`app_bundle`]) that players
//!    double-click in Finder; the executable, its library and its assets go in
//!    `Contents/MacOS`, and the bundle is signed ad hoc once complete;
//! 2. packs the asset directory into chunks in `assets/` next to the executable (one pack per
//!    top-level folder, named by hash, plus `assets/index.abm`; see `ab_vfs::ChunkedSource`),
//!    or, with `single_file`, appends one pack to the executable;
//! 3. with `extras`, adds a README and the licenses as files (they always ship inside the
//!    pack, and the game shows them with `--licenses`);
//! 4. verifies: the pack is read back in full, the exported game is run headless from an empty
//!    directory (so an export that still depends on the source tree fails here instead of on a
//!    player's PC) and Windows binaries are checked for DLLs players may not have;
//! 5. with `archive`, zips it (Windows) or makes a `.tar.gz` (Linux, macOS), ready to upload.
//!
//! Progress goes to a log callback, one line at a time: `=== <title> <version> for
//! <platform> ===` starts a platform, `exported <title> to <folder>` and `archive: <path>`
//! report results.

use crate::{archive, engine_root, process, Platform, Profile, Project};
use ab_vfs::{append_to_executable, is_chunked, ChunkedSource, Pack, PackWriter, CHUNK_DIR};
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

/// File in every exported folder: only a folder holding it is replaced by the next export.
const EXPORT_MARKER: &str = ".ablink-export";

/// Removes a previous export at `dir` so the new one starts clean. Refuses a folder that is
/// not an earlier export (neither the marker file nor the executable is there) or that holds the game's sources: a
/// mistyped output folder must never delete a project.
fn clear_export_dir(dir: &Path, game: &Game, exe_name: &str) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let target = canonical(dir);
    let source = canonical(game.source_dir());
    if source.starts_with(&target) {
        bail!(
            "{} holds the game's sources: choose another output folder",
            dir.display()
        );
    }
    let empty = std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none());
    // Exports made before the marker are known by their executable.
    let earlier = dir.join(EXPORT_MARKER).is_file() || dir.join(exe_name).is_file();
    if !empty && !earlier {
        bail!(
            "{} already exists and is not an earlier export: move it away or choose another \
             output folder",
            dir.display()
        );
    }
    std::fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))
}

impl Game {
    /// The folder of the game's sources (the project, or the engine workspace).
    fn source_dir(&self) -> &Path {
        match self {
            Self::Project(project) => &project.root,
            Self::Engine { workspace, .. } => workspace,
        }
    }

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

    /// File name stem of the exported executable and its pack: `[export] file_name`, else
    /// the package's name.
    pub fn name(&self) -> String {
        match self {
            Self::Project(project) if !project.export.file_name.is_empty() => {
                project.export.file_name.clone()
            }
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
    /// One pack appended to the executable instead of chunks in `assets/`.
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
pub const USAGE: &str = "export <slice|demo|project folder> [--platform linux|windows|macos|all] \
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
                        .context("--platform needs linux, windows, macos or all")?,
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
        // A clone or a pull may have packs not unpacked yet: export what git holds.
        if project.pack_dir().is_dir() {
            let report = crate::asset_store::sync(project).context("unpacking the assets")?;
            log(&report.summary());
        }
        if project.package.is_none() {
            bail!(
                "{} has no Cargo.toml (no scripts to build)",
                project.root.display()
            );
        }
        // What the game says about itself: errors stop the export before any build.
        let issues = project.export.check(&project.root, &project.title);
        for issue in issues.iter().filter(|i| !i.error) {
            say!(log, "warning: {}: {}", issue.field, issue.message);
        }
        let errors: Vec<String> = issues
            .iter()
            .filter(|i| i.error)
            .map(|i| format!("{}: {}", i.field, i.message))
            .collect();
        if !errors.is_empty() {
            bail!("the export settings need fixing: {}", errors.join("; "));
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

/// Writes the game's icon, version information and console choice into the Windows
/// executable at `exe` (see [`crate::export_settings`]).
fn windows_metadata(
    game: &Game,
    version: &str,
    exe_name: &str,
    exe: &Path,
    log: Log<'_>,
) -> Result<()> {
    let settings = match game {
        Game::Project(project) => project.export.clone(),
        Game::Engine { .. } => crate::export_settings::ExportSettings::default(),
    };
    let icon = match game {
        Game::Project(project) => settings
            .icon_path(&project.root)
            .map(|p| std::fs::read(&p).with_context(|| format!("reading the icon {}", p.display())))
            .transpose()?,
        Game::Engine { .. } => None,
    };
    let meta = crate::export_settings::WindowsMetadata {
        title: game.title().to_owned(),
        version: version.to_owned(),
        creator: settings.creator.clone(),
        description: settings.description.clone(),
        copyright: settings.copyright.clone(),
        file_name: exe_name.to_owned(),
        icon,
        console: settings.console,
    };
    crate::export_settings::apply_windows(exe, &meta)
        .with_context(|| format!("writing the game's details into {exe_name}"))?;
    say!(
        log,
        "windows: {} {version}{}{}{}",
        meta.title,
        if meta.creator.is_empty() {
            String::new()
        } else {
            format!(" by {}", meta.creator)
        },
        if meta.icon.is_some() {
            ", with its icon"
        } else {
            ", no icon"
        },
        if meta.console { ", with a console" } else { "" },
    );
    Ok(())
}

/// Bytes of a file, or of everything in a folder.
fn file_size(path: &Path) -> u64 {
    if path.is_dir() {
        return std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| file_size(&entry.path()))
            .sum();
    }
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
    clear_export_dir(&dir, game, &exe_name)?;
    std::fs::create_dir_all(&dir)?;
    // Marks the folder as an export: the next export may replace it, nothing else.
    std::fs::write(dir.join(EXPORT_MARKER), b"")?;
    // Where the executable and the files it needs next to it go.
    let (bundle, bin_dir) = if platform == Platform::MacOs {
        let (app, bin) = app_bundle(&dir, game.title(), &name, version)?;
        (Some(app), bin)
    } else {
        (None, dir.clone())
    };
    let exe = bin_dir.join(&exe_name);
    // A pack appended to a Mach-O executable fails code signing, and the bundle is a single
    // item for players anyway: macOS keeps the chunks inside it.
    let single_file = options.single_file && bundle.is_none();
    if options.single_file && !single_file {
        say!(log, "single file: the app bundle holds the assets on macOS");
    }
    // The player, with the game's icon and details on Windows (before a pack goes on its end).
    let player = bin_dir.join(format!("{exe_name}.player"));
    std::fs::copy(&built.executable, &player)
        .with_context(|| format!("copying {}", built.executable.display()))?;
    if platform == Platform::Windows {
        windows_metadata(game, version, &exe_name, &player, log)?;
    }
    let (stats, packs, assets) = if single_file {
        let made = append_to_executable(&player, pack, &exe);
        let _ = std::fs::remove_file(&player);
        (made?, 1, exe.clone())
    } else {
        std::fs::rename(&player, &exe)
            .with_context(|| format!("moving {} to {}", player.display(), exe.display()))?;
        let chunks = bin_dir.join(CHUNK_DIR);
        let written = pack
            .write_chunks(&chunks)
            .with_context(|| format!("writing {}", chunks.display()))?;
        (written.totals, written.chunks, chunks)
    };
    let library = match &built.library {
        Some(library) => {
            let shipped = bin_dir.join(platform.shipped_library(&name));
            std::fs::copy(library, &shipped)
                .with_context(|| format!("copying {}", library.display()))?;
            Some(shipped)
        }
        None => None,
    };
    if let Some(app) = &bundle {
        sign_ad_hoc(app, log);
    }
    if options.extras {
        let launch = bundle
            .as_deref()
            .and_then(Path::file_name)
            .map_or_else(|| exe_name.clone(), |n| n.to_string_lossy().into_owned());
        write_extras(&dir, game.title(), version, platform, &launch)?;
    }
    say!(
        log,
        "packed {} files ({} compressed) in {packs} {}: {:.1} MB of assets → {:.1} MB",
        stats.files,
        stats.compressed_files,
        if packs == 1 { "pack" } else { "chunks" },
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
            verify_export(&exe, &assets, log)?;
        } else {
            // Cannot run another platform's program here; the pack can still be checked.
            verify_assets(&assets)?;
            say!(
                log,
                "verify: pack OK (run the game once on {} to finish checking)",
                platform.name()
            );
        }
    }
    say!(log, "exported {} to {}", game.title(), dir.display());
    // For the editor: the folder alone (the line above can hold " to " in the title too).
    say!(log, "folder: {}", dir.display());
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
    // Line by line as cargo writes them: the editor shows what is compiling now.
    let status = process::run_streaming(command.current_dir(dir), |line| log(line))
        .context("failed to run cargo: is Rust installed (https://rustup.rs)?")?;
    if !status.success() {
        bail!("building {what} failed ({status})");
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

/// Makes the app bundle `<title>.app` in `dir`: its `Info.plist` and an empty
/// `Contents/MacOS`, where the executable `exe_name` and everything it reads next to itself
/// go. Returns the bundle and that folder.
fn app_bundle(
    dir: &Path,
    title: &str,
    exe_name: &str,
    version: &str,
) -> Result<(PathBuf, PathBuf)> {
    // Finder shows the title; `/` and `:` cannot be in a file name there.
    let file_name: String = title
        .chars()
        .map(|c| {
            if matches!(c, '/' | ':' | '\\') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let file_name = match file_name.trim() {
        "" => exe_name,
        trimmed => trimmed,
    };
    let app = dir.join(format!("{file_name}.app"));
    let bin = app.join("Contents/MacOS");
    std::fs::create_dir_all(&bin).with_context(|| format!("creating {}", bin.display()))?;
    std::fs::write(
        app.join("Contents/Info.plist"),
        info_plist(title, exe_name, version),
    )?;
    Ok((app, bin))
}

/// The `Info.plist` of a game's app bundle. Games run in full resolution on Retina screens,
/// and macOS turns on Game Mode for them (`public.app-category.games`).
fn info_plist(title: &str, exe_name: &str, version: &str) -> String {
    let xml = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    // Reverse-DNS identifier: letters, digits, `-` and `.` only.
    let id: String = exe_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // CFBundleVersion takes up to three numbers: `0.1.0-beta` → `0.1.0`.
    let build: String = version
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let build = if build.is_empty() {
        "0"
    } else {
        build.trim_end_matches('.')
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>{title}</string>
	<key>CFBundleExecutable</key>
	<string>{exe}</string>
	<key>CFBundleIdentifier</key>
	<string>com.aprilblink.game.{id}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>{title}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{build}</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.games</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSSupportsAutomaticGraphicsSwitching</key>
	<false/>
</dict>
</plist>
"#,
        title = xml(title),
        exe = xml(exe_name),
        version = xml(version),
    )
}

/// Signs the bundle `app` ad hoc (`codesign --sign -`): Apple silicon Macs run no unsigned
/// code, and the files added after linking would otherwise break the linker's signature.
/// Only warns when it fails: the game still runs on the Mac that made it.
fn sign_ad_hoc(app: &Path, log: Log<'_>) {
    let mut command = process::command("codesign");
    command.args(["--force", "--deep", "--sign", "-"]).arg(app);
    match command.output() {
        Ok(output) if output.status.success() => say!(log, "signed {} (ad hoc)", app.display()),
        Ok(output) => say!(
            log,
            "warning: codesign failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(err) => say!(log, "warning: codesign not run: {err}"),
    }
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
        Platform::MacOs => (
            format!(
                "Double-click `{exe_name}` (the first time, right-click it and choose Open: \
                 the game is not notarized by Apple)."
            ),
            "~/Library/Application Support/<game>",
        ),
    };
    let keep = if platform == Platform::MacOs {
        ""
    } else {
        " Keep the other files next to it."
    };
    let text = format!(
        "{title} {version}\n\n{run}{keep}\n\n\
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

/// Reads a pack or a chunk folder in full; returns how many files it holds.
fn verify_assets(path: &Path) -> Result<usize> {
    let opening = || format!("opening {}", path.display());
    if is_chunked(path) {
        let chunks = ChunkedSource::open(path).with_context(opening)?;
        chunks.verify().context("the asset chunks are damaged")?;
        Ok(chunks.manifest().file_count())
    } else {
        let pack = Pack::open(path).with_context(opening)?;
        pack.verify().context("the pack is damaged")?;
        Ok(pack.len())
    }
}

/// Checks the pack or chunks and runs the exported game headless from an empty directory.
/// How long the exported game may take to start, run its frames and quit.
const VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

fn verify_export(exe: &Path, assets: &Path, log: Log<'_>) -> Result<()> {
    say!(log, "verify: pack OK ({} files)", verify_assets(assets)?);

    let sandbox = std::env::temp_dir().join(format!("ab-export-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sandbox);
    std::fs::create_dir_all(&sandbox)?;
    // A relative path (`--out dist`) would be looked up from the sandbox on Unix.
    let exe = std::path::absolute(exe)?;
    let mut check = process::command(&exe);
    check
        // `--overlay` proves a shipping build: it must be ignored, with a warning.
        .args(["--overlay", "--headless", "180"])
        .current_dir(&sandbox)
        .env_remove("AB_ASSETS")
        // Keep the check's log and saves out of the developer's own data folder.
        .env("AB_SAVE_DIR", sandbox.join("data"))
        // A failure is read from the output: no message box waiting for a click.
        .env("AB_NO_DIALOGS", "1")
        .env("RUST_LOG", "info");
    // A game whose scripts loop forever must not hang the export.
    let output = process::output_with_timeout(&mut check, VERIFY_TIMEOUT)
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
        .any(|l| l.contains("assets mounted") && (l.contains("pack:") || l.contains("chunks:")));
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
        let platforms = Platform::parse("all,host").unwrap();
        let options = Options::new("/tmp/out").with_platforms(&platforms);
        assert!(options.suffix_platform);
        let host = options
            .builds
            .iter()
            .find(|b| b.platform == Platform::host())
            .unwrap();
        assert!(host.triple.is_none(), "the host builds natively");
        assert_eq!(options.builds.len(), platforms.len());
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

    #[test]
    fn an_export_replaces_only_an_earlier_export() {
        let root = std::env::temp_dir().join(format!("ab-export-clear-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let project_dir = root.join("game");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(project_dir.join("game.toml"), "").unwrap();
        let game = Game::Engine {
            workspace: project_dir.clone(),
            package: "game".into(),
            binary: "game".into(),
            assets: project_dir.join("assets"),
            project_files: Vec::new(),
            title: "Game".into(),
            version: "0.1.0".into(),
        };
        // The sources themselves, or a folder holding them: never.
        assert!(clear_export_dir(&project_dir, &game, "game").is_err());
        assert!(clear_export_dir(&root, &game, "game").is_err());
        assert!(project_dir.join("game.toml").is_file());
        // Someone else's folder: refused.
        let other = root.join("out/game");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("notes.txt"), "mine").unwrap();
        assert!(clear_export_dir(&other, &game, "game").is_err());
        // An earlier export: replaced.
        std::fs::write(other.join(EXPORT_MARKER), "").unwrap();
        clear_export_dir(&other, &game, "game").unwrap();
        assert!(!other.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn macos_games_are_app_bundles() {
        let root = std::env::temp_dir().join(format!("ab-export-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (app, bin) = app_bundle(&root, "Cats & Dogs: 2/3", "cats-dogs", "1.2.0-beta").unwrap();
        assert_eq!(app, root.join("Cats & Dogs- 2-3.app"));
        assert_eq!(bin, app.join("Contents/MacOS"));
        assert!(bin.is_dir());
        let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains("<string>Cats &amp; Dogs: 2/3</string>"));
        assert!(plist.contains("<string>cats-dogs</string>"));
        assert!(plist.contains("<string>com.aprilblink.game.cats-dogs</string>"));
        assert!(plist.contains("<string>1.2.0-beta</string>"));
        assert!(plist.contains("<key>CFBundleVersion</key>\n\t<string>1.2.0</string>"));
        assert!(plist.contains("public.app-category.games"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
