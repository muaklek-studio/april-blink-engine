use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tracing::{error, info};

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
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            msaa_samples: 4,
            vsync: true,
            shadow_map_size: 2048,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("no compatible GPU adapter found (is a Vulkan/DX12/Metal driver installed?)")]
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
        descriptor.backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY);
        descriptor.flags = instance_flags();
        wgpu::Instance::new(descriptor)
    }

    /// Opens the best high-performance adapter, compatible with `surface` when given.
    pub async fn new(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self, RenderError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: surface,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|_| RenderError::NoAdapter)?;
        let adapter_info = adapter.get_info();
        info!(
            name = adapter_info.name,
            backend = ?adapter_info.backend,
            driver = adapter_info.driver,
            "GPU adapter selected"
        );
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("ab-rendering device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::Performance,
                ..Default::default()
            })
            .await?;
        let lost = Arc::new(AtomicBool::new(false));
        install_error_handlers(&device, &lost);
        Ok(Self {
            instance: Some(instance),
            adapter,
            device,
            queue,
            lost,
        })
    }

    /// True once the driver reported the device lost; the renderer must be recreated.
    pub fn is_device_lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }

    /// Largest texture width or height this device accepts.
    pub fn max_texture_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
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
