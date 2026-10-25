//! Still images (PNG, JPG, WebP) decoded upright to RGBA.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

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

/// File identity used to invalidate immutable still pixels after a rewrite or relink.
pub type Version = (u64, Option<SystemTime>);

pub fn version(path: &Path) -> std::io::Result<Version> {
    let m = std::fs::metadata(path)?;
    Ok((m.len(), m.modified().ok()))
}

/// Formats enabled for upright RGBA decoding. Other media, including animated GIFs,
/// retain their timed video decoder rather than being frozen or sent to a disabled decoder.
pub fn is_still(path: &Path) -> bool {
    matches!(
        image::ImageFormat::from_path(path),
        Ok(image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP)
    )
}

#[derive(Default)]
struct Cache {
    images: HashMap<PathBuf, (Version, Arc<RgbaImage>, u64)>,
    clock: u64,
}

impl Cache {
    fn get(&mut self, path: &Path) -> Result<Arc<RgbaImage>, String> {
        let version = version(path).map_err(|e| e.to_string())?;
        self.clock += 1;
        if let Some((v, image, used)) = self.images.get_mut(path)
            && *v == version
        {
            *used = self.clock;
            return Ok(image.clone());
        }
        let image = Arc::new(decode_still(path).map_err(|e| e.to_string())?);
        self.images
            .insert(path.into(), (version, image.clone(), self.clock));
        // Keep at most 256 MiB, except that one oversized still must fit. Live callers retain
        // their Arc when its cache entry is evicted.
        let mut bytes: usize = self.images.values().map(|(_, i, _)| i.len()).sum();
        while bytes > 256 << 20 && self.images.len() > 1 {
            let oldest = self
                .images
                .iter()
                .filter(|(p, _)| p.as_path() != path)
                .min_by_key(|(_, (_, _, used))| used)
                .map(|(p, _)| p.clone())
                .unwrap();
            bytes -= self.images.remove(&oldest).unwrap().1.len();
        }
        Ok(image)
    }
}

/// Upright straight RGBA, shared by Source, Program and export. Call on a decode worker:
/// the lock ensures concurrent readers decode a file version only once.
pub fn cached(path: &Path) -> Result<Arc<RgbaImage>, String> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(path)
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
    fn classifies_only_enabled_still_formats_without_file_io() {
        for name in ["a.png", "a.PNG", "a.jpg", "a.jpeg", "a.webp"] {
            assert!(is_still(Path::new(name)), "{name}");
        }
        for name in ["a.gif", "a.bmp", "a.tiff", "a.mp4", "a.mov"] {
            assert!(!is_still(Path::new(name)), "{name}");
        }
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

    #[test]
    fn cached_stills_share_pixels_until_the_file_version_changes() {
        let dir = std::env::temp_dir().join(format!("jvid-still-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("alpha.png");
        let rgba = RgbaImage::from_fn(3, 1, |x, _| Rgba([123, 45, 67, [0, 128, 255][x as usize]]));
        rgba.save(&path).unwrap();
        let a = cached(&path).unwrap();
        assert_eq!(*a, rgba);
        assert!(std::sync::Arc::ptr_eq(&a, &cached(&path).unwrap()));
        // Different length makes the version change deterministic even on coarse clocks.
        RgbaImage::from_pixel(7, 5, Rgba([17, 23, 91, 128]))
            .save(&path)
            .unwrap();
        let b = cached(&path).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&a, &b));
        assert_eq!(b.dimensions(), (7, 5));
        assert_eq!(b.get_pixel(0, 0).0, [17, 23, 91, 128]);
        std::fs::remove_dir_all(dir).unwrap();
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
