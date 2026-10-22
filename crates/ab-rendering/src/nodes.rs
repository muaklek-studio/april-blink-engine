//! Godot-style constructors for render nodes.

use crate::components::{Camera3D, DirectionalLight3D, MeshInstance3D, OmniLight3D, SpotLight3D};
use ab_scene::{NodeHandle, SceneTree, Transform};

pub mod class {
    pub const CAMERA_3D: &str = "Camera3D";
    pub const MESH_INSTANCE_3D: &str = "MeshInstance3D";
    pub const DIRECTIONAL_LIGHT_3D: &str = "DirectionalLight3D";
    pub const OMNI_LIGHT_3D: &str = "OmniLight3D";
    pub const SPOT_LIGHT_3D: &str = "SpotLight3D";
}

/// Adds `create_camera3d`, `create_mesh_instance3d`, ... to [`SceneTree`]. Every node is a
/// detached `Node3D` carrying the render component; attach it with `add_child`.
pub trait RenderNodes {
    fn create_camera3d(&mut self, name: &str, camera: Camera3D) -> NodeHandle;
    fn create_mesh_instance3d(&mut self, name: &str, instance: MeshInstance3D) -> NodeHandle;
    fn create_directional_light3d(&mut self, name: &str, light: DirectionalLight3D) -> NodeHandle;
    fn create_omni_light3d(&mut self, name: &str, light: OmniLight3D) -> NodeHandle;
    fn create_spot_light3d(&mut self, name: &str, light: SpotLight3D) -> NodeHandle;
}

impl RenderNodes for SceneTree {
    fn create_camera3d(&mut self, name: &str, camera: Camera3D) -> NodeHandle {
        self.spawn_node(name, class::CAMERA_3D, (Transform::default(), camera))
    }

    fn create_mesh_instance3d(&mut self, name: &str, instance: MeshInstance3D) -> NodeHandle {
        self.spawn_node(
            name,
            class::MESH_INSTANCE_3D,
            (Transform::default(), instance),
        )
    }

    fn create_directional_light3d(&mut self, name: &str, light: DirectionalLight3D) -> NodeHandle {
        self.spawn_node(
            name,
            class::DIRECTIONAL_LIGHT_3D,
            (Transform::default(), light),
        )
    }

    fn create_omni_light3d(&mut self, name: &str, light: OmniLight3D) -> NodeHandle {
        self.spawn_node(name, class::OMNI_LIGHT_3D, (Transform::default(), light))
    }

    fn create_spot_light3d(&mut self, name: &str, light: SpotLight3D) -> NodeHandle {
        self.spawn_node(name, class::SPOT_LIGHT_3D, (Transform::default(), light))
    }
}
