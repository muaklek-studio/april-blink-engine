use crate::assets::TextureHandle;
use crate::shader::{ShaderHandle, MAX_PARAMS};
use ab_core::Color;

/// Surface parameters for the forward PBR shader (metallic/roughness workflow, glTF style).
///
/// Factors multiply their textures: e.g. the final base color is `albedo × albedo_texture`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// Base color, linear space. Alpha is used for alpha-cutoff only (no blending yet).
    pub albedo: Color,
    pub albedo_texture: Option<TextureHandle>,
    /// 0 = dielectric, 1 = metal.
    pub metallic: f32,
    /// 0 = mirror, 1 = fully rough. Clamped to a small minimum in the shader.
    pub roughness: f32,
    /// glTF layout: roughness in G, metallic in B (linear).
    pub metallic_roughness_texture: Option<TextureHandle>,
    /// Tangent-space normal map (linear).
    pub normal_texture: Option<TextureHandle>,
    /// Strength of the normal map's XY.
    pub normal_scale: f32,
    /// Emitted light (linear), multiplied by `emission_energy`. Unaffected by lights.
    pub emission: Color,
    pub emission_energy: f32,
    pub emission_texture: Option<TextureHandle>,
    /// Fragments whose albedo alpha is below this value are discarded (foliage, fences).
    pub alpha_cutoff: Option<f32>,
    /// Render back faces too (and light them with a flipped normal).
    pub double_sided: bool,
    /// Ignore lights; output `albedo` (plus emission) directly.
    pub unshaded: bool,
    /// A custom surface shader (see [`crate::shader`]); `None` is the standard surface.
    pub shader: Option<ShaderHandle>,
    /// The values of the shader's `// param` lines, in order.
    pub shader_params: [f32; MAX_PARAMS],
}

impl Material {
    pub fn from_color(albedo: Color) -> Self {
        Self {
            albedo,
            ..Self::default()
        }
    }

    pub fn unshaded(albedo: Color) -> Self {
        Self {
            albedo,
            unshaded: true,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_albedo_texture(mut self, texture: TextureHandle) -> Self {
        self.albedo_texture = Some(texture);
        self
    }

    #[must_use]
    pub fn with_normal_texture(mut self, texture: TextureHandle) -> Self {
        self.normal_texture = Some(texture);
        self
    }

    #[must_use]
    pub fn with_roughness(mut self, roughness: f32) -> Self {
        self.roughness = roughness;
        self
    }

    #[must_use]
    pub fn with_metallic(mut self, metallic: f32) -> Self {
        self.metallic = metallic;
        self
    }

    #[must_use]
    pub fn with_emission(mut self, emission: Color, energy: f32) -> Self {
        self.emission = emission;
        self.emission_energy = energy;
        self
    }

    /// The four texture slots in shader binding order.
    pub(crate) fn texture_slots(&self) -> [Option<TextureHandle>; 4] {
        [
            self.albedo_texture,
            self.metallic_roughness_texture,
            self.normal_texture,
            self.emission_texture,
        ]
    }
}

impl Default for Material {
    fn default() -> Self {
        Self {
            albedo: Color::WHITE,
            albedo_texture: None,
            metallic: 0.0,
            roughness: 0.6,
            metallic_roughness_texture: None,
            normal_texture: None,
            normal_scale: 1.0,
            emission: Color::BLACK,
            emission_energy: 0.0,
            emission_texture: None,
            alpha_cutoff: None,
            double_sided: false,
            unshaded: false,
            shader: None,
            shader_params: [0.0; MAX_PARAMS],
        }
    }
}
