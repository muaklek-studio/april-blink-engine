//! Platforms games are exported for, and their file naming.

use anyhow::{bail, Result};

/// A platform players run games on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    Linux,
    Windows,
}

impl Platform {
    /// The platform this program runs on.
    pub fn host() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
        }
    }

    /// The Rust target triple of its builds.
    pub fn triple(self) -> &'static str {
        match self {
            Self::Linux => "x86_64-unknown-linux-gnu",
            Self::Windows => "x86_64-pc-windows-msvc",
        }
    }

    pub fn of_triple(triple: &str) -> Self {
        if triple.contains("windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    /// `linux`, `windows`, `all`, `host`, comma-separated.
    pub fn parse(list: &str) -> Result<Vec<Self>> {
        let mut out = Vec::new();
        for name in list.split(',').map(str::trim) {
            let platforms: &[Self] = match name {
                "linux" => &[Self::Linux],
                "windows" | "win" => &[Self::Windows],
                "all" => &[Self::Linux, Self::Windows],
                "host" => &[Self::host()],
                other => bail!("unknown platform `{other}`: use linux, windows or all"),
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
            Self::Linux => stem.to_owned(),
            Self::Windows => format!("{stem}.exe"),
        }
    }

    /// A dynamic library's file name as cargo builds it: `lib<stem>.so`, `<stem>.dll`.
    pub fn library(self, stem: &str) -> String {
        match self {
            Self::Linux => format!("lib{stem}.so"),
            Self::Windows => format!("{stem}.dll"),
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
        assert!(Platform::parse("macos").is_err());
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
    }
}
