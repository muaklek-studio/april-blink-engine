//! Picking the GPU and the graphics API (backend) at start-up.
//!
//! Every adapter of every backend this build supports is listed and ranked: a dedicated GPU
//! before an integrated one before a software rasterizer, then the backend that works best on
//! this OS (DX12 on Windows, Metal on macOS, Vulkan elsewhere, OpenGL last). The device is
//! opened on the best one, and on the next ones in order if that fails, so a broken driver on
//! one API does not stop the game when another API works.
//!
//! `WGPU_BACKEND` (`vulkan`, `dx12`, `metal`, `gl`, comma-separated) limits the choice, and
//! `WGPU_ADAPTER_NAME` prefers adapters whose name contains it.

use std::fmt;

/// Every backend worth trying, unless `WGPU_BACKEND` says otherwise.
pub(crate) fn backends() -> wgpu::Backends {
    wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY | wgpu::Backends::GL)
}

/// One adapter found, with what decides its rank.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub name: String,
    pub backend: wgpu::Backend,
    pub device_type: wgpu::DeviceType,
    /// Has everything the renderer needs (storage buffers in vertex shaders, the default
    /// limits). Others are only tried when nothing else exists.
    pub capable: bool,
    /// Can present to the window (true without a window).
    pub presents: bool,
    /// Its name contains `WGPU_ADAPTER_NAME`.
    pub requested: bool,
}

impl Candidate {
    /// Describes `adapter`; `surface`: the window it must present to, `wanted_name`: part of
    /// the name of the adapter the user asked for.
    pub fn of(
        adapter: &wgpu::Adapter,
        surface: Option<&wgpu::Surface<'_>>,
        wanted_name: Option<&str>,
    ) -> Self {
        let info = adapter.get_info();
        let downlevel = adapter.get_downlevel_capabilities();
        let limits = adapter.limits();
        let capable = downlevel.is_webgpu_compliant()
            || (downlevel
                .flags
                .contains(wgpu::DownlevelFlags::VERTEX_STORAGE)
                && required_limits(&limits).check_limits(&limits));
        let requested = wanted_name.is_some_and(|wanted| {
            info.name
                .to_lowercase()
                .contains(&wanted.trim().to_lowercase())
        });
        Self {
            name: info.name.trim().to_owned(),
            backend: info.backend,
            device_type: info.device_type,
            capable,
            presents: surface.is_none_or(|s| adapter.is_surface_supported(s)),
            requested,
        }
    }

    /// Higher is better; 0 means unusable.
    pub fn score(&self) -> u32 {
        if !self.presents || self.backend == wgpu::Backend::Noop {
            return 0;
        }
        let device = match self.device_type {
            wgpu::DeviceType::DiscreteGpu => 4,
            wgpu::DeviceType::IntegratedGpu => 3,
            wgpu::DeviceType::VirtualGpu | wgpu::DeviceType::Other => 2,
            wgpu::DeviceType::Cpu => 1,
        };
        let capable = if self.capable { 1000 } else { 0 };
        let requested = if self.requested { 10_000 } else { 0 };
        requested + capable + device * 10 + backend_preference(self.backend)
    }
}

impl fmt::Display for Candidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} · {} · {}",
            self.name,
            backend_name(self.backend),
            device_type_name(self.device_type)
        )
    }
}

/// How well a backend runs on this OS, 1 (last resort) to 4.
fn backend_preference(backend: wgpu::Backend) -> u32 {
    use wgpu::Backend;
    match backend {
        Backend::Dx12 if cfg!(windows) => 4,
        Backend::Metal if cfg!(target_vendor = "apple") => 4,
        Backend::Vulkan if cfg!(windows) => 3,
        Backend::Vulkan => 4,
        Backend::Gl => 1,
        _ => 2,
    }
}

/// The limits the renderer asks for: the WebGPU defaults, with textures as large as the
/// adapter allows.
pub(crate) fn required_limits(adapter: &wgpu::Limits) -> wgpu::Limits {
    wgpu::Limits::default().using_resolution(adapter.clone())
}

