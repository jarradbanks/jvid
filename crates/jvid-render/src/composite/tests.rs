use std::future::Future;
use std::path::Path;

use suite_ui::test_support::gpu;
use wgpu::util::DeviceExt;

use super::*;

/// In 8-bit codes, for filtering and rounding differences between software rasterizers.
const TOLERANCE: i32 = 2;

/// wgpu's native futures are ready when polled, so a busy poll stands in for an executor.
pub(crate) fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

/// A software device: WARP on Windows, lavapipe (or what `gpu()` allows) elsewhere.
/// `None` skips, as on macOS, which has no software adapter.
pub(crate) fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let warp = cfg!(windows);
    if !warp && !gpu() {
        return None;
    }
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
    if warp {
        desc.backends = wgpu::Backends::DX12;
    }
    let instance = wgpu::Instance::new(desc);
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        force_fallback_adapter: warp,
        ..Default::default()
    }))
    .expect("software adapter");
    Some(block_on(adapter.request_device(&Default::default())).expect("device"))
}

fn image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    (w, h): (u32, u32),
    px: impl Fn(u32, u32) -> [u8; 4],
) -> wgpu::Texture {
    let data: Vec<u8> = (0..w * h).flat_map(|i| px(i % w, i / w)).collect();
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        Default::default(),
        &data,
    )
}

/// Compares against `testdata/<name>.png`. To bless (add or update) goldens, run
/// `JVID_BLESS=1 cargo test -p jvid-render` and commit the PNGs; a missing golden fails otherwise.
fn golden(name: &str, w: u32, h: u32, rgba: &[u8]) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("testdata/{name}.png"));
    if std::env::var_os("JVID_BLESS").is_some() {
        image::save_buffer(&path, rgba, w, h, image::ExtendedColorType::Rgba8).unwrap();
        return;
    }
    assert!(
        path.exists(),
        "missing golden {}; bless with JVID_BLESS=1",
        path.display()
    );
    let want = image::open(&path).unwrap().into_rgba8();
    assert_eq!(want.dimensions(), (w, h), "{name}");
    for (i, (g, e)) in rgba.iter().zip(want.as_raw()).enumerate() {
        let (x, y) = ((i / 4) as u32 % w, (i / 4) as u32 / w);
        assert!(
            (*g as i32 - *e as i32).abs() <= TOLERANCE,
            "{name} at ({x}, {y}) channel {}: {g}, want {e}",
            i % 4
        );
    }
}

fn op(opacity: f64) -> Transform {
    Transform {
        opacity,
        ..Default::default()
    }
}

fn near(got: u8, want: i32) -> bool {
    (got as i32 - want).abs() <= 1
}

#[test]
fn layer_stack_fits_and_blends_over_background() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping compositor golden test: no software adapter on this platform");
        return;
    };
    // Bottom: 2:1 yellow | blue, filling the 2:1 frame. Middle: 4:1 green, letterboxed.
    // Top: 1:1 red at half opacity, pillarboxed. Above that, an invisible layer.
    let base = image(&device, &queue, (128, 64), |x, _| {
        if x < 64 {
            [255, 255, 0, 255]
        } else {
            [0, 0, 255, 255]
        }
    });
    let green = image(&device, &queue, (64, 16), |_, _| [0, 255, 0, 255]);
    let red = image(&device, &queue, (32, 32), |_, _| [255, 0, 0, 255]);
    let white = image(&device, &queue, (8, 8), |_, _| [255; 4]);
    let layers = [
        (&white, op(0.0)),
        (&red, op(0.5)),
        (&green, op(1.0)),
        (&base, op(1.0)),
    ];
    let mut comp = Compositor::new(&device);
    for scale in [1, 2] {
        let tex = comp.render(&device, &queue, (64, 32), scale, [32, 64, 96, 255], &layers);
        let w = tex.width();
        assert_eq!((w, tex.height()), (64 / scale, 32 / scale));
        let rgba = read_back(&device, &queue, tex);
        // Pixels in full-frame coordinates.
        let px = |x: u32, y: u32| {
            let i = ((y / scale) * w + x / scale) as usize * 4;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };
        assert_eq!(px(2, 2), [255, 255, 0, 255], "uncovered base");
        assert_eq!(px(62, 16), [0, 255, 0, 255], "letterboxed green");
        let [r, g, _, a] = px(30, 16);
        assert!(
            near(r, 128) && near(g, 128) && a == 255,
            "half red over green"
        );
        let [r, _, b, _] = px(40, 2);
        assert!(near(r, 128) && near(b, 128), "half red over blue");
        golden(&format!("stack-1-{scale}"), w, tex.height(), &rgba);
    }
    // Nothing visible: the background alone, premultiplied.
    let tex = comp.render(&device, &queue, (8, 8), 1, [255, 0, 0, 128], &[]);
    let rgba = read_back(&device, &queue, tex);
    assert!(
        near(rgba[0], 128) && rgba[1] == 0 && rgba[3] == 128,
        "{:?}",
        &rgba[..4]
    );
}

