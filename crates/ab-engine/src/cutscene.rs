//! Cutscenes: a camera move and timed events, played on a timeline.
//!
//! A cutscene file (`cutscenes/intro.abcut`, TOML) has camera keys and events:
//!
//! ```toml
//! length = 8.0
//! camera_interpolation = "ease"     # step | linear | smooth | ease
//!
//! [[camera]]
//! time = 0.0
//! position = [0.0, 3.0, 10.0]
//! rotation = [-10.0, 0.0, 0.0]      # degrees, as in scene files
//! fov = 60.0
//!
//! [[event]]
//! time = 1.0
//! kind = "animation"                # play a clip of an AnimationPlayer
//! target = "Door/Animator"          # path from the cutscene node's parent
//! clip = "Open"
//!
//! [[event]]
//! time = 1.5
//! kind = "sound"
//! stream = "audio/creak.ogg"
//!
//! [[event]]
//! time = 3.0
//! kind = "subtitle"
//! text = "Who's there?"            # or `key` for a translated text
//! duration = 2.5
//!
//! [[event]]
//! time = 7.0
//! kind = "fade"                     # to black (to = 1) or back (to = 0)
//! to = 1.0
//! duration = 1.0
//! ```
//!
//! Other events: `signal` (the cutscene node emits `cutscene_event` with the event's
//! `name`, for scripts to act on) and `visible` (shows or hides `target`).
//!
//! A `Cutscene` node plays one (`cutscene = { file = "...", autoplay = true }`): while it
//! plays, its own camera is the current one; at the end the previous camera is current
//! again and the node emits `finished`. Scripts start, stop and follow it
//! (`Tree::play_cutscene`).

use crate::animation_file::{rotation_from_degrees, Track, TrackInterpolation, TrackProperty};
use crate::scene_file::{Generated, NodeDesc, SceneFile};
use ab_core::{Quat, Transform3D, Vec3};
use ab_rendering::Camera3D;
use ab_scene::{NodeHandle, SceneTree, Signal};
use serde::{Deserialize, Serialize};

/// The class of cutscene nodes.
pub const CUTSCENE: &str = "Cutscene";
/// File extension of cutscene files.
pub const CUTSCENE_EXTENSION: &str = "abcut";

/// A camera key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraKey {
    pub time: f32,
    pub position: [f32; 3],
    /// Degrees, applied Y, X, Z as in scene files.
    #[serde(default)]
    pub rotation: [f32; 3],
    #[serde(default = "default_fov")]
    pub fov: f32,
}

fn default_fov() -> f32 {
    60.0
}

/// What an event does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EventKind {
    /// Plays `clip` of the AnimationPlayer at `target` (crossfading over `crossfade`).
    Animation {
        target: String,
        clip: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crossfade: Option<f32>,
    },
    /// Plays a sound once.
    Sound {
        stream: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        volume_db: Option<f32>,
    },
    /// The cutscene node emits `cutscene_event` with `name`.
    Signal { name: String },
    /// Shows or hides `target`.
    Visible { target: String, visible: bool },
    /// Fades the screen to `to` (0 clear, 1 covered) of `color` over `duration`.
    Fade {
        to: f32,
        #[serde(default = "default_duration")]
        duration: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
    },
    /// Shows `text` (or the translation of `key`) for `duration` seconds.
    Subtitle {
        #[serde(default)]
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        #[serde(default = "default_duration")]
        duration: f32,
    },
}

fn default_duration() -> f32 {
    1.0
}

impl EventKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Animation { .. } => "animation",
            Self::Sound { .. } => "sound",
            Self::Signal { .. } => "signal",
            Self::Visible { .. } => "visible",
            Self::Fade { .. } => "fade",
            Self::Subtitle { .. } => "subtitle",
        }
    }

    /// How long it lasts on the timeline (0 for instant events).
    pub fn duration(&self) -> f32 {
        match self {
            Self::Fade { duration, .. } | Self::Subtitle { duration, .. } => duration.max(0.0),
            _ => 0.0,
        }
    }
}

/// An event at a time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub time: f32,
    #[serde(flatten)]
    pub kind: EventKind,
}

