//! Serde mirror of the scene file format.

use ab_core::Color;
use serde::{Deserialize, Serialize};

/// A parsed scene file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentDesc>,
    #[serde(default, rename = "node")]
    pub nodes: Vec<NodeDesc>,
}

impl SceneFile {
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Compact, hand-editable TOML: one line per key, `[x, y, z]` and `{ k = v }` inline,
    /// floats in their shortest form, `name`/`parent`/`type` first.
    pub fn to_toml(&self) -> String {
        let value = toml::Value::try_from(self).expect("scene descriptions always serialize");
        let mut out = String::new();
        if let Some(toml::Value::Table(environment)) = value.get("environment") {
            out.push_str("[environment]\n");
            write_keys(&mut out, environment, &[]);
        }
        if let Some(toml::Value::Array(nodes)) = value.get("node") {
            for node in nodes {
                if let toml::Value::Table(node) = node {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str("[[node]]\n");
                    write_keys(&mut out, node, KEY_ORDER);
                }
            }
        }
        out
    }
}

/// Keys written first, in this order; the rest follow alphabetically.
const KEY_ORDER: &[&str] = &[
    "name", "parent", "type", "instance", "position", "rotation", "scale", "visible", "groups",
    "script",
];

fn write_keys(out: &mut String, table: &toml::Table, first: &[&str]) {
    let mut keys: Vec<&String> = table.keys().collect();
    keys.sort_by_key(|k| (first.iter().position(|f| f == k).unwrap_or(first.len()), *k));
    for key in keys {
        out.push_str(key);
        out.push_str(" = ");
        write_inline(out, &table[key]);
        out.push('\n');
    }
}

fn write_inline(out: &mut String, value: &toml::Value) {
    use std::fmt::Write as _;
    match value {
        // Values come from f32 fields: print the f32's shortest form (0.45, not
        // 0.44999998807907104), keeping a decimal point so they stay floats.
        toml::Value::Float(f) => {
            let text = (*f as f32).to_string();
            out.push_str(&text);
            if !text.contains(['.', 'e', 'i', 'n']) {
                out.push_str(".0");
            }
        }
        toml::Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_inline(out, item);
            }
            out.push(']');
        }
        toml::Value::Table(table) => {
            if table.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{ ");
            for (i, (key, item)) in table.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let _ = write!(out, "{key} = ");
                write_inline(out, item);
            }
            out.push_str(" }");
        }
        other => {
            let _ = write!(out, "{other}");
        }
    }
}

/// One node.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NodeDesc {
    pub name: String,
    /// Path of the parent from the scene root (`"."` = the root). Absent on the root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Node class (`Node3D`, `MeshInstance3D`, `RigidBody3D`, ...).
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Instead of `type`: a glTF model or another scene file to instance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 3]>,
    /// Euler angles in degrees, applied Y, X, Z (Godot's `rotation_degrees`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    /// Script registry name (a node with one script).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    /// Several scripts, run in this order (after `script`, when both are given). Saved
    /// scenes use `script` for one script and `scripts` for more.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scripts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cast_shadows: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mesh: Option<MeshDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material: Option<MaterialDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light: Option<LightDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character: Option<CharacterDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<ShapeDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timer: Option<TimerDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub particles: Option<ParticlesDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub joint: Option<JointDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ragdoll: Option<RagdollDesc>,
}

/// A `Joint3D`: which bodies, how. Angles in degrees.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct JointDesc {
    /// `pin` (default), `hinge`, `cone` (cone-twist) or `fixed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Paths of the two bodies relative to the joint node (`"../Torso"`); empty leaves the
    /// joint unconnected.
    pub body_a: String,
    pub body_b: String,
    /// Hinge limits `[min, max]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<[f32; 2]>,
    /// Cone-twist: swing away from / twist around the joint's X axis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swing: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twist: Option<f32>,
    /// Let the joined bodies collide with each other.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collide: Option<bool>,
}

/// A `Ragdoll`: limp from the start (`active = true`) or kinematic until activated.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RagdollDesc {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
}

/// `"#rrggbb"` / `"#rrggbbaa"` in sRGB, or linear `[r, g, b]` / `[r, g, b, a]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColorDesc {
    Hex(String),
    Rgb([f32; 3]),
    Rgba([f32; 4]),
}

