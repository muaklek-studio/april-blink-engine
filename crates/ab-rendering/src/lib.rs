//! Forward renderer built on wgpu.
//!
//! Data flow each frame:
//!
//! 1. [`extract`] walks the scene (ECS world) and builds a GPU-agnostic [`RenderList`]:
//!    active camera, visible meshes (frustum-culled), lights.
//! 2. [`SceneRenderer`] uploads per-frame data and records the passes:
//!    shadow maps (directional + up to 4 spot lights) → HDR forward pass (textured PBR,
//!    normal maps, GPU skinning, height fog, MSAA) → post-process (exposure, ACES tonemap,
//!    grading, vignette).
//! 3. [`WindowRenderer`] presents to the window surface; [`OffscreenTarget`] renders to a
//!    texture that can be read back (tests, screenshots).
//!
//! Game code registers meshes/materials in the [`RenderAssets`] resource, tweaks the
//! [`Environment`] resource and creates nodes through [`RenderNodes`].

mod assets;
mod bounds;
mod camera;
mod components;
mod debug_draw;
mod environment;
mod extract;
mod gpu;
mod material;
mod mesh;
mod nodes;
mod particles;
mod shadow;
mod texture;
mod ui;

pub use assets::{MaterialHandle, MeshHandle, RenderAssets, TextureHandle};
pub use bounds::{entity_mesh_bounds, for_each_mesh_bounds, posed_bounds, Bounds, MeshBounds};
pub use camera::{CameraView, Frustum};
pub use components::{
    Camera3D, DirectionalLight3D, MeshInstance3D, OmniLight3D, Skin, SpotLight3D,
};
pub use debug_draw::{debug_draw_system, DebugBrush, DebugDraw, DebugLine, MAX_DEBUG_LINES};
pub use environment::{
    Bloom, Environment, Fog, LightShafts, PostProcess, ScreenOverlay, ShadowSettings,
};
pub use extract::{
    extract, CameraOverride, DirectionalLightData, DrawItem, OmniLightData, RenderList,
    SpotLightData,
};
pub use gpu::{
    backend_name, instance_flags, test_gpu_context, AdapterCandidate, FrameStats, GpuContext,
    OffscreenTarget, RenderError, RendererConfig, SceneRenderer, WindowRenderer,
    DEFAULT_TEXTURE_UPLOAD_BUDGET,
};
pub use material::Material;
pub use mesh::{Aabb, Mesh, MeshError, SkinWeights, Vertex};
pub use nodes::{class, RenderNodes};
pub use particles::{ParticleDrawList, ParticleInstance};
pub use shadow::{directional_shadow_matrix, spot_shadow_matrix};
pub use texture::{
    ColorSpace, MipLevel, SamplerSettings, Texture, TextureError, TextureFilter, TextureWrap,
};
pub use ui::{GlyphAtlasImage, UiDrawList, UiFill, UiQuad};

/// Light limits of the forward shader. Extra lights (farthest from the camera) are dropped.
pub mod limits {
    pub const MAX_OMNI_LIGHTS: usize = 16;
    pub const MAX_SPOT_LIGHTS: usize = 8;
    /// Spot lights (nearest first) that get a shadow map.
    pub const MAX_SPOT_SHADOWS: usize = 4;
    /// Shadow-map layers: one directional + the spot shadows.
    pub const SHADOW_LAYERS: usize = 1 + MAX_SPOT_SHADOWS;
}
