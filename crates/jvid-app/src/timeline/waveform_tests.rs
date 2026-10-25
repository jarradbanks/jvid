//! Real waveform-cache / job-pool regressions for Detect Silence's source refresh.

use std::fs;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use jvid_jobs::{Pool, Priority};
use jvid_media::audio::SAMPLE_RATE;
use jvid_media::waveforms::ScanError;

use super::*;

const TIMEOUT: Duration = Duration::from_secs(5);

struct Scan {
    source: Vec<u8>,
    stream: usize,
    finish: Sender<Result<Vec<Peak>, ScanError>>,
}

struct Fixture {
    // Drop pending gates before the pool, including when an assertion fails.
    started: Receiver<Scan>,
    timeline: Timeline,
    project: Project,
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("jvid-waveform-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("source.wav");
        fs::write(&path, b"generation A").unwrap();
        let (start, started) = mpsc::channel();
        let cache = waveforms::Cache::with_scan(
            dir.join("peaks"),
            u64::MAX,
            Box::new(move |path, stream, _, _| {
                let (finish, gate) = mpsc::channel();
                start
                    .send(Scan {
                        source: fs::read(path)?,
                        stream,
                        finish,
                    })
                    .unwrap();
                gate.recv_timeout(TIMEOUT).expect("scan gate timed out")
            }),
        );
        let caches = Arc::new(Caches {
            peaks: Arc::new(cache),
            jobs: Pool::new(1),
            thumbs: Arc::new(thumbnails::Cache::new(dir.join("thumbs"), u64::MAX)),
            proxies: Arc::new(jvid_media::proxy::Proxies::new(dir.join("proxies"))),
            transcripts: dir.join("transcripts"),
        });
        let mut history = History::new(tests::project(4));
        let asset = history.project().assets()[0].id;
        history
            .apply(Command::RelinkAssets(vec![(asset, path)]))
            .unwrap();
        let project = history.project().clone();
        let mut timeline = Timeline::new(caches);
        timeline.sync(&project);
        timeline.selected = project.sequence().tracks()[1]
            .clips()
            .iter()
            .map(|c| c.id)
            .collect();
        assert!(timeline.action("detect_silence", &project));
        Self {
            started,
            timeline,
            project,
            dir,
        }
    }

    fn path(&self) -> &std::path::Path {
        &self.project.assets()[0].path
    }

    fn spans(&mut self) -> Option<Vec<silence::Span>> {
        self.timeline
            .silence_spans(&self.project, &egui::Context::default())
    }

    fn scan(&self) -> Scan {
        self.started
            .recv_timeout(TIMEOUT)
            .expect("current source generation never received its own waveform scan")
    }

    /// Put a marker ahead of queued preview work, while the current scanner is gated.
    /// Receiving it proves the preceding job's callback and pool cleanup both finished.
    fn checkpoint(&self) -> Receiver<()> {
        let (send, received) = mpsc::channel();
        self.timeline.media.caches.as_ref().unwrap().jobs.submit(
            "waveform-test-checkpoint",
            Priority::Interactive,
            move |_| send.send(()).unwrap(),
        );
        received
    }

    fn pending(&mut self) {
        for _ in 0..10 {
            assert!(self.spans().is_none());
            assert!(self.timeline.media.peaks[&(self.path().to_owned(), 0)].is_none());
        }
    }

    fn ready(&mut self) -> Vec<silence::Span> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(spans) = self.spans() {
                return spans;
            }
            assert!(
                Instant::now() < deadline,
                "silence analysis never became ready"
            );
            std::thread::yield_now();
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn peaks(quiet: bool) -> Vec<Peak> {
    vec![if quiet { [0.0; 2] } else { [-0.5, 0.5] }; 2 * SAMPLE_RATE as usize / BUCKET as usize]
}

#[test]
fn audit_465_waveform_generation_refresh_gets_current_peaks_and_silence() {
    let mut f = Fixture::new("generation");
    assert!(f.spans().is_none());
    let a = f.scan();
    assert_eq!(a.source, b"generation A");
    assert_eq!(a.stream, 0);
    f.pending();
    fs::write(f.path(), b"generation B has a different length").unwrap();
    assert!(f.spans().is_none()); // Actual refresh_peaks + pyramid while A is running.
    f.pending();
    let checkpoint = f.checkpoint();
    a.finish.send(Ok(peaks(false))).unwrap();
    checkpoint.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(f.timeline.media.peaks_done.lock().unwrap().len(), 1);
    f.pending(); // A's real completion is rejected without publishing into B.
    let b = f.scan();
    assert_eq!(b.source, b"generation B has a different length");
    let checkpoint = f.checkpoint();
    b.finish.send(Ok(peaks(true))).unwrap();
    checkpoint.recv_timeout(TIMEOUT).unwrap();
    let spans = f.ready();
    assert_eq!(
        spans.len(),
        2,
        "both cuts receive the current silent source"
    );
    let key = (f.path().to_owned(), 0);
    let current = f.timeline.media.peaks[&key].clone().unwrap();
    assert_eq!(current.finest(), peaks(true));
    for _ in 0..10 {
        assert_eq!(f.spans().unwrap(), spans);
        assert!(Arc::ptr_eq(
            f.timeline.media.peaks[&key].as_ref().unwrap(),
            &current
        ));
    }
    assert!(
        f.started.try_recv().is_err(),
        "unchanged cuts/frames must share one scan"
    );
    // A cannot poison B's disk entry either: a fresh Timeline loads B without decoding.
    let mut fresh = Timeline::new(f.timeline.media.caches.as_ref().unwrap().clone());
    fresh.sync(&f.project);
    fresh.selected = f.timeline.selected.clone();
    assert!(fresh.action("detect_silence", &f.project));
    f.timeline = fresh;
    assert!(f.spans().is_none());
    assert_eq!(f.ready(), spans);
    assert!(f.started.try_recv().is_err());
}

#[test]
fn audit_465_waveform_generation_failure_retries_current_source() {
    let mut f = Fixture::new("generation-retry");
    assert!(f.spans().is_none());
    let a = f.scan();
    fs::write(f.path(), b"generation B has a different length").unwrap();
    assert!(f.spans().is_none());
    let checkpoint = f.checkpoint();
    a.finish
        .send(Err(ScanError::Failed("old failure".into())))
        .unwrap();
    checkpoint.recv_timeout(TIMEOUT).unwrap();
    f.pending(); // A's failure cannot remove B's pending entry and submit extra work.
    let b = f.scan();
    assert_eq!(b.source, b"generation B has a different length");
    let checkpoint = f.checkpoint();
    b.finish
        .send(Err(ScanError::Failed("retryable failure".into())))
        .unwrap();
    checkpoint.recv_timeout(TIMEOUT).unwrap();
    assert!(f.spans().is_none());
    let retry = f.scan();
    assert_eq!(retry.source, b"generation B has a different length");
    f.pending();
    let checkpoint = f.checkpoint();
    retry.finish.send(Ok(peaks(true))).unwrap();
    checkpoint.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(f.ready().len(), 2);
    assert!(f.started.try_recv().is_err());
}
