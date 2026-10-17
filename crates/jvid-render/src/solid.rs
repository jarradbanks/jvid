//! Solid color layers: a uniform image with the project's aspect, so the compositor's fit
//! fills the frame and transforms, crop and opacity apply as to any clip.

use wgpu::util::DeviceExt;

/// Straight RGBA in 0..=1 to 8-bit codes, rounded to nearest.
pub fn rgba8(c: [f64; 4]) -> [u8; 4] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// The smallest image with the aspect of `size`, e.g. 16x9 for 1920x1080.
fn aspect(size: (u32, u32)) -> (u32, u32) {
    let (mut a, mut b) = size;
    while b != 0 {
        (a, b) = (b, a % b);
    }
    (size.0 / a.max(1), size.1 / a.max(1))
}

/// A texture of `color` for a `size` project, in [`crate::composite::FORMAT`].
pub fn solid(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: (u32, u32),
    color: [u8; 4],
) -> wgpu::Texture {
    let (w, h) = aspect(size);
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("solid"),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::composite::FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        Default::default(),
        &color.repeat((w.max(1) * h.max(1)) as usize),
    )
}

#[cfg(test)]
mod tests {
    use jvid_core::model::Transform;

    use super::*;
    use crate::composite::tests::device;
    use crate::composite::{Compositor, export_png, read_back};

    #[test]
    fn aspect_and_codes() {
        assert_eq!(aspect((1920, 1080)), (16, 9));
        assert_eq!(aspect((1921, 1080)), (1921, 1080));
        assert_eq!(rgba8([1.0, 0.5, -1.0, 2.0]), [255, 128, 0, 255]);
    }

    #[test]
    fn solid_exports_at_the_exact_color() {
        let Some((device, queue)) = device() else {
            eprintln!("skipping solid export test: no software adapter on this platform");
            return;
        };
        let size = (1920, 1080);
        let color = [12, 200, 77, 255];
        let tex = solid(&device, &queue, size, color);
        let layers = [(&tex, Transform::default())];
        let rgba = read_back(
            &device,
            &queue,
            Compositor::new(&device).render(&device, &queue, size, 1, [0, 0, 0, 255], &layers),
        );
        assert!(
            rgba.chunks(4).all(|p| p == color),
            "every pixel is the solid"
        );
        let path = std::env::temp_dir().join(format!("jvid-solid-{}.png", std::process::id()));
        export_png(&device, &queue, size, [0, 0, 0, 255], &layers, &path).unwrap();
        let png = image::open(&path).unwrap().into_rgba8();
        let _ = std::fs::remove_file(&path);
        assert!(png.pixels().all(|p| p.0 == color), "PNG export is exact");
    }
}