#[test]
fn exported_png_matches_read_back_exactly() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping PNG export test: no software adapter on this platform");
        return;
    };
    let base = image(&device, &queue, (48, 20), |x, _| {
        [x as u8 * 5, 40, 200, 255]
    });
    let red = image(&device, &queue, (16, 16), |_, _| [255, 0, 0, 255]);
    let layers = [(&red, op(0.4)), (&base, op(1.0))];
    let (size, bg) = ((40, 30), [10, 20, 30, 255]);
    let mut comp = Compositor::new(&device);
    let tex = comp.render(&device, &queue, size, 1, bg, &layers);
    let want = read_back(&device, &queue, tex);
    let path = std::env::temp_dir().join(format!("jvid-export-{}.png", std::process::id()));
    export_png(&device, &queue, size, bg, &layers, &path).unwrap();
    let got = image::open(&path).unwrap().into_rgba8();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(got.dimensions(), size);
    assert!(
        *got.as_raw() == want,
        "PNG differs from the composited frame"
    );
}

/// Frames rendered into one reused target come back one push late, in order, each its own.
#[test]
fn readback_returns_each_frame_one_push_late() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping readback test: no software adapter on this platform");
        return;
    };
    let mut comp = Compositor::new(&device);
    let mut rb = Readback::default();
    let mut got = Vec::new();
    for v in [10u8, 20, 30] {
        let tex = comp.render(&device, &queue, (24, 10), 1, [v, 0, 0, 255], &[]);
        let prev = rb.push(&device, &queue, tex);
        assert_eq!(prev.is_some(), v > 10);
        got.extend(prev);
    }
    got.extend(rb.take(&device));
    assert!(rb.take(&device).is_none());
    let reds: Vec<_> = got
        .iter()
        .map(|f| (f.len(), f[0], f[f.len() - 4]))
        .collect();
    assert_eq!(reds, [(960, 10, 10), (960, 20, 20), (960, 30, 30)]);
}

