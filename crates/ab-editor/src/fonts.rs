//! The editor's fonts: what draws each piece of text, with fallbacks.
//!
//! Proportional text tries, in order: the user's font (Preferences), Prompt (bundled: Latin
//! and Thai), fonts found on the system for other scripts (Chinese, Japanese, Korean,
//! Arabic, …), then egui's own (symbols, emoji). Monospace text puts Hack first and falls
//! back the same way, so Thai or CJK in the console and code views is not drawn as boxes.
//! Headings use Prompt SemiBold ([`BOLD`]).
//!
//! A font file is checked before use: one that does not parse is refused with a message,
//! never handed to egui (which would fail later, while drawing).

use eframe::egui::{FontData, FontDefinitions, FontFamily};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// The family of headings and strong labels.
pub const BOLD: &str = "bold";

const PROMPT: &[u8] = include_bytes!("../../ab-ui/fonts/Prompt-Regular.ttf");
const PROMPT_SEMIBOLD: &[u8] = include_bytes!("../../ab-ui/fonts/Prompt-SemiBold.ttf");
/// Font files larger than this are refused (a mistaken pick, not a font).
const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;

/// A font file read and checked.
#[derive(Debug, Clone)]
pub struct LoadedFont {
    pub name: String,
    pub bytes: Arc<[u8]>,
    /// Face inside a collection (`.ttc`).
    pub index: u32,
}

/// Reads `path` and checks it is a font (TrueType, OpenType or a collection).
pub fn load_font(path: &Path) -> Result<LoadedFont, String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    if size > MAX_FONT_BYTES {
        return Err(format!("{} is too big to be a font", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if rustybuzz::Face::from_slice(&bytes, 0).is_none() {
        return Err(format!(
            "{} is not a font this editor can read (use .ttf, .otf or .ttc)",
            path.display()
        ));
    }
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "custom".into());
    Ok(LoadedFont {
        name,
        bytes: bytes.into(),
        index: 0,
    })
}

/// Whether `font` has a glyph for every character of `sample` (spaces aside).
#[cfg(test)]
pub fn covers(font: &LoadedFont, sample: &str) -> bool {
    rustybuzz::Face::from_slice(&font.bytes, font.index).is_some_and(|face| {
        sample
            .chars()
            .filter(|c| !c.is_whitespace())
            .all(|c| face.glyph_index(c).is_some())
    })
}

/// Fonts on this system for scripts Prompt lacks, found once. Each is a known file name in
/// the usual font folders of Linux, Windows and macOS.
pub fn system_fallbacks() -> &'static [LoadedFont] {
    static FOUND: OnceLock<Vec<LoadedFont>> = OnceLock::new();
    FOUND.get_or_init(|| {
        let dirs = font_dirs();
        let mut found: Vec<LoadedFont> = Vec::new();
        // One per script group: the first file found wins.
        for group in FALLBACKS {
            let font = group.iter().find_map(|file| {
                let path = dirs.iter().map(|d| d.join(file)).find(|p| p.is_file())?;
                load_font(&path)
                    .map_err(|err| tracing::debug!("skipping fallback font: {err}"))
                    .ok()
            });
            if let Some(font) = font {
                tracing::debug!(font = %font.name, "fallback font");
                found.push(font);
            }
        }
        found
    })
}

/// Candidate files per script group, most suitable first.
const FALLBACKS: &[&[&str]] = &[
    // Chinese, Japanese, Korean.
    &[
        "opentype/noto/NotoSansCJK-Regular.ttc",
        "noto-cjk/NotoSansCJK-Regular.ttc",
        "google-noto-cjk/NotoSansCJK-Regular.ttc",
        "NotoSansCJK-Regular.ttc",
        "msyh.ttc",
        "YuGothM.ttc",
        "malgun.ttf",
        "Hiragino Sans GB.ttc",
        "PingFang.ttc",
        "truetype/wqy/wqy-microhei.ttc",
    ],
    // Arabic, Hebrew, Greek, Cyrillic and many symbols.
    &[
        "truetype/dejavu/DejaVuSans.ttf",
        "dejavu/DejaVuSans.ttf",
        "DejaVuSans.ttf",
        "segoeui.ttf",
        "arial.ttf",
        "Supplemental/Arial Unicode.ttf",
    ],
    // Devanagari and other Indic scripts.
    &[
        "truetype/noto/NotoSansDevanagari-Regular.ttf",
        "Nirmala.ttf",
        "Kohinoor.ttc",
    ],
];

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        let windows = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
        dirs.push(PathBuf::from(windows).join("Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Microsoft\\Windows\\Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        dirs.extend(["/System/Library/Fonts", "/Library/Fonts"].map(PathBuf::from));
    } else {
        dirs.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(PathBuf::from));
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            dirs.push(home.join(".local/share/fonts"));
            dirs.push(home.join(".fonts"));
        }
    }
    dirs
}

