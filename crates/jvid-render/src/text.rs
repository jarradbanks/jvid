//! Text/title layers: shaped and rasterized with `cosmic-text` into a straight RGBA image the
//! size of the frame, so the compositor's fit fills the frame and transforms, crop and
//! opacity apply as to any clip.

use std::sync::{Arc, Mutex, OnceLock};

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};
use jvid_core::model::{Align, Text};
use wgpu::util::DeviceExt;

/// A style and frame size.
type Key = (Text, (u32, u32));

struct Fonts {
    system: FontSystem,
    swash: SwashCache,
    /// Recent rasters, newest last.
    // shortcut: a linear scan over 8 entries; key by hash if many text layers show at once.
    recent: Vec<(Key, Arc<Vec<u8>>)>,
}

fn fonts() -> &'static Mutex<Fonts> {
    static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();
    FONTS.get_or_init(|| {
        Mutex::new(Fonts {
            system: FontSystem::new(),
            swash: SwashCache::new(),
            recent: Vec::new(),
        })
    })
}

/// Glyph coverage of `text` laid out on a `size` frame: one byte per pixel.
fn coverage(f: &mut Fonts, text: &Text, size: (u32, u32)) -> Vec<u8> {
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
    // Wrap at the frame width; lines align within the widest one.
    buf.set_size(&mut f.system, Some(w as f32), None);
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
        Align::Center => (w as f32 - block_w) / 2.0,
        Align::Right => w as f32 - block_w,
    };
    let dx = (text.anchor[0] * w as f32 - block_w / 2.0 - left).round() as i32;
    let dy = (text.anchor[1] * h as f32 - block_h / 2.0).round() as i32;
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
    let fill = coverage(&mut f, text, size);
    // Premultiplied layers, bottom first: box, shadow, outline, fill.
    let mut px = vec![[0f32; 4]; fill.len()];
    let mut over = |mask: &dyn Fn(i64, i64) -> f32, c: [f64; 4]| {
        for (k, p) in px.iter_mut().enumerate() {
            let a = mask((k as u32 % w) as i64, (k as u32 / w) as i64) * c[3] as f32;
            if a > 0.0 {
                for i in 0..3 {
                    p[i] = c[i] as f32 * a + p[i] * (1.0 - a);
                }
                p[3] = a + p[3] * (1.0 - a);
            }
        }
    };
    let at = |m: &[u8], x: i64, y: i64| {
        if (0..*w as i64).contains(&x) && (0..*h as i64).contains(&y) {
            m[(y * *w as i64 + x) as usize] as f32 / 255.0
        } else {
            0.0
        }
    };
    let shape = if text.outline > 0.0 {
        dilate(&fill, (*w, *h), text.outline)
    } else {
        fill.clone()
    };
    if text.box_color[3] > 0.0
        && let Some([x0, y0, x1, y1]) = bounds(&shape, *w)
    {
        let p = text.box_padding.max(0.0) as i64;
        let inside = |x, y| {
            ((x0 as i64 - p..x1 as i64 + p).contains(&x)
                && (y0 as i64 - p..y1 as i64 + p).contains(&y)) as u8 as f32
        };
        over(&inside, text.box_color);
    }
    if text.shadow != [0.0, 0.0] {
        let [sx, sy] = text.shadow.map(|v| v.round() as i64);
        over(&|x, y| at(&shape, x - sx, y - sy), text.shadow_color);
    }
    if text.outline > 0.0 {
        over(&|x, y| at(&shape, x, y), text.outline_color);
    }
    over(&|x, y| at(&fill, x, y), text.color);
    let img: Vec<u8> = px
        .iter()
        .flat_map(|p| {
            let a = p[3];
            let un = |v: f32| if a > 0.0 { v / a } else { 0.0 };
            [un(p[0]), un(p[1]), un(p[2]), a].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        })
        .collect();
    let img = Arc::new(img);
    if f.recent.len() >= 8 {
        f.recent.remove(0);
    }
    f.recent.push((key, img.clone()));
    img
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
        // Glyph 0 is .notdef: the font fell back to a tofu box.
        (!ids.contains(&0)).then_some((run.rtl, ids))
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
        // Joining picks contextual forms: the same letter differs alone and joined.
        let (_, alone) = shape("س").unwrap();
        assert_eq!(ids.len(), 3, "lam-alef forms one ligature: {ids:?}");
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
            eprintln!("skipping: no installed font");
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
    fn dilate_grows_by_radius() {
        let mut m = vec![0u8; 25];
        m[12] = 255;
        let d = dilate(&m, (5, 5), 1.0);
        assert_eq!(d.iter().filter(|v| **v == 255).count(), 5, "a plus shape");
        assert_eq!(bounds(&d, 5), Some([1, 1, 4, 4]));
    }
}
