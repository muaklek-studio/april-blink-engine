//! `cargo xtask new-game "<title>" [--in <dir>]`: a new project, the same as the editor's
//! New Project (`ab_project::create`): a scene (floor, light, camera), an input map and a
//! script crate building against the SDK.

use anyhow::{bail, Context as _, Result};
use std::path::PathBuf;

pub fn new_game(args: &[&str]) -> Result<()> {
    let mut title = None;
    let mut location = None;
    let mut args = args.iter();
    while let Some(&arg) = args.next() {
        match arg {
            "--in" => location = Some(PathBuf::from(args.next().context("--in needs a folder")?)),
            other if other.starts_with("--") => bail!("unknown option `{other}`\n\n{USAGE}"),
            other if title.is_none() => title = Some(other.to_owned()),
            _ => bail!("{USAGE}"),
        }
    }
    let Some(title) = title else {
        bail!("{USAGE}");
    };
    let location = match location {
        Some(dir) => std::path::absolute(dir)?,
        None => std::env::current_dir()?,
    };
    std::fs::create_dir_all(&location)
        .with_context(|| format!("creating {}", location.display()))?;
    let project = ab_project::create(&ab_project::NewProject {
        title,
        location,
        new_folder: true,
    })?;
    let root = project.root.display();
    println!(
        "created {}: {root}\n\n\
         next:\n  cargo xtask editor {root}            # edit its scenes and scripts (F5 plays)\n  \
         cargo xtask export {root}            # a shippable build\n\n\
         Scripts are in src/scripts, scenes in assets/scenes.",
        project.title
    );
    Ok(())
}

const USAGE: &str = "usage: cargo xtask new-game \"<title>\" [--in <folder>]   \
                     (creates <folder>/<name>; default folder: the current one)";
