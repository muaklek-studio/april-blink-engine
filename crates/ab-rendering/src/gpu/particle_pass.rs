//! Draws the [`ParticleDrawList`] inside the HDR forward pass.

use super::context::GpuContext;
use crate::camera::CameraView;
use crate::particles::ParticleDrawList;
use bytemuck::{Pod, Zeroable};
use std::mem::size_of;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct ViewGpu {
    right: [f32; 4],
    up: [f32; 4],
    forward: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct ParticleGpu {
    position_size: [f32; 4],
    color: [f32; 4],
    velocity_stretch: [f32; 4],
    params: [f32; 4],
}

const MIN_CAPACITY: usize = 1024;

pub(crate) struct ParticlePass {
    pipeline: wgpu::RenderPipeline,
    view_buffer: wgpu::Buffer,
    view_bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    capacity: usize,
    count: u32,
    scratch: Vec<(f32, ParticleGpu)>,
    packed: Vec<ParticleGpu>,
}

impl ParticlePass {
    pub(crate) fn new(
        gpu: &GpuContext,
        frame_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let device = &gpu.device;
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle view layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle view"),
            size: size_of::<ViewGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle view bind group"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buffer.as_entire_binding(),
            }],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particles.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/particles.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particle pipeline layout"),
            bind_group_layouts: &[Some(frame_layout), Some(&view_layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4,
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("particle pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_particle"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<ParticleGpu>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                })],
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_particle"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let capacity = MIN_CAPACITY;
        Self {
            pipeline,
            view_buffer,
            view_bind_group,
            instances: create_buffer(device, capacity),
            capacity,
            count: 0,
            scratch: Vec::new(),
            packed: Vec::new(),
        }
    }

    /// Sorts (back to front) and uploads the particles. Returns how many will be drawn.
    pub(crate) fn prepare(
        &mut self,
        gpu: &GpuContext,
        list: Option<&ParticleDrawList>,
        camera: Option<&CameraView>,
    ) -> u32 {
        self.count = 0;
        let (Some(list), Some(camera)) = (list, camera) else {
            return 0;
        };
        if list.instances.is_empty() {
            return 0;
        }
        self.scratch.clear();
        for p in &list.instances {
            if p.size <= 0.0 || p.color.a <= 0.0 {
                continue;
            }
            let depth = (p.position - camera.position).dot(camera.forward);
            if depth < camera.near - p.size {
                continue; // behind the camera
            }
            self.scratch.push((
                depth,
                ParticleGpu {
                    position_size: [p.position.x, p.position.y, p.position.z, p.size],
                    color: p.color.to_array(),
                    velocity_stretch: [p.velocity.x, p.velocity.y, p.velocity.z, p.stretch],
                    params: [p.rotation, p.hardness, f32::from(u8::from(p.additive)), 0.0],
                },
            ));
        }
        self.scratch.sort_by(|a, b| b.0.total_cmp(&a.0));
        self.packed.clear();
        self.packed.extend(self.scratch.iter().map(|(_, p)| *p));
        if self.packed.is_empty() {
            return 0;
        }
        if self.packed.len() > self.capacity {
            self.capacity = self.packed.len().next_power_of_two();
            self.instances = create_buffer(&gpu.device, self.capacity);
        }
        gpu.queue
            .write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.packed));
        // The camera's axes are the rows of the view matrix's rotation.
        let rows = camera.view.transpose();
        let view = ViewGpu {
            right: rows.x_axis.truncate().extend(0.0).to_array(),
            up: rows.y_axis.truncate().extend(0.0).to_array(),
            forward: camera.forward.extend(0.0).to_array(),
        };
        gpu.queue
            .write_buffer(&self.view_buffer, 0, bytemuck::bytes_of(&view));
        self.count = self.packed.len() as u32;
        self.count
    }

    /// Draws into the current HDR pass (whose group 0 is the frame bind group).
    pub(crate) fn record<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        frame: &'a wgpu::BindGroup,
    ) {
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, frame, &[]);
        pass.set_bind_group(1, &self.view_bind_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..6, 0..self.count);
    }
}

fn create_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("particle instances"),
        size: (capacity * size_of::<ParticleGpu>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