/// The font set: `custom` first when given, then Prompt and the fallbacks.
pub fn definitions(custom: Option<&LoadedFont>, system: &[LoadedFont]) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let egui_proportional = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let egui_monospace = fonts
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();
    let add = |fonts: &mut FontDefinitions, key: String, data: FontData| {
        fonts.font_data.insert(key.clone(), Arc::new(data));
        key
    };
    let shared = |font: &LoadedFont| {
        let mut data = FontData::from_owned(font.bytes.to_vec());
        data.index = font.index;
        data
    };

    let custom = custom.map(|font| add(&mut fonts, format!("custom:{}", font.name), shared(font)));
    let prompt = add(&mut fonts, "prompt".into(), FontData::from_static(PROMPT));
    let semibold = add(
        &mut fonts,
        "prompt-semibold".into(),
        FontData::from_static(PROMPT_SEMIBOLD),
    );
    let fallbacks: Vec<String> = system
        .iter()
        .map(|font| add(&mut fonts, format!("system:{}", font.name), shared(font)))
        .collect();

    let chain = |first: &[String], rest: &[String]| -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for name in first.iter().chain(&fallbacks).chain(rest) {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    };
    let base: Vec<String> = custom.iter().cloned().chain([prompt.clone()]).collect();
    fonts
        .families
        .insert(FontFamily::Proportional, chain(&base, &egui_proportional));
    // Hack (egui's monospace) first for code, then the UI fonts for other scripts.
    let mono_first: Vec<String> = egui_monospace
        .iter()
        .take(1)
        .cloned()
        .chain(base.iter().cloned())
        .collect();
    fonts
        .families
        .insert(FontFamily::Monospace, chain(&mono_first, &egui_monospace));
    let bold_first: Vec<String> = custom.into_iter().chain([semibold, prompt]).collect();
    fonts.families.insert(
        FontFamily::Name(BOLD.into()),
        chain(&bold_first, &egui_proportional),
    );
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    // Icons in headings and window titles too.
    if let Some(bold) = fonts.families.get_mut(&FontFamily::Name(BOLD.into())) {
        bold.insert(1.min(bold.len()), "phosphor".into());
    }
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled() -> LoadedFont {
        let path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ab-ui/fonts/Prompt-Regular.ttf"
        ));
        load_font(path).unwrap()
    }

    #[test]
    fn fonts_are_checked_before_use() {
        let font = bundled();
        assert!(covers(&font, "Save บันทึก"));
        assert!(!covers(&font, "保存"), "Prompt has no Chinese");
        let dir = std::env::temp_dir().join(format!("ab-fonts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake.ttf");
        std::fs::write(&fake, b"not a font at all").unwrap();
        assert!(load_font(&fake).unwrap_err().contains("not a font"));
        assert!(load_font(&dir.join("missing.ttf")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_chains_put_the_chosen_font_first_and_keep_fallbacks() {
        let mut custom = bundled();
        custom.name = "Mine".into();
        let fonts = definitions(Some(&custom), &[]);
        // Phosphor's icons slot in after the first font; they draw no letters.
        let without_icons = |family: FontFamily| -> Vec<String> {
            fonts.families[&family]
                .iter()
                .filter(|n| *n != "phosphor")
                .cloned()
                .collect()
        };
        let proportional = &without_icons(FontFamily::Proportional);
        assert_eq!(proportional[0], "custom:Mine");
        assert_eq!(proportional[1], "prompt");
        assert!(
            proportional.len() > 3,
            "egui's fonts stay as the last resort"
        );
        let mono = &without_icons(FontFamily::Monospace);
        assert_eq!(mono[0], "Hack");
        assert!(mono.contains(&"prompt".to_owned()), "Thai in the console");
        let bold = &without_icons(FontFamily::Name(BOLD.into()));
        assert_eq!(
            bold[..2],
            ["custom:Mine".to_owned(), "prompt-semibold".to_owned()]
        );
        // Without a custom font: Prompt leads.
        let plain = definitions(None, &[]);
        assert_eq!(plain.families[&FontFamily::Proportional][0], "prompt");
    }

    #[test]
    fn every_editor_font_renders_thai_and_falls_back_for_the_rest() {
        let ctx = eframe::egui::Context::default();
        ctx.set_fonts(definitions(None, system_fallbacks()));
        let mut output = ctx.run_ui(Default::default(), |ui| {
            ui.label("บันทึก Save");
            ui.heading("หัวข้อ");
            ui.monospace("ภาษาไทย");
        });
        output.textures_delta.clear();
    }
}
