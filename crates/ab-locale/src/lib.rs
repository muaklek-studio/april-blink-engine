//! Translations: language files, the current language, fallbacks and placeholders.
//!
//! A language file is TOML. `[language]` names it; every other key is a translated text,
//! in tables or dotted keys as you like (`[menu] file = "File"` and `"menu.file" = "File"`
//! are the same key `menu.file`):
//!
//! ```toml
//! [language]
//! code = "th"          # what `set_language` and settings use
//! name = "ไทย"          # shown in language pickers, in the language itself
//! fallback = "en"      # where missing texts come from (default: the catalog's base)
//!
//! [menu]
//! file = "ไฟล์"
//! saved = "บันทึก {path} แล้ว"   # {name} placeholders; {{ and }} for braces
//! ```
//!
//! Looking a text up tries the current language, its fallback chain, then the base
//! language (the first one added, normally English); a key found nowhere is returned as
//! the key itself, so a missing text shows up instead of vanishing. Adding a language is
//! adding a file.
//!
//! ```
//! let mut catalog = ab_locale::Catalog::new();
//! catalog.add_toml("[language]\ncode = \"en\"\nname = \"English\"\n[menu]\nsave = \"Save\"\nsaved = \"Saved {path}\"\n").unwrap();
//! catalog.add_toml("[language]\ncode = \"th\"\nname = \"ไทย\"\n[menu]\nsave = \"บันทึก\"\n").unwrap();
//! catalog.set_language("th").unwrap();
//! assert_eq!(catalog.text("menu.save"), "บันทึก");
//! // Not translated yet: English.
//! assert_eq!(catalog.format("menu.saved", &[("path", &"a.abscene")]), "Saved a.abscene");
//! assert_eq!(catalog.text("menu.unknown"), "menu.unknown");
//! ```

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Display;
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum LocaleError {
    #[error("language file: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("language file needs [language] with `code` and `name`")]
    MissingHeader,
    #[error("`{key}` must be text (found {found})")]
    NotText { key: String, found: &'static str },
    #[error("no language `{0}`")]
    UnknownLanguage(String),
    #[error("{path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    File {
        path: String,
        source: Box<LocaleError>,
    },
}

/// One language's texts.
#[derive(Debug, Clone, PartialEq)]
pub struct Language {
    /// `en`, `th`, `ja`, `pt-BR`…
    pub code: String,
    /// The language's own name for itself (`English`, `ไทย`).
    pub name: String,
    /// Where texts this language lacks come from.
    pub fallback: Option<String>,
    texts: HashMap<String, String>,
}

impl Language {
    /// Parses a language file (see the crate docs).
    pub fn from_toml(text: &str) -> Result<Self, LocaleError> {
        let mut table: toml::Table = text.parse()?;
        let header = match table.remove("language") {
            Some(toml::Value::Table(header)) => header,
            _ => return Err(LocaleError::MissingHeader),
        };
        let field = |key: &str| {
            header
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let (Some(code), Some(name)) = (field("code"), field("name")) else {
            return Err(LocaleError::MissingHeader);
        };
        let mut texts = HashMap::new();
        flatten("", &toml::Value::Table(table), &mut texts)?;
        Ok(Self {
            code,
            name,
            fallback: field("fallback"),
            texts,
        })
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.texts.get(key).map(String::as_str)
    }

    /// Every key, sorted.
    pub fn keys(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self.texts.keys().map(String::as_str).collect();
        keys.sort_unstable();
        keys
    }

    pub fn len(&self) -> usize {
        self.texts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }
}

fn flatten(
    prefix: &str,
    value: &toml::Value,
    out: &mut HashMap<String, String>,
) -> Result<(), LocaleError> {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                let key = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(&key, value, out)?;
            }
            Ok(())
        }
        toml::Value::String(text) => {
            out.insert(prefix.to_owned(), text.clone());
            Ok(())
        }
        other => Err(LocaleError::NotText {
            key: prefix.to_owned(),
            found: other.type_str(),
        }),
    }
}

/// Every language, and which one is current.
#[derive(Debug, Default)]
pub struct Catalog {
    languages: Vec<Language>,
    current: usize,
    /// Keys already reported missing, so the log says it once.
    reported: Mutex<HashSet<String>>,
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a language file. The first language added is the base: every other language
    /// falls back to it. Adding a code again replaces that language.
    pub fn add_toml(&mut self, text: &str) -> Result<&Language, LocaleError> {
        let language = Language::from_toml(text)?;
        Ok(self.add(language))
    }

    pub fn add(&mut self, language: Language) -> &Language {
        let index = match self.index(&language.code) {
            Some(index) => {
                self.languages[index] = language;
                index
            }
            None => {
                self.languages.push(language);
                self.languages.len() - 1
            }
        };
        &self.languages[index]
    }

