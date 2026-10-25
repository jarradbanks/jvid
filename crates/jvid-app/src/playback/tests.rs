use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use jvid_audio::pan_gains;
use jvid_core::history::{Command, Created, History};
use jvid_core::model::{NewClip, TrackSettings};

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
        hdr: None,
    }
}

fn frame_of(px: &Pixels) -> &Frame {
    match px {
        Pixels::Frame(f) => f,
        _ => panic!("generated"),
    }
}

fn silent() -> OpenAudio {
    Box::new(|_, _| Box::new(|_, _| {}))
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

/// Longer than any fake decode takes, however loaded the machine, and well under a CI timeout.
const HUNG: Duration = Duration::from_secs(10);

/// `pb`'s finished decodes, taken from it so the test hands them back when it chooses.
struct Decodes {
    decoded: Receiver<Done>,
    to_pb: Sender<Done>,
}

impl Decodes {
    fn take(pb: &mut Playback) -> Self {
        let (to_pb, rx) = channel();
        let decoded = std::mem::replace(&mut pb.done, rx);
        Self { decoded, to_pb }
    }

    /// Hands back the decodes `pb` queues at `clock` as they finish, each as instant so the lead
    /// stays zero however loaded the machine, until `done`. Bounded by decodes, not time: a decode
    /// queued over and over fails rather than hangs; one never finishing fails after [`HUNG`].
    fn until(
        &self,
        pb: &mut Playback,
        p: &Project,
        clock: u64,
        mut done: impl FnMut(&mut Playback) -> bool,
    ) {
        for _ in 0..1_000 {
            pb.decode_ahead(p, clock);
            if done(pb) {
                return;
            }
            assert!(!pb.pending.is_empty(), "nothing left to decode");
            let (k, _, r) = self.decoded.recv_timeout(HUNG).unwrap_or_else(|_| {
                panic!("no decode finished in {HUNG:?}: {:?}", pb.pending.keys())
            });
            self.to_pb.send((k, Duration::ZERO, r)).unwrap();
        }
        panic!("decodes never settled");
    }

    /// Waits for every decode `pb` queues at `clock`.
    fn settle(&self, pb: &mut Playback, p: &Project, clock: u64) {
        self.until(pb, p, clock, |pb| pb.pending.is_empty());
    }
}

/// Decodes until the frame at `clock` is shown, as a fast decoder would have it by then.
fn catch_up(pb: &mut Playback, p: &Project, d: &Decodes, clock: u64, last: &mut Option<Shown>) {
    d.until(pb, p, clock, |pb| {
        if let Some(s) = pb.show(p, clock) {
            *last = Some(s);
        }
        pb.shown == Some((pb.transport.frame(clock), true))
    });
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
    let d = Decodes::take(&mut pb);
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
        catch_up(&mut pb, p, &d, out.clock(), &mut last);
        assert!(pb.cache.bytes() <= budget);
        // Only 1x audio carries source positions; the rest is silence or underrun.
        let heard = pulled.last().copied().unwrap_or(0.0);
        if pb.transport.speed() != 1 || heard == 0.0 {
            continue;
        }
        let audio_secs = ((heard / gain) as f64 - 1.0) / SAMPLE_RATE as f64;
        let shown = frame_of(&last.as_ref().unwrap().layers[0].0);
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

/// How far [`play`] lets its clock fall behind real time before it stops catching up.
const STALL: Duration = Duration::from_millis(100);

/// Plays `pb` from its playhead as the app would: first parked until the frame there is shown and
/// decode-ahead is idle, then playing until the clock reaches `until` samples or play stops, with
/// one `tick` per 800-sample device callback (~60Hz) and the fake device held to wall time, so
/// decoding has to keep up for real. Every frame shown is the one for the clock, and goes to `each`.
/// Wall clock on purpose, for the release-only throughput test; the rest use [`play_stepped`].
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
    let mut start = Instant::now();
    while out.clock() < until && pb.transport.speed() != 0 {
        let shown = pb.tick(p, &mut out);
        if let Some(s) = &shown {
            assert_eq!(s.index, pb.transport.frame(out.clock()), "shown on time");
            each(s);
        }
        let heard = fake.pull(800);
        if let Some(s) = shown {
            let pts = s.layers.iter().map(|(f, _)| frame_of(f).pts).collect();
            seen.push(Seen {
                index: s.index,
                pts,
                heard: heard[0],
            });
        }
        let due = Duration::from_secs_f64(out.clock() as f64 / SAMPLE_RATE as f64);
        // Oversleeping is caught up, so the clock keeps real time. A longer stall (the process
        // descheduled on a loaded CI runner) is mostly waited out, as a device would, rather
        // than pulled in a burst no decoder could keep up with. Tests of throughput check the
        // real time taken themselves.
        start += start.elapsed().saturating_sub(due + STALL);
        std::thread::sleep(due.saturating_sub(start.elapsed()));
    }
    seen
}

/// Audio whose every sample holds its own source index + 1.
fn indexed() -> OpenAudio {
    Box::new(|_, _| {
        Box::new(|start, out| {
            for (i, f) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                f.fill((start + i as i64 + 1) as f32);
            }
        })
    })
}

