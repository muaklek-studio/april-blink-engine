//! In-game translations: language files in the assets, the current language, and UI texts
//! that follow it.
//!
//! `game.toml` lists the languages; each is `locales/<code>.toml` in the assets (the
//! format of [`ab_locale`]). The first is the base the others fall back to:
//!
//! ```toml
//! [localization]
//! languages = ["en", "th"]
//! default_language = "th"   # optional: the language at start (default: the first)
//! fonts = ["fonts/NotoSansJP-Regular.otf"]  # optional: for scripts Prompt lacks
//! ```
//!
//! The built-in font (Prompt) draws Latin and Thai. Characters it lacks (Chinese, Japanese,
//! Korean, Arabic…) come from `fonts`, tried in order, so a Japanese translation needs a
//! Japanese font in the assets.
//!
//! A `Label` or `Button` with a text key (`label = { key = "hud.score", text = "Score" }`
//! in a scene file, or `TextKey` from code) shows the key's text in the current language
//! and changes with it at once; its `text` is only what shows where no translation is
//! loaded (in the editor without the project's languages, say). Scripts read texts and
//! switch languages through the SDK (`Tree::text`, `Tree::set_language`).

use ab_asset::AssetServer;
use ab_ecs::World;
use ab_locale::Catalog;
use serde::{Deserialize, Serialize};
use std::fmt::Display;

/// `[localization]` of `game.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LocalizationSettings {
    /// Language codes; each is `<folder>/<code>.toml` in the assets. The first is the base.
    pub languages: Vec<String>,
    /// The language at start (default: the first).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_language: Option<String>,
    /// Asset folder of the language files.
    pub folder: String,
    /// Font files (asset paths) for the characters the UI's fonts lack, tried in order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fonts: Vec<String>,
}

impl Default for LocalizationSettings {
    fn default() -> Self {
        Self {
            languages: Vec::new(),
            default_language: None,
            folder: "locales".into(),
            fonts: Vec::new(),
        }
    }
}

/// The game's languages (a resource). Absent when the game has none.
#[derive(Debug, Default)]
pub struct Localization {
    pub catalog: Catalog,
    /// Bumped on every language change, so texts update once.
    revision: u64,
}

impl Localization {
    pub fn new(catalog: Catalog) -> Self {
        Self {
            catalog,
            revision: 1,
        }
    }

    /// Switches language; UI texts with a key follow on the next frame.
    pub fn set_language(&mut self, code: &str) -> Result<(), ab_locale::LocaleError> {
        self.catalog.set_language(code)?;
        self.revision += 1;
        Ok(())
    }

    pub fn language(&self) -> &str {
        self.catalog.language()
    }

    pub fn text(&self, key: &str) -> String {
        self.catalog.text(key).into_owned()
    }

    pub fn format(&self, key: &str, args: &[(&str, &dyn Display)]) -> String {
        self.catalog.format(key, args)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// The translation key of a `Label` or `Button` (a component next to it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextKey {
    pub key: String,
    /// The text written in the scene, saved back instead of the translation.
    pub fallback: String,
    /// The language revision this text was last set for.
    applied: u64,
}

impl TextKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self::with_fallback(key, "")
    }

    pub fn with_fallback(key: impl Into<String>, fallback: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            fallback: fallback.into(),
            applied: 0,
        }
    }
}

/// Gives `node` the key `key` (its text written as `fallback`), or takes its key away.
/// Its text follows the language from the next frame.
pub fn set_text_key(
    tree: &mut ab_scene::SceneTree,
    node: ab_scene::NodeHandle,
    key: Option<&str>,
    fallback: &str,
) {
    match key.filter(|k| !k.trim().is_empty()) {
        Some(key) => {
            let _ = tree.insert_component(node, TextKey::with_fallback(key.trim(), fallback));
        }
        None => {
            tree.world_mut().remove::<TextKey>(node.entity());
        }
    }
}

/// Reads the languages `settings` lists. A missing or broken file is an error naming it;
/// nothing is half loaded.
pub fn load(assets: &AssetServer, settings: &LocalizationSettings) -> Result<Localization, String> {
    let mut catalog = Catalog::new();
    for code in &settings.languages {
        let path = format!("{}/{code}.toml", settings.folder.trim_end_matches('/'));
        let bytes = assets.read(&path).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&bytes);
        let language = catalog
            .add_toml(&text)
            .map_err(|e| format!("{path}: {e}"))?;
        if !language.code.eq_ignore_ascii_case(code) {
            return Err(format!(
                "{path} says it is `{}`, but game.toml lists it as `{code}`",
                language.code
            ));
        }
    }
    if let Some(code) = &settings.default_language {
        catalog
            .set_language(code)
            .map_err(|_| format!("default_language `{code}` is not one of `languages`"))?;
    }
    Ok(Localization::new(catalog))
}

