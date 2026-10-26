//! `PostVolume` nodes: a box that changes the post-processing while the camera is in or
//! near it (see [`ab_rendering::PostVolume`]).
//!
//! ```toml
//! [[node]]
//! name = "Cave"
//! type = "PostVolume"
//! position = [0.0, 2.0, -30.0]
//! post_volume = { size = [20.0, 8.0, 30.0], blend = 3.0, exposure = 0.6, tint = "#a0b8ff", motion_blur = 0.5 }
//! ```

use super::format::{ColorDesc, NodeDesc};
use ab_core::Vec3;
use ab_rendering::{PostOverrides, PostVolume};
use ab_scene::{NodeHandle, SceneTree};
use serde::{Deserialize, Serialize};

/// The class of post-processing volume nodes.
pub const POST_VOLUME: &str = "PostVolume";

/// `post_volume = { ... }`: the box, how it blends, and the values it sets (the keys are
/// the `[environment]` ones; those left out keep the scene's values).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PostVolumeDesc {
    /// The box's size in metres (the node's scale multiplies it).
    pub size: [f32; 3],
    /// Metres outside the box over which it fades out.
    pub blend: f32,
    #[serde(skip_serializing_if = "is_zero")]
    pub priority: i32,
    /// 0..1: how much it counts at most.
    #[serde(skip_serializing_if = "is_one")]
    pub weight: f32,
    /// Counts everywhere.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub global: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exposure: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contrast: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<ColorDesc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vignette: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bloom: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bloom_threshold: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chromatic_aberration: Option<f32>,
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
}

fn is_zero(v: &i32) -> bool {
    *v == 0
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

impl Default for PostVolumeDesc {
    fn default() -> Self {
        Self {
            size: [10.0, 10.0, 10.0],
            blend: 2.0,
            priority: 0,
            weight: 1.0,
            global: false,
            exposure: None,
            contrast: None,
            saturation: None,
            tint: None,
            vignette: None,
            bloom: None,
            bloom_threshold: None,
            chromatic_aberration: None,
            grain: None,
            focus_distance: None,
            focus_range: None,
            depth_of_field: None,
            motion_blur: None,
        }
    }
}

/// The description a volume node was made from (saved back as it was written).
#[derive(Debug, Clone, PartialEq)]
pub struct PostVolumeNode(pub PostVolumeDesc);

impl PostVolumeDesc {
    /// The renderer's volume, checked.
    pub fn volume(&self) -> Result<PostVolume, String> {
        let finite = |name: &str, v: Option<f32>| -> Result<Option<f32>, String> {
            match v {
                Some(v) if !v.is_finite() => Err(format!("post_volume.{name} must be a number")),
                v => Ok(v),
            }
        };
        if self.size.iter().any(|s| !s.is_finite() || *s < 0.0) {
            return Err("post_volume.size must be three sizes of 0 or more".into());
        }
        Ok(PostVolume {
            half_extents: Vec3::from(self.size) * 0.5,
            blend_distance: finite("blend", Some(self.blend))?.unwrap_or(0.0).max(0.0),
            priority: self.priority,
            weight: finite("weight", Some(self.weight))?
                .unwrap_or(1.0)
                .clamp(0.0, 1.0),
            global: self.global,
            overrides: PostOverrides {
                exposure: finite("exposure", self.exposure)?.map(|v| v.max(0.0)),
                contrast: finite("contrast", self.contrast)?.map(|v| v.max(0.0)),
                saturation: finite("saturation", self.saturation)?.map(|v| v.max(0.0)),
                tint: self.tint.as_ref().map(ColorDesc::to_color).transpose()?,
                vignette_intensity: finite("vignette", self.vignette)?.map(|v| v.clamp(0.0, 1.0)),
                bloom_intensity: finite("bloom", self.bloom)?.map(|v| v.max(0.0)),
                bloom_threshold: finite("bloom_threshold", self.bloom_threshold)?,
                chromatic_aberration: finite("chromatic_aberration", self.chromatic_aberration)?,
                grain: finite("grain", self.grain)?,
                focus_distance: finite("focus_distance", self.focus_distance)?,
                focus_range: finite("focus_range", self.focus_range)?,
                max_blur: finite("depth_of_field", self.depth_of_field)?.map(|v| v.max(0.0)),
                motion_blur: finite("motion_blur", self.motion_blur)?.map(|v| v.max(0.0)),
            },
        })
    }
}

pub(super) fn create(
    tree: &mut SceneTree,
    name: &str,
    desc: &NodeDesc,
) -> Result<NodeHandle, String> {
    let v = desc.post_volume.clone().unwrap_or_default();
    let volume = v.volume()?;
    let node = tree.create_node3d(name);
    tree.set_class_name(node, POST_VOLUME)
        .map_err(|e| e.to_string())?;
    tree.insert_component(node, volume)
        .map_err(|e| e.to_string())?;
    tree.insert_component(node, PostVolumeNode(v))
        .map_err(|e| e.to_string())?;
    Ok(node)
}

pub(super) fn describe(tree: &SceneTree, node: NodeHandle, desc: &mut NodeDesc) {
    desc.post_volume = tree.component::<PostVolumeNode>(node).map(|v| v.0.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_check_their_values() {
        let desc: PostVolumeDesc =
            toml::from_str("size = [4.0, 2.0, 6.0]\nexposure = 0.5\ndepth_of_field = 8.0\n")
                .unwrap();
        let volume = desc.volume().unwrap();
        assert_eq!(volume.half_extents, Vec3::new(2.0, 1.0, 3.0));
        assert_eq!(volume.overrides.exposure, Some(0.5));
        assert_eq!(volume.overrides.max_blur, Some(8.0));
        assert_eq!(volume.blend_distance, 2.0, "default blend");
        let bad = PostVolumeDesc {
            size: [-1.0, 1.0, 1.0],
            ..Default::default()
        };
        assert!(bad.volume().is_err());
        assert!(toml::from_str::<PostVolumeDesc>("glow = 1.0").is_err());
    }
}
