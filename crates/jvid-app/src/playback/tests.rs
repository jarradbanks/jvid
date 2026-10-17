use std::time::{Duration, Instant};

use jvid_audio::pan_gains;
use jvid_core::history::{Command, Created, History};
use jvid_core::model::NewClip;

use super::*;
use crate::transport::tests::Fake;

const FPS30: Rate = Rate::new(30, 1);

fn secs(s: i64) -> Time {
    Time(s * FLICKS_PER_SECOND)
}

/// Adds `path` as an asset and a new track holding all `len` of it from time zero.
fn add(h: &mut History, kind: TrackKind, path: &str, len: Time) -> ClipId {
    let Ok(Created::Asset(asset)) = h.apply(Command::AddAsset(path.into())) else {
        panic!("asset")
    };
    let name = path.into();
    let Ok(Created::Track(track)) = h.apply(Command::AddTrack { kind, name }) else {
        panic!("track")
    };
    let clip = NewClip {
        asset,
        source_in: Time(0),
        source_out: len,
        start: Time(0),
        link: None,
    };
    let Ok(Created::Clip(c)) = h.apply(Command::AddClip { track, clip }) else {
        panic!("clip")
    };
    c
}

/// A 2x2 frame stamped with the time it was asked for.
fn tiny(t: Time) -> Frame {
    Frame {
        pts: t,
        width: 2,
        height: 2,
        planes: [vec![0; 4], vec![0], vec![0]],
        bt709: true,
        full_range: false,
    }
}

fn silent() -> OpenAudio {
    Box::new(|_| Box::new(|_, _| {}))
}

#[test]
fn cache_evicts_least_recently_used_over_budget() {
    let frame = |n| {
        Some(Arc::new(Frame {
            planes: [vec![0; n], Vec::new(), Vec::new()],
            ..tiny(Time(0))
        }))
    };
    let key = |i| (AssetId(1), Time(i));
    let mut c = FrameCache::new(300);
    for i in 0..3 {
        c.insert(key(i), frame(100));
    }
    assert_eq!(c.bytes(), 300);
    c.get(&key(0));
    c.insert(key(3), frame(100));
    assert_eq!(c.bytes(), 300);
    assert!(!c.contains(&key(1)), "least recently used goes first");
    assert!(c.contains(&key(0)) && c.contains(&key(2)) && c.contains(&key(3)));
    c.insert(key(4), None);
    assert!(c.contains(&key(4)), "failed decodes are kept, at no cost");
    assert!(matches!(c.get(&key(4)), Some(None)));
    c.insert(key(5), frame(500));
    assert!(c.contains(&key(5)), "a frame over budget stays, alone");
    assert_eq!(c.bytes(), 500);
}

/// Decodes until the frame at `clock` is shown, as a fast decoder would have it by then.
fn catch_up(pb: &mut Playback, p: &Project, clock: u64, last: &mut Option<Shown>) {
    for _ in 0..10_000 {
        pb.decode_ahead(p, clock);
        if let Some(s) = pb.show(p, clock) {
            *last = Some(s);
        }
        if pb.shown == Some(pb.transport.frame(clock)) {
            return;
        }
        std::thread::sleep(Duration::from_micros(100));
    }
    panic!("frame at {clock} never decoded");
}

/// Five minutes of a video clip over an audio clip, through the mixer, where every audio sample
/// holds its own source index + 1 and every frame its source time. After each device callback
/// the frame shown is compared with the last sample heard; they must stay within one frame
/// throughout, across pauses, seeks, shuttle and uneven UI/device timing, with the frame cache
/// held to a small budget.
#[test]
fn video_stays_within_one_frame_of_audio_over_five_minutes() {
    let mut h = History::new(Project::default());
    add(&mut h, TrackKind::Video, "v.mp4", secs(300));
    add(&mut h, TrackKind::Audio, "a.m4a", secs(300));
    let p = h.project();
    let budget = 64 * size(&tiny(Time(0)));
    let mut pb = Playback::new(
        Arc::new(|_, t| Ok(tiny(t))),
        Box::new(|_| {
            Box::new(|start, out| {
                for (i, f) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                    f.fill((start + i as i64 + 1) as f32);
                }
            })
        }),
        budget,
    );
    let fake = Fake::default();
    let mut out = Output::new(Box::new(fake.clone()), None);
    let gain = pan_gains(0.0).0;
    let frame = FPS30.time_floor(1).0 as f64 / FLICKS_PER_SECOND as f64;
    let (mut checked, mut worst) = (0, 0.0f64);
    let mut last = pb.tick(p, &mut out);
    pb.transport.toggle(out.clock());
    for step in 0u64.. {
        // UI frames at ~60Hz with jitter; device callbacks of 480 or 512 frames.
        if step % 3 != 2
            && let Some(s) = pb.tick(p, &mut out)
        {
            last = Some(s);
        }
        let pulled = fake.pull(if step % 2 == 0 { 480 } else { 512 });
        if matches!(step, 3_000 | 3_100 | 6_000 | 6_200 | 6_300 | 6_301) {
            out.flush();
        }
        let t = &mut pb.transport;
        match step {
            3_000 => t.stop(out.clock()),
            3_100 => t.toggle(out.clock()),
            6_000 => t.shuttle(1, out.clock()),
            6_200 => t.shuttle(-1, out.clock()),
            6_300 => t.seek(secs(120), out.clock()),
            6_301 => t.shuttle(1, out.clock()),
            _ => {}
        }
        if t.speed() == 0 && t.position(out.clock()) >= t.duration {
            break;
        }
        catch_up(&mut pb, p, out.clock(), &mut last);
        assert!(pb.cache.bytes() <= budget);
        // Only 1x audio carries source positions; the rest is silence or underrun.
        let heard = pulled.last().copied().unwrap_or(0.0);
        if pb.transport.speed() != 1 || heard == 0.0 {
            continue;
        }
        let audio_secs = ((heard / gain) as f64 - 1.0) / SAMPLE_RATE as f64;
        let shown = &last.as_ref().unwrap().layers[0].0;
        let video_secs = shown.pts.0 as f64 / FLICKS_PER_SECOND as f64;
        let drift = (video_secs - audio_secs).abs();
        worst = worst.max(drift);
        assert!(drift <= frame, "drift {drift}s at step {step}");
        checked += 1;
    }
    assert!(checked > 20_000, "only {checked} checks");
    assert!(worst <= frame, "{worst}");
}

