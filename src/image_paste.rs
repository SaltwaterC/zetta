//! Clipboard image validation and PNG normalization shared by image stores.

use std::io::Cursor;

use anyhow::{Context as _, Result, bail};
use gpui::{Image, ImageFormat};

const MAX_IMAGE_DIMENSION: u32 = 16_384;
const MAX_DECODED_IMAGE_BYTES: u64 = 256 * 1024 * 1024;

/// Converts a supported clipboard raster image to the bounded PNG payload used
/// by both the local SSH fallback and the session multiplexer.
pub(crate) fn normalize_image(image: &Image) -> Result<Vec<u8>> {
    anyhow::ensure!(
        !image.bytes.is_empty() && image.bytes.len() <= zmux::messages::MAX_IMAGE_BYTES,
        "clipboard image is empty or larger than {} bytes",
        zmux::messages::MAX_IMAGE_BYTES
    );
    let Some(format) = image_format(image.format) else {
        bail!(
            "clipboard image format {} cannot be converted to PNG",
            image.format.extension()
        );
    };
    let mut reader = image::ImageReader::with_format(Cursor::new(&image.bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_IMAGE_BYTES);
    reader.limits(limits);
    let decoded = reader.decode().context("decoding clipboard image")?;
    let mut bytes = Vec::new();
    decoded
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .context("encoding clipboard image as PNG")?;
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len() <= zmux::messages::MAX_IMAGE_BYTES,
        "normalized clipboard image is larger than {} bytes",
        zmux::messages::MAX_IMAGE_BYTES
    );
    Ok(bytes)
}

fn image_format(format: ImageFormat) -> Option<image::ImageFormat> {
    Some(match format {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageFormat::Webp => image::ImageFormat::WebP,
        ImageFormat::Gif => image::ImageFormat::Gif,
        ImageFormat::Bmp => image::ImageFormat::Bmp,
        ImageFormat::Tiff => image::ImageFormat::Tiff,
        ImageFormat::Ico => image::ImageFormat::Ico,
        ImageFormat::Pnm => image::ImageFormat::Pnm,
        ImageFormat::Svg => return None,
    })
}

#[cfg(test)]
#[path = "tests/image_paste.rs"]
mod tests;
