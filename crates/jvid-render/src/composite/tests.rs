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

#[test]
fn readback_dropped_mid_frame_does_not_panic() {
    let Some((device, queue)) = device() else {
        return;
    };
    let mut comp = Compositor::new(&device);
    let mut rb = Readback::default();
    let tex = comp.render(&device, &queue, (24, 10), 1, [10, 0, 0, 255], &[]);
    assert!(rb.push(&device, &queue, tex).is_none());
    drop(rb);
    // Runs the dropped copy's map callback, which used to unwrap a BufferAsyncError.
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
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
        ..x.clone()
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
        ("position", Transform { position: [16.0, -8.0], ..x.clone() }, x.clone(), "position",
            Value::Vec2([0.0, 0.0]), Value::Vec2([32.0, -16.0]),
            &[(20, 10, TL), (10, 30, BG), (20, 36, BL), (60, 4, BG)]),
        // Linked half size: x 16..48, y 24..40.
        ("scale", Transform { scale: [0.5; 2], ..x.clone() }, x.clone(), "scale",
            Value::Scalar(1.0), Value::Scalar(0.0),
            &[(18, 26, TL), (46, 38, BR), (12, 32, BG), (20, 20, BG)]),
        // Separate X/Y: full width, half height, y 24..40.
        ("scale-xy", Transform { scale: [1.0, 0.5], ..x.clone() }, x.clone(), "scale",
            Value::Vec2([1.0, 1.0]), Value::Vec2([1.0, 0.0]),
            &[(2, 26, TL), (62, 38, BR), (2, 22, BG), (62, 42, BG)]),
        // 90 degrees clockwise: portrait x 16..48, y 0..64, top-left corner now top-right.
        ("rotation", Transform { rotation: 90.0, ..x.clone() }, x.clone(), "rotation",
            Value::Scalar(0.0), Value::Scalar(180.0),
            &[(44, 4, TL), (44, 60, TR), (20, 4, BL), (20, 60, BR), (8, 32, BG)]),
        // The top-left corner sits at the centre: x 32..96, y 32..64.
        ("anchor", anchor_tl.clone(), x.clone(), "anchor",
            Value::Vec2([0.0, 0.0]), Value::Vec2([-64.0, -32.0]),
            &[(34, 34, TL), (30, 34, BG), (34, 30, BG), (34, 62, BL)]),
        // Rotation pivots on the anchor: clockwise about the top-left gives x 0..32, y 32..96.
        ("anchor-rotation", Transform { rotation: 90.0, ..anchor_tl.clone() }, anchor_tl.clone(), "rotation",
            Value::Scalar(0.0), Value::Scalar(180.0),
            &[(30, 34, TL), (34, 34, BG), (30, 30, BG), (2, 34, BL)]),
        // 16 px off the left; the rest doesn't move: x 16..64.
        ("crop", Transform { crop: [16.0, 0.0, 0.0, 0.0], ..x.clone() }, x.clone(), "crop_left",
            Value::Scalar(0.0), Value::Scalar(32.0),
            &[(12, 20, BG), (18, 20, TL), (40, 20, TR), (40, 44, BR)]),
        ("opacity", Transform { opacity: 0.5, ..x.clone() }, x.clone(), "opacity",
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
            stream: Some(0),
            enabled: true,
            speed: 1.0,
            reverse: false,
            freeze: None,
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

#[test]
fn neutral_grade_is_bit_identical_and_adjustments_apply_in_linear_light() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping grade test: no software adapter on this platform");
        return;
    };
    // Every 8-bit code on each channel, one texel per project pixel.
    let px = |x: u32, y: u32| {
        [
            x as u8,
            (x as u8).wrapping_add(85 * y as u8),
            255 - x as u8,
            255,
        ]
    };
    let img = image(&device, &queue, (256, 4), px);
    let mut comp = Compositor::new(&device);
    let mut run = |t: Transform| {
        let tex = comp.render(&device, &queue, (256, 4), 1, [0, 0, 0, 255], &[(&img, t)]);
        read_back(&device, &queue, tex)
    };
    let want: Vec<u8> = (0..256 * 4).flat_map(|i| px(i % 256, i / 256)).collect();
    assert_eq!(
        run(Transform::default()),
        want,
        "neutral leaves pixels untouched"
    );
    let blended = Transform {
        blend: jvid_core::model::Blend::Lighten,
        ..Default::default()
    };
    assert_eq!(run(blended), want, "neutral through the blend pipeline too");

    let at = |rgba: &[u8], x: usize| [rgba[x * 4], rgba[x * 4 + 1], rgba[x * 4 + 2]];
    // +1 stop doubles linear light: sRGB 128 (0.216) -> 0.432 -> 176.
    let up = run(Transform {
        exposure: 1.0,
        ..Default::default()
    });
    assert!(near(at(&up, 128)[0], 176), "{:?}", at(&up, 128));
    // No saturation leaves linear Rec.709 luma: pure red (x 255 has r 255, b 0) -> 0.2126 -> 127.
    let grey = run(Transform {
        saturation: -1.0,
        ..Default::default()
    });
    let g = at(&grey, 255);
    assert!(
        g.iter().all(|c| (*c as i32 - g[0] as i32).abs() <= 1),
        "{g:?}"
    );
    // Warmer raises red and lowers blue.
    let warm = run(Transform {
        temperature: 1.0,
        ..Default::default()
    });
    assert!(at(&warm, 128)[0] > 128 && at(&warm, 128)[2] < 127);
}