/// Each property static, then keyframed so its value at the midpoint equals the static one: both
/// frames must hit the same pixels, and the first keyframe's frame must differ.
#[test]
fn transform_crop_and_opacity_static_and_animated() {
    use jvid_core::keyframe::{Easing, Keyframe, Value};
    use jvid_core::model::{AssetId, Clip, ClipId};
    use jvid_core::time::Time;
    let Some((device, queue)) = device() else {
        eprintln!("skipping transform golden test: no software adapter on this platform");
        return;
    };
    // 2:1 quadrants, fitted into the 64x64 frame at x 0..64, y 16..48 (4 project px per texel).
    let img = image(&device, &queue, (16, 8), |x, y| {
        let (l, t) = (x < 8, y < 4);
        [
            if l { 255 } else { 0 },
            if l { 0 } else { 255 },
            if t { 255 } else { 0 },
            255,
        ]
    });
    const BG: [u8; 4] = [0, 0, 0, 255];
    const TL: [u8; 4] = [255, 0, 255, 255];
    const TR: [u8; 4] = [0, 255, 255, 255];
    const BL: [u8; 4] = [255, 0, 0, 255];
    const BR: [u8; 4] = [0, 255, 0, 255];
    const HALF_TL: [u8; 4] = [128, 0, 128, 255];
    let x = Transform::default();
    let anchor_tl = Transform {
        anchor: [-32.0, -16.0],
        ..x
    };
    type Case = (
        &'static str,
        Transform,
        Transform,
        &'static str,
        Value,
        Value,
        &'static [(u32, u32, [u8; 4])],
    );
    #[rustfmt::skip]
    let cases: [Case; 8] = [
        // Anchor lands at (48, 24): the image covers x 16..80, y 8..40.
        ("position", Transform { position: [16.0, -8.0], ..x }, x, "position",
            Value::Vec2([0.0, 0.0]), Value::Vec2([32.0, -16.0]),
            &[(20, 10, TL), (10, 30, BG), (20, 36, BL), (60, 4, BG)]),
        // Linked half size: x 16..48, y 24..40.
        ("scale", Transform { scale: [0.5; 2], ..x }, x, "scale",
            Value::Scalar(1.0), Value::Scalar(0.0),
            &[(18, 26, TL), (46, 38, BR), (12, 32, BG), (20, 20, BG)]),
        // Separate X/Y: full width, half height, y 24..40.
        ("scale-xy", Transform { scale: [1.0, 0.5], ..x }, x, "scale",
            Value::Vec2([1.0, 1.0]), Value::Vec2([1.0, 0.0]),
            &[(2, 26, TL), (62, 38, BR), (2, 22, BG), (62, 42, BG)]),
        // 90 degrees clockwise: portrait x 16..48, y 0..64, top-left corner now top-right.
        ("rotation", Transform { rotation: 90.0, ..x }, x, "rotation",
            Value::Scalar(0.0), Value::Scalar(180.0),
            &[(44, 4, TL), (44, 60, TR), (20, 4, BL), (20, 60, BR), (8, 32, BG)]),
        // The top-left corner sits at the centre: x 32..96, y 32..64.
        ("anchor", anchor_tl, x, "anchor",
            Value::Vec2([0.0, 0.0]), Value::Vec2([-64.0, -32.0]),
            &[(34, 34, TL), (30, 34, BG), (34, 30, BG), (34, 62, BL)]),
        // Rotation pivots on the anchor: clockwise about the top-left gives x 0..32, y 32..96.
        ("anchor-rotation", Transform { rotation: 90.0, ..anchor_tl }, anchor_tl, "rotation",
            Value::Scalar(0.0), Value::Scalar(180.0),
            &[(30, 34, TL), (34, 34, BG), (30, 30, BG), (2, 34, BL)]),
        // 16 px off the left; the rest doesn't move: x 16..64.
        ("crop", Transform { crop: [16.0, 0.0, 0.0, 0.0], ..x }, x, "crop_left",
            Value::Scalar(0.0), Value::Scalar(32.0),
            &[(12, 20, BG), (18, 20, TL), (40, 20, TR), (40, 44, BR)]),
        ("opacity", Transform { opacity: 0.5, ..x }, x, "opacity",
            Value::Scalar(1.0), Value::Scalar(0.0),
            &[(10, 20, HALF_TL), (10, 8, BG)]),
    ];
    let mut comp = Compositor::new(&device);
    let mut frame = |t: Transform| {
        let tex = comp.render(&device, &queue, (64, 64), 1, BG, &[(&img, t)]);
        read_back(&device, &queue, tex)
    };
    for (name, still, base, param, a, b, checks) in cases {
        let key = |time, value| Keyframe {
            time: Time(time),
            value,
            easing: Easing::Linear,
        };
        let clip = Clip {
            id: ClipId(1),
            asset: AssetId(1),
            source_in: Time(0),
            source_out: Time(20),
            start: Time(0),
            link: None,
            params: Default::default(),
            keyframes: [(param.to_string(), vec![key(0, a), key(10, b)])].into(),
            transform: base,
        };
        assert_eq!(clip.transform_at(Time(5)), still, "{name} midpoint");
        let first = frame(clip.transform_at(Time(0)));
        for (kind, rgba) in [
            ("static", frame(still)),
            ("animated", frame(clip.transform_at(Time(5)))),
        ] {
            for &(x, y, want) in checks {
                let i = (y * 64 + x) as usize * 4;
                let got = &rgba[i..i + 4];
                assert!(
                    got.iter().zip(want).all(|(&g, w)| near(g, w as i32)),
                    "{name} {kind} at ({x}, {y}): {got:?}, want {want:?}"
                );
            }
            assert!(
                first != rgba,
                "{name} {kind}: no change from the first keyframe"
            );
            golden(&format!("{name}-{kind}"), 64, 64, &rgba);
        }
    }
}