/// With a decoder too slow for 30fps, playback keeps to the clock: every frame shown is the one
/// for the clock's time, frames are skipped rather than shown late, and it keeps up to the end.
#[test]
fn slow_decoder_drops_frames_instead_of_drifting() {
    let mut h = History::new(Project::default());
    add(&mut h, TrackKind::Video, "v.mp4", secs(10));
    let p = h.project();
    // 4 decode threads at 200ms a frame: 20fps.
    let decode: DecodeFn = Arc::new(|_, t| {
        std::thread::sleep(Duration::from_millis(200));
        Ok(tiny(t))
    });
    let mut pb = Playback::new(decode, silent(), DEFAULT_BUDGET);
    let fake = Fake::default();
    let mut out = Output::new(Box::new(fake.clone()), None);
    pb.tick(p, &mut out);
    pb.transport.toggle(out.clock());
    let mut shown = Vec::new();
    let start = Instant::now();
    while out.clock() < 3 * SAMPLE_RATE as u64 {
        if let Some(s) = pb.tick(p, &mut out) {
            assert_eq!(s.index, pb.transport.frame(out.clock()), "shown on time");
            assert_eq!(s.layers[0].0.pts, FPS30.time_floor(s.index), "right frame");
            shown.push(s.index);
        }
        fake.pull(800);
        // Hold the fake device to wall time, so the decoder is slow relative to it.
        let due = Duration::from_secs_f64(out.clock() as f64 / SAMPLE_RATE as f64);
        std::thread::sleep(due.saturating_sub(start.elapsed()));
    }
    assert!(shown.windows(2).all(|w| w[0] < w[1]), "{shown:?}");
    let skips = shown.windows(2).filter(|w| w[1] - w[0] > 1).count();
    assert!(skips > 0, "nothing dropped: {shown:?}");
    assert!(shown.len() > 20, "too few shown: {shown:?}");
    assert!(*shown.last().unwrap() >= 80, "fell behind: {shown:?}");
    eprintln!("{} of 90 frames shown: {shown:?}", shown.len());
}

/// Release only (debug FFmpeg and GPU code is too slow to say anything): ten seconds of two
/// stacked 1080p30 layers, decoded through the engine in under ten seconds and composited at
/// half-size preview scale. Only the decode side is gated: CI composites on lavapipe, a software
/// rasterizer at ~30ms a frame, which says nothing about a real GPU, so its time is reported.
/// shortcut: gate compositing too once a perf harness (#42) runs on real GPU hardware.
#[test]
fn two_layer_1080p30_plays_faster_than_real_time() {
    if cfg!(debug_assertions) {
        eprintln!("skipping throughput test in a debug build; run with --release");
        return;
    }
    let Some(dir) = jvid_media::test_media::corpus() else {
        return;
    };
    let Some((device, queue)) = device() else {
        return;
    };
    jvid_media::ffmpeg::init().unwrap();
    let path = dir.join("frame-index-1080p30.mp4");
    let path = path.to_str().unwrap();
    let mut h = History::new(Project::default());
    add(&mut h, TrackKind::Video, path, secs(10));
    let top = add(&mut h, TrackKind::Video, path, secs(10));
    h.apply(Command::SetClipParam {
        id: top,
        key: "opacity".into(),
        value: Some(0.5),
    })
    .unwrap();
    let p = h.project();
    let mut gpu = Gpu::new(&device);
    // Plays the timeline through a fresh engine, frame by frame as fast as it decodes; `draw`
    // composites each one too.
    let mut play = |draw: bool| {
        let mut pb = Playback::from_files(DEFAULT_BUDGET);
        pb.sync(p);
        let start = Instant::now();
        for k in 0..300 {
            pb.transport.seek(FPS30.time_floor(k), 0);
            let s = loop {
                pb.decode_ahead(p, 0);
                if let Some(s) = pb.show(p, 0) {
                    break s;
                }
                std::thread::sleep(Duration::from_millis(1));
            };
            assert_eq!(s.layers.len(), 2);
            for (f, _) in &s.layers {
                let index = jvid_media::test_media::frame_index(&f.planes[0], f.width as usize);
                assert_eq!(index, k as u32);
            }
            if draw {
                gpu.draw(&device, &queue, p, 2, &s);
            }
        }
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        start.elapsed()
    };
    let decoding = play(false);
    let composited = play(true);
    eprintln!("10s of two-layer 1080p30: decoded in {decoding:?}, with compositing {composited:?}");
    assert!(decoding < Duration::from_secs(10), "{decoding:?}");
}

/// wgpu's native futures are ready when polled, so a busy poll stands in for an executor.
fn block_on<F: std::future::Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

/// A software device where `gpu()` allows one (lavapipe on Linux); `None` skips.
fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    if !suite_ui::test_support::gpu() {
        return None;
    }
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = block_on(instance.request_adapter(&Default::default())).expect("adapter");
    Some(block_on(adapter.request_device(&Default::default())).expect("device"))
}
