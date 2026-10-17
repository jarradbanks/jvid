use std::future::Future;

use jvid_core::time::Time;
use jvid_media::decode::VideoDecoder;
use jvid_media::test_media::corpus;
use suite_ui::test_support::gpu;

use super::*;

/// The corpus bars: 75% white, yellow, cyan, green, magenta, red, blue, black, 80px each.
const BARS: [[u8; 3]; 8] = [
    [191, 191, 191],
    [191, 191, 0],
    [0, 191, 191],
    [0, 191, 0],
    [191, 0, 191],
    [191, 0, 0],
    [0, 0, 191],
    [0, 0, 0],
];
/// In 8-bit codes: limited-range rounding and 4:2:0 chroma cost a level or two.
const TOLERANCE: i32 = 3;

/// wgpu's native futures are ready when polled, so a busy poll stands in for an executor.
fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = block_on(instance.request_adapter(&Default::default())).expect("adapter");
    block_on(adapter.request_device(&Default::default())).expect("device")
}

fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture) -> Vec<u8> {
    let row = (tex.width() * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * tex.height()) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        tex.size(),
    );
    queue.submit([encoder.finish()]);
    buf.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .unwrap();
    let data = buf.slice(..).get_mapped_range().unwrap();
    data.chunks(row as usize)
        .flat_map(|r| &r[..tex.width() as usize * 4])
        .copied()
        .collect()
}

#[test]
fn corpus_colour_bars_match_reference_rgb() {
    let Some(dir) = corpus() else { return };
    if !gpu() {
        return;
    }
    jvid_media::ffmpeg::init().unwrap();
    let (device, queue) = device();
    let mut preview = Preview::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    for file in [
        "bars-bt601-limited.mp4",
        "bars-bt601-full.mp4",
        "bars-bt709-limited.mp4",
        "bars-bt709-full.mp4",
    ] {
        let f = VideoDecoder::open(&dir.join(file))
            .unwrap()
            .frame_at(Time(0))
            .unwrap();
        let yuv = Yuv {
            width: f.width,
            height: f.height,
            planes: [&f.planes[0], &f.planes[1], &f.planes[2]],
            bt709: f.bt709,
            full_range: f.full_range,
        };
        for scale in [1, 2, 4] {
            preview.upload(&device, &queue, &yuv, scale);
            let tex = preview.image().unwrap();
            assert_eq!((tex.width(), tex.height()), scaled(640, 360, scale));
            let rgba = read_back(&device, &queue, tex);
            for (i, want) in BARS.iter().enumerate() {
                // Centre of bar i, halfway down.
                let (x, y) = ((80 * i as u32 + 40) / scale, 180 / scale);
                let px = (y * tex.width() + x) as usize * 4;
                let got = &rgba[px..px + 3];
                for c in 0..3 {
                    assert!(
                        (got[c] as i32 - want[c] as i32).abs() <= TOLERANCE,
                        "{file} at 1/{scale}: bar {i} is {got:?}, want {want:?}"
                    );
                }
            }
        }
    }
}

/// The same bars through the matrix on the CPU: the WGSL is a straight dot product of these rows.
#[test]
fn matrix_inverts_ffmpeg_encode() {
    // (Y, U, V) codes of 75% yellow, as FFmpeg wrote the corpus (BT.601 limited and BT.709 full).
    for (bt709, full, yuv) in [(false, false, [161, 44, 141]), (true, true, [177, 33, 137])] {
        let rows = yuv_to_rgb(bt709, full);
        let rgb = rows.map(|r| {
            let v = r[0] * yuv[0] as f32 + r[1] * yuv[1] as f32 + r[2] * yuv[2] as f32;
            (v + r[3] * 255.0).round() as i32
        });
        for (got, want) in rgb.iter().zip([191, 191, 0]) {
            assert!((got - want).abs() <= 2, "{rgb:?}");
        }
    }
}

#[test]
fn fit_letterboxes_and_pillarboxes() {
    // 16:9 in a tall view: full width, centred vertically.
    assert_eq!(
        place([1920.0, 1080.0], [0.0, 0.0, 400.0, 400.0], None),
        [0.0, 87.5, 400.0, 225.0]
    );
    // 9:16 in a wide view: full height, centred horizontally, offset by the view origin.
    assert_eq!(
        place([1080.0, 1920.0], [10.0, 20.0, 800.0, 320.0], None),
        [320.0, 20.0, 180.0, 320.0]
    );
}

#[test]
fn zoom_centres_and_can_overflow() {
    assert_eq!(
        place([100.0, 50.0], [0.0, 0.0, 300.0, 300.0], Some(1.0)),
        [100.0, 125.0, 100.0, 50.0]
    );
    // 200% of a frame bigger than the view spills past every edge, still centred.
    assert_eq!(
        place([200.0, 200.0], [0.0, 0.0, 100.0, 100.0], Some(2.0)),
        [-150.0, -150.0, 400.0, 400.0]
    );
}

#[test]
fn clip_space_maps_view_corners() {
    let view = [10.0, 20.0, 100.0, 50.0];
    assert_eq!(to_clip(view, view), [-1.0, 1.0, 1.0, -1.0]);
    assert_eq!(
        to_clip([35.0, 32.5, 50.0, 25.0], view),
        [-0.5, 0.5, 0.5, -0.5]
    );
}

#[test]
fn scale_rounds_up_and_never_hits_zero() {
    assert_eq!(scaled(1920, 1080, 4), (480, 270));
    assert_eq!(scaled(1081, 1, 2), (541, 1));
    assert_eq!(scaled(3, 3, 4), (1, 1));
}
