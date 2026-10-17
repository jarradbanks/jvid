use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;

use jvid_core::model::{AssetId, ClipId, TrackId};
use jvid_core::time::Time;
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
    px: impl Fn(u32) -> [u8; 4],
) -> wgpu::Texture {
    let data: Vec<u8> = (0..w * h).flat_map(|i| px(i % w)).collect();
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
    let base = image(&device, &queue, (128, 64), |x| {
        if x < 64 {
            [255, 255, 0, 255]
        } else {
            [0, 0, 255, 255]
        }
    });
    let green = image(&device, &queue, (64, 16), |_| [0, 255, 0, 255]);
    let red = image(&device, &queue, (32, 32), |_| [255, 0, 0, 255]);
    let white = image(&device, &queue, (8, 8), |_| [255; 4]);
    let layers = [(&white, 0.0), (&red, 0.5), (&green, 1.0), (&base, 1.0)];
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
    let base = image(&device, &queue, (48, 20), |x| [x as u8 * 5, 40, 200, 255]);
    let red = image(&device, &queue, (16, 16), |_| [255, 0, 0, 255]);
    let layers = [(&red, 0.4), (&base, 1.0)];
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

#[test]
fn opacity_defaults_to_one_and_clamps() {
    let of = |o: Option<f64>| {
        let params: BTreeMap<_, _> = o.map(|o| ("opacity".to_string(), o)).into_iter().collect();
        opacity(&Layer {
            track: TrackId(0),
            clip: ClipId(0),
            asset: AssetId(0),
            source_time: Time(0),
            params: &params,
        })
    };
    assert_eq!(of(None), 1.0);
    assert_eq!(of(Some(0.25)), 0.25);
    assert_eq!(of(Some(2.0)), 1.0);
    assert_eq!(of(Some(-1.0)), 0.0);
}
