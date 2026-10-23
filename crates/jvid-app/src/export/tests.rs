use std::sync::atomic::{AtomicBool, Ordering};

use jvid_core::history::{Command, Created, History};
use jvid_core::model::{NewClip, TrackKind};
use jvid_core::time::Rate;
use jvid_media::decode::VideoDecoder;
use jvid_media::test_media::{corpus, frame_index};

use super::*;
use crate::export::Lossless;
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

fn retry_request(name: &str) -> Request {
    let mut p = preset("YouTube 1080p");
    p.size = None;
    p.audio = None;
    Request {
        path: out(name),
        range: (Time(0), secs(1)),
        preset: p,
        loudness: None,
        lossless: None,
        sidecar: false,
    }
}

fn with_context<T: Send + 'static>(work: impl FnOnce(&Ctx) -> T + Send + 'static) -> T {
    let pool = Pool::new(1);
    let (tx, rx) = channel();
    let _handle = pool.submit("injected export", Priority::Interactive, move |ctx| {
        tx.send(work(ctx)).ok();
    });
    rx.recv().unwrap()
}

#[test]
fn in_progress_hardware_failure_replays_fresh_software_output() {
    jvid_media::ffmpeg::init().unwrap();
    let mut req = retry_request("retry-fresh.mkv");
    req.preset.audio = Some(jvid_core::preset::AudioCodec::Aac);
    let destination = req.path.clone();
    let unrelated = req.path.with_extension("mkv.best");
    std::fs::write(&destination, b"previous output").unwrap();
    std::fs::write(&unrelated, b"not this attempt").unwrap();
    let original_options = req.preset.options.clone();
    let (result, paths) = with_context(move |ctx| {
        let mut paths = Vec::new();
        let result = replay(&req, ctx, |attempt| {
            if let Some(previous) = paths.last() {
                assert!(
                    !Path::new(previous).exists(),
                    "failed attempt already removed"
                );
            }
            paths.push(attempt.path.clone());
            assert_eq!(std::fs::read(&req.path).unwrap(), b"previous output");
            if paths.len() == 2 {
                assert_eq!(
                    attempt.preset.options.get("encoder").map(String::as_str),
                    Some("software")
                );
            }
            // Use a real software encoder on every host; inject a VAAPI failure after frames
            // were submitted. The second attempt must encode from frame zero into a fresh file.
            let mut p = attempt.preset.clone();
            p.options.insert("encoder".into(), "software".into());
            let mut encoder =
                Encoder::create(&attempt.path, (64, 64), Rate::new(30, 1), &p).unwrap();
            for i in 0..10 {
                encoder.video(&vec![(i * 20) as u8; 64 * 64 * 4])?;
                encoder
                    .audio(&vec![i as f32 / 20.0; 1600 * 2])
                    .map_err(err)?;
                if paths.len() == 1 && i == 3 {
                    std::fs::write(
                        attempt.path.with_extension("mkv.best"),
                        b"saved partial pass",
                    )
                    .unwrap();
                    return Err(EncodeError::Vaapi((-5).into()).into());
                }
            }
            encoder.finish()?;
            Ok(())
        });
        assert_eq!(
            req.preset.options, original_options,
            "request settings are unchanged"
        );
        (result, paths)
    });
    result.unwrap();
    assert_eq!(paths.len(), 2);
    for path in paths {
        assert!(
            !path.parent().unwrap().exists(),
            "attempt directory cleaned up"
        );
    }
    let info = jvid_media::probe::probe(&destination).unwrap();
    assert_eq!((info.width, info.height), (Some(64), Some(64)));
    assert_eq!((info.audio_streams, info.sample_rate), (1, Some(48_000)));
    let rate = Rate::new(30, 1);
    assert!((info.duration - rate.time_floor(10)).0.abs() <= rate.time_floor(1).0);
    let mut decoder = VideoDecoder::open(&destination).unwrap();
    let origin = decoder.frame_at(Time(0)).unwrap().pts;
    let mut previous = None;
    for i in 0..10 {
        // MKV rounds timestamps to milliseconds and shifts video by AAC's priming delay.
        let want = origin + rate.time_floor(i);
        let frame = decoder
            .frame_at(want + Time(FLICKS_PER_SECOND / 1000))
            .unwrap();
        assert!(
            (frame.pts - want).0.abs() <= FLICKS_PER_SECOND / 1000,
            "replayed frame {i}"
        );
        let value = frame.planes[0][0];
        if let Some(previous) = previous {
            assert!(value > previous, "replayed frame {i}");
        }
        previous = Some(value);
    }
    drop(decoder);
    let mut audio = AudioDecoder::open(&destination, 0).unwrap();
    let mut samples = 0;
    while let Some(chunk) = audio.next_chunk().unwrap() {
        samples += chunk.samples.len() / 2;
    }
    assert!(
        (16_000..=18_048).contains(&samples),
        "fresh audio samples: {samples}"
    );
    drop(audio);
    assert_eq!(std::fs::read(&unrelated).unwrap(), b"not this attempt");
    std::fs::remove_file(destination).unwrap();
    std::fs::remove_file(unrelated).unwrap();
}

