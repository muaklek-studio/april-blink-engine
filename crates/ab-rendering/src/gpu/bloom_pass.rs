//! Bloom: bright parts of the HDR image blurred over a half-resolution mip chain.

use super::uniforms::BloomGpu;
use crate::environment::Bloom;
use std::num::NonZeroU64;

/// Most mips in the chain (half resolution down to 1/64).
const MAX_MIPS: u32 = 6;

/// Textures and bind groups for one target size.
struct BloomChain {
    size: (u32, u32),
    /// One view per mip; `views[0]` is the result the post pass reads.
    views: Vec<wgpu::TextureView>,
    /// Samples the HDR image (prefilter into mip 0).
    prefilter: wgpu::BindGroup,
    /// `sample[i]` samples mip `i`.
    sample: Vec<wgpu::BindGroup>,
}

pub(crate) struct BloomPass {
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    prefilter: wgpu::RenderPipeline,
    downsample: wgpu::RenderPipeline,
    upsample: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
    chain: Option<BloomChain>,
}

/// Mips for a half-resolution base of `width` × `height` (every mip at least 1 × 1).
pub(crate) fn mip_count(width: u32, height: u32) -> u32 {
    let smallest = (width / 2).max(1).min((height / 2).max(1));
    (32 - smallest.leading_zeros()).clamp(1, MAX_MIPS)
}

impl BloomPass {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(size_of::<BloomGpu>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bloom.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/bloom.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bloom pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |label, entry_point, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_fullscreen"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let additive = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let prefilter = pipeline("bloom prefilter", "fs_prefilter", None);
        let downsample = pipeline("bloom downsample", "fs_downsample", None);
        let upsample = pipeline(
            "bloom upsample",
            "fs_upsample",
            Some(wgpu::BlendState {
                color: additive,
                alpha: additive,
            }),
        );
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bloom uniforms"),
            size: size_of::<BloomGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bloom sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            format,
            layout,
            prefilter,
            downsample,
            upsample,
            uniform,
            sampler,
            chain: None,
        }
    }

    /// (Re)creates the mip chain for a `width` × `height` HDR image and returns the view the
    /// post pass samples.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        hdr_view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> &wgpu::TextureView {
        let mips = mip_count(width, height);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bloom chain"),
            size: wgpu::Extent3d {
                width: (width / 2).max(1),
                height: (height / 2).max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let views: Vec<_> = (0..mips)
            .map(|mip| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("bloom mip"),
                    base_mip_level: mip,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let bind = |view: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bloom bind group"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.uniform.as_entire_binding(),
                    },
                ],
            })
        };
        let prefilter = bind(hdr_view);
        let sample = views.iter().map(bind).collect();
        let chain = self.chain.insert(BloomChain {
            size: (width, height),
            views,
            prefilter,
            sample,
        });
        &chain.views[0]
    }

    pub fn write_uniforms(&self, queue: &wgpu::Queue, settings: &Bloom) {
        let threshold = if settings.threshold.is_finite() {
            settings.threshold.max(0.0)
        } else {
            1.0
        };
        let gpu = BloomGpu {
            params: [threshold, settings.soft_knee.clamp(0.0, 1.0), 1.0, 0.0],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&gpu));
    }

    /// Prefilter → downsample chain → additive upsamples back to mip 0.
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(chain) = &self.chain else {
            return;
        };
        debug_assert!(chain.size.0 > 0 && chain.size.1 > 0);
        let pass = |encoder: &mut wgpu::CommandEncoder,
                    pipeline: &wgpu::RenderPipeline,
                    source: &wgpu::BindGroup,
                    target: &wgpu::TextureView,
                    clear: bool| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bloom pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if clear {
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, source, &[]);
            pass.draw(0..3, 0..1);
        };
        pass(
            encoder,
            &self.prefilter,
            &chain.prefilter,
            &chain.views[0],
            true,
        );
        let mips = chain.views.len();
        for mip in 1..mips {
            pass(
                encoder,
                &self.downsample,
                &chain.sample[mip - 1],
                &chain.views[mip],
                true,
            );
        }
        for mip in (1..mips).rev() {
            pass(
                encoder,
                &self.upsample,
                &chain.sample[mip],
                &chain.views[mip - 1],
                false,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mip_count;

    #[test]
    fn mip_chain_fits_any_target() {
        assert_eq!(mip_count(1, 1), 1);
        assert_eq!(mip_count(2, 2), 1);
        assert_eq!(mip_count(8, 8), 3);
        assert_eq!(mip_count(1920, 1080), 6);
        assert_eq!(mip_count(4000, 3), 1);
        // Every mip of the half-resolution base is at least one texel.
        for (w, h) in [(3, 7), (64, 64), (1280, 720), (17, 9000)] {
            let base = (w / 2).max(1).min((h / 2).max(1));
            assert!(base >> (mip_count(w, h) - 1) >= 1, "{w}x{h}");
        }
    }
}
