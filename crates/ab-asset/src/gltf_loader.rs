//! glTF 2.0 → engine resources.
//!
//! Supported: `.gltf` (embedded data URIs or external files) and `.glb`; triangle meshes with
//! normals, `TEXCOORD_0`, tangents, `JOINTS_0`/`WEIGHTS_0`; metallic-roughness materials with
//! base color, metallic-roughness, normal and emissive textures, alpha mask, double-sided,
//! `KHR_materials_unlit` and `KHR_materials_emissive_strength`; skins; translation, rotation and
//! scale animations (step, linear, cubic spline).
//!
//! Not yet supported (skipped with a warning): non-triangle primitives, morph targets,
//! `BLEND` alpha (rendered opaque), extra UV sets, cameras and lights.

use crate::base64::decode_data_uri;
use crate::error::AssetError;
use crate::image_loader::decode_image;
use crate::source::{resolve_relative, AssetSource};
use ab_animation::{
    AnimationClip, AnimationLibrary, Channel, ClipHandle, Interpolation, Keyframes,
};
use ab_core::{Color, Mat4, Quat, Transform3D, Vec2, Vec3};
use ab_rendering::{
    ColorSpace, Material, MaterialHandle, Mesh, MeshHandle, RenderAssets, SamplerSettings,
    SkinWeights, TextureFilter, TextureHandle, TextureWrap, Vertex,
};
use gltf::animation::util::ReadOutputs;
use gltf::texture::{MagFilter, MinFilter, WrappingMode};
use std::collections::HashMap;
use tracing::warn;