#[test]
fn hardware_retry_is_bounded_and_unrelated_errors_do_not_retry() {
    with_context(|ctx| {
        for (label, failure, software, expected) in [
            (
                "hardware",
                AttemptError::Vaapi(EncodeError::Vaapi((-5).into())),
                false,
                2,
            ),
            (
                "software",
                AttemptError::Vaapi(EncodeError::Vaapi((-5).into())),
                true,
                1,
            ),
            (
                "cancelled",
                AttemptError::Export(Error::Cancelled),
                false,
                1,
            ),
            (
                "io",
                AttemptError::from(EncodeError::Other((-5).into())),
                false,
                1,
            ),
            (
                "render",
                AttemptError::from("render failed".to_string()),
                false,
                1,
            ),
            (
                "format",
                AttemptError::from("invalid output format".to_string()),
                false,
                1,
            ),
        ] {
            let mut req = retry_request(&format!("retry-{label}.mp4"));
            if software {
                req.preset
                    .options
                    .insert("encoder".into(), "software".into());
            }
            std::fs::write(&req.path, b"keep").unwrap();
            let mut calls = 0;
            let mut attempt_paths = Vec::new();
            let result: Result<(), _> = replay(&req, ctx, |attempt| {
                calls += 1;
                attempt_paths.push(attempt.path.clone());
                std::fs::write(&attempt.path, b"partial").unwrap();
                Err(match &failure {
                    AttemptError::Vaapi(e) => AttemptError::Vaapi(*e),
                    AttemptError::Export(e) => AttemptError::Export(e.clone()),
                })
            });
            assert!(result.is_err(), "{label}");
            if label == "cancelled" {
                assert_eq!(result, Err(Error::Cancelled));
            }
            assert_eq!(calls, expected, "{label}");
            assert_eq!(std::fs::read(&req.path).unwrap(), b"keep", "{label}");
            assert!(attempt_paths.iter().all(|p| !p.parent().unwrap().exists()));
            std::fs::remove_file(&req.path).unwrap();
        }
    });
}

