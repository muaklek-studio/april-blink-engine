//! The editor's own preferences, kept between runs with the recent projects (`SaveStore`
//! "April Blink Editor", slot `settings`): the engine sources folder, language, fonts and
//! interface scale, selection highlighting and snapping.
//!
//! Missing fields take their defaults and an unreadable file is replaced by the defaults, so
//! an older or damaged settings file never stops the editor.

use crate::projects::RecentProjects;
use ab_engine::save::SaveStore;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const SLOT: &str = "settings";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    /// The last engine checkout found or chosen (see [`ab_project::locate_engine`]).
    pub engine_root: Option<PathBuf>,
    /// Outlines around selected objects in the viewport.
    pub highlight_selection: bool,
    /// Outlines around objects under the pointer (and drop targets).
    pub highlight_hover: bool,
    pub snap: SnapSettings,
    /// The editor's language code (`en`, `th`, …).
    pub language: String,
    /// A font file for the interface (`None`: the built-in Prompt).
    pub ui_font: Option<PathBuf>,
    /// Fonts of the system draw the scripts the interface font lacks.
    pub system_fonts: bool,
    /// Interface zoom, 1 = 100 %.
    pub ui_scale: f32,
    /// The tool windows open when the editor last closed (`animation`, `ui_designer`, …),
    /// opened again with the next project.
    pub windows: Vec<String>,
    /// Where each tool window was last: outer position and inner size, in points.
    pub window_rects: std::collections::BTreeMap<String, [f32; 4]>,
    /// Scenes opened or saved lately, newest first, by project folder.
    pub recent_scenes: std::collections::BTreeMap<String, Vec<String>>,
}

/// Recent scenes kept per project.
const RECENT_SCENES: usize = 12;

/// The interface zoom a user can pick.
pub const UI_SCALES: std::ops::RangeInclusive<f32> = 0.75..=2.0;

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            engine_root: None,
            highlight_selection: true,
            highlight_hover: true,
            snap: SnapSettings::default(),
            language: "en".into(),
            ui_font: None,
            system_fonts: true,
            ui_scale: 1.0,
            windows: Vec::new(),
            window_rects: Default::default(),
            recent_scenes: Default::default(),
        }
    }
}

/// Snapping steps of the transform gizmo and the UI editor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapSettings {
    /// Snapping on without holding Ctrl.
    pub enabled: bool,
    /// Metres.
    pub translate: f32,
    /// Degrees.
    pub rotate: f32,
    /// Scale factor step.
    pub scale: f32,
}

impl Default for SnapSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            translate: 0.5,
            rotate: 15.0,
            scale: 0.1,
        }
    }
}

impl SnapSettings {
    /// Steps a user can set: positive and not absurdly small.
    pub fn sanitized(mut self) -> Self {
        let default = Self::default();
        let valid = |v: f32, min: f32| v.is_finite() && v >= min;
        if !valid(self.translate, 0.001) {
            self.translate = default.translate;
        }
        if !valid(self.rotate, 0.01) {
            self.rotate = default.rotate;
        }
        if !valid(self.scale, 0.001) {
            self.scale = default.scale;
        }
        self
    }

    pub fn translate(&self, value: f32) -> f32 {
        snap(value, self.translate)
    }

    /// Degrees in, degrees out.
    pub fn rotate(&self, degrees: f32) -> f32 {
        snap(degrees, self.rotate)
    }

    /// A scale value, never below one step.
    pub fn scale(&self, value: f32) -> f32 {
        snap(value, self.scale).max(self.scale)
    }
}

/// `value` rounded to the nearest multiple of `step`.
pub fn snap(value: f32, step: f32) -> f32 {
    if step > 0.0 {
        (value / step).round() * step
    } else {
        value
    }
}

impl EditorSettings {
    pub fn store() -> SaveStore {
        RecentProjects::store()
    }