#[test]
fn default_wheels_change_nothing_and_lift_raises_shadows_but_not_white() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping wheels test: no software adapter on this platform");
        return;
    };
    // A grey ramp: every 8-bit code once.
    let px = |x: u32, _| [x as u8, x as u8, x as u8, 255];
    let img = image(&device, &queue, (256, 1), px);
    let mut comp = Compositor::new(&device);
    let mut run = |t: Transform| {
        let tex = comp.render(&device, &queue, (256, 1), 1, [0, 0, 0, 255], &[(&img, t)]);
        read_back(&device, &queue, tex)
    };
    let want: Vec<u8> = (0..256).flat_map(|x| px(x, 0)).collect();
    let mut t = Transform::default();
    assert!(t.wheels.is_default());
    assert_eq!(
        run(t.clone()),
        want,
        "default wheels leave pixels untouched"
    );

    t.wheels.lift.master = 1.0;
    let lifted = run(t.clone());
    let at = |rgba: &[u8], x: usize| rgba[x * 4];
    assert!(at(&lifted, 0) > 100, "black lifts: {}", at(&lifted, 0));
    assert!(at(&lifted, 64) > 64, "shadows rise: {}", at(&lifted, 64));
    assert_eq!(at(&lifted, 255), 255, "white stays");

    // Lift towards red: shadows warm, white stays white.
    t.wheels.lift = jvid_core::model::Wheel {
        color: [0.0, 1.0],
        master: 0.0,
    };
    let red = run(t.clone());
    assert!(
        red[64 * 4] > red[64 * 4 + 2],
        "{:?}",
        &red[64 * 4..64 * 4 + 3]
    );
    assert_eq!(&red[255 * 4..255 * 4 + 3], &[255, 255, 255]);

    // Gain halves, gamma brightens mids, offset shifts black.
    t.wheels = Default::default();
    t.wheels.gain.master = -1.0;
    assert!(at(&run(t.clone()), 255) < 200);
    t.wheels = Default::default();
    t.wheels.gamma.master = 0.5;
    let g = run(t.clone());
    assert!(at(&g, 128) > 128 && at(&g, 0) == 0 && at(&g, 255) == 255);
    t.wheels = Default::default();
    t.wheels.offset.master = 0.5;
    assert!(at(&run(t), 0) > 0);
}

/// Trilinear lookup on the CPU, the reference the shader must match.
fn lut_reference(lut: &crate::lut::Lut, c: [f32; 3]) -> [f32; 3] {
    let n = lut.size as usize;
    let p = c.map(|v| v.clamp(0.0, 1.0) * (n - 1) as f32);
    let lo = p.map(|v| (v.floor() as usize).min(n - 2));
    let f: [f32; 3] = std::array::from_fn(|i| p[i] - lo[i] as f32);
    let mut out = [0.0; 3];
    for corner in 0..8 {
        let d = [corner & 1, corner >> 1 & 1, corner >> 2];
        let w: f32 = (0..3)
            .map(|i| if d[i] == 1 { f[i] } else { 1.0 - f[i] })
            .product();
        let node = lut.rgb[(lo[0] + d[0]) + (lo[1] + d[1]) * n + (lo[2] + d[2]) * n * n];
        (0..3).for_each(|i| out[i] += w * node[i]);
    }
    out
}

