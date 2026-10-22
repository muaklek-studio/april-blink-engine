use super::batch::{BatchPlan, DrawClass};
use super::bloom_pass::BloomPass;
use super::context::{GpuContext, RendererConfig};
use super::line_pass::LinePass;
use super::particle_pass::ParticlePass;
use super::ui_pass::UiPass;
use super::uniforms::{FrameGpu, MaterialGpu, ObjectGpu, OmniGpu, PostGpu, SpotGpu};
use crate::assets::{RenderAssets, TextureHandle};
use crate::debug_draw::DebugDraw;
use crate::environment::{Environment, ScreenOverlay};
use crate::extract::{extract, RenderList};
use crate::limits::{MAX_OMNI_LIGHTS, MAX_SPOT_LIGHTS, MAX_SPOT_SHADOWS, SHADOW_LAYERS};
use crate::material::Material;
use crate::mesh::{SkinWeights, Vertex};
use crate::particles::ParticleDrawList;
use crate::shadow::{directional_shadow_matrix, spot_shadow_matrix};
use crate::texture::{ColorSpace, SamplerSettings, Texture, TextureFilter, TextureWrap};
use crate::ui::UiDrawList;
use ab_core::{Color, Mat3, Mat4, Time};
use ab_ecs::World;
use bytemuck::Zeroable as _;
use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU64;
use tracing::{debug, warn};
use wgpu::util::DeviceExt;

const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Distance (alpha of the HDR target) of pixels where nothing was drawn; `FAR_DISTANCE` in
/// forward.wgsl.
const FAR_DISTANCE: f64 = 60000.0;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Per-layer slot in the shadow-pass uniform buffer (max `min_uniform_buffer_offset_alignment`).
const SHADOW_UNIFORM_STRIDE: u64 = 256;
const MIN_OBJECT_CAPACITY: usize = 256;
const MIN_MATERIAL_CAPACITY: usize = 64;
const MIN_JOINT_CAPACITY: usize = 256;
/// Texture slots per material, in shader binding order.
const MATERIAL_TEXTURE_SLOTS: usize = 4;

/// Counters for the last rendered frame (debug overlay, profiling).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameStats {
    /// Instanced draw calls in the camera pass.
    pub draw_calls: u32,
    /// Objects drawn in the camera pass.
    pub objects: u32,
    /// Mesh instances rejected by frustum culling.
    pub culled: u32,
    /// Draw calls across all shadow passes.
    pub shadow_draw_calls: u32,
    /// Shadow-map layers rendered.
    pub shadow_layers: u32,
    /// Lights sent to the shader (after limits).
    pub lights: u32,
    /// Skinned instances drawn in the camera pass.
    pub skinned: u32,
    /// Particles drawn.
    pub particles: u32,
    /// Debug lines drawn.
    pub debug_lines: u32,
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    skin: Option<wgpu::Buffer>,
    indices: wgpu::Buffer,
    index_count: u32,
}

struct GpuTexture {
    view: wgpu::TextureView,
    sampler: SamplerSettings,
}

/// Bind group for one material, with the texture handles it was built from.
struct MaterialBinding {
    key: [Option<TextureHandle>; MATERIAL_TEXTURE_SLOTS],
    bind_group: wgpu::BindGroup,
}

struct FrameTargets {
    size: (u32, u32),
    hdr_view: wgpu::TextureView,
    /// Light shafts, half resolution.
    shafts_view: wgpu::TextureView,
    /// The scene color for the light shafts pass.
    shafts_bind_group: wgpu::BindGroup,
    msaa_view: Option<wgpu::TextureView>,
    depth_view: wgpu::TextureView,
    post_bind_group: wgpu::BindGroup,
}

/// Renders a world's scene into any color target: shadow maps → HDR forward pass →
/// post-process into the target.
pub struct SceneRenderer {
    target_format: wgpu::TextureFormat,
    msaa_samples: u32,
    shadow_map_size: u32,

    /// Indexed by [`forward_variant`].
    forward_pipelines: [wgpu::RenderPipeline; 4],
    /// The background through the fog (drawn when fog is on).
    sky_pipeline: wgpu::RenderPipeline,
    /// Whether this frame draws the fogged sky.
    sky: bool,
    /// Light shafts (volumetric light), traced at half resolution.
    shafts_pipeline: wgpu::RenderPipeline,
    shafts_layout: wgpu::BindGroupLayout,
    /// Whether this frame traces light shafts.
    shafts: bool,
    /// Static, skinned.
    shadow_pipelines: [wgpu::RenderPipeline; 2],
    post_pipeline: wgpu::RenderPipeline,
    bloom_pass: BloomPass,
    ui_pass: UiPass,
    particle_pass: ParticlePass,
    line_pass: LinePass,
    forward_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    shadow_layout: wgpu::BindGroupLayout,
    post_layout: wgpu::BindGroupLayout,

    frame_buffer: wgpu::Buffer,
    post_buffer: wgpu::Buffer,
    shadow_pass_buffer: wgpu::Buffer,
    object_buffer: wgpu::Buffer,
    object_capacity: usize,
    material_buffer: wgpu::Buffer,
    material_capacity: usize,
    joint_buffer: wgpu::Buffer,
    joint_capacity: usize,

    shadow_array_view: wgpu::TextureView,
    shadow_layer_views: Vec<wgpu::TextureView>,
    shadow_sampler: wgpu::Sampler,
    linear_sampler: wgpu::Sampler,
    /// The fog patch volume and its repeating sampler.
    fog_noise: (wgpu::TextureView, wgpu::Sampler),
    forward_bind_group: wgpu::BindGroup,
    shadow_bind_group: wgpu::BindGroup,

    /// Fallbacks for empty material slots: white sRGB, white linear, flat normal, white sRGB.
    default_textures: [GpuTexture; MATERIAL_TEXTURE_SLOTS],
    samplers: HashMap<SamplerSettings, wgpu::Sampler>,
    textures: Vec<GpuTexture>,
    meshes: Vec<GpuMesh>,
    material_bindings: Vec<Option<MaterialBinding>>,
    targets: Option<FrameTargets>,

