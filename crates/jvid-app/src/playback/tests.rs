use std::sync::Mutex;
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
    add_at(h, kind, path, Time(0), len)
}

/// Like [`add`], with the clip starting at `start`.
fn add_at(h: &mut History, kind: TrackKind, path: &str, start: Time, len: Time) -> ClipId {
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
        start,
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
        Arc::new(Frame {
            planes: [vec![0; n], Vec::new(), Vec::new()],
            ..tiny(Time(0))
        })
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
    let mut pb = Playback::new(Arc::new(|_, t| Ok(tiny(t))), indexed(), budget);
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

/// A frame `tick` returned during [`play`]: its index, its layers' source times, and the first
/// sample the device played after it.
struct Seen {
    index: i64,
    pts: Vec<Time>,
    heard: f32,
}

/// Plays `pb` from its playhead as the app would: first parked until the frame there is shown and
/// decode-ahead is idle, then playing until the clock reaches `until` samples or play stops, with
/// one `tick` per 800-sample device callback (~60Hz) and the fake device held to wall time, so
/// decoding has to keep up for real. Every frame shown is the one for the clock, and goes to `each`.
fn play(pb: &mut Playback, p: &Project, until: u64, mut each: impl FnMut(&Shown)) -> Vec<Seen> {
    let fake = Fake::default();
    let mut out = Output::new(Box::new(fake.clone()), None);
    let mut seen = Vec::new();
    let mut primed = false;
    while !primed || !pb.pending.is_empty() {
        primed |= pb.tick(p, &mut out).is_some();
        std::thread::sleep(Duration::from_millis(1));
    }
    pb.transport.toggle(out.clock());
    let start = Instant::now();
    while out.clock() < until && pb.transport.speed() != 0 {
        let shown = pb.tick(p, &mut out);
        if let Some(s) = &shown {
            assert_eq!(s.index, pb.transport.frame(out.clock()), "shown on time");
            each(s);
        }
        let heard = fake.pull(800);
        if let Some(s) = shown {
            let pts = s.layers.iter().map(|(f, _)| f.pts).collect();
            seen.push(Seen {
                index: s.index,
                pts,
                heard: heard[0],
            });
        }
        let due = Duration::from_secs_f64(out.clock() as f64 / SAMPLE_RATE as f64);
        std::thread::sleep(due.saturating_sub(start.elapsed()));
    }
    seen
}

/// Audio whose every sample holds its own source index + 1.
fn indexed() -> OpenAudio {
    Box::new(|_| {
        Box::new(|start, out| {
            for (i, f) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                f.fill((start + i as i64 + 1) as f32);
            }
        })
    })
}

/// With a decoder too slow for 30fps, so frames really arrive late, playback keeps to the audio
/// clock: every frame shown is the one for the clock's time and within a frame of the audio heard
/// with it, frames are skipped rather than shown late, and it keeps up to the end.
#[test]
fn slow_decoder_drops_frames_instead_of_drifting() {
    let mut h = History::new(Project::default());
    add(&mut h, TrackKind::Video, "v.mp4", secs(10));
    add(&mut h, TrackKind::Audio, "a.m4a", secs(10));
    let p = h.project();
    // 4 decode threads at 200ms a frame: 20fps.
    let decode: DecodeFn = Arc::new(|_, t| {
        std::thread::sleep(Duration::from_millis(200));
        Ok(tiny(t))
    });
    let mut pb = Playback::new(decode, indexed(), DEFAULT_BUDGET);
    let seen = play(&mut pb, p, 3 * SAMPLE_RATE as u64, |_| {});
    let gain = pan_gains(0.0).0;
    let frame = FPS30.time_floor(1).0 as f64 / FLICKS_PER_SECOND as f64;
    let mut checked = 0;
    for s in &seen {
        assert_eq!(s.pts, [FPS30.time_floor(s.index)], "right frame");
        if s.heard != 0.0 {
            let audio = ((s.heard / gain) as f64 - 1.0) / SAMPLE_RATE as f64;
            let video = s.pts[0].0 as f64 / FLICKS_PER_SECOND as f64;
            assert!((video - audio).abs() <= frame, "{video}s shown at {audio}s");
            checked += 1;
        }
    }
    let shown: Vec<_> = seen.iter().map(|s| s.index).collect();
    assert!(checked > 20, "only {checked} checked");
    assert!(shown.windows(2).all(|w| w[0] < w[1]), "{shown:?}");
    let skips = shown.windows(2).filter(|w| w[1] - w[0] > 1).count();
    assert!(skips > 0, "nothing dropped: {shown:?}");
    assert!(shown.len() > 20, "too few shown: {shown:?}");
    assert!(*shown.last().unwrap() >= 80, "fell behind: {shown:?}");
    eprintln!("{} of 90 frames shown: {shown:?}", shown.len());
}