#[test]
fn lut_matches_a_trilinear_reference_and_mixes_by_intensity() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping lut test: no software adapter on this platform");
        return;
    };
    // A non-separable 9³ grid, coarse so interpolation between nodes is exercised.
    let n = 9;
    let mut text = format!("TITLE \"test\"\nLUT_3D_SIZE {n}\n");
    for i in 0..n * n * n {
        let [r, g, b] = [i % n, i / n % n, i / n / n].map(|v| v as f32 / (n - 1) as f32);
        text += &format!("{} {} {}\n", g * g, (r + b) / 2.0, 1.0 - r * b);
    }
    let lut = crate::lut::parse(&text).unwrap();
    let px = |x: u32, y: u32| {
        [
            x as u8,
            (x as u8).wrapping_mul(7).wrapping_add(60 * y as u8),
            255 - x as u8,
            255,
        ]
    };
    let img = image(&device, &queue, (256, 4), px);
    let mut comp = Compositor::new(&device);
    comp.add_lut(&device, &queue, "a.cube", &lut);
    let mut run = |t: Transform| {
        let tex = comp.render(&device, &queue, (256, 4), 1, [0, 0, 0, 255], &[(&img, t)]);
        read_back(&device, &queue, tex)
    };
    let with = |path: &str, lut_intensity| Transform {
        lut: Some(path.into()),
        lut_intensity,
        ..Default::default()
    };
    let want: Vec<u8> = (0..256 * 4).flat_map(|i| px(i % 256, i / 256)).collect();
    assert_eq!(
        run(with("missing.cube", 1.0)),
        want,
        "an unloaded LUT is skipped"
    );
    assert_eq!(run(with("a.cube", 0.0)), want, "zero intensity is skipped");
    for (k, got) in [
        (1.0, run(with("a.cube", 1.0))),
        (0.5, run(with("a.cube", 0.5))),
    ] {
        for (i, (got, src)) in got.chunks(4).zip(want.chunks(4)).enumerate() {
            let c = [0, 1, 2].map(|j| src[j] as f32 / 255.0);
            let r = lut_reference(&lut, c);
            for j in 0..3 {
                let w = ((c[j] + (r[j] - c[j]) * k) * 255.0).round() as i32;
                assert!(
                    near(got[j], w),
                    "px {i} ch {j} at {k}: got {}, want {w}",
                    got[j]
                );
            }
        }
    }
}

#[test]
fn effects_run_in_order_and_unknown_ones_pass_through() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping effect test: no software adapter on this platform");
        return;
    };
    let src = image(&device, &queue, (8, 8), |_, _| [200, 100, 50, 255]);
    let invert = crate::effect::parse(
        "// @jvid effect \"Invert\"
// @param amount float 0.0..1.0 = 1.0
fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = textureSample(src, smp, uv);
    return vec4<f32>(mix(c.rgb, 1.0 - c.rgb, fx.amount), c.a);
}",
    )
    .unwrap();
    let mut comp = Compositor::new(&device);
    comp.set_effect(&device, "invert", Some(&invert)).unwrap();
    let fx = |shader: &str, amount: f64| Effect {
        shader: shader.into(),
        params: [("amount".to_owned(), amount)].into(),
        enabled: true,
    };
    let run = |comp: &mut Compositor, effects: Vec<Effect>| {
        let t = Transform {
            effects,
            ..Default::default()
        };
        let tex = comp.render(&device, &queue, (8, 8), 1, [0, 0, 0, 255], &[(&src, t)]);
        read_back(&device, &queue, tex)[..3].to_vec()
    };
    let same = |a: &[u8], b: [i32; 3]| a.iter().zip(b).all(|(a, b)| near(*a, b));
    assert!(same(&run(&mut comp, vec![]), [200, 100, 50]));
    assert!(same(
        &run(&mut comp, vec![fx("invert", 1.0)]),
        [55, 155, 205]
    ));
    // Missing (or failed) shaders are skipped; two inverts cancel.
    assert!(same(
        &run(&mut comp, vec![fx("gone", 1.0), fx("invert", 0.0)]),
        [200, 100, 50]
    ));
    assert!(same(
        &run(&mut comp, vec![fx("invert", 1.0), fx("invert", 1.0)]),
        [200, 100, 50]
    ));
    // Bypassed: skipped, so only the other one inverts.
    let off = Effect {
        enabled: false,
        ..fx("invert", 1.0)
    };
    assert!(same(&run(&mut comp, vec![off.clone()]), [200, 100, 50]));
    assert!(same(
        &run(&mut comp, vec![off, fx("invert", 1.0)]),
        [55, 155, 205]
    ));
    comp.set_effect(&device, "invert", None).unwrap();
    assert!(same(
        &run(&mut comp, vec![fx("invert", 1.0)]),
        [200, 100, 50]
    ));
}