    // Per-frame scratch, kept to avoid reallocating.
    list: RenderList,
    plan: BatchPlan,
    objects: Vec<ObjectGpu>,
    materials: Vec<MaterialGpu>,
    material_flags: Vec<bool>,
    warned_light_limit: bool,
}

/// Index of the forward pipeline for a draw class.
fn forward_variant(class: DrawClass) -> usize {
    (class.skinned as usize) * 2 + class.double_sided as usize
}

impl SceneRenderer {
    pub fn new(
        gpu: &GpuContext,
        target_format: wgpu::TextureFormat,
        config: &RendererConfig,
    ) -> Self {
        let device = &gpu.device;
        let msaa_samples = supported_msaa(gpu, config.msaa_samples);
        let shadow_map_size = config
            .shadow_map_size
            .clamp(256, device.limits().max_texture_dimension_2d);

        let forward_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("forward layout"),
            entries: &[
                uniform_entry(
                    0,
                    wgpu::ShaderStages::VERTEX_FRAGMENT,
                    false,
                    size_of::<FrameGpu>(),
                ),
                storage_entry(1, wgpu::ShaderStages::VERTEX),
                storage_entry(2, wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                storage_entry(5, wgpu::ShaderStages::VERTEX),
                // Fog patches (fog.wgsl).
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let material_entries: Vec<_> = (0..MATERIAL_TEXTURE_SLOTS as u32)
            .map(texture_entry)
            .chain(
                (0..MATERIAL_TEXTURE_SLOTS as u32).map(|i| wgpu::BindGroupLayoutEntry {
                    binding: MATERIAL_TEXTURE_SLOTS as u32 + i,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                }),
            )
            .collect();
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material layout"),
            entries: &material_entries,
        });
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow layout"),
            entries: &[
                uniform_entry(
                    0,
                    wgpu::ShaderStages::VERTEX,
                    true,
                    size_of::<[[f32; 4]; 4]>(),
                ),
                storage_entry(1, wgpu::ShaderStages::VERTEX),
                storage_entry(2, wgpu::ShaderStages::VERTEX),
            ],
        });
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post layout"),
            entries: &[
                texture_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform_entry(2, wgpu::ShaderStages::FRAGMENT, false, size_of::<PostGpu>()),
                texture_entry(3),
                texture_entry(4),
            ],
        });
        let shafts_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("light shafts layout"),
            entries: &[texture_entry(0)],
        });

        let forward_module = shader(
            device,
            "forward.wgsl",
            concat!(
                include_str!("../shaders/fog.wgsl"),
                include_str!("../shaders/forward.wgsl")
            ),
        );
        let forward_pipelines = [(false, false), (false, true), (true, false), (true, true)].map(
            |(skinned, double_sided)| {
                create_forward_pipeline(
                    device,
                    &forward_module,
                    &[Some(&forward_layout), Some(&material_layout)],
                    msaa_samples,
                    skinned,
                    double_sided,
                )
            },
        );
        let sky_pipeline =
            create_sky_pipeline(device, &forward_module, &forward_layout, msaa_samples);
        let shafts_pipeline =
            create_shafts_pipeline(device, &forward_module, &forward_layout, &shafts_layout);
        let shadow_module = shader(
            device,
            "shadow.wgsl",
            include_str!("../shaders/shadow.wgsl"),
        );
        let shadow_pipelines = [false, true]
            .map(|skinned| create_shadow_pipeline(device, &shadow_module, &shadow_layout, skinned));
        let post_pipeline = create_post_pipeline(device, &post_layout, target_format);

        let frame_buffer = create_uniform(device, "frame uniforms", size_of::<FrameGpu>() as u64);
        let post_buffer = create_uniform(device, "post uniforms", size_of::<PostGpu>() as u64);
        let shadow_pass_buffer = create_uniform(
            device,
            "shadow pass uniforms",
            SHADOW_UNIFORM_STRIDE * SHADOW_LAYERS as u64,
        );
        let object_buffer = create_storage(
            device,
            "objects",
            MIN_OBJECT_CAPACITY * size_of::<ObjectGpu>(),
        );
        let material_buffer = create_storage(
            device,
            "materials",
            MIN_MATERIAL_CAPACITY * size_of::<MaterialGpu>(),
        );
        let joint_buffer = create_storage(
            device,
            "joint matrices",
            MIN_JOINT_CAPACITY * size_of::<Mat4>(),
        );

        let shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow maps"),
            size: wgpu::Extent3d {
                width: shadow_map_size,
                height: shadow_map_size,
                depth_or_array_layers: SHADOW_LAYERS as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_array_view = shadow_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("shadow maps (array)"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let shadow_layer_views = (0..SHADOW_LAYERS as u32)
            .map(|layer| {
                shadow_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("shadow map layer"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let fog_noise = super::fog_noise::create(device, &gpu.queue);
        let (forward_bind_group, shadow_bind_group) = create_scene_bind_groups(
            device,
            &forward_layout,
            &shadow_layout,
            SceneBuffers {
                frame: &frame_buffer,
                objects: &object_buffer,
                materials: &material_buffer,
                joints: &joint_buffer,
                shadow_pass: &shadow_pass_buffer,
            },
            &shadow_array_view,
            &shadow_sampler,
            &fog_noise,
        );

        let default_textures = [
            Texture::solid([255; 4], ColorSpace::Srgb),
            Texture::solid([255; 4], ColorSpace::Linear),
            Texture::solid([128, 128, 255, 255], ColorSpace::Linear),
            Texture::solid([255; 4], ColorSpace::Srgb),
        ]
        .map(|t| upload_texture(gpu, &t, "default texture"));

        debug!(
            ?target_format,
            msaa_samples, shadow_map_size, "scene renderer created"
        );
        Self {
            target_format,
            msaa_samples,
            shadow_map_size,
            forward_pipelines,
            sky_pipeline,
            sky: false,
            shafts_pipeline,
            shafts_layout,
            shafts: false,
            shadow_pipelines,
            post_pipeline,
            bloom_pass: BloomPass::new(device, HDR_FORMAT),
            ui_pass: UiPass::new(gpu, target_format),
            particle_pass: ParticlePass::new(
                gpu,
                &forward_layout,
                HDR_FORMAT,
                DEPTH_FORMAT,
                msaa_samples,
            ),
            line_pass: LinePass::new(gpu, &forward_layout, HDR_FORMAT, DEPTH_FORMAT, msaa_samples),
            forward_layout,
            material_layout,
            shadow_layout,
            post_layout,
            frame_buffer,
            post_buffer,
            shadow_pass_buffer,
            object_buffer,
            object_capacity: MIN_OBJECT_CAPACITY,
            material_buffer,
            material_capacity: MIN_MATERIAL_CAPACITY,
            joint_buffer,
            joint_capacity: MIN_JOINT_CAPACITY,
            shadow_array_view,
            shadow_layer_views,
            shadow_sampler,
            linear_sampler,
            fog_noise,
            forward_bind_group,
            shadow_bind_group,
            default_textures,
            samplers: HashMap::new(),
            textures: Vec::new(),
            meshes: Vec::new(),
            material_bindings: Vec::new(),
            targets: None,
            list: RenderList::default(),
            plan: BatchPlan::default(),
            objects: Vec::new(),
            materials: Vec::new(),
            material_flags: Vec::new(),
            warned_light_limit: false,
        }
    }

    pub fn target_format(&self) -> wgpu::TextureFormat {
        self.target_format
    }

    /// Effective MSAA sample count (may be lower than requested).
    pub fn msaa_samples(&self) -> u32 {
        self.msaa_samples
    }

    /// The render list built for the last frame.
    pub fn last_render_list(&self) -> &RenderList {
        &self.list
    }

    /// Renders the world's scene into `target` (`width` × `height`, format `target_format`).
    pub fn render(
        &mut self,
        gpu: &GpuContext,
        world: &World,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> FrameStats {
        let max = gpu.max_texture_size();
        let (width, height) = (width.clamp(1, max), height.clamp(1, max));
        self.ensure_targets(gpu, width, height);
        extract(world, width as f32 / height as f32, &mut self.list);
        let environment = world
            .resource::<Environment>()
            .map(|e| e.clone())
            .unwrap_or_default();

        let default_material = [Material::default()];
        match world.resource::<RenderAssets>() {
            Some(assets) => {
                self.upload_meshes(gpu, &assets);
                self.upload_textures(gpu, &assets);
                let materials = if assets.materials().is_empty() {
                    &default_material[..]
                } else {
                    assets.materials()
                };
                self.prepare_materials(gpu, materials);
            }
            None => self.prepare_materials(gpu, &default_material),
        }

        let meshes = &self.meshes;
        let double_sided = &self.material_flags;
        self.plan.build(&self.list.draws, |d| {
            let mesh = meshes.get(d.mesh.index())?;
            let double_sided = *double_sided.get(d.material.index())?;
            Some(DrawClass {
                skinned: d.joints.is_some() && mesh.skin.is_some(),
                double_sided,
            })
        });
        self.pack_objects();
        self.upload_scene_buffers(gpu);

        let mut stats = FrameStats {
            culled: self.list.draws.iter().filter(|d| !d.in_view).count() as u32,
            ..Default::default()
        };
        let seconds = world.resource::<Time>().map_or(0.0, |t| t.elapsed as f32);
        let (frame, active_layers) = self.build_frame_uniforms(&environment, seconds, &mut stats);
        gpu.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));
        gpu.queue.write_buffer(
            &self.post_buffer,
            0,
            bytemuck::bytes_of(&self.pack_post(&environment, world)),
        );
        let bloom = environment.post.bloom.is_enabled();
        if bloom {
            self.bloom_pass
                .write_uniforms(&gpu.queue, &environment.post.bloom);
        }
        self.write_shadow_pass_uniforms(gpu, &active_layers);

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scene frame"),
            });
        if self.list.camera.is_some() {
            self.record_shadow_passes(&mut encoder, &active_layers, &mut stats);
        }
        stats.particles = self.particle_pass.prepare(
            gpu,
            world.resource::<ParticleDrawList>().as_deref(),
            self.list.camera.as_ref(),
        );
        stats.debug_lines = self
            .line_pass
            .prepare(gpu, world.resource::<DebugDraw>().as_deref());
        self.record_forward_pass(&mut encoder, environment.background, &mut stats);
        if self.shafts {
            self.record_shafts_pass(&mut encoder);
        }
        if bloom {
            self.bloom_pass.record(&mut encoder);
        }
        self.record_post_pass(&mut encoder, target);
        if let Some(ui) = world.resource::<UiDrawList>() {
            let textures = &self.textures;
            let ready = self.ui_pass.prepare(gpu, &ui, width, height, |handle| {
                textures.get(handle.index()).map(|t| &t.view)
            });
            if ready {
                self.ui_pass.record(&mut encoder, target);
            }
        }
        gpu.queue.submit(Some(encoder.finish()));
        stats
    }

    fn ensure_targets(&mut self, gpu: &GpuContext, width: u32, height: u32) {
        if self
            .targets
            .as_ref()
            .is_some_and(|t| t.size == (width, height))
        {
            return;
        }
        let device = &gpu.device;
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = |label, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let hdr = texture(
            "hdr color",
            HDR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let hdr_view = hdr.create_view(&Default::default());
        let msaa_view = (self.msaa_samples > 1).then(|| {
            texture(
                "hdr color (msaa)",
                HDR_FORMAT,
                self.msaa_samples,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
            .create_view(&Default::default())
        });
        let depth_view = texture(
            "depth",
            DEPTH_FORMAT,
            self.msaa_samples,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        )
        .create_view(&Default::default());
        let bloom_view = self.bloom_pass.resize(device, &hdr_view, width, height);
        let shafts_view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("light shafts"),
                size: wgpu::Extent3d {
                    width: width.div_ceil(2),
                    height: height.div_ceil(2),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let shafts_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("light shafts bind group"),
            layout: &self.shafts_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&hdr_view),
            }],
        });
        let post_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post bind group"),
            layout: &self.post_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.post_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(bloom_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&shafts_view),
                },
            ],
        });
        self.targets = Some(FrameTargets {
            size: (width, height),
            hdr_view,
            shafts_view,
            shafts_bind_group,
            msaa_view,
            depth_view,
            post_bind_group,
        });
    }

    fn upload_meshes(&mut self, gpu: &GpuContext, assets: &RenderAssets) {
        // Meshes are append-only in RenderAssets, so only new ones need uploading.
        for entry in &assets.mesh_entries()[self.meshes.len()..] {
            let buffer = |label, contents: &[u8], usage| {
                gpu.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(label),
                        contents,
                        usage,
                    })
            };
            let mesh = &entry.mesh;
            self.meshes.push(GpuMesh {
                vertices: buffer(
                    "mesh vertices",
                    bytemuck::cast_slice::<Vertex, u8>(&mesh.vertices),
                    wgpu::BufferUsages::VERTEX,
                ),
                skin: mesh.skin.as_ref().map(|skin| {
                    buffer(
                        "mesh skin",
                        bytemuck::cast_slice::<SkinWeights, u8>(skin),
                        wgpu::BufferUsages::VERTEX,
                    )
                }),
                indices: buffer(
                    "mesh indices",
                    bytemuck::cast_slice::<u32, u8>(&mesh.indices),
                    wgpu::BufferUsages::INDEX,
                ),
                index_count: mesh.indices.len() as u32,
            });
        }
    }

    /// Uploads textures added since the last frame. Their mip chains (the CPU-heavy part)
    /// are built in parallel; the uploads themselves stay on this thread.
    fn upload_textures(&mut self, gpu: &GpuContext, assets: &RenderAssets) {
        let new = &assets.textures()[self.textures.len()..];
        let chains = ab_core::jobs::par_map(new, 1, Texture::mip_chain);
        for (texture, levels) in new.iter().zip(&chains) {
            self.textures.push(upload_texture_levels(
                gpu,
                texture,
                levels,
                "material texture",
            ));
        }
    }

    /// Packs material uniforms and (re)builds material bind groups whose textures changed.
    fn prepare_materials(&mut self, gpu: &GpuContext, materials: &[Material]) {
        self.materials.clear();
        self.material_flags.clear();
        self.material_bindings.resize_with(materials.len(), || None);
        let texture_count = self.textures.len();
        for (index, material) in materials.iter().enumerate() {
            let key = material
                .texture_slots()
                .map(|slot| slot.filter(|t| t.index() < texture_count));
            self.materials
                .push(pack_material(material, key[2].is_some()));
            self.material_flags.push(material.double_sided);

            let up_to_date = self.material_bindings[index]
                .as_ref()
                .is_some_and(|binding| binding.key == key);
            if !up_to_date {
                let bind_group = self.create_material_bind_group(gpu, &key);
                self.material_bindings[index] = Some(MaterialBinding { key, bind_group });
            }
        }
    }

    fn create_material_bind_group(
        &mut self,
        gpu: &GpuContext,
        key: &[Option<TextureHandle>; MATERIAL_TEXTURE_SLOTS],
    ) -> wgpu::BindGroup {
        let textures: [&GpuTexture; MATERIAL_TEXTURE_SLOTS] = std::array::from_fn(|slot| {
            key[slot].map_or(&self.default_textures[slot], |t| &self.textures[t.index()])
        });
        for texture in &textures {
            self.samplers
                .entry(texture.sampler)
                .or_insert_with(|| create_sampler(&gpu.device, &texture.sampler));
        }
        let mut entries = Vec::with_capacity(MATERIAL_TEXTURE_SLOTS * 2);
        for (slot, texture) in textures.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: slot as u32,
                resource: wgpu::BindingResource::TextureView(&texture.view),
            });
        }
        for (slot, texture) in textures.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: (MATERIAL_TEXTURE_SLOTS + slot) as u32,
                resource: wgpu::BindingResource::Sampler(&self.samplers[&texture.sampler]),
            });
        }
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material bind group"),
            layout: &self.material_layout,
            entries: &entries,
        })
    }

    fn pack_objects(&mut self) {
        self.objects.clear();
        for (&index, class) in self.plan.order.iter().zip(&self.plan.classes) {
            let draw = &self.list.draws[index];
            let model = Mat4::from(draw.model);
            let linear = Mat3::from(draw.model.matrix3);
            let normal = if linear.determinant().abs() > 1e-12 {
                Mat4::from_mat3(linear.inverse().transpose())
            } else {
                Mat4::IDENTITY
            };
            let first_joint = match (&draw.joints, class.skinned) {
                (Some(joints), true) => joints.start,
                _ => 0,
            };
            self.objects.push(ObjectGpu {
                model: model.to_cols_array_2d(),
                normal: normal.to_cols_array_2d(),
                material: [draw.material.index() as u32, first_joint, 0, 0],
            });
        }
    }

    fn upload_scene_buffers(&mut self, gpu: &GpuContext) {
        let device = &gpu.device;
        let mut rebind = false;
        let mut grow = |buffer: &mut wgpu::Buffer,
                        capacity: &mut usize,
                        needed: usize,
                        stride: usize,
                        label| {
            if needed > *capacity {
                *capacity = needed.next_power_of_two();
                *buffer = create_storage(device, label, *capacity * stride);
                rebind = true;
            }
        };
        grow(
            &mut self.object_buffer,
            &mut self.object_capacity,
            self.objects.len(),
            size_of::<ObjectGpu>(),
            "objects",
        );
        grow(
            &mut self.material_buffer,
            &mut self.material_capacity,
            self.materials.len(),
            size_of::<MaterialGpu>(),
            "materials",
        );
        grow(
            &mut self.joint_buffer,
            &mut self.joint_capacity,
            self.list.joint_matrices.len(),
            size_of::<Mat4>(),
            "joint matrices",
        );
        if rebind {
            (self.forward_bind_group, self.shadow_bind_group) = create_scene_bind_groups(
                device,
                &self.forward_layout,
                &self.shadow_layout,
                SceneBuffers {
                    frame: &self.frame_buffer,
                    objects: &self.object_buffer,
                    materials: &self.material_buffer,
                    joints: &self.joint_buffer,
                    shadow_pass: &self.shadow_pass_buffer,
                },
                &self.shadow_array_view,
                &self.shadow_sampler,
                &self.fog_noise,
            );
        }
        if !self.objects.is_empty() {
            gpu.queue
                .write_buffer(&self.object_buffer, 0, bytemuck::cast_slice(&self.objects));
        }
        gpu.queue.write_buffer(
            &self.material_buffer,
            0,
            bytemuck::cast_slice(&self.materials),
        );
        if !self.list.joint_matrices.is_empty() {
            let joints: Vec<[[f32; 4]; 4]> = self
                .list
                .joint_matrices
                .iter()
                .map(Mat4::to_cols_array_2d)
                .collect();
            gpu.queue
                .write_buffer(&self.joint_buffer, 0, bytemuck::cast_slice(&joints));
        }
    }

    /// Fills the frame uniforms and returns `(layer, light matrix)` for each shadow map to render.
    fn build_frame_uniforms(
        &mut self,
        environment: &Environment,
        seconds: f32,
        stats: &mut FrameStats,
    ) -> (FrameGpu, Vec<(usize, Mat4)>) {
        let mut frame = FrameGpu::zeroed();
        let mut layers = Vec::new();
        let map_size = self.shadow_map_size;
        let shadows = &environment.shadows;

        frame.ambient = rgb(environment.ambient_color, environment.ambient_energy);
        let fog = &environment.fog;
        frame.fog_color = [
            fog.color.r,
            fog.color.g,
            fog.color.b,
            fog.enabled as u32 as f32,
        ];
        frame.fog_params = [
            fog.density.max(0.0),
            fog.height,
            fog.height_falloff.max(0.0),
            fog.max_opacity.clamp(0.0, 1.0),
        ];
        frame.sun_radiance[3] = -1.0;
        self.sky = false;
        self.shafts = false;

        let Some(camera) = self.list.camera else {
            return (frame, layers);
        };
        frame.fog_scatter = [
            fog.sun_scattering.max(0.0),
            fog.sun_scattering_anisotropy.clamp(-0.95, 0.95),
            fog_camera_term(fog, camera.position.y),
            fog.patchiness.clamp(0.0, 1.0),
        ];
        let patch_size = fog.patch_size.max(0.5);
        // In patch units, wrapped: the volume repeats every unit, and small numbers keep
        // their precision however long the game runs.
        let offset = (fog.wind * seconds / patch_size).fract_gl();
        frame.fog_wind = [offset.x, offset.y, offset.z, 1.0 / patch_size];
        frame.view_proj = camera.view_projection.to_cols_array_2d();
        frame.inverse_view_proj = camera.view_projection.inverse().to_cols_array_2d();
        frame.background = rgb(environment.background, 1.0);
        self.sky = fog.enabled && fog.density > 0.0;
        let shafts = &environment.light_shafts;
        let lit = self.list.directional.is_some() || !self.list.spot_lights.is_empty();
        self.shafts = shafts.enabled && shafts.intensity > 0.0 && lit;
        frame.shafts = [
            shafts.intensity.max(0.0),
            shafts.anisotropy.clamp(-0.95, 0.95),
            shafts.max_distance.clamp(1.0, 5000.0),
            shafts.density.max(0.0),
        ];
        frame.camera_position = camera.position.extend(1.0).into();

        let mut sun_normal_offset = 0.0;
        if let Some(sun) = self.list.directional {
            frame.sun_direction = sun.direction.extend(1.0).into();
            frame.sun_radiance = sun.radiance.extend(-1.0).into();
            stats.lights += 1;
            if sun.shadow {
                let matrix = directional_shadow_matrix(
                    sun.direction,
                    &camera,
                    shadows.directional_distance,
                    map_size,
                );
                let radius = 1.0 / matrix.row(0).truncate().length();
                sun_normal_offset = shadows.normal_bias * 2.0 * radius / map_size as f32;
                frame.sun_radiance[3] = 0.0;
                frame.shadow_matrices[0] = matrix.to_cols_array_2d();
                layers.push((0, matrix));
            }
        }
        frame.shadow_params = [
            shadows.depth_bias,
            sun_normal_offset,
            1.0 / map_size as f32,
            0.0,
        ];

        let omni_count = self.list.omni_lights.len().min(MAX_OMNI_LIGHTS);
        for (slot, light) in frame.omni.iter_mut().zip(&self.list.omni_lights) {
            *slot = OmniGpu {
                position_range: light.position.extend(light.range).into(),
                radiance: light.radiance.extend(0.0).into(),
            };
        }

        let spot_count = self.list.spot_lights.len().min(MAX_SPOT_LIGHTS);
        let mut spot_shadows = 0;
        for (slot, light) in frame.spot.iter_mut().zip(&self.list.spot_lights) {
            let mut shadow = [-1.0, 0.0, 0.0, 0.0];
            if light.shadow && spot_shadows < MAX_SPOT_SHADOWS {
                let layer = 1 + spot_shadows;
                spot_shadows += 1;
                let matrix =
                    spot_shadow_matrix(light.position, light.direction, light.angle, light.range);
                frame.shadow_matrices[layer] = matrix.to_cols_array_2d();
                layers.push((layer, matrix));
                // World size of one texel at 1 m from the light, matching the projection's FOV.
                let per_metre = 2.0 * (light.angle * 1.1).min(1.5).tan() / map_size as f32;
                shadow = [layer as f32, shadows.normal_bias * per_metre, 0.0, 0.0];
            }
            *slot = SpotGpu {
                position_range: light.position.extend(light.range).into(),
                direction_cos_outer: light.direction.extend(light.cos_outer).into(),
                radiance_cos_inner: light.radiance.extend(light.cos_inner).into(),
                shadow,
            };
        }
        frame.light_counts = [omni_count as u32, spot_count as u32, 0, 0];
        stats.lights += (omni_count + spot_count) as u32;
        stats.shadow_layers = layers.len() as u32;

        let dropped =
            self.list.omni_lights.len() - omni_count + self.list.spot_lights.len() - spot_count;
        if dropped > 0 && !self.warned_light_limit {
            self.warned_light_limit = true;
            warn!(
                dropped,
                max_omni = MAX_OMNI_LIGHTS,
                max_spot = MAX_SPOT_LIGHTS,
                "too many lights; the farthest from the camera are ignored"
            );
        }
        (frame, layers)
    }

    fn pack_post(&self, environment: &Environment, world: &World) -> PostGpu {
        let post = &environment.post;
        let overlay = world
            .resource::<ScreenOverlay>()
            .map(|o| *o)
            .unwrap_or_default();
        // Seconds wrapped to keep f32 precision for the grain's noise seed.
        let seconds = world
            .resource::<Time>()
            .map_or(0.0, |t| (t.elapsed % 1000.0) as f32);
        let finite = |v: f32, fallback: f32| if v.is_finite() { v } else { fallback };
        let aberration = finite(post.chromatic_aberration, 0.0).max(0.0)
            + finite(overlay.chromatic_aberration, 0.0).max(0.0);
        let bloom = if post.bloom.is_enabled() {
            post.bloom.intensity
        } else {
            0.0
        };
        PostGpu {
            params: [
                post.exposure.max(0.0),
                post.contrast.max(0.0),
                post.saturation.max(0.0),
                post.tonemap as u32 as f32,
            ],
            tint: [post.tint.r, post.tint.g, post.tint.b, 1.0],
            vignette: [
                post.vignette_intensity.clamp(0.0, 1.0),
                post.vignette_smoothness,
                (!self.target_format.is_srgb()) as u32 as f32,
                self.shafts as u32 as f32,
            ],
            effects: [
                aberration.min(4.0),
                finite(post.grain, 0.0).clamp(0.0, 4.0),
                bloom,
                seconds,
            ],
            overlay: [
                overlay.color.r,
                overlay.color.g,
                overlay.color.b,
                finite(overlay.color.a, 0.0).clamp(0.0, 1.0),
            ],
        }
    }

    /// One light matrix per layer, each at a `SHADOW_UNIFORM_STRIDE` offset (dynamic binding).
    fn write_shadow_pass_uniforms(&self, gpu: &GpuContext, layers: &[(usize, Mat4)]) {
        if layers.is_empty() {
            return;
        }
        let mut uniforms = [0u8; SHADOW_UNIFORM_STRIDE as usize * SHADOW_LAYERS];
        for &(layer, matrix) in layers {
            let start = layer * SHADOW_UNIFORM_STRIDE as usize;
            uniforms[start..start + 64]
                .copy_from_slice(bytemuck::bytes_of(&matrix.to_cols_array_2d()));
        }
        gpu.queue
            .write_buffer(&self.shadow_pass_buffer, 0, &uniforms);
    }

    fn record_shadow_passes(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        layers: &[(usize, Mat4)],
        stats: &mut FrameStats,
    ) {
        for &(layer, _) in layers {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_layer_views[layer],
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(
                0,
                &self.shadow_bind_group,
                &[(layer as u64 * SHADOW_UNIFORM_STRIDE) as u32],
            );
            let mut bound_pipeline = None;
            for batch in &self.plan.shadow {
                let skinned = batch.class.skinned;
                if bound_pipeline != Some(skinned) {
                    pass.set_pipeline(&self.shadow_pipelines[skinned as usize]);
                    bound_pipeline = Some(skinned);
                }
                let mesh = &self.meshes[batch.mesh.index()];
                bind_mesh(&mut pass, mesh, skinned);
                pass.draw_indexed(0..mesh.index_count, 0, batch.instances.clone());
                stats.shadow_draw_calls += 1;
            }
        }
    }

    fn record_forward_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        background: Color,
        stats: &mut FrameStats,
    ) {
        let targets = self.targets.as_ref().expect("targets created in render()");
        let (view, resolve_target, store) = match &targets.msaa_view {
            Some(msaa) => (msaa, Some(&targets.hdr_view), wgpu::StoreOp::Discard),
            None => (&targets.hdr_view, None, wgpu::StoreOp::Store),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("forward pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: background.r as f64,
                        g: background.g as f64,
                        b: background.b as f64,
                        // Distance from the camera: nothing there.
                        a: FAR_DISTANCE,
                    }),
                    store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if self.list.camera.is_none() {
            return;
        }
        pass.set_bind_group(0, &self.forward_bind_group, &[]);
        let mut bound_pipeline = None;
        let mut bound_material = None;
        for batch in &self.plan.forward {
            let variant = forward_variant(batch.class);
            if bound_pipeline != Some(variant) {
                pass.set_pipeline(&self.forward_pipelines[variant]);
                bound_pipeline = Some(variant);
            }
            if bound_material != Some(batch.material) {
                let binding = self.material_bindings[batch.material.index()]
                    .as_ref()
                    .expect("bindings prepared for every material");
                pass.set_bind_group(1, &binding.bind_group, &[]);
                bound_material = Some(batch.material);
            }
            let mesh = &self.meshes[batch.mesh.index()];
            bind_mesh(&mut pass, mesh, batch.class.skinned);
            pass.draw_indexed(0..mesh.index_count, 0, batch.instances.clone());
            stats.draw_calls += 1;
            stats.objects += batch.instances.len() as u32;
            if batch.class.skinned {
                stats.skinned += batch.instances.len() as u32;
            }
        }
        if self.sky {
            pass.set_pipeline(&self.sky_pipeline);
            pass.draw(0..3, 0..1);
        }
        // Translucent effects over the opaque scene, depth-tested.
        self.particle_pass
            .record(&mut pass, &self.forward_bind_group);
        self.line_pass.record(&mut pass, &self.forward_bind_group);
    }

    fn record_shafts_pass(&self, encoder: &mut wgpu::CommandEncoder) {
        let targets = self.targets.as_ref().expect("targets created in render()");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("light shafts pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.shafts_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.shafts_pipeline);
        pass.set_bind_group(0, &self.forward_bind_group, &[]);
        pass.set_bind_group(2, &targets.shafts_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn record_post_pass(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let targets = self.targets.as_ref().expect("targets created in render()");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("post pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.post_pipeline);
        pass.set_bind_group(0, &targets.post_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

impl fmt::Debug for SceneRenderer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SceneRenderer")
            .field("target_format", &self.target_format)
            .field("msaa_samples", &self.msaa_samples)
            .field("shadow_map_size", &self.shadow_map_size)
            .field("meshes", &self.meshes.len())
            .field("textures", &self.textures.len())
            .field("target_size", &self.targets.as_ref().map(|t| t.size))
            .finish()
    }
}

fn bind_mesh(pass: &mut wgpu::RenderPass<'_>, mesh: &GpuMesh, skinned: bool) {
    pass.set_vertex_buffer(0, mesh.vertices.slice(..));
    if skinned {
        if let Some(skin) = &mesh.skin {
            pass.set_vertex_buffer(1, skin.slice(..));
        }
    }
    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
}

fn rgb(color: Color, energy: f32) -> [f32; 4] {
    [color.r * energy, color.g * energy, color.b * energy, 0.0]
}

fn pack_material(material: &Material, has_normal_map: bool) -> MaterialGpu {
    let a = material.albedo;
    MaterialGpu {
        albedo: [a.r, a.g, a.b, a.a],
        emission: rgb(material.emission, material.emission_energy.max(0.0)),
        params: [
            material.metallic,
            material.roughness,
            material.unshaded as u32 as f32,
            material.alpha_cutoff.unwrap_or(0.0).max(0.0),
        ],
        extra: [
            material.normal_scale,
            has_normal_map as u32 as f32,
            0.0,
            0.0,
        ],
    }
}

fn upload_texture(gpu: &GpuContext, texture: &Texture, label: &str) -> GpuTexture {
    upload_texture_levels(gpu, texture, &texture.mip_chain(), label)
}

/// Uploads `texture` with its mip chain `levels` (see [`Texture::mip_chain`]).
fn upload_texture_levels(
    gpu: &GpuContext,
    texture: &Texture,
    levels: &[(u32, u32, Vec<u8>)],
    label: &str,
) -> GpuTexture {
    let format = match texture.color_space {
        ColorSpace::Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        ColorSpace::Linear => wgpu::TextureFormat::Rgba8Unorm,
    };
    // Textures larger than the GPU allows (8192 on many integrated GPUs) start at the first
    // mip that fits, or are shrunk when they have no mips.
    let max = gpu.max_texture_size();
    let shrunk;
    let levels = match levels.iter().position(|(w, h, _)| *w <= max && *h <= max) {
        Some(0) => levels,
        Some(first) => {
            warn!(
                label,
                width = texture.width,
                height = texture.height,
                max,
                "texture larger than the GPU supports; using a smaller mip"
            );
            &levels[first..]
        }
        None => {
            warn!(
                label,
                width = texture.width,
                height = texture.height,
                max,
                "texture larger than the GPU supports; shrinking it"
            );
            shrunk = [shrink_to_fit(texture, max)];
            &shrunk[..]
        }
    };
    let (width, height) = (levels[0].0, levels[0].1);
    let gpu_texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, (width, height, data)) in levels.iter().enumerate() {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &gpu_texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(*height),
            },
            wgpu::Extent3d {
                width: *width,
                height: *height,
                depth_or_array_layers: 1,
            },
        );
    }
    GpuTexture {
        view: gpu_texture.create_view(&Default::default()),
        sampler: texture.sampler,
    }
}

/// A nearest-neighbour copy of `texture` no larger than `max` on either side.
fn shrink_to_fit(texture: &Texture, max: u32) -> (u32, u32, Vec<u8>) {
    let scale = (texture.width.max(texture.height) as f64 / max as f64).max(1.0);
    let width = ((texture.width as f64 / scale) as u32).clamp(1, max);
    let height = ((texture.height as f64 / scale) as u32).clamp(1, max);
    let mut data = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        let sy = ((y as u64 * texture.height as u64) / height as u64) as usize;
        for x in 0..width {
            let sx = ((x as u64 * texture.width as u64) / width as u64) as usize;
            let at = (sy * texture.width as usize + sx) * 4;
            data.extend_from_slice(&texture.data[at..at + 4]);
        }
    }
    (width, height, data)
}

