use super::context::{GpuContext, RenderError, RendererConfig};
use super::scene_renderer::{FrameStats, SceneRenderer};
use ab_ecs::World;
use std::sync::Arc;
use tracing::{debug, info, warn};
use winit::window::Window;

/// Renders the scene to a window surface.
#[derive(Debug)]
pub struct WindowRenderer {
    gpu: GpuContext,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    scene: SceneRenderer,
    window: Arc<Window>,
}

impl WindowRenderer {
    pub fn new(window: Arc<Window>, config: &RendererConfig) -> Result<Self, RenderError> {
        pollster::block_on(Self::new_async(window, config))
    }

    pub async fn new_async(
        window: Arc<Window>,
        config: &RendererConfig,
    ) -> Result<Self, RenderError> {
        let instance = GpuContext::create_instance();
        let surface = instance.create_surface(window.clone())?;
        let gpu = GpuContext::new(instance, Some(&surface)).await?;

        let capabilities = surface.get_capabilities(&gpu.adapter);
        // Prefer an sRGB format so the hardware encodes gamma; post.wgsl handles the others.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .ok_or(RenderError::SurfaceUnsupported)?;
        let present_mode = if config.vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        let size = window.inner_size();
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.clamp(1, gpu.max_texture_size()),
            height: size.height.clamp(1, gpu.max_texture_size()),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&gpu.device, &surface_config);
        let scene = SceneRenderer::new(&gpu, format, config);
        info!(
            ?format,
            ?present_mode,
            msaa = scene.msaa_samples(),
            "renderer ready"
        );
        Ok(Self {
            gpu,
            surface,
            surface_config,
            scene,
            window,
        })
    }

    pub fn gpu(&self) -> &GpuContext {
        &self.gpu
    }

    pub fn scene_renderer(&self) -> &SceneRenderer {
        &self.scene
    }

    /// Call when the window's client area changes size (physical pixels).
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return; // minimised: keep the old configuration, rendering is skipped
        }
        // A window spanning several monitors can be larger than the GPU's textures.
        let max = self.gpu.max_texture_size();
        let (width, height) = (width.min(max), height.min(max));
        if (width, height) == (self.surface_config.width, self.surface_config.height) {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface
            .configure(&self.gpu.device, &self.surface_config);
    }

    /// Renders and presents one frame. Returns `Ok(None)` when the frame was skipped
    /// (minimised window, surface being recreated).
    pub fn render(&mut self, world: &World) -> Result<Option<FrameStats>, RenderError> {
        if self.gpu.is_device_lost() {
            return Err(RenderError::DeviceLost);
        }
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(None);
        }
        self.resize(size.width, size.height);
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                debug!("surface lost or outdated; reconfiguring");
                self.surface
                    .configure(&self.gpu.device, &self.surface_config);
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Occluded => return Ok(None),
            wgpu::CurrentSurfaceTexture::Timeout => {
                warn!("timed out acquiring the next frame; skipping it");
                return Ok(None);
            }
            // The uncaptured-error handler has logged it.
            wgpu::CurrentSurfaceTexture::Validation => return Ok(None),
        };
        let view = frame.texture.create_view(&Default::default());
        let stats = self.scene.render(
            &self.gpu,
            world,
            &view,
            self.surface_config.width,
            self.surface_config.height,
        );
        self.window.pre_present_notify();
        self.gpu.queue.present(frame);
        Ok(Some(stats))
    }
}
