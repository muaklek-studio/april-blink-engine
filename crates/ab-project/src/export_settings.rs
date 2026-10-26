//! What an exported game says about itself: `[export]` in `game.toml`.
//!
//! ```toml
//! title = "Night Relics"
//!
//! [export]
//! creator = "Moonlit Games"
//! description = "Find the relics before dawn."
//! copyright = "© 2026 Moonlit Games"
//! version = "1.2.0"              # else the Cargo.toml version
//! icon = "assets/ui/icon.png"    # PNG (256 × 256 best) or ICO, in the project
//! file_name = "NightRelics"      # the executable's name (else the package's)
//! console = false                # Windows: no console window behind the game
//! out = "dist"                   # where exports go
//! ```
//!
//! On Windows they go into the executable: its icon (Explorer, the taskbar, shortcuts), and
//! its version information (the Details tab of its properties: product name, company,
//! description, copyright, versions). [`ExportSettings::check`] says what is wrong before
//! an export starts; [`apply_windows`] writes them into an executable.

use anyhow::{bail, Context as _, Result};
use std::path::{Path, PathBuf};

/// `[export]` of `game.toml` (see the module docs). Empty values are unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportSettings {
    pub creator: String,
    pub description: String,
    pub copyright: String,
    /// Overrides the `Cargo.toml` version.
    pub version: String,
    /// Relative to the project folder.
    pub icon: String,
    /// The executable's name without extension.
    pub file_name: String,
    /// Windows: show a console window with the game (for its log).
    pub console: bool,
    /// Relative to the project folder, or absolute.
    pub out: String,
}

/// One thing wrong with the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// The setting (`version`, `icon`...).
    pub field: &'static str,
    pub message: String,
    /// It stops the export (else a warning).
    pub error: bool,
}

/// Names Windows keeps for devices: no file may be called so.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

