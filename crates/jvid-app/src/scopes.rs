//! The Media panel's Scopes tab: luma waveform, vectorscope and RGB histogram of the Program
//! frame, counted by jvid-render from a small copy taken as the frame draws. Parked, every
//! new frame updates them; playing or scrubbing, at most every [`MOVING_INTERVAL`].

use egui::{
    Align2, Color32, ColorImage, Pos2, Rect, TextureHandle, TextureOptions, Ui, pos2, vec2,
};
use jvid_render::scope::{Analysis, RANGE, VECTOR, vector_at};
use suite_ui::theme;
use suite_ui::tokens::{color, radius, space};

use crate::i18n::tr;

/// Seconds between scope updates while playing or scrubbing.
pub const MOVING_INTERVAL: f64 = 0.25;
/// Histogram height in texels.
const HISTOGRAM_H: usize = 128;
/// 100% primaries and secondaries, the vectorscope's targets.
const TARGETS: [[u8; 3]; 6] = [
    [255, 0, 0],
    [255, 255, 0],
    [0, 255, 0],
    [0, 255, 255],
    [0, 0, 255],
    [255, 0, 255],
];

/// The scopes' textures live in egui memory under this, so the bin needn't hold them.
pub(crate) fn id() -> egui::Id {
    egui::Id::new("scopes")
}

/// When the Program frame is copied for the scopes.
#[derive(Default)]
pub struct State {
    /// The Scopes tab is showing; nothing is copied or counted otherwise.
    pub open: bool,
    /// The program generation and time of the last copy.
    last: Option<(u64, f64)>,
    /// Copy the Program frame as it's next drawn.
    pub copy: bool,
}

impl State {
    /// Whether to copy Program frame `generation`, drawn at `now` seconds: each new frame when
    /// parked, at most every [`MOVING_INTERVAL`] while `moving`, and never while `busy` with
    /// the last copy. Sets [`State::copy`] when it is.
    pub fn due(&mut self, generation: u64, now: f64, moving: bool, busy: bool) -> bool {
        let due = self.open
            && !busy
            && self
                .last
                .is_none_or(|(g, t)| g != generation && (!moving || now - t >= MOVING_INTERVAL));
        if due {
            self.last = Some((generation, now));
            self.copy = true;
        }
        due
    }
}

/// Shows `a` in the Scopes tab from now on.
pub fn set(ctx: &egui::Context, a: &Analysis) {
    let images = images(a);
    let opts = TextureOptions::LINEAR;
    match ctx.data(|d| d.get_temp::<[TextureHandle; 3]>(id())) {
        Some(mut textures) => {
            for (t, i) in textures.iter_mut().zip(images) {
                t.set(i, opts);
            }
        }
        None => {
            let [w, v, h] = images;
            let textures = [
                ctx.load_texture("waveform", w, opts),
                ctx.load_texture("vectorscope", v, opts),
                ctx.load_texture("histogram", h, opts),
            ];
            ctx.data_mut(|d| d.insert_temp(id(), textures));
        }
    }
}

/// Nothing to measure: the tab shows its empty state.
pub fn clear(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<[TextureHandle; 3]>(id()));
}

/// `count` of `full` as a trace brightness: square root, so sparse values still show.
fn trace(count: u32, full: f32) -> Color32 {
    color::ink().gamma_multiply((count as f32 / full.max(1.0)).sqrt().min(1.0))
}

/// The waveform (luma up, 255 at the top), vectorscope and histogram as images.
fn images(a: &Analysis) -> [ColorImage; 3] {
    let w = a.width.max(1);
    let column = a.pixels as f32 / w as f32;
    let wave = (0..256)
        .rev()
        .flat_map(|level| (0..w).map(move |x| (level, x)))
        .map(|(level, x)| trace(a.waveform[level * w + x], column / 16.0))
        .collect();
    let vector = a
        .vectorscope
        .iter()
        .map(|n| trace(*n, a.pixels as f32 / 256.0))
        .collect();
    let max = a
        .histogram
        .iter()
        .flatten()
        .copied()
        .max()
        .unwrap_or(0)
        .max(1);
    let heights = a
        .histogram
        .map(|c| c.map(|n| (n as f32 / max as f32).sqrt() * HISTOGRAM_H as f32));
    let hist = (0..HISTOGRAM_H)
        .flat_map(|row| (0..256).map(move |x| (row, x)))
        .map(|(row, x)| {
            let lit = heights.map(|h| h[x] > (HISTOGRAM_H - 1 - row) as f32);
            let [r, g, b] = lit.map(|on| if on { 200 } else { 0 });
            if lit.contains(&true) {
                Color32::from_rgb(r, g, b)
            } else {
                Color32::TRANSPARENT
            }
        })
        .collect();
    [
        ColorImage::new([w, 256], wave),
        ColorImage::new([VECTOR, VECTOR], vector),
        ColorImage::new([256, HISTOGRAM_H], hist),
    ]
}