fn create_sampler(device: &wgpu::Device, settings: &SamplerSettings) -> wgpu::Sampler {
    let address = |wrap| match wrap {
        TextureWrap::Repeat => wgpu::AddressMode::Repeat,
        TextureWrap::ClampToEdge => wgpu::AddressMode::ClampToEdge,
        TextureWrap::MirrorRepeat => wgpu::AddressMode::MirrorRepeat,
    };
    let filter = |f| match f {
        TextureFilter::Nearest => wgpu::FilterMode::Nearest,
        TextureFilter::Linear => wgpu::FilterMode::Linear,
    };
    let all_linear = settings.mipmaps
        && settings.mag_filter == TextureFilter::Linear
        && settings.min_filter == TextureFilter::Linear;
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("material sampler"),
        address_mode_u: address(settings.wrap_u),
        address_mode_v: address(settings.wrap_v),
        mag_filter: filter(settings.mag_filter),
        min_filter: filter(settings.min_filter),
        mipmap_filter: if all_linear {
            wgpu::MipmapFilterMode::Linear
        } else {
            wgpu::MipmapFilterMode::Nearest
        },
        // Anisotropy requires every filter to be linear.
        anisotropy_clamp: if all_linear { 8 } else { 1 },
        ..Default::default()
    })
}