/// A decoder that takes `cold` for the first frame of each file (opening it and seeking to a
/// keyframe) and `warm` for the rest.
fn cold_open(cold: Duration, warm: Duration) -> DecodeFn {
    let opened = Mutex::new(HashSet::new());
    Arc::new(move |path, t| {
        let first = opened.lock().unwrap().insert(path.to_owned());
        std::thread::sleep(if first { cold } else { warm });
        Ok(tiny(t))
    })
}

/// Back-to-back clips of two files, then a gap and a third: each file is slow to open, but
/// decode-ahead opens it before the playhead gets there, so no frame drops at either boundary;
/// the gap shows nothing. The audio clip after the gap is opened and seeked to its in point
/// before it's heard.
#[test]
fn clip_boundaries_play_without_drops() {
    use TrackKind::{Audio, Video};
    let mut h = History::new(Project::default());
    let half = Time(FLICKS_PER_SECOND / 2);
    add_at(&mut h, Video, "a.mp4", Time(0), secs(1));
    add_at(&mut h, Video, "b.mp4", secs(1), secs(1));
    add_at(&mut h, Video, "c.mp4", secs(2) + half, secs(1));
    add_at(&mut h, Audio, "a.wav", Time(0), secs(1));
    add_at(&mut h, Audio, "c.wav", secs(2) + half, secs(1));
    let p = h.project();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let log = calls.clone();
    let open: OpenAudio = Box::new(move |path| {
        let (log, path) = (log.clone(), path.to_owned());
        Box::new(move |start, out| log.lock().unwrap().push((path.clone(), start, out.len())))
    });
    let decode = cold_open(Duration::from_millis(150), Duration::from_millis(5));
    let mut pb = Playback::new(decode, open, DEFAULT_BUDGET);
    let seen = play(&mut pb, p, u64::MAX, |_| {});
    let shown: Vec<_> = seen.iter().map(|s| s.index).collect();
    assert!(shown.windows(2).all(|w| w[1] == w[0] + 1), "{shown:?}");
    assert!(shown[0] == 1 && *shown.last().unwrap() >= 104, "{shown:?}");
    for s in &seen {
        let want = match s.index {
            0..30 => vec![FPS30.time_floor(s.index)],
            30..60 => vec![FPS30.time_floor(s.index - 30)],
            60..75 => vec![],
            75..105 => vec![FPS30.time_floor(s.index - 75)],
            _ => continue,
        };
        assert_eq!(s.pts, want, "frame {}", s.index);
    }
    let calls = calls.lock().unwrap();
    let mut c = calls.iter().filter(|(path, ..)| path.ends_with("c.wav"));
    assert_eq!(
        c.next().map(|(_, s, n)| (*s, *n)),
        Some((0, 0)),
        "seeked first"
    );
    assert!(c.any(|(_, _, n)| *n > 0), "then heard");
}

/// Looping the in/out range plays it over and over with no frame dropped at the wrap: decode-ahead
/// wraps too, so the in point's frames are decoded (again, with the cache too small to hold the
/// loop) before the playhead jumps back.
#[test]
fn loop_wraps_without_dropping_frames() {
    let mut h = History::new(Project::default());
    add(&mut h, TrackKind::Video, "v.mp4", secs(10));
    h.apply(Command::SetInOut {
        in_point: Some(secs(1)),
        out_point: Some(secs(3)),
    })
    .unwrap();
    let p = h.project();
    // About two frames of latency, so a frame queued only after the wrap would be late.
    let decode = cold_open(Duration::from_millis(60), Duration::from_millis(60));
    // Room for the decode-ahead window and what was just shown, not for the whole loop.
    let mut pb = Playback::new(decode, silent(), 40 * size(&tiny(Time(0))));
    pb.looping = true;
    pb.sync(p);
    pb.transport.seek(secs(1), 0);
    let seen = play(&mut pb, p, 6 * SAMPLE_RATE as u64, |_| {});
    let shown: Vec<_> = seen.iter().map(|s| s.index).collect();
    assert_eq!(shown[0], 31, "the in point was shown parked");
    assert!(
        shown
            .windows(2)
            .all(|w| w[1] == w[0] + 1 || (w[0] == 89 && w[1] == 30)),
        "{shown:?}"
    );
    let wraps = shown.windows(2).filter(|w| w[1] < w[0]).count();
    assert!(wraps >= 2, "{wraps} wraps: {shown:?}");
}