/// A cutscene file (see the module docs).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CutsceneFile {
    /// Seconds; `None`: until the last key or event ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<f32>,
    #[serde(default)]
    pub camera_interpolation: TrackInterpolation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub camera: Vec<CameraKey>,
    #[serde(default, rename = "event", skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Event>,
}

impl CutsceneFile {
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        let mut file: Self = toml::from_str(text)?;
        file.sort();
        Ok(file)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("cutscenes always serialize")
    }

    /// Keys and events in time order (events at the same time keep their order).
    pub fn sort(&mut self) {
        self.camera.sort_by(|a, b| a.time.total_cmp(&b.time));
        self.events.sort_by(|a, b| a.time.total_cmp(&b.time));
    }

    /// How long it plays.
    pub fn duration(&self) -> f32 {
        self.length.unwrap_or_else(|| {
            let keys = self.camera.last().map_or(0.0, |k| k.time);
            let events = self
                .events
                .iter()
                .map(|e| e.time + e.kind.duration())
                .fold(0.0, f32::max);
            keys.max(events)
        })
    }

    /// The camera at `time`: position, rotation, field of view. `None` without keys.
    pub fn camera_at(&self, time: f32) -> Option<(Vec3, Quat, f32)> {
        let track = |property, value: fn(&CameraKey) -> [f32; 3]| Track {
            target: ".".into(),
            property,
            interpolation: self.camera_interpolation,
            keys: self.camera.iter().map(|k| (k.time, value(k))).collect(),
        };
        let position = track(TrackProperty::Position, |k| k.position).sample(time)?;
        let rotation = track(TrackProperty::Rotation, |k| k.rotation).sample(time)?;
        let fov = track(TrackProperty::Scale, |k| [k.fov; 3]).sample(time)?[0];
        Some((Vec3::from(position), rotation_from_degrees(rotation), fov))
    }
}

/// `cutscene = { file = "...", autoplay = true }` of a `Cutscene` node.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CutsceneDesc {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autoplay: Option<bool>,
}

/// A fade in progress: from, to, start time, duration, colour (rgb).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fade {
    from: f32,
    to: f32,
    start: f32,
    duration: f32,
    color: [f32; 3],
}

/// The cutscene a node plays, and where it is (a component).
#[derive(Debug, Clone, PartialEq)]
pub struct CutscenePlayer {
    pub desc: CutsceneDesc,
    pub file: CutsceneFile,
    pub time: f32,
    pub playing: bool,
    /// The next event to fire.
    next: usize,
    /// Cameras that were current before it started, to make current again.
    previous: Vec<NodeHandle>,
    /// It has taken the screen (its camera current, the overlay shown) since it started.
    started: bool,
    fade: Option<Fade>,
    /// The covered amount now.
    cover: f32,
    subtitle_until: f32,
}

impl CutscenePlayer {
    pub fn new(desc: CutsceneDesc, file: CutsceneFile) -> Self {
        Self {
            playing: desc.autoplay.unwrap_or(false),
            desc,
            file,
            time: 0.0,
            next: 0,
            previous: Vec::new(),
            started: false,
            fade: None,
            cover: 0.0,
            subtitle_until: 0.0,
        }
    }
}

/// The overlay a cutscene draws its fades and subtitles on.
const OVERLAY: &str = r##"
[[node]]
name = "CutsceneOverlay"
type = "CanvasLayer"
canvas = { layer = 90 }

[[node]]
name = "Fade"
parent = "."
type = "Panel"
control = { preset = "full_rect", mouse = "ignore" }
panel = { color = "#00000000", corner_radius = 0.0 }

[[node]]
name = "Subtitle"
parent = "."
type = "Label"
control = { preset = "bottom_wide", size = [0.0, 140.0], margin = 48.0, mouse = "ignore" }
label = { text = "", size = 30.0, align = "center", valign = "center", outline = 2.0, autowrap = true }
"##;

