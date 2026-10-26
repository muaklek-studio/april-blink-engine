//! The editor's languages: English and Thai built in, more from `locales/*.toml` in the
//! editor's settings folder (no rebuild needed to add one). Texts are looked up with [`t`]
//! and [`tf`]; a text missing in the chosen language comes from English (see
//! [`ab_locale`]). The language can change at any time: every frame reads the texts anew.

use ab_locale::Catalog;
use std::fmt::Display;
use std::sync::RwLock;

const BUILT_IN: &[&str] = &[
    include_str!("../locales/en.toml"),
    include_str!("../locales/th.toml"),
];

static CATALOG: RwLock<Option<Catalog>> = RwLock::new(None);

fn build() -> Catalog {
    let mut catalog = Catalog::new();
    for text in BUILT_IN {
        if let Err(err) = catalog.add_toml(text) {
            // Checked by a test: only a broken build gets here.
            tracing::error!("built-in language file: {err}");
        }
    }
    let user = crate::settings::EditorSettings::store()
        .dir()
        .join("locales");
    if user.is_dir() {
        match catalog.add_dir(&user) {
            Ok(count) => tracing::info!(count, dir = %user.display(), "extra editor languages"),
            Err(err) => tracing::warn!("extra editor languages not loaded: {err}"),
        }
    }
    catalog
}

fn with<R>(read: impl FnOnce(&Catalog) -> R) -> R {
    {
        let guard = CATALOG.read().unwrap_or_else(|e| e.into_inner());
        if let Some(catalog) = guard.as_ref() {
            return read(catalog);
        }
    }
    let mut guard = CATALOG.write().unwrap_or_else(|e| e.into_inner());
    read(guard.get_or_insert_with(build))
}

/// A field or section name of the panels in the editor's language: looked up as
/// `field.<its English text, lowercased, other characters as _>`, the text itself when
/// there is no translation (or it is not English to start with).
pub fn field(text: &str) -> String {
    match field_key(text) {
        Some(key) => {
            with(|catalog| catalog.get(&key).map(str::to_owned)).unwrap_or_else(|| text.to_owned())
        }
        None => text.to_owned(),
    }
}

/// The key [`field`] looks `text` up by, if it has English letters to make one of.
fn field_key(text: &str) -> Option<String> {
    let mut key = String::from("field.");
    let mut gap = false;
    for c in text.chars() {
        // Degrees count: `Max (°)` is not `Max`.
        if c == '°' {
            if !key.ends_with('.') {
                key.push('_');
            }
            key.push_str("deg");
            gap = true;
            continue;
        }
        if c.is_ascii_alphanumeric() {
            if gap && !key.ends_with('.') {
                key.push('_');
            }
            key.push(c.to_ascii_lowercase());
            gap = false;
        } else {
            gap = true;
        }
    }
    // Text in another script makes no key: it is shown as it is.
    let ascii = text.chars().all(|c| c.is_ascii() || c == '°' || c == '→');
    (key.len() > "field.".len() && ascii).then_some(key)
}

/// Switches the editor to `code`; an unknown code keeps the current language.
pub fn set_language(code: &str) -> bool {
    with(|_| ());
    let mut guard = CATALOG.write().unwrap_or_else(|e| e.into_inner());
    guard
        .as_mut()
        .is_some_and(|catalog| catalog.set_language(code).is_ok())
}

/// (code, name) of every language.
pub fn languages() -> Vec<(String, String)> {
    with(|catalog| {
        catalog
            .languages()
            .map(|(code, name)| (code.to_owned(), name.to_owned()))
            .collect()
    })
}

/// The text for `key` in the current language.
pub fn t(key: &str) -> String {
    with(|catalog| catalog.text(key).into_owned())
}

/// [`t`] with `{name}` placeholders filled.
pub fn tf(key: &str, args: &[(&str, &dyn Display)]) -> String {
    with(|catalog| catalog.format(key, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built_in() -> Catalog {
        let mut catalog = Catalog::new();
        for text in BUILT_IN {
            catalog.add_toml(text).unwrap();
        }
        catalog
    }

    #[test]
    fn built_in_languages_are_complete_and_consistent() {
        let catalog = built_in();
        assert_eq!(catalog.language(), "en");
        for (code, _) in catalog.languages() {
            assert!(
                catalog.placeholder_mismatches(code).is_empty(),
                "{code}: {:?}",
                catalog.placeholder_mismatches(code)
            );
            assert!(
                catalog.untranslated(code).is_empty(),
                "{code} lacks {:?}",
                catalog.untranslated(code)
            );
        }
    }

    #[test]
    fn field_names_are_found_by_their_english_text() {
        let mut catalog = built_in();
        catalog.set_language("th").unwrap();
        let look = |text: &str| {
            field_key(text)
                .and_then(|k| catalog.get(&k).map(str::to_owned))
                .unwrap_or_else(|| text.to_owned())
        };
        assert_eq!(look("Field of view"), "มุมมองภาพ");
        assert_eq!(look("Max (°)"), "สูงสุด (°)");
        assert_eq!(look("Max"), "สูงสุด");
        assert_eq!(look("Size start → end"), "ขนาดเริ่ม → จบ");
        // Unknown, or not English: as it is.
        assert_eq!(look("Something new"), "Something new");
        assert_eq!(field_key("ระดับ"), None);
    }

    #[test]
    fn every_command_has_a_text() {
        let catalog = built_in();
        for binding in crate::commands::BINDINGS {
            let key = binding.command.text_key();
            assert!(catalog.get(&key).is_some(), "{key}");
        }
    }

    #[test]
    fn thai_renders_with_the_editor_fonts() {
        let catalog = {
            let mut c = built_in();
            c.set_language("th").unwrap();
            c
        };
        let font = crate::fonts::load_font(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ab-ui/fonts/Prompt-Regular.ttf"
        )))
        .unwrap();
        let language = catalog.get_language("th").unwrap();
        for key in language.keys() {
            let text = language.get(key).unwrap();
            let letters: String = text
                .chars()
                .filter(|c| !"{}…·→←↑↓—–".contains(*c))
                .collect();
            assert!(crate::fonts::covers(&font, &letters), "{key}: {text}");
        }
    }
}