/// A failed decode leaves its layer out, then is retried; a relink drops frames decoded from the
/// asset's old path, so the new file is decoded.
#[test]
fn failed_decodes_retry_and_relink_redecodes() {
    let mut h = History::new(Project::default());
    add(&mut h, TrackKind::Video, "old.mp4", secs(1));
    let asset = h.project().assets()[0].id;
    let fail = Arc::new(Mutex::new(true));
    let paths = Arc::new(Mutex::new(Vec::new()));
    let (f, log) = (fail.clone(), paths.clone());
    let decode: DecodeFn = Arc::new(move |path, t| {
        log.lock().unwrap().push(path.to_owned());
        match *f.lock().unwrap() {
            true => Err("not yet".into()),
            false => Ok(tiny(t)),
        }
    });
    let mut pb = Playback::new(decode, silent(), DEFAULT_BUDGET);
    let mut out = Output::new(Box::new(Fake::default()), None);
    // The frame at the playhead once it has `layers` layers.
    let mut until = |pb: &mut Playback, p: &Project, layers: usize| {
        let start = Instant::now();
        while start.elapsed() < 3 * RETRY {
            pb.refresh();
            if pb
                .tick(p, &mut out)
                .is_some_and(|s| s.layers.len() == layers)
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("never shown with {layers} layers");
    };
    until(&mut pb, h.project(), 0);
    *fail.lock().unwrap() = false;
    until(&mut pb, h.project(), 1);
    h.apply(Command::RelinkAssets(vec![(asset, "new.mp4".into())]))
        .unwrap();
    until(&mut pb, h.project(), 1);
    assert!(paths.lock().unwrap().contains(&"new.mp4".into()));
}

/// Release only (debug FFmpeg and GPU code is too slow to say anything): ten seconds of two
/// stacked 1080p30 layers over an audio track, played in real time through the engine as the app
/// plays it (decode-ahead, lead, dropping late frames, audio mixed into the device), shows all 300
/// frames, each the right one: decoding keeps up with zero drops. The same run with every frame
/// composited at half-size preview scale is only reported: CI composites on lavapipe, a software
/// rasterizer at ~30ms a frame, which says nothing about a real GPU. Full rate with compositing
/// is checked by hand on real hardware (#32).
/// shortcut: gate compositing too once a perf harness (#42) runs on real GPU hardware.
#[test]
fn two_layer_1080p30_plays_in_real_time() {
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
    let tone = dir.join("tone-1khz.m4a");
    add(&mut h, TrackKind::Audio, tone.to_str().unwrap(), secs(10));
    let p = h.project();
    let mut gpu = Gpu::new(&device);
    for draw in [false, true] {
        let mut pb = Playback::from_files(DEFAULT_BUDGET);
        let mut composite = Duration::ZERO;
        let seen = play(&mut pb, p, u64::MAX, |s| {
            if s.index >= 300 {
                return;
            }
            assert_eq!(s.layers.len(), 2);
            for (f, _) in &s.layers {
                let index = jvid_media::test_media::frame_index(&f.planes[0], f.width as usize);
                assert_eq!(index, s.index as u32);
            }
            if draw {
                let start = Instant::now();
                gpu.draw(&device, &queue, p, 2, s);
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                composite += start.elapsed();
            }
        });
        let shown: Vec<_> = seen.iter().map(|s| s.index).filter(|i| *i < 300).collect();
        if draw {
            eprintln!(
                "10s of two-layer 1080p30 composited on this device: {} of 300 frames shown, \
                 {:?} a frame compositing",
                shown.len() + 1,
                composite / shown.len().max(1) as u32
            );
        } else {
            // Frame 0 was shown parked.
            assert_eq!(shown, (1..300).collect::<Vec<_>>(), "decode dropped frames");
            eprintln!("10s of two-layer 1080p30 decoded in real time: all 300 frames shown");
        }
    }
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
