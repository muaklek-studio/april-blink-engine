//! Animation clips made in the editor: keyframed position, rotation and scale tracks of the
//! nodes under an `AnimationPlayer` node, saved as TOML (`assets/anims/door_open.toml`):
//!
//! ```toml
//! name = "Open"
//! length = 1.5
//!
//! [[track]]
//! target = "Door"            # path from the AnimationPlayer's parent
//! property = "rotation"      # position | rotation (degrees) | scale
//! interpolation = "smooth"   # linear | step | smooth
//! keys = [[0.0, [0.0, 0.0, 0.0]], [1.5, [0.0, 95.0, 0.0]]]
//! ```
//!
//! Scenes play them with an `AnimationPlayer` node:
//!
//! ```toml
//! [[node]]
//! name = "Animator"
//! parent = "."
//! type = "AnimationPlayer"
//! animation = { clips = ["anims/door_open.toml"], autoplay = "Open" }
//! ```

use ab_animation::{AnimationClip, Channel, Interpolation, Keyframes};
use ab_core::{EulerRot, Quat, Vec3};
use serde::{Deserialize, Serialize};

/// What a track animates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TrackProperty {
    #[default]
    Position,
    /// Euler angles in degrees, as in scene files.
    Rotation,
    Scale,
}

impl TrackProperty {
    pub const ALL: [TrackProperty; 3] = [Self::Position, Self::Rotation, Self::Scale];

    pub fn name(self) -> &'static str {
        match self {
            Self::Position => "position",
            Self::Rotation => "rotation",
            Self::Scale => "scale",
        }
    }
}

/// How a track moves between keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TrackInterpolation {
    /// Hold each key until the next.
    Step,
    #[default]
    Linear,
    /// Eased through the keys (Catmull-Rom).
    Smooth,
}

/// One animated property of one node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// Path of the node from the animated root (the `AnimationPlayer`'s parent); `.` is the
    /// root itself.
    pub target: String,
    #[serde(default)]
    pub property: TrackProperty,
    #[serde(default)]
    pub interpolation: TrackInterpolation,
    /// `(seconds, value)`, by time.
    #[serde(default)]
    pub keys: Vec<(f32, [f32; 3])>,
}

impl Track {
    /// Sets the key at `time` (replacing one within a frame of it), keeping keys in order.
    pub fn set_key(&mut self, time: f32, value: [f32; 3]) {
        match self
            .keys
            .iter_mut()
            .find(|(t, _)| (*t - time).abs() < 1.0 / 120.0)
        {
            Some(key) => key.1 = value,
            None => {
                self.keys.push((time, value));
                self.sort();
            }
        }
    }

    pub fn sort(&mut self) {
        self.keys.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
}

/// A clip file (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ClipFile {
    pub name: String,
    /// Seconds; `None`: until the last key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<f32>,
    #[serde(default, rename = "track", skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<Track>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClipFileError {
    #[error("invalid animation clip: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("cannot write the animation clip: {0}")]
    Write(#[from] toml::ser::Error),
}

impl ClipFile {
    pub fn from_toml(text: &str) -> Result<Self, ClipFileError> {
        let mut clip: Self = toml::from_str(text)?;
        for track in &mut clip.tracks {
            track.sort();
        }
        Ok(clip)
    }

    pub fn to_toml(&self) -> Result<String, ClipFileError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// The clip's length: `length`, else its last key.
    pub fn duration(&self) -> f32 {
        self.length.unwrap_or_else(|| {
            self.tracks
                .iter()
                .filter_map(|t| t.keys.last().map(|k| k.0))
                .fold(0.0, f32::max)
        })
    }

    /// The track of `target`'s `property`, made if missing.
    pub fn track_mut(&mut self, target: &str, property: TrackProperty) -> &mut Track {
        let index = match self
            .tracks
            .iter()
            .position(|t| t.target == target && t.property == property)
        {
            Some(i) => i,
            None => {
                self.tracks.push(Track {
                    target: target.to_owned(),
                    property,
                    interpolation: TrackInterpolation::default(),
                    keys: Vec::new(),
                });
                self.tracks.len() - 1
            }
        };
        &mut self.tracks[index]
    }

