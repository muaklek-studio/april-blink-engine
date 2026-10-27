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

/// Version of the scene file format this engine writes (`format_version` at the top of a
/// file). Files without one are version 1. Raise it when a change would misread older files,
/// and add the step that upgrades them to [`MIGRATIONS`].
pub const SCENE_FORMAT_VERSION: u32 = 1;

/// Upgrades of older files, applied before parsing: `MIGRATIONS[i]` turns a version `i + 1`
/// file into version `i + 2` (renaming keys, converting values, …).
const MIGRATIONS: &[fn(&mut toml::Table)] = &[];

const _: () = assert!(MIGRATIONS.len() as u32 + 1 == SCENE_FORMAT_VERSION);

/// What [`SceneFile::from_toml`] reads first, to know how to read the rest.
#[derive(Deserialize)]
struct Header {
    format_version: Option<u32>,
}

/// A scene file with its version (a separate type, so [`SceneFile`] stays version-free).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionedScene {
    #[serde(default)]
    #[allow(dead_code)]
    format_version: Option<u32>,
    #[serde(default)]
    environment: Option<EnvironmentDesc>,
    #[serde(default, rename = "node")]
    nodes: Vec<NodeDesc>,
}

impl SceneFile {
    /// Parses a scene file of any supported version, upgrading an older one. A file of a
    /// newer engine is an error rather than half understood.
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        let version = toml::from_str::<Header>(text)?.format_version.unwrap_or(1);
        let scene: VersionedScene = if version == SCENE_FORMAT_VERSION {
            toml::from_str(text)?
        } else if (1..SCENE_FORMAT_VERSION).contains(&version) {
            let mut table: toml::Table = toml::from_str(text)?;
            for migrate in &MIGRATIONS[version as usize - 1..] {
                migrate(&mut table);
            }
            table.try_into()?
        } else {
            return Err(serde::de::Error::custom(format!(
                "scene format version {version} is not supported: this engine reads versions \
                 1 to {SCENE_FORMAT_VERSION} (was the file saved by a newer April Blink?)"
            )));
        };
        // Destructured in full, so a new field of `SceneFile` is not forgotten here.
        let VersionedScene {
            format_version: _,
            environment,
            nodes,
        } = scene;
        Ok(Self { environment, nodes })
    }

    /// Compact, hand-editable TOML: one line per key, `[x, y, z]` and `{ k = v }` inline,
    /// floats in their shortest form, `name`/`parent`/`type` first.
    pub fn to_toml(&self) -> String {
        let value = toml::Value::try_from(self).expect("scene descriptions always serialize");
        let mut out = format!("format_version = {SCENE_FORMAT_VERSION}\n");
        if let Some(toml::Value::Table(environment)) = value.get("environment") {
            out.push_str("\n[environment]\n");
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
    /// Changes to an instanced scene's nodes, kept with this instance (see
    /// [`super::prefab`]): `[{ path = "Lamp", visible = false }]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<toml::Table>,
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
    /// Exported property values, per script name:
    /// `script_properties = { Mover = { speed = 4.0, tint = "#ff8040ff" } }`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub script_properties: std::collections::BTreeMap<String, toml::Table>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cast_shadows: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mesh: Option<MeshDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material: Option<MaterialDesc>,
    /// A `Path3D`'s points and what it builds (see [`crate::spline`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathDesc>,
    /// A `Generator`'s kind, seed and parameters (see [`crate::procgen`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generator: Option<GeneratorDesc>,
    /// A `Cutscene`'s file (see [`crate::cutscene`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutscene: Option<crate::cutscene::CutsceneDesc>,
    /// An animation state machine driving the model's player (`anims/hero.abstate`, see
    /// [`crate::state_machine`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_machine: Option<String>,
    /// Levels of detail for the meshes in the node (see [`super::lod`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lod: Option<super::lod::LodDesc>,
    /// An `HLOD` group's switch distance and detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hlod: Option<super::lod::HlodDesc>,
    /// A rig file for the model (`rigs/knight.abrig`, see [`crate::rig`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rig: Option<String>,
    /// IK chains (see [`crate::ik`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ik: Vec<crate::ik::IkDesc>,
    /// A `PostVolume`'s box and values (see [`super::post_volume`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_volume: Option<super::post_volume::PostVolumeDesc>,
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
    pub audio_zone: Option<AudioZoneDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timer: Option<TimerDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub particles: Option<ParticlesDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub animation: Option<AnimationDesc>,
    /// UI: `CanvasLayer`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canvas: Option<CanvasDesc>,
    /// UI: where any control sits in its parent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control: Option<ControlDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<LabelDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<ButtonDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub panel: Option<PanelDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<ProgressDesc>,
    /// `VBoxContainer` / `HBoxContainer`.
    #[serde(rename = "box", skip_serializing_if = "Option::is_none")]
    pub box_container: Option<BoxDesc>,
    /// `TextureRect`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageDesc>,
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
///
/// With `skeleton`, it is a humanoid ragdoll for a character, built when the scene loads:
///
/// ```toml
/// ragdoll = { skeleton = "../Knight", mass = 80.0, bones = { left_hand = "hand_l_ik" } }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RagdollDesc {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    /// Path from the ragdoll to the character whose skeleton it is built on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skeleton: Option<String>,
    /// Bones set by hand (`left_upper_arm = "bone name"`, `""` for none); the others are found
    /// by name or by the skeleton's shape.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub bones: std::collections::BTreeMap<String, String>,
    /// Total mass in kg (default 70).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mass: Option<f32>,
    /// Body thickness, 1 = average build.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thickness: Option<f32>,
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
    /// A material file (`materials/brick.abmat`) to start from; the keys written here
    /// override its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorDesc>,
    /// Image multiplied with `color` (sRGB).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub albedo_texture: Option<String>,
    /// Tangent-space normal map (linear).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normal_texture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normal_scale: Option<f32>,
    /// glTF layout: roughness in G, metallic in B (linear).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metallic_roughness_texture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emission_texture: Option<String>,
    /// Pixels whose albedo alpha is below this are cut out (foliage, fences).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha_cutoff: Option<f32>,
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
    /// A custom surface shader (`shaders/water.wgsl`; see `ab_rendering::shader`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shader: Option<String>,
    /// Values for the shader's `// param` lines, by name (the others keep their defaults).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<std::collections::BTreeMap<String, f32>>,
}

impl MaterialDesc {
    /// A material file's contents (`.abmat`: the keys of a `material = { … }` table).
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("material descriptions always serialize")
    }

    /// `self`'s keys over `base`'s (`file` excepted: the result stands alone).
    pub fn over(&self, base: &MaterialDesc) -> MaterialDesc {
        fn pick<T: Clone>(a: &Option<T>, b: &Option<T>) -> Option<T> {
            a.clone().or_else(|| b.clone())
        }
        MaterialDesc {
            file: None,
            color: pick(&self.color, &base.color),
            albedo_texture: pick(&self.albedo_texture, &base.albedo_texture),
            normal_texture: pick(&self.normal_texture, &base.normal_texture),
            normal_scale: pick(&self.normal_scale, &base.normal_scale),
            metallic_roughness_texture: pick(
                &self.metallic_roughness_texture,
                &base.metallic_roughness_texture,
            ),
            emission_texture: pick(&self.emission_texture, &base.emission_texture),
            alpha_cutoff: pick(&self.alpha_cutoff, &base.alpha_cutoff),
            roughness: pick(&self.roughness, &base.roughness),
            metallic: pick(&self.metallic, &base.metallic),
            emission: pick(&self.emission, &base.emission),
            emission_energy: pick(&self.emission_energy, &base.emission_energy),
            unshaded: pick(&self.unshaded, &base.unshaded),
            double_sided: pick(&self.double_sided, &base.double_sided),
            shader: pick(&self.shader, &base.shader),
            // A node's values over the file's, name by name.
            params: match (&self.params, &base.params) {
                (Some(mine), Some(theirs)) => {
                    let mut all = theirs.clone();
                    all.extend(mine.iter().map(|(k, v)| (k.clone(), *v)));
                    Some(all)
                }
                (mine, theirs) => pick(mine, theirs),
            },
        }
    }
}