impl ColorDesc {
    pub fn to_color(&self) -> Result<Color, String> {
        match self {
            Self::Rgb([r, g, b]) => Ok(Color::rgb(*r, *g, *b)),
            Self::Rgba([r, g, b, a]) => Ok(Color::rgba(*r, *g, *b, *a)),
            Self::Hex(hex) => {
                let digits = hex.strip_prefix('#').unwrap_or(hex);
                let byte = |i: usize| {
                    digits
                        .get(i..i + 2)
                        .and_then(|s| u8::from_str_radix(s, 16).ok())
                        .ok_or_else(|| {
                            format!("invalid color `{hex}` (expected #rrggbb or #rrggbbaa)")
                        })
                };
                match digits.len() {
                    6 => Ok(Color::from_srgb8(byte(0)?, byte(2)?, byte(4)?, 255)),
                    8 => Ok(Color::from_srgb8(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
                    _ => Err(format!(
                        "invalid color `{hex}` (expected #rrggbb or #rrggbbaa)"
                    )),
                }
            }
        }
    }

    /// Hex form of a linear color.
    pub fn from_color(color: Color) -> Self {
        let encode = |c: f32| {
            let c = c.clamp(0.0, 1.0);
            let s = if c <= 0.003_130_8 {
                c * 12.92
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0).round() as u8
        };
        let [r, g, b] = [encode(color.r), encode(color.g), encode(color.b)];
        let a = (color.a.clamp(0.0, 1.0) * 255.0).round() as u8;
        Self::Hex(if a == 255 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        })
    }
}

/// Built-in meshes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "primitive", rename_all = "lowercase", deny_unknown_fields)]
pub enum MeshDesc {
    Cube {
        #[serde(default = "one")]
        size: f32,
    },
    Sphere {
        #[serde(default = "half")]
        radius: f32,
        #[serde(default = "segments")]
        segments: u32,
        #[serde(default = "rings")]
        rings: u32,
    },
    Plane {
        #[serde(default = "one")]
        size: f32,
        #[serde(default = "one_u32")]
        subdivisions: u32,
    },
}

fn one() -> f32 {
    1.0
}
fn half() -> f32 {
    0.5
}
fn segments() -> u32 {
    32
}
fn rings() -> u32 {
    16
}
fn one_u32() -> u32 {
    1
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MaterialDesc {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roughness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metallic: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emission: Option<ColorDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emission_energy: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unshaded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub double_sided: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CameraDesc {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fov: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub near: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub far: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<bool>,
}

/// Any light; keys not used by the light type are ignored.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LightDesc {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub energy: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<f32>,
    /// Spot cone angle in degrees.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub angle: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub softness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadows: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BodyDesc {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mass: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gravity_scale: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub friction: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounce: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linear_damping: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub angular_damping: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_rotation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuous_cd: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_sleep: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CharacterDesc {
    /// Degrees.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub floor_max_angle: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub floor_snap_length: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_step_height: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push_rigid_bodies: Option<bool>,
}

/// `{ box = [x, y, z] }`, `{ sphere = r }`, `{ capsule = [r, height] }`,
/// `{ cylinder = [r, height] }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum ShapeDesc {
    Box([f32; 3]),
    Sphere(f32),
    Capsule([f32; 2]),
    Cylinder([f32; 2]),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AudioDesc {
    /// Asset path of the sound.
    pub stream: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_db: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub looping: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autoplay: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_distance: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_distance: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TimerDesc {
    pub wait_time: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub one_shot: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autostart: Option<bool>,
}

/// A particle emitter: a preset plus overrides.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ParticlesDesc {
    /// `smoke`, `fire`, `embers`, `sparks`, `dust`, `magic`, `wisps`, `explosion`, `rain`,
    /// `snow`, `heal`, `fireflies`, `splash`, `leaves` (default: a plain white emitter).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifetime: Option<f32>,
    /// `[start, end]` in metres.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<[f32; 2]>,
    /// `[min, max]` in m/s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<[f32; 2]>,
    /// Replaces the color ramp with a fade between these two.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_start: Option<ColorDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_end: Option<ColorDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emission: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub one_shot: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emitting: Option<bool>,
}

impl NodeDesc {
    /// The node's scripts in order: `script`, then `scripts`.
    pub fn script_list(&self) -> Vec<String> {
        self.script.iter().chain(&self.scripts).cloned().collect()
    }

    /// Sets the scripts in order, as `script` for one and `scripts` for several.
    pub fn set_script_list(&mut self, list: Vec<String>) {
        let mut list: Vec<String> = list
            .into_iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        if list.len() == 1 {
            self.script = list.pop();
            self.scripts.clear();
        } else {
            self.script = None;
            self.scripts = list;
        }
    }
}

/// Overrides of the `Environment` resource applied when the scene is loaded.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EnvironmentDesc {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<ColorDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambient_color: Option<ColorDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambient_energy: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fog_color: Option<ColorDesc>,
    /// Setting a density enables fog (0 disables it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fog_density: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fog_height: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fog_height_falloff: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exposure: Option<f32>,
    /// 1 = neutral.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contrast: Option<f32>,
    /// 1 = neutral, 0 = greyscale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f32>,
    /// Multiplied into the final image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<ColorDesc>,
    /// Darkening at the corners (0 = off).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vignette: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vignette_smoothness: Option<f32>,
    /// Glow intensity around bright pixels (0 = off, 0.05..0.3 looks natural).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bloom: Option<f32>,
    /// HDR brightness where bloom starts (default 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bloom_threshold: Option<f32>,
    /// Color split towards the edges (0 = off).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chromatic_aberration: Option<f32>,
    /// Film grain (0 = off).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grain: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        let white = ColorDesc::Hex("#ffffff".into()).to_color().unwrap();
        assert!((white.r - 1.0).abs() < 1e-6 && white.a == 1.0);
        let half = ColorDesc::Hex("#ff000080".into()).to_color().unwrap();
        assert!((half.a - 128.0 / 255.0).abs() < 1e-6);
        assert!(ColorDesc::Hex("#12".into()).to_color().is_err());
        assert!(ColorDesc::Hex("#gg0000".into()).to_color().is_err());
        assert_eq!(
            ColorDesc::Rgb([0.5, 0.25, 1.0]).to_color().unwrap(),
            Color::rgb(0.5, 0.25, 1.0)
        );
        // Hex round trip through linear.
        let color = ColorDesc::Hex("#6a5f55".into()).to_color().unwrap();
        assert_eq!(
            ColorDesc::from_color(color),
            ColorDesc::Hex("#6a5f55".into())
        );
    }

