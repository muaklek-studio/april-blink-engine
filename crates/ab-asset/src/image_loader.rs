use crate::error::AssetError;
use ab_rendering::{ColorSpace, SamplerSettings, Texture};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Largest image side accepted (the limit `Texture` validates too).
const MAX_IMAGE_SIZE: u32 = 16384;

/// Where decoded images are kept (see [`set_decode_cache`]).
static DECODE_CACHE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Smaller encoded images decode about as fast as they would read back: not cached.
const CACHE_MIN_BYTES: usize = 32 * 1024;
const CACHE_MAGIC: &[u8; 4] = b"ABIM";
const CACHE_VERSION: u32 = 1;
const CACHE_HEADER: usize = 16;

/// Keeps decoded images in `dir` (the editor's project cache), so the next load of the same
/// image (by content, whatever its path) reads its pixels back instead of decoding the PNG
/// or JPEG again. `None` turns the cache off (games do without it).
pub fn set_decode_cache(dir: Option<PathBuf>) {
    *DECODE_CACHE.write().unwrap_or_else(|e| e.into_inner()) = dir;
}

fn cache_dir() -> Option<PathBuf> {
    DECODE_CACHE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// The cache file of an encoded image: named after its content.
fn cache_file(dir: &Path, bytes: &[u8]) -> PathBuf {
    let hash = ab_core::utils::hash::fnv1a64(bytes);
    dir.join(format!("{hash:016x}-{:x}.rgba", bytes.len()))
}

/// Width, height and RGBA pixels of a cache file, if it is whole.
fn read_cached(file: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let data = std::fs::read(file).ok()?;
    if data.get(..4)? != CACHE_MAGIC
        || u32::from_le_bytes(data.get(4..8)?.try_into().ok()?) != CACHE_VERSION
    {
        return None;
    }
    let width = u32::from_le_bytes(data.get(8..12)?.try_into().ok()?);
    let height = u32::from_le_bytes(data.get(12..16)?.try_into().ok()?);
    if width == 0 || height == 0 || width > MAX_IMAGE_SIZE || height > MAX_IMAGE_SIZE {
        return None;
    }
    let pixels = lz4_flex::block::decompress(
        data.get(CACHE_HEADER..)?,
        width as usize * height as usize * 4,
    )
    .ok()?;
    Some((width, height, pixels))
}

fn write_cached(file: &Path, width: u32, height: u32, pixels: &[u8]) {
    let mut data = Vec::with_capacity(CACHE_HEADER + pixels.len() / 2);
    data.extend_from_slice(CACHE_MAGIC);
    data.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    data.extend_from_slice(&width.to_le_bytes());
    data.extend_from_slice(&height.to_le_bytes());
    data.extend_from_slice(&lz4_flex::block::compress(pixels));
    let partial = file.with_extension("partial");
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&partial, &data))
        .and_then(|()| std::fs::rename(&partial, file));
    if let Err(err) = written {
        tracing::debug!("decoded image not cached: {err}");
    }
}

/// Decodes a PNG or JPEG into an RGBA8 [`Texture`]. `path` is only used in errors.
pub fn decode_image(
    path: &str,
    bytes: &[u8],
    color_space: ColorSpace,
    sampler: SamplerSettings,
) -> Result<Texture, AssetError> {
    let error = |source| AssetError::Image {
        path: path.to_owned(),
        source,
    };
    // Check the size before decoding: a huge greyscale image passes the decoder's memory
    // limit and then takes 4× that as RGBA.
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| AssetError::invalid(path, e.to_string()))?
        .into_dimensions()
        .map_err(error)?;
    if width > MAX_IMAGE_SIZE || height > MAX_IMAGE_SIZE {
        return Err(AssetError::invalid(
            path,
            format!("{width}x{height} is larger than {MAX_IMAGE_SIZE} pixels on a side"),
        ));
    }
    let cache = cache_dir()
        .filter(|_| bytes.len() >= CACHE_MIN_BYTES)
        .map(|dir| cache_file(&dir, bytes));
    let cached = cache
        .as_deref()
        .and_then(read_cached)
        .filter(|&(w, h, _)| (w, h) == (width, height));
    let (width, height, pixels) = match cached {
        Some(cached) => cached,
        None => {
            let image = image::load_from_memory(bytes).map_err(error)?.into_rgba8();
            let (width, height) = image.dimensions();
            let pixels = image.into_raw();
            if let Some(file) = &cache {
                write_cached(file, width, height, &pixels);
            }
            (width, height, pixels)
        }
    };
    Texture::new(width, height, pixels, color_space)
        .map(|t| t.with_sampler(sampler))
        .map_err(|e| AssetError::invalid(path, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let pixels: Vec<u8> = rgba.repeat((width * height) as usize);
        image::write_buffer_with_format(
            &mut std::io::Cursor::new(&mut bytes),
            &pixels,
            width,
            height,
            image::ExtendedColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .unwrap();
        bytes
    }

    #[test]
    fn decodes_png() {
        let texture = decode_image(
            "t.png",
            &png(3, 2, [10, 20, 30, 40]),
            ColorSpace::Srgb,
            SamplerSettings::default(),
        )
        .unwrap();
        assert_eq!((texture.width, texture.height), (3, 2));
        assert_eq!(&texture.data[..4], &[10, 20, 30, 40]);
        assert_eq!(texture.color_space, ColorSpace::Srgb);
    }

    #[test]
    fn decoded_images_are_cached_by_content() {
        let dir = std::env::temp_dir().join(format!("ab-decode-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Noise, so the PNG is large enough to be cached.
        let pixels: Vec<u8> = (0..256 * 256 * 4u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let mut bytes = Vec::new();
        image::write_buffer_with_format(
            &mut std::io::Cursor::new(&mut bytes),
            &pixels,
            256,
            256,
            image::ExtendedColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .unwrap();
        assert!(bytes.len() >= CACHE_MIN_BYTES);
        let file = cache_file(&dir, &bytes);
        assert_eq!(read_cached(&file), None);
        write_cached(&file, 256, 256, &pixels);
        let (w, h, back) = read_cached(&file).unwrap();
        assert_eq!((w, h), (256, 256));
        assert_eq!(back, pixels);
        // A damaged file is ignored (decoded again), not trusted.
        std::fs::write(&file, b"ABIM").unwrap();
        assert_eq!(read_cached(&file), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reports_corrupt_images() {
        let err = decode_image(
            "bad.png",
            b"not a png",
            ColorSpace::Srgb,
            Default::default(),
        );
        assert!(matches!(err, Err(AssetError::Image { .. })));
    }
}
