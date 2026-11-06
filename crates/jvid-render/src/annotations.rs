//! Straight-alpha, project-size vector rasters shared by Program playback and export.
//! The ordinary compositor applies transforms, ordering, effects and clip fades afterwards.

use std::sync::{Arc, Mutex, OnceLock};

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache};
use jvid_core::annotations::{Annotation, AnnotationKind};

type Cached = (Annotation, (u32, u32), Arc<Vec<u8>>);
static CACHE: OnceLock<Mutex<Vec<Cached>>> = OnceLock::new();

/// Numbering is project content, so its font cannot depend on installed/system fonts.
/// Inter is the existing bundled OFL face; this database never loads external font paths.
fn digits(a: &Annotation, size: (u32, u32), rgba: &mut [u8]) {
    static FONTS: OnceLock<Mutex<(FontSystem, SwashCache)>> = OnceLock::new();
    let fonts = FONTS.get_or_init(|| {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_font_data(include_bytes!("../../suite-ui/assets/Inter-Regular.ttf").to_vec());
        db.set_sans_serif_family("Inter");
        Mutex::new((
            FontSystem::new_with_locale_and_db("en-US".into(), db),
            SwashCache::new(),
        ))
    });
    let mut fonts = fonts.lock().expect("annotation digits");
    let (system, swash) = &mut *fonts;
    let content = a.number.to_string();
    let font_size = (a.style.callout_radius * 1.25 / (content.len() as f64 * 0.5).max(1.0)) as f32;
    let mut buffer = Buffer::new(system, Metrics::new(font_size, font_size * 1.2));
    buffer.set_size(system, Some((a.style.callout_radius * 2.0) as f32), None);
    buffer.set_text(
        system,
        &content,
        &Attrs::new().family(Family::Name("Inter")),
        Shaping::Advanced,
        Some(cosmic_text::Align::Center),
    );
    buffer.shape_until_scroll(system, false);
    let height = buffer
        .layout_runs()
        .last()
        .map_or(0.0, |r| r.line_top + r.line_height);
    let cx = f64::from(size.0) / 2.0 + a.points[0][0];
    let cy = f64::from(size.1) / 2.0 + a.points[0][1];
    let dx = (cx - a.style.callout_radius).round() as i32;
    let dy = (cy - f64::from(height) / 2.0).round() as i32;
    buffer.draw(
        system,
        swash,
        Color::rgb(255, 255, 255),
        |x, y, w, h, color| {
            for yy in 0..h as i32 {
                for xx in 0..w as i32 {
                    let (x, y) = (x + xx + dx, y + yy + dy);
                    if x < 0 || y < 0 || x >= size.0 as i32 || y >= size.1 as i32 {
                        continue;
                    }
                    if (f64::from(x) + 0.5 - cx).hypot(f64::from(y) + 0.5 - cy)
                        >= a.style.callout_radius - a.style.width / 2.0
                    {
                        continue;
                    }
                    let i = ((y as u32 * size.0 + x as u32) * 4) as usize;
                    over(
                        &mut rgba[i..i + 4],
                        a.style.text_color,
                        f64::from(color.a()) / 255.0,
                    );
                }
            }
        },
    );
}

fn segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let length = d[0] * d[0] + d[1] * d[1];
    let k = if length > 0.0 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - k * d[0]).hypot(p[1] - a[1] - k * d[1])
}

fn triangle(p: [f64; 2], v: [[f64; 2]; 3]) -> bool {
    let cross =
        |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let s = [cross(v[0], v[1]), cross(v[1], v[2]), cross(v[2], v[0])];
    s.iter().all(|s| *s >= 0.0) || s.iter().all(|s| *s <= 0.0)
}

