//! Text/title layers: shaped and rasterized with `cosmic-text` into a straight RGBA image the
//! size of the frame, so the compositor's fit fills the frame and transforms, crop and
//! opacity apply as to any clip.

use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};
use jvid_core::model::{Align, Text};
use wgpu::util::DeviceExt;

/// A style and frame size.
type Key = (Text, (u32, u32));

/// Glyph coverage and the outline-grown shape, one byte per pixel, and the shape's bounds.
struct Masks {
    fill: Vec<u8>,
    shape: Vec<u8>,
    bounds: Option<[u32; 4]>,
}

struct Fonts {
    system: FontSystem,
    swash: SwashCache,
    /// Recent rasters, newest last.
    // shortcut: a linear scan over 8 entries; key by hash if many text layers show at once.
    recent: Vec<(Key, Arc<Vec<u8>>)>,
    /// Recent masks keyed by colorless styles, so animated colors only re-tint.
    masks: Vec<(Key, Arc<Masks>)>,
}

fn fonts() -> &'static Mutex<Fonts> {
    static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();
    FONTS.get_or_init(|| {
        Mutex::new(Fonts {
            system: FontSystem::new(),
            swash: SwashCache::new(),
            recent: Vec::new(),
            masks: Vec::new(),
        })
    })
}

/// Loads the system fonts, which takes seconds on some systems; call it off the UI thread
/// before the first text frame.
pub fn warm() {
    drop(fonts().lock());
}

/// The font families the renderer can draw, sorted and deduplicated; loads the fonts on first
/// call, like [`warm`].
pub fn families() -> &'static [String] {
    static LIST: OnceLock<Vec<String>> = OnceLock::new();
    LIST.get_or_init(|| {
        let f = fonts().lock().unwrap_or_else(|e| e.into_inner());
        let mut v: Vec<String> = f
            .system
            .db()
            .faces()
            .filter_map(|face| face.families.first().map(|(n, _)| n.clone()))
            .collect();
        v.sort_by_key(|n| n.to_lowercase());
        v.dedup();
        v
    })
}

/// Pushes `v` under `k`, keeping the newest 8.
fn remember<T>(list: &mut Vec<(Key, T)>, k: Key, v: T) {
    if list.len() >= 8 {
        list.remove(0);
    }
    list.push((k, v));
}

/// `text` shaped for a `size` frame, the wrap width, and the offset from buffer to frame pixels.
fn layout(f: &mut Fonts, text: &Text, size: (u32, u32)) -> (Buffer, f32, [i32; 2]) {
    let (w, h) = size;
    let metrics = Metrics::new(text.size.max(1.0), text.size.max(1.0) * 1.2);
    let mut buf = Buffer::new(&mut f.system, metrics);
    let attrs = Attrs::new()
        .weight(Weight(text.weight))
        .family(if text.family.is_empty() {
            Family::SansSerif
        } else {
            Family::Name(&text.family)
        });
    let align = match text.align {
        Align::Left => cosmic_text::Align::Left,
        Align::Center => cosmic_text::Align::Center,
        Align::Right => cosmic_text::Align::Right,
    };
    // Wrap so a block centered on the anchor stays in frame; lines align within the widest.
    let ax = text.anchor[0].clamp(0.0, 1.0);
    let wrap = (2.0 * ax.min(1.0 - ax) * w as f32).max(text.size);
    buf.set_size(&mut f.system, Some(wrap), None);
    buf.set_text(
        &mut f.system,
        &text.content,
        &attrs,
        Shaping::Advanced,
        Some(align),
    );
    buf.shape_until_scroll(&mut f.system, false);
    let runs: Vec<_> = buf
        .layout_runs()
        .map(|r| (r.line_w, r.line_top + r.line_height))
        .collect();
    let block_w = runs.iter().map(|r| r.0).fold(0.0, f32::max);
    let block_h = runs.last().map_or(0.0, |r| r.1);
    // Alignment places lines across the full wrap width; shift the widest to the anchor.
    let left = match text.align {
        Align::Left => 0.0,
        Align::Center => (wrap - block_w) / 2.0,
        Align::Right => wrap - block_w,
    };
    let dx = (text.anchor[0] * w as f32 - block_w / 2.0 - left).round() as i32;
    let dy = (text.anchor[1] * h as f32 - block_h / 2.0).round() as i32;
    (buf, wrap, [dx, dy])
}

