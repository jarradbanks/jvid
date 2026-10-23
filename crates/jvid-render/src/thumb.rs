//! Effect and text thumbnails: a 160×90 copy of a composited frame, and that frame with one
//! effect or text preset drawn over it, each an [`crate::composite::FORMAT`] texture egui can
//! show as a native texture.

use jvid_core::model::{Effect, Text, Transform};

use crate::composite::{Compositor, FORMAT};

/// Thumbnail size in pixels.
pub const SIZE: (u32, u32) = (160, 90);

/// The frame shown with nothing on the timeline: the owner's own footage, see the assets README.
const FALLBACK: &[u8] = include_bytes!("../../../docs/prototypes/v3/sample.jpg");

const BLACK: [u8; 4] = [0, 0, 0, 255];

/// The thumbnail frame and what draws on it.
pub struct Thumbs {
    compositor: Compositor,
    /// Always the same texture, so an egui texture registered for it once stays current.
    frame: wgpu::Texture,
    /// Bumped each time the frame changes, so thumbnails drawn on it know to redraw.
    pub generation: u64,
}

impl Thumbs {
    /// Starts on the fallback frame.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let mut thumbs = Self {
            compositor: Compositor::new(device),
            frame: texture(device),
            generation: 0,
        };
        thumbs.set_fallback(device, queue);
        thumbs
    }

    /// The thumbnail frame.
    pub fn frame(&self) -> &wgpu::Texture {
        &self.frame
    }

    /// Copies `src` (a composited frame of any size) into the frame, fitted whole over black.
    pub fn set_frame(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, src: &wgpu::Texture) {
        // One bilinear tap skips pixels past 2x, so halve first: each halving averages 2x2.
        let mut src = src.clone();
        while src.width() > 2 * SIZE.0 {
            let size = (src.width(), src.height());
            let layer = [(&src, Transform::default())];
            src = self
                .compositor
                .render(device, queue, size, 2, BLACK, &layer)
                .clone();
        }
        let layer = [(&src, Transform::default())];
        draw(&mut self.compositor, device, queue, &layer, &self.frame);
        self.generation += 1;
    }

    /// Puts the bundled fallback frame back.
    pub fn set_fallback(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let img = image::load_from_memory_with_format(FALLBACK, image::ImageFormat::Jpeg)
            .expect("the bundled frame decodes")
            .into_rgba8();
        let tex = crate::text::texture(device, queue, img.dimensions(), &img);
        self.set_frame(device, queue, &tex);
    }

    /// Builds `shader` for [`Thumbs::effect`] under `key`, or removes it; see
    /// [`Compositor::set_effect`]. Kept until removed, so redraws don't rebuild it.
    pub fn set_effect(
        &mut self,
        device: &wgpu::Device,
        key: &str,
        shader: Option<&crate::effect::Shader>,
    ) -> Result<(), String> {
        self.compositor.set_effect(device, key, shader)
    }

    /// The frame with `effect` run over it, as a new texture; its `shader` is a key given to
    /// [`Thumbs::set_effect`], and parameters it leaves out take their defaults.
    pub fn effect(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        effect: &Effect,
    ) -> wgpu::Texture {
        let t = Transform {
            effects: vec![effect.clone()],
            ..Transform::default()
        };
        let out = texture(device);
        draw(
            &mut self.compositor,
            device,
            queue,
            &[(&self.frame, t)],
            &out,
        );
        out
    }

    /// The frame with `text` drawn over it at its true size and place on a `project`-sized
    /// frame, as a new texture.
    pub fn text(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        text: &Text,
        project: (u32, u32),
    ) -> wgpu::Texture {
        // The project fitted into the thumbnail, as the frame was; text pixels scale with it.
        let k =
            (SIZE.0 as f32 / project.0.max(1) as f32).min(SIZE.1 as f32 / project.1.max(1) as f32);
        let fit = |v: u32| ((v as f32 * k).round() as u32).max(1);
        let size = (fit(project.0), fit(project.1));
        let small = Text {
            size: text.size * k,
            outline: text.outline * k,
            shadow: text.shadow.map(|v| v * k),
            box_padding: text.box_padding * k,
            ..text.clone()
        };
        let rgba = crate::text::rasterize(&small, None, size);
        let tex = crate::text::texture(device, queue, size, &rgba);
        let out = texture(device);
        let layers = [
            (&tex, Transform::default()),
            (&self.frame, Transform::default()),
        ];
        draw(&mut self.compositor, device, queue, &layers, &out);
        out
    }
}