impl ExportSettings {
    /// The `[export]` table of `game.toml`'s settings.
    pub fn from_table(settings: &toml::Table) -> Self {
        let export = settings.get("export").and_then(toml::Value::as_table);
        let text = |key: &str| {
            export
                .and_then(|t| t.get(key))
                .and_then(toml::Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned()
        };
        Self {
            creator: text("creator"),
            description: text("description"),
            copyright: text("copyright"),
            version: text("version"),
            icon: text("icon"),
            file_name: text("file_name"),
            console: export
                .and_then(|t| t.get("console"))
                .and_then(toml::Value::as_bool)
                .unwrap_or(false),
            out: text("out"),
        }
    }

    /// Writes these settings (and `title`) into the `game.toml` of `root`, keeping its other
    /// keys.
    pub fn save(&self, root: &Path, title: &str) -> Result<()> {
        let path = root.join(crate::GAME_FILE);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut settings: toml::Table = text
            .parse()
            .with_context(|| format!("invalid {}", path.display()))?;
        if !title.trim().is_empty() {
            settings.insert("title".into(), toml::Value::String(title.trim().to_owned()));
        }
        let mut export = toml::Table::new();
        for (key, value) in [
            ("creator", &self.creator),
            ("description", &self.description),
            ("copyright", &self.copyright),
            ("version", &self.version),
            ("icon", &self.icon),
            ("file_name", &self.file_name),
            ("out", &self.out),
        ] {
            if !value.trim().is_empty() {
                export.insert(key.into(), toml::Value::String(value.trim().to_owned()));
            }
        }
        if self.console {
            export.insert("console".into(), toml::Value::Boolean(true));
        }
        if export.is_empty() {
            settings.remove("export");
        } else {
            settings.insert("export".into(), toml::Value::Table(export));
        }
        let text = toml::to_string(&settings).context("writing game.toml")?;
        write_whole(&path, text.as_bytes())
    }

    /// The icon file, if one is set.
    pub fn icon_path(&self, root: &Path) -> Option<PathBuf> {
        (!self.icon.is_empty()).then(|| root.join(&self.icon))
    }

    /// The output folder, if one is set.
    pub fn out_dir(&self, root: &Path) -> Option<PathBuf> {
        (!self.out.is_empty()).then(|| {
            let out = Path::new(&self.out);
            if out.is_absolute() {
                out.to_path_buf()
            } else {
                root.join(out)
            }
        })
    }

    /// What is wrong with the settings of the project at `root` titled `title`.
    pub fn check(&self, root: &Path, title: &str) -> Vec<Issue> {
        let mut issues = Vec::new();
        let mut add = |field, message: String, error| {
            issues.push(Issue {
                field,
                message,
                error,
            })
        };
        if title.trim().is_empty() {
            add("title", "the game needs a title".into(), true);
        } else if title.chars().count() > 128 {
            add(
                "title",
                "the title is longer than 128 characters".into(),
                true,
            );
        }
        if !self.version.is_empty() {
            if let Err(err) = windows_version(&self.version) {
                add("version", err, true);
            }
        }
        for (field, value, max) in [
            ("creator", &self.creator, 128),
            ("description", &self.description, 512),
            ("copyright", &self.copyright, 256),
        ] {
            if value.chars().count() > max {
                add(field, format!("longer than {max} characters"), true);
            }
            if value.chars().any(char::is_control) {
                add(
                    field,
                    "has control characters (line breaks, tabs)".into(),
                    true,
                );
            }
        }
        if self.creator.is_empty() {
            add(
                "creator",
                "not set: Windows shows the company as empty".into(),
                false,
            );
        }
        if !self.file_name.is_empty() {
            if let Err(err) = check_file_name(&self.file_name) {
                add("file_name", err, true);
            }
        }
        if let Some(icon) = self.icon_path(root) {
            match std::fs::read(&icon) {
                Err(err) => add("icon", format!("{}: {err}", icon.display()), true),
                Ok(bytes) => match icon_info(&bytes) {
                    Err(err) => add("icon", err, true),
                    Ok((w, h)) => {
                        if w != h {
                            add("icon", format!("{w} × {h}: icons are square"), true);
                        } else if w < 256 {
                            add(
                                "icon",
                                format!("{w} × {h}: 256 × 256 looks sharp on large icons"),
                                false,
                            );
                        }
                    }
                },
            }
        } else {
            add(
                "icon",
                "not set: the game gets the player's icon".into(),
                false,
            );
        }
        if let Some(out) = self.out_dir(root) {
            if out.starts_with(root.join("assets"))
                || out == root
                || out.starts_with(root.join("src"))
            {
                add(
                    "out",
                    format!("{} is inside the game's sources", out.display()),
                    true,
                );
            } else if out.is_file() {
                add("out", format!("{} is a file", out.display()), true);
            }
        }
        issues
    }
}

fn write_whole(path: &Path, bytes: &[u8]) -> Result<()> {
    // Written whole or not at all: a crash midway never leaves half a game.toml.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}

/// Whether `name` can name an executable on every platform.
pub fn check_file_name(name: &str) -> Result<(), String> {
    if name.trim() != name || name.is_empty() {
        return Err("the file name starts or ends with spaces".into());
    }
    if let Some(c) = name
        .chars()
        .find(|c| r#"<>:"/\|?*"#.contains(*c) || c.is_control())
    {
        return Err(format!("`{c}` cannot be in a file name"));
    }
    if name.ends_with('.') {
        return Err("the file name ends with a dot".into());
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return Err(format!("{name} is a name Windows keeps for devices"));
    }
    if name.len() > 100 {
        return Err("the file name is longer than 100 characters".into());
    }
    Ok(())
}

/// `1.2.3` (up to four numbers, each up to 65535) as Windows' version numbers.
pub fn windows_version(version: &str) -> Result<[u16; 4], String> {
    // A pre-release or build part (`1.0.0-beta`) is not a number Windows keeps.
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<&str> = core.split('.').collect();
    if parts.is_empty() || parts.len() > 4 || parts.iter().any(|p| p.is_empty()) {
        return Err(format!(
            "`{version}`: a version is 1 to 4 numbers like 1.2.0"
        ));
    }
    let mut out = [0u16; 4];
    for (slot, part) in out.iter_mut().zip(&parts) {
        *slot = part
            .parse()
            .map_err(|_| format!("`{version}`: `{part}` is not a number from 0 to 65535"))?;
    }
    Ok(out)
}

/// Width and height of a PNG or ICO (its largest image).
pub fn icon_info(bytes: &[u8]) -> Result<(u32, u32), String> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.starts_with(PNG) {
        if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
            return Err("the PNG is damaged".into());
        }
        let w = u32::from_be_bytes(bytes[16..20].try_into().expect("4 bytes"));
        let h = u32::from_be_bytes(bytes[20..24].try_into().expect("4 bytes"));
        return Ok((w, h));
    }
    if bytes.len() >= 6 && bytes[0..4] == [0, 0, 1, 0] {
        let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        if count == 0 || bytes.len() < 6 + count * 16 {
            return Err("the ICO is damaged".into());
        }
        let size = |b: u8| if b == 0 { 256 } else { u32::from(b) };
        return Ok((0..count)
            .map(|i| {
                let e = &bytes[6 + i * 16..];
                (size(e[0]), size(e[1]))
            })
            .max()
            .expect("at least one image"));
    }
    Err("the icon is not a PNG or ICO file".into())
}

/// An ICO holding `png` as its one image (Windows scales it to the sizes it shows).
pub fn png_to_ico(png: &[u8]) -> Result<Vec<u8>, String> {
    let (w, h) = icon_info(png)?;
    let byte = |v: u32| if v >= 256 { 0 } else { v as u8 };
    let mut ico = Vec::with_capacity(22 + png.len());
    ico.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    ico.extend_from_slice(&[byte(w), byte(h), 0, 0]);
    ico.extend_from_slice(&1u16.to_le_bytes()); // planes
    ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(png);
    Ok(ico)
}

/// What a Windows executable says about the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsMetadata {
    pub title: String,
    pub version: String,
    pub creator: String,
    pub description: String,
    pub copyright: String,
    /// The executable's file name (with `.exe`).
    pub file_name: String,
    /// ICO or PNG bytes.
    pub icon: Option<Vec<u8>>,
    pub console: bool,
}