/// Loads the languages into `world` (replacing earlier ones). Without languages, removes
/// them. Returns the problem, if any; the game then runs with the texts written in its
/// scenes.
pub fn install(world: &mut World, settings: &LocalizationSettings) -> Result<(), String> {
    install_fonts(world, &settings.fonts)?;
    if settings.languages.is_empty() {
        world.remove_resource::<Localization>();
        return Ok(());
    }
    let loaded = world
        .resource::<AssetServer>()
        .ok_or_else(|| "the AssetServer resource is missing".to_owned())
        .and_then(|assets| load(&assets, settings));
    match loaded {
        Ok(localization) => {
            tracing::info!(
                languages = ?settings.languages,
                current = localization.language(),
                "languages loaded"
            );
            world.insert_resource(localization);
            Ok(())
        }
        Err(err) => {
            world.remove_resource::<Localization>();
            Err(err)
        }
    }
}

/// Adds `fonts` (asset paths) to the UI's fonts as fallbacks, in order.
fn install_fonts(world: &mut World, fonts: &[String]) -> Result<(), String> {
    if fonts.is_empty() {
        return Ok(());
    }
    let mut loaded = Vec::with_capacity(fonts.len());
    {
        let assets = world
            .resource::<AssetServer>()
            .ok_or_else(|| "the AssetServer resource is missing".to_owned())?;
        for path in fonts {
            loaded.push((path, assets.read(path).map_err(|e| e.to_string())?));
        }
    }
    let mut ui_fonts = world
        .resource_mut::<ab_ui::Fonts>()
        .ok_or_else(|| "the UI's Fonts resource is missing".to_owned())?;
    let mut ids = Vec::with_capacity(loaded.len());
    for (path, bytes) in loaded {
        ids.push(ui_fonts.add(bytes).map_err(|e| format!("{path}: {e}"))?);
    }
    ui_fonts.set_fallbacks(ids);
    Ok(())
}

