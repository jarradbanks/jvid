//! Real-model tests: they download the base model (148 MB) into the data folder on first run.
//! `cargo test -p jvid-transcribe -- --ignored`

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use jvid_jobs::Pool;
use jvid_transcribe::models::{self, Model};
use jvid_transcribe::{Options, Source, Whisper, Word};

/// The public-domain JFK sample from whisper.cpp, as Opus so it also goes through FFmpeg decode:
/// <https://github.com/ggml-org/whisper.cpp/blob/master/samples/jfk.wav>
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/jfk.opus")
}

/// The base model, and a guard that keeps these tests apart: only one downloads, and the cancel
/// test counts whisper threads.
fn whisper(jobs: &Pool) -> (MutexGuard<'static, ()>, Arc<Whisper>) {
    static ONE: Mutex<()> = Mutex::new(());
    let one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    jvid_media::ffmpeg::init().unwrap();
    let dir = models::dir().unwrap();
    if !Model::Base.is_downloaded(&dir) {
        let (tx, rx) = mpsc::channel();
        Model::Base.download(jobs, &dir, move |r| tx.send(r).unwrap());
        rx.recv_timeout(Duration::from_secs(600)).unwrap().unwrap();
    }
    (
        one,
        Arc::new(Whisper::load(&Model::Base.path(&dir)).unwrap()),
    )
}

fn source() -> Source {
    Source {
        path: fixture(),
        nth: 0,
        range: 0..i64::MAX,
    }
}

/// Acceptance: a short speech fixture transcribes to the expected words with the smallest model,
/// and its language is detected.
#[test]
#[ignore = "downloads a model"]
fn jfk_transcribes_with_base_model() {
    let jobs = Pool::new(2);
    let (_one, engine) = whisper(&jobs);
    let (tx, rx) = mpsc::channel();
    jvid_transcribe::submit(&jobs, engine, source(), Options::default(), move |r| {
        tx.send(r).unwrap()
    });
    let t = rx.recv_timeout(Duration::from_secs(120)).unwrap().unwrap();
    assert_eq!(t.language.as_deref(), Some("en"));
    let words: Vec<Word> = t.words;
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
    let (_one, engine) = whisper(&jobs);
    let h = jvid_transcribe::submit(&jobs, engine, source(), Options::default(), |_| {
        panic!("done is not called after cancel")
    });
    // Decoding the fixture takes milliseconds; cancel once whisper is running. Its progress stays
    // at 0 through a clip this short, so watch for its thread.
    let start = Instant::now();
    while !cfg!(target_os = "linux") && start.elapsed() < Duration::from_millis(300)
        || cfg!(target_os = "linux") && whisper_threads() == 0
    {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "whisper never started"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!h.is_finished(), "finished before it could be cancelled");
    let at = Instant::now();
    h.cancel();
    while !h.is_finished() {
        assert!(at.elapsed() < Duration::from_secs(1), "still running");
        std::thread::sleep(Duration::from_millis(5));
    }
    eprintln!("cancelled in {:?}", at.elapsed());
    // Whisper itself stops at its next graph boundary, up to one encoder pass later: about 8 s for
    // base on a 6-core box at load 15.
    if cfg!(target_os = "linux") {
        while whisper_threads() > 0 {
            assert!(
                at.elapsed() < Duration::from_secs(30),
                "whisper still running"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        eprintln!("whisper's thread gone in {:?}", at.elapsed());
    }
}

/// Live `jvid-whisper` threads in this process (Linux only).
fn whisper_threads() -> usize {
    std::fs::read_dir("/proc/self/task")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|t| {
            std::fs::read_to_string(t.path().join("comm")).is_ok_and(|c| c.trim() == "jvid-whisper")
        })
        .count()
}