#[test]
fn effect_ignoring_fx_renders_and_extra_bindings_are_rejected() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping effect test: no software adapter on this platform");
        return;
    };
    let src = image(&device, &queue, (8, 8), |_, _| [200, 100, 50, 255]);
    // No `fx` use, so an auto-derived layout would leave out binding 2.
    let parse = |decl: &str, ret: &str| {
        crate::effect::parse(&format!(
            "// @jvid effect \"X\"\n{decl}\nfn effect(uv: vec2<f32>) -> vec4<f32> {{ return {ret}; }}"
        ))
        .unwrap()
    };
    let mut comp = Compositor::new(&device);
    comp.set_effect(
        &device,
        "swap",
        Some(&parse("", "textureSample(src, smp, uv).bgra")),
    )
    .unwrap();
    let extra = parse(
        "@group(0) @binding(3) var<uniform> mine: vec4<f32>;",
        "mine",
    );
    assert!(comp.set_effect(&device, "extra", Some(&extra)).is_err());
    let t = Transform {
        effects: vec![Effect {
            shader: "swap".into(),
            params: Default::default(),
            enabled: true,
        }],
        ..Default::default()
    };
    let tex = comp.render(&device, &queue, (8, 8), 1, [0, 0, 0, 255], &[(&src, t)]);
    let px = read_back(&device, &queue, tex);
    assert!(near(px[0], 50) && near(px[2], 200), "{:?}", &px[..4]);
}

#[test]
fn chroma_key_shows_the_lower_track_and_suppresses_spill() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping effect test: no software adapter on this platform");
        return;
    };
    // Columns: green screen, skin-tone foreground, a foreground edge with a green cast, blue screen.
    let cols = [
        [40, 200, 60, 255],
        [220, 170, 140, 255],
        [150, 190, 140, 255],
        [30, 60, 210, 255],
    ];
    let fg = image(&device, &queue, (32, 8), |x, _| cols[x as usize / 8]);
    let red = image(&device, &queue, (32, 8), |_, _| [255, 0, 0, 255]);
    let (_, src) = crate::effect::BUNDLED
        .iter()
        .find(|(f, _)| *f == "chroma_key.wgsl")
        .unwrap();
    let mut comp = Compositor::new(&device);
    comp.set_effect(
        &device,
        "chroma_key",
        Some(&crate::effect::parse(src).unwrap()),
    )
    .unwrap();
    // The centre pixel of each column, keyed with defaults but `key_hue` and `spill_suppression`.
    let mut run = |hue: f64, spill: f64| {
        let t = Transform {
            effects: vec![Effect {
                shader: "chroma_key".into(),
                params: [
                    ("key_hue".to_owned(), hue),
                    ("spill_suppression".to_owned(), spill),
                ]
                .into(),
                enabled: true,
            }],
            ..Default::default()
        };
        let layers = [(&fg, t), (&red, Transform::default())];
        let tex = comp.render(&device, &queue, (32, 8), 1, [0, 0, 0, 255], &layers);
        let rgba = read_back(&device, &queue, tex);
        [4, 12, 20, 28].map(|x| {
            let i = (4 * 32 + x) * 4;
            [rgba[i], rgba[i + 1], rgba[i + 2]].map(i32::from)
        })
    };
    let same = |a: [i32; 3], b: [i32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() <= 1);
    let green = run(120.0, 0.0);
    assert!(same(green[0], [255, 0, 0]), "green keyed: {:?}", green[0]);
    assert!(
        same(green[1], [220, 170, 140]),
        "foreground kept: {:?}",
        green[1]
    );
    assert!(same(green[2], [150, 190, 140]), "edge kept: {:?}", green[2]);
    assert!(
        same(green[3], [30, 60, 210]),
        "blue not keyed: {:?}",
        green[3]
    );
    let blue = run(240.0, 0.0);
    assert!(same(blue[3], [255, 0, 0]), "blue keyed: {:?}", blue[3]);
    assert!(
        same(blue[0], [40, 200, 60]),
        "green not keyed: {:?}",
        blue[0]
    );
    // Green cast: how far green leads the other channels on the edge pixel.
    let cast = |[r, g, b]: [i32; 3]| g - r.max(b);
    let full = run(120.0, 1.0);
    assert_eq!(cast(green[2]), 40);
    assert!(cast(full[2]).abs() <= 1, "spill removed: {:?}", full[2]);
    assert!(cast(run(120.0, 0.5)[2]) <= 21, "half spill");
    assert!(same(full[1], [220, 170, 140]), "no cast, no change");
}

