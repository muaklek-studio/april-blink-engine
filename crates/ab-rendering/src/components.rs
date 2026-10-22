//! Render components attached to `Node3D`s. Names and defaults follow Godot.

use crate::assets::{MaterialHandle, MeshHandle};
use ab_core::Color;

/// Perspective camera looking down its node's -Z axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera3D {
    /// Vertical field of view in degrees.
    pub fov_y_degrees: f32,
    pub near: f32,
    pub far: f32,
    /// The active camera. If several are current, the first in tree order wins; if none is,
    /// the first camera found is used.
    pub current: bool,
}

impl Default for Camera3D {
    fn default() -> Self {
        Self {
            fov_y_degrees: 75.0,
            near: 0.05,
            far: 1000.0,
            current: false,
        }
    }
}

/// Draws a mesh with a material at its node's global transform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshInstance3D {
    pub mesh: MeshHandle,
    pub material: MaterialHandle,
    pub cast_shadows: bool,
}

impl MeshInstance3D {
    pub fn new(mesh: MeshHandle, material: MaterialHandle) -> Self {
        Self {
            mesh,
            material,
            cast_shadows: true,
        }
    }
}

/// Sun-like light shining along its node's -Z axis. Only the first one in the tree is used.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DirectionalLight3D {
    pub color: Color,
    pub energy: f32,
    pub shadow_enabled: bool,
}

impl Default for DirectionalLight3D {
    fn default() -> Self {
        Self {
            color: Color::WHITE,
            energy: 1.0,
            shadow_enabled: false,
        }
    }
}

/// Point light radiating in every direction, fading to zero at `range`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OmniLight3D {
    pub color: Color,
    pub energy: f32,
    pub range: f32,
}

impl Default for OmniLight3D {
    fn default() -> Self {
        Self {
            color: Color::WHITE,
            energy: 1.0,
            range: 5.0,
        }
    }
}

/// Cone light shining along its node's -Z axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpotLight3D {
    pub color: Color,
    pub energy: f32,
    pub range: f32,
    /// Half-angle of the cone in degrees (Godot's `spot_angle`).
    pub angle_degrees: f32,
    /// Fraction of the cone (0..1) over which the edge fades out.
    pub edge_softness: f32,
    pub shadow_enabled: bool,
}

impl Default for SpotLight3D {
    fn default() -> Self {
        Self {
            color: Color::WHITE,
            energy: 1.0,
            range: 5.0,
            angle_degrees: 45.0,
            edge_softness: 0.2,
            shadow_enabled: false,
        }
    }
}

/// Skeleton binding for a skinned [`MeshInstance3D`] (its mesh must carry skin weights).
///
/// Vertex joint index `i` refers to `joints[i]`. Skinned vertices are placed by the joints'
/// global transforms (the mesh node's own transform is ignored, as in glTF), and skinned
/// meshes are never frustum-culled.
#[derive(Debug, Clone, PartialEq)]
pub struct Skin {
    pub joints: Vec<ab_scene::NodeHandle>,
    /// Bind-pose inverse of each joint's global transform (same length as `joints`).
    pub inverse_bind_matrices: Vec<ab_core::Mat4>,
}