/// Writes `meta` into the Windows executable at `exe`: icon, version information and
/// whether it opens a console. Fails (and leaves the file as it was) on an executable it
/// cannot read.
pub fn apply_windows(exe: &Path, meta: &WindowsMetadata) -> Result<()> {
    use editpe::{Image, VersionInfo, VersionStringTable};
    let numbers = windows_version(&meta.version).map_err(anyhow::Error::msg)?;
    let bytes = std::fs::read(exe).with_context(|| format!("reading {}", exe.display()))?;
    let mut image = Image::parse(bytes.as_slice())
        .map_err(|e| anyhow::anyhow!("{}: not a Windows executable ({e})", exe.display()))?;
    let mut resources = image.resource_directory().cloned().unwrap_or_default();
    if let Some(icon) = &meta.icon {
        let ico = if icon.starts_with(&[0, 0, 1, 0]) {
            icon.clone()
        } else {
            png_to_ico(icon).map_err(anyhow::Error::msg)?
        };
        // The player's own icon goes: only the game's is left.
        let _ = resources.remove_main_icon();
        resources
            .set_main_icon(ico)
            .map_err(|e| anyhow::anyhow!("icon: {e}"))?;
    }
    let mut info = VersionInfo::default();
    let packed = |[a, b, c, d]: [u16; 4]| editpe::types::VersionU32 {
        major: (u32::from(a) << 16) | u32::from(b),
        minor: (u32::from(c) << 16) | u32::from(d),
    };
    info.info.file_version = packed(numbers);
    info.info.product_version = packed(numbers);
    let mut table = VersionStringTable {
        // US English, Unicode.
        key: "040904B0".into(),
        ..Default::default()
    };
    let description = if meta.description.is_empty() {
        &meta.title
    } else {
        &meta.description
    };
    for (key, value) in [
        ("CompanyName", meta.creator.as_str()),
        ("FileDescription", description.as_str()),
        ("FileVersion", meta.version.as_str()),
        ("InternalName", meta.file_name.trim_end_matches(".exe")),
        ("LegalCopyright", meta.copyright.as_str()),
        ("OriginalFilename", meta.file_name.as_str()),
        ("ProductName", meta.title.as_str()),
        ("ProductVersion", meta.version.as_str()),
    ] {
        if !value.is_empty() {
            table.strings.insert(key.into(), value.into());
        }
    }
    info.strings = vec![table];
    info.vars = vec![editpe::types::VersionU16 {
        major: 0x0409,
        minor: 1200,
    }];
    resources
        .set_version_info(&info)
        .map_err(|e| anyhow::anyhow!("version information: {e}"))?;
    image
        .set_resource_directory(resources)
        .map_err(|e| anyhow::anyhow!("writing resources: {e}"))?;
    // 2: a window program, 3: a console program.
    image.set_subsystem(if meta.console { 3 } else { 2 });
    let mut out = Vec::with_capacity(bytes.len() + 64 * 1024);
    image
        .write_writer(&mut out)
        .map_err(|e| anyhow::anyhow!("{}: {e}", exe.display()))?;
    if !bytes.is_empty() && out.is_empty() {
        bail!("{}: writing the executable failed", exe.display());
    }
    write_whole(exe, &out)
}

