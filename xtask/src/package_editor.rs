//! `cargo xtask package-editor`: the editor as game makers get it.
//!
//! ```text
//! april-blink-editor-<platform>/
//!   ab-editor[.exe]
//!   players/<platform>/ab-player[.exe]   exported games are this + a pack + a script library
//!   sdk/ab-script, sdk/ab-script-api     new projects build their scripts against these
//!   README.txt, LICENSE.txt, licenses/
//! ```
//!
//! The editor needs no engine sources: it finds the players and the SDK next to itself
//! (`ab_project::export::player`, `ab_project::sdk_dir`). Game makers need Rust (rustup) to
//! build their scripts, and cargo-xwin to export Windows games from Linux.

use crate::{cargo, workspace_root};
use ab_project::export::{self, Build};
use ab_project::Platform;
use anyhow::{bail, Context as _, Result};
use std::path::{Path, PathBuf};

const NAME: &str = "april-blink-editor";

/// The SDK's crates, in dependency order.
const SDK: &[&str] = &["ab-script-api", "ab-script"];

pub fn package(args: &[&str]) -> Result<()> {
    let mut platforms = vec![Platform::host()];
    let mut archive = false;
    let mut out = workspace_root().join("dist");
    let mut args = args.iter();
    while let Some(&arg) = args.next() {
        match arg {
            "--platform" | "-p" => {
                platforms = Platform::parse(
                    args.next()
                        .context("--platform needs linux, windows or all")?,
                )?;
            }
            "--archive" | "--zip" => archive = true,
            "--out" => out = PathBuf::from(args.next().context("--out needs a directory")?),
            other => bail!(
                "unknown argument `{other}`\n\
                 usage: cargo xtask package-editor [--platform linux|windows|all] [--archive] \
                 [--out <dir>]"
            ),
        }
    }
    let builds: Vec<Build> = platforms.into_iter().map(Build::for_platform).collect();
    for build in &builds {
        export::check_toolchain(build)?;
    }
    let sdk = workspace_root().join("target").join("sdk-package");
    write_sdk(&sdk)?;
    check_sdk(&sdk)?;
    let mut log = |line: &str| println!("{line}");
    for build in &builds {
        let platform = build.platform;
        let dir = out.join(format!("{NAME}-{}", platform.name()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("removing the old {}", dir.display()))?;
        }
        let editor = export::engine_binary(
            &workspace_root(),
            build,
            "ab-editor",
            "ab-editor",
            &[],
            &mut log,
        )?;
        let player = export::player(build, &mut log)?;
        copy_file(&editor, &dir.join(platform.executable("ab-editor")))?;
        copy_file(
            &player,
            &dir.join("players")
                .join(platform.name())
                .join(platform.executable("ab-player")),
        )?;
        copy_dir(&sdk, &dir.join("sdk"))?;
        write_texts(&dir, platform)?;
        println!("{:<8} {}", platform.name(), dir.display());
        if archive {
            let root = format!("{NAME}-{}", platform.name());
            let file = match platform {
                Platform::Windows => {
                    let file = out.join(format!("{root}.zip"));
                    ab_project::archive::zip(&dir, &root, &file)?;
                    file
                }
                Platform::Linux | Platform::MacOs => {
                    let file = out.join(format!("{root}.tar.gz"));
                    ab_project::archive::tar_gz(&dir, &root, &file)?;
                    file
                }
            };
            println!("{:<8} {}", "", file.display());
        }
    }
    Ok(())
}

/// Copies the SDK's crates into `out` as standalone packages: workspace settings written
/// out, the SDK's crates depending on each other by relative path, no tests (they run the
/// engine).
fn write_sdk(out: &Path) -> Result<()> {
    if out.exists() {
        std::fs::remove_dir_all(out)?;
    }
    let workspace: toml::Table = read(&workspace_root().join("Cargo.toml"))?.parse()?;
    let shared = &workspace["workspace"];
    for name in SDK {
        let source = workspace_root().join("crates").join(name);
        let dest = out.join(name);
        copy_dir(&source.join("src"), &dest.join("src"))?;
        if source.join("README.md").is_file() {
            copy_file(&source.join("README.md"), &dest.join("README.md"))?;
        }
        let manifest: toml::Table = read(&source.join("Cargo.toml"))?.parse()?;
        let manifest = standalone(manifest, shared)
            .with_context(|| format!("writing out {name}'s Cargo.toml"))?;
        std::fs::write(
            dest.join("Cargo.toml"),
            format!(
                "# The April Blink script SDK, shipped with the editor (`cargo xtask package-editor`).\n\n{}",
                toml::to_string(&manifest)?
            ),
        )?;
    }
    copy_file(&workspace_root().join("LICENSE"), &out.join("LICENSE.txt"))
}

