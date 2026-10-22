//! GPU side of the renderer (wgpu).

mod adapter;
mod batch;
mod bloom_pass;
mod context;
mod fog_noise;
mod line_pass;
mod offscreen;
mod particle_pass;
mod scene_renderer;
mod ui_pass;
mod uniforms;
mod window;

pub use adapter::{backend_name, Candidate as AdapterCandidate};
pub use context::{instance_flags, test_gpu_context, GpuContext, RenderError, RendererConfig};
pub use offscreen::OffscreenTarget;
pub use scene_renderer::{FrameStats, SceneRenderer};
pub use window::WindowRenderer;
