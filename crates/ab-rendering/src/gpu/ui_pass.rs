//! Draws the [`UiDrawList`] over the post-processed frame.

use super::context::GpuContext;
use crate::assets::TextureHandle;
use crate::ui::{GlyphAtlasImage, UiDrawList, UiFill};
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;
use std::mem::size_of;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct ScreenGpu {
    size: [f32; 2],
    flags: [f32; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct UiInstanceGpu {
    rect: [f32; 4],
    uv: [f32; 4],
    color: [f32; 4],
    stroke_color: [f32; 4],
    clip: [f32; 4],
    params: [f32; 4],
    kind: [u32; 4],
}

const MIN_INSTANCE_CAPACITY: usize = 256;

/// Texture a run of quads is drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    White,
    Atlas,
    Image(TextureHandle),
}

struct Atlas {
    revision: u64,
    size: u32,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

pub(crate) struct UiPass {
    pipeline: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    screen_buffer: wgpu::Buffer,
    screen_bind_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    white: wgpu::BindGroup,
    atlas: Option<Atlas>,
    images: HashMap<TextureHandle, wgpu::BindGroup>,
    instances: wgpu::Buffer,
    capacity: usize,
    scratch: Vec<UiInstanceGpu>,
    batches: Vec<(Source, std::ops::Range<u32>)>,
    /// The target stores linear values, so the shader encodes sRGB itself.
    encode_srgb: bool,
}

impl UiPass {
    pub(crate) fn new(gpu: &GpuContext, target_format: wgpu::TextureFormat) -> Self {
        let device = &gpu.device;
        let screen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui screen layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui texture layout"),
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
            ],
        });
        let screen_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui screen"),
            size: size_of::<ScreenGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let screen_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui screen bind group"),
            layout: &screen_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: screen_buffer.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ui sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let white_texture = create_texture(
            gpu,
            "ui white",
            1,
            wgpu::TextureFormat::Rgba8Unorm,
            &[255; 4],
        );
        let white = texture_bind_group(
            device,
            &texture_layout,
            &white_texture.create_view(&Default::default()),
            &sampler,
        );

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/ui.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui pipeline layout"),
            bind_group_layouts: &[Some(&screen_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4,
            4 => Float32x4, 5 => Float32x4, 6 => Uint32x4,
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_ui"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<UiInstanceGpu>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                })],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_ui"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let capacity = MIN_INSTANCE_CAPACITY;
        Self {
            pipeline,
            texture_layout,
            screen_buffer,
            screen_bind_group,
            sampler,
            white,
            atlas: None,
            images: HashMap::new(),
            instances: create_instance_buffer(device, capacity),
            capacity,
            scratch: Vec::new(),
            batches: Vec::new(),
            encode_srgb: !target_format.is_srgb(),
        }
    }

    /// Uploads instances and textures for `list`. Returns whether there is anything to draw.
    /// Drops the bind group of image `handle`: its view changed (a streamed texture got
    /// sharper), so the next frame binds the new one.
    pub(crate) fn forget_image(&mut self, handle: TextureHandle) {
        self.images.remove(&handle);
    }

    /// `image_view` resolves a texture handle to its GPU view.
    pub(crate) fn prepare<'a>(
        &mut self,
        gpu: &GpuContext,
        list: &UiDrawList,
        width: u32,
        height: u32,
        image_view: impl Fn(TextureHandle) -> Option<&'a wgpu::TextureView>,
    ) -> bool {
        self.scratch.clear();
        self.batches.clear();
        if list.quads.is_empty() {
            return false;
        }
        if let Some(atlas) = &list.atlas {
            self.upload_atlas(gpu, atlas);
        }

        for quad in &list.quads {
            let source = match quad.fill {
                UiFill::Solid => Source::White,
                UiFill::Glyph if self.atlas.is_some() => Source::Atlas,
                UiFill::Glyph => continue,
                UiFill::Image(handle) => {
                    if !self.images.contains_key(&handle) {
                        let Some(view) = image_view(handle) else {
                            continue;
                        };
                        let group = texture_bind_group(
                            &gpu.device,
                            &self.texture_layout,
                            view,
                            &self.sampler,
                        );
                        self.images.insert(handle, group);
                    }
                    Source::Image(handle)
                }
            };
            let index = self.scratch.len() as u32;
            match self.batches.last_mut() {
                Some((last, range)) if *last == source => range.end = index + 1,
                _ => self.batches.push((source, index..index + 1)),
            }
            let kind = match quad.fill {
                UiFill::Solid => 0,
                UiFill::Image(_) => 1,
                UiFill::Glyph => 2,
            };
            self.scratch.push(UiInstanceGpu {
                rect: quad.rect,
                uv: quad.uv,
                color: quad.color.to_array(),
                stroke_color: quad.stroke_color.to_array(),
                clip: quad.clip,
                params: [
                    quad.corner_radius,
                    quad.stroke_width,
                    quad.softness,
                    quad.screen_px_range,
                ],
                kind: [kind, 0, 0, 0],
            });
        }
        if self.scratch.is_empty() {
            return false;
        }

        if self.scratch.len() > self.capacity {
            self.capacity = self.scratch.len().next_power_of_two();
            self.instances = create_instance_buffer(&gpu.device, self.capacity);
        }
        gpu.queue
            .write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.scratch));
        let screen = ScreenGpu {
            size: [width as f32, height as f32],
            flags: [f32::from(u8::from(self.encode_srgb)), 0.0],
        };
        gpu.queue
            .write_buffer(&self.screen_buffer, 0, bytemuck::bytes_of(&screen));
        true
    }

    /// Draws what [`UiPass::prepare`] uploaded on top of `target`'s contents.
    pub(crate) fn record(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ui pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.screen_bind_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        for (source, range) in &self.batches {
            let group = match source {
                Source::White => &self.white,
                Source::Atlas => match &self.atlas {
                    Some(atlas) => &atlas.bind_group,
                    None => continue,
                },
                Source::Image(handle) => match self.images.get(handle) {
                    Some(group) => group,
                    None => continue,
                },
            };
            pass.set_bind_group(1, group, &[]);
            pass.draw(0..6, range.clone());
        }
    }

    fn upload_atlas(&mut self, gpu: &GpuContext, image: &GlyphAtlasImage) {
        if self
            .atlas
            .as_ref()
            .is_some_and(|a| a.revision == image.revision)
        {
            return;
        }
        let expected = (image.size as usize).pow(2) * 4;
        if image.pixels.len() != expected {
            return;
        }
        match &mut self.atlas {
            Some(atlas) if atlas.size == image.size => {
                write_texture(gpu, &atlas.texture, image.size, &image.pixels);
                atlas.revision = image.revision;
            }
            _ => {
                let texture = create_texture(
                    gpu,
                    "ui glyph atlas",
                    image.size,
                    wgpu::TextureFormat::Rgba8Unorm,
                    &image.pixels,
                );
                let bind_group = texture_bind_group(
                    &gpu.device,
                    &self.texture_layout,
                    &texture.create_view(&Default::default()),
                    &self.sampler,
                );
                self.atlas = Some(Atlas {
                    revision: image.revision,
                    size: image.size,
                    texture,
                    bind_group,
                });
            }
        }
    }
}

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ui instances"),
        size: (capacity * size_of::<UiInstanceGpu>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_texture(
    gpu: &GpuContext,
    label: &str,
    size: u32,
    format: wgpu::TextureFormat,
    pixels: &[u8],
) -> wgpu::Texture {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    write_texture(gpu, &texture, size, pixels);
    texture
}

fn write_texture(gpu: &GpuContext, texture: &wgpu::Texture, size: u32, pixels: &[u8]) {
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size * 4),
            rows_per_image: Some(size),
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
}

fn texture_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ui texture bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
