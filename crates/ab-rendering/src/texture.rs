//! CPU-side textures (RGBA8) and their sampling settings.

use std::sync::{Arc, Mutex};

/// Width, height and RGBA8 texels of one mip level.
pub type MipLevel = (u32, u32, Vec<u8>);

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
    /// Mips built ahead of time ([`Texture::with_prepared_mips`]).
    prepared: PreparedMips,
}

/// Mip levels built on a loading thread, handed over once to the renderer (which then
/// frees them). Not part of the texture's value: ignored by `==`, shared by clones.
#[derive(Clone, Default)]
struct PreparedMips(Arc<Mutex<Option<Vec<MipLevel>>>>);

impl PartialEq for PreparedMips {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for PreparedMips {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ready = self.0.lock().is_ok_and(|mips| mips.is_some());
        f.write_str(if ready { "prepared" } else { "none" })
    }
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
            prepared: PreparedMips::default(),
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
            prepared: PreparedMips::default(),
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
    pub fn mip_chain(&self) -> Vec<MipLevel> {
        let mut levels = vec![(self.width, self.height, self.data.clone())];
        levels.extend(self.mips());
        levels
    }

    /// The mip levels below level 0 (largest first), as in [`Texture::mip_chain`] without
    /// copying the texture itself. Rows are filtered in parallel on the job pool.
    pub fn mips(&self) -> Vec<MipLevel> {
        let mut levels: Vec<MipLevel> = Vec::new();
        for _ in 1..self.mip_level_count() {
            let (w, h, src) = match levels.last() {
                Some((w, h, data)) => (*w, *h, data.as_slice()),
                None => (self.width, self.height, self.data.as_slice()),
            };
            let level = downsample(w, h, src, self.color_space);
            levels.push(level);
        }
        levels
    }

    /// Builds the mips now (call it on a loading thread), so the renderer does not have to
    /// when the texture is first drawn. Change the texture before this, not after.
    #[must_use]
    pub fn with_prepared_mips(self) -> Self {
        let mips = self.mips();
        if let Ok(mut prepared) = self.prepared.0.lock() {
            *prepared = Some(mips);
        }
        self
    }

    /// Whether mips were prepared ([`Texture::with_prepared_mips`]) and not handed over yet.
    pub fn has_prepared_mips(&self) -> bool {
        self.prepared.0.lock().is_ok_and(|mips| mips.is_some())
    }

    /// The mips ([`Texture::mips`]): the prepared ones the first time (handing them over),
    /// otherwise built now.
    pub fn take_mips(&self) -> Vec<MipLevel> {
        let prepared = self.prepared.0.lock().ok().and_then(|mut p| p.take());
        let expected = self.mip_level_count() as usize - 1;
        let first = ((self.width / 2).max(1), (self.height / 2).max(1));
        match prepared {
            // Built for this size and sampler.
            Some(mips)
                if mips.len() == expected
                    && mips.first().is_none_or(|(w, h, _)| (*w, *h) == first) =>
            {
                mips
            }
            _ => self.mips(),
        }
    }
}

/// One mip level below `src` (`w` × `h`): each texel the average of a 2×2 block.
fn downsample(w: u32, h: u32, src: &[u8], color_space: ColorSpace) -> MipLevel {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut dst = vec![0u8; (nw * nh * 4) as usize];
    let srgb = color_space == ColorSpace::Srgb;
    let tables = srgb.then(srgb_tables);
    let mut rows: Vec<&mut [u8]> = dst.chunks_mut((nw * 4) as usize).collect();
    // Small levels are not worth spreading over threads.
    let min_rows = (16_384 / nw.max(1)).max(1) as usize;
    ab_core::jobs::par_for_each_indexed_mut(&mut rows, min_rows, |y, row| {
        let y = y as u32;
        let (y0, y1) = ((2 * y).min(h - 1), (2 * y + 1).min(h - 1));
        for x in 0..nw {
            let (x0, x1) = ((2 * x).min(w - 1), (2 * x + 1).min(w - 1));
            let at = |sx: u32, sy: u32, c: usize| src[((sy * w + sx) * 4) as usize + c];
            for c in 0..4 {
                let texels = [at(x0, y0, c), at(x1, y0, c), at(x0, y1, c), at(x1, y1, c)];
                row[(x * 4) as usize + c] = match tables {
                    Some((to_linear, thresholds)) if c < 3 => {
                        let sum: f32 = texels.iter().map(|&v| to_linear[v as usize]).sum();
                        // The sRGB value whose range holds the average: the same as
                        // rounding `linear_to_srgb(avg) * 255`, without the `powf`.
                        thresholds.partition_point(|&t| t <= sum / 4.0) as u8
                    }
                    _ => {
                        let sum: u32 = texels.iter().map(|&v| u32::from(v)).sum();
                        ((sum + 2) / 4) as u8
                    }
                };
            }
        }
    });
    (nw, nh, dst)
}

