use crate::error::AssetError;
use ab_rendering::{ColorSpace, SamplerSettings, Texture};

/// Largest image side accepted (the limit `Texture` validates too).
const MAX_IMAGE_SIZE: u32 = 16384;

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
    let image = image::load_from_memory(bytes).map_err(error)?.into_rgba8();
    let (width, height) = image.dimensions();
    Texture::new(width, height, image.into_raw(), color_space)
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