/// One drawable part of a glTF mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GltfPrimitive {
    pub mesh: MeshHandle,
    pub material: MaterialHandle,
    pub skinned: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GltfMesh {
    pub name: String,
    pub primitives: Vec<GltfPrimitive>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GltfSkin {
    /// Node indices.
    pub joints: Vec<usize>,
    pub inverse_bind_matrices: Vec<Mat4>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GltfNode {
    pub name: String,
    pub transform: Transform3D,
    pub children: Vec<usize>,
    pub mesh: Option<usize>,
    pub skin: Option<usize>,
}

/// A loaded glTF file: resources are registered, the node hierarchy is kept as a template to
/// instantiate any number of times (Godot's `PackedScene`).
#[derive(Debug, Clone, PartialEq)]
pub struct GltfScene {
    /// Normalised asset path it was loaded from.
    pub path: String,
    pub nodes: Vec<GltfNode>,
    /// Top-level nodes of the default scene.
    pub roots: Vec<usize>,
    pub meshes: Vec<GltfMesh>,
    pub skins: Vec<GltfSkin>,
    /// Clip names and handles; channel targets are node indices.
    pub animations: Vec<(String, ClipHandle)>,
    pub materials: Vec<MaterialHandle>,
    pub textures: Vec<TextureHandle>,
}

impl GltfScene {
    /// File stem of the source path (`models/robot.glb` → `robot`).
    pub fn name(&self) -> &str {
        let file = self.path.rsplit('/').next().unwrap_or(&self.path);
        file.split('.')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("Scene")
    }
}

struct Loader<'a> {
    path: &'a str,
    buffers: Vec<Vec<u8>>,
    source: &'a dyn AssetSource,
    render: &'a mut RenderAssets,
    textures: HashMap<(usize, ColorSpace), TextureHandle>,
    default_material: Option<MaterialHandle>,
}

pub(crate) fn load(
    path: &str,
    bytes: &[u8],
    source: &dyn AssetSource,
    render: &mut RenderAssets,
    library: &mut AnimationLibrary,
) -> Result<GltfScene, AssetError> {
    let gltf = gltf::Gltf::from_slice(bytes).map_err(|source| AssetError::Gltf {
        path: path.to_owned(),
        source,
    })?;
    let buffers = load_buffers(path, &gltf, source)?;
    let document = &gltf.document;
    let mut loader = Loader {
        path,
        buffers,
        source,
        render,
        textures: HashMap::new(),
        default_material: None,
    };

    loader.decode_textures(document)?;
    let materials = document
        .materials()
        .map(|m| loader.material(&m))
        .collect::<Result<Vec<_>, _>>()?;
    let meshes = document
        .meshes()
        .map(|m| loader.mesh(&m, &materials))
        .collect::<Result<Vec<_>, _>>()?;
    let skins = document.skins().map(|s| loader.skin(&s)).collect();
    let nodes = document.nodes().map(node).collect();
    let roots = roots(document);
    let animations = document
        .animations()
        .map(|a| {
            let clip = loader.animation(&a);
            let name = clip.name.clone();
            (name, library.add(clip))
        })
        .collect();
    let mut textures: Vec<TextureHandle> = loader.textures.values().copied().collect();
    textures.sort();

    Ok(GltfScene {
        path: path.to_owned(),
        nodes,
        roots,
        meshes,
        skins,
        animations,
        materials,
        textures,
    })
}

fn load_buffers(
    path: &str,
    gltf: &gltf::Gltf,
    source: &dyn AssetSource,
) -> Result<Vec<Vec<u8>>, AssetError> {
    gltf.document
        .buffers()
        .map(|buffer| {
            let data = match buffer.source() {
                gltf::buffer::Source::Bin => gltf
                    .blob
                    .clone()
                    .ok_or_else(|| AssetError::invalid(path, "missing GLB binary chunk"))?,
                gltf::buffer::Source::Uri(uri) => read_uri(path, uri, source)?,
            };
            if data.len() < buffer.length() {
                return Err(AssetError::invalid(
                    path,
                    format!(
                        "buffer {} has {} bytes, expected {}",
                        buffer.index(),
                        data.len(),
                        buffer.length()
                    ),
                ));
            }
            Ok(data)
        })
        .collect()
}

/// Reads a data URI or a file relative to the glTF.
fn read_uri(path: &str, uri: &str, source: &dyn AssetSource) -> Result<Vec<u8>, AssetError> {
    if uri.starts_with("data:") {
        return decode_data_uri(uri).ok_or_else(|| AssetError::invalid(path, "malformed data URI"));
    }
    let file = resolve_relative(path, uri)
        .ok_or_else(|| AssetError::invalid(path, format!("URI `{uri}` escapes the asset root")))?;
    source
        .read(&file)
        .map_err(|source| AssetError::Io { path: file, source })
}

fn sampler_settings(sampler: &gltf::texture::Sampler<'_>) -> SamplerSettings {
    let wrap = |mode| match mode {
        WrappingMode::Repeat => TextureWrap::Repeat,
        WrappingMode::ClampToEdge => TextureWrap::ClampToEdge,
        WrappingMode::MirroredRepeat => TextureWrap::MirrorRepeat,
    };
    let (min_filter, mipmaps) = match sampler.min_filter() {
        Some(MinFilter::Nearest) => (TextureFilter::Nearest, false),
        Some(MinFilter::Linear) => (TextureFilter::Linear, false),
        Some(MinFilter::NearestMipmapNearest | MinFilter::NearestMipmapLinear) => {
            (TextureFilter::Nearest, true)
        }
        Some(MinFilter::LinearMipmapNearest | MinFilter::LinearMipmapLinear) | None => {
            (TextureFilter::Linear, true)
        }
    };
    SamplerSettings {
        wrap_u: wrap(sampler.wrap_s()),
        wrap_v: wrap(sampler.wrap_t()),
        mag_filter: match sampler.mag_filter() {
            Some(MagFilter::Nearest) => TextureFilter::Nearest,
            _ => TextureFilter::Linear,
        },
        min_filter,
        mipmaps,
    }
}

/// Every texture a material samples, with the color space it is sampled in, in first-use
/// order.
fn material_textures(document: &gltf::Document) -> Vec<(gltf::Texture<'_>, ColorSpace)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for m in document.materials() {
        let pbr = m.pbr_metallic_roughness();
        let slots = [
            pbr.base_color_texture()
                .map(|i| (i.texture(), ColorSpace::Srgb)),
            pbr.metallic_roughness_texture()
                .map(|i| (i.texture(), ColorSpace::Linear)),
            m.normal_texture()
                .map(|n| (n.texture(), ColorSpace::Linear)),
            m.emissive_texture()
                .map(|i| (i.texture(), ColorSpace::Srgb)),
        ];
        for (texture, space) in slots.into_iter().flatten() {
            if seen.insert((texture.index(), space)) {
                out.push((texture, space));
            }
        }
    }
    out
}