#[test]
fn cancellation_during_hardware_failure_prevents_retry() {
    let req = retry_request("retry-cancel-race.mp4");
    std::fs::write(&req.path, b"keep").unwrap();
    let destination = req.path.clone();
    let pool = Pool::new(1);
    let (ready_tx, ready_rx) = channel();
    let (continue_tx, continue_rx) = channel();
    let (done_tx, done_rx) = channel();
    let handle = pool.submit("cancel hardware retry", Priority::Interactive, move |ctx| {
        let mut calls = 0;
        let result: Result<(), _> = replay(&req, ctx, |attempt| {
            calls += 1;
            assert_eq!(calls, 1);
            std::fs::write(&attempt.path, b"partial").unwrap();
            ready_tx.send(()).unwrap();
            continue_rx.recv().unwrap();
            Err(EncodeError::Vaapi((-5).into()).into())
        });
        done_tx.send((result, calls)).unwrap();
    });
    ready_rx.recv().unwrap();
    handle.cancel();
    continue_tx.send(()).unwrap();
    assert_eq!(done_rx.recv().unwrap(), (Err(Error::Cancelled), 1));
    assert_eq!(std::fs::read(&destination).unwrap(), b"keep");
    std::fs::remove_file(destination).unwrap();
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
            lossless: None,
            sidecar: false,
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

/// The dialog's In to Out range exports exactly the marked frames: In at 1 s, Out at 2.5 s.
#[test]
fn in_to_out_exports_the_marked_range() {
    let (Some(dir), Some(_)) = (corpus(), device()) else {
        return eprintln!("skipping export test: needs the corpus and a software adapter");
    };
    let mut h = History::new(project(&dir));
    let (in_point, out_point) = (Some(secs(1)), Some(half(5)));
    h.apply(Command::SetInOut {
        in_point,
        out_point,
    })
    .unwrap();
    let seq = h.project().sequence();
    let range = dialog::span(dialog::Range::InOut, seq, &Default::default()).unwrap();
    assert_eq!(range, (secs(1), half(5)));
    let path = out("in-out.mp4");
    let req = Request {
        path: path.clone(),
        range,
        preset: preset("YouTube 1080p"),
        loudness: None,
        lossless: None,
        sidecar: false,
    };
    let info = run_now(h.project(), &req, false).unwrap();
    let frame = Rate::new(30, 1).time_floor(1);
    assert!(
        (info.duration - half(3)).0.abs() <= frame.0,
        "{:?}",
        info.duration
    );
    let mut v = VideoDecoder::open(&path).unwrap();
    let index = |f: &jvid_media::decode::Frame| frame_index(&f.planes[0], f.width as usize);
    let (first, last) = (v.frame_at(Time(0)).unwrap(), v.frame_at(secs(10)).unwrap());
    assert_eq!((index(&first), index(&last)), (30, 74));
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn ten_highlights_export_as_ten_trimmed_files() {
    let (Some(dir), Some(_)) = (corpus(), device()) else {
        return eprintln!("skipping export test: needs the corpus and a software adapter");
    };
    let mut h = History::new(project(&dir));
    // Half-second highlights every 0.9 s from 0.5 s: frames 15+27i to 29+27i.
    for i in 0..10 {
        let time = Time(half(1).0 + i * 9 * FLICKS_PER_SECOND / 10);
        let (duration, name) = (half(1), format!("h{i}"));
        h.apply(Command::AddMarker {
            time,
            duration,
            name,
        })
        .unwrap();
    }
    let seq = h.project().sequence();
    let chosen = dialog::chosen(seq, &Default::default());
    assert_eq!(chosen.len(), 10);
    let frame = Rate::new(30, 1).time_floor(1);
    let index = |f: &jvid_media::decode::Frame| frame_index(&f.planes[0], f.width as usize);
    for (i, m) in chosen.iter().enumerate() {
        let name = dialog::file_name(dialog::TEMPLATE, "hl", m, i + 1);
        let path = out(&format!("{name}.mp4"));
        let req = Request {
            path: path.clone(),
            range: (m.time, m.time + m.duration),
            preset: preset("YouTube 1080p"),
            loudness: None,
            lossless: None,
            sidecar: false,
        };
        let info = run_now(h.project(), &req, false).unwrap();
        assert!((info.duration - half(1)).0.abs() <= frame.0, "{name}");
        let mut v = VideoDecoder::open(&path).unwrap();
        let (first, last) = (v.frame_at(Time(0)).unwrap(), v.frame_at(secs(10)).unwrap());
        let start = 15 + 27 * i as u32;
        assert_eq!((index(&first), index(&last)), (start, start + 14), "{name}");
        std::fs::remove_file(&path).unwrap();
    }
}

#[test]
fn cancelled_export_leaves_no_file() {
    let (Some(dir), Some(_)) = (corpus(), device()) else {
        return eprintln!("skipping export test: needs the corpus and a software adapter");
    };
    let path = out("cancel.mp4");
    let _ = std::fs::remove_file(&path);
    let req = Request {
        path: path.clone(),
        range: (Time(0), secs(10)),
        preset: preset("YouTube 1080p"),
        loudness: None,
        lossless: None,
        sidecar: false,
    };
    assert_eq!(run_now(&project(&dir), &req, true), Err(Error::Cancelled));
    assert!(!path.exists());
}

#[test]
fn cancelled_export_preserves_existing_file() {
    let Some(_) = device() else { return };
    let req = retry_request("cancel-keep.mp4");
    std::fs::write(&req.path, b"keep").unwrap();
    assert_eq!(
        run_now(&Project::default(), &req, true),
        Err(Error::Cancelled)
    );
    assert_eq!(std::fs::read(&req.path).unwrap(), b"keep");
    std::fs::remove_file(req.path).unwrap();
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
        lossless: None,
        sidecar: false,
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
        let size = preset.output_size((1920, 1080));
        let has_video = preset.video.is_some();
        let name = preset.name.clone();
        let req = Request {
            path: path.clone(),
            range: (secs(1), secs(2)),
            preset,
            loudness: None,
            lossless: None,
            sidecar: false,
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
        lossless: None,
        sidecar: false,
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
            lossless: None,
            sidecar: false,
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

/// Audio-only WAV of 1 s to 3 s holds exactly the mixer's samples for that range, at 16 and 24
/// bits: each decoded sample is the mix quantized the way the encoder does.
#[test]
fn audio_only_wav_is_the_mix() {
    let (Some(dir), Some(_)) = (corpus(), device()) else {
        return eprintln!("skipping export test: needs the corpus and a software adapter");
    };
    let p = project(&dir);
    for (name, bits) in [("Audio only (WAV)", 16), ("Audio only (WAV 24-bit)", 24)] {
        let path = out(&format!("mix{bits}.wav"));
        let req = Request {
            path: path.clone(),
            range: (secs(1), secs(3)),
            preset: preset(name),
            loudness: None,
            lossless: None,
            sidecar: false,
        };
        let info = run_now(&p, &req, false).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!((info.width, info.audio_streams), (None, 1), "{name}");
        let mut want = Vec::new();
        let mut audio = Audio::new(Box::new(blocking));
        for (_, time, a, b) in spans(&p, &req) {
            let mut mix = vec![0.0; (b - a) as usize * 2];
            audio.prime(&p, time);
            audio.mix(&p, a, &mut mix);
            want.extend(mix);
        }
        let mut got = Vec::new();
        let mut a = AudioDecoder::open(&path, 0).unwrap();
        while let Some(c) = a.next_chunk().unwrap() {
            got.extend(c.samples);
        }
        assert_eq!(got.len(), want.len(), "{name}");
        let scale = (1i64 << (bits - 1)) as f64;
        let q = |s: f32| (s.clamp(-1.0, 1.0) as f64 * (scale - 1.0)) as i64;
        let d = |s: f32| (s as f64 * scale).round() as i64;
        let diff = got.iter().zip(&want).position(|(g, w)| d(*g) != q(*w));
        assert_eq!(diff, None, "{name}: first differing sample");
        assert!(
            want.iter().any(|s| s.abs() > 0.02),
            "{name}: the range has the tone"
        );
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

/// A job queued behind a busy pool still renders the project as it was when queued.
#[test]
fn editing_after_queueing_doesnt_change_the_export() {
    let (Some(dir), Some(gpu)) = (corpus(), device()) else {
        return eprintln!("skipping export test: needs the corpus and a software adapter");
    };
    let mut h = History::new(project(&dir));
    let pool = Pool::new(1);
    let (go, wait) = channel::<()>();
    pool.submit("busy", Priority::Interactive, move |_| {
        let _ = wait.recv();
    });
    let path = out("snapshot.mp4");
    let req = Request {
        path: path.clone(),
        range: (secs(1), secs(2)),
        preset: preset("YouTube 1080p"),
        loudness: None,
        lossless: None,
        sidecar: false,
    };
    let mut q = queue::Queue::default();
    assert!(q.push(
        path.clone(),
        submit(&pool, h.project().clone(), req.clone(), gpu)
    ));
    assert!(!q.writing(&out("other.mp4")) && q.writing(&path));
    let video = h.project().sequence().tracks()[0].clips()[0].id;
    h.apply(Command::RemoveClip(video)).unwrap();
    go.send(()).unwrap();
    let r = loop {
        if let Some((_, r)) = q.poll().pop() {
            break r;
        }
        std::thread::yield_now();
    };
    assert_eq!(r, Ok(()));
    assert!(!q.writing(&path));
    let mut v = VideoDecoder::open(&path).unwrap();
    let f = v.frame_at(Time(0)).unwrap();
    assert_eq!(frame_index(&f.planes[0], f.width as usize), 30);
    std::fs::remove_file(&path).unwrap();
}

/// Cancelling a job before it starts reports it cancelled, not crashed.
#[test]
fn queue_reports_cancelled_waiting_job() {
    let pool = Pool::new(1);
    let (go, wait) = channel::<()>();
    pool.submit("busy", Priority::Interactive, move |_| {
        let _ = wait.recv();
    });
    let (tx, rx) = channel::<Result<ProbeInfo, Error>>();
    let h = pool.submit("job", Priority::Interactive, move |_| drop(tx));
    let mut q = queue::Queue::default();
    assert!(q.push("a.mp4".into(), (h.clone(), rx)));
    assert_eq!(q.running(), Some(0.0));
    h.cancel();
    go.send(()).unwrap();
    while !h.is_finished() {
        std::thread::yield_now();
    }
    assert_eq!(q.poll(), vec![("a.mp4".into(), Err(Error::Cancelled))]);
    assert_eq!((q.running(), q.pending()), (None, 0));
}

/// Target-size exports land under the target and within 3% of it, across corpus sources of
/// different content and sizes, for H.264 in MP4, MKV and MOV. The near-static screen recording
/// needs less than the target at the encoder's best quality, so it only has to land under.
#[test]
fn target_size_lands_within_three_percent() {
    let (Some(dir), Some(_)) = (corpus(), device()) else {
        return eprintln!("skipping export test: needs the corpus and a software adapter");
    };
    for (src, ext, target, fill) in [
        ("frame-index-1080p30.mp4", "mp4", 2_500_000, 0.97),
        ("frame-index-1080p30.mp4", "mkv", 4_000_000, 0.97),
        ("9x16.mp4", "mp4", 2_000_000, 0.97),
        ("vfr-phone.mp4", "mov", 1_500_000, 0.97),
        ("screen-1440p60.mp4", "mp4", 1_000_000, 0.0),
    ] {
        let mut h = History::new(Project::default());
        add(&mut h, TrackKind::Video, dir.join(src), Time(0), secs(2));
        add(
            &mut h,
            TrackKind::Audio,
            dir.join("tone-1khz.wav"),
            Time(0),
            secs(2),
        );
        let mut preset = preset("YouTube 1080p");
        preset.extension = ext.into();
        preset
            .options
            .insert("target_size".into(), target.to_string());
        let path = out(&format!("target.{ext}"));
        let req = Request {
            path: path.clone(),
            range: (Time(0), secs(2)),
            preset,
            loudness: None,
            lossless: None,
            sidecar: false,
        };
        run_now(h.project(), &req, false).unwrap_or_else(|e| panic!("{src}: {e:?}"));
        let size = std::fs::metadata(&path).unwrap().len();
        let off = size as f64 / target as f64;
        assert!(
            (fill..=1.0).contains(&off),
            "{src} {ext}: {size} of {target}"
        );
        assert!(!PathBuf::from(format!("{}.best", path.display())).exists());
        std::fs::remove_file(&path).unwrap();
    }
}

#[test]
fn next_bits_closes_in_on_the_target() {
    // One pass: in proportion to the miss, at most 2x.
    assert_eq!(next_bits(None, Some((1_000_000, 200.0)), 100.0), 500_000);
    assert_eq!(next_bits(Some((1_000_000, 10.0)), None, 100.0), 2_000_000);
    // Both sides: along their line, here 1 byte per 10 kb/s.
    let (lo, hi) = (Some((1_000_000, 100.0)), Some((2_000_000, 200.0)));
    assert_eq!(next_bits(lo, hi, 150.0), 1_500_000);
    // Never right at either end, which a flat stretch would otherwise give.
    assert_eq!(next_bits(lo, hi, 100.0), 1_100_000);
}

#[test]
fn lossless_copies_the_marked_cut() {
    let (Some(dir), Some(_)) = (corpus(), device()) else {
        return eprintln!("skipping export test: needs the corpus and a software adapter");
    };
    let src = dir.join("frame-index-1080p30.mp4");
    let mut h = History::new(Project::default());
    add(&mut h, TrackKind::Video, src.clone(), Time(0), secs(10));
    let id = h.project().assets()[0].id;
    let probe = Some(jvid_media::probe::probe(&src).unwrap());
    h.apply(Command::SetAssetProbe { id, probe }).unwrap();
    let path = out("lossless.mov");
    // Keyframes every second, so the cut snaps to 1-4 s.
    let mut req = Request {
        path: path.clone(),
        range: (
            secs(1) + Time(FLICKS_PER_SECOND / 3),
            secs(4) - Time(FLICKS_PER_SECOND / 3),
        ),
        preset: preset("YouTube 1080p"),
        loudness: None,
        lossless: Some(Lossless::Copy),
        sidecar: false,
    };
    let err = run_now(h.project(), &req, false).unwrap_err();
    assert!(format!("{err:?}").contains(".mp4"), "{err:?}");
    req.path = path.with_extension("mp4");
    let info = run_now(h.project(), &req, false).unwrap();
    std::fs::remove_file(&req.path).unwrap();
    assert_eq!(info.duration, secs(3));
}

/// Two queued exports both show in the Export menu, each with a right-aligned Cancel; Cancel on
/// one leaves the other, and Cancel All (confirmed, as two are live) stops both.
#[test]
fn export_menu_cancels_one_or_all() {
    use egui_kittest::kittest::Queryable;
    let pool = Pool::new(1);
    let (go, wait) = channel::<()>();
    pool.submit("busy", Priority::Interactive, move |_| {
        let _ = wait.recv();
    });
    let mut q = queue::Queue::default();
    let mut handles = Vec::new();
    for name in ["a.mp4", "b.mp4"] {
        let (tx, rx) = channel::<Result<ProbeInfo, Error>>();
        let h = pool.submit(name, Priority::Interactive, move |_| drop(tx));
        handles.push(h.clone());
        assert!(q.push(name.into(), (h, rx)));
    }
    let mut fresh = true;
    let mut harness = egui_kittest::Harness::builder()
        .with_size([480.0, 200.0])
        .build_ui_state(
            move |ui, q: &mut queue::Queue| {
                if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                    q.menu(ui);
                }
            },
            q,
        );
    harness.run_steps(2);
    harness.get_by_label("a.mp4");
    harness.get_by_label("b.mp4");
    let rights: Vec<f32> = harness
        .get_all_by_label("Cancel")
        .map(|n| n.rect().max.x)
        .collect();
    assert_eq!(rights.len(), 2);
    let all = harness.get_by_label("Cancel All").rect();
    assert!(
        rights[0] == rights[1] && rights[0] > all.max.x,
        "{rights:?}"
    );

    harness.get_all_by_label("Cancel").next().unwrap().click();
    harness.run();
    assert!(handles[0].is_cancelled() && !handles[1].is_cancelled());

    // Both still count as live until the pool skips them, so Cancel All asks first.
    harness.get_by_label("Cancel All").click();
    harness.run();
    assert!(!handles[1].is_cancelled());
    harness.get_by_label("Cancel 2 Exports?");
    harness.get_by_label("Cancel All").click();
    harness.run();
    assert!(handles.iter().all(|h| h.is_cancelled()));

    go.send(()).unwrap();
    while !handles.iter().all(|h| h.is_finished()) {
        std::thread::yield_now();
    }
    let q = harness.state_mut();
    assert_eq!(q.poll().len(), 2);
    q.clear_finished();
    assert!(q.is_empty());
}

#[test]
fn built_in_preset_names_are_their_english_catalog_names() {
    let all = jvid_core::preset::builtin();
    assert_eq!(all.len(), BUILT_IN_NAMES.len());
    for (p, key) in all.iter().zip(BUILT_IN_NAMES) {
        assert_eq!(crate::i18n::CATALOG.get_in("en", key), Some(&*p.name));
        assert!(crate::i18n::CATALOG.get_in("es", key).is_some(), "{key}");
    }
    let mine = Preset {
        name: "Mine".into(),
        ..all[0].clone()
    };
    assert_eq!(preset_name(all.len(), &mine), "Mine");
}