    /// Adds every `*.toml` file of `dir` (sorted by name, so the order is the same on every
    /// machine), after the languages already added. Returns how many were added.
    pub fn add_dir(&mut self, dir: &Path) -> Result<usize, LocaleError> {
        let io = |source| LocaleError::Io {
            path: dir.display().to_string(),
            source,
        };
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .map_err(io)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        files.sort();
        for path in &files {
            let text = std::fs::read_to_string(path).map_err(|source| LocaleError::Io {
                path: path.display().to_string(),
                source,
            })?;
            self.add_toml(&text).map_err(|source| LocaleError::File {
                path: path.display().to_string(),
                source: Box::new(source),
            })?;
        }
        Ok(files.len())
    }

    fn index(&self, code: &str) -> Option<usize> {
        self.languages
            .iter()
            .position(|l| l.code.eq_ignore_ascii_case(code))
    }

    /// Makes `code` the current language. An unknown code changes nothing.
    pub fn set_language(&mut self, code: &str) -> Result<(), LocaleError> {
        self.current = self
            .index(code)
            .ok_or_else(|| LocaleError::UnknownLanguage(code.to_owned()))?;
        Ok(())
    }

    /// The current language's code (empty for an empty catalog).
    pub fn language(&self) -> &str {
        self.languages
            .get(self.current)
            .map_or("", |l| l.code.as_str())
    }

    pub fn has_language(&self, code: &str) -> bool {
        self.index(code).is_some()
    }

    /// (code, name) of every language, in the order they were added.
    pub fn languages(&self) -> impl Iterator<Item = (&str, &str)> {
        self.languages
            .iter()
            .map(|l| (l.code.as_str(), l.name.as_str()))
    }

    pub fn get_language(&self, code: &str) -> Option<&Language> {
        self.index(code).map(|i| &self.languages[i])
    }

    /// The languages a lookup tries: the current one, its fallbacks, then the base.
    fn chain(&self) -> Vec<usize> {
        let mut chain = Vec::new();
        let mut next = Some(self.current).filter(|&i| i < self.languages.len());
        while let Some(index) = next {
            if chain.contains(&index) {
                break; // a fallback cycle
            }
            chain.push(index);
            next = self.languages[index]
                .fallback
                .as_deref()
                .and_then(|code| self.index(code));
        }
        if !self.languages.is_empty() && !chain.contains(&0) {
            chain.push(0);
        }
        chain
    }

    /// The text for `key`, if any language of the chain has it.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.chain()
            .into_iter()
            .find_map(|i| self.languages[i].get(key))
    }

    /// The text for `key`, or the key itself when no language has it (reported once).
    pub fn text<'a>(&'a self, key: &'a str) -> Cow<'a, str> {
        match self.get(key) {
            Some(text) => Cow::Borrowed(text),
            None => {
                self.report_missing(key);
                Cow::Borrowed(key)
            }
        }
    }

    /// [`Catalog::text`] with `{name}` placeholders filled from `args`. A placeholder
    /// without a value stays as written.
    pub fn format(&self, key: &str, args: &[(&str, &dyn Display)]) -> String {
        fill(&self.text(key), args)
    }

    fn report_missing(&self, key: &str) {
        let mut reported = self.reported.lock().unwrap_or_else(|e| e.into_inner());
        if reported.insert(key.to_owned()) {
            tracing::warn!(key, language = self.language(), "no translation");
        }
    }

    /// Keys the base language has and `code` (with its fallbacks) lacks, sorted: what is
    /// left to translate.
    pub fn untranslated(&self, code: &str) -> Vec<String> {
        let (Some(base), Some(language)) = (self.languages.first(), self.get_language(code)) else {
            return Vec::new();
        };
        base.keys()
            .into_iter()
            .filter(|key| language.get(key).is_none())
            .map(str::to_owned)
            .collect()
    }

    /// Placeholders used differently by the base language and `code`, per key: a
    /// translation that drops or renames `{path}` would show the wrong text.
    pub fn placeholder_mismatches(
        &self,
        code: &str,
    ) -> BTreeMap<String, (Vec<String>, Vec<String>)> {
        let mut found = BTreeMap::new();
        let (Some(base), Some(language)) = (self.languages.first(), self.get_language(code)) else {
            return found;
        };
        for key in base.keys() {
            if let (Some(a), Some(b)) = (base.get(key), language.get(key)) {
                let (a, b) = (placeholders(a), placeholders(b));
                if a != b {
                    found.insert(key.to_owned(), (a, b));
                }
            }
        }
        found
    }
}

/// The `{name}` placeholders of `text`, sorted.
pub fn placeholders(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        if rest[start..].starts_with("{{") {
            rest = &rest[start + 2..];
            continue;
        }
        let Some(len) = rest[start + 1..].find('}') else {
            break;
        };
        names.push(rest[start + 1..start + 1 + len].to_owned());
        rest = &rest[start + 2 + len..];
    }
    names.sort();
    names.dedup();
    names
}

/// Fills `{name}` placeholders; `{{` and `}}` are literal braces.
pub fn fill(text: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '{' if chars.peek().map(|&(_, n)| n) == Some('{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek().map(|&(_, n)| n) == Some('}') => {
                chars.next();
                out.push('}');
            }
            '{' => match text[i + 1..].find('}') {
                Some(len) => {
                    let name = &text[i + 1..i + 1 + len];
                    match args.iter().find(|(n, _)| *n == name) {
                        Some((_, value)) => out.push_str(&value.to_string()),
                        None => out.push_str(&text[i..i + 2 + len]),
                    }
                    for _ in 0..name.chars().count() + 1 {
                        chars.next();
                    }
                }
                None => out.push('{'),
            },
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const EN: &str = r#"
[language]
code = "en"
name = "English"