#[test]
fn quad_corners_follow_rotation_and_crop() {
    let frame = [100.0, 50.0];
    // A 2:1 image fills the frame; 90 degrees clockwise puts its top-left corner at top-right.
    let t = Transform {
        rotation: 90.0,
        ..Default::default()
    };
    let q = quad([20.0, 10.0], frame, &t).unwrap();
    let px = |c: [f32; 4]| [(c[0] + 1.0) * 50.0, (1.0 - c[1]) * 25.0];
    let [x, y] = px(q[0]);
    assert!(
        (x - 75.0).abs() < 1e-3 && (y - -25.0).abs() < 1e-3,
        "{x}, {y}"
    );
    assert_eq!([q[3][2], q[3][3]], [1.0, 1.0]);
    let crop = |c| {
        quad(
            [20.0, 10.0],
            frame,
            &Transform {
                crop: c,
                ..Default::default()
            },
        )
    };
    assert_eq!(crop([25.0, 0.0, 0.0, 0.0]).unwrap()[0][2], 0.25);
    assert!(crop([50.0, 0.0, 50.0, 0.0]).is_none());
}

#[test]
fn blend_modes_match_reference_and_goldens() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping blend golden test: no software adapter on this platform");
        return;
    };
    // Base: four stripes over the opaque background. Top: one colour, opaque above,
    // half alpha below, so both the mode and its alpha mix show.
    let stripes = [[0, 0, 0], [64, 128, 32], [192, 64, 224], [255, 255, 255]];
    let base = image(&device, &queue, (64, 32), |x, _| {
        let [r, g, b] = stripes[x as usize / 16];
        [r, g, b, 255]
    });
    let top = image(&device, &queue, (64, 32), |_, y| {
        [200, 100, 50, if y < 16 { 255 } else { 128 }]
    });
    let reference = |mode: Blend, b: f64, s: f64| match mode {
        Blend::Normal => s,
        Blend::Multiply => b * s,
        Blend::Screen => b + s - b * s,
        Blend::Overlay if b <= 0.5 => 2.0 * b * s,
        Blend::Overlay => 1.0 - 2.0 * (1.0 - b) * (1.0 - s),
        Blend::Add => (b + s).min(1.0),
        Blend::Darken => b.min(s),
        Blend::Lighten => b.max(s),
        Blend::Difference => (b - s).abs(),
    };
    let mut comp = Compositor::new(&device);
    for mode in Blend::ALL {
        let t = Transform {
            blend: mode,
            ..Default::default()
        };
        let layers = [(&top, t), (&base, Transform::default())];
        let tex = comp.render(&device, &queue, (64, 32), 1, [0, 0, 0, 255], &layers);
        let rgba = read_back(&device, &queue, tex);
        for (x, y) in [(8, 8), (24, 8), (40, 8), (56, 8), (24, 24), (40, 24)] {
            let i = (y * 64 + x) as usize * 4;
            let a = if y < 16 { 1.0 } else { 128.0 / 255.0 };
            let under = stripes[x as usize / 16];
            for (c, s) in [200.0, 100.0, 50.0].into_iter().enumerate() {
                let b = under[c] as f64 / 255.0;
                let want = a * reference(mode, b, s / 255.0) + (1.0 - a) * b;
                let want = (want * 255.0).round() as i32;
                assert!(
                    (rgba[i + c] as i32 - want).abs() <= TOLERANCE,
                    "{mode:?} at ({x}, {y}) channel {c}: {}, want {want}",
                    rgba[i + c]
                );
            }
        }
        golden(&format!("blend-{mode:?}").to_lowercase(), 64, 32, &rgba);
    }
}

#[test]
fn blend_over_transparent_background_keeps_the_layer() {
    let Some((device, queue)) = device() else {
        return;
    };
    // A blended bottom layer over nothing has no backdrop to mix with, so it shows as is.
    let top = image(&device, &queue, (8, 8), |_, _| [200, 100, 50, 255]);
    let t = Transform {
        blend: Blend::Multiply,
        ..Default::default()
    };
    let mut comp = Compositor::new(&device);
    let tex = comp.render(&device, &queue, (8, 8), 1, [0; 4], &[(&top, t)]);
    let rgba = read_back(&device, &queue, tex);
    assert!(
        rgba[..4]
            .iter()
            .zip([200, 100, 50, 255])
            .all(|(&g, w)| near(g, w)),
        "{:?}",
        &rgba[..4]
    );
}