/// Stroke distance and whether the point lies in a filled arrowhead/callout badge.
fn sample(a: &Annotation, p: [f64; 2]) -> (f64, bool) {
    let [from, to] = a.points;
    let d = [to[0] - from[0], to[1] - from[1]];
    let len = d[0].hypot(d[1]);
    let unit = if len > 1e-9 {
        d.map(|v| v / len)
    } else {
        [1.0, 0.0]
    };
    match a.kind {
        AnnotationKind::Rectangle => {
            let c = [to[0], from[1]];
            let e = [from[0], to[1]];
            (
                [
                    segment(p, from, c),
                    segment(p, c, to),
                    segment(p, to, e),
                    segment(p, e, from),
                ]
                .into_iter()
                .fold(f64::INFINITY, f64::min),
                false,
            )
        }
        AnnotationKind::Arrow => {
            if len <= 1e-9 {
                return (segment(p, from, to), false);
            }
            let head = a.style.arrow_head.min(len);
            let base = [to[0] - unit[0] * head, to[1] - unit[1] * head];
            let wing = [unit[1] * head * 0.45, -unit[0] * head * 0.45];
            let v = [
                to,
                [base[0] + wing[0], base[1] + wing[1]],
                [base[0] - wing[0], base[1] - wing[1]],
            ];
            let edge = [
                segment(p, v[0], v[1]),
                segment(p, v[1], v[2]),
                segment(p, v[2], v[0]),
            ]
            .into_iter()
            .fold(f64::INFINITY, f64::min);
            (segment(p, from, base).min(edge), triangle(p, v))
        }
        AnnotationKind::Callout => {
            let r = a.style.callout_radius;
            let radial = (p[0] - from[0]).hypot(p[1] - from[1]);
            let leader = if len > r {
                segment(p, [from[0] + unit[0] * r, from[1] + unit[1] * r], to)
            } else {
                f64::INFINITY
            };
            ((radial - r).abs().min(leader), radial <= r)
        }
    }
}

/// Precise content hit with tolerance in project pixels. Editor converts logical screen
/// points through the common viewer/clip mapping before calling this.
pub fn hit(a: &Annotation, p: [f64; 2], tolerance: f64) -> bool {
    if !a.valid() {
        return false;
    }
    let (distance, inside) = sample(a, p);
    inside || distance <= a.style.width / 2.0 + tolerance.max(0.0)
}

pub fn drawn_bounds(a: &Annotation, size: (u32, u32)) -> Option<[u32; 4]> {
    if !a.valid() || size.0 == 0 || size.1 == 0 {
        return None;
    }
    let [x0, y0, x1, y1] = a.bounds();
    let x = |n: f64| (n + f64::from(size.0) / 2.0).clamp(0.0, f64::from(size.0));
    let y = |n: f64| (n + f64::from(size.1) / 2.0).clamp(0.0, f64::from(size.1));
    let b = [
        x(x0 - 1.0).floor() as u32,
        y(y0 - 1.0).floor() as u32,
        x(x1 + 1.0).ceil() as u32,
        y(y1 + 1.0).ceil() as u32,
    ];
    (b[0] < b[2] && b[1] < b[3]).then_some(b)
}

fn over(dst: &mut [u8], color: [f64; 4], coverage: f64) {
    let alpha = color[3] * coverage;
    if alpha <= 0.0 {
        return;
    }
    let back = f64::from(dst[3]) / 255.0;
    let out = alpha + back * (1.0 - alpha);
    for i in 0..3 {
        dst[i] = ((color[i] * alpha + f64::from(dst[i]) / 255.0 * back * (1.0 - alpha)) / out
            * 255.0)
            .round() as u8;
    }
    dst[3] = (out * 255.0).round() as u8;
}

