//! Project automation: `cargo xtask <command>` (alias defined in `.cargo/config.toml`).

mod new_game;
mod package_editor;

use anyhow::{bail, Context as _, Result};
use std::path::PathBuf;
use std::process::Command;

const USAGE: &str = "\
April Blink Engine xtask

usage: cargo xtask <command> [args...]

commands:
  build [cargo args]   build the workspace
  run [app args]       run the bootstrap app (ab-app)
  slice [args]         play Night Relics, the vertical slice (crates/ab-slice)
  hot [demo|slice] [args]
                       run the demo (default) or the slice with its scripts hot reloaded:
                       saving a script rebuilds the library and the game picks it up
  editor [project folder] [args]
                       open the editor on a project (default: Night Relics); saving a
                       script rebuilds the library and the editor reloads it
  new-game \"<title>\" [--in <folder>]
                       create a project in <folder>/<name> (default: the current folder),
                       the same as the editor's New Project
  export <slice|demo|project folder> [--platform linux|windows|all] [--archive]
         [--single-file] [--extras] [--target <triple>] [--out <dir>] [--no-verify]
                       a shippable build in dist/<name>: the game's executable (a project
                       ships the player + its script library) and assets/ (chunks named
                       by hash + index.abm: a patch ships the changed ones), or one file
                       (--single-file; --extras adds README and licenses as files); then
                       runs it from an empty folder to prove it is self-contained.
                       --platform builds for Windows and/or Linux (dist/<name>-<platform>;
                       Windows from Linux uses cargo-xwin), --archive also makes a .zip
                       (Windows) / .tar.gz (Linux) to upload
  package-editor [--platform linux|windows|all] [--archive] [--out <dir>]
                       the editor to give to game makers, in dist/april-blink-editor-<platform>:
                       the editor, the players it exports games with (players/<platform>/)
                       and the script SDK new projects build against (sdk/)
  test                 run all workspace tests
  fmt                  format all code
  clippy               lint with warnings denied
  smoke                headless run of ab-app (no window/GPU needed)
  release              milestone check: the headless runs again in --release, then an
                       export of Night Relics (shipping profile) run from its pack
  ci                   everything CI runs: fmt --check, clippy, test, smoke, a build
                       without optional features, an export of Night Relics, and a new
                       project created, built and exported with the player";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        println!("{USAGE}");
        return Ok(());
    };
    let rest: Vec<String> = args.collect();
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();

    match command.as_str() {
        "build" => cargo(&[&["build", "--workspace"], &rest[..]].concat()),
        "run" => cargo(&[&["run", "-p", "ab-app", "--"], &rest[..]].concat()),
        "slice" => cargo(&[&["run", "-p", "ab-slice", "--"], &rest[..]].concat()),
        "hot" => match rest.first() {
            Some(&"slice") => hot(&HOT_SLICE, &rest[1..]),
            Some(&"demo") => hot(&HOT_DEMO, &rest[1..]),
            _ => hot(&HOT_DEMO, &rest),
        },
        "editor" => editor(&rest),
        "export" => export(&rest),
        "package-editor" => package_editor::package(&rest),
        "new-game" => new_game::new_game(&rest),
        "test" => test(),
        "fmt" => cargo(&["fmt", "--all"]),
        "clippy" => clippy(),
        "smoke" => smoke(&[]),
        "release" => {
            // Optimized builds hide different bugs (no overflow checks, LTO): run the
            // games as players get them.
            smoke(&["--release"])?;
            let out = workspace_root().join("target").join("release-dist");
            export(&["slice", "--out", &out.to_string_lossy()])
        }
        "ci" => {
            cargo(&["fmt", "--all", "--check"])?;
            clippy()?;
            test()?;
            // Optional features must stay optional.
            cargo(&["check", "-p", "ab-engine", "--no-default-features"])?;
            smoke(&[])?;
            // A shipped build must run from its pack alone.
            let out = workspace_root().join("target").join("ci-dist");
            export(&["slice", "--out", &out.to_string_lossy()])?;
            // A project made outside the engine builds against the SDK and ships with the
            // player.
            cargo(&[
                "test",
                "-p",
                "ab-project",
                "--test",
                "project_flow",
                "--",
                "--ignored",
            ])
        }
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

fn test() -> Result<()> {
    cargo(&["test", "--workspace", "--all-targets"])?;
    cargo(&["test", "--workspace", "--doc"])
}

fn clippy() -> Result<()> {
    cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ])
}

/// Runs each game headless for a number of frames; `profile` is extra cargo flags.
fn smoke(profile: &[&str]) -> Result<()> {
    for (package, frames) in [("ab-app", "600"), ("ab-slice", "600")] {
        cargo(
            &[
                &["run", "-p", package],
                profile,
                &["--", "--headless", frames],
            ]
            .concat(),
        )?;
    }
    Ok(())
}

fn cargo(args: &[&str]) -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    println!("$ cargo {}", args.join(" "));
    let status = Command::new(&cargo)
        .args(args)
        .current_dir(workspace_root())
        .status()
        .with_context(|| format!("failed to run `{cargo}`"))?;
    if !status.success() {
        bail!("`cargo {}` failed ({status})", args.join(" "));
    }
    Ok(())
}

