//! Gated decoder tests through the actual bin and timeline paint consumers.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use super::*;
use crate::media_bin::filmstrip_tests::BinConsumer;

const TIMEOUT: Duration = Duration::from_secs(5);

struct Grab {
    finish: Sender<Result<(), String>>,
}

struct Fixture {
    started: Receiver<Grab>,
    bin: BinConsumer,
    media: Media,
    caches: Arc<Caches>,
    grabs: Arc<AtomicUsize>,
    asset: MediaAsset,
    ctx: egui::Context,
    ui_time: f64,
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("jvid-filmstrip-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("source.mov");
        std::fs::write(&path, b"source generation A").unwrap();
        let grabs = Arc::new(AtomicUsize::new(0));
        let count = grabs.clone();
        let gated = Arc::new(AtomicBool::new(false));
        let gate = gated.clone();
        let (start, started) = mpsc::channel();
        let thumbs = Arc::new(thumbnails::Cache::with_grab(
            dir.join("cache"),
            u64::MAX,
            Box::new(move |_, _, _| {
                if gate.load(Ordering::SeqCst) {
                    count.fetch_add(1, Ordering::SeqCst);
                    let (finish, wait) = mpsc::channel();
                    start.send(Grab { finish }).map_err(|e| e.to_string())?;
                    wait.recv_timeout(TIMEOUT).expect("grab gate timed out")?;
                }
                Ok(image::RgbaImage::new(thumbnails::WIDTH, 90))
            }),
        ));
        let duration = Time(3 * FLICKS_PER_SECOND);
        thumbs.poster(&path, duration).unwrap();
        let mut caches = Caches::new(&jvid_core::settings::Settings {
            cache_dir: Some(dir.clone()),
            ..Default::default()
        });
        Arc::get_mut(&mut caches).unwrap().thumbs = thumbs;
        Arc::get_mut(&mut caches).unwrap().jobs = jvid_jobs::Pool::new(1);
        let mut fixture = Self {
            started,
            bin: BinConsumer::new(caches.clone()),
            media: Media {
                caches: Some(caches.clone()),
                ..Default::default()
            },
            caches,
            grabs,
            asset: MediaAsset {
                id: AssetId(1),
                path,
                probe: Some(ProbeInfo {
                    duration,
                    width: Some(160),
                    height: Some(90),
                    ..Default::default()
                }),
                name: None,
                in_point: None,
                out_point: None,
                solid: None,
                text: None,
                sequence: None,
            },
            ctx: egui::Context::default(),
            ui_time: 0.0,
            dir,
        };
        // Finish the separate poster job before gating filmstrip grabs. In particular, source
        // replacement must not turn an unrelated queued poster into a blocked fake filmstrip.
        let mut output = fixture.ctx.run_ui(Default::default(), |ui| {
            fixture.bin.poster(ui, &fixture.asset)
        });
        output.textures_delta.clear();
        fixture.idle();
        let mut output = fixture.ctx.run_ui(Default::default(), |ui| {
            fixture.bin.poster(ui, &fixture.asset)
        });
        output.textures_delta.clear();
        gated.store(true, Ordering::SeqCst);
        fixture
    }

    fn paint(&mut self, bin: bool, timeline: bool) {
        self.paint_clips(bin, timeline, 1);
    }