#[test]
fn a_feathered_ellipse_mask_fades_the_clip_out_across_its_outline() {
    use jvid_core::model::{Mask, MaskShape};
    let Some((device, queue)) = device() else {
        eprintln!("skipping mask test: no software adapter on this platform");
        return;
    };
    // A 4:1 frame filled by an opaque red clip, masked to an ellipse 64 wide and 16 tall at
    // the centre with an 8 px feather, over a transparent background: alpha is coverage.
    let red = image(&device, &queue, (32, 8), |_, _| [255, 0, 0, 255]);
    let ellipse = Mask {
        feather: 8.0,
        ..Mask::new(MaskShape::Ellipse, vec![[0.0, 0.0], [64.0, 16.0]])
    };
    let masked = |invert| Transform {
        masks: vec![Mask {
            invert,
            ..ellipse.clone()
        }],
        ..Default::default()
    };
    let mut comp = Compositor::new(&device);
    for scale in [1, 2] {
        let layers = [(&red, masked(false))];
        let tex = comp.render(&device, &queue, (128, 32), scale, [0; 4], &layers);
        let w = tex.width();
        let rgba = read_back(&device, &queue, tex);
        let alpha = |x: u32, y: u32| rgba[((y / scale * w + x / scale) * 4 + 3) as usize];
        // Along the middle row from the centre out: opaque, a soft edge about the outline at
        // x = 96, then nothing.
        let row: Vec<u8> = (64..128)
            .step_by(scale as usize)
            .map(|x| alpha(x, 16))
            .collect();
        assert!(row.windows(2).all(|p| p[1] <= p[0]), "falls off: {row:?}");
        assert_eq!(alpha(64, 16), 255, "inside");
        assert_eq!(alpha(90, 16), 255, "inside the feather");
        assert!(
            (64..192).contains(&alpha(96, 16)),
            "soft at the outline: {row:?}"
        );
        assert_eq!(alpha(102, 16), 0, "outside the feather");
        assert_eq!((alpha(2, 16), alpha(64, 0)), (0, 0), "outside");
        // Shown pixels keep their color, premultiplied by coverage.
        let i = ((16 / scale * w + 96 / scale) * 4) as usize;
        assert!(near(rgba[i], rgba[i + 3] as i32) && rgba[i + 1] == 0);
        // Inverted, the ellipse is the hole.
        let layers = [(&red, masked(true))];
        let tex = comp.render(&device, &queue, (128, 32), scale, [0; 4], &layers);
        let rgba = read_back(&device, &queue, tex);
        let alpha = |x: u32, y: u32| rgba[((y / scale * w + x / scale) * 4 + 3) as usize];
        assert_eq!((alpha(64, 16), alpha(2, 16)), (0, 255), "inverted");
    }
}

