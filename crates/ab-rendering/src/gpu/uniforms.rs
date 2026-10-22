//! `#[repr(C)]` mirrors of the WGSL structs. Everything is built from 16-byte vectors so the
//! Rust and WGSL layouts match without manual padding.

use crate::limits::{MAX_OMNI_LIGHTS, MAX_SPOT_LIGHTS, SHADOW_LAYERS};
use bytemuck::{Pod, Zeroable};

pub(crate) type Mat4 = [[f32; 4]; 4];

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct OmniGpu {
    pub position_range: [f32; 4],
    pub radiance: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct SpotGpu {
    pub position_range: [f32; 4],
    pub direction_cos_outer: [f32; 4],
    pub radiance_cos_inner: [f32; 4],
    pub shadow: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct FrameGpu {
    pub view_proj: Mat4,
    pub camera_position: [f32; 4],
    pub ambient: [f32; 4],
    pub fog_color: [f32; 4],
    pub fog_params: [f32; 4],
    /// sun scattering, anisotropy, camera height term, patchiness
    pub fog_scatter: [f32; 4],
    /// noise offset (wind × time, in patch units), 1 / patch size
    pub fog_wind: [f32; 4],
    pub sun_direction: [f32; 4],
    pub sun_radiance: [f32; 4],
    pub shadow_params: [f32; 4],
    pub light_counts: [u32; 4],
    pub shadow_matrices: [Mat4; SHADOW_LAYERS],
    pub omni: [OmniGpu; MAX_OMNI_LIGHTS],
    pub spot: [SpotGpu; MAX_SPOT_LIGHTS],
    /// For the fogged sky: clip space back to the world.
    pub inverse_view_proj: Mat4,
    /// rgb: the background colour
    pub background: [f32; 4],
    /// Light shafts: intensity, anisotropy, max distance, density
    pub shafts: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ObjectGpu {
    pub model: Mat4,
    pub normal: Mat4,
    /// material index, first joint matrix (skinned only), unused, unused
    pub material: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct MaterialGpu {
    pub albedo: [f32; 4],
    pub emission: [f32; 4],
    /// metallic, roughness, unshaded (0/1), alpha cutoff (0 = off)
    pub params: [f32; 4],
    /// normal scale, has normal map (0/1), unused, unused
    pub extra: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct PostGpu {
    /// exposure, contrast, saturation, tonemap (0/1)
    pub params: [f32; 4],
    pub tint: [f32; 4],
    /// intensity, smoothness, encode sRGB (0/1), unused
    pub vignette: [f32; 4],
    /// chromatic aberration, grain, bloom intensity, seconds (grain animation)
    pub effects: [f32; 4],
    /// Full-screen color mixed over the image (rgb, amount).
    pub overlay: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct BloomGpu {
    /// threshold, soft knee, upsample radius in texels, unused
    pub params: [f32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_match_wgsl_layouts() {
        // WGSL: mat4 = 64, vec4 = 16; structs round to 16.
        assert_eq!(std::mem::size_of::<OmniGpu>(), 32);
        assert_eq!(std::mem::size_of::<SpotGpu>(), 64);
        assert_eq!(std::mem::size_of::<ObjectGpu>(), 144);
        assert_eq!(std::mem::size_of::<MaterialGpu>(), 64);
        assert_eq!(std::mem::size_of::<PostGpu>(), 80);
        assert_eq!(std::mem::size_of::<BloomGpu>(), 16);
        let header = 64 + 16 * 10;
        let expected = header
            + 64 * SHADOW_LAYERS
            + 32 * MAX_OMNI_LIGHTS
            + 64 * MAX_SPOT_LIGHTS
            + 64
            + 16
            + 16;
        assert_eq!(std::mem::size_of::<FrameGpu>(), expected);
    }
}
