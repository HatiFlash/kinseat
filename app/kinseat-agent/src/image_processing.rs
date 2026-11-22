use crate::dimensions;
use image::{imageops::FilterType, DynamicImage, ImageBuffer, Rgb};

pub fn jpeg_from_bgra(
    pixels: &[u8],
    width: u32,
    height: u32,
    max_width: u32,
) -> Result<(Vec<u8>, u32, u32), String> {
    if width == 0
        || height == 0
        || max_width == 0
        || pixels.len() != width as usize * height as usize * 4
    {
        return Err("image invalide".into());
    }
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for px in pixels.chunks_exact(4) {
        rgb.extend_from_slice(&[px[2], px[1], px[0]]);
    }
    let img = ImageBuffer::<Rgb<u8>, _>::from_raw(width, height, rgb).ok_or("image invalide")?;
    let (w, h) = dimensions(width, height, max_width);
    let img = DynamicImage::ImageRgb8(img).resize_exact(w, h, FilterType::Triangle);
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 70)
        .encode_image(&img)
        .map_err(|e| e.to_string())?;
    Ok((bytes, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn downsizes_and_encodes_in_memory() {
        let (jpeg, w, h) = jpeg_from_bgra(&[0, 0, 255, 255].repeat(8), 4, 2, 2).unwrap();
        assert_eq!((w, h), (2, 1));
        let decoded = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 1));
        assert!(decoded.to_rgb8().get_pixel(0, 0)[0] > 200);
        assert!(jpeg_from_bgra(&[], 1, 1, 1).is_err());
    }
}