/// Creates a `Cutscene` node: the file read, its camera and overlay made.
pub(crate) fn create(
    tree: &mut SceneTree,
    name: &str,
    desc: &NodeDesc,
) -> Result<NodeHandle, String> {
    let c = desc
        .cutscene
        .clone()
        .ok_or("Cutscene needs `cutscene = { file = ... }`")?;
    let file = if c.file.trim().is_empty() {
        CutsceneFile::default()
    } else {
        let bytes = tree
            .world()
            .resource::<ab_asset::AssetServer>()
            .ok_or("the AssetServer resource is missing")?
            .read(&c.file)
            .map_err(|e| format!("cutscene {}: {e}", c.file))?;
        CutsceneFile::from_toml(&String::from_utf8_lossy(&bytes))
            .map_err(|e| format!("cutscene {}: {e}", c.file))?
    };
    let node = tree.create_node3d(name);
    tree.set_class_name(node, CUTSCENE)
        .map_err(|e| e.to_string())?;
    tree.insert_component(node, CutscenePlayer::new(c, file))
        .map_err(|e| e.to_string())?;
    // Its camera (not current until it plays) and overlay: made again on load.
    use ab_rendering::RenderNodes as _;
    let camera = tree.create_camera3d(
        "CutsceneCamera",
        Camera3D {
            current: false,
            ..Default::default()
        },
    );
    tree.insert_component(camera, Generated)
        .map_err(|e| e.to_string())?;
    tree.add_child(node, camera).map_err(|e| e.to_string())?;
    let overlay = SceneFile::from_toml(OVERLAY).expect("the overlay parses");
    let overlay = crate::scene_file::instantiate_scene(tree, &overlay, "cutscene")
        .map_err(|e| e.to_string())?;
    tree.insert_component(overlay, Generated)
        .map_err(|e| e.to_string())?;
    tree.set_visible(overlay, false)
        .map_err(|e| e.to_string())?;
    tree.add_child(node, overlay).map_err(|e| e.to_string())?;
    Ok(node)
}

pub(crate) fn describe(tree: &SceneTree, node: NodeHandle, desc: &mut NodeDesc) {
    desc.cutscene = tree
        .component::<CutscenePlayer>(node)
        .map(|p| p.desc.clone());
}

/// Starts (from the beginning) the cutscene of `node`.
pub fn play(tree: &mut SceneTree, node: NodeHandle) -> bool {
    let Some(mut player) = tree.component_mut::<CutscenePlayer>(node) else {
        return false;
    };
    player.time = 0.0;
    player.next = 0;
    player.fade = None;
    player.cover = 0.0;
    player.subtitle_until = 0.0;
    player.playing = true;
    player.previous.clear();
    player.started = false;
    true
}

/// Stops it where it is: its camera hands back, the overlay goes, `finished` is emitted.
pub fn stop(tree: &mut SceneTree, node: NodeHandle) {
    let was_playing = tree
        .component::<CutscenePlayer>(node)
        .is_some_and(|p| p.playing);
    if was_playing {
        finish(tree, node);
    }
}

fn finish(tree: &mut SceneTree, node: NodeHandle) {
    let previous = match tree.component_mut::<CutscenePlayer>(node) {
        Some(mut player) => {
            player.playing = false;
            std::mem::take(&mut player.previous)
        }
        None => return,
    };
    if let Some(camera) = tree.get_node(node, "CutsceneCamera") {
        if let Some(mut c) = tree.component_mut::<Camera3D>(camera) {
            c.current = false;
        }
    }
    for camera in previous {
        if let Some(mut c) = tree.component_mut::<Camera3D>(camera) {
            c.current = true;
        }
    }
    if let Some(overlay) = tree.get_node(node, "CutsceneOverlay") {
        let _ = tree.set_visible(overlay, false);
    }
    tree.emit(Signal::new(node, "finished"));
}

/// Advances every playing cutscene by `delta` seconds (once a frame, with the scripts).
pub fn update(tree: &mut SceneTree, delta: f32) {
    let playing: Vec<NodeHandle> = tree
        .world()
        .query::<(ab_ecs::Entity, &CutscenePlayer)>()
        .iter()
        .filter(|(_, p)| p.playing)
        .map(|(entity, _)| NodeHandle::from_entity(entity))
        .collect();
    for node in playing {
        advance(tree, node, delta);
    }
}

