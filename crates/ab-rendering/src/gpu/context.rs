use super::adapter;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tracing::{error, info, warn};

/// Validation errors logged in full before they are only counted (one bad draw repeats every
/// frame).
const LOGGED_GPU_ERRORS: u32 = 8;

/// Renderer start-up settings.
#[derive(Debug, Clone, PartialEq)]
pub struct RendererConfig {
    /// Multisample anti-aliasing: 1 (off) or 4. Falls back to 1 if unsupported.
    pub msaa_samples: u32,
    /// Wait for the display's vertical blank (no tearing, frame rate capped to refresh).
    pub vsync: bool,
    /// Resolution of each shadow-map layer (square).
    pub shadow_map_size: u32,
    /// Most texture data (bytes) prepared and uploaded to the GPU in one frame; `None`
    /// uploads every new texture whole at once. With a budget, textures stream: each first
    /// arrives as a small, blurry version (its mips up to 128 px), then sharpens a mip
    /// level at a time over the next frames, so a level bringing many large textures never
    /// freezes on one frame. Some progress is made every frame, whatever the budget.
    pub texture_upload_budget: Option<u64>,
}

/// A texture upload budget that keeps frames smooth: about three 2048² textures a frame.
pub const DEFAULT_TEXTURE_UPLOAD_BUDGET: u64 = 64 * 1024 * 1024;

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            msaa_samples: 4,
            vsync: true,
            shadow_map_size: 2048,
            texture_upload_budget: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("no compatible GPU adapter found (is a Vulkan, DirectX 12, Metal or OpenGL driver installed?)")]
    NoAdapter,
    #[error("failed to open the GPU device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    #[error("failed to create the window surface: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
    #[error("the window surface is not supported by the selected GPU")]
    SurfaceUnsupported,
    #[error("the GPU ran out of memory")]
    OutOfMemory,
    #[error("failed to read back GPU data: {0}")]
    Readback(String),
    #[error("the GPU device was lost (driver reset or crash)")]
    DeviceLost,
}

/// Instance, adapter, device and queue. wgpu handles are reference counted, so a host
/// application, such as the editor's UI, can render with the same device
/// ([`GpuContext::from_shared`]).
#[derive(Debug, Clone)]
pub struct GpuContext {
    /// `None` when the device came from a host application.
    pub instance: Option<wgpu::Instance>,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Set by the device-lost callback.
    lost: Arc<AtomicBool>,
}

impl GpuContext {
    pub(crate) fn create_instance() -> wgpu::Instance {
        // Surfaces are created from winit windows, which bring their own display handle.
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = adapter::backends();
        descriptor.flags = instance_flags();
        wgpu::Instance::new(descriptor)
    }

    /// Opens the best adapter of any backend (see [`adapter`](super::adapter)), compatible
    /// with `surface` when given; the next best when a device cannot be opened on it.
    pub async fn new(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self, RenderError> {
        let adapters = instance.enumerate_adapters(adapter::backends()).await;
        let ranked = adapter::rank(adapters, surface);
        for (n, (_, candidate)) in ranked.iter().enumerate() {
            info!(
                rank = n + 1,
                score = candidate.score(),
                "GPU found: {candidate}"
            );
        }
        let mut last_error = None;
        for (adapter, candidate) in ranked {
            let limits = adapter::required_limits(&adapter.limits());
            let opened = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("ab-rendering device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: limits,
                    memory_hints: wgpu::MemoryHints::Performance,
                    ..Default::default()
                })
                .await;
            match opened {
                Ok((device, queue)) => {
                    let adapter_info = adapter.get_info();
                    info!(
                        name = adapter_info.name,
                        backend = ?adapter_info.backend,
                        driver = adapter_info.driver,
                        "GPU adapter selected: {candidate}"
                    );
                    let lost = Arc::new(AtomicBool::new(false));
                    install_error_handlers(&device, &lost);
                    return Ok(Self {
                        instance: Some(instance),
                        adapter,
                        device,
                        queue,
                        lost,
                    });
                }
                Err(err) => {
                    warn!("cannot open {candidate}: {err}; trying the next GPU");
                    last_error = Some(err);
                }
            }
        }
        Err(last_error.map_or(RenderError::NoAdapter, RenderError::RequestDevice))
    }