/// A game that can run with hot-reloaded scripts.
struct HotGame {
    /// Package of the executable.
    package: &'static str,
    executable: &'static str,
    /// Package (and crate directory) of the script library.
    scripts: &'static str,
    /// File stem of the library (`lib<stem>.so`, `<stem>.dll`, ...).
    library: &'static str,
}

const HOT_DEMO: HotGame = HotGame {
    package: "ab-app",
    executable: "ab-app",
    scripts: "ab-demo-scripts",
    library: "ab_demo_scripts",
};

const HOT_SLICE: HotGame = HotGame {
    package: "ab-slice",
    executable: "night-relics",
    scripts: "ab-slice",
    library: "ab_slice",
};

/// `target/debug`.
fn debug_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"))
        .join("debug")
}

fn library_path(stem: &str) -> PathBuf {
    debug_dir().join(format!(
        "{}{stem}{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ))
}

/// Runs a game with hot reload and rebuilds its script library whenever its sources change.
fn hot(game: &HotGame, app_args: &[&str]) -> Result<()> {
    cargo(&["build", "-p", game.package, "-p", game.scripts])?;
    let exe = debug_dir().join(format!(
        "{}{}",
        game.executable,
        std::env::consts::EXE_SUFFIX
    ));
    let child = Command::new(&exe)
        .arg("--hot")
        .arg(library_path(game.library))
        .args(app_args)
        .current_dir(workspace_root())
        .spawn()
        .with_context(|| format!("failed to start {}", exe.display()))?;
    rebuild_while_running(child, game.scripts)
}

/// Opens the editor on Night Relics with its scripts, rebuilt (and so hot reloaded) on save,
/// or on a project folder (the editor builds and reloads a project's scripts itself).
fn editor(args: &[&str]) -> Result<()> {
    let (project, rest) = match args.first() {
        Some(first) if !first.starts_with("--") && *first != "slice" => {
            let project = ab_project::Project::open(&absolute(first)?)?;
            (Some(project), &args[1..])
        }
        Some(_) if args[0] == "slice" => (None, &args[1..]),
        _ => (None, args),
    };
    cargo(&["build", "-p", "ab-editor"])?;
    let exe = debug_dir().join(format!("ab-editor{}", std::env::consts::EXE_SUFFIX));
    let mut command = Command::new(&exe);
    command.current_dir(workspace_root());
    let Some(project) = project else {
        cargo(&["build", "-p", HOT_SLICE.scripts, "--lib"])?;
        let child = command
            .arg("--project")
            .arg(workspace_root().join("assets"))
            .args(["--scene", "slice/main.abscene", "--scripts"])
            .arg(library_path(HOT_SLICE.library))
            .args(rest)
            .spawn()
            .with_context(|| format!("failed to start {}", exe.display()))?;
        return rebuild_while_running(child, HOT_SLICE.scripts);
    };
    let status = command
        .arg("--project")
        .arg(&project.root)
        .args(rest)
        .status()
        .with_context(|| format!("failed to start {}", exe.display()))?;
    if !status.success() {
        bail!("the editor exited with {status}");
    }
    Ok(())
}

/// `ab_project::export` with the editor's arguments (output in `dist/` by default).
fn export(args: &[&str]) -> Result<()> {
    use ab_project::export;
    let (name, options) = export::parse_args(args, &workspace_root().join("dist"))?;
    let game = match name.as_str() {
        "slice" | "night-relics" => export::Game::slice(&workspace_root()),
        "demo" | "ab-app" => export::Game::demo(&workspace_root()),
        path => export::resolve_game(&absolute(path)?.to_string_lossy())?,
    };
    for done in export::export(&game, &options, &mut |line: &str| println!("{line}"))? {
        println!("{:<8} {}", done.platform.name(), done.folder.display());
        if let Some(archive) = done.archive {
            println!("{:<8} {}", "", archive.display());
        }
    }
    Ok(())
}

/// `path` from the folder xtask was started in (cargo runs it there).
fn absolute(path: &str) -> Result<PathBuf> {
    Ok(std::path::absolute(path)?)
}

/// Rebuilds the `scripts` package's library whenever its sources change, until `child` exits.
fn rebuild_while_running(child: std::process::Child, scripts: &str) -> Result<()> {
    let sources = workspace_root().join("crates").join(scripts).join("src");
    rebuild_sources_while_running(child, scripts, &sources)
}

fn rebuild_sources_while_running(
    mut child: std::process::Child,
    scripts: &str,
    sources: &std::path::Path,
) -> Result<()> {
    println!(
        "hot reload: edit {} and save; the running program reloads it",
        sources.display()
    );
    let mut seen = newest_change(sources)?;
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            bail!("the program exited with {status}");
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
        let changed = newest_change(sources)?;
        if changed > seen {
            seen = changed;
            // A failed build is reported by cargo; the game keeps running the last good version.
            if let Err(err) = cargo(&["build", "-p", scripts, "--lib"]) {
                eprintln!("hot reload: {err}");
            }
        }
    }
}

/// Latest modification time of any file under `dir` (skipping `target`).
fn newest_change(dir: &std::path::Path) -> Result<std::time::SystemTime> {
    let mut newest = std::time::SystemTime::UNIX_EPOCH;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                if path.file_name().is_some_and(|n| n != "target") {
                    stack.push(path);
                }
            } else {
                newest = newest.max(entry.metadata()?.modified()?);
            }
        }
    }
    Ok(newest)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace")
        .into()
}