/// Glyph coverage of `text` laid out on a `size` frame: one byte per pixel.
fn coverage(f: &mut Fonts, text: &Text, size: (u32, u32)) -> Vec<u8> {
    let (w, h) = size;
    let (buf, _, [dx, dy]) = layout(f, text, size);
    let mut out = vec![0u8; (w * h) as usize];
    let Fonts { system, swash, .. } = f;
    buf.draw(
        system,
        swash,
        Color::rgb(255, 255, 255),
        |x, y, gw, gh, c| {
            for (yy, xx) in (0..gh as i32).flat_map(|j| (0..gw as i32).map(move |i| (j, i))) {
                let (px, py) = (x + xx + dx, y + yy + dy);
                if (0..w as i32).contains(&px) && (0..h as i32).contains(&py) {
                    let o = &mut out[(py as u32 * w + px as u32) as usize];
                    *o = (*o).max(c.a());
                }
            }
        },
    );
    out
}

/// `mask` grown by `r` pixels: the max over a disc.
fn dilate(mask: &[u8], (w, h): (u32, u32), r: f32) -> Vec<u8> {
    let ri = r.ceil() as i32;
    let disc: Vec<_> = (-ri..=ri)
        .flat_map(|j| (-ri..=ri).map(move |i| (i, j)))
        .filter(|(i, j)| ((i * i + j * j) as f32) <= r * r)
        .collect();
    let mut out = mask.to_vec();
    for (k, &m) in mask.iter().enumerate().filter(|(_, m)| **m > 0) {
        let (x, y) = ((k as u32 % w) as i32, (k as u32 / w) as i32);
        for (i, j) in &disc {
            let (px, py) = (x + i, y + j);
            if (0..w as i32).contains(&px) && (0..h as i32).contains(&py) {
                let o = &mut out[(py * w as i32 + px) as usize];
                *o = (*o).max(m);
            }
        }
    }
    out
}

/// Bounding box `[x0, y0, x1, y1)` of nonzero coverage.
fn bounds(mask: &[u8], w: u32) -> Option<[u32; 4]> {
    let mut b: Option<[u32; 4]> = None;
    for (k, _) in mask.iter().enumerate().filter(|(_, m)| **m > 0) {
        let (x, y) = (k as u32 % w, k as u32 / w);
        let e = b.get_or_insert([x, y, x + 1, y + 1]);
        *e = [e[0].min(x), e[1].min(y), e[2].max(x + 1), e[3].max(y + 1)];
    }
    b
}

/// The masks of `text` on a `size` frame, cached by its colorless style.
fn masks(f: &mut Fonts, text: &Text, size: (u32, u32)) -> Arc<Masks> {
    let key = (
        Text {
            color: [0.0; 4],
            outline_color: [0.0; 4],
            shadow_color: [0.0; 4],
            box_color: [0.0; 4],
            ..text.clone()
        },
        size,
    );
    if let Some((_, m)) = f.masks.iter().find(|(k, _)| *k == key) {
        return m.clone();
    }
    let fill = coverage(f, text, size);
    let shape = if text.outline > 0.0 {
        dilate(&fill, size, text.outline)
    } else {
        fill.clone()
    };
    let bounds = bounds(&shape, size.0);
    let m = Arc::new(Masks {
        fill,
        shape,
        bounds,
    });
    remember(&mut f.masks, key, m.clone());
    m
}

