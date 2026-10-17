//! Still images (PNG, JPG, WebP) decoded upright to RGBA.

use std::path::Path;

use image::{ImageDecoder, ImageReader, RgbaImage};

pub use jvid_core::settings::DEFAULT_STILL_SECS;

/// Decodes a still image to RGBA, applying its EXIF orientation.
pub fn decode_still(path: &Path) -> image::ImageResult<RgbaImage> {
    let mut decoder = ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img.into_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgba};

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);

    /// 4x2 image, red top-left quadrant, rest blue.
    fn asym() -> RgbaImage {
        RgbaImage::from_fn(4, 2, |x, y| {
            if x < 2 && y < 1 {
                RED
            } else {
                Rgba([0, 0, 255, 255])
            }
        })
    }

    fn roundtrip(name: &str, bytes: &[u8]) -> RgbaImage {
        let dir = std::env::temp_dir().join(format!("jvid-stills-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        decode_still(&path).unwrap()
    }

    fn encode(img: &RgbaImage, fmt: ImageFormat) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        match fmt {
            ImageFormat::Jpeg => image::DynamicImage::ImageRgba8(img.clone())
                .into_rgb8()
                .write_to(&mut out, fmt),
            _ => img.write_to(&mut out, fmt),
        }
        .unwrap();
        out.into_inner()
    }

    fn is_red(p: &Rgba<u8>) -> bool {
        p[0] > 200 && p[2] < 60
    }

    #[test]
    fn decodes_png_and_webp_losslessly() {
        for (name, fmt) in [("a.png", ImageFormat::Png), ("a.webp", ImageFormat::WebP)] {
            assert_eq!(roundtrip(name, &encode(&asym(), fmt)), asym(), "{name}");
        }
    }

    #[test]
    fn decodes_jpeg() {
        let img = roundtrip("a.jpg", &encode(&asym(), ImageFormat::Jpeg));
        assert_eq!(img.dimensions(), (4, 2));
        assert!(is_red(img.get_pixel(0, 0)));
    }

    /// Inserts an APP1 EXIF segment with the given orientation after SOI.
    fn with_orientation(jpeg: &[u8], orientation: u8) -> Vec<u8> {
        #[rustfmt::skip]
        let tiff = [
            b'M', b'M', 0, 42, 0, 0, 0, 8, // big-endian header, IFD at 8
            0, 1,                          // one entry
            0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, orientation, 0, 0, // Orientation SHORT
            0, 0, 0, 0,                    // no next IFD
        ];
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let len = (app1.len() + 2) as u16;
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn rotated_phone_jpeg_is_upright() {
        let jpeg = encode(&asym(), ImageFormat::Jpeg);
        // Orientation 6: stored image must be rotated 90° clockwise to display.
        let img = roundtrip("rot6.jpg", &with_orientation(&jpeg, 6));
        assert_eq!(img.dimensions(), (2, 4));
        assert!(is_red(img.get_pixel(1, 0)), "red moves to top-right");
        assert!(!is_red(img.get_pixel(0, 0)));
        // Orientation 8: rotate 90° counter-clockwise.
        let img = roundtrip("rot8.jpg", &with_orientation(&jpeg, 8));
        assert_eq!(img.dimensions(), (2, 4));
        assert!(is_red(img.get_pixel(0, 3)), "red moves to bottom-left");
    }
}