/// The tab: the waveform, vectorscope and histogram stacked, each on Viewer black.
pub fn show(ui: &mut Ui) {
    let rect = ui.max_rect();
    let p = ui.painter().clone();
    let Some([wave, vector, hist]) = ui.data(|d| d.get_temp::<[TextureHandle; 3]>(id())) else {
        let empty = tr!("bin.scopes_empty");
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            empty,
            theme::body(),
            color::ink_3(),
        );
        return;
    };
    let gap = space::S8;
    let side = rect.width().min((rect.height() - 2.0 * gap) * 0.4).max(0.0);
    let h = ((rect.height() - 2.0 * gap - side) / 2.0).max(0.0);
    let wave_r = Rect::from_min_size(rect.min, vec2(rect.width(), h));
    let vector_r = Rect::from_min_size(
        pos2(rect.center().x - side / 2.0, wave_r.bottom() + gap),
        vec2(side, side),
    );
    let hist_r = Rect::from_min_size(pos2(rect.left(), vector_r.bottom() + gap), wave_r.size());
    let line = theme::hairline(ui.ctx(), color::ink().gamma_multiply(0.15));
    let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
    let scopes = [
        (wave_r, &wave, tr!("bin.scope_waveform")),
        (vector_r, &vector, tr!("bin.scope_vectorscope")),
        (hist_r, &hist, tr!("bin.scope_histogram")),
    ];
    for (r, tex, label) in scopes {
        p.rect_filled(r, radius::SMALL, color::VIEWER);
        p.image(tex.id(), r, uv, Color32::WHITE);
        // Left-aligned with the others, so the vectorscope's sits beside its square.
        let at = pos2(rect.left(), r.top()) + vec2(space::S4, space::S4);
        p.text(at, Align2::LEFT_TOP, label, theme::small(), color::ink_3());
    }
    // Waveform graticule: 0, 25, 50, 75 and 100% luma.
    for i in 0..=4 {
        let y = wave_r.bottom() - wave_r.height() * i as f32 / 4.0;
        p.hline(wave_r.x_range(), y, line);
    }
    // Vectorscope: the ±0.5 chroma circle, axes and colour targets.
    let c = vector_r.center();
    p.circle_stroke(c, side / 2.0 * 0.5 / RANGE, line);
    p.hline(vector_r.x_range(), c.y, line);
    p.vline(c.x, vector_r.y_range(), line);
    let cell = side / VECTOR as f32;
    for rgb in TARGETS {
        let (x, y) = vector_at(rgb);
        let at = vector_r.min + vec2(x as f32 + 0.5, y as f32 + 0.5) * cell;
        let target = Rect::from_center_size(at, vec2(6.0, 6.0));
        let stroke = theme::hairline(ui.ctx(), color::ink_3());
        p.rect_stroke(target, 0.0, stroke, egui::StrokeKind::Middle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Playing at 60fps for 10 s, a new Program frame each draw and every copy landing at once:
    /// the scopes copy at most every [`MOVING_INTERVAL`], and nothing at all while closed.
    #[test]
    fn playback_copies_no_more_often_than_the_reduced_rate() {
        let mut s = State::default();
        let play = |s: &mut State| {
            (0..600u64)
                .filter(|&f| s.due(f, f as f64 / 60.0, true, false))
                .count()
        };
        assert_eq!(play(&mut s), 0, "closed: no scope work");
        s.open = true;
        let copies = play(&mut s);
        let most = (10.0 / MOVING_INTERVAL) as usize + 1;
        assert!((most - 1..=most).contains(&copies), "{copies} copies");
        // A copy still in flight holds the next one back.
        assert!(!s.due(1000, 100.0, true, true));
    }

    #[test]
    fn parked_every_new_frame_updates_once() {
        let mut s = State {
            open: true,
            ..State::default()
        };
        assert!(s.due(1, 0.0, false, false));
        assert!(std::mem::take(&mut s.copy));
        assert!(!s.due(1, 0.01, false, false), "the same frame");
        assert!(s.due(2, 0.02, false, false), "a new frame, straight away");
        // Stopping between throttled copies catches up on the frame it stopped on.
        assert!(!s.due(3, 0.03, true, false));
        assert!(s.due(3, 0.04, false, false));
    }

    #[test]
    fn solid_red_images() {
        let red = [255, 0, 0, 255].repeat(16 * 9);
        let [wave, vector, hist] = images(&jvid_render::scope::analyze(&red, 16));
        // Red's luma, 54, is row 255 - 54 from the top; the trace is full brightness there.
        assert_eq!(wave.size, [16, 256]);
        assert_eq!(wave.pixels[(255 - 54) * 16 + 3], color::ink());
        assert_eq!(wave.pixels[(255 - 200) * 16 + 3], Color32::TRANSPARENT);
        let (x, y) = vector_at([255, 0, 0]);
        assert_eq!(vector.pixels[y * VECTOR + x], color::ink());
        // Red's column full height in red, black's G and B column in green and blue.
        let top = |x: usize| hist.pixels[x];
        assert_eq!(top(255), Color32::from_rgb(200, 0, 0));
        assert_eq!(top(0), Color32::from_rgb(0, 200, 200));
        assert_eq!(top(128), Color32::TRANSPARENT);
    }
}