/// A thumbnail-sized texture to draw into, show and read back.
fn texture(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("thumbnail"),
        size: wgpu::Extent3d {
            width: SIZE.0,
            height: SIZE.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// `layers` (top first) composited over black at thumbnail size, copied into `into`.
fn draw(
    compositor: &mut Compositor,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layers: &[(&wgpu::Texture, Transform)],
    into: &wgpu::Texture,
) {
    let out = compositor.render(device, queue, SIZE, 1, BLACK, layers);
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_texture(out.as_image_copy(), into.as_image_copy(), out.size());
    queue.submit([encoder.finish()]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composite::read_back;
    use crate::composite::tests::device;
    use crate::effect::{BUNDLED, parse};

    /// Mean RGB of RGBA8 pixels.
    fn mean(rgba: &[u8]) -> [f64; 3] {
        let n = (rgba.len() / 4) as f64;
        std::array::from_fn(|c| rgba.chunks(4).map(|p| p[c] as f64).sum::<f64>() / n)
    }

    #[test]
    fn starts_on_the_fallback_frame() {
        let Some((device, queue)) = device() else {
            eprintln!("skipping thumbnail test: no software adapter on this platform");
            return;
        };
        let thumbs = Thumbs::new(&device, &queue);
        let got = read_back(&device, &queue, thumbs.frame());
        let want = image::load_from_memory(FALLBACK).unwrap().into_rgba8();
        assert_eq!(
            want.dimensions(),
            (480, 270),
            "16:9, so it fills the thumbnail"
        );
        let (got, want) = (mean(&got), mean(want.as_raw()));
        for c in 0..3 {
            assert!((got[c] - want[c]).abs() < 3.0, "{got:?} vs {want:?}");
        }
    }

    #[test]
    fn a_frame_is_fitted_whole() {
        let Some((device, queue)) = device() else {
            eprintln!("skipping thumbnail test: no software adapter on this platform");
            return;
        };
        let mut thumbs = Thumbs::new(&device, &queue);
        // 4:3 at a size that halves twice: pillarboxed in black, the color unchanged.
        let src = crate::text::texture(
            &device,
            &queue,
            (1440, 1080),
            &[200, 40, 90, 255].repeat(1440 * 1080),
        );
        thumbs.set_frame(&device, &queue, &src);
        let rgba = read_back(&device, &queue, thumbs.frame());
        let at = |x: u32, y: u32| &rgba[((y * SIZE.0 + x) * 4) as usize..][..4];
        assert_eq!(at(80, 45), [200, 40, 90, 255]);
        assert_eq!(at(2, 45), BLACK);
        assert_eq!(at(157, 45), BLACK);
    }

    #[test]
    fn a_bundled_effect_changes_the_frame() {
        let Some((device, queue)) = device() else {
            eprintln!("skipping thumbnail test: no software adapter on this platform");
            return;
        };
        let mut thumbs = Thumbs::new(&device, &queue);
        let frame = read_back(&device, &queue, thumbs.frame());
        let (_, src) = BUNDLED.iter().find(|(f, _)| *f == "vignette.wgsl").unwrap();
        thumbs
            .set_effect(&device, "vignette", Some(&parse(src).unwrap()))
            .unwrap();
        let effect = Effect {
            shader: "vignette".into(),
            params: Default::default(),
        };
        let out = read_back(&device, &queue, &thumbs.effect(&device, &queue, &effect));
        let changed = frame
            .chunks(4)
            .zip(out.chunks(4))
            .filter(|(a, b)| a != b)
            .count();
        assert!(changed > frame.len() / 8, "only {changed} pixels changed");
        // Corners darken most; the frame itself is untouched.
        assert!(mean(&out[..4])[0] < mean(&frame[..4])[0]);
        assert_eq!(read_back(&device, &queue, thumbs.frame()), frame);
    }

    #[test]
    fn a_text_preset_draws_where_it_lands() {
        let Some((device, queue)) = device() else {
            eprintln!("skipping thumbnail test: no software adapter on this platform");
            return;
        };
        if crate::text::families().is_empty() {
            eprintln!("skipping text thumbnail test: no fonts installed");
            return;
        }
        let mut thumbs = Thumbs::new(&device, &queue);
        let frame = read_back(&device, &queue, thumbs.frame());
        // The lower third's box sits low on the frame, whichever font draws it.
        let (_, lower) = &Text::presets()[1];
        let out = read_back(
            &device,
            &queue,
            &thumbs.text(&device, &queue, lower, (1920, 1080)),
        );
        let rows: Vec<usize> = (0..SIZE.1 as usize)
            .filter(|y| {
                let row = |p: &[u8]| p[y * SIZE.0 as usize * 4..][..SIZE.0 as usize * 4].to_vec();
                row(&frame) != row(&out)
            })
            .collect();
        assert!(!rows.is_empty(), "the box draws");
        assert!(
            rows.iter().all(|y| *y > SIZE.1 as usize / 2),
            "below the middle: {rows:?}"
        );
    }
}
