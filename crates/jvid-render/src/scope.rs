//! Video scopes: a small copy of a composited frame, read back without waiting, and the luma
//! waveform, vectorscope and RGB histogram counted from it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use jvid_core::model::Transform;

use crate::composite::{Compositor, copy_out, unpad};

/// Width of the copy the scopes count; its height keeps the frame's aspect.
pub const WIDTH: u32 = 256;
/// Vectorscope grid size: Cb across, Cr up, each spanning ±[`RANGE`]. Coarse, so a solid
/// colour's dot shows at panel size; odd, so greys land in one centre cell, not on an edge.
pub const VECTOR: usize = 65;
/// Half the chroma span the vectorscope shows; a little past ±0.5 so 100% red and blue fit.
pub const RANGE: f32 = 0.6;

/// What the scopes show, counted from one frame.
pub struct Analysis {
    /// Waveform columns, one per pixel column of the copy.
    pub width: usize,
    /// `waveform[level * width + x]`: pixels in column `x` with BT.709 luma code `level`.
    pub waveform: Vec<u32>,
    /// `vectorscope[y * VECTOR + x]`: pixels landing there; see [`vector_at`].
    pub vectorscope: Vec<u32>,
    /// Pixels at each code value, for R, G and B.
    pub histogram: [[u32; 256]; 3],
    /// Pixels counted.
    pub pixels: u32,
}

/// BT.709 luma (Y') of 8-bit R'G'B', as a code value.
fn luma([r, g, b]: [u8; 3]) -> f32 {
    0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32
}

/// Where a colour lands on the vectorscope grid, as (column, row): BT.709 Cb across and Cr up.
pub fn vector_at(rgb: [u8; 3]) -> (usize, usize) {
    let y = luma(rgb);
    let cb = (rgb[2] as f32 - y) / 255.0 / 1.8556;
    let cr = (rgb[0] as f32 - y) / 255.0 / 1.5748;
    let at = |v: f32| (((v + RANGE) / (2.0 * RANGE)).clamp(0.0, 1.0) * (VECTOR - 1) as f32).round();
    (at(cb) as usize, VECTOR - 1 - at(cr) as usize)
}

/// Counts the scopes of `width`-wide RGBA8 pixels.
pub fn analyze(rgba: &[u8], width: usize) -> Analysis {
    let mut a = Analysis {
        width,
        waveform: vec![0; width * 256],
        vectorscope: vec![0; VECTOR * VECTOR],
        histogram: [[0; 256]; 3],
        pixels: (rgba.len() / 4) as u32,
    };
    for (i, p) in rgba.as_chunks::<4>().0.iter().enumerate() {
        let rgb = [p[0], p[1], p[2]];
        let level = luma(rgb).round() as usize;
        a.waveform[level * width + i % width] += 1;
        let (x, y) = vector_at(rgb);
        a.vectorscope[y * VECTOR + x] += 1;
        for (counts, v) in a.histogram.iter_mut().zip(rgb) {
            counts[v as usize] += 1;
        }
    }
    a
}

/// Copies composited frames down to scope size and reads them back, one at a time, never
/// waiting on the GPU.
pub struct Scopes {
    compositor: Compositor,
    /// The copy in flight: its buffer, width, and whether it has mapped.
    pending: Option<(wgpu::Buffer, u32, Arc<AtomicBool>)>,
}

