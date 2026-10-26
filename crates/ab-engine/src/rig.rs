//! Rigs: what a character's skeleton is, beyond its bones.
//!
//! A rig file (`.abrig`, TOML) goes with a model and says:
//!
//! * which bone is which humanoid part, where the names do not say so (`[bones]`; the rest
//!   is found from the names and the skeleton's shape, as for ragdolls);
//! * how far joints may turn (`[[limit]]`): a hinge (knees, elbows) turns about one axis
//!   between two angles; a cone (shoulders, hips, the neck) keeps the bone within an angle
//!   of where it points at rest;
//! * named IK chains (`[[chain]]`) a node's `ik` can use by name.
//!
//! ```toml
//! [bones]
//! left_hand = "hand_l"
//!
//! [[limit]]
//! bone = "calf_l"
//! hinge = { axis = [1.0, 0.0, 0.0], min = 0.0, max = 150.0 }
//!
//! [[limit]]
//! bone = "neck"
//! cone = 50.0
//!
//! [[chain]]
//! name = "left_leg"
//! tip = "left_foot"
//! bones = 2
//! ```
//!
//! A model node uses it with `rig = "rigs/knight.abrig"`.

use crate::ragdoll::skeleton_bones;
use ab_core::{Quat, Vec3};
use ab_physics::{HumanBone, HumanoidMap};
use ab_scene::{NodeHandle, SceneTree};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// File extension of rig files.
pub const RIG_EXTENSION: &str = "abrig";

/// `hinge = { axis = [x, y, z], min = degrees, max = degrees }` of a limit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HingeDesc {
    /// In the bone's own space (at rest).
    pub axis: [f32; 3],
    pub min: f32,
    pub max: f32,
}

/// `[[limit]]` of a rig file: one bone's hinge or cone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitDesc {
    /// The bone's name, or a humanoid part (`left_lower_leg`).
    pub bone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hinge: Option<HingeDesc>,
    /// Degrees the bone may point away from where it points at rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cone: Option<f32>,
}

/// `[[chain]]` of a rig file: bones an IK target moves together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainDesc {
    pub name: String,
    /// The last bone (the hand, the foot): a bone name or a humanoid part.
    pub tip: String,
    /// Bones above the tip that turn (2: upper and lower arm).
    #[serde(default = "two")]
    pub bones: usize,
}

fn two() -> usize {
    2
}

/// A rig file's contents (see the module docs).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RigFile {
    /// Humanoid part (`left_hand`) → bone name; `""` for none.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub bones: BTreeMap<String, String>,
    #[serde(rename = "limit", skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<LimitDesc>,
    #[serde(rename = "chain", skip_serializing_if = "Vec::is_empty")]
    pub chains: Vec<ChainDesc>,
}

impl RigFile {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let file: RigFile = toml::from_str(text).map_err(|e| e.to_string())?;
        file.check()?;
        Ok(file)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("rig files always serialize")
    }

    /// Values that cannot work, named.
    pub fn check(&self) -> Result<(), String> {
        for key in self.bones.keys() {
            if HumanBone::from_key(key).is_none() {
                return Err(format!("[bones]: `{key}` is not a humanoid part"));
            }
        }
        for limit in &self.limits {
            match (&limit.hinge, limit.cone) {
                (Some(_), Some(_)) | (None, None) => {
                    return Err(format!(
                        "limit of `{}`: give it a hinge or a cone",
                        limit.bone
                    ))
                }
                (Some(h), None) => {
                    if Vec3::from(h.axis).length_squared() < 1e-6 {
                        return Err(format!("limit of `{}`: the hinge axis is zero", limit.bone));
                    }
                    if h.min > h.max {
                        return Err(format!("limit of `{}`: min is above max", limit.bone));
                    }
                }
                (None, Some(c)) => {
                    if !(0.0..=180.0).contains(&c) {
                        return Err(format!(
                            "limit of `{}`: the cone is 0..180 degrees",
                            limit.bone
                        ));
                    }
                }
            }
        }
        for chain in &self.chains {
            if chain.bones == 0 || chain.bones > 32 {
                return Err(format!("chain `{}`: 1 to 32 bones", chain.name));
            }
        }
        Ok(())
    }
}

/// How far a joint may turn (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JointLimit {
    /// About `axis` (the bone's own space), `min..max` radians from rest.
    Hinge { axis: Vec3, min: f32, max: f32 },
    /// The bone within this many radians of its rest direction.
    Cone(f32),
}

impl JointLimit {
    pub fn from_desc(desc: &LimitDesc) -> Option<Self> {
        match (&desc.hinge, desc.cone) {
            (Some(h), _) => Some(JointLimit::Hinge {
                axis: Vec3::from(h.axis).normalize_or(Vec3::X),
                min: h.min.to_radians(),
                max: h.max.to_radians(),
            }),
            (None, Some(c)) => Some(JointLimit::Cone(c.to_radians())),
            (None, None) => None,
        }
    }