/// `generator = { kind = "terrain", seed = 3, height = 20.0, ... }` of a `Generator`: its
/// other keys are the generator's parameters.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GeneratorDesc {
    pub kind: String,
    #[serde(default)]
    pub seed: u64,
    #[serde(flatten)]
    pub params: toml::Table,
}

/// `path = { points = [...], mode = "ribbon", ... }` of a `Path3D`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PathDesc {
    /// Control points, in the node's space.
    pub points: Vec<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed: Option<bool>,
    /// `none` (a path only), `ribbon`, `tube` or `instances`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Ribbon width (m).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f32>,
    /// Tube radius (m).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sides: Option<u32>,
    /// How far a tube's middle hangs (m): cables.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sag: Option<f32>,
    /// What `instances` places: a model or prefab.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub piece: Option<String>,
    /// Metres between instances.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spacing: Option<f32>,
    /// Instances face along the path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align: Option<bool>,
    /// A ribbon or tube gets a collision mesh.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collide: Option<bool>,
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
    /// Omni lights: fade out over this many seconds (muzzle flashes, explosions in effect
    /// scenes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flash: Option<f32>,
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
    /// What the body is (bit mask, default 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<u32>,
    /// What it responds to (bit mask, default all): solid bodies collide when each one's
    /// mask has the other's layer; an area sees what its own mask selects.
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
    /// Asset path of the sound file or sound event (`*.absound`).
    pub stream: String,
    /// Bus to play on (else the event's, else the master).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bus: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rolloff: Option<ab_audio::Rolloff>,
    /// Points of a custom rolloff, `[distance, volume]` in 0..=1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curve: Option<Vec<[f32; 2]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spread: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doppler: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occlusion: Option<bool>,
}