fn advance(tree: &mut SceneTree, node: NodeHandle, delta: f32) {
    let (file, time, first) = {
        let Some(mut p) = tree.component_mut::<CutscenePlayer>(node) else {
            return;
        };
        // Not from the time: a paused frame (no time passing) must not take the screen twice
        // and forget the cameras to give it back to.
        let first = !std::mem::replace(&mut p.started, true);
        p.time += delta.max(0.0);
        (p.file.clone(), p.time, first)
    };
    let parent = tree.parent(node).unwrap_or(node);
    let camera = tree.get_node(node, "CutsceneCamera");
    let overlay = tree.get_node(node, "CutsceneOverlay");
    // Take the screen: its camera current, the others remembered.
    if first {
        let cameras: Vec<NodeHandle> = tree
            .world()
            .query::<(ab_ecs::Entity, &Camera3D)>()
            .iter()
            .filter(|(_, c)| c.current)
            .map(|(e, _)| NodeHandle::from_entity(e))
            .filter(|&n| Some(n) != camera)
            .collect();
        for &c in &cameras {
            if let Some(mut cam) = tree.component_mut::<Camera3D>(c) {
                cam.current = false;
            }
        }
        if let Some(mut p) = tree.component_mut::<CutscenePlayer>(node) {
            p.previous = cameras;
        }
        if let Some(overlay) = overlay {
            let _ = tree.set_visible(overlay, true);
        }
    }
    if let (Some(camera), Some((position, rotation, fov))) = (camera, file.camera_at(time)) {
        let world = crate::procgen::world_of(tree, node);
        let local =
            world.inverse() * ab_core::Affine3A::from_rotation_translation(rotation, position);
        let (_, r, t) = local.to_scale_rotation_translation();
        let _ = tree.set_transform(camera, Transform3D::from_trs(t, r, Vec3::ONE));
        if let Some(mut c) = tree.component_mut::<Camera3D>(camera) {
            c.current = true;
            c.fov_y_degrees = fov.clamp(1.0, 179.0);
        }
    }
    // Events up to now, in order.
    loop {
        let event = {
            let Some(p) = tree.component::<CutscenePlayer>(node) else {
                return;
            };
            match file.events.get(p.next) {
                Some(e) if e.time <= time => e.clone(),
                _ => break,
            }
        };
        if let Some(mut p) = tree.component_mut::<CutscenePlayer>(node) {
            p.next += 1;
        }
        fire(tree, node, parent, &event);
    }
    // Fade and subtitle.
    if let Some(overlay) = overlay {
        let (cover, color, subtitle_over) = {
            let Some(mut p) = tree.component_mut::<CutscenePlayer>(node) else {
                return;
            };
            if let Some(f) = p.fade {
                let k = if f.duration > 0.0 {
                    ((time - f.start) / f.duration).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                p.cover = f.from + (f.to - f.from) * k;
            }
            (
                p.cover,
                p.fade.map_or([0.0; 3], |f| f.color),
                p.subtitle_until <= time,
            )
        };
        if let Some(panel) = tree.get_node(overlay, "Fade") {
            if let Some(mut panel) = tree.component_mut::<ab_ui::Panel>(panel) {
                panel.style.color = ab_core::Color::rgba(color[0], color[1], color[2], cover);
            }
        }
        if subtitle_over {
            if let Some(label) = tree.get_node(overlay, "Subtitle") {
                if let Some(mut label) = tree.component_mut::<ab_ui::Label>(label) {
                    if !label.text.is_empty() {
                        label.text.clear();
                    }
                }
            }
        }
    }
    if time >= file.duration() {
        finish(tree, node);
    }
}

fn fire(tree: &mut SceneTree, node: NodeHandle, parent: NodeHandle, event: &Event) {
    let target = |tree: &SceneTree, path: &str| {
        let found = tree.get_node(parent, path);
        if found.is_none() {
            tracing::warn!("cutscene: no node `{path}`");
        }
        found
    };
    match &event.kind {
        EventKind::Animation {
            target: path,
            clip,
            crossfade,
        } => {
            let Some(t) = target(tree, path) else { return };
            match tree.component_mut::<ab_animation::AnimationPlayer>(t) {
                Some(mut player) => {
                    let ok = match crossfade {
                        Some(seconds) if *seconds > 0.0 => player.crossfade_named(clip, *seconds),
                        _ => player.play_named(clip),
                    };
                    if !ok {
                        tracing::warn!("cutscene: `{path}` has no clip `{clip}`");
                    }
                }
                None => tracing::warn!("cutscene: `{path}` is not an AnimationPlayer"),
            }
        }
        EventKind::Sound { stream, volume_db } => {
            let loaded = tree
                .world()
                .resource_mut::<ab_asset::AssetServer>()
                .map(|mut a| a.load_audio(stream));
            match loaded {
                Some(Ok(sound)) => {
                    if let Some(mut audio) = tree.world().resource_mut::<ab_audio::Audio>() {
                        audio.play(
                            &sound,
                            ab_audio::SoundOptions {
                                volume_db: volume_db.unwrap_or(0.0),
                                ..Default::default()
                            },
                        );
                    }
                }
                Some(Err(err)) => tracing::warn!("cutscene: {stream}: {err}"),
                None => {}
            }
        }
        EventKind::Signal { name } => {
            tree.emit(Signal::new(node, "cutscene_event").with_arg(name.clone()));
        }
        EventKind::Visible {
            target: path,
            visible,
        } => {
            if let Some(t) = target(tree, path) {
                let _ = tree.set_visible(t, *visible);
            }
        }
        EventKind::Fade {
            to,
            duration,
            color,
        } => {
            let rgb = color
                .as_deref()
                .and_then(|c| {
                    crate::scene_file::ColorDesc::Hex(c.to_owned())
                        .to_color()
                        .ok()
                })
                .map_or([0.0; 3], |c| [c.r, c.g, c.b]);
            if let Some(mut p) = tree.component_mut::<CutscenePlayer>(node) {
                p.fade = Some(Fade {
                    from: p.cover,
                    to: to.clamp(0.0, 1.0),
                    start: event.time,
                    duration: *duration,
                    color: rgb,
                });
            }
        }
        EventKind::Subtitle {
            text,
            key,
            duration,
        } => {
            let text = match key {
                Some(key) => tree
                    .world()
                    .resource::<crate::localization::Localization>()
                    .map_or_else(|| text.clone(), |l| l.text(key)),
                None => text.clone(),
            };
            if let Some(mut p) = tree.component_mut::<CutscenePlayer>(node) {
                p.subtitle_until = event.time + duration;
            }
            if let Some(label) = tree
                .get_node(node, "CutsceneOverlay")
                .and_then(|o| tree.get_node(o, "Subtitle"))
            {
                if let Some(mut label) = tree.component_mut::<ab_ui::Label>(label) {
                    label.text = text;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_round_trip_and_know_their_length() {
        let text = r#"
camera_interpolation = "ease"

[[camera]]
time = 0.0
position = [0.0, 2.0, 10.0]
fov = 50.0

[[camera]]
time = 4.0
position = [10.0, 2.0, 10.0]
rotation = [0.0, 90.0, 0.0]
fov = 70.0

[[event]]
time = 3.5
kind = "subtitle"
text = "Hello"
duration = 2.0

[[event]]
time = 1.0
kind = "signal"
name = "boom"
"#;
        let file = CutsceneFile::from_toml(text).unwrap();
        assert_eq!(file.events[0].kind.name(), "signal", "sorted by time");
        assert_eq!(file.duration(), 5.5, "until the subtitle ends");
        let (p, _, fov) = file.camera_at(2.0).unwrap();
        assert!((p.x - 5.0).abs() < 1e-3 && (fov - 60.0).abs() < 1e-3);
        let (start, _, _) = file.camera_at(0.2).unwrap();
        assert!(start.x < 0.2, "eased out of the first key");
        assert_eq!(CutsceneFile::from_toml(&file.to_toml()).unwrap(), file);
        assert!(CutsceneFile::from_toml("[[event]]\ntime = 1.0\nkind = \"teleport\"\n").is_err());
        assert!(CutsceneFile::default().camera_at(1.0).is_none());
    }
}