fn supported_msaa(gpu: &GpuContext, requested: u32) -> u32 {
    let requested = if requested >= 4 { 4 } else { 1 };
    let ok = |format| {
        gpu.adapter
            .get_texture_format_features(format)
            .flags
            .sample_count_supported(requested)
    };
    if requested > 1 && !(ok(HDR_FORMAT) && ok(DEPTH_FORMAT)) {
        warn!(
            requested,
            "MSAA sample count unsupported by this GPU; anti-aliasing disabled"
        );
        return 1;
    }
    requested
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    dynamic: bool,
    size: usize,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: NonZeroU64::new(size as u64),
        },
        count: None,
    }
}

fn storage_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn create_uniform(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_storage(device: &wgpu::Device, label: &str, size: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

struct SceneBuffers<'a> {
    frame: &'a wgpu::Buffer,
    objects: &'a wgpu::Buffer,
    materials: &'a wgpu::Buffer,
    joints: &'a wgpu::Buffer,
    shadow_pass: &'a wgpu::Buffer,
}

fn create_scene_bind_groups(
    device: &wgpu::Device,
    forward_layout: &wgpu::BindGroupLayout,
    shadow_layout: &wgpu::BindGroupLayout,
    buffers: SceneBuffers<'_>,
    shadow_array_view: &wgpu::TextureView,
    shadow_sampler: &wgpu::Sampler,
    fog_noise: &(wgpu::TextureView, wgpu::Sampler),
) -> (wgpu::BindGroup, wgpu::BindGroup) {
    let forward = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("forward bind group"),
        layout: forward_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffers.frame.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: buffers.objects.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: buffers.materials.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(shadow_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: buffers.joints.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&fog_noise.0),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::Sampler(&fog_noise.1),
            },
        ],
    });
    let shadow = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shadow bind group"),
        layout: shadow_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: buffers.shadow_pass,
                    offset: 0,
                    size: NonZeroU64::new(64),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: buffers.objects.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: buffers.joints.as_entire_binding(),
            },
        ],
    });
    (forward, shadow)
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4
];
const SKIN_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![4 => Uint16x4, 5 => Float32x4];

