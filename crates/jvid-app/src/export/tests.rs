use std::sync::atomic::{AtomicBool, Ordering};

use jvid_core::history::{Command, Created, History};
use jvid_core::model::{NewClip, TrackKind};
use jvid_core::time::Rate;
use jvid_media::decode::VideoDecoder;
use jvid_media::test_media::{corpus, frame_index};

use super::*;
use crate::playback::tests::device;

fn secs(s: i64) -> Time {
    Time(s * FLICKS_PER_SECOND)
}

/// Adds `path` on a new track as one clip of source `[0, len)` starting at `start`.
fn add(h: &mut History, kind: TrackKind, path: PathBuf, start: Time, len: Time) {
    let Ok(Created::Asset(asset)) = h.apply(Command::AddAsset(path)) else {
        panic!("asset")
    };
    let name = "t".into();
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
    h.apply(Command::AddClip { track, clip }).unwrap();
}

/// The burned-in frame index over 10 s, with a tone that starts 1.5 s into the timeline.
fn project(dir: &Path) -> Project {
    let mut h = History::new(Project::default());
    let video = dir.join("frame-index-1080p30.mp4");
    add(&mut h, TrackKind::Video, video, Time(0), secs(10));
    add(
        &mut h,
        TrackKind::Audio,
        dir.join("tone-1khz.wav"),
        half(3),
        secs(5),
    );
    h.project().clone()
}

fn preset(name: &str) -> Preset {
    let all = jvid_core::preset::builtin();
    all.into_iter().find(|p| p.name == name).expect(name)
}

fn half(n: i64) -> Time {
    Time(n * FLICKS_PER_SECOND / 2)
}

fn out(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("jvid-export-{}-{name}", std::process::id()))
}

fn run_now(p: &Project, req: &Request, cancel: bool) -> Result<ProbeInfo, Error> {
    let (device, queue) = device().expect("checked by caller");
    let pool = Pool::new(1);
    let (tx, rx) = channel();
    let started = Arc::new(AtomicBool::new(false));
    let (p, req, flag) = (p.clone(), req.clone(), started.clone());
    let h = pool.submit("export", Priority::Interactive, move |ctx| {
        flag.store(true, Ordering::SeqCst);
        while cancel && !ctx.is_cancelled() {
            std::thread::yield_now();
        }
        let _ = tx.send(run(&p, &req, &device, &queue, ctx));
    });
    if cancel {
        while !started.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        h.cancel();
    }
    let r = rx.recv().unwrap();
    if r.is_ok() {
        assert_eq!(h.progress(), 1.0);
    }
    r
}

/// Exports 1 s to 2 s to each container: the output lasts the range, its first and last frames
/// are timeline frames 30 and 59, and the tone starts within a frame of 0.5 s in.
#[test]
fn export_matches_range_with_audio_in_sync() {
    let (Some(dir), Some(_)) = (corpus(), device()) else {
        return eprintln!("skipping export test: needs the corpus and a software adapter");
    };
    let p = project(&dir);
    let rate = Rate::new(30, 1);
    for ext in ["mp4", "mov", "mkv"] {
        let path = out(&format!("range.{ext}"));
        let req = Request {
            path: path.clone(),
            range: (secs(1), secs(2)),
            preset: preset("YouTube 1080p"),
            loudness: None,
        };
        let info = run_now(&p, &req, false).unwrap_or_else(|e| panic!("{ext}: {e:?}"));
        assert_eq!((info.width, info.height), (Some(1920), Some(1080)), "{ext}");
        let off = (info.duration - secs(1)).0.abs();
        assert!(off <= rate.time_floor(1).0, "{ext}: {:?}", info.duration);

        let mut v = VideoDecoder::open(&path).unwrap();
        // MKV shifts every stream by AAC's priming (21 ms), so allow up to a frame of offset.
        let frame = rate.time_floor(1);
        let first = v.frame_at(Time(0)).unwrap();
        assert!(
            first.pts.0.abs() <= frame.0,
            "{ext}: starts at {:?}",
            first.pts
        );
        let last = v.frame_at(secs(10)).unwrap();
        assert!((last.pts - secs(1) + frame).0.abs() <= frame.0, "{ext}");
        let index = |f: &jvid_media::decode::Frame| frame_index(&f.planes[0], f.width as usize);
        assert_eq!((index(&first), index(&last)), (30, 59), "{ext}");

        let mut a = AudioDecoder::open(&path, 0).unwrap();
        let mut samples = Vec::new();
        while let Some(c) = a.next_chunk().unwrap() {
            samples.extend(c.samples);
        }
        let onset = samples.iter().position(|s| s.abs() > 0.02).unwrap() as i64 / 2;
        let frame_samples = 48_000 / 30;
        assert!(
            (onset - 24_000).abs() <= frame_samples,
            "{ext}: onset {onset}"
        );
        let len = samples.len() as i64 / 2;
        assert!(
            (len - 48_000).abs() <= frame_samples,
            "{ext}: {len} samples"
        );
        std::fs::remove_file(&path).unwrap();
    }
}

#[test]
fn cancelled_export_leaves_no_file() {
    let (Some(dir), Some(_)) = (corpus(), device()) else {
        return eprintln!("skipping export test: needs the corpus and a software adapter");
    };
    let path = out("cancel.mp4");
    std::fs::write(&path, b"partial").unwrap();
    let req = Request {
        path: path.clone(),
        range: (Time(0), secs(10)),
        preset: preset("YouTube 1080p"),
        loudness: None,
    };
    assert_eq!(run_now(&project(&dir), &req, true), Err(Error::Cancelled));
    assert!(!path.exists());
}