#[test]
fn a_path_subtracts_from_a_rectangle() {
    use jvid_core::model::{Mask, MaskMode, MaskShape};
    let Some((device, queue)) = device() else {
        eprintln!("skipping mask test: no software adapter on this platform");
        return;
    };
    let red = image(&device, &queue, (32, 8), |_, _| [255, 0, 0, 255]);
    // The left half, less a straight-sided square inside it at x 16..48, y 8..24.
    let rect = Mask::new(MaskShape::Rectangle, vec![[-32.0, 0.0], [64.0, 32.0]]);
    let square = [[-48.0, -8.0], [-16.0, -8.0], [-16.0, 8.0], [-48.0, 8.0]];
    let path = square.iter().flat_map(|v| [*v, [0.0; 2], [0.0; 2]]);
    let hole = Mask {
        mode: MaskMode::Subtract,
        ..Mask::new(MaskShape::Bezier, path.collect())
    };
    let x = Transform {
        masks: vec![rect, hole],
        ..Default::default()
    };
    let mut comp = Compositor::new(&device);
    let tex = comp.render(&device, &queue, (128, 32), 1, [0; 4], &[(&red, x)]);
    let rgba = read_back(&device, &queue, tex);
    let alpha = |x: usize, y: usize| rgba[(y * 128 + x) * 4 + 3];
    let got = [alpha(8, 16), alpha(32, 2), alpha(32, 16), alpha(100, 16)];
    assert_eq!(got, [255, 255, 0, 0]);
}

#[test]
fn masked_blur_and_pixelate_change_only_inside_the_mask() {
    use jvid_core::model::{Mask, MaskShape};
    let Some((device, queue)) = device() else {
        eprintln!("skipping effect test: no software adapter on this platform");
        return;
    };
    // A 4 px red checkerboard over a green ramp, so blur and pixelation both show.
    let (w, h) = (64u32, 32u32);
    let px = |x: u32, y: u32| [((x / 4 + y / 4) % 2 * 255) as u8, (x * 4) as u8, 0, 255];
    let src = image(&device, &queue, (w, h), px);
    let mut comp = Compositor::new(&device);
    let plain = read_back(
        &device,
        &queue,
        comp.render(
            &device,
            &queue,
            (w, h),
            1,
            [0, 0, 0, 255],
            &[(&src, op(1.0))],
        ),
    );
    let at = |rgba: &[u8], x: u32, y: u32| rgba[((y * w + x) * 4) as usize..][..4].to_vec();
    for (file, params) in [("gaussian_blur.wgsl", 6.0), ("pixelate.wgsl", 8.0)] {
        let (_, text) = crate::effect::BUNDLED
            .iter()
            .find(|(f, _)| *f == file)
            .unwrap();
        let shader = crate::effect::parse(text).unwrap();
        comp.set_effect(&device, file, Some(&shader)).unwrap();
        // The clip again on top, masked to x 16..48, y 8..24, with the effect: a hidden region.
        let top = Transform {
            masks: vec![Mask::new(
                MaskShape::Rectangle,
                vec![[0.0, 0.0], [32.0, 16.0]],
            )],
            effects: vec![Effect {
                shader: file.into(),
                params: [(shader.params[0].name.clone(), params)].into(),
                enabled: true,
            }],
            ..Default::default()
        };
        let layers = [(&src, top), (&src, op(1.0))];
        let tex = comp.render(&device, &queue, (w, h), 1, [0, 0, 0, 255], &layers);
        let out = read_back(&device, &queue, tex);
        for (x, y) in (0..w).flat_map(|x| (0..h).map(move |y| (x, y))) {
            // One pixel of antialiasing either side of the mask's edge.
            let outside = !(15..49).contains(&x) || !(7..25).contains(&y);
            if outside {
                assert_eq!(at(&out, x, y), at(&plain, x, y), "{file} at ({x}, {y})");
            }
        }
        if file == "gaussian_blur.wgsl" {
            // The checkerboard averages out to mid red.
            for (x, y) in (18..46).flat_map(|x| (10..22).map(move |y| (x, y))) {
                let r = at(&out, x, y)[0] as i32;
                assert!((r - 128).abs() < 40, "blurred at ({x}, {y}): {r}");
            }
        } else {
            // Each 8 px block clear of the mask's edge is one colour.
            for (x, y) in (24..40).flat_map(|x| (16..24).map(move |y| (x, y))) {
                let corner = at(&out, x / 8 * 8, y / 8 * 8);
                assert_eq!(at(&out, x, y), corner, "pixelated at ({x}, {y})");
            }
        }
    }
}