fn vertex_layouts<'a>(
    vertex_attributes: &'a [wgpu::VertexAttribute],
    skinned: bool,
) -> Vec<Option<wgpu::VertexBufferLayout<'a>>> {
    let mut layouts = vec![Some(wgpu::VertexBufferLayout {
        array_stride: size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: vertex_attributes,
    })];
    if skinned {
        layouts.push(Some(wgpu::VertexBufferLayout {
            array_stride: size_of::<SkinWeights>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &SKIN_ATTRIBUTES,
        }));
    }
    layouts
}

/// The camera's part of the height-fog integral, `exp(-falloff × (camera height - fog
/// height))`, the same for every pixel (1 for fog without falloff).
fn fog_camera_term(fog: &crate::environment::Fog, camera_height: f32) -> f32 {
    let falloff = fog.height_falloff.max(0.0);
    if falloff == 0.0 {
        return 1.0;
    }
    (-falloff * (camera_height - fog.height))
        .clamp(-80.0, 80.0)
        .exp()
}

fn shader(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

fn create_forward_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layouts: &[Option<&wgpu::BindGroupLayout>],
    samples: u32,
    skinned: bool,
    double_sided: bool,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("forward pipeline layout"),
        bind_group_layouts: layouts,
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("forward pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(if skinned { "vs_skinned" } else { "vs_main" }),
            compilation_options: Default::default(),
            buffers: &vertex_layouts(&VERTEX_ATTRIBUTES, skinned),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: (!double_sided).then_some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Light shafts: `fs_shafts` over a half resolution target (group 1 is unused).
fn create_shafts_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    frame_layout: &wgpu::BindGroupLayout,
    shafts_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("light shafts pipeline layout"),
        bind_group_layouts: &[Some(frame_layout), None, Some(shafts_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("light shafts pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_sky"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_shafts"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// The fogged sky: a full-screen triangle on the far plane, only where the depth buffer is
/// still clear.
fn create_sky_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::BindGroupLayout,
    samples: u32,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sky pipeline layout"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_sky"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_sky"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn create_shadow_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::BindGroupLayout,
    skinned: bool,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("shadow pipeline layout"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("shadow pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(if skinned {
                "vs_shadow_skinned"
            } else {
                "vs_shadow"
            }),
            compilation_options: Default::default(),
            buffers: &vertex_layouts(&VERTEX_ATTRIBUTES[..1], skinned),
        },
        // No culling: thin or open meshes (planes) must still cast.
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: SHADOW_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: wgpu::DepthBiasState {
                constant: 2,
                slope_scale: 2.0,
                clamp: 0.0,
            },
        }),
        multisample: Default::default(),
        fragment: None,
        multiview_mask: None,
        cache: None,
    })
}

fn create_post_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    target_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let module = shader(device, "post.wgsl", include_str!("../shaders/post.wgsl"));
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("post pipeline layout"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("post pipeline"),
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
            entry_point: Some("fs_post"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinking_keeps_the_aspect_and_stays_in_bounds() {
        let texture = Texture::new(300, 100, vec![9; 300 * 100 * 4], ColorSpace::Srgb).unwrap();
        let (w, h, data) = shrink_to_fit(&texture, 64);
        assert_eq!((w, h), (64, 21));
        assert_eq!(data.len(), (w * h * 4) as usize);
        assert!(data.iter().all(|&b| b == 9));
        let (w, h, _) = shrink_to_fit(&texture, 4096);
        assert_eq!((w, h), (300, 100), "already fits");
    }
}