impl Loader<'_> {
    /// Decodes every material texture up front, in parallel on the [`ab_core::jobs`]
    /// workers (PNG/JPEG decoding dominates model load times), and caches the handles for
    /// [`Loader::texture`].
    fn decode_textures(&mut self, document: &gltf::Document) -> Result<(), AssetError> {
        enum Bytes<'b> {
            View(&'b [u8]),
            Owned(Vec<u8>),
        }
        let used = material_textures(document);
        // Reading goes through the asset source, serially; views borrow the buffers.
        let mut pending = Vec::with_capacity(used.len());
        for (texture, space) in &used {
            let image = texture.source();
            let bytes = match image.source() {
                gltf::image::Source::View { view, .. } => {
                    let buffer = &self.buffers[view.buffer().index()];
                    let range = view.offset()..view.offset() + view.length();
                    Bytes::View(buffer.get(range).ok_or_else(|| {
                        AssetError::invalid(self.path, "image view out of bounds")
                    })?)
                }
                gltf::image::Source::Uri { uri, .. } => {
                    Bytes::Owned(read_uri(self.path, uri, self.source)?)
                }
            };
            let label = format!("{}#image{}", self.path, image.index());
            pending.push((label, bytes, *space, sampler_settings(&texture.sampler())));
        }
        let decoded = ab_core::jobs::par_map(&pending, 1, |(label, bytes, space, sampler)| {
            let bytes = match bytes {
                Bytes::View(bytes) => bytes,
                Bytes::Owned(bytes) => bytes.as_slice(),
            };
            decode_image(label, bytes, *space, *sampler)
        });
        for (((texture, space), (label, ..)), decoded) in used.iter().zip(&pending).zip(decoded) {
            let handle = self
                .render
                .add_texture(decoded?)
                .map_err(|e| AssetError::invalid(label, e.to_string()))?;
            self.textures.insert((texture.index(), *space), handle);
        }
        Ok(())
    }

    fn texture(
        &mut self,
        texture: gltf::Texture<'_>,
        tex_coord: u32,
        color_space: ColorSpace,
    ) -> Result<TextureHandle, AssetError> {
        if tex_coord != 0 {
            warn!(
                path = self.path,
                tex_coord, "only TEXCOORD_0 is supported; using it instead"
            );
        }
        let key = (texture.index(), color_space);
        if let Some(&handle) = self.textures.get(&key) {
            return Ok(handle);
        }
        let image = texture.source();
        let label = format!("{}#image{}", self.path, image.index());
        let bytes = match image.source() {
            gltf::image::Source::View { view, .. } => {
                let buffer = &self.buffers[view.buffer().index()];
                let range = view.offset()..view.offset() + view.length();
                buffer
                    .get(range)
                    .ok_or_else(|| AssetError::invalid(self.path, "image view out of bounds"))?
                    .to_vec()
            }
            gltf::image::Source::Uri { uri, .. } => read_uri(self.path, uri, self.source)?,
        };
        let decoded = decode_image(
            &label,
            &bytes,
            color_space,
            sampler_settings(&texture.sampler()),
        )?;
        let handle = self
            .render
            .add_texture(decoded)
            .map_err(|e| AssetError::invalid(&label, e.to_string()))?;
        self.textures.insert(key, handle);
        Ok(handle)
    }

    fn material(&mut self, m: &gltf::Material<'_>) -> Result<MaterialHandle, AssetError> {
        let pbr = m.pbr_metallic_roughness();
        let [r, g, b, a] = pbr.base_color_factor();
        let [er, eg, eb] = m.emissive_factor();
        let mut material = Material {
            albedo: Color::rgba(r, g, b, a),
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            emission: Color::rgb(er, eg, eb),
            emission_energy: m.emissive_strength().unwrap_or(1.0),
            double_sided: m.double_sided(),
            unshaded: m.unlit(),
            ..Material::default()
        };
        if let Some(info) = pbr.base_color_texture() {
            material.albedo_texture =
                Some(self.texture(info.texture(), info.tex_coord(), ColorSpace::Srgb)?);
        }
        if let Some(info) = pbr.metallic_roughness_texture() {
            material.metallic_roughness_texture =
                Some(self.texture(info.texture(), info.tex_coord(), ColorSpace::Linear)?);
        }
        if let Some(normal) = m.normal_texture() {
            material.normal_texture =
                Some(self.texture(normal.texture(), normal.tex_coord(), ColorSpace::Linear)?);
            material.normal_scale = normal.scale();
        }
        if let Some(info) = m.emissive_texture() {
            material.emission_texture =
                Some(self.texture(info.texture(), info.tex_coord(), ColorSpace::Srgb)?);
        }
        match m.alpha_mode() {
            gltf::material::AlphaMode::Opaque => {}
            gltf::material::AlphaMode::Mask => {
                material.alpha_cutoff = Some(m.alpha_cutoff().unwrap_or(0.5))
            }
            gltf::material::AlphaMode::Blend => {
                warn!(
                    path = self.path,
                    material = m.name(),
                    "BLEND alpha is not supported yet; rendering opaque"
                );
            }
        }
        Ok(self.render.add_material(material))
    }

    fn default_material(&mut self) -> MaterialHandle {
        *self
            .default_material
            .get_or_insert_with(|| self.render.add_material(Material::default()))
    }

    fn mesh(
        &mut self,
        m: &gltf::Mesh<'_>,
        materials: &[MaterialHandle],
    ) -> Result<GltfMesh, AssetError> {
        let name = m
            .name()
            .map_or_else(|| format!("Mesh{}", m.index()), str::to_owned);
        let mut primitives = Vec::new();
        for primitive in m.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                warn!(path = self.path, mesh = name, mode = ?primitive.mode(), "skipping non-triangle primitive");
                continue;
            }
            if primitive.morph_targets().next().is_some() {
                warn!(
                    path = self.path,
                    mesh = name,
                    "morph targets are not supported; using the base shape"
                );
            }
            let mesh = self.primitive_mesh(&primitive, &name)?;
            let skinned = mesh.is_skinned();
            let mesh = self
                .render
                .add_mesh(mesh)
                .map_err(|e| AssetError::invalid(self.path, format!("mesh `{name}`: {e}")))?;
            let material = match primitive.material().index() {
                Some(i) => materials[i],
                None => self.default_material(),
            };
            primitives.push(GltfPrimitive {
                mesh,
                material,
                skinned,
            });
        }
        Ok(GltfMesh { name, primitives })
    }

    fn primitive_mesh(
        &self,
        primitive: &gltf::Primitive<'_>,
        name: &str,
    ) -> Result<Mesh, AssetError> {
        let buffers = &self.buffers;
        let reader = primitive.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
        let positions: Vec<[f32; 3]> = reader
            .read_positions()
            .ok_or_else(|| {
                AssetError::invalid(self.path, format!("mesh `{name}` has no positions"))
            })?
            .collect();
        let count = positions.len();
        let normals: Option<Vec<[f32; 3]>> = reader.read_normals().map(Iterator::collect);
        let uvs: Option<Vec<[f32; 2]>> = reader.read_tex_coords(0).map(|t| t.into_f32().collect());
        let tangents: Option<Vec<[f32; 4]>> = reader.read_tangents().map(Iterator::collect);
        let indices: Vec<u32> = match reader.read_indices() {
            Some(indices) => indices.into_u32().collect(),
            None => (0..count as u32).collect(),
        };

        let mut vertices: Vec<Vertex> = (0..count)
            .map(|i| {
                Vertex::new(
                    Vec3::from(positions[i]),
                    normals
                        .as_ref()
                        .and_then(|n| n.get(i))
                        .map_or(Vec3::ZERO, |&n| Vec3::from(n)),
                    uvs.as_ref()
                        .and_then(|t| t.get(i))
                        .map_or(Vec2::ZERO, |&t| Vec2::from(t)),
                )
            })
            .collect();
        if let Some(tangents) = &tangents {
            for (vertex, tangent) in vertices.iter_mut().zip(tangents) {
                vertex.tangent = *tangent;
            }
        }
        let mut mesh = Mesh::new(vertices, indices);
        if normals.as_ref().is_none_or(|n| n.len() != count) {
            mesh.compute_normals();
        }
        if tangents.as_ref().is_none_or(|t| t.len() != count) {
            mesh.compute_tangents();
        }

        if let (Some(joints), Some(weights)) = (reader.read_joints(0), reader.read_weights(0)) {
            let skin: Vec<SkinWeights> = joints
                .into_u16()
                .zip(weights.into_f32())
                .map(|(joints, mut weights)| {
                    let sum: f32 = weights.iter().sum();
                    if sum > 0.0 {
                        weights.iter_mut().for_each(|w| *w /= sum);
                    } else {
                        weights = [1.0, 0.0, 0.0, 0.0];
                    }
                    SkinWeights { joints, weights }
                })
                .collect();
            mesh = mesh.with_skin(skin);
        }
        Ok(mesh)
    }

    fn skin(&self, skin: &gltf::Skin<'_>) -> GltfSkin {
        let buffers = &self.buffers;
        let joints: Vec<usize> = skin.joints().map(|j| j.index()).collect();
        let reader = skin.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
        let mut inverse_bind_matrices: Vec<Mat4> = reader
            .read_inverse_bind_matrices()
            .map(|m| m.map(|cols| Mat4::from_cols_array_2d(&cols)).collect())
            .unwrap_or_default();
        inverse_bind_matrices.resize(joints.len(), Mat4::IDENTITY);
        GltfSkin {
            joints,
            inverse_bind_matrices,
        }
    }

    fn animation(&self, animation: &gltf::Animation<'_>) -> AnimationClip {
        let buffers = &self.buffers;
        let name = animation
            .name()
            .map_or_else(|| format!("Animation{}", animation.index()), str::to_owned);
        let mut channels = Vec::new();
        for channel in animation.channels() {
            let reader = channel.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
            let (Some(inputs), Some(outputs)) = (reader.read_inputs(), reader.read_outputs())
            else {
                continue;
            };
            let keyframes = match outputs {
                ReadOutputs::Translations(v) => Keyframes::Translation(v.map(Vec3::from).collect()),
                ReadOutputs::Rotations(v) => {
                    Keyframes::Rotation(v.into_f32().map(Quat::from_array).collect())
                }
                ReadOutputs::Scales(v) => Keyframes::Scale(v.map(Vec3::from).collect()),
                ReadOutputs::MorphTargetWeights(_) => {
                    warn!(
                        path = self.path,
                        animation = name,
                        "morph target animation is not supported"
                    );
                    continue;
                }
            };
            channels.push(Channel {
                target: channel.target().node().index(),
                interpolation: match channel.sampler().interpolation() {
                    gltf::animation::Interpolation::Step => Interpolation::Step,
                    gltf::animation::Interpolation::Linear => Interpolation::Linear,
                    gltf::animation::Interpolation::CubicSpline => Interpolation::CubicSpline,
                },
                times: inputs.collect(),
                keyframes,
            });
        }
        let clip = AnimationClip::new(name, channels);
        if clip.channels.len() < animation.channels().count() {
            warn!(
                path = self.path,
                animation = clip.name,
                "some animation channels were invalid and dropped"
            );
        }
        clip
    }
}