    /// The saved settings, or the defaults when there are none or they cannot be read.
    pub fn load(store: &SaveStore) -> Self {
        match store.load::<Self>(SLOT) {
            Ok(settings) => settings.unwrap_or_default(),
            Err(err) => {
                tracing::warn!("editor settings unreadable, using the defaults: {err}");
                keep_unreadable(store, SLOT);
                Self::default()
            }
        }
        .sanitized()
    }

    pub fn save(&self, store: &SaveStore) {
        if let Err(err) = store.save(SLOT, self) {
            tracing::warn!("cannot save the editor settings: {err}");
        }
    }

    /// Records `scene` (of the project at `project`) as the newest recent scene.
    pub fn add_recent_scene(&mut self, project: &std::path::Path, scene: &str) {
        let list = self
            .recent_scenes
            .entry(project.to_string_lossy().into_owned())
            .or_default();
        list.retain(|s| s != scene);
        list.insert(0, scene.to_owned());
        list.truncate(RECENT_SCENES);
    }

    /// The project's recent scenes, newest first.
    pub fn recent_scenes(&self, project: &std::path::Path) -> &[String] {
        self.recent_scenes
            .get(project.to_string_lossy().as_ref())
            .map_or(&[], Vec::as_slice)
    }

    fn sanitized(mut self) -> Self {
        self.snap = self.snap.sanitized();
        if !(self.ui_scale.is_finite() && UI_SCALES.contains(&self.ui_scale)) {
            self.ui_scale = 1.0;
        }
        if self.language.trim().is_empty() {
            self.language = "en".into();
        }
        self
    }
}

/// Keeps a copy of slot `slot` that could not be read (`<slot>.unreadable.json`) before the
/// defaults replace it, so a hand edit gone wrong never costs every other setting.
pub(crate) fn keep_unreadable(store: &SaveStore, slot: &str) {
    let path = store.dir().join(format!("{slot}.json"));
    let copy = store.dir().join(format!("{slot}.unreadable.json"));
    if path.is_file() {
        match std::fs::copy(&path, &copy) {
            Ok(_) => tracing::warn!("the unreadable file was kept as {}", copy.display()),
            Err(err) => tracing::warn!("cannot keep a copy of {}: {err}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults_for_missing_fields() {
        let dir = std::env::temp_dir().join(format!("ab-editor-settings-{}", std::process::id()));
        let store = SaveStore::in_dir(&dir);
        assert_eq!(EditorSettings::load(&store), EditorSettings::default());

        let settings = EditorSettings {
            highlight_selection: false,
            snap: SnapSettings {
                translate: 0.25,
                ..Default::default()
            },
            ..Default::default()
        };
        settings.save(&store);
        assert_eq!(EditorSettings::load(&store), settings);

        // Older files lack fields; broken values fall back.
        std::fs::write(
            dir.join("settings.json"),
            r#"{ "highlight_hover": false, "snap": { "rotate": -3.0 } }"#,
        )
        .unwrap();
        let loaded = EditorSettings::load(&store);
        assert!(!loaded.highlight_hover && loaded.highlight_selection);
        assert_eq!(loaded.snap.rotate, 15.0);
        assert_eq!((loaded.language.as_str(), loaded.ui_scale), ("en", 1.0));

        std::fs::write(
            dir.join("settings.json"),
            r#"{ "ui_scale": 40.0, "language": "" }"#,
        )
        .unwrap();
        let loaded = EditorSettings::load(&store);
        assert_eq!((loaded.language.as_str(), loaded.ui_scale), ("en", 1.0));

        std::fs::write(dir.join("settings.json"), "not json").unwrap();
        assert_eq!(EditorSettings::load(&store), EditorSettings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapping() {
        let snap = SnapSettings::default();
        assert_eq!(snap.translate(1.3), 1.5);
        assert_eq!(snap.translate(-0.2), 0.0);
        assert_eq!(snap.rotate(22.0), 15.0);
        assert_eq!(snap.rotate(23.0), 30.0);
        assert!((snap.scale(0.02) - 0.1).abs() < 1e-6, "never below a step");
        assert_eq!(super::snap(3.0, 0.0), 3.0);
    }
}