/// What a Windows executable says about itself (see [`read_windows`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsDetails {
    /// Bytes of its main icon's group, if it has one.
    pub icon: Option<usize>,
    /// Its version strings (`ProductName`, `FileVersion`...).
    pub strings: Vec<(String, String)>,
    /// 2: a window program, 3: a console program.
    pub subsystem: u16,
}

/// What `apply_windows` wrote, read back (to check an export).
pub fn read_windows(exe: &Path) -> Result<WindowsDetails> {
    let bytes = std::fs::read(exe).with_context(|| format!("reading {}", exe.display()))?;
    let image = editpe::Image::parse(bytes.as_slice()).map_err(|e| anyhow::anyhow!("{e}"))?;
    let resources = image.resource_directory();
    let icon = resources
        .and_then(|r| r.get_main_icon().ok().flatten())
        .map(<[u8]>::len);
    let strings = resources
        .and_then(|r| r.get_version_info().ok().flatten())
        .map(|v| {
            v.strings
                .iter()
                .flat_map(|t| t.strings.iter().map(|(k, v)| (k.clone(), v.clone())))
                .collect()
        })
        .unwrap_or_default();
    Ok(WindowsDetails {
        icon,
        strings,
        subsystem: image.subsystem(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_names_and_icons_are_checked() {
        assert_eq!(windows_version("1.2.3"), Ok([1, 2, 3, 0]));
        assert_eq!(windows_version("2.0.0-beta.1"), Ok([2, 0, 0, 0]));
        assert!(windows_version("1.x").is_err());
        assert!(windows_version("70000.0").is_err());
        assert!(windows_version("1.2.3.4.5").is_err());
        assert!(check_file_name("NightRelics").is_ok());
        assert!(check_file_name("con").unwrap_err().contains("devices"));
        assert!(check_file_name("a:b").is_err());
        assert!(check_file_name("game.").is_err());
        // A 256 × 256 PNG header, wrapped as an ICO.
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&256u32.to_be_bytes());
        png.extend_from_slice(&256u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        assert_eq!(icon_info(&png), Ok((256, 256)));
        let ico = png_to_ico(&png).unwrap();
        assert_eq!(icon_info(&ico), Ok((256, 256)));
        assert!(icon_info(b"GIF89a").is_err());
    }

    /// On a real Windows executable: `AB_TEST_WINDOWS_EXE`, or a player cross-built in this
    /// checkout. Skipped without one.
    #[test]
    fn icons_and_details_go_into_windows_executables() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let exe = std::env::var_os("AB_TEST_WINDOWS_EXE")
            .map(PathBuf::from)
            .or_else(|| {
                [
                    "x86_64-pc-windows-msvc/dist",
                    "x86_64-pc-windows-msvc/release",
                    "x86_64-pc-windows-gnu/release",
                ]
                .iter()
                .map(|dir| root.join("target").join(dir).join("ab-player.exe"))
                .find(|p| p.is_file())
            });
        let Some(exe) = exe else {
            eprintln!("no Windows executable to test with: skipped");
            return;
        };
        let copy = std::env::temp_dir().join(format!("ab-windows-meta-{}.exe", std::process::id()));
        std::fs::copy(&exe, &copy).unwrap();
        let png = std::fs::read(root.join("assets/models/totem_albedo.png")).unwrap();
        let meta = WindowsMetadata {
            title: "Night Relics".into(),
            version: "1.4.2".into(),
            creator: "Moonlit Games".into(),
            description: "Find the relics before dawn.".into(),
            copyright: "© 2026 Moonlit Games".into(),
            file_name: "NightRelics.exe".into(),
            icon: Some(png),
            console: false,
        };
        apply_windows(&copy, &meta).unwrap();
        let WindowsDetails {
            icon,
            strings,
            subsystem,
        } = read_windows(&copy).unwrap();
        assert!(icon.is_some(), "the icon is in");
        let get = |k: &str| {
            strings
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("ProductName"), Some("Night Relics"));
        assert_eq!(get("CompanyName"), Some("Moonlit Games"));
        assert_eq!(get("FileVersion"), Some("1.4.2"));
        assert_eq!(get("OriginalFilename"), Some("NightRelics.exe"));
        assert_eq!(get("LegalCopyright"), Some("© 2026 Moonlit Games"));
        assert_eq!(subsystem, 2, "a window program");
        // Again, with a console: replaced, not added to.
        apply_windows(
            &copy,
            &WindowsMetadata {
                console: true,
                version: "1.5.0".into(),
                ..meta
            },
        )
        .unwrap();
        let WindowsDetails {
            strings, subsystem, ..
        } = read_windows(&copy).unwrap();
        assert_eq!(subsystem, 3);
        assert_eq!(
            strings.iter().filter(|(k, _)| k == "FileVersion").count(),
            1
        );
        assert!(strings
            .iter()
            .any(|(k, v)| k == "FileVersion" && v == "1.5.0"));
        // Not an executable: refused, and left as it was.
        let text = std::env::temp_dir().join(format!("ab-windows-meta-{}.txt", std::process::id()));
        std::fs::write(&text, "hello").unwrap();
        let err = apply_windows(
            &text,
            &WindowsMetadata {
                console: false,
                version: "1.0".into(),
                file_name: "x.exe".into(),
                title: "x".into(),
                creator: String::new(),
                description: String::new(),
                copyright: String::new(),
                icon: None,
            },
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("not a Windows executable"),
            "{err:#}"
        );
        assert_eq!(std::fs::read_to_string(&text).unwrap(), "hello");
        let _ = std::fs::remove_file(&copy);
        let _ = std::fs::remove_file(&text);
    }

    #[test]
    fn settings_round_trip_through_game_toml_and_report_problems() {
        let dir = std::env::temp_dir().join(format!("ab-export-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(crate::GAME_FILE),
            "title = \"Old\"\nmain_scene = \"main.abscene\"\n",
        )
        .unwrap();
        let settings = ExportSettings {
            creator: "Moonlit".into(),
            version: "1.4.0".into(),
            file_name: "NightRelics".into(),
            icon: "icon.png".into(),
            console: true,
            ..Default::default()
        };
        settings.save(&dir, "Night Relics").unwrap();
        let text = std::fs::read_to_string(dir.join(crate::GAME_FILE)).unwrap();
        let table: toml::Table = text.parse().unwrap();
        assert_eq!(ExportSettings::from_table(&table), settings);
        assert_eq!(table["title"].as_str(), Some("Night Relics"));
        assert_eq!(
            table["main_scene"].as_str(),
            Some("main.abscene"),
            "other keys kept"
        );
        // The icon is missing: an error; a bad version too.
        let issues = settings.check(&dir, "Night Relics");
        assert!(
            issues.iter().any(|i| i.field == "icon" && i.error),
            "{issues:?}"
        );
        let bad = ExportSettings {
            version: "one".into(),
            out: "assets/x".into(),
            ..settings.clone()
        };
        let issues = bad.check(&dir, "");
        for field in ["title", "version", "out"] {
            assert!(
                issues.iter().any(|i| i.field == field && i.error),
                "{field}: {issues:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