    #[test]
    fn parses_all_sections() {
        let text = r##"
            [environment]
            background = "#05060a"
            fog_density = 0.05

            [[node]]
            name = "Root"
            type = "Node3D"

            [[node]]
            name = "Ball"
            parent = "."
            type = "RigidBody3D"
            position = [0.0, 2.0, 0.0]
            rotation = [0.0, 90.0, 0.0]
            groups = ["props"]
            body = { mass = 3.0, bounce = 0.5 }

            [[node]]
            name = "Shape"
            parent = "Ball"
            type = "CollisionShape3D"
            shape = { sphere = 0.5 }

            [[node]]
            name = "Mesh"
            parent = "Ball"
            type = "MeshInstance3D"
            mesh = { primitive = "sphere", radius = 0.5 }
            material = { color = [1.0, 0.0, 0.0], metallic = 1.0 }
        "##;
        let scene = SceneFile::from_toml(text).unwrap();
        assert_eq!(scene.nodes.len(), 4);
        assert_eq!(scene.nodes[1].body.as_ref().unwrap().mass, Some(3.0));
        assert_eq!(scene.nodes[2].shape, Some(ShapeDesc::Sphere(0.5)));
        assert_eq!(
            scene.nodes[3].mesh,
            Some(MeshDesc::Sphere {
                radius: 0.5,
                segments: 32,
                rings: 16
            })
        );
        // Round trip through text, which stays compact.
        let text = scene.to_toml();
        assert_eq!(SceneFile::from_toml(&text).unwrap(), scene);
        assert!(text.contains("position = [0.0, 2.0, 0.0]\n"), "{text}");
        assert!(
            text.contains("body = { bounce = 0.5, mass = 3.0 }\n"),
            "{text}"
        );
        assert!(text.contains("fog_density = 0.05\n"), "{text}");
        assert!(text.starts_with("[environment]\n"));
        assert!(
            text.contains("[[node]]\nname = \"Ball\"\nparent = \".\"\ntype = \"RigidBody3D\"\n"),
            "{text}"
        );
    }

    #[test]
    fn unknown_keys_are_errors_with_location() {
        let err =
            SceneFile::from_toml("[[node]]\nname = \"A\"\npositon = [0, 0, 0]\n").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("positon"), "{message}");
        assert!(message.contains("line 3"), "{message}");
    }
}