/// Usable adapters, best first.
pub(crate) fn rank(
    adapters: Vec<wgpu::Adapter>,
    surface: Option<&wgpu::Surface<'_>>,
) -> Vec<(wgpu::Adapter, Candidate)> {
    let wanted = std::env::var("WGPU_ADAPTER_NAME").ok();
    let mut ranked: Vec<_> = adapters
        .into_iter()
        .map(|adapter| {
            let candidate = Candidate::of(&adapter, surface, wanted.as_deref());
            (adapter, candidate)
        })
        .filter(|(_, c)| c.score() > 0)
        .collect();
    // Stable: equal scores keep wgpu's own order.
    ranked.sort_by_key(|(_, c)| std::cmp::Reverse(c.score()));
    ranked
}

pub fn backend_name(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Vulkan => "Vulkan",
        wgpu::Backend::Dx12 => "DirectX 12",
        wgpu::Backend::Metal => "Metal",
        wgpu::Backend::Gl => "OpenGL",
        wgpu::Backend::BrowserWebGpu => "WebGPU",
        wgpu::Backend::Noop => "none",
    }
}

fn device_type_name(device_type: wgpu::DeviceType) -> &'static str {
    match device_type {
        wgpu::DeviceType::DiscreteGpu => "dedicated GPU",
        wgpu::DeviceType::IntegratedGpu => "integrated GPU",
        wgpu::DeviceType::VirtualGpu => "virtual GPU",
        wgpu::DeviceType::Cpu => "software",
        wgpu::DeviceType::Other => "GPU",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::{Backend, DeviceType};

    fn candidate(backend: Backend, device_type: DeviceType) -> Candidate {
        Candidate {
            name: "GPU".into(),
            backend,
            device_type,
            capable: true,
            presents: true,
            requested: false,
        }
    }

    #[test]
    fn dedicated_gpu_wins_over_backend() {
        let discrete_gl = candidate(Backend::Gl, DeviceType::DiscreteGpu);
        let integrated_vk = candidate(Backend::Vulkan, DeviceType::IntegratedGpu);
        assert!(discrete_gl.score() > integrated_vk.score());
        let software = candidate(Backend::Vulkan, DeviceType::Cpu);
        assert!(integrated_vk.score() > software.score());
    }

    #[test]
    fn native_backend_preferred_on_the_same_gpu() {
        let vulkan = candidate(Backend::Vulkan, DeviceType::DiscreteGpu);
        let dx12 = candidate(Backend::Dx12, DeviceType::DiscreteGpu);
        let gl = candidate(Backend::Gl, DeviceType::DiscreteGpu);
        assert!(vulkan.score() > gl.score());
        assert!(dx12.score() > gl.score());
        if cfg!(windows) {
            assert!(dx12.score() > vulkan.score());
        } else {
            assert!(vulkan.score() > dx12.score());
        }
    }

    #[test]
    fn incapable_or_unpresentable_adapters_come_last() {
        let mut weak = candidate(Backend::Vulkan, DeviceType::DiscreteGpu);
        weak.capable = false;
        let software = candidate(Backend::Vulkan, DeviceType::Cpu);
        assert!(software.score() > weak.score());
        assert!(weak.score() > 0, "still tried when nothing else exists");
        let mut headless = candidate(Backend::Vulkan, DeviceType::DiscreteGpu);
        headless.presents = false;
        assert_eq!(headless.score(), 0);
    }

    #[test]
    fn requested_name_wins() {
        let mut integrated = candidate(Backend::Vulkan, DeviceType::IntegratedGpu);
        integrated.requested = true;
        assert!(integrated.score() > candidate(Backend::Vulkan, DeviceType::DiscreteGpu).score());
    }

    #[test]
    fn display() {
        assert_eq!(
            candidate(Backend::Vulkan, DeviceType::DiscreteGpu).to_string(),
            "GPU · Vulkan · dedicated GPU"
        );
    }
}