impl Scopes {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            compositor: Compositor::new(device),
            pending: None,
        }
    }

    /// A copy is in flight; [`Scopes::capture`] skips until [`Scopes::poll`] returns it.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Starts a scope-sized copy of `src` (a composited frame) unless one is in flight.
    pub fn capture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, src: &wgpu::Texture) {
        if self.busy() {
            return;
        }
        let w = WIDTH.min(src.width());
        let size = (w, (w * src.height() / src.width()).max(1));
        let src = crate::thumb::halved(&mut self.compositor, device, queue, src, w);
        let layer = [(&src, Transform::default())];
        let out = self
            .compositor
            .render(device, queue, size, 1, [0, 0, 0, 255], &layer);
        let mapped = Arc::new(AtomicBool::new(false));
        let flag = mapped.clone();
        let (buf, _) = copy_out(device, queue, out, None, move || {
            flag.store(true, Ordering::Release)
        });
        self.pending = Some((buf, w, mapped));
    }

    /// The scopes of the copy in flight once it has landed; `None` before then, never waiting.
    pub fn poll(&mut self, device: &wgpu::Device) -> Option<Analysis> {
        let (_, _, mapped) = self.pending.as_ref()?;
        // Runs the map callback if the copy is done.
        let _ = device.poll(wgpu::PollType::Poll);
        if !mapped.load(Ordering::Acquire) {
            return None;
        }
        let (buf, width, _) = self.pending.take()?;
        let rgba = unpad(&buf, width);
        Some(analyze(&rgba, width as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 4] = [255, 0, 0, 255];

    /// The one lit cell of a count grid, as (index, count); panics on more or none.
    fn only(counts: &[u32]) -> (usize, u32) {
        let lit: Vec<_> = counts
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(i, n)| (i, *n))
            .collect();
        assert_eq!(lit.len(), 1, "{lit:?}");
        lit[0]
    }

    #[test]
    fn solid_red_lands_on_the_red_target_and_code_values() {
        let (w, h) = (16, 9);
        let a = analyze(&RED.repeat(w * h), w);
        let n = (w * h) as u32;
        // BT.709: Cb = -0.2126 / 1.8556 and Cr = 0.7874 / 1.5748 = 0.5, on a ±0.6 grid of 65.
        let x = ((-0.2126 / 1.8556 + 0.6) / 1.2 * 64.0_f64).round() as usize;
        let y = 64 - ((0.5 + 0.6) / 1.2 * 64.0_f64).round() as usize;
        assert_eq!((x, y), (26, 5));
        assert_eq!(vector_at([255, 0, 0]), (x, y), "the red target");
        assert_eq!(only(&a.vectorscope), (y * VECTOR + x, n));
        // Upper left: red's hue, well clear of the centre where greys land.
        assert_eq!(vector_at([128, 128, 128]), (32, 32));
        assert_eq!(only(&a.histogram[0]), (255, n));
        assert_eq!(only(&a.histogram[1]), (0, n));
        assert_eq!(only(&a.histogram[2]), (0, n));
        // Every column at red's luma, 0.2126 × 255.
        for x in 0..w {
            let column: Vec<u32> = (0..256).map(|l| a.waveform[l * w + x]).collect();
            assert_eq!(only(&column), (54, h as u32));
        }
    }

    #[test]
    fn a_black_to_white_ramp_draws_a_diagonal_waveform() {
        let (w, h) = (256, 4);
        let ramp: Vec<u8> = (0..h)
            .flat_map(|_| (0..w).flat_map(|x| [x as u8, x as u8, x as u8, 255]))
            .collect();
        let a = analyze(&ramp, w);
        for x in 0..w {
            let column: Vec<u32> = (0..256).map(|l| a.waveform[l * w + x]).collect();
            assert_eq!(only(&column), (x, h as u32), "column {x}");
        }
        // Greys have no chroma: all on the centre of the vectorscope.
        assert_eq!(only(&a.vectorscope).1, a.pixels);
    }

    #[test]
    fn a_copy_reads_back_without_waiting() {
        let Some((device, queue)) = crate::composite::tests::device() else {
            eprintln!("skipping scope test: no software adapter on this platform");
            return;
        };
        let mut scopes = Scopes::new(&device);
        // 1080p halves twice, then draws down to 256 × 144.
        let src = crate::text::texture(&device, &queue, (1920, 1080), &RED.repeat(1920 * 1080));
        scopes.capture(&device, &queue, &src);
        assert!(scopes.busy());
        // A second capture while one is in flight does nothing.
        scopes.capture(&device, &queue, &src);
        let a = loop {
            if let Some(a) = scopes.poll(&device) {
                break a;
            }
            std::thread::yield_now();
        };
        assert!(!scopes.busy());
        assert_eq!((a.width, a.pixels), (256, 256 * 144));
        assert_eq!(only(&a.histogram[0]), (255, a.pixels));
        assert_eq!(only(&a.vectorscope).0, 5 * VECTOR + 26);
    }
}