/// An `AudioZone`: `audio_zone = { snapshot = "Cave", size = [10, 4, 10] }` (a box) or
/// `radius = 6` (a sphere).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AudioZoneDesc {
    pub snapshot: String,
    /// Full size of a box zone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<[f32; 3]>,
    /// Radius of a sphere zone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fade_distance: Option<f32>,
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

/// A `CanvasLayer`: its controls draw above lower layers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CanvasDesc {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<i32>,
}

/// Where a control sits inside its parent (in UI units, 1920 × 1080 by default).
///
/// Either a preset with a size and a margin, `{ preset = "top_right", size = [300, 50],
/// margin = 24 }`, or anchors and offsets as Godot has them (these win).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ControlDesc {
    /// `top_left`, `top_right`, `bottom_left`, `bottom_right`, `center_left`, `center_top`,
    /// `center_right`, `center_bottom`, `center`, `left_wide`, `top_wide`, `right_wide`,
    /// `bottom_wide`, `vcenter_wide`, `hcenter_wide`, `full_rect`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin: Option<f32>,
    /// Fractions of the parent's rect: left, top, right, bottom.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchors: Option<[f32; 4]>,
    /// Edges' distances from the anchors: left, top, right, bottom.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offsets: Option<[f32; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_size: Option<[f32; 2]>,
    /// Takes a share of a box container's spare room.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expand: Option<bool>,
    /// `stop` (takes the pointer) or `ignore` (lets it through).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mouse: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modulate: Option<ColorDesc>,
    /// Children are cut to this control's rect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LabelDesc {
    pub text: String,
    /// Translation key: the text shown in the game's language (see
    /// [`crate::localization`]); `text` is what shows without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Font size in UI units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorDesc>,
    /// `left`, `center`, `right`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
    /// `top`, `center`, `bottom`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valign: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outline: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outline_color: Option<ColorDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autowrap: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ButtonDesc {
    pub text: String,
    /// Translation key (see [`LabelDesc::key`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    /// Background (hover and press shades follow).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_color: Option<ColorDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PanelDesc {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corner_radius: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border_width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border_color: Option<ColorDesc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ProgressDesc {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_color: Option<ColorDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_color: Option<ColorDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_percentage: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BoxDesc {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub separation: Option<f32>,
    /// `begin`, `center`, `end`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alignment: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ImageDesc {
    /// A PNG or JPEG under assets/.
    pub texture: String,
    /// `scale`, `keep_aspect`, `cover`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stretch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<ColorDesc>,
}

/// An `AnimationPlayer` playing clip files (see [`crate::animation_file`]) on the nodes
/// under its parent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AnimationDesc {
    /// Clip files, e.g. `["anims/door_open.toml"]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<String>,
    /// The clip to play when the scene starts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autoplay: Option<String>,
    /// Loop (default true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub looping: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
}