/// Like [`play`], stepped rather than in real time, one tick per frame so a frame missed at its
/// tick is dropped, not shown a tick late. Decodes finish at once and are handed back as
/// [`DECODE_THREADS`] threads would if each took `cost`, rounded up to whole frames.
fn play_stepped(
    pb: &mut Playback,
    p: &Project,
    until: u64,
    mut cost: impl FnMut(&Key) -> Duration,
) -> Vec<Seen> {
    let fake = Fake::default();
    let mut out = Output::new(Box::new(fake.clone()), None);
    let d = Decodes::take(pb);
    // Decodes waiting for a thread, oldest (nearest) first, and those on one with the step they end.
    let mut queued = BTreeMap::new();
    let mut running: Vec<(u64, Done)> = Vec::new();
    let (mut primed, mut playing, mut seen) = (false, false, Vec::new());
    for step in 0u64.. {
        let (done, busy): (Vec<_>, Vec<_>) = std::mem::take(&mut running)
            .into_iter()
            .partition(|r| r.0 <= step);
        running = busy;
        for (_, done) in done {
            d.to_pb.send(done).unwrap();
        }
        let shown = pb.tick(p, &mut out);
        if let Some(s) = &shown {
            assert_eq!(s.index, pb.transport.frame(out.clock()), "shown on time");
        }
        // Everything playback is waiting on is held here, not still in the real decode.
        let held = |k: &Key, q: &BTreeMap<Key, _>, r: &[(u64, Done)]| {
            q.contains_key(k) || r.iter().any(|r| r.1.0 == *k)
        };
        while !pb.pending.keys().all(|k| held(k, &queued, &running)) {
            let (k, _, r) = d.decoded.recv().unwrap();
            queued.insert(k, r);
        }
        // A decode cancelled before a thread took it never runs.
        queued.retain(|k, _| pb.pending.contains_key(k));
        while running.len() < DECODE_THREADS
            && let Some((k, r)) = queued.pop_first()
        {
            let took = cost(&k);
            let frames = (took.as_micros() * 30).div_ceil(1_000_000).max(1) as u64;
            running.push((step + frames, (k, took, r)));
        }
        if !playing {
            primed |= shown.is_some();
            playing = primed && pb.pending.is_empty();
            if playing {
                pb.transport.toggle(out.clock());
            }
            continue;
        }
        let heard = fake.pull(SAMPLE_RATE as usize / 30);
        if let Some(s) = shown {
            let pts = s.layers.iter().map(|(f, _)| frame_of(f).pts).collect();
            seen.push(Seen {
                index: s.index,
                pts,
                heard: heard[0],
            });
        }
        if out.clock() >= until || pb.transport.speed() == 0 {
            break;
        }
    }
    seen
}

/// With a decoder too slow for 30fps, so frames really arrive late, playback keeps to the audio
/// clock: every frame shown is the one for the clock's time and within a frame of the audio heard
/// with it, frames are skipped rather than shown late, and it keeps up to the end. Stepped: 4
/// decode threads at 200ms (6 frames) a frame, 20fps.
#[test]
fn slow_decoder_drops_frames_instead_of_drifting() {
    let mut h = History::new(Project::default());
    add(&mut h, TrackKind::Video, "v.mp4", secs(10));
    add(&mut h, TrackKind::Audio, "a.m4a", secs(10));
    let p = h.project();
    let mut pb = Playback::new(Arc::new(|_, t| Ok(tiny(t))), indexed(), DEFAULT_BUDGET);
    let seen = play_stepped(&mut pb, p, 3 * SAMPLE_RATE as u64, |_| {
        Duration::from_millis(200)
    });
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

/// Back-to-back clips of two files, then a gap and a third: each file is slow to open (150ms for
/// its first frame, 5ms for the rest), but decode-ahead opens it before the playhead gets there,
/// so no frame drops at either boundary; the gap shows nothing. The audio clip after the gap is
/// opened and seeked to its in point before it's heard. Stepped, not real time.
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
    let open: OpenAudio = Box::new(move |path, _| {
        let (log, path) = (log.clone(), path.to_owned());
        Box::new(move |start, out| log.lock().unwrap().push((path.clone(), start, out.len())))
    });
    let mut pb = Playback::new(Arc::new(|_, t| Ok(tiny(t))), open, DEFAULT_BUDGET);
    let mut opened = HashSet::new();
    let seen = play_stepped(&mut pb, p, u64::MAX, |k| {
        Duration::from_millis(if opened.insert(k.0) { 150 } else { 5 })
    });
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
/// loop) before the playhead jumps back. Stepped, not real time: each tick's decodes finish before
/// the next, so a frame shows only if it was queued before the playhead got there.
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
    // Room for the decode-ahead window and what was just shown, not for the whole loop.
    let mut pb = Playback::new(
        Arc::new(|_, t| Ok(tiny(t))),
        silent(),
        40 * size(&tiny(Time(0))),
    );
    pb.looping = true;
    pb.sync(p);
    pb.transport.seek(secs(1), 0);
    let seen = play_stepped(&mut pb, p, 6 * SAMPLE_RATE as u64, |_| Duration::ZERO);
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

/// A failed decode leaves its layer out, then is retried after [`RETRY`]; a relink drops frames
/// decoded from the asset's old path, so the new file is decoded. Stepped, not real time: decodes
/// are waited for, and failures dated rather than waited out.
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
    let d = Decodes::take(&mut pb);
    let mut out = Output::new(Box::new(Fake::default()), None);
    // Decodes what's queued, dates every failure `failed_at`, then the layers at the playhead.
    let mut layers = |pb: &mut Playback, p: &Project, failed_at: Instant| {
        pb.tick(p, &mut out);
        d.settle(pb, p, 0);
        pb.failed.values_mut().for_each(|at| *at = failed_at);
        pb.refresh();
        pb.tick(p, &mut out).map(|s| s.layers.len())
    };
    let (held, due) = (Instant::now() + 1000 * RETRY, Instant::now() - RETRY);
    assert_eq!(layers(&mut pb, h.project(), held), Some(0), "left out");
    *fail.lock().unwrap() = false;
    assert_eq!(
        layers(&mut pb, h.project(), held),
        Some(0),
        "not before RETRY"
    );
    layers(&mut pb, h.project(), due);
    assert_eq!(layers(&mut pb, h.project(), held), Some(1), "retried");
    h.apply(Command::RelinkAssets(vec![(asset, "new.mp4".into())]))
        .unwrap();
    assert_eq!(layers(&mut pb, h.project(), held), Some(1), "relinked");
    assert!(paths.lock().unwrap().contains(&"new.mp4".into()));
}

/// A frame retried the moment its failure is taken is decoded again: its job is still live in the
/// pool then, and must not hand the retry its spent handle (#371). Racy by nature, so many times.
#[test]
fn retry_queued_as_a_failure_lands_redecodes() {
    let mut h = History::new(Project::default());
    add(&mut h, TrackKind::Video, "v.mp4", secs(1));
    let p = h.project();
    let mut pb = Playback::new(Arc::new(|_, _| Err("no".into())), silent(), DEFAULT_BUDGET);
    let d = Decodes::take(&mut pb);
    pb.sync(p);
    for _ in 0..1_000 {
        d.settle(&mut pb, p, 0);
        pb.failed.values_mut().for_each(|at| *at -= RETRY);
    }
}

