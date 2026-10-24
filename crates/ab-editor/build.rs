//! Records how the editor was built, for Help → About: the compiler, the target, the
//! profile and the commit of the engine sources.

use std::path::Path;
use std::process::Command;

fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let compiler = Command::new(&rustc)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=AB_BUILD_RUSTC={compiler}");
    println!(
        "cargo:rustc-env=AB_BUILD_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );
    println!(
        "cargo:rustc-env=AB_BUILD_PROFILE={}",
        std::env::var("PROFILE").unwrap_or_default()
    );

    // The commit, when built from a git checkout (a source archive has none).
    let commit = Command::new("git")
        .args(["rev-parse", "--short=9", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    println!("cargo:rustc-env=AB_BUILD_COMMIT={commit}");
    // Built again when the commit changes; a missing file would rebuild every time.
    for file in ["../../.git/HEAD", "../../.git/refs/heads"] {
        if Path::new(file).exists() {
            println!("cargo:rerun-if-changed={file}");
        }
    }
    println!("cargo:rerun-if-env-changed=RUSTC");
    println!("cargo:rerun-if-changed=build.rs");
}