/// sRGB byte → linear value, and the linear values where the rounded sRGB byte steps up
/// (`thresholds[i]`: the start of byte `i + 1`).
fn srgb_tables() -> &'static ([f32; 256], [f32; 255]) {
    static TABLES: std::sync::OnceLock<([f32; 256], [f32; 255])> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| {
        let to_linear = std::array::from_fn(|v| srgb_to_linear(v as u8));
        let thresholds = std::array::from_fn(|i| srgb_unit_to_linear((i as f32 + 0.5) / 255.0));
        (to_linear, thresholds)
    })
}

fn srgb_unit_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn srgb_to_linear(v: u8) -> f32 {
    srgb_unit_to_linear(v as f32 / 255.0)
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
    fn mips_match_the_exact_filter() {
        fn exact(texels: [u8; 4], srgb: bool) -> u8 {
            if !srgb {
                let sum: f32 = texels.iter().map(|&v| v as f32 / 255.0).sum();
                return (sum / 4.0 * 255.0).round() as u8;
            }
            let avg = texels.iter().map(|&v| srgb_to_linear(v)).sum::<f32>() / 4.0;
            let out = if avg <= 0.0031308 {
                avg * 12.92
            } else {
                1.055 * avg.powf(1.0 / 2.4) - 0.055
            };
            (out * 255.0).round() as u8
        }
        let (w, h) = (37u32, 22u32);
        let data: Vec<u8> = (0..w * h * 4)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        for space in [ColorSpace::Srgb, ColorSpace::Linear] {
            let texture = Texture::new(w, h, data.clone(), space).unwrap();
            let (nw, nh, mip) = &texture.mips()[0];
            let mut off_by_one = 0;
            for y in 0..*nh {
                for x in 0..*nw {
                    for c in 0..4 {
                        let at = |sx: u32, sy: u32| data[((sy * w + sx) * 4 + c) as usize];
                        let texels = [
                            at(2 * x, 2 * y),
                            at(2 * x + 1, 2 * y),
                            at(2 * x, 2 * y + 1),
                            at(2 * x + 1, 2 * y + 1),
                        ];
                        let want = exact(texels, space == ColorSpace::Srgb && c < 3);
                        let got = mip[((y * nw + x) * 4 + c) as usize];
                        assert!(got.abs_diff(want) <= 1, "{space:?} ({x}, {y}).{c}");
                        off_by_one += usize::from(got != want);
                    }
                }
            }
            // Only float ties may round the other way.
            assert!(off_by_one <= 2, "{space:?}: {off_by_one} values differ");
        }
    }

    #[test]
    fn prepared_mips_are_handed_over_once() {
        let data: Vec<u8> = (0..16 * 8 * 4).map(|i| i as u8).collect();
        let texture = Texture::new(16, 8, data, ColorSpace::Srgb).unwrap();
        let built = texture.mips();
        let prepared = texture.clone().with_prepared_mips();
        assert_eq!(prepared, texture, "prepared mips do not change the value");
        assert_eq!(prepared.take_mips(), built);
        assert_eq!(prepared.take_mips(), built, "built again once taken");

        // Prepared for other settings: rebuilt.
        let stale = texture
            .clone()
            .with_prepared_mips()
            .with_sampler(SamplerSettings {
                mipmaps: false,
                ..Default::default()
            });
        assert!(stale.take_mips().is_empty());
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