    /// The engine's clip, with `targets` the track targets in order of first appearance
    /// (the player's targets): `(clip, target paths)`.
    pub fn build(&self) -> (AnimationClip, Vec<String>) {
        let mut targets: Vec<String> = Vec::new();
        let mut channels = Vec::new();
        for track in self.tracks.iter().filter(|t| !t.keys.is_empty()) {
            let target = match targets.iter().position(|t| *t == track.target) {
                Some(i) => i,
                None => {
                    targets.push(track.target.clone());
                    targets.len() - 1
                }
            };
            channels.push(channel(track, target));
        }
        let mut clip = AnimationClip::new(self.name.clone(), channels);
        if let Some(length) = self.length {
            clip.duration = length.max(0.0);
        }
        (clip, targets)
    }
}

/// Scene-file rotation (degrees, YXZ as in scene files) as a quaternion.
pub fn rotation_from_degrees(degrees: [f32; 3]) -> Quat {
    let [x, y, z] = degrees.map(f32::to_radians);
    Quat::from_euler(EulerRot::YXZ, y, x, z)
}

/// A quaternion as scene-file degrees.
pub fn rotation_to_degrees(rotation: Quat) -> [f32; 3] {
    let (y, x, z) = rotation.to_euler(EulerRot::YXZ);
    [x, y, z].map(f32::to_degrees)
}

fn channel(track: &Track, target: usize) -> Channel {
    let keys = expand_rotation_keys(track);
    let times: Vec<f32> = keys.iter().map(|k| k.0).collect();
    let values: Vec<[f32; 3]> = keys.iter().map(|k| k.1).collect();
    let (interpolation, values) = match track.interpolation {
        TrackInterpolation::Step => (
            Interpolation::Step,
            values.iter().map(|v| (*v, None)).collect(),
        ),
        TrackInterpolation::Linear => (
            Interpolation::Linear,
            values.iter().map(|v| (*v, None)).collect(),
        ),
        // Catmull-Rom tangents (per second), as cubic spline triples.
        TrackInterpolation::Smooth => {
            let n = values.len();
            let tangent = |i: usize| -> [f32; 3] {
                if n < 2 {
                    return [0.0; 3];
                }
                let (a, b) = (i.saturating_sub(1), (i + 1).min(n - 1));
                let dt = (times[b] - times[a]).max(1e-4);
                std::array::from_fn(|c| (values[b][c] - values[a][c]) / dt)
            };
            (
                Interpolation::CubicSpline,
                (0..n)
                    .map(|i| (values[i], Some(tangent(i))))
                    .collect::<Vec<_>>(),
            )
        }
    };
    let keyframes = match track.property {
        TrackProperty::Position | TrackProperty::Scale => {
            let v: Vec<Vec3> = flatten(&values).into_iter().map(Vec3::from).collect();
            if track.property == TrackProperty::Position {
                Keyframes::Translation(v)
            } else {
                Keyframes::Scale(v)
            }
        }
        TrackProperty::Rotation => {
            // Tangents of Euler angles do not carry over to quaternions: smooth rotations
            // use the quaternion's own neighbours for theirs.
            // Each on the same side as the one before, or a spline through them would swing
            // the long way between keys.
            let mut quats: Vec<Quat> = values
                .iter()
                .map(|(v, _)| rotation_from_degrees(*v))
                .collect();
            for i in 1..quats.len() {
                quats[i] = align(quats[i], quats[i - 1]);
            }
            if interpolation == Interpolation::CubicSpline {
                let n = quats.len();
                let mut out = Vec::with_capacity(n * 3);
                for i in 0..n {
                    let (a, b) = (i.saturating_sub(1), (i + 1).min(n - 1));
                    let dt = (times[b] - times[a]).max(1e-4);
                    let (qa, qb) = (quats[a], align(quats[b], quats[a]));
                    let tangent = (qb - qa) * (1.0 / dt);
                    out.extend([tangent, quats[i], tangent]);
                }
                Keyframes::Rotation(out)
            } else {
                Keyframes::Rotation(quats)
            }
        }
    };
    Channel {
        target,
        interpolation,
        times,
        keyframes,
    }
}

/// `q` or `-q`, whichever is on the same side as `reference` (shortest blend).
fn align(q: Quat, reference: Quat) -> Quat {
    if q.dot(reference) < 0.0 {
        -q
    } else {
        q
    }
}

/// Values and, for smooth tracks, tangents, as the `[in, value, out]` triples cubic splines
/// use (or plain values).
fn flatten(values: &[([f32; 3], Option<[f32; 3]>)]) -> Vec<[f32; 3]> {
    values
        .iter()
        .flat_map(|(v, tangent)| match tangent {
            Some(t) => vec![*t, *v, *t],
            None => vec![*v],
        })
        .collect()
}

/// Rotation keys more than 90° apart get keys in between, so spins of 180° and more turn the
/// way the angles say rather than the short way round.
fn expand_rotation_keys(track: &Track) -> Vec<(f32, [f32; 3])> {
    if track.property != TrackProperty::Rotation || track.interpolation == TrackInterpolation::Step
    {
        return track.keys.clone();
    }
    let mut out = Vec::with_capacity(track.keys.len());
    for pair in track.keys.windows(2) {
        let ((t0, a), (t1, b)) = (pair[0], pair[1]);
        out.push((t0, a));
        let largest = (0..3).map(|c| (b[c] - a[c]).abs()).fold(0.0, f32::max);
        let steps = (largest / 90.0).ceil() as usize;
        for s in 1..steps {
            let k = s as f32 / steps as f32;
            out.push((
                t0 + (t1 - t0) * k,
                std::array::from_fn(|c| a[c] + (b[c] - a[c]) * k),
            ));
        }
    }
    if let Some(&last) = track.keys.last() {
        out.push(last);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_animation::Sample;

    fn door() -> ClipFile {
        ClipFile::from_toml(
            r#"
name = "Open"
length = 2.0

[[track]]
target = "Door"
property = "rotation"
keys = [[1.0, [0.0, 90.0, 0.0]], [0.0, [0.0, 0.0, 0.0]]]

[[track]]
target = "Door/Handle"
property = "position"
interpolation = "smooth"
keys = [[0.0, [0.0, 0.0, 0.0]], [0.5, [0.0, -0.1, 0.0]], [1.0, [0.0, 0.0, 0.0]]]
"#,
        )
        .unwrap()
    }

    #[test]
    fn parses_sorts_and_round_trips() {
        let clip = door();
        assert_eq!(clip.tracks[0].keys[0].0, 0.0, "keys sorted by time");
        assert_eq!(clip.duration(), 2.0);
        let again = ClipFile::from_toml(&clip.to_toml().unwrap()).unwrap();
        assert_eq!(again, clip);
    }

    #[test]
    fn builds_an_engine_clip() {
        let (clip, targets) = door().build();
        assert_eq!(targets, ["Door", "Door/Handle"]);
        assert_eq!(clip.duration, 2.0);
        let rotation = &clip.channels[0];
        let Sample::Rotation(q) = rotation.sample(0.5) else {
            panic!("rotation");
        };
        let degrees = rotation_to_degrees(q);
        assert!((degrees[1] - 45.0).abs() < 0.1, "{degrees:?}");
        // Smooth: through the middle key, eased (not a straight line) between.
        let handle = &clip.channels[1];
        let Sample::Translation(p) = handle.sample(0.5) else {
            panic!("translation");
        };
        assert!((p.y + 0.1).abs() < 1e-4);
        let Sample::Translation(q) = handle.sample(0.25) else {
            panic!("translation");
        };
        assert!(q.y < -0.04 && q.y > -0.1, "{q}");
    }

    #[test]
    fn big_rotations_turn_the_long_way() {
        let mut clip = ClipFile {
            name: "Spin".into(),
            ..Default::default()
        };
        let track = clip.track_mut(".", TrackProperty::Rotation);
        track.set_key(0.0, [0.0, 0.0, 0.0]);
        track.set_key(1.0, [0.0, 360.0, 0.0]);
        let (built, _) = clip.build();
        let Sample::Rotation(q) = built.channels[0].sample(0.5) else {
            panic!("rotation");
        };
        // Half way through a full turn: facing backwards, not standing still.
        let forward = q * Vec3::NEG_Z;
        assert!(forward.z > 0.99, "{forward}");
    }

    #[test]
    fn keys_replace_near_ones() {
        let mut track = Track {
            target: ".".into(),
            property: TrackProperty::Position,
            interpolation: TrackInterpolation::Linear,
            keys: vec![],
        };
        track.set_key(1.0, [1.0; 3]);
        track.set_key(0.0, [0.0; 3]);
        track.set_key(1.004, [2.0; 3]);
        assert_eq!(track.keys, vec![(0.0, [0.0; 3]), (1.0, [2.0; 3])]);
    }
}
