//! CPU-side textures (RGBA8) and their sampling settings.

/// How texel values are interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorSpace {
    /// Colors authored for display (albedo, emission). Decoded to linear when sampled.
    Srgb,
    /// Data (normal maps, metallic/roughness).
    Linear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextureWrap {
    #[default]
    Repeat,
    ClampToEdge,
    MirrorRepeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextureFilter {
    Nearest,
    #[default]
    Linear,
}

/// Sampler settings stored with each texture (glTF samplers map onto these).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SamplerSettings {
    pub wrap_u: TextureWrap,
    pub wrap_v: TextureWrap,
    pub mag_filter: TextureFilter,
    pub min_filter: TextureFilter,
    /// Generate and sample mipmaps (trilinear when filters are linear).
    pub mipmaps: bool,
}

impl Default for SamplerSettings {
    fn default() -> Self {
        Self {
            wrap_u: TextureWrap::Repeat,
            wrap_v: TextureWrap::Repeat,
            mag_filter: TextureFilter::Linear,
            min_filter: TextureFilter::Linear,
            mipmaps: true,
        }
    }
}

/// An RGBA8 image ready for upload.
#[derive(Debug, Clone, PartialEq)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8 rows, top row first.
    pub data: Vec<u8>,
    pub color_space: ColorSpace,
    pub sampler: SamplerSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextureError {
    #[error("texture size {width}x{height} is invalid")]
    InvalidSize { width: u32, height: u32 },
    #[error("expected {expected} bytes of RGBA8 data, got {actual}")]
    DataLength { expected: usize, actual: usize },
}

impl Texture {
    pub fn new(
        width: u32,
        height: u32,
        data: Vec<u8>,
        color_space: ColorSpace,
    ) -> Result<Self, TextureError> {
        let texture = Self {
            width,
            height,
            data,
            color_space,
            sampler: SamplerSettings::default(),
        };
        texture.validate()?;
        Ok(texture)
    }

    /// 1×1 texture of a single RGBA8 color.
    pub fn solid(rgba: [u8; 4], color_space: ColorSpace) -> Self {
        Self {
            width: 1,
            height: 1,
            data: rgba.to_vec(),
            color_space,
            sampler: SamplerSettings::default(),
        }
    }

    #[must_use]
    pub fn with_sampler(mut self, sampler: SamplerSettings) -> Self {
        self.sampler = sampler;
        self
    }

    pub fn validate(&self) -> Result<(), TextureError> {
        if self.width == 0 || self.height == 0 || self.width > 16384 || self.height > 16384 {
            return Err(TextureError::InvalidSize {
                width: self.width,
                height: self.height,
            });
        }
        let expected = self.width as usize * self.height as usize * 4;
        if self.data.len() != expected {
            return Err(TextureError::DataLength {
                expected,
                actual: self.data.len(),
            });
        }
        Ok(())
    }

    /// Number of mip levels for this size (1 when mipmaps are disabled).
    pub fn mip_level_count(&self) -> u32 {
        if self.sampler.mipmaps {
            32 - self.width.max(self.height).leading_zeros()
        } else {
            1
        }
    }

    /// The full mip chain, level 0 first, built with a 2×2 box filter (odd edges are clamped).
    /// sRGB data is averaged in linear space so mips do not darken.
    pub fn mip_chain(&self) -> Vec<(u32, u32, Vec<u8>)> {
        let mut levels = vec![(self.width, self.height, self.data.clone())];
        for _ in 1..self.mip_level_count() {
            let (w, h, src) = levels.last().expect("level 0 exists");
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            let mut dst = vec![0u8; (nw * nh * 4) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    for c in 0..4 {
                        let srgb = self.color_space == ColorSpace::Srgb && c < 3;
                        let mut sum = 0.0;
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let sx = (2 * x + dx).min(w - 1);
                            let sy = (2 * y + dy).min(h - 1);
                            let v = src[((sy * w + sx) * 4 + c) as usize];
                            sum += if srgb {
                                srgb_to_linear(v)
                            } else {
                                v as f32 / 255.0
                            };
                        }
                        let avg = sum / 4.0;
                        let out = if srgb { linear_to_srgb(avg) } else { avg };
                        dst[((y * nw + x) * 4 + c) as usize] = (out * 255.0).round() as u8;
                    }
                }
            }
            levels.push((nw, nh, dst));
        }
        levels
    }
}

fn srgb_to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_size_and_length() {
        assert!(Texture::new(2, 2, vec![0; 16], ColorSpace::Linear).is_ok());
        assert_eq!(
            Texture::new(2, 2, vec![0; 15], ColorSpace::Linear),
            Err(TextureError::DataLength {
                expected: 16,
                actual: 15
            })
        );
        assert!(matches!(
            Texture::new(0, 2, vec![], ColorSpace::Linear),
            Err(TextureError::InvalidSize { .. })
        ));
    }

    #[test]
    fn mip_chain_halves_down_to_one_texel() {
        let texture = Texture::new(8, 3, vec![255; 8 * 3 * 4], ColorSpace::Linear).unwrap();
        let sizes: Vec<_> = texture
            .mip_chain()
            .iter()
            .map(|(w, h, _)| (*w, *h))
            .collect();
        assert_eq!(sizes, [(8, 3), (4, 1), (2, 1), (1, 1)]);
    }

    #[test]
    fn linear_mips_average_and_srgb_mips_preserve_brightness() {
        // Checkerboard of black and white.
        let mut data = Vec::new();
        for i in 0..4 {
            let v = if i == 0 || i == 3 { 255 } else { 0 };
            data.extend([v, v, v, 255]);
        }
        let linear = Texture::new(2, 2, data.clone(), ColorSpace::Linear).unwrap();
        assert_eq!(linear.mip_chain()[1].2, [128, 128, 128, 255]);
        // 50% linear grey is ~188 in sRGB, not 128.
        let srgb = Texture::new(2, 2, data, ColorSpace::Srgb).unwrap();
        assert_eq!(srgb.mip_chain()[1].2, [188, 188, 188, 255]);
    }

    #[test]
    fn mipmaps_can_be_disabled() {
        let texture =
            Texture::solid([1, 2, 3, 4], ColorSpace::Srgb).with_sampler(SamplerSettings {
                mipmaps: false,
                ..Default::default()
            });
        assert_eq!(texture.mip_chain().len(), 1);
    }
}