/// Release only (debug FFmpeg and GPU code is too slow to say anything): ten seconds of two
/// stacked 1080p30 layers over an audio track, played in real time through the engine as the app
/// plays it (decode-ahead, lead, dropping late frames, audio mixed into the device), shows all 300
/// frames, each the right one: decoding keeps up with zero drops. The same run with every frame
/// composited at half-size preview scale is only reported: CI composites on lavapipe, a software
/// rasterizer at ~30ms a frame, which says nothing about a real GPU. Full rate with compositing
/// is checked by hand on real hardware (#32). Wall clock on purpose: it times real decoding.
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
        let mut span = [None; 2];
        let seen = play(&mut pb, p, u64::MAX, |s| {
            if s.index >= 300 {
                return;
            }
            span[(s.index != 1) as usize] = Some(Instant::now());
            assert_eq!(s.layers.len(), 2);
            for (f, _) in &s.layers {
                let f = frame_of(f);
                let index = jvid_media::test_media::frame_index(&f.planes[0], f.width as usize);
                assert_eq!(index, s.index as u32);
            }
            if draw {
                let start = Instant::now();
                gpu.draw(&device, &queue, p.settings(), 2, s);
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
            println!(
                "PERF composite two-layer 1080p30 (report only on lavapipe): {} of 300 frames shown, \
                 {:?} a frame compositing",
                shown.len() + 1,
                composite / shown.len().max(1) as u32
            );
        } else {
            // Frame 0 was shown parked.
            assert_eq!(shown, (1..300).collect::<Vec<_>>(), "decode dropped frames");
            // In real time, give or take a runner stall [`play`] waited out.
            let [Some(first), Some(last)] = span else {
                unreachable!()
            };
            let fps = 298.0 / (last - first).as_secs_f64();
            let budget = 27.0;
            println!(
                "PERF playback two-layer 1080p30: {fps:.1} fps, 300/300 frames shown \
                 (budget 30 fps, at least {budget:.1})"
            );
            assert!(fps >= budget, "{fps:.1} fps");
        }
    }
}

/// Solids draw without decoding: the asset's color, or the clip's `color` keyframes over it.
#[test]
fn solids_show_their_color_without_decoding() {
    use jvid_core::keyframe::{Easing, Keyframe, Value};
    let mut h = History::new(Project::default());
    let Ok(Created::Asset(asset)) = h.apply(Command::AddSolid {
        name: "Red".into(),
        color: [1.0, 0.0, 0.0, 1.0],
    }) else {
        panic!("solid")
    };
    let name = "V1".into();
    let Ok(Created::Track(track)) = h.apply(Command::AddTrack {
        kind: TrackKind::Video,
        name,
    }) else {
        panic!("track")
    };
    let clip = NewClip {
        asset,
        source_in: Time(0),
        source_out: secs(1),
        start: Time(0),
        link: None,
    };
    let Ok(Created::Clip(id)) = h.apply(Command::AddClip { track, clip }) else {
        panic!("clip")
    };
    let decode: DecodeFn = Arc::new(|_, _| panic!("solids never decode"));
    let mut pb = Playback::new(decode, silent(), DEFAULT_BUDGET);
    let mut out = Output::new(Box::new(Fake::default()), None);
    let mut color = |pb: &mut Playback, p: &Project| {
        pb.refresh();
        match pb.tick(p, &mut out).expect("shown").layers[..] {
            [(Pixels::Solid(c), _)] => c,
            _ => panic!("one solid layer"),
        }
    };
    assert_eq!(color(&mut pb, h.project()), [255, 0, 0, 255]);
    h.apply(Command::AddKeyframe {
        id,
        param: "color".into(),
        key: Keyframe {
            time: Time(0),
            value: Value::Color([0.0, 0.0, 1.0, 1.0]),
            easing: Easing::Linear,
        },
    })
    .unwrap();
    assert_eq!(color(&mut pb, h.project()), [0, 0, 255, 255]);
}

/// Release only. Perf (docs/perf.md): parked seeks to scattered frames of a 1080p30 H.264 file,
/// timed from the seek until `tick` shows that frame. Under one frame (33ms) scrubs at 30fps.
/// Wall clock on purpose: it times real decoding.
#[test]
fn seek_latency_1080p30() {
    if cfg!(debug_assertions) {
        eprintln!("skipping seek latency in a debug build; run with --release");
        return;
    }
    let Some(dir) = jvid_media::test_media::corpus() else {
        return;
    };
    jvid_media::ffmpeg::init().unwrap();
    let path = dir.join("frame-index-1080p30.mp4");
    let mut h = History::new(Project::default());
    add(&mut h, TrackKind::Video, path.to_str().unwrap(), secs(10));
    let p = h.project();
    let mut pb = Playback::from_files(DEFAULT_BUDGET);
    let mut out = Output::new(Box::new(Fake::default()), None);
    // Picks up the sequence length, so seeks aren't clamped to zero.
    pb.tick(p, &mut out);
    let mut times = Vec::new();
    // Frames far apart and out of order, so each is a real seek rather than decode-ahead.
    for frame in [150, 17, 280, 93, 211, 4, 260, 120, 45, 199] {
        pb.transport
            .seek(p.settings().frame_rate.time_floor(frame), out.clock());
        let start = Instant::now();
        loop {
            if pb
                .tick(p, &mut out)
                .is_some_and(|s| s.index == frame && s.layers.len() == 1)
            {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "frame {frame} never shown"
            );
            std::thread::sleep(Duration::from_micros(200));
        }
        times.push(start.elapsed());
    }
    times.sort();
    println!(
        "PERF seek latency 1080p30 H.264: median {:.1} ms, worst {:.1} ms (budget 33 ms)",
        times[times.len() / 2].as_secs_f64() * 1e3,
        times[times.len() - 1].as_secs_f64() * 1e3
    );
}

/// A software device where `gpu()` allows one (lavapipe on Linux); `None` skips.
pub(crate) fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    if !suite_ui::test_support::gpu() {
        return None;
    }
    Some(crate::export::headless_gpu().expect("device"))
}

