//! Embeds the new-project template (`template/`) so editors and tools create projects
//! without reading the engine's sources at run time.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let template = manifest.join("template");
    let template = template
        .canonicalize()
        .unwrap_or_else(|e| panic!("{}: {e}", template.display()));
    println!("cargo:rerun-if-changed={}", template.display());

    let mut files = Vec::new();
    collect(&template, &template, &mut files);
    files.sort();
    let mut out = String::from(
        "/// The new-game template's files: path relative to the game folder, contents.\n\
         pub const TEMPLATE: &[(&str, &[u8])] = &[\n",
    );
    for (relative, absolute) in files {
        writeln!(out, "    ({relative:?}, include_bytes!({absolute:?})),").unwrap();
    }
    out.push_str("];\n");
    let dest =
        PathBuf::from(std::env::var_os("OUT_DIR").expect("set by cargo")).join("template.rs");
    std::fs::write(dest, out).expect("writing template.rs");
}

fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).expect("reading the game template") {
        let path = entry.expect("reading the game template").path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name == ".git" || name == "target" {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("inside the template")
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            files.push((relative, path.to_string_lossy().into_owned()));
        }
    }
}
