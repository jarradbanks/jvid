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
    };
    assert!(matches!(
        run_now(&project(&dir), &req, false),
        Err(Error::Failed(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"keep");
    std::fs::remove_file(&path).unwrap();
}