/// `manifest` with every `workspace = true` replaced by the workspace's value.
fn standalone(mut manifest: toml::Table, shared: &toml::Value) -> Result<toml::Table> {
    let inherited = |value: &toml::Value| {
        value
            .get("workspace")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false)
    };
    let package = manifest
        .get_mut("package")
        .and_then(toml::Value::as_table_mut)
        .context("no [package]")?;
    for (key, value) in package.iter_mut() {
        if inherited(value) {
            *value = shared["package"]
                .get(key)
                .with_context(|| format!("no workspace.package.{key}"))?
                .clone();
        }
    }
    let version = shared["package"]["version"].clone();
    if let Some(dependencies) = manifest
        .get_mut("dependencies")
        .and_then(toml::Value::as_table_mut)
    {
        for (name, value) in dependencies.iter_mut() {
            if !inherited(value) {
                continue;
            }
            let mut dependency = match shared["dependencies"]
                .get(name)
                .with_context(|| format!("no workspace dependency {name}"))?
            {
                toml::Value::String(version) => {
                    toml::Table::from_iter([("version".to_owned(), version.clone().into())])
                }
                toml::Value::Table(table) => table.clone(),
                other => bail!("unexpected workspace dependency {name} = {other}"),
            };
            if dependency.contains_key("path") {
                if !SDK.contains(&name.as_str()) {
                    bail!("the SDK depends on {name}, which is not part of it");
                }
                dependency.insert("path".into(), format!("../{name}").into());
                dependency.insert("version".into(), version.clone());
            }
            // Keys next to `workspace = true` (features, optional) add to the workspace's.
            for (key, extra) in value.as_table().into_iter().flatten() {
                if key == "workspace" {
                    continue;
                }
                match (dependency.get_mut(key), extra) {
                    (Some(toml::Value::Array(features)), toml::Value::Array(more)) => {
                        features.extend(more.iter().cloned());
                    }
                    _ => {
                        dependency.insert(key.clone(), extra.clone());
                    }
                }
            }
            *value = dependency.into();
        }
    }
    manifest.remove("dev-dependencies");
    manifest.remove("lints");
    // Its own workspace: it builds wherever it is unpacked.
    manifest.insert("workspace".into(), toml::Table::new().into());
    Ok(manifest)
}

/// Builds the SDK copy on its own, as a new project will.
fn check_sdk(sdk: &Path) -> Result<()> {
    let manifest = sdk.join("ab-script").join("Cargo.toml");
    let target = workspace_root().join("target").join("sdk-check");
    cargo(&[
        "check",
        "--manifest-path",
        &manifest.to_string_lossy(),
        "--target-dir",
        &target.to_string_lossy(),
    ])?;
    // Projects resolve their own versions.
    for name in SDK {
        let _ = std::fs::remove_file(sdk.join(name).join("Cargo.lock"));
    }
    Ok(())
}

fn write_texts(dir: &Path, platform: Platform) -> Result<()> {
    let editor = platform.executable("ab-editor");
    let run = match platform {
        Platform::Windows => format!("Double-click `{editor}`."),
        Platform::Linux | Platform::MacOs => format!("Run `./{editor}`."),
    };
    let windows_from_linux = match platform {
        Platform::Linux | Platform::MacOs => {
            "\nTo export Windows games from Linux, also install cargo-xwin, clang and lld:\n  \
             cargo install --locked cargo-xwin\n"
        }
        Platform::Windows => {
            "On Windows, Rust needs Microsoft's C++ build tools for its linker: let rustup install\n\
             them, or install \"Desktop development with C++\" from\n\
             https://visualstudio.microsoft.com/visual-cpp-build-tools/\n"
        }
    };
    let text = format!(
        "April Blink Editor {version}\n\n\
         {run} Keep the other files next to it: players/ is what exported games run on, and\n\
         sdk/ is the script SDK new projects build against.\n\n\
         Game scripts are Rust: install Rust from https://rustup.rs before making a project.\n\
         The editor builds a project's scripts with cargo whenever you save one.\n\
         {windows_from_linux}\n\
         Exporting from the command line (no window):\n  \
         {editor} --export <project folder> [--platform linux|windows|all] [--archive]\n\n\
         The Prompt font is under the SIL Open Font License (licenses/Prompt-font-OFL.txt);\n\
         the editor is under the MIT license (LICENSE.txt).\n",
        version = env!("CARGO_PKG_VERSION"),
    );
    let text = if platform == Platform::Windows {
        text.replace('\n', "\r\n")
    } else {
        text
    };
    std::fs::write(dir.join("README.txt"), text)?;
    copy_file(&workspace_root().join("LICENSE"), &dir.join("LICENSE.txt"))?;
    copy_file(
        &workspace_root().join("crates/ab-ui/fonts/OFL.txt"),
        &dir.join("licenses/Prompt-font-OFL.txt"),
    )
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

fn copy_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(from, to)
        .with_context(|| format!("copying {} to {}", from.display(), to.display()))?;
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&path, &dest)?;
        } else {
            copy_file(&path, &dest)?;
        }
    }
    Ok(())
}