    /// The adapter in words: `NVIDIA GeForce RTX 3060 · Vulkan · dedicated GPU`.
    pub fn adapter_summary(&self) -> String {
        adapter::Candidate::of(&self.adapter, None, None).to_string()
    }

    /// True once the driver reported the device lost; the renderer must be recreated.
    pub fn is_device_lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }

    /// Largest texture width or height this device accepts.
    pub fn max_texture_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// True when the adapter is a software rasterizer: Windows' WARP, Linux' llvmpipe.
    ///
    /// Such an adapter draws every pixel on the CPU, and some of them are broken enough to
    /// crash the process instead of drawing (gfx-rs/wgpu#5591).
    pub fn is_software_adapter(&self) -> bool {
        self.adapter.get_info().device_type == wgpu::DeviceType::Cpu
    }

    /// Renders with a device another part of the application opened (e.g. the editor's UI
    /// toolkit). The device must support the default limits.
    pub fn from_shared(adapter: wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            instance: None,
            adapter,
            device,
            queue,
            lost: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Opens a device without any window (tests, tools, screenshots).
    pub fn new_headless() -> Result<Self, RenderError> {
        pollster::block_on(Self::new(Self::create_instance(), None))
    }
}

/// Instance flags: wgpu's debug labels in debug builds, but not the driver's validation layer
/// (`WGPU_VALIDATION=1` turns it on). wgpu checks every call itself in all builds; the Vulkan
/// layer adds slow driver-level checks that are only worth it when chasing a driver issue.
pub fn instance_flags() -> wgpu::InstanceFlags {
    let mut flags = wgpu::InstanceFlags::from_build_config();
    flags.remove(wgpu::InstanceFlags::VALIDATION);
    flags.with_env()
}

/// Opens a headless GPU context for the offscreen rendering tests, or `None` when the machine
/// has no GPU to render on.
///
/// CI runners have no graphics hardware: Windows offers WARP, a software rasterizer that
/// crashes the driver instead of drawing (gfx-rs/wgpu#5591), and Linux has no Vulkan driver
/// at all. Software adapters are therefore skipped; set `AB_TEST_SOFTWARE_GPU=1` to run the
/// tests on one anyway.
pub fn test_gpu_context() -> Option<GpuContext> {
    match GpuContext::new_headless() {
        Ok(gpu) if gpu.is_software_adapter() && !software_gpu_allowed() => {
            let info = gpu.adapter.get_info();
            eprintln!(
                "skipping GPU test: only a software adapter is available ({} · {:?})",
                info.name.trim(),
                info.backend
            );
            None
        }
        Ok(gpu) => Some(gpu),
        Err(RenderError::NoAdapter) => {
            eprintln!("skipping GPU test: no adapter available");
            None
        }
        Err(e) => panic!("GPU initialisation failed: {e}"),
    }
}

/// Whether `AB_TEST_SOFTWARE_GPU` asks for the GPU tests to run on a software adapter.
fn software_gpu_allowed() -> bool {
    std::env::var_os("AB_TEST_SOFTWARE_GPU").is_some_and(|value| value != "0")
}

/// wgpu panics on uncaptured errors by default, which would take the whole game down for one
/// bad resource; log them instead (the frame may draw wrongly, the game keeps running), and
/// remember when the device is lost so the renderer can be rebuilt.
fn install_error_handlers(device: &wgpu::Device, lost: &Arc<AtomicBool>) {
    let count = AtomicU32::new(0);
    device.on_uncaptured_error(Arc::new(move |err| {
        let n = count.fetch_add(1, Ordering::Relaxed);
        if n < LOGGED_GPU_ERRORS {
            error!("GPU error: {err}");
        } else if n == LOGGED_GPU_ERRORS || n.is_multiple_of(1000) {
            error!(
                total = n + 1,
                "GPU errors keep happening; only counting them now"
            );
        }
    }));
    let lost = lost.clone();
    device.set_device_lost_callback(move |reason, message| {
        // Shutting down (dropping or destroying the device) is not a failure.
        if matches!(reason, wgpu::DeviceLostReason::Unknown) {
            error!(?reason, "GPU device lost: {message}");
            lost.store(true, Ordering::Relaxed);
        }
    });
}