/// `text` on a transparent `size` frame, straight RGBA8. `fill` overrides `text.color`, so a
/// clip's `color` keyframes animate it.
pub fn rasterize(text: &Text, fill: Option<[f64; 4]>, size: (u32, u32)) -> Arc<Vec<u8>> {
    let text = Text {
        color: fill.unwrap_or(text.color),
        ..text.clone()
    };
    let key = (text, size);
    let mut f = fonts().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, img)) = f.recent.iter().find(|(k, _)| *k == key) {
        return img.clone();
    }
    let (text, (w, h)) = &key;
    let (w, h) = (*w as i64, *h as i64);
    let m = masks(&mut f, text, size);
    let mut img = vec![0u8; (w * h * 4) as usize];
    let Some([x0, y0, x1, y1]) = m.bounds.map(|b| b.map(i64::from)) else {
        let img = Arc::new(img);
        remember(&mut f.recent, key, img.clone());
        return img;
    };
    let [sx, sy] = text.shadow.map(|v| v.round() as i64);
    let p = padding(text);
    let (rx, ry) = reach(text, [x0, y0, x1, y1], (w, h));
    let at = |m: &[u8], x: i64, y: i64| {
        if (0..w).contains(&x) && (0..h).contains(&y) {
            m[(y * w + x) as usize] as f32 / 255.0
        } else {
            0.0
        }
    };
    for y in ry {
        for x in rx.clone() {
            // Premultiplied layers, bottom first: box, shadow, outline, fill.
            let mut px = [0f32; 4];
            let mut over = |a: f32, c: [f64; 4]| {
                let a = a * c[3] as f32;
                if a > 0.0 {
                    for i in 0..3 {
                        px[i] = c[i] as f32 * a + px[i] * (1.0 - a);
                    }
                    px[3] = a + px[3] * (1.0 - a);
                }
            };
            let inside = (x0 - p..x1 + p).contains(&x) && (y0 - p..y1 + p).contains(&y);
            over(inside as u8 as f32, text.box_color);
            if text.shadow != [0.0, 0.0] {
                over(at(&m.shape, x - sx, y - sy), text.shadow_color);
            }
            if text.outline > 0.0 {
                over(at(&m.shape, x, y), text.outline_color);
            }
            over(at(&m.fill, x, y), text.color);
            let a = px[3];
            let un = |v: f32| if a > 0.0 { v / a } else { 0.0 };
            let rgba = [un(px[0]), un(px[1]), un(px[2]), a];
            let o = ((y * w + x) * 4) as usize;
            for (d, v) in img[o..o + 4].iter_mut().zip(rgba) {
                *d = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    let img = Arc::new(img);
    remember(&mut f.recent, key, img.clone());
    img
}

/// The box padding drawn around the glyphs, 0 without a box.
fn padding(text: &Text) -> i64 {
    if text.box_color[3] > 0.0 {
        text.box_padding.max(0.0) as i64
    } else {
        0
    }
}

/// The pixels the box, shadow and glyphs within `[x0, y0, x1, y1)` can reach on a `w`x`h` frame.
fn reach(text: &Text, [x0, y0, x1, y1]: [i64; 4], (w, h): (i64, i64)) -> (Range<i64>, Range<i64>) {
    let [sx, sy] = text.shadow.map(|v| v.round() as i64);
    let p = padding(text);
    let rx = (x0.min(x0 + sx) - p).max(0)..(x1.max(x1 + sx) + p).min(w);
    let ry = (y0.min(y0 + sy) - p).max(0)..(y1.max(y1 + sy) + p).min(h);
    (rx, ry)
}

/// The pixel rect `[x0, y0, x1, y1)` [`rasterize`] draws `text` into on a `size` frame;
/// `None` when nothing shows.
pub fn drawn_bounds(text: &Text, size: (u32, u32)) -> Option<[u32; 4]> {
    let mut f = fonts().lock().unwrap_or_else(|e| e.into_inner());
    let b = masks(&mut f, text, size).bounds?.map(i64::from);
    let (rx, ry) = reach(text, b, (size.0 as i64, size.1 as i64));
    (!rx.is_empty() && !ry.is_empty())
        .then(|| [rx.start, ry.start, rx.end, ry.end].map(|v| v as u32))
}

/// Byte offsets where each line (paragraph) of `s` starts.
fn line_starts(s: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(s.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// Where a caret before each char of `text.content`, and after the last, sits on a `size`
/// frame: `[x, top, bottom]` in frame pixels, one more than there are chars.
// shortcut: left-to-right only; a caret in right-to-left text sits mirrored.
pub fn carets(text: &Text, size: (u32, u32)) -> Vec<[f32; 3]> {
    let s = &text.content;
    let starts = line_starts(s);
    let mut f = fonts().lock().unwrap_or_else(|e| e.into_inner());
    let (buf, wrap, [dx, dy]) = layout(&mut f, text, size);
    let empty = match text.align {
        Align::Left => 0.0,
        Align::Center => wrap / 2.0,
        Align::Right => wrap,
    };
    let runs: Vec<_> = buf.layout_runs().collect();
    let bytes = s.char_indices().map(|(i, _)| i).chain([s.len()]);
    bytes
        .map(|byte| {
            let line = starts.partition_point(|&b| b <= byte) - 1;
            let index = byte - starts[line];
            let mut out = [empty, 0.0, text.size];
            for run in runs.iter().filter(|r| r.line_i == line) {
                let end = run.glyphs.last().map_or(empty, |g| g.x + g.w);
                out = [end, run.line_top, run.line_top + run.line_height];
                // The first glyph ending past the caret starts at it; none means a later run.
                if let Some(g) = run.glyphs.iter().find(|g| g.end > index) {
                    out[0] = g.x;
                    break;
                }
            }
            [out[0] + dx as f32, out[1] + dy as f32, out[2] + dy as f32]
        })
        .collect()
}

/// The char of `text.content` nearest frame pixel `[x, y]` on a `size` frame, as a caret index.
pub fn hit(text: &Text, size: (u32, u32), [x, y]: [f32; 2]) -> usize {
    let mut f = fonts().lock().unwrap_or_else(|e| e.into_inner());
    let (buf, _, [dx, dy]) = layout(&mut f, text, size);
    let Some(c) = buf.hit(x - dx as f32, y - dy as f32) else {
        return 0;
    };
    let s = &text.content;
    let byte = line_starts(s).get(c.line).map_or(s.len(), |b| b + c.index);
    s[..byte.min(s.len())].chars().count()
}

/// A texture of [`rasterize`]'s image, in [`crate::composite::FORMAT`].
pub fn texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: (u32, u32),
    rgba: &[u8],
) -> wgpu::Texture {
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("text"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
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
        rgba,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shapes `s` with the system fonts; `None` when no installed font covers it.
    fn shape(s: &str) -> Option<(bool, Vec<u16>)> {
        let mut f = fonts().lock().unwrap();
        let mut buf = Buffer::new(&mut f.system, Metrics::new(32.0, 40.0));
        buf.set_text(&mut f.system, s, &Attrs::new(), Shaping::Advanced, None);
        buf.shape_until_scroll(&mut f.system, false);
        let run = buf.layout_runs().next()?;
        let ids: Vec<u16> = run.glyphs.iter().map(|g| g.glyph_id).collect();
        // Which fallback font shaped it differs per OS, so say, for failures.
        let faces: Vec<_> = run
            .glyphs
            .iter()
            .filter_map(|g| f.system.db().face(g.font_id))
            .map(|face| face.post_script_name.clone())
            .collect();
        eprintln!("{s}: {ids:?} from {faces:?}");
        // Glyph 0 is .notdef: the font fell back to a tofu box.
        (!ids.contains(&0)).then_some((run.rtl, ids))
    }

    #[test]
    fn every_listed_family_resolves_to_a_face() {
        let names = families();
        let f = fonts().lock().unwrap();
        for name in names {
            let q = cosmic_text::fontdb::Query {
                families: &[cosmic_text::fontdb::Family::Name(name)],
                ..Default::default()
            };
            assert!(f.system.db().query(&q).is_some(), "{name}");
        }
    }

    /// Skips a shaping test, except on Linux CI, which installs the fonts.
    fn no_font(script: &str) {
        assert!(
            !(cfg!(target_os = "linux") && std::env::var_os("CI").is_some()),
            "Linux CI must have a font covering {script}"
        );
        eprintln!("skipping: no installed font covers {script}");
    }

    #[test]
    fn arabic_shapes_right_to_left_with_joined_forms() {
        let Some((rtl, ids)) = shape("سلام") else {
            no_font("Arabic");
            return;
        };
        assert!(rtl, "Arabic runs right to left");
        assert!(!ids.is_empty(), "Arabic produces shaped glyphs");
        // Joining picks contextual forms: the same letter differs alone and joined. How many
        // glyphs lam-alef makes is up to the font (DejaVu: one ligature, Segoe UI: two).
        let (_, alone) = shape("س").unwrap();
        assert!(
            !ids.contains(&alone[0]),
            "initial seen is not the isolated form"
        );
    }

    #[test]
    fn cjk_shapes_one_glyph_per_character() {
        let Some((rtl, ids)) = shape("你好世界") else {
            no_font("CJK");
            return;
        };
        assert!(!rtl);
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn raster_draws_fill_box_and_anchor() {
        let size = (320, 180);
        let text = Text {
            content: "Hi".into(),
            size: 40.0,
            box_color: [0.0, 0.0, 1.0, 1.0],
            box_padding: 4.0,
            ..Text::default()
        };
        let img = rasterize(&text, Some([1.0, 0.0, 0.0, 1.0]), size);
        let px = |x: u32, y: u32| &img[((y * size.0 + x) * 4) as usize..][..4];
        assert_eq!(px(0, 0), [0, 0, 0, 0], "outside the box is transparent");
        let red = (0..img.len() / 4).filter(|k| img[k * 4..][..4] == [255, 0, 0, 255]);
        let blue = (0..img.len() / 4).filter(|k| img[k * 4..][..4] == [0, 0, 255, 255]);
        let (red, blue): (Vec<_>, Vec<_>) = (red.collect(), blue.collect());
        if red.is_empty() {
            no_font("Latin");
            return;
        }
        assert!(!blue.is_empty(), "the box shows around the glyphs");
        let cx = red.iter().map(|k| (k % 320) as f32).sum::<f32>() / red.len() as f32;
        assert!((cx - 160.0).abs() < 20.0, "centered on the anchor: {cx}");
        assert!(Arc::ptr_eq(
            &img,
            &rasterize(&text, Some([1.0, 0.0, 0.0, 1.0]), size)
        ));
    }

    #[test]
    fn bounds_cover_exactly_the_drawn_pixels() {
        let size = (320, 180);
        let text = Text {
            content: "Hi".into(),
            size: 40.0,
            box_color: [0.0, 0.0, 1.0, 1.0],
            box_padding: 4.0,
            ..Text::default()
        };
        let Some([x0, y0, x1, y1]) = drawn_bounds(&text, size) else {
            no_font("Latin");
            return;
        };
        assert!(
            x1 - x0 < size.0 / 2 && y1 - y0 < size.1 / 2,
            "smaller than the frame"
        );
        let img = rasterize(&text, None, size);
        for (k, px) in img.chunks(4).enumerate() {
            let (x, y) = (k as u32 % size.0, k as u32 / size.0);
            let inside = (x0..x1).contains(&x) && (y0..y1).contains(&y);
            assert_eq!(px[3] > 0, inside, "at {x},{y}");
        }
    }

    #[test]
    fn carets_step_right_and_down_and_hits_find_them() {
        let size = (640, 360);
        let text = Text {
            content: "Hi there\nab".into(),
            size: 40.0,
            ..Text::default()
        };
        if drawn_bounds(&text, size).is_none() {
            no_font("Latin");
            return;
        }
        let c = carets(&text, size);
        assert_eq!(c.len(), 12);
        let xs: Vec<f32> = c[..=8].iter().map(|c| c[0]).collect();
        assert!(xs.windows(2).all(|w| w[0] < w[1]), "{xs:?}");
        let [_, top, bottom] = c[9];
        assert!(top >= c[0][2] - 1.0 && bottom > top, "line two sits below");
        for i in [0, 3, 9, 11] {
            let [x, top, bottom] = c[i];
            assert_eq!(hit(&text, size, [x + 1.0, (top + bottom) / 2.0]), i);
        }
    }

    #[test]
    fn recoloring_reuses_the_masks() {
        let mut f = fonts().lock().unwrap();
        let t = Text::default();
        let red = Text {
            color: [1.0, 0.0, 0.0, 1.0],
            box_color: [0.0, 0.0, 1.0, 1.0],
            ..t.clone()
        };
        let a = masks(&mut f, &t, (64, 36));
        assert!(Arc::ptr_eq(&a, &masks(&mut f, &red, (64, 36))));
    }

    #[test]
    fn dilate_grows_by_radius() {
        let mut m = vec![0u8; 25];
        m[12] = 255;
        let d = dilate(&m, (5, 5), 1.0);
        assert_eq!(d.iter().filter(|v| **v == 255).count(), 5, "a plus shape");
        assert_eq!(bounds(&d, 5), Some([1, 1, 4, 4]));
    }
}
