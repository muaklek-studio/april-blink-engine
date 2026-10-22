use ab_core::Vec3;
use ab_scene::NodeHandle;

/// Filters for scene queries (raycasts, overlap tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryOptions {
    /// Only bodies on one of these layers are considered.
    pub collision_mask: u32,
    /// Bodies to ignore, typically the caster itself.
    pub exclude: Vec<NodeHandle>,
    /// Whether areas (trigger volumes) can be hit. Off by default, as in Godot.
    pub include_areas: bool,
}

impl QueryOptions {
    /// Every layer, no exclusions, no areas.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_mask(mut self, mask: u32) -> Self {
        self.collision_mask = mask;
        self
    }

    pub fn excluding(mut self, node: NodeHandle) -> Self {
        self.exclude.push(node);
        self
    }

    pub fn with_areas(mut self, include: bool) -> Self {
        self.include_areas = include;
        self
    }
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            collision_mask: u32::MAX,
            exclude: Vec::new(),
            include_areas: false,
        }
    }
}

/// Closest hit of a ray.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// Node of the body that was hit.
    pub node: NodeHandle,
    /// Node of the [`CollisionShape3D`](crate::CollisionShape3D) that was hit.
    pub shape: NodeHandle,
    /// Global hit point.
    pub position: Vec3,
    /// Surface normal at the hit point (pointing out of the hit shape).
    pub normal: Vec3,
    /// Distance from the ray origin.
    pub distance: f32,
}