/// Small bounded cache keeps parked playback from rerasterizing/uploading identical content.
/// No viewer zoom/pan enters either the raster or its cache key.
pub fn rasterize(a: &Annotation, size: (u32, u32)) -> Arc<Vec<u8>> {
    let cache = CACHE.get_or_init(Default::default);
    if let Some((_, _, pixels)) = cache
        .lock()
        .expect("annotation cache")
        .iter()
        .find(|(old, s, _)| old == a && *s == size)
    {
        return pixels.clone();
    }
    let mut rgba = vec![0; size.0 as usize * size.1 as usize * 4];
    if let Some([x0, y0, x1, y1]) = drawn_bounds(a, size) {
        for y in y0..y1 {
            for x in x0..x1 {
                let p = [
                    f64::from(x) + 0.5 - f64::from(size.0) / 2.0,
                    f64::from(y) + 0.5 - f64::from(size.1) / 2.0,
                ];
                let (distance, inside) = sample(a, p);
                let pixel = &mut rgba[((y * size.0 + x) * 4) as usize..][..4];
                if inside && a.kind == AnnotationKind::Callout {
                    over(pixel, a.style.fill_color, 1.0);
                }
                over(
                    pixel,
                    a.style.stroke_color,
                    if inside && a.kind == AnnotationKind::Arrow {
                        1.0
                    } else {
                        (a.style.width / 2.0 + 0.5 - distance).clamp(0.0, 1.0)
                    },
                );
            }
        }
        if a.kind == AnnotationKind::Callout {
            digits(a, size, &mut rgba);
        }
    }
    let pixels = Arc::new(rgba);
    let mut cache = cache.lock().expect("annotation cache");
    // ponytail: four project-sized rasters; use a byte budget if many 8K annotations matter.
    if cache.len() >= 4 {
        cache.remove(0);
    }
    cache.push((a.clone(), size, pixels.clone()));
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composite::{Compositor, export_png, read_back};
    use jvid_core::model::Transform;

    #[test]
    fn shapes_are_transparent_outside_content_and_hit_precisely() {
        for kind in [
            AnnotationKind::Arrow,
            AnnotationKind::Rectangle,
            AnnotationKind::Callout,
        ] {
            let a = Annotation {
                kind,
                points: [[-40.0, -20.0], [40.0, 20.0]],
                ..Default::default()
            };
            let pixels = rasterize(&a, (192, 108));
            assert_eq!(&pixels[..4], &[0; 4]);
            assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
            assert!(Arc::ptr_eq(&pixels, &rasterize(&a, (192, 108))));
            assert!(hit(&a, a.points[0], 3.0));
            assert!(!hit(&a, [500.0, 500.0], 3.0));
            if kind == AnnotationKind::Rectangle {
                assert!(!hit(&a, [0.0, 0.0], 0.0));
            }
        }
        let a = Annotation {
            points: [[0.0; 2]; 2],
            ..Default::default()
        };
        assert!(
            rasterize(&a, (64, 64))
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] > 0)
        );
    }

    #[test]
    fn transformed_stacking_preview_and_png_export_match() {
        let Some((device, queue)) = crate::composite::tests::device() else {
            eprintln!("skipping annotation GPU test: no configured adapter");
            return;
        };
        let size = (192, 108);
        let mut arrow = Annotation {
            points: [[-50.0, 0.0], [50.0, 0.0]],
            ..Default::default()
        };
        arrow.style.stroke_color = [1.0, 0.0, 0.0, 1.0];
        let texture = crate::text::texture(&device, &queue, size, &rasterize(&arrow, size));
        let background = crate::solid::solid(&device, &queue, size, [10, 20, 30, 255]);
        let transform = Transform {
            rotation: 37.0,
            scale: [0.8, 1.2],
            opacity: 0.65,
            ..Default::default()
        };
        let layers = [(&texture, transform), (&background, Transform::default())];
        let preview = read_back(
            &device,
            &queue,
            Compositor::new(&device).render(&device, &queue, size, 1, [0, 0, 0, 255], &layers),
        );
        let path = std::env::temp_dir().join(format!("jvid-annotation-{}.png", std::process::id()));
        export_png(&device, &queue, size, [0, 0, 0, 255], &layers, &path).unwrap();
        let exported = image::open(&path).unwrap().into_rgba8();
        std::fs::remove_file(path).unwrap();
        assert_eq!(exported.as_raw(), &preview);
        assert!(
            preview.as_chunks::<4>().0.iter().any(|p| p[0] > 100),
            "arrow appears above background"
        );
        assert_eq!(&preview[..4], &[10, 20, 30, 255]);
    }

    #[test]
    fn callout_digits_use_only_bundled_inter_and_are_antialiased() {
        let a = Annotation {
            kind: AnnotationKind::Callout,
            points: [[0.0; 2]; 2],
            number: 1234,
            ..Default::default()
        };
        let mut one = vec![0; 128 * 128 * 4];
        let mut two = one.clone();
        digits(&a, (128, 128), &mut one);
        digits(&a, (128, 128), &mut two);
        assert_eq!(one, two, "isolated embedded font is deterministic");
        assert!(
            one.as_chunks::<4>().0.iter().any(|p| p[3] == 255),
            "digits have visible ink without system fonts"
        );
        assert!(
            one.as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] > 0 && p[3] < 255),
            "glyph edges are antialiased"
        );
        two.fill(0);
        digits(&Annotation { number: 5678, ..a }, (128, 128), &mut two);
        assert_ne!(one, two);
    }
}