/// Acceptance (#90): a 2-hour 1440p60 recording is editable within 2 s of import, and a seek
/// anywhere shows a frame in under 150 ms. Release only; wall clock on purpose, it times real
/// decoding.
#[test]
fn long_recording_imports_and_seeks_fast() {
    if cfg!(debug_assertions) {
        eprintln!("skipping long recording timing in a debug build; run with --release");
        return;
    }
    let Some(dir) = jvid_media::test_media::corpus() else {
        return;
    };
    jvid_media::ffmpeg::init().unwrap();
    let path = dir.join("long-2h-1440p60.mp4");
    let start = Instant::now();
    let probe = jvid_media::probe::probe(&path).unwrap();
    let mut h = History::new(Project::default());
    add(
        &mut h,
        TrackKind::Video,
        path.to_str().unwrap(),
        probe.duration,
    );
    let p = h.project();
    let mut pb = Playback::from_files(DEFAULT_BUDGET);
    let mut out = Output::new(Box::new(Fake::default()), None);
    while pb.tick(p, &mut out).is_none_or(|s| s.layers.is_empty()) {
        std::thread::sleep(Duration::from_micros(200));
    }
    // Editable means seeks are quick too: the stand-in decoder, queued for frame 0, is open.
    let warm = pb.quick_jobs.submit("warm", Priority::Interactive, |_| {});
    while !warm.is_finished() {
        std::thread::sleep(Duration::from_micros(200));
    }
    let import = start.elapsed();
    let rate = p.settings().frame_rate;
    let last = rate.index_floor(probe.duration) - 1;
    let (mut first, mut exact) = (Vec::new(), Vec::new());
    // Spread over the whole file and out of order, so every one is a real seek.
    for k in [7, 2, 9, 0, 5, 3, 8, 1, 6, 4] {
        let frame = last * k / 9 - k;
        pb.transport.seek(rate.time_floor(frame), out.clock());
        let start = Instant::now();
        loop {
            if let Some(s) = pb.tick(p, &mut out).filter(|s| s.index == frame) {
                assert_eq!(s.layers.len(), 1);
                if first.len() < exact.len() + 1 {
                    first.push(start.elapsed());
                }
                if s.exact {
                    exact.push(start.elapsed());
                    break;
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "frame {frame} never exact"
            );
            std::thread::sleep(Duration::from_micros(200));
        }
    }
    let ms = |v: &mut Vec<Duration>| {
        v.sort();
        (v[v.len() / 2].as_secs_f64() * 1e3, v[v.len() - 1])
    };
    let ((first_med, worst), (exact_med, exact_worst)) = (ms(&mut first), ms(&mut exact));
    println!(
        "PERF long 2h 1440p60: import to first frame {:.0} ms (budget 2000), seek to first frame median {first_med:.1} ms, worst {:.1} ms (budget 150), to exact frame median {exact_med:.0} ms, worst {:.0} ms",
        import.as_secs_f64() * 1e3,
        worst.as_secs_f64() * 1e3,
        exact_worst.as_secs_f64() * 1e3,
    );
    assert!(import < Duration::from_secs(2), "import took {import:?}");
    assert!(worst < Duration::from_millis(150), "seek took {worst:?}");
}

#[test]
fn parked_seek_shows_a_keyframe_stand_in_then_the_exact_frame() {
    let mut h = History::new(Project::default());
    add(&mut h, TrackKind::Video, "/a.mp4", secs(10));
    let p = h.project();
    // Exact decodes wait for the release; the stand-in is always the frame at 0.
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let r = release.clone();
    let decode: DecodeFn = Arc::new(move |_, t| {
        while !r.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(tiny(t))
    });
    let mut pb = Playback::new(decode, silent(), DEFAULT_BUDGET);
    pb.quick = Some(Arc::new(|_, _| Ok(tiny(Time(0)))));
    let d = Decodes::take(&mut pb);
    let mut out = Output::new(Box::new(Fake::default()), None);
    pb.sync(p);
    pb.transport.seek(secs(5), out.clock());
    pb.tick(p, &mut out);
    // Waited for, not polled: the stand-in thread's result is handed back once it's sent.
    assert!(
        pb.stand_ins.contains_key(&(p.assets()[0].id, secs(5))),
        "no stand-in queued"
    );
    let r = pb.quick_done.recv().unwrap();
    pb.quick_tx.send(r).unwrap();
    let mut next = |pb: &mut Playback| {
        let s = pb.tick(p, &mut out).expect("shown");
        assert_eq!(s.index, 150);
        let Pixels::Frame(f) = &s.layers[0].0 else {
            panic!("frame")
        };
        (s.exact, f.pts)
    };
    assert_eq!(next(&mut pb), (false, Time(0)));
    release.store(true, std::sync::atomic::Ordering::SeqCst);
    d.settle(&mut pb, p, 0);
    assert_eq!(next(&mut pb), (true, secs(5)));
}

/// Acceptance (#90): a 144 fps clip in a 60 fps project shows, at every project frame over its
/// whole length, the source frame at that frame's time. Conformed by timestamp, so it can't drift.
#[test]
fn high_fps_clip_conforms_to_the_project_rate_without_drift() {
    let Some(dir) = jvid_media::test_media::corpus() else {
        return;
    };
    jvid_media::ffmpeg::init().unwrap();
    let path = dir.join("144fps.mp4");
    let duration = jvid_media::probe::probe(&path).unwrap().duration;
    let mut h = History::new(Project::default());
    let mut settings = h.project().settings().clone();
    settings.frame_rate = Rate::new(60, 1);
    h.apply(Command::SetSettings(settings)).unwrap();
    add(&mut h, TrackKind::Video, path.to_str().unwrap(), duration);
    let p = h.project();
    let rate = p.settings().frame_rate;
    let pool = DecoderPool::default();
    let frames = rate.index_floor(duration);
    assert_eq!(frames, 180);
    for k in 0..frames {
        let layers = jvid_core::eval::video_at(p.sequence(), rate.time_floor(k));
        let got = pool.frame_at(&path, layers[0].source_time).unwrap().pts;
        // Source frame floor(k * 144 / 60), exactly: flicks divide both rates.
        let want = Time(k * 144 / 60 * FLICKS_PER_SECOND / 144);
        assert_eq!(got, want, "project frame {k}");
    }
}

/// Adds a solid of `color` and a new video track holding it from `start` for 1s.
pub(crate) fn add_solid(h: &mut History, color: [f64; 4], start: Time) -> ClipId {
    let Ok(Created::Asset(asset)) = h.apply(Command::AddSolid {
        name: "Solid".into(),
        color,
    }) else {
        panic!("solid")
    };
    let kind = TrackKind::Video;
    let Ok(Created::Track(track)) = h.apply(Command::AddTrack {
        kind,
        name: "V".into(),
    }) else {
        panic!("track")
    };
    let clip = NewClip {
        asset,
        source_in: Time(0),
        source_out: secs(1),
        start,
        link: None,
    };
    let Ok(Created::Clip(c)) = h.apply(Command::AddClip { track, clip }) else {
        panic!("clip")
    };
    c
}

#[test]
fn two_layers_composite_with_opacity() {
    let Some((device, queue)) = device() else {
        return;
    };
    let mut h = History::new(Project::default());
    add_solid(&mut h, [1.0, 0.0, 0.0, 1.0], Time(0));
    let top = add_solid(&mut h, [0.0, 0.0, 1.0, 1.0], Time(0));
    let half = Transform {
        opacity: 0.5,
        ..Transform::default()
    };
    h.apply(Command::SetClipProps(vec![(top, half, Default::default())]))
        .unwrap();
    let decode: DecodeFn = Arc::new(|_, _| panic!("solids never decode"));
    let mut pb = Playback::new(decode, silent(), DEFAULT_BUDGET);
    let shown = pb.park(h.project()).expect("shown");
    assert_eq!(shown.layers.len(), 2);
    let tex = Gpu::new(&device)
        .draw(&device, &queue, h.project().settings(), 4, &shown)
        .clone();
    let px = composite::read_back(&device, &queue, &tex);
    let [r, g, b, a] = px[..4] else { panic!() };
    assert!(
        r.abs_diff(128) <= 2 && g == 0 && b.abs_diff(127) <= 2 && a == 255,
        "{:?}",
        &px[..4]
    );
}

#[test]
fn text_layer_draws_over_a_solid() {
    let Some((device, queue)) = device() else {
        return;
    };
    let mut h = History::new(Project::default());
    add_solid(&mut h, [1.0, 0.0, 0.0, 1.0], Time(0));
    let Ok(Created::Asset(asset)) = h.apply(Command::AddText {
        name: "Title".into(),
        text: jvid_core::model::Text::default(),
    }) else {
        panic!("text")
    };
    let Ok(Created::Track(track)) = h.apply(Command::AddTrack {
        kind: TrackKind::Video,
        name: "V2".into(),
    }) else {
        panic!("track")
    };
    let clip = NewClip {
        asset,
        source_in: Time(0),
        source_out: secs(1),
        start: Time(0),
        link: None,
    };
    h.apply(Command::AddClip { track, clip }).unwrap();
    let decode: DecodeFn = Arc::new(|_, _| panic!("generated layers never decode"));
    let mut pb = Playback::new(decode, silent(), DEFAULT_BUDGET);
    let shown = pb.park(h.project()).expect("shown");
    assert!(matches!(shown.layers[0].0, Pixels::Text(_)), "text on top");
    let tex = Gpu::new(&device)
        .draw(&device, &queue, h.project().settings(), 1, &shown)
        .clone();
    let px = composite::read_back(&device, &queue, &tex);
    let px: Vec<_> = px.chunks(4).collect();
    assert_eq!(px[0], [255, 0, 0, 255], "the solid shows around the text");
    assert!(px.contains(&&[255, 255, 255, 255][..]), "white glyphs show");
}

/// A merged clip sums a source per stream; a per-stream clip opens only its own.
#[test]
fn merged_clip_mixes_every_stream() {
    use jvid_core::model::ProbeInfo;
    let probe = ProbeInfo {
        duration: secs(1),
        audio_streams: 3,
        ..Default::default()
    };
    let mut gains = Vec::new();
    for merge in [true, false] {
        let mut h = History::new(Project::default());
        h.apply(Command::Import(vec![("obs.mkv".into(), probe.clone())]))
            .unwrap();
        let asset = h.project().assets()[0].id;
        let p = h.project().placement(asset, Time(0), false, None, merge);
        h.apply(Command::Place(p.unwrap())).unwrap();
        // Stream n holds n + 1, so the sum says which streams were heard.
        let mut audio = Audio::new(Box::new(|_, nth| {
            Box::new(move |_, out: &mut [f32]| out.fill(nth as f32 + 1.0))
        }));
        let mut out = vec![0.0; 8];
        audio.mix(h.project(), 0, &mut out);
        gains.push(out[0] / pan_gains(0.0).0);
    }
    // Merged: 1 + 2 + 3; separate tracks: the same total, one stream per clip.
    assert_eq!(gains, [6.0, 6.0]);
}

/// Acceptance (#50): a keyframed volume ramp is applied per sample, so across block edges it
/// rises in even steps with no zipper.
#[test]
fn volume_ramp_has_no_zipper() {
    use jvid_core::keyframe::{Easing, Keyframe, Value};
    let mut h = History::new(Project::default());
    let id = add(&mut h, TrackKind::Audio, "/a.wav", secs(2));
    let c = h.project().sequence().clip(id).unwrap().1.clone();
    let key = |t, v| Keyframe {
        time: c.source_in + t,
        value: Value::Scalar(v),
        easing: Easing::Linear,
    };
    let keys = [(
        "volume".to_string(),
        vec![key(Time(0), 0.0), key(secs(1), 1.0)],
    )];
    let x = Transform {
        pan: -1.0,
        ..c.transform
    };
    h.apply(Command::SetClipProps(vec![(id, x, keys.into())]))
        .unwrap();
    let mut audio = Audio::new(Box::new(|_, _| {
        Box::new(|_, out: &mut [f32]| out.fill(1.0))
    }));
    let mut left: Vec<f32> = Vec::new();
    let mut out = vec![0.0; 2 * 512];
    for b in 0..(SAMPLE_RATE as i64 / 512) {
        audio.mix(h.project(), b * 512, &mut out);
        assert!(
            out.iter().skip(1).step_by(2).all(|&r| r == 0.0),
            "panned left"
        );
        left.extend(out.iter().step_by(2));
    }
    let step = pan_gains(0.0).0 / SAMPLE_RATE as f32;
    for w in left.windows(2) {
        let d = w[1] - w[0];
        assert!(d > 0.0 && (d - step).abs() < step * 0.05, "{w:?}");
    }
}

/// A fade on an eased speed ramp finds the clip's length once per block, not once per sample
/// (#82): a second of mix integrates dozens of pieces of the ramp, not two per sample.
#[cfg(debug_assertions)]
#[test]
fn eased_ramp_fade_integrates_per_block_not_per_sample() {
    use jvid_core::keyframe::{Easing, Keyframe, SPEED, Value, integrated};
    let mut h = History::new(Project::default());
    let id = add(&mut h, TrackKind::Audio, "/a.wav", secs(4));
    let c = h.project().sequence().clip(id).unwrap().1.clone();
    let key = |t, v| Keyframe {
        time: t,
        value: Value::Scalar(v),
        easing: Easing::EaseInOut,
    };
    // Keyed on past the out point, so every lookup of the clip's length lands mid-ease.
    let keys = [(
        SPEED.to_string(),
        vec![key(Time(0), 1.0), key(secs(8), 2.0)],
    )];
    let x = Transform {
        fade_in: secs(2),
        ..c.transform
    };
    h.apply(Command::SetClipProps(vec![(id, x, keys.into())]))
        .unwrap();
    let mut audio = Audio::new(Box::new(|_, _| {
        Box::new(|_, out: &mut [f32]| out.fill(1.0))
    }));
    let mut out = vec![0.0; 2 * SAMPLE_RATE as usize];
    let before = integrated();
    audio.mix(h.project(), 0, &mut out);
    let n = integrated() - before;
    assert!(n < 1_000, "{n} pieces integrated for {SAMPLE_RATE} samples");
    // Still fading in: half way through the 2 s fade at the block's end.
    let end = out[out.len() - 2] / pan_gains(0.0).0;
    assert!(out[0] == 0.0 && (end - 0.5).abs() < 0.01, "{end}");
}

#[test]
fn scrub_queues_a_snippet_from_the_parked_playhead() {
    let mut h = History::new(Project::default());
    add(&mut h, TrackKind::Audio, "/a.wav", secs(10));
    let p = h.project();
    let mut pb = Playback::new(Arc::new(|_, t| Ok(tiny(t))), indexed(), DEFAULT_BUDGET);
    pb.sync(p);
    let fake = Fake::default();
    let mut out = Output::new(Box::new(fake.clone()), None);
    pb.tick(p, &mut out);
    pb.transport.seek(secs(2), out.clock());
    pb.scrub(p, &mut out);
    let heard = fake.pull(SCRUB_SAMPLES);
    let gain = pan_gains(0.0).0;
    let first = sample(secs(2)) as f32 + 1.0;
    assert_eq!(heard[0], first * gain);
    assert_eq!(
        heard[(SCRUB_SAMPLES - 1) * 2],
        (first + SCRUB_SAMPLES as f32 - 1.0) * gain
    );
    assert_eq!(pb.transport.speed(), 0, "scrubbing doesn't start playback");
}

/// Acceptance (#41): dragging the playhead across 1080p H.264, the Program monitor updates 30+
/// times a second, showing keyframe stand-ins first and exact frames once decoded. Release only;
/// wall clock on purpose, it times real decoding.
#[test]
fn scrub_1080p30_updates_30_per_second() {
    if cfg!(debug_assertions) {
        eprintln!("skipping scrub rate in a debug build; run with --release");
        return;
    }
    let Some(dir) = jvid_media::test_media::corpus() else {
        return;
    };
    jvid_media::ffmpeg::init().unwrap();
    let path = dir.join("frame-index-1080p30.mp4");
    let mut h = History::new(Project::default());
    add(&mut h, TrackKind::Video, path.to_str().unwrap(), secs(10));
    let p = h.project();
    let mut pb = Playback::from_files(DEFAULT_BUDGET);
    let mut out = Output::new(Box::new(Fake::default()), None);
    pb.tick(p, &mut out);
    // A 60 Hz UI dragging the playhead across all ten seconds in three.
    let start = Instant::now();
    let (mut updates, mut exact) = (0, 0);
    for ui_frame in 0..180 {
        let frame = ui_frame * 300 / 180;
        pb.transport.seek(FPS30.time_floor(frame), out.clock());
        pb.refresh();
        if let Some(s) = pb.tick(p, &mut out) {
            updates += 1;
            exact += s.exact as u32;
        }
        let due = Duration::from_secs_f64((ui_frame + 1) as f64 / 60.0);
        std::thread::sleep(due.saturating_sub(start.elapsed()));
    }
    let rate = updates as f64 / start.elapsed().as_secs_f64();
    println!(
        "PERF scrub 1080p30 H.264: {rate:.1} updates/s ({exact} exact of {updates}, budget 30)"
    );
    assert!(rate >= 30.0, "{rate:.1} updates/s");
}

#[test]
fn speed_resamples_and_reverse_reads_backwards() {
    // Left channel holds the source frame index.
    let mut src: ClipAudio = Box::new(|from, out: &mut [f32]| {
        for (i, f) in out.chunks_mut(2).enumerate() {
            f[0] = (from + i as i64) as f32;
        }
    });
    let seg = |speed, reverse| AudioSegment {
        track: jvid_core::model::TrackId(1),
        clip: ClipId(1),
        asset: AssetId(1),
        start: Time(0),
        end: Time(0),
        source_start: Time(100 * FLICKS_PER_SAMPLE),
        stream: None,
        crossfade: false,
        volume: 1.0,
        pan: 0.0,
        speed,
        source_end: None,
        reverse,
    };
    let left = |s: &AudioSegment, src: &mut ClipAudio| {
        let mut out = vec![0.0; 8];
        read(src, s, &mut out, &mut Vec::new());
        out.iter().step_by(2).copied().collect::<Vec<_>>()
    };
    assert_eq!(
        left(&seg(1.0, false), &mut src),
        [100.0, 101.0, 102.0, 103.0]
    );
    assert_eq!(
        left(&seg(2.0, false), &mut src),
        [100.0, 102.0, 104.0, 106.0]
    );
    assert_eq!(
        left(&seg(0.5, false), &mut src),
        [100.0, 100.5, 101.0, 101.5]
    );
    assert_eq!(left(&seg(1.0, true), &mut src), [100.0, 99.0, 98.0, 97.0]);
    // A ramp from 1x glides to land 8 samples on over these 4: x = i + i²/4.
    let end = Some(Time(108 * FLICKS_PER_SAMPLE));
    let ramp = |reverse| AudioSegment {
        source_end: end.map(|e| {
            if reverse {
                Time(92 * FLICKS_PER_SAMPLE)
            } else {
                e
            }
        }),
        ..seg(1.0, reverse)
    };
    let glide = [100.0, 101.25, 103.0, 105.25];
    assert_eq!(left(&ramp(false), &mut src), glide);
    assert_eq!(left(&ramp(true), &mut src), glide.map(|x| 200.0 - x));
}

/// Through a transition both clips on a track are summed at equal power, not one written over
/// the other.
#[test]
fn transition_mixes_both_clips_at_equal_power() {
    use jvid_core::model::{Transition, TransitionKind};
    let mut h = History::new(Project::default());
    let a = add(&mut h, TrackKind::Audio, "/a.wav", secs(1));
    let (track, c) = h
        .project()
        .sequence()
        .clip(a)
        .map(|(t, c)| (t, c.clone()))
        .unwrap();
    let clip = NewClip {
        asset: c.asset,
        source_in: secs(1),
        source_out: secs(2),
        start: secs(1),
        link: None,
    };
    h.apply(Command::AddClip { track, clip }).unwrap();
    let x = Transform {
        transition: Some(Transition {
            kind: TransitionKind::Crossfade,
            duration: secs(1),
        }),
        ..c.transform.clone()
    };
    h.apply(Command::SetClipProps(vec![(a, x, c.keyframes.clone())]))
        .unwrap();
    let mut audio = Audio::new(Box::new(|_, _| {
        Box::new(|_, out: &mut [f32]| out.fill(1.0))
    }));
    let mut out = vec![0.0; 8];
    let unity = pan_gains(0.0).0;
    audio.mix(h.project(), 0, &mut out);
    assert_eq!(out[0], unity, "before the transition");
    audio.mix(h.project(), SAMPLE_RATE as i64, &mut out);
    assert!(
        (out[0] - unity * std::f32::consts::SQRT_2).abs() < 1e-3,
        "{}",
        out[0]
    );
}

/// `h`'s project with every clip of its open sequence nested into one sequence clip.
fn nest_all(h: &History) -> Project {
    let seq = h.project().sequence();
    let ids = seq.tracks().iter().flat_map(|t| t.clips()).map(|c| c.id);
    let mut nested = h.clone();
    let nest = Command::Nest {
        ids: ids.collect(),
        name: "Nest".into(),
        outer: "Main".into(),
    };
    assert!(matches!(nested.apply(nest), Ok(Created::Clip(_))));
    nested.project().clone()
}

/// Acceptance (#83): a nested sequence renders the same frame as the clips it replaced, pixel
/// for pixel, with a part-covering, half-opaque layer and a text layer inside it.
#[test]
fn nested_sequence_renders_the_frame_it_replaced() {
    let Some((device, queue)) = device() else {
        return;
    };
    let mut h = History::new(Project::default());
    add_solid(&mut h, [0.9, 0.2, 0.1, 1.0], Time(0));
    let top = add_solid(&mut h, [0.1, 0.3, 1.0, 1.0], Time(0));
    let x = Transform {
        opacity: 0.5,
        scale: [0.5, 0.5],
        position: [200.0, -100.0],
        rotation: 10.0,
        ..Transform::default()
    };
    h.apply(Command::SetClipProps(vec![(top, x, Default::default())]))
        .unwrap();
    let Ok(Created::Asset(asset)) = h.apply(Command::AddText {
        name: "Title".into(),
        text: jvid_core::model::Text::default(),
    }) else {
        panic!("text")
    };
    let Ok(Created::Track(track)) = h.apply(Command::AddTrack {
        kind: TrackKind::Video,
        name: "V3".into(),
    }) else {
        panic!("track")
    };
    let clip = NewClip {
        asset,
        source_in: Time(0),
        source_out: secs(1),
        start: Time(0),
        link: None,
    };
    h.apply(Command::AddClip { track, clip }).unwrap();
    let nested = nest_all(&h);
    let draw = |p: &Project| {
        let decode: DecodeFn = Arc::new(|_, _| panic!("generated layers never decode"));
        let shown = Playback::new(decode, silent(), DEFAULT_BUDGET)
            .park(p)
            .expect("shown");
        let tex = Gpu::new(&device)
            .draw(&device, &queue, p.settings(), 1, &shown)
            .clone();
        (
            shown.layers.len(),
            composite::read_back(&device, &queue, &tex),
        )
    };
    let (flat, nest) = (draw(h.project()), draw(&nested));
    assert_eq!(
        (flat.0, nest.0),
        (3, 1),
        "one nested layer in place of three"
    );
    assert!(flat.1 == nest.1, "nested frame differs from the flat one");
}

/// A nested sequence mixes the same samples as the clips it replaced, at their speed.
#[test]
fn nested_sequence_mixes_what_it_replaced() {
    let mut h = History::new(Project::default());
    add_at(&mut h, TrackKind::Audio, "/a.wav", secs(1), secs(1));
    let b = add_at(&mut h, TrackKind::Audio, "/b.wav", Time(0), secs(3));
    let c = h.project().sequence().clip(b).unwrap().1.clone();
    let quiet = Transform {
        volume: 0.5,
        ..c.transform.clone()
    };
    h.apply(Command::SetClipProps(vec![(b, quiet, c.keyframes)]))
        .unwrap();
    // And with b's track turned down and panned, which the nest takes inside with it.
    let mut turned = h.clone();
    let t = turned.project().sequence().clip(b).unwrap().0;
    let t = turned.project().sequence().track(t).unwrap().settings();
    turned
        .apply(Command::SetTrack(TrackSettings {
            volume: 0.3,
            pan: 0.5,
            ..t
        }))
        .unwrap();
    for h in [h, turned] {
        mixes_the_same(&h);
    }
}

fn mixes_the_same(h: &History) {
    let nested = nest_all(h);
    let mix = |p: &Project| {
        let mut audio = Audio::new(indexed());
        let mut all = Vec::new();
        for block in 0..(3 * SAMPLE_RATE as i64 / 1000) {
            let mut out = vec![0.0; 2000];
            audio.prime(p, Time(block * 1000 * FLICKS_PER_SAMPLE));
            audio.mix(p, block * 1000, &mut out);
            all.extend(out);
        }
        all
    };
    let (flat, nest) = (mix(h.project()), mix(&nested));
    assert!(flat.iter().any(|&s| s != 0.0));
    assert_eq!(flat.len(), nest.len());
    // Two tracks' pan gains applied apiece or once to their sum: equal but for rounding.
    let off = |(a, b): (&f32, &f32)| (a - b).abs() / a.abs().max(1.0);
    let worst = flat.iter().zip(&nest).map(off).fold(0.0, f32::max);
    assert!(
        worst < 1e-6,
        "nested mix differs from the flat one by {worst}"
    );
}

/// A nest clip ramped from 100% to 200% maps time into its sequence as the same ramp on a plain
/// clip maps it into the file (#82, #83): the same inner frames, and the same samples gliding.
#[test]
fn a_ramped_nest_clip_plays_like_a_ramped_plain_clip() {
    use jvid_core::keyframe::{Easing, Keyframe, Value};
    let mut flat = History::new(Project::default());
    add(&mut flat, TrackKind::Video, "/a.mp4", secs(3));
    add(&mut flat, TrackKind::Audio, "/a.wav", secs(3));
    let mut nested = History::new(nest_all(&flat));
    let key = |time, v| Keyframe {
        time,
        value: Value::Scalar(v),
        easing: Easing::Linear,
    };
    for h in [&mut flat, &mut nested] {
        let ids: Vec<_> = (h.project().sequence().tracks().iter())
            .flat_map(|t| t.clips())
            .map(|c| c.id)
            .collect();
        for id in ids {
            let keys = vec![key(Time(0), 1.0), key(secs(3), 2.0)];
            let ramp = Command::SetSpeedKeys {
                id,
                speed: 1.0,
                keys,
                ripple: true,
            };
            h.apply(ramp).unwrap();
        }
    }
    let (flat, nested) = (flat.project(), nested.project());
    // 3 s of source at 1 + s/3 per second play in 3 ln 2 s.
    let len = nested.sequence().tracks()[0].clips()[0].duration();
    assert!((len.0 as f64 / FLICKS_PER_SECOND as f64 - 3.0 * 2f64.ln()).abs() < 1e-6);
    for n in 0..FPS30.index_floor(len) {
        let t = FPS30.time_floor(n);
        let shown = |p| {
            leaves(frame_at(p, t))
                .iter()
                .map(|l| l.source_time)
                .collect::<Vec<_>>()
        };
        assert_eq!(shown(flat), shown(nested), "frame {n}");
    }
    // Short of the end, where the nest's sequence runs out of samples to interpolate towards.
    let mix = |p: &Project| {
        let mut audio = Audio::new(indexed());
        let mut all = Vec::new();
        for block in 0..(2 * SAMPLE_RATE as i64 / 1000) {
            let mut out = vec![0.0; 2000];
            audio.prime(p, Time(block * 1000 * FLICKS_PER_SAMPLE));
            audio.mix(p, block * 1000, &mut out);
            all.extend(out);
        }
        all
    };
    let (a, b) = (mix(flat), mix(nested));
    let off = |(a, b): (&f32, &f32)| (a - b).abs() / a.abs().max(1.0);
    let worst = a.iter().zip(&b).map(off).fold(0.0, f32::max);
    assert!(worst < 1e-6, "ramped nest mix differs by {worst}");
    // Gliding: the last block reads near twice as many source samples as it plays.
    let step = (a[a.len() - 2] - a[a.len() - 4]) / pan_gains(0.0).0;
    assert!(step > 1.8, "{step}");
}

/// Clip ids are per sequence, so after a switch a clip sharing an id with one in the last
/// sequence plays its own file, not the source opened for the other.
#[test]
fn a_clip_id_shared_across_a_sequence_switch_plays_its_own_audio() {
    // Each file plays a constant: its name's first letter.
    let by_name = || -> OpenAudio {
        Box::new(|p, _| {
            let x = p.file_name().unwrap().as_encoded_bytes()[0] as f32;
            Box::new(move |_, out| out.fill(x))
        })
    };
    let mut h = History::new(Project::default());
    let a = add(&mut h, TrackKind::Audio, "/a.wav", secs(3));
    let nest = Command::Nest {
        ids: vec![a],
        name: "Nest".into(),
        outer: "Main".into(),
    };
    h.apply(nest).unwrap();
    let c = add(&mut h, TrackKind::Audio, "/c.wav", secs(3));
    let main = h.project().clone();
    h.open_sequence(SequenceId(1)).unwrap();
    let kind = TrackKind::Audio;
    h.apply(Command::AddTrack {
        kind,
        name: "A".into(),
    })
    .unwrap();
    let b = add(&mut h, TrackKind::Audio, "/b.wav", secs(3));
    assert_eq!(b, c, "the two sequences allocate ids apart");
    let n = h.project();
    let decode: DecodeFn = Arc::new(|_, t| Ok(tiny(t)));
    let mut pb = Playback::new(decode, by_name(), DEFAULT_BUDGET);
    let mut got = vec![0.0; 2000];
    for p in [&main, n] {
        pb.sync(p);
        pb.audio.prime(p, Time(0));
        pb.audio.mix(p, 0, &mut got);
    }
    let mut fresh = Audio::new(by_name());
    let mut want = vec![0.0; 2000];
    fresh.prime(n, Time(0));
    fresh.mix(n, 0, &mut want);
    assert!(want.iter().all(|&s| s != 0.0));
    assert_eq!(got, want);
}