/// A particle emitter: a preset plus overrides.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ParticlesDesc {
    /// `smoke`, `fire`, `embers`, `sparks`, `dust`, `magic`, `wisps`, `explosion`, `rain`,
    /// `snow`, `heal`, `fireflies`, `splash`, `leaves`, `blood`, `blood_mist` (default: a
    /// plain white emitter).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifetime: Option<f32>,
    /// 0..1: how much lifetimes vary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifetime_random: Option<f32>,
    /// Where particles start: `"point"`, `{ sphere = { radius = 0.2 } }`,
    /// `{ box = { size = [1.0, 0.0, 1.0] } }`, `{ ring = { radius = 0.5 } }`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<ab_effects::EmissionShape>,
    /// Initial direction, and the cone around it in degrees (180: every way).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spread: Option<f32>,
    /// Acceleration in m/s² (positive y rises).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gravity: Option<[f32; 3]>,
    /// Air drag per second.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damping: Option<f32>,
    /// Random swirling, m/s².
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turbulence: Option<f32>,
    /// 0..1: how much sizes vary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_random: Option<f32>,
    /// Color over the particles' life: `[[0.0, "#ffcc66"], [1.0, "#ff000000"]]`. Replaces
    /// the preset's ramp and `color_start`/`color_end`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<(f32, ColorDesc)>,
    /// Spin in radians per second, `[min, max]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spin: Option<[f32; 2]>,
    /// Edge: 0 soft (smoke), 1 hard dot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hardness: Option<f32>,
    /// Elongation along the velocity, per m/s (sparks, rain).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stretch: Option<f32>,
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
    /// Most the fog can hide, 0..1 (default 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fog_max_opacity: Option<f32>,
    /// Glow of sunlight in the fog (0 = none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fog_sun_scattering: Option<f32>,
    /// How tightly the glow gathers around the sun: 0 even, 0.9 a tight halo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fog_sun_anisotropy: Option<f32>,
    /// How uneven the fog is near the camera: 0 even, 1 clear gaps between thick banks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fog_patchiness: Option<f32>,
    /// Size of the fog patches, in metres.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fog_patch_size: Option<f32>,
    /// Wind blowing the fog patches along, in metres per second.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fog_wind: Option<[f32; 3]>,
    /// Light shafts: beams where the sun and spot lights shine through openings. Setting an
    /// intensity enables them (0 disables them).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light_shafts: Option<f32>,
    /// How thick the air is for the shafts, per metre (the fog adds to it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light_shafts_density: Option<f32>,
    /// 0 lights the air evenly, 0.8 makes it bright looking towards the light.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light_shafts_anisotropy: Option<f32>,
    /// How far from the camera the shafts are traced, in metres.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light_shafts_distance: Option<f32>,
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
    /// Depth of field: the sharp distance in metres.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_distance: Option<f32>,
    /// Depth of field: metres either side of the focus to the full blur.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_range: Option<f32>,
    /// Depth of field: the largest blur in pixels at 1080 lines (0 = off).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth_of_field: Option<f32>,
    /// Camera motion blur (0 = off, 1 = over one frame's motion).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_blur: Option<f32>,
    /// Skip meshes hidden behind others (see `ab_rendering::OcclusionCulling`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occlusion_culling: Option<bool>,
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
        assert!(text.starts_with("format_version = 1\n\n[environment]\n"));
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

    #[test]
    fn format_versions() {
        let node = "[[node]]\nname = \"A\"\n";
        // Files from before versions were written are version 1.
        let unversioned = SceneFile::from_toml(node).unwrap();
        let versioned = SceneFile::from_toml(&format!("format_version = 1\n{node}")).unwrap();
        assert_eq!(unversioned, versioned);
        assert!(unversioned
            .to_toml()
            .starts_with("format_version = 1\n\n[[node]]\n"));

        for bad in ["format_version = 0\n", "format_version = 99\n"] {
            let message = SceneFile::from_toml(&format!("{bad}{node}"))
                .unwrap_err()
                .to_string();
            assert!(message.contains("not supported"), "{message}");
        }
        let message = SceneFile::from_toml(&format!("format_version = \"1\"\n{node}"))
            .unwrap_err()
            .to_string();
        assert!(message.contains("format_version"), "{message}");
    }
}