    fn paint_clips(&mut self, bin: bool, timeline: bool, clips: usize) -> Duration {
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                time: Some(self.ui_time),
                ..Default::default()
            },
            |ui| {
                if bin {
                    self.bin.paint(ui, &self.asset);
                }
                for _ in 0..if timeline { clips } else { 0 } {
                    self.media.filmstrip(
                        ui,
                        ui.painter(),
                        Rect::from_min_size(pos2(0.0, 100.0), vec2(480.0, 90.0)),
                        0.0,
                        &self.asset,
                        &|x| Time((x / 160.0) as i64 * FLICKS_PER_SECOND),
                    );
                }
            },
        );
        let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        output.textures_delta.clear();
        delay
    }

    fn grab(&self) -> Grab {
        self.started
            .recv_timeout(TIMEOUT)
            .expect("decode did not start")
    }

    fn block_pool(&self) -> Sender<()> {
        let (release, wait) = mpsc::channel();
        let (started, ready) = mpsc::channel();
        self.caches.jobs.submit(
            "refresh-gate",
            jvid_jobs::Priority::Interactive,
            move |_| {
                started.send(()).unwrap();
                wait.recv_timeout(TIMEOUT).expect("refresh gate timed out");
            },
        );
        ready.recv_timeout(TIMEOUT).unwrap();
        release
    }

    fn idle(&self) {
        let deadline = Instant::now() + TIMEOUT;
        while !self.caches.jobs.is_idle() || !self.caches.filmstrips.settled() {
            assert!(Instant::now() < deadline, "jobs never completed");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn finish(&self) {
        for _ in 0..3 {
            self.grab().finish.send(Ok(())).unwrap();
        }
        self.idle();
    }

    fn frames(&self) -> (usize, usize) {
        (
            self.bin.frames(&self.asset.path),
            self.media
                .strips
                .get(&self.asset.path)
                .and_then(Option::as_ref)
                .map_or(0, Vec::len),
        )
    }

    fn assert_terminal(&self) {
        assert_eq!(self.bin.pending(), 0);
        assert_eq!(self.media.strip_requests.pending(), 0);
        assert_eq!(
            self.caches.filmstrips.retained(),
            (0, 0),
            "CPU replay frames or subscribers leaked"
        );
    }

    fn assert_failed(&self) {
        self.assert_terminal();
        assert!(!self.bin.contains(&self.asset.path));
        assert!(!self.media.strips.contains_key(&self.asset.path));
    }

    fn retry(&mut self) {
        self.ui_time += crate::media_bin::filmstrips::RETRY_DELAY.as_secs_f64() + 1.0;
        self.paint(true, true);
        self.finish();
        self.paint(true, true);
        assert_eq!(self.frames(), (3, 3));
        self.assert_terminal();
    }

    fn strip_files(&self) -> usize {
        std::fs::read_dir(self.dir.join("cache"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".strip.png"))
            .count()
    }
}

#[test]
fn repeated_pending_paints_do_no_metadata_reads_or_new_requests() {
    let mut f = Fixture::new("pending-hotpath");
    f.paint(true, true);
    let first = f.grab();
    let reads = crate::media_bin::filmstrips::metadata_reads();
    let worker_reads = f.caches.filmstrips.metadata_reads();
    let checks = crate::media_bin::filmstrips::generation_checks();
    let requests = f.caches.filmstrips.requests_started();
    let refreshes = f.caches.filmstrips.refreshes_started();
    let mut repaint_delay = Duration::ZERO;
    for _ in 0..10 {
        repaint_delay = f.paint_clips(true, true, 500);
    }
    let worker_reads_after = f.caches.filmstrips.metadata_reads();
    // Release the gate before asserting, so a failed regression cannot strand the worker.
    first.finish.send(Ok(())).unwrap();
    for _ in 0..2 {
        f.grab().finish.send(Ok(())).unwrap();
    }
    f.idle();
    assert_eq!(crate::media_bin::filmstrips::metadata_reads(), reads);
    assert_eq!(worker_reads_after, worker_reads);
    assert_eq!(
        crate::media_bin::filmstrips::generation_checks() - checks,
        20
    );
    assert_eq!(f.caches.filmstrips.requests_started(), requests);
    assert_eq!(f.caches.filmstrips.refreshes_started(), refreshes);
    assert_eq!(
        repaint_delay,
        Duration::MAX,
        "pending strips keep repainting"
    );
}

#[test]
fn non_hovered_bin_tiles_never_request_filmstrip_metadata() {
    let mut f = Fixture::new("non-hovered-hotpath");
    let reads = crate::media_bin::filmstrips::metadata_reads();
    for _ in 0..10 {
        let mut output = f.ctx.run_ui(Default::default(), |ui| {
            for _ in 0..500 {
                f.bin.poster(ui, &f.asset);
            }
        });
        output.textures_delta.clear();
    }
    assert_eq!(crate::media_bin::filmstrips::metadata_reads(), reads);
    assert_eq!(f.caches.filmstrips.metadata_reads(), 0);
    assert_eq!(f.caches.filmstrips.requests_started(), 0);
    assert_eq!(f.caches.filmstrips.refreshes_started(), 0);
    assert_eq!(f.grabs.load(Ordering::SeqCst), 0);
}

#[test]
fn repeated_completed_paints_do_no_metadata_reads_or_new_requests() {
    let mut f = Fixture::new("completed-hotpath");
    f.paint(true, true);
    f.finish();
    f.paint(true, true);
    let reads = crate::media_bin::filmstrips::metadata_reads();
    let worker_reads = f.caches.filmstrips.metadata_reads();
    let checks = crate::media_bin::filmstrips::generation_checks();
    let requests = f.caches.filmstrips.requests_started();
    let refreshes = f.caches.filmstrips.refreshes_started();
    let mut repaint_delay = Duration::ZERO;
    for _ in 0..10 {
        repaint_delay = f.paint_clips(true, true, 500);
    }
    // Leaving hover is also paint-only. Keep the non-hovered tiles stable across frames.
    for _ in 0..10 {
        let mut output = f.ctx.run_ui(Default::default(), |ui| {
            for _ in 0..500 {
                f.bin.poster(ui, &f.asset);
            }
        });
        output.textures_delta.clear();
    }
    assert_eq!(crate::media_bin::filmstrips::metadata_reads(), reads);
    assert_eq!(f.caches.filmstrips.metadata_reads(), worker_reads);
    assert_eq!(
        crate::media_bin::filmstrips::generation_checks() - checks,
        30
    );
    assert_eq!(f.caches.filmstrips.requests_started(), requests);
    assert_eq!(f.caches.filmstrips.refreshes_started(), refreshes);
    assert_eq!(
        repaint_delay,
        Duration::MAX,
        "completed strips keep repainting"
    );
    assert_eq!(f.grabs.load(Ordering::SeqCst), 3);
    f.assert_terminal();
}

#[test]
fn explicit_source_refresh_replaces_both_completed_consumers() {
    let mut f = Fixture::new("completed-replacement");
    f.paint(true, true);
    f.finish();
    f.paint(true, true);
    f.assert_terminal();
    let before = (
        f.bin.texture(&f.asset.path),
        f.media.strips[&f.asset.path].as_ref().unwrap()[0].id(),
    );
    let reads = crate::media_bin::filmstrips::metadata_reads();
    std::fs::write(
        &f.asset.path,
        b"a longer completed replacement generation B",
    )
    .unwrap();
    // Both panels use this same explicit refresh boundary. Even if the first check
    // finishes immediately, a second caller in this pass must not enqueue another stat.
    for _ in 0..2 {
        f.caches
            .filmstrips
            .refresh_source(&f.caches, &f.asset.path, &f.ctx);
        f.idle();
    }
    assert_eq!(crate::media_bin::filmstrips::metadata_reads(), reads);
    assert_eq!(f.caches.filmstrips.refreshes_started(), 1);
    f.paint(true, true);
    assert_eq!(f.frames(), (0, 0), "old textures survived invalidation");
    f.finish();
    f.paint(true, true);
    assert_eq!(f.frames(), (3, 3));
    assert_ne!(f.bin.texture(&f.asset.path), before.0);
    assert_ne!(
        f.media.strips[&f.asset.path].as_ref().unwrap()[0].id(),
        before.1
    );
    assert_eq!(f.grabs.load(Ordering::SeqCst), 6);
    assert_eq!(f.caches.filmstrips.requests_started(), 2);
    f.assert_terminal();
}

#[test]
fn hover_reentry_checks_completed_source_and_reloads_only_after_replacement() {
    let mut f = Fixture::new("hover-replacement");
    f.paint(true, true);
    f.finish();
    f.paint(true, true);
    for replaced in [false, true] {
        if replaced {
            std::fs::write(&f.asset.path, b"a longer hover replacement generation B").unwrap();
        }
        let reads = crate::media_bin::filmstrips::metadata_reads();
        // Hold the refresh until both real consumers have painted the old generation.
        let release = f.block_pool();
        let mut output = f
            .ctx
            .run_ui(Default::default(), |ui| f.bin.poster(ui, &f.asset));
        output.textures_delta.clear();
        f.paint(true, true);
        assert_eq!(f.frames(), (3, 3));
        release.send(()).unwrap();
        f.idle();
        assert_eq!(crate::media_bin::filmstrips::metadata_reads(), reads);
        assert_eq!(
            f.caches.filmstrips.refreshes_started(),
            if replaced { 2 } else { 1 }
        );
        f.paint(true, true);
        if replaced {
            assert_eq!(f.frames(), (0, 0));
            f.finish();
            f.paint(true, true);
        }
        assert_eq!(f.frames(), (3, 3));
        assert_eq!(f.grabs.load(Ordering::SeqCst), if replaced { 6 } else { 3 });
        assert_eq!(
            f.caches.filmstrips.requests_started(),
            if replaced { 2 } else { 1 }
        );
        f.assert_terminal();
    }
}

#[test]
fn focus_return_checks_shared_completed_source_once_and_refreshes_both_panels() {
    let mut f = Fixture::new("focus-replacement");
    f.paint(true, true);
    f.finish();
    f.paint(true, true);
    std::fs::write(
        &f.asset.path,
        b"a longer focus-return replacement generation B",
    )
    .unwrap();
    let reads = crate::media_bin::filmstrips::metadata_reads();
    for focused in [false, true] {
        let mut output = f.ctx.run_ui(
            egui::RawInput {
                focused,
                ..Default::default()
            },
            |_ui| {
                f.bin.on_focus(&f.ctx);
                f.media
                    .strip_requests
                    .on_focus(&f.ctx, f.media.caches.as_deref());
            },
        );
        output.textures_delta.clear();
    }
    f.idle();
    assert_eq!(crate::media_bin::filmstrips::metadata_reads(), reads);
    assert_eq!(f.caches.filmstrips.refreshes_started(), 1);
    f.paint(true, true);
    assert_eq!(f.frames(), (0, 0));
    f.finish();
    f.paint(true, true);
    assert_eq!(f.frames(), (3, 3));
    assert_eq!(f.grabs.load(Ordering::SeqCst), 6);
    f.assert_terminal();
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn bin_first_delivers_to_both_consumers() {
    let mut f = Fixture::new("bin-first");
    f.paint(true, false);
    let first = f.grab();
    f.paint(true, true);
    first.finish.send(Ok(())).unwrap();
    for _ in 0..2 {
        f.grab().finish.send(Ok(())).unwrap();
    }
    f.idle();
    f.paint(true, true);
    assert_eq!(f.frames(), (3, 3));
    assert_eq!(f.grabs.load(Ordering::SeqCst), 3);
}

#[test]
fn timeline_first_delivers_to_both_consumers() {
    let mut f = Fixture::new("timeline-first");
    f.paint(false, true);
    let first = f.grab();
    f.paint(true, true);
    first.finish.send(Ok(())).unwrap();
    for _ in 0..2 {
        f.grab().finish.send(Ok(())).unwrap();
    }
    f.idle();
    f.paint(true, true);
    assert_eq!(f.frames(), (3, 3));
    assert_eq!(f.grabs.load(Ordering::SeqCst), 3);
}

#[test]
fn late_subscribers_replay_incremental_frames_in_either_order() {
    for bin_first in [true, false] {
        let mut f = Fixture::new(if bin_first {
            "late-timeline"
        } else {
            "late-bin"
        });
        f.paint(bin_first, !bin_first);
        f.grab().finish.send(Ok(())).unwrap();
        let second = f.grab(); // First frame has been published; second cannot finish yet.
        f.paint(true, true);
        assert_eq!(f.frames(), (1, 1));
        assert_eq!(f.caches.filmstrips.retained(), (1, 2));
        for _ in 0..10 {
            f.paint(true, true);
        }
        assert_eq!(
            f.caches.filmstrips.retained(),
            (1, 2),
            "paint duplicated subscriptions"
        );
        second.finish.send(Ok(())).unwrap();
        f.grab().finish.send(Ok(())).unwrap();
        f.idle();
        f.paint(true, true);
        assert_eq!(f.frames(), (3, 3));
        assert_eq!(f.grabs.load(Ordering::SeqCst), 3);
        f.assert_terminal();
    }
}

#[test]
fn subscriber_after_completion_gets_frames_and_terminal_without_another_job() {
    let mut f = Fixture::new("late-completed");
    f.paint(true, false);
    f.finish();
    // The bin has not consumed its completion, so its result is still replayable.
    assert_eq!(f.caches.filmstrips.retained(), (3, 1));
    f.paint(false, true);
    assert_eq!(f.frames(), (0, 3));
    assert_eq!(f.media.strip_requests.pending(), 0);
    assert!(
        f.caches.jobs.is_idle(),
        "late completed subscription queued another job"
    );
    f.paint(true, true);
    assert_eq!(f.frames(), (3, 3));
    assert_eq!(f.grabs.load(Ordering::SeqCst), 3);
    f.assert_terminal();
}

#[test]
fn failure_clears_partial_results_and_both_panels_retry() {
    let mut f = Fixture::new("failure-retry");
    f.paint(true, true);
    f.grab().finish.send(Ok(())).unwrap();
    let second = f.grab();
    f.paint(true, true);
    assert_eq!(f.frames(), (1, 1));
    second
        .finish
        .send(Err("gated decoder failure".into()))
        .unwrap();
    f.idle();
    f.paint(true, true);
    f.assert_failed();
    assert_eq!(f.strip_files(), 0);
    for _ in 0..10 {
        f.paint(true, true);
    }
    assert_eq!(
        f.grabs.load(Ordering::SeqCst),
        2,
        "failure immediately resubmitted work"
    );
    f.retry();
    assert_eq!(f.grabs.load(Ordering::SeqCst), 5);
    assert_eq!(f.strip_files(), 1);
}

#[test]
fn queued_cancellation_delivers_terminal_and_allows_retry() {
    let mut f = Fixture::new("queued-cancel");
    let (release, wait) = mpsc::channel();
    let (started, ready) = mpsc::channel();
    f.caches
        .jobs
        .submit("block", jvid_jobs::Priority::Interactive, move |_| {
            started.send(()).unwrap();
            wait.recv_timeout(TIMEOUT).expect("block gate timed out");
        });
    ready.recv_timeout(TIMEOUT).unwrap();
    f.paint(true, true);
    f.caches.filmstrips.job(&f.asset.path).cancel();
    release.send(()).unwrap();
    f.idle();
    f.paint(true, true);
    f.assert_failed();
    assert_eq!(f.grabs.load(Ordering::SeqCst), 0);
    f.retry();
    assert_eq!(f.grabs.load(Ordering::SeqCst), 3);
}

#[test]
fn cancellation_during_progress_drops_partial_frames_and_skips_disk_write() {
    let mut f = Fixture::new("progress-cancel");
    f.paint(true, true);
    f.grab().finish.send(Ok(())).unwrap();
    let second = f.grab();
    f.paint(true, true);
    assert_eq!(f.frames(), (1, 1));
    f.caches.filmstrips.job(&f.asset.path).cancel();
    second.finish.send(Ok(())).unwrap();
    f.idle();
    f.paint(true, true);
    f.assert_failed();
    assert_eq!(f.grabs.load(Ordering::SeqCst), 2);
    assert_eq!(f.strip_files(), 0);
    f.retry();
    assert_eq!(f.grabs.load(Ordering::SeqCst), 5);
}

#[test]
fn dropping_one_subscriber_keeps_the_other_live() {
    let mut f = Fixture::new("drop-one");
    f.paint(true, true);
    let first = f.grab();
    let job = f.caches.filmstrips.job(&f.asset.path);
    f.bin = BinConsumer::new(f.caches.clone());
    assert!(!job.is_cancelled());
    assert_eq!(f.caches.filmstrips.retained(), (0, 1));
    first.finish.send(Ok(())).unwrap();
    for _ in 0..2 {
        f.grab().finish.send(Ok(())).unwrap();
    }
    f.idle();
    f.paint(false, true);
    assert_eq!(f.frames(), (0, 3));
    f.assert_terminal();
    f.paint(true, false);
    f.idle();
    f.paint(true, false);
    assert_eq!(f.frames(), (3, 3));
    assert_eq!(f.grabs.load(Ordering::SeqCst), 3);
}

#[test]
fn last_subscriber_cancellation_serializes_rejoining_consumers_and_retry() {
    let mut f = Fixture::new("drop-all");
    f.paint(true, true);
    f.grab().finish.send(Ok(())).unwrap();
    let second = f.grab();
    let job = f.caches.filmstrips.job(&f.asset.path);
    f.bin = BinConsumer::new(f.caches.clone());
    f.media = Media {
        caches: Some(f.caches.clone()),
        ..Default::default()
    };
    assert!(job.is_cancelled());
    assert_eq!(f.caches.filmstrips.retained(), (1, 0));
    // Rejoin before the cancelled grab returns: reuse its terminal delivery, not a second writer.
    f.paint(true, true);
    assert_eq!(f.caches.filmstrips.retained(), (1, 2));
    assert_eq!(f.grabs.load(Ordering::SeqCst), 2);
    second.finish.send(Ok(())).unwrap();
    f.idle();
    f.paint(true, true);
    f.assert_failed();
    assert_eq!(f.strip_files(), 0);
    f.retry();
    assert_eq!(f.grabs.load(Ordering::SeqCst), 5);
    assert_eq!(f.strip_files(), 1);
}

#[test]
fn hover_only_and_timeline_only_reuse_disk_cache_after_cpu_results_are_released() {
    for bin in [true, false] {
        let mut f = Fixture::new(if bin { "cached-bin" } else { "cached-timeline" });
        f.paint(bin, !bin);
        f.finish();
        f.paint(bin, !bin);
        assert_eq!(f.frames(), if bin { (3, 0) } else { (0, 3) });
        f.assert_terminal();
        f.bin = BinConsumer::new(f.caches.clone());
        f.media = Media {
            caches: Some(f.caches.clone()),
            ..Default::default()
        };
        f.paint(bin, !bin);
        f.idle();
        f.paint(bin, !bin);
        assert_eq!(f.frames(), if bin { (3, 0) } else { (0, 3) });
        assert_eq!(
            f.grabs.load(Ordering::SeqCst),
            3,
            "warm cache decoded again"
        );
        assert_eq!(f.strip_files(), 1);
        f.assert_terminal();
    }
}

#[test]
fn changed_source_drops_stale_incremental_results_and_reloads_both_panels() {
    let mut f = Fixture::new("generation");
    f.paint(true, true);
    f.grab().finish.send(Ok(())).unwrap();
    let second = f.grab();
    f.paint(true, true);
    assert_eq!(f.frames(), (1, 1));
    std::fs::write(&f.asset.path, b"a longer replacement generation B").unwrap();
    second.finish.send(Ok(())).unwrap();
    f.idle();
    assert_eq!(
        f.strip_files(),
        0,
        "old generation was published after replacement"
    );
    f.paint(true, true);
    f.finish();
    f.paint(true, true);
    assert_eq!(f.frames(), (3, 3));
    assert_eq!(f.grabs.load(Ordering::SeqCst), 5);
    f.assert_terminal();
}
