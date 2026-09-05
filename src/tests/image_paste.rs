use super::*;

#[test]
fn raster_clipboard_images_are_normalized_to_png() {
    let mut source = Vec::new();
    image::DynamicImage::new_rgba8(1, 1)
        .write_to(
            &mut std::io::Cursor::new(&mut source),
            image::ImageFormat::Png,
        )
        .unwrap();
    let image = gpui::Image {
        format: gpui::ImageFormat::Png,
        bytes: source,
        id: 1,
    };
    let normalized = normalize_image(&image).unwrap();
    assert!(normalized.starts_with(b"\x89PNG\r\n\x1a\n"));
}

#[test]
fn svg_clipboard_images_are_rejected() {
    let image = gpui::Image {
        format: gpui::ImageFormat::Svg,
        bytes: b"<svg/>".to_vec(),
        id: 1,
    };
    assert!(normalize_image(&image).is_err());
}