[menu]
file = "File"
save = "Save"
saved = "Saved {path}"

[count]
nodes = "{n} nodes"
"#;

    const TH: &str = r#"
[language]
code = "th"
name = "ไทย"

[menu]
file = "ไฟล์"
saved = "บันทึก {file} แล้ว"
"#;

    fn catalog() -> Catalog {
        let mut catalog = Catalog::new();
        catalog.add_toml(EN).unwrap();
        catalog.add_toml(TH).unwrap();
        catalog
    }

    #[test]
    fn lookups_fall_back_to_the_base_then_the_key() {
        let mut catalog = catalog();
        assert_eq!(catalog.language(), "en");
        assert_eq!(catalog.text("menu.file"), "File");
        catalog.set_language("TH").unwrap();
        assert_eq!(catalog.language(), "th");
        assert_eq!(catalog.text("menu.file"), "ไฟล์");
        assert_eq!(catalog.text("menu.save"), "Save");
        assert_eq!(catalog.text("nope"), "nope");
        assert!(catalog.set_language("xx").is_err());
        assert_eq!(
            catalog.language(),
            "th",
            "an unknown language changes nothing"
        );
        assert_eq!(
            catalog.languages().collect::<Vec<_>>(),
            [("en", "English"), ("th", "ไทย")]
        );
    }

    #[test]
    fn fallback_chains() {
        let mut catalog = catalog();
        catalog
            .add_toml("[language]\ncode = \"th-x\"\nname = \"X\"\nfallback = \"th\"\n[menu]\nsave = \"เก็บ\"\n")
            .unwrap();
        catalog.set_language("th-x").unwrap();
        assert_eq!(catalog.text("menu.save"), "เก็บ");
        assert_eq!(catalog.text("menu.file"), "ไฟล์", "from its fallback");
        assert_eq!(catalog.text("count.nodes"), "{n} nodes", "from the base");
        // A cycle does not hang.
        catalog
            .add_toml("[language]\ncode = \"a\"\nname = \"A\"\nfallback = \"b\"\n")
            .unwrap();
        catalog
            .add_toml("[language]\ncode = \"b\"\nname = \"B\"\nfallback = \"a\"\n")
            .unwrap();
        catalog.set_language("a").unwrap();
        assert_eq!(catalog.text("menu.file"), "File");
    }

    #[test]
    fn placeholders_fill_and_check() {
        let mut catalog = catalog();
        assert_eq!(
            catalog.format("menu.saved", &[("path", &"a.abscene")]),
            "Saved a.abscene"
        );
        assert_eq!(catalog.format("count.nodes", &[("n", &3)]), "3 nodes");
        catalog.set_language("th").unwrap();
        // The Thai text names the wrong placeholder: left visible, and reported.
        assert_eq!(
            catalog.format("menu.saved", &[("path", &"a")]),
            "บันทึก {file} แล้ว"
        );
        let mismatches = catalog.placeholder_mismatches("th");
        assert_eq!(
            mismatches["menu.saved"],
            (vec!["path".to_owned()], vec!["file".to_owned()])
        );
        assert_eq!(catalog.untranslated("th"), ["count.nodes", "menu.save"]);
        assert_eq!(fill("{{x}} {y}", &[("y", &1)]), "{x} 1");
        assert_eq!(fill("ไทย {a}!", &[("a", &"ok")]), "ไทย ok!");
        assert_eq!(fill("open {", &[]), "open {");
        assert_eq!(placeholders("{a} {{b}} {c} {a}"), ["a", "c"]);
    }

    #[test]
    fn bad_files_are_refused_with_a_reason() {
        let err = Language::from_toml("[menu]\nfile = \"x\"\n").unwrap_err();
        assert!(matches!(err, LocaleError::MissingHeader));
        let err = Language::from_toml("[language]\ncode = \"x\"\nname = \"X\"\n[a]\nb = 3\n")
            .unwrap_err();
        assert_eq!(err.to_string(), "`a.b` must be text (found integer)");
        assert!(Language::from_toml("not toml =").is_err());
    }

    #[test]
    fn languages_load_from_a_folder_and_replace_by_code() {
        let dir = std::env::temp_dir().join(format!("ab-locale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("en.toml"), EN).unwrap();
        std::fs::write(dir.join("th.toml"), TH).unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let mut catalog = Catalog::new();
        assert_eq!(catalog.add_dir(&dir).unwrap(), 2);
        assert!(catalog.has_language("th"));
        catalog
            .add_toml("[language]\ncode = \"en\"\nname = \"English\"\n[menu]\nfile = \"Files\"\n")
            .unwrap();
        assert_eq!(catalog.text("menu.file"), "Files");
        std::fs::write(dir.join("zz.toml"), "[language]\n").unwrap();
        let err = catalog.add_dir(&dir).unwrap_err().to_string();
        assert!(err.contains("zz.toml"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
