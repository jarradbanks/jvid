use jvid_core::hdr::{BT709_TO_BT2020, BT2020_TO_BT709, Hdr, nits_to_pq, pq_to_nits};
use jvid_core::time::Time;
use jvid_media::decode::VideoDecoder;
use jvid_media::test_media::corpus;
use suite_ui::test_support::gpu;

use super::*;
use crate::composite::read_back;
use crate::composite::tests::device;

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

#[test]
fn corpus_colour_bars_match_reference_rgb() {
    let Some(dir) = corpus() else { return };
    if !gpu() {
        return;
    }
    jvid_media::ffmpeg::init().unwrap();
    let (device, queue) = device().unwrap();
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
            hdr: None,
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

/// End to end: a decoded corpus frame through the preview upload, the compositor and read-back.
#[test]
fn corpus_bars_survive_compositing() {
    let Some(dir) = corpus() else { return };
    let Some((device, queue)) = device() else {
        eprintln!("skipping compositor end-to-end test: no software adapter on this platform");
        return;
    };
    jvid_media::ffmpeg::init().unwrap();
    let f = VideoDecoder::open(&dir.join("bars-bt709-limited.mp4"))
        .unwrap()
        .frame_at(Time(0))
        .unwrap();
    let mut preview = Preview::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    preview.upload(
        &device,
        &queue,
        &Yuv {
            width: f.width,
            height: f.height,
            planes: [&f.planes[0], &f.planes[1], &f.planes[2]],
            bt709: f.bt709,
            full_range: f.full_range,
            hdr: None,
        },
        1,
    );
    let mut comp = crate::composite::Compositor::new(&device);
    let layers = [(preview.image().unwrap(), Default::default())];
    let tex = comp.render(&device, &queue, (640, 360), 1, [0, 0, 0, 255], &layers);
    let rgba = read_back(&device, &queue, tex);
    for (i, want) in BARS.iter().enumerate() {
        let px = (180 * 640 + 80 * i + 40) * 4;
        let got = &rgba[px..px + 3];
        for c in 0..3 {
            assert!(
                (got[c] as i32 - want[c] as i32).abs() <= TOLERANCE,
                "bar {i} is {got:?}, want {want:?}"
            );
        }
    }
}

/// `eetf` from convert.wgsl on a PQ signal: BT.2390 EETF from 1000 to 203 cd/m².
fn eetf(pq: f32) -> f32 {
    let peak = nits_to_pq(1000.0);
    let max_lum = nits_to_pq(203.0) / peak;
    let knee = 1.5 * max_lum - 0.5;
    let e = (pq / peak).min(1.0);
    let t = (e - knee) / (1.0 - knee);
    let (t2, t3) = (t * t, t * t * t);
    let e = if e < knee {
        e
    } else {
        (2.0 * t3 - 3.0 * t2 + 1.0) * knee
            + (t3 - 2.0 * t2 + t) * (1.0 - knee)
            + (-2.0 * t3 + 3.0 * t2) * max_lum
    };
    e * peak
}

/// `fs_pq` from convert.wgsl for a PQ BT.2020 R'G'B' signal: the EETF per channel, then linear
/// BT.2020 to BT.709, clipped, then gamma 2.4.
fn tone_mapped_rgb(pq: [f32; 3]) -> [f32; 3] {
    let linear = pq.map(|e| pq_to_nits(eetf(e)) / 203.0);
    BT2020_TO_BT709.map(|row| {
        let v: f32 = row.iter().zip(linear).map(|(m, c)| m * c).sum();
        v.clamp(0.0, 1.0).powf(1.0 / 2.4)
    })
}

/// [`tone_mapped_rgb`] for a neutral grey of `nits`.
fn tone_mapped(nits: f32) -> f32 {
    tone_mapped_rgb([nits_to_pq(nits); 3])[0]
}

/// Uploads the first frame of a generated `hdr` clip of `patches` and reads back the centre of
/// each patch in 8-bit codes.
fn render_hdr_clip(hdr: Hdr, patches: &[[f32; 3]]) -> Vec<[u8; 3]> {
    let path =
        std::env::temp_dir().join(format!("jvid-preview-{hdr:?}-{}.mkv", std::process::id()));
    jvid_media::test_media::hdr_clip(&path, hdr, patches).unwrap();
    let f = VideoDecoder::open_hdr(&path)
        .unwrap()
        .frame_at(Time(0))
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(f.hdr, Some(hdr));
    let (device, queue) = device().unwrap();
    let mut preview = Preview::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    let yuv = Yuv {
        width: f.width,
        height: f.height,
        planes: [&f.planes[0], &f.planes[1], &f.planes[2]],
        bt709: f.bt709,
        full_range: f.full_range,
        hdr: f.hdr,
    };
    preview.upload(&device, &queue, &yuv, 1);
    let tex = preview.image().unwrap();
    let rgba = read_back(&device, &queue, tex);
    (0..patches.len())
        .map(|i| {
            let px = (8 * tex.width() as usize + 16 * i + 8) * 4;
            [rgba[px], rgba[px + 1], rgba[px + 2]]
        })
        .collect()
}

fn assert_near(got: [u8; 3], want: [f32; 3], what: &str) {
    let want = want.map(|c| (c * 255.0).round() as i32);
    assert!(
        got.iter()
            .zip(want)
            .all(|(g, w)| (*g as i32 - w).abs() <= TOLERANCE),
        "{what}: {got:?}, want {want:?}"
    );
}

#[test]
fn tone_map_keeps_mid_grey_and_rolls_off_highlights() {
    // BT.2408 puts an 18% grey card at 26 cd/m²; SDR shows it at about 42%.
    assert!(
        (tone_mapped(26.0) - 0.425).abs() < 0.01,
        "{}",
        tone_mapped(26.0)
    );
    assert_eq!(tone_mapped(1000.0), 1.0);
    assert_eq!(tone_mapped(10_000.0), 1.0);
    let curve: Vec<f32> = (0..=1000).map(|n| tone_mapped(n as f32)).collect();
    assert!(
        curve.windows(2).all(|w| w[0] <= w[1] && w[1] <= 1.0),
        "not monotonic"
    );
    // Highlights roll off below the knee's straight line instead of clipping at 203 cd/m².
    assert!(tone_mapped(203.0) < 0.95 && tone_mapped(500.0) < 1.0);
}

/// The tone-map check from #86: a PQ clip previews inside SDR range with mid-grey near mid-grey.
#[test]
fn pq_clip_previews_tone_mapped() {
    if !gpu() {
        return;
    }
    jvid_media::ffmpeg::init().unwrap();
    let nits = [26.0, 203.0, 1000.0, 4000.0];
    // Linear BT.709 (0.15, 0.5, 0.08) of 203 cd/m², a saturated green, carried in BT.2020: the
    // grey patches alone would pass with the gamut matrix wrong or missing (red would read
    // 146, not 116, without it).
    let green = BT709_TO_BT2020.map(|row| {
        let v: f32 = row.iter().zip([0.15, 0.5, 0.08]).map(|(m, c)| m * c).sum();
        nits_to_pq(v * 203.0)
    });
    let mut patches: Vec<_> = nits.iter().map(|n| [nits_to_pq(*n); 3]).collect();
    patches.push(green);
    let got = render_hdr_clip(Hdr::Pq, &patches);
    for (g, p) in got.iter().zip(&patches) {
        assert_near(*g, tone_mapped_rgb(*p), &format!("PQ {p:?}"));
    }
}

/// HLG's 75% signal is the BT.2408 reference white, so it previews like 203 cd/m² PQ.
#[test]
fn hlg_clip_previews_tone_mapped() {
    if !gpu() {
        return;
    }
    jvid_media::ffmpeg::init().unwrap();
    let got = render_hdr_clip(Hdr::Hlg, &[[0.75; 3]]);
    assert_near(got[0], [tone_mapped(203.0); 3], "HLG 75%");
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
