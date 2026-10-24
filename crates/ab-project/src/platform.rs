//! Platforms games are exported for, and their file naming.

use anyhow::{bail, Result};

/// A platform players run games on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    Linux,
    Windows,
    /// Only as the host (the editor and Play mode on a Mac): games are not cross-compiled
    /// for it.
    MacOs,
}

impl Platform {
    /// The platform this program runs on.
    pub fn host() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
            Self::MacOs => "macos",
        }
    }

    /// The Rust target triple of its builds.
    pub fn triple(self) -> &'static str {
        match self {
            Self::Linux => "x86_64-unknown-linux-gnu",
            Self::Windows => "x86_64-pc-windows-msvc",
            Self::MacOs if cfg!(target_arch = "x86_64") => "x86_64-apple-darwin",
            Self::MacOs => "aarch64-apple-darwin",
        }
    }

    pub fn of_triple(triple: &str) -> Self {
        if triple.contains("windows") {
            Self::Windows
        } else if triple.contains("apple") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    /// `linux`, `windows`, `all`, `host`, comma-separated; `macos` on a Mac.
    pub fn parse(list: &str) -> Result<Vec<Self>> {
        let mut out = Vec::new();
        for name in list.split(',').map(str::trim) {
            let platforms: &[Self] = match name {
                "linux" => &[Self::Linux],
                "windows" | "win" => &[Self::Windows],
                "all" => &[Self::Linux, Self::Windows],
                "host" => &[Self::host()],
                "macos" | "mac" if Self::host() == Self::MacOs => &[Self::MacOs],
                other if other.starts_with("mac") => {
                    bail!("macOS games are exported on a Mac (Apple's linker and SDK)")
                }
                other => bail!("unknown platform `{other}`: use linux, windows, macos or all"),
            };
            for &p in platforms {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
        Ok(out)
    }

    /// An executable's file name: `name` or `name.exe`.
    pub fn executable(self, stem: &str) -> String {
        match self {
            Self::Linux | Self::MacOs => stem.to_owned(),
            Self::Windows => format!("{stem}.exe"),
        }
    }

    /// A dynamic library's file name as cargo builds it: `lib<stem>.so`, `<stem>.dll`,
    /// `lib<stem>.dylib`.
    pub fn library(self, stem: &str) -> String {
        match self {
            Self::Linux => format!("lib{stem}.so"),
            Self::Windows => format!("{stem}.dll"),
            Self::MacOs => format!("lib{stem}.dylib"),
        }
    }

    /// The script library of an exported game whose executable is `<stem>[.exe]`; the
    /// player looks for it there (`lib<stem>_scripts.so`, `<stem>_scripts.dll`).
    pub fn shipped_library(self, stem: &str) -> String {
        self.library(&format!("{stem}_scripts"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lists_and_names_files() {
        assert_eq!(
            Platform::parse("all").unwrap(),
            [Platform::Linux, Platform::Windows]
        );
        assert_eq!(
            Platform::parse("windows, linux,win").unwrap(),
            [Platform::Windows, Platform::Linux]
        );
        assert_eq!(
            Platform::parse("macos").is_ok(),
            Platform::host() == Platform::MacOs
        );
        assert_eq!(Platform::Windows.executable("game"), "game.exe");
        assert_eq!(
            Platform::Linux.shipped_library("my-game"),
            "libmy-game_scripts.so"
        );
        assert_eq!(
            Platform::Windows.shipped_library("my-game"),
            "my-game_scripts.dll"
        );
        // What the host's loader expects matches what cargo builds.
        let host = Platform::host();
        assert_eq!(
            host.library("x"),
            format!(
                "{}x{}",
                std::env::consts::DLL_PREFIX,
                std::env::consts::DLL_SUFFIX
            )
        );
        assert_eq!(
            Platform::of_triple(Platform::Windows.triple()),
            Platform::Windows
        );
        assert_eq!(Platform::of_triple("aarch64-apple-darwin"), Platform::MacOs);
        assert_eq!(Platform::MacOs.library("x"), "libx.dylib");
    }
}
