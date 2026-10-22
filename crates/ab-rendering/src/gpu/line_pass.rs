//! Draws the [`DebugDraw`] lines inside the HDR forward pass: depth-tested ones first, then
//! those drawn on top.

use super::context::GpuContext;
use crate::debug_draw::DebugDraw;
use bytemuck::{Pod, Zeroable};
use std::mem::size_of;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct LineVertex {
    position: [f32; 3],
    color: [f32; 4],
}

const MIN_CAPACITY: usize = 4096;

pub(crate) struct LinePass {
    depth_tested: wgpu::RenderPipeline,
    on_top: wgpu::RenderPipeline,
    vertices: wgpu::Buffer,
    capacity: usize,
    /// Vertex counts: depth-tested, then on top.
    counts: (u32, u32),
    packed: Vec<LineVertex>,
}

impl LinePass {
    pub(crate) fn new(
        gpu: &GpuContext,
        frame_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let device = &gpu.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lines.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/lines.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("line pipeline layout"),
            bind_group_layouts: &[Some(frame_layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];
        let pipeline = |compare: wgpu::CompareFunction, label: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_line"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<LineVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_line"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            depth_tested: pipeline(wgpu::CompareFunction::LessEqual, "debug lines"),
            on_top: pipeline(wgpu::CompareFunction::Always, "debug lines on top"),
            vertices: create_buffer(device, MIN_CAPACITY),
            capacity: MIN_CAPACITY,
            counts: (0, 0),
            packed: Vec::new(),
        }
    }

    /// Uploads the lines. Returns how many will be drawn.
    pub(crate) fn prepare(&mut self, gpu: &GpuContext, debug: Option<&DebugDraw>) -> u32 {
        self.counts = (0, 0);
        let Some(debug) = debug.filter(|d| d.enabled && d.line_count() > 0) else {
            return 0;
        };
        self.packed.clear();
        for on_top in [false, true] {
            for line in debug.lines().filter(|l| l.on_top == on_top) {
                let color = line.color.to_array();
                self.packed.push(LineVertex {
                    position: line.a.to_array(),
                    color,
                });
                self.packed.push(LineVertex {
                    position: line.b.to_array(),
                    color,
                });
            }
            if !on_top {
                self.counts.0 = self.packed.len() as u32;
            }
        }
        self.counts.1 = self.packed.len() as u32 - self.counts.0;
        if self.packed.len() > self.capacity {
            self.capacity = self.packed.len().next_power_of_two();
            self.vertices = create_buffer(&gpu.device, self.capacity);
        }
        gpu.queue
            .write_buffer(&self.vertices, 0, bytemuck::cast_slice(&self.packed));
        self.packed.len() as u32 / 2
    }

    /// Draws into the current HDR pass (whose group 0 is the frame bind group).
    pub(crate) fn record<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        frame: &'a wgpu::BindGroup,
    ) {
        let (tested, on_top) = self.counts;
        if tested + on_top == 0 {
            return;
        }
        pass.set_bind_group(0, frame, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        if tested > 0 {
            pass.set_pipeline(&self.depth_tested);
            pass.draw(0..tested, 0..1);
        }
        if on_top > 0 {
            pass.set_pipeline(&self.on_top);
            pass.draw(tested..tested + on_top, 0..1);
        }
    }
}

fn create_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("debug line vertices"),
        size: (capacity * size_of::<LineVertex>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