/// Puts the current language's text in every keyed label and button (each once per
/// language change, or when its key is new).
pub fn localize_ui_system(world: &mut World) {
    let Some(localization) = world.resource::<Localization>() else {
        return;
    };
    let revision = localization.revision;
    for (label, key) in world.query::<(&mut ab_ui::Label, &mut TextKey)>().iter() {
        if key.applied != revision {
            key.applied = revision;
            let text = localization.text(&key.key);
            if label.text != text {
                label.text = text;
            }
        }
    }
    for (button, key) in world.query::<(&mut ab_ui::Button, &mut TextKey)>().iter() {
        if key.applied != revision {
            key.applied = revision;
            let text = localization.text(&key.key);
            if button.text != text {
                button.text = text;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, EngineConfig};
    use ab_ui::UiNodes as _;

    fn engine_with(name: &str, files: &[(&str, &str)]) -> Engine {
        let dir =
            std::env::temp_dir().join(format!("ab-localization-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let file = dir.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        Engine::new(&EngineConfig {
            asset_root: dir,
            audio_output: ab_audio::AudioOutput::Silent,
            ..Default::default()
        })
    }

    const EN: &str = "[language]\ncode = \"en\"\nname = \"English\"\n[hud]\nscore = \"Score\"\nplay = \"Play\"\n";
    const TH: &str = "[language]\ncode = \"th\"\nname = \"ไทย\"\n[hud]\nscore = \"คะแนน\"\n";

    fn settings(languages: &[&str]) -> LocalizationSettings {
        LocalizationSettings {
            languages: languages.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn keyed_texts_follow_the_language() {
        let mut engine = engine_with(
            "follow",
            &[("locales/en.toml", EN), ("locales/th.toml", TH)],
        );
        let tree = engine.tree_mut();
        install(tree.world_mut(), &settings(&["en", "th"])).unwrap();
        let label = tree.create_label("Score", ab_ui::Control::new(), ab_ui::Label::new("?"));
        tree.insert_component(label, TextKey::new("hud.score"))
            .unwrap();
        let button = tree.create_button("Play", ab_ui::Control::new(), ab_ui::Button::new("?"));
        tree.insert_component(button, TextKey::new("hud.play"))
            .unwrap();
        let root = tree.root();
        tree.add_child(root, label).unwrap();
        tree.add_child(root, button).unwrap();

        engine.update(1.0 / 60.0);
        let text = |engine: &Engine| {
            let tree = engine.tree();
            (
                tree.component::<ab_ui::Label>(label).unwrap().text.clone(),
                tree.component::<ab_ui::Button>(button)
                    .unwrap()
                    .text
                    .clone(),
            )
        };
        assert_eq!(text(&engine), ("Score".into(), "Play".into()));
        engine
            .tree()
            .world()
            .resource_mut::<Localization>()
            .unwrap()
            .set_language("th")
            .unwrap();
        engine.update(1.0 / 60.0);
        // Not translated: English.
        assert_eq!(text(&engine), ("คะแนน".into(), "Play".into()));
    }

    #[test]
    fn the_new_project_template_has_working_languages() {
        let template =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ab-project/template");
        let text = std::fs::read_to_string(template.join("game.toml"))
            .unwrap()
            .replace("{{title}}", "Test");
        let settings = crate::game::GameSettings::from_toml(&text).unwrap();
        // Its config parses and names the start language.
        let config =
            std::fs::read_to_string(template.join(settings.config.as_deref().unwrap())).unwrap();
        let config = crate::game_config::GameConfig::from_toml(&config).unwrap();
        assert_eq!(config.text("Game.DefaultLanguage").unwrap(), "en");
        let mut engine = Engine::new(&EngineConfig {
            asset_root: template.join("assets"),
            audio_output: ab_audio::AudioOutput::Silent,
            ..Default::default()
        });
        let world = engine.tree_mut().world_mut();
        install(world, &settings.localization).unwrap();
        let mut localization = world.resource_mut::<Localization>().unwrap();
        assert_eq!(localization.text("menu.play"), "Play");
        localization.set_language("th").unwrap();
        assert_eq!(localization.text("menu.play"), "เล่น");
        for (code, _) in localization.catalog.languages() {
            assert!(localization.catalog.untranslated(code).is_empty(), "{code}");
        }
    }

    #[test]
    fn fallback_fonts_join_the_ui_fonts() {
        let font = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ab-ui/fonts/Prompt-Regular.ttf"
        ))
        .unwrap();
        let mut engine = engine_with("fonts", &[("locales/en.toml", EN)]);
        let root =
            std::env::temp_dir().join(format!("ab-localization-fonts-{}", std::process::id()));
        std::fs::create_dir_all(root.join("fonts")).unwrap();
        std::fs::write(root.join("fonts/extra.ttf"), font).unwrap();
        let mut with_font = settings(&["en"]);
        with_font.fonts = vec!["fonts/extra.ttf".into()];
        let world = engine.tree_mut().world_mut();
        install(world, &with_font).unwrap();
        let fonts = world.resource::<ab_ui::Fonts>().unwrap();
        assert_eq!(fonts.len(), 2);
        assert_eq!(fonts.fallbacks().len(), 1);
    }

    #[test]
    fn problems_name_the_file_and_leave_no_languages() {
        let mut engine = engine_with(
            "problems",
            &[("locales/en.toml", EN), ("locales/xx.toml", TH)],
        );
        let world = engine.tree_mut().world_mut();
        let err = install(world, &settings(&["en", "fr"])).unwrap_err();
        assert!(err.contains("fr.toml"), "{err}");
        assert!(world.resource::<Localization>().is_none());
        let err = install(world, &settings(&["en", "xx"])).unwrap_err();
        assert!(err.contains("`th`") && err.contains("`xx`"), "{err}");
        let mut wrong_default = settings(&["en"]);
        wrong_default.default_language = Some("de".into());
        assert!(install(world, &wrong_default).unwrap_err().contains("de"));
        install(world, &settings(&[])).unwrap();
        assert!(world.resource::<Localization>().is_none());
        // A font that is not there, or not a font.
        let mut fonts = settings(&[]);
        fonts.fonts = vec!["fonts/missing.ttf".into()];
        assert!(install(world, &fonts).unwrap_err().contains("missing.ttf"));
        fonts.fonts = vec!["locales/en.toml".into()];
        assert!(install(world, &fonts).unwrap_err().contains("en.toml"));
    }
}