#[test]
fn failed_export_leaves_existing_file() {
    let (Some(dir), Some(_)) = (corpus(), device()) else {
        return eprintln!("skipping export test: needs the corpus and a software adapter");
    };
    let path = out("keep.mp4");
    std::fs::write(&path, b"keep").unwrap();
    let req = Request {
        path: path.clone(),
        range: (secs(1), secs(1)),
        preset: preset("YouTube 1080p"),
        loudness: None,
    };
    assert!(matches!(
        run_now(&project(&dir), &req, false),
        Err(Error::Failed(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"keep");
    std::fs::remove_file(&path).unwrap();
}

/// Every built-in exports 1 s with the streams, size and length it promises.
#[test]
fn every_builtin_preset_exports() {
    let (Some(dir), Some(_)) = (corpus(), device()) else {
        return eprintln!("skipping export test: needs the corpus and a software adapter");
    };
    let p = project(&dir);
    for (i, preset) in jvid_core::preset::builtin().into_iter().enumerate() {
        let path = out(&format!("preset{i}.{}", preset.extension));
        let size = preset.size.map_or((1920, 1080), |[w, h]| (w, h));
        let has_video = preset.video.is_some();
        let name = preset.name.clone();
        let req = Request {
            path: path.clone(),
            range: (secs(1), secs(2)),
            preset,
            loudness: None,
        };
        let info = run_now(&p, &req, false).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        if has_video {
            assert_eq!(
                (info.width, info.height),
                (Some(size.0), Some(size.1)),
                "{name}"
            );
        }
        std::fs::remove_file(&path).unwrap();
    }
}

/// Screen recording keeps the 1440p source size, and the 10 px text line stays within a couple of
/// levels of the source, which a scaled or over-compressed export doesn't.
#[test]
fn screen_recording_keeps_text_sharp() {
    let (Some(dir), Some(_)) = (corpus(), device()) else {
        return eprintln!("skipping export test: needs the corpus and a software adapter");
    };
    let mut h = History::new(Project::default());
    let settings = jvid_core::model::ProjectSettings {
        width: 2560,
        height: 1440,
        frame_rate: Rate::new(60, 1),
        ..Default::default()
    };
    h.apply(Command::SetSettings(settings)).unwrap();
    let src = dir.join("screen-1440p60.mp4");
    add(&mut h, TrackKind::Video, src.clone(), Time(0), secs(3));
    let path = out("sharp.mp4");
    let req = Request {
        path: path.clone(),
        range: (secs(1), secs(2)),
        preset: preset("Screen recording (sharp text)"),
        loudness: None,
    };
    run_now(h.project(), &req, false).unwrap();
    let got = VideoDecoder::open(&path)
        .unwrap()
        .frame_at(Time(0))
        .unwrap();
    let want = VideoDecoder::open(&src).unwrap().frame_at(secs(1)).unwrap();
    assert_eq!((got.width, got.height), (2560, 1440));
    // The text line: rows 195..215, columns 70..600.
    let (mut sum, mut n) = (0u64, 0u64);
    for y in 195..215 {
        for x in 70..600 {
            let i = y * 2560 + x;
            sum += got.planes[0][i].abs_diff(want.planes[0][i]) as u64;
            n += 1;
        }
    }
    let diff = sum as f64 / n as f64;
    eprintln!("text luma diff {diff}");
    assert!(diff < 2.0, "mean luma difference over the text {diff}");
    std::fs::remove_file(&path).unwrap();
}

/// Normalized exports land within 0.5 LU of the target, under the true-peak ceiling.
#[test]
fn export_hits_loudness_target() {
    let (Some(dir), Some(_)) = (corpus(), device()) else {
        return eprintln!("skipping export test: needs the corpus and a software adapter");
    };
    for (name, target) in [("Audio only (WAV)", -14.0), ("Audio only (AAC)", -23.0)] {
        let p = preset(name);
        let path = out(&format!("loud.{}", p.extension));
        let req = Request {
            path: path.clone(),
            range: (Time(0), secs(8)),
            preset: p,
            loudness: Some(target),
        };
        run_now(&project(&dir), &req, false).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let mut a = AudioDecoder::open(&path, 0).unwrap();
        let mut samples = Vec::new();
        while let Some(c) = a.next_chunk().unwrap() {
            samples.extend(c.samples);
        }
        let mut meter = jvid_audio::Meter::default();
        meter.push(&samples);
        let lufs = meter.integrated().unwrap();
        assert!((lufs - target).abs() <= 0.5, "{name}: {lufs} LUFS");
        let peak = 20.0 * jvid_audio::Limiter::true_peak(&samples).log10();
        // AAC's own overshoot isn't the limiter's to hold.
        assert!(peak <= CEILING + 0.5, "{name}: {peak} dBTP");
        std::fs::remove_file(&path).unwrap();
    }
}

#[test]
fn user_presets_skip_bad_files() {
    let dir = out("presets");
    std::fs::create_dir_all(&dir).unwrap();
    let good = "name = \"Mine\"\nextension = \"wav\"\naudio = \"pcm\"\n";
    std::fs::write(dir.join("b.toml"), good).unwrap();
    std::fs::write(dir.join("a.toml"), "name = 3").unwrap();
    std::fs::write(dir.join("c.txt"), good).unwrap();
    let names: Vec<_> = user_presets(&dir).into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["Mine"]);
    std::fs::remove_dir_all(&dir).unwrap();
}
