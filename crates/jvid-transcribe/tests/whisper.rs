//! Real-model tests: they download the base model (148 MB) into the data folder on first run.
//! `cargo test -p jvid-transcribe -- --ignored`

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use jvid_jobs::Pool;
use jvid_transcribe::models::{self, Model};
use jvid_transcribe::{Options, Source, Whisper, Word};

/// The public-domain JFK sample from whisper.cpp, as Opus so it also goes through FFmpeg decode.
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/jfk.opus")
}

fn whisper(jobs: &Pool) -> Arc<Whisper> {
    // Tests run in parallel; only one downloads.
    static DOWNLOAD: Mutex<()> = Mutex::new(());
    let _one = DOWNLOAD.lock().unwrap_or_else(|e| e.into_inner());
    jvid_media::ffmpeg::init().unwrap();
    let dir = models::dir().unwrap();
    if !Model::Base.is_downloaded(&dir) {
        let (tx, rx) = mpsc::channel();
        Model::Base.download(jobs, &dir, move |r| tx.send(r).unwrap());
        rx.recv_timeout(Duration::from_secs(600)).unwrap().unwrap();
    }
    Arc::new(Whisper::load(&Model::Base.path(&dir)).unwrap())
}

fn source() -> Source {
    Source {
        path: fixture(),
        nth: 0,
        range: 0..i64::MAX,
    }
}

/// Acceptance: a short speech fixture transcribes to the expected words with the smallest model.
#[test]
#[ignore = "downloads a model"]
fn jfk_transcribes_with_base_model() {
    let jobs = Pool::new(2);
    let engine = whisper(&jobs);
    let (tx, rx) = mpsc::channel();
    let opts = Options {
        language: Some("en".into()),
        ..Options::default()
    };
    jvid_transcribe::submit(&jobs, engine, source(), opts, move |r| tx.send(r).unwrap());
    let words: Vec<Word> = rx.recv_timeout(Duration::from_secs(120)).unwrap().unwrap();
    let text: Vec<String> = words
        .iter()
        .map(|w| {
            w.text
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .collect();
    let want = "and so my fellow americans ask not what your country can do for you \
                ask what you can do for your country";
    assert_eq!(text.join(" "), want, "{words:#?}");
    let secs = |w: &Word| w.start.0 as f64 / jvid_core::time::FLICKS_PER_SECOND as f64;
    // Words are in order, inside the 11 s clip, and mostly confident.
    assert!(words.windows(2).all(|p| p[0].start <= p[1].start));
    assert!(words.iter().all(|w| w.start <= w.end && secs(w) < 11.0));
    let mean = words.iter().map(|w| w.confidence).sum::<f32>() / words.len() as f32;
    assert!(mean > 0.7, "mean confidence {mean}");
    // "country" closes the clip, in its last few seconds.
    assert!(secs(words.last().unwrap()) > 8.0, "{words:#?}");
}

/// Acceptance: Cancel stops within a second, mid-inference.
#[test]
#[ignore = "downloads a model"]
fn cancel_stops_whisper_within_a_second() {
    let jobs = Pool::new(2);
    let engine = whisper(&jobs);
    let h = jvid_transcribe::submit(&jobs, engine, source(), Options::default(), |_| {
        panic!("done is not called after cancel")
    });
    // Decoding the fixture takes milliseconds; give whisper a moment to be mid-encode.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!h.is_finished(), "finished before it could be cancelled");
    let at = Instant::now();
    h.cancel();
    while !h.is_finished() {
        assert!(at.elapsed() < Duration::from_secs(1), "still running");
        std::thread::sleep(Duration::from_millis(5));
    }
    eprintln!("cancelled in {:?}", at.elapsed());
}