    /// `local` (the joint's rotation in its parent) kept within the limit. `rest`: its rotation
    /// at rest; `child`: where its child sits in its own space (the bone's direction).
    pub fn apply(&self, local: Quat, rest: Quat, child: Vec3) -> Quat {
        match *self {
            JointLimit::Hinge { axis, min, max } => {
                // The turn about the axis (twist) kept and clamped; turns about others dropped.
                let relative = rest.inverse() * local;
                let v = Vec3::new(relative.x, relative.y, relative.z);
                let along = axis * v.dot(axis);
                let twist = Quat::from_xyzw(along.x, along.y, along.z, relative.w);
                let twist = if twist.length_squared() < 1e-12 {
                    Quat::IDENTITY
                } else {
                    twist.normalize()
                };
                let (twist_axis, mut angle) = twist.to_axis_angle();
                if twist_axis.dot(axis) < 0.0 {
                    angle = -angle;
                }
                // to_axis_angle gives 0..2π: back to -π..π.
                if angle > std::f32::consts::PI {
                    angle -= std::f32::consts::TAU;
                } else if angle < -std::f32::consts::PI {
                    angle += std::f32::consts::TAU;
                }
                rest * Quat::from_axis_angle(axis, angle.clamp(min, max))
            }
            JointLimit::Cone(max) => {
                let Some(bone) = child.try_normalize() else {
                    return local;
                };
                let at_rest = rest * bone;
                let now = local * bone;
                let angle = at_rest.angle_between(now);
                if angle <= max {
                    return local;
                }
                // Back onto the cone's edge, in the plane of the two directions.
                let axis = at_rest
                    .cross(now)
                    .try_normalize()
                    .unwrap_or_else(|| at_rest.any_orthonormal_vector());
                let edge = Quat::from_axis_angle(axis, max) * at_rest;
                Quat::from_rotation_arc(now, edge) * local
            }
        }
    }
}

/// A rig resolved on a model: its humanoid parts and each bone's limit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rig {
    /// The file it was read from (`None`: made in code).
    pub file: Option<String>,
    pub desc: RigFile,
}

impl Rig {
    /// The humanoid parts of the skeleton under `model`, with this rig's `[bones]` applied.
    pub fn humanoid(&self, tree: &SceneTree, model: NodeHandle) -> HumanoidMap {
        crate::ragdoll::humanoid_map(tree, model, &self.desc.bones)
    }

    /// The bone `name` names under `model`: a bone of that name, else a humanoid part.
    pub fn bone(&self, tree: &SceneTree, model: NodeHandle, name: &str) -> Option<NodeHandle> {
        let bones = skeleton_bones(tree, model);
        bones
            .iter()
            .copied()
            .find(|&b| tree.name(b).is_some_and(|n| *n == *name))
            .or_else(|| {
                let part = HumanBone::from_key(name)?;
                self.humanoid(tree, model).get(part)
            })
    }

    /// The limit of `bone`, if the rig gives it one.
    pub fn limit(
        &self,
        tree: &SceneTree,
        model: NodeHandle,
        bone: NodeHandle,
    ) -> Option<JointLimit> {
        self.desc
            .limits
            .iter()
            .find(|l| self.bone(tree, model, &l.bone) == Some(bone))
            .and_then(JointLimit::from_desc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rig_files_round_trip_and_are_checked() {
        let text = "[bones]\nleft_hand = \"hand_l\"\n\n[[limit]]\nbone = \"calf_l\"\nhinge = { axis = [1.0, 0.0, 0.0], min = 0.0, max = 150.0 }\n\n[[limit]]\nbone = \"neck\"\ncone = 50.0\n\n[[chain]]\nname = \"left_leg\"\ntip = \"left_foot\"\n";
        let rig = RigFile::from_toml(text).unwrap();
        assert_eq!(rig.chains[0].bones, 2);
        assert_eq!(RigFile::from_toml(&rig.to_toml()).unwrap(), rig);
        assert!(RigFile::from_toml("[bones]\ntail = \"t\"\n")
            .unwrap_err()
            .contains("tail"));
        assert!(RigFile::from_toml("[[limit]]\nbone = \"a\"\n")
            .unwrap_err()
            .contains("hinge or a cone"));
        assert!(RigFile::from_toml(
            "[[limit]]\nbone = \"a\"\nhinge = { axis = [1.0, 0.0, 0.0], min = 10.0, max = 0.0 }\n"
        )
        .is_err());
    }

    #[test]
    fn hinges_keep_their_axis_and_range_and_cones_their_angle() {
        let knee = JointLimit::Hinge {
            axis: Vec3::X,
            min: 0.0,
            max: 90f32.to_radians(),
        };
        // Bent 120° about the axis, and twisted about another: 90° about the axis only.
        let bent = Quat::from_rotation_x(120f32.to_radians()) * Quat::from_rotation_y(0.4);
        let kept = knee.apply(bent, Quat::IDENTITY, Vec3::NEG_Y);
        assert!(
            kept.angle_between(Quat::from_rotation_x(90f32.to_radians())) < 1e-3,
            "{kept}"
        );
        // Backwards: the knee stays straight.
        let back = knee.apply(Quat::from_rotation_x(-0.5), Quat::IDENTITY, Vec3::NEG_Y);
        assert!(back.angle_between(Quat::IDENTITY) < 1e-3);
        // Inside the range: unchanged.
        let fine = Quat::from_rotation_x(0.7);
        assert!(
            knee.apply(fine, Quat::IDENTITY, Vec3::NEG_Y)
                .angle_between(fine)
                < 1e-4
        );

        let neck = JointLimit::Cone(30f32.to_radians());
        let far = Quat::from_rotation_z(80f32.to_radians());
        let kept = neck.apply(far, Quat::IDENTITY, Vec3::Y);
        let angle = Vec3::Y.angle_between(kept * Vec3::Y).to_degrees();
        assert!((angle - 30.0).abs() < 0.1, "{angle}");
    }
}