fn node(node: gltf::Node<'_>) -> GltfNode {
    let (translation, rotation, scale) = node.transform().decomposed();
    GltfNode {
        name: node
            .name()
            .map_or_else(|| format!("Node{}", node.index()), str::to_owned),
        transform: Transform3D::from_trs(
            Vec3::from(translation),
            Quat::from_array(rotation).normalize(),
            Vec3::from(scale),
        ),
        children: node.children().map(|c| c.index()).collect(),
        mesh: node.mesh().map(|m| m.index()),
        skin: node.skin().map(|s| s.index()),
    }
}

/// Root nodes of the default scene; without scenes, every node that is nobody's child.
fn roots(document: &gltf::Document) -> Vec<usize> {
    if let Some(scene) = document
        .default_scene()
        .or_else(|| document.scenes().next())
    {
        return scene.nodes().map(|n| n.index()).collect();
    }
    let mut is_child = vec![false; document.nodes().len()];
    for node in document.nodes() {
        for child in node.children() {
            is_child[child.index()] = true;
        }
    }
    (0..is_child.len()).filter(|&i| !is_child[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node_from_matrix(matrix: Mat4) -> GltfNode {
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"nodes":[{{"matrix":{:?}}}]}}"#,
            matrix.to_cols_array()
        );
        let gltf = gltf::Gltf::from_slice(json.as_bytes()).unwrap();
        node(gltf.document.nodes().next().unwrap())
    }

    #[test]
    fn node_matrices_round_trip_including_mirrored_ones() {
        let cases = [
            Mat4::from_scale_rotation_translation(
                Vec3::new(-1.0, 1.0, 1.0),
                Quat::from_rotation_y(0.7),
                Vec3::new(1.0, 2.0, 3.0),
            ),
            Mat4::from_scale_rotation_translation(
                Vec3::new(1.0, -2.0, 0.5),
                Quat::from_euler(ab_core::EulerRot::XYZ, 0.3, -1.2, 2.5),
                Vec3::ZERO,
            ),
            Mat4::from_scale_rotation_translation(
                Vec3::new(2.0, 3.0, -4.0),
                Quat::from_rotation_x(3.0),
                Vec3::new(-5.0, 0.0, 1.0),
            ),
            Mat4::from_rotation_z(std::f32::consts::PI),
        ];
        for matrix in cases {
            let node = node_from_matrix(matrix);
            let back = node.transform.to_matrix();
            assert!(
                back.abs_diff_eq(matrix, 1e-4),
                "{matrix:?}\n decomposed to {:?}\n which is {back:?}",
                node.transform
            );
        }
    }
}
