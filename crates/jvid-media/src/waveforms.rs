//! Min/max peak pyramids for drawing waveforms at any zoom, cached on disk next to the
//! thumbnails (same key and LRU cap, see [`crate::thumbnails`]).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use jvid_jobs::{Handle, Priority};

use crate::audio::AudioDecoder;
use crate::thumbnails::{evict, key};

/// Sample frames per finest-level peak; each level above doubles it.
pub const BUCKET: i64 = 256;
/// Bump when the peak format changes so old entries miss.
const VERSION: u32 = 1;
/// How often a scan in progress hands out what it has so far.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// Lowest and highest sample (over both channels) in a span.
pub type Peak = [f32; 2];

/// A cancelled scan is not corrupt media and must not publish a cache entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("waveform scan cancelled"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ScanError {}

impl From<String> for ScanError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl From<std::io::Error> for ScanError {
    fn from(error: std::io::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

/// Returns the finest-level peaks, polling cancellation between decoded chunks and before
/// handing partial results to the callback.
pub type Scan = dyn Fn(&Path, usize, &dyn Fn() -> bool, &mut dyn FnMut(&[Peak])) -> Result<Vec<Peak>, ScanError>
    + Send
    + Sync;

fn check_cancelled(cancelled: &dyn Fn() -> bool) -> Result<(), ScanError> {
    if cancelled() {
        Err(ScanError::Cancelled)
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pyramid {
    /// `levels[l][i]` covers sample frames `[i, i + 1) * (BUCKET << l)`.
    levels: Vec<Vec<Peak>>,
}

impl Pyramid {
    pub fn new(finest: Vec<Peak>) -> Self {
        let mut levels = vec![finest];
        while levels.last().unwrap().len() > 1 {
            let next = levels.last().unwrap().chunks(2).map(merge).collect();
            levels.push(next);
        }
        Self { levels }
    }

    /// Peaks of [`BUCKET`] sample frames each, the full resolution scanned.
    pub fn finest(&self) -> &[Peak] {
        &self.levels[0]
    }

    /// One peak per pixel for `width` pixels spanning sample frames `[start, end)`. Pixels past
    /// the end of the audio are `[0, 0]`.
    pub fn query(&self, start: i64, end: i64, width: usize) -> Vec<Peak> {
        let spp = (end - start) as f64 / width.max(1) as f64;
        // The coarsest level whose peaks are no wider than a pixel, so each pixel reads 1–3.
        let l = (0..self.levels.len())
            .rev()
            .find(|&l| (BUCKET << l) as f64 <= spp)
            .unwrap_or(0);
        query(&self.levels[l], BUCKET << l, start, end, width)
    }
}

/// [`Pyramid::query`] on one level whose peaks span `size` sample frames each, such as a
/// recording's finest peaks while they grow.
pub fn query(level: &[Peak], size: i64, start: i64, end: i64, width: usize) -> Vec<Peak> {
    let spp = (end - start) as f64 / width.max(1) as f64;
    (0..width)
        .map(|x| {
            let lo = start + (x as f64 * spp) as i64;
            let hi = (start + ((x + 1) as f64 * spp) as i64).max(lo + 1);
            let a = lo.div_euclid(size).clamp(0, level.len() as i64) as usize;
            let b = (hi + size - 1)
                .div_euclid(size)
                .clamp(0, level.len() as i64) as usize;
            if a >= b {
                [0.0; 2]
            } else {
                merge(&level[a..b])
            }
        })
        .collect()
}

fn merge(p: &[Peak]) -> Peak {
    p.iter().fold([f32::MAX, f32::MIN], |m, p| {
        [m[0].min(p[0]), m[1].max(p[1])]
    })
}

pub struct Cache {
    dir: PathBuf,
    cap_bytes: u64,
    scan: Box<Scan>,
}

impl Cache {
    /// A cache in `dir` (the thumbnail cache's) decoding through [`AudioDecoder`].
    pub fn new(dir: PathBuf, cap_bytes: u64) -> Self {
        Self::with_scan(dir, cap_bytes, Box::new(scan))
    }

    pub fn with_scan(dir: PathBuf, cap_bytes: u64, scan: Box<Scan>) -> Self {
        Self {
            dir,
            cap_bytes,
            scan,
        }
    }

    /// The peak pyramid of audio stream `nth` of `path`, from disk when cached.
    pub fn load(&self, path: &Path, nth: usize) -> Result<Pyramid, String> {
        self.load_with(path, nth, |_| {})
    }

    /// [`Self::load`], handing a pyramid of the audio scanned so far to `progress` every
    /// [`PROGRESS_EVERY`] when not cached, so a long recording's waveform fills in as it scans.
    pub fn load_with(
        &self,
        path: &Path,
        nth: usize,
        progress: impl FnMut(Pyramid),
    ) -> Result<Pyramid, String> {
        self.load_cancellable(path, nth, || false, progress)
            .map_err(|error| error.to_string())
    }

    /// Like [`Self::load_with`], stopping cooperatively without publishing cancelled work.
    pub fn load_cancellable(
        &self,
        path: &Path,
        nth: usize,
        cancelled: impl Fn() -> bool,
        mut progress: impl FnMut(Pyramid),
    ) -> Result<Pyramid, ScanError> {
        check_cancelled(&cancelled)?;
        let file = self
            .dir
            .join(format!("{}.{nth}.v{VERSION}.peaks", key(path)?));
        if let Ok(bytes) = fs::read(&file) {
            check_cancelled(&cancelled)?;
            // The file's mtime is its last use, for LRU eviction.
            let _ = fs::File::options()
                .write(true)
                .open(&file)
                .and_then(|f| f.set_modified(SystemTime::now()));
            let f: Vec<f32> = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect();
            check_cancelled(&cancelled)?;
            let pyramid = Pyramid::new(f.as_chunks::<2>().0.to_vec());
            check_cancelled(&cancelled)?;
            return Ok(pyramid);
        }
        check_cancelled(&cancelled)?;
        let mut last = Instant::now();
        let finest = (self.scan)(path, nth, &cancelled, &mut |partial| {
            if !cancelled() && last.elapsed() >= PROGRESS_EVERY {
                let pyramid = Pyramid::new(partial.to_vec());
                if !cancelled() {
                    progress(pyramid);
                }
                last = Instant::now();
            }
        })?;
        check_cancelled(&cancelled)?;
        let pyramid = Pyramid::new(finest);
        check_cancelled(&cancelled)?;
        match self.store(&file, pyramid.finest(), &cancelled) {
            Err(ScanError::Cancelled) => return Err(ScanError::Cancelled),
            Err(error) => tracing::warn!("waveforms: can't write {}: {error}", file.display()),
            Ok(()) => {}
        }
        Ok(pyramid)
    }

    /// Loads on `jobs` at preview priority and hands the pyramid to `done` there: partial ones
    /// while scanning, then the whole one or the error.
    pub fn submit(
        self: &Arc<Self>,
        jobs: &jvid_jobs::Pool,
        path: PathBuf,
        nth: usize,
        mut done: impl FnMut(Result<Pyramid, String>) + Send + 'static,
    ) -> Handle {
        let cache = self.clone();
        let key = format!("peaks:{}:{nth}", path.display());
        jobs.submit(key, Priority::Preview, move |ctx| {
            let result = cache.load_cancellable(
                &path,
                nth,
                || ctx.is_cancelled(),
                |p| {
                    if !ctx.is_cancelled() {
                        done(Ok(p));
                    }
                },
            );
            if !ctx.is_cancelled() {
                match result {
                    Ok(pyramid) => done(Ok(pyramid)),
                    Err(ScanError::Failed(error)) => done(Err(error)),
                    Err(ScanError::Cancelled) => {}
                }
            }
        })
    }

    fn store(
        &self,
        file: &Path,
        finest: &[Peak],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), ScanError> {
        check_cancelled(cancelled)?;
        fs::create_dir_all(&self.dir)?;
        let bytes: Vec<u8> = finest
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        // Write then rename, so a crash never leaves a half-written entry.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let tmp = file.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        check_cancelled(cancelled)?;
        let mut output = fs::File::options()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        let tmp = Pending(tmp);
        let written = (|| -> Result<(), ScanError> {
            for chunk in bytes.chunks(64 * 1024) {
                check_cancelled(cancelled)?;
                output.write_all(chunk)?;
            }
            Ok(())
        })();
        drop(output);
        written?;
        check_cancelled(cancelled)?;
        fs::rename(&tmp.0, file)?;
        evict(&self.dir, self.cap_bytes);
        Ok(())
    }
}

/// Removes only the temporary entry owned by this write, including after cancellation.
struct Pending(PathBuf);

impl Drop for Pending {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn scan(
    path: &Path,
    nth: usize,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(&[Peak]),
) -> Result<Vec<Peak>, ScanError> {
    check_cancelled(cancelled)?;
    let mut d = AudioDecoder::open(path, nth).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    loop {
        check_cancelled(cancelled)?;
        let Some(c) = d.next_chunk().map_err(|e| e.to_string())? else {
            break;
        };
        check_cancelled(cancelled)?;
        for (i, f) in c.samples.as_chunks::<2>().0.iter().enumerate() {
            let b = ((c.start + i as i64) / BUCKET) as usize;
            if b >= out.len() {
                out.resize(b + 1, [f32::MAX, f32::MIN]);
            }
            let p = &mut out[b];
            *p = [p[0].min(f[0]).min(f[1]), p[1].max(f[0]).max(f[1])];
        }
        check_cancelled(cancelled)?;
        progress(&out);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, mpsc};
    use std::time::{Duration, Instant};

    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jvid-peaks-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Peak `i` is `[i, i]`, so a span's merge is its first and last index.
    fn ramp(n: usize) -> Vec<Peak> {
        (0..n).map(|i| [i as f32; 2]).collect()
    }

    fn counting(dir: PathBuf, cap: u64) -> (Cache, Arc<AtomicUsize>) {
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        let scan = Box::new(
            move |_: &Path, _, _: &dyn Fn() -> bool, _: &mut dyn FnMut(&[Peak])| {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ramp(1000))
            },
        );
        (Cache::with_scan(dir, cap, scan), n)
    }

    #[test]
    fn cancelled_job_stops_at_next_chunk_without_publishing_and_can_retry() {
        let dir = tmp("cancel-job");
        let source = dir.join("source.wav");
        fs::write(&source, b"fake source").unwrap();
        let cache_dir = dir.join("cache");
        let steps = Arc::new(AtomicUsize::new(0));
        let scanned = steps.clone();
        let (started, start) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let resumed = Mutex::new(resumed);
        let scanner = Box::new(
            move |_: &Path, _, cancelled: &dyn Fn() -> bool, progress: &mut dyn FnMut(&[Peak])| {
                let mut peaks = Vec::new();
                for _ in 0..64 {
                    check_cancelled(cancelled)?;
                    if scanned.fetch_add(1, Ordering::SeqCst) == 0 {
                        started.send(()).unwrap();
                        resumed
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(10))
                            .unwrap();
                    }
                    peaks.push([-0.1, 0.1]);
                    check_cancelled(cancelled)?;
                    progress(&peaks);
                }
                Ok(peaks)
            },
        );
        let cache = Arc::new(Cache::with_scan(cache_dir.clone(), u64::MAX, scanner));
        let calls = Arc::new(AtomicUsize::new(0));
        let published = calls.clone();
        let pool = jvid_jobs::Pool::new(1);
        let handle = cache.submit(&pool, source.clone(), 0, move |_| {
            published.fetch_add(1, Ordering::SeqCst);
        });
        start.recv_timeout(Duration::from_secs(10)).unwrap();
        handle.cancel();
        resume.send(()).unwrap();
        // Shutdown joins the job; the gate makes remaining work deterministic, without timing
        // assertions that depend on runner speed.
        drop(pool);
        assert!(handle.is_finished());
        assert_eq!(steps.load(Ordering::SeqCst), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!cache_dir.exists());
        assert_eq!(cache.load(&source, 0).unwrap().finest().len(), 64);
        assert_eq!(steps.load(Ordering::SeqCst), 65);
        assert_eq!(fs::read_dir(&cache_dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancellation_during_cache_write_removes_owned_temporary_file() {
        let dir = tmp("cancel-write");
        let (cache, _) = counting(dir.clone(), u64::MAX);
        let file = dir.join("finished.peaks");
        let result = cache.store(&file, &ramp(10_000), &|| {
            fs::read_dir(&dir).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "tmp")
            })
        });
        assert_eq!(result, Err(ScanError::Cancelled));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancellation_is_distinct_from_bad_media_and_preserves_existing_cache() {
        let dir = tmp("cancel-hit");
        let source = dir.join("source.wav");
        fs::write(&source, b"fake source").unwrap();
        let (cache, scans) = counting(dir.join("cache"), u64::MAX);
        let expected = cache.load(&source, 0).unwrap();
        let result = cache.load_cancellable(&source, 0, || true, |_| panic!("cancelled progress"));
        assert_eq!(result, Err(ScanError::Cancelled));
        assert_eq!(cache.load(&source, 0).unwrap(), expected);
        assert_eq!(scans.load(Ordering::SeqCst), 1);
        let bad = Cache::new(dir.join("bad"), u64::MAX);
        assert!(matches!(
            bad.load_cancellable(&source, 0, || false, |_| {}),
            Err(ScanError::Failed(_))
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn real_audio_cancellation_publishes_no_cache_and_can_retry() {
        let Some(corpus) = crate::test_media::corpus() else {
            return;
        };
        crate::ffmpeg::init().unwrap();
        let dir = tmp("cancel-audio");
        let cache = Cache::new(dir.join("cache"), u64::MAX);
        let polls = Cell::new(0);
        let result = cache.load_cancellable(
            &corpus.join("tone-1khz.wav"),
            0,
            || {
                polls.set(polls.get() + 1);
                polls.get() >= 8
            },
            |_| panic!("cancelled audio published progress"),
        );
        assert_eq!(result, Err(ScanError::Cancelled));
        assert!(!cache.dir.exists());
        let finished = cache.load(&corpus.join("tone-1khz.wav"), 0).unwrap();
        assert!(finished.finest().iter().any(|p| p[1] > 0.09));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn query_matches_brute_force_at_every_zoom() {
        let finest = ramp(1000);
        let p = Pyramid::new(finest.clone());
        let total = 1000 * BUCKET;
        for (start, end, w) in [
            (0, total, 7),
            (0, total, 1000),
            (BUCKET * 3, BUCKET * 900, 50),
        ] {
            let got = p.query(start, end, w);
            for (x, peak) in got.iter().enumerate() {
                let lo = start + (end - start) * x as i64 / w as i64;
                let hi = start + (end - start) * (x as i64 + 1) / w as i64;
                let spp = (end - start) / w as i64;
                // Covers the pixel's samples, overshooting by at most one pixel each side.
                assert!(peak[1] >= ((hi - 1) / BUCKET) as f32, "{x}: {peak:?}");
                assert!(peak[1] <= ((hi + spp) / BUCKET) as f32, "{x}: {peak:?}");
                assert!(peak[0] <= (lo / BUCKET) as f32, "{x}: {peak:?}");
                assert!(peak[0] >= ((lo - spp) / BUCKET) as f32, "{x}: {peak:?}");
            }
        }
        // Zoomed past the finest level, and past the end.
        assert_eq!(p.query(0, BUCKET, 4), vec![[0.0, 0.0]; 4]);
        assert_eq!(p.query(total, total + BUCKET, 2), vec![[0.0; 2]; 2]);
    }

    #[test]
    fn scan_progress_is_throttled_and_partial() {
        let d = tmp("progress");
        let a = d.join("a.wav");
        fs::write(&a, b"x").unwrap();
        // Grows by 100 peaks every 100 ms (or more, on a loaded machine), short of the whole.
        let scan = Box::new(
            |_: &Path, _, _: &dyn Fn() -> bool, progress: &mut dyn FnMut(&[Peak])| {
                let all = ramp(1000);
                for n in (100..1000).step_by(100) {
                    std::thread::sleep(Duration::from_millis(100));
                    progress(&all[..n]);
                }
                Ok(all)
            },
        );
        let c = Cache::with_scan(d.join("cache"), u64::MAX, scan);
        let mut partials = Vec::new();
        let mut at = Vec::new();
        let full = c
            .load_with(&a, 0, |p| {
                partials.push(p);
                at.push(Instant::now());
            })
            .unwrap();
        // Throttled by time, not by count, since sleeps overshoot on slow CI runners.
        assert!(partials.len() >= 2, "{}", partials.len());
        assert!(
            at.windows(2).all(|w| w[1] - w[0] >= PROGRESS_EVERY),
            "{at:?}"
        );
        // Each partial reads like the finished pyramid over what it has, and silent past it
        // (but for the pixel either side of its end, which may overshoot).
        for p in &partials {
            let n = p.levels[0].len() as i64;
            assert!(n < 1000);
            // The last pixel may overshoot into audio only the finished one has.
            let (a, b) = (p.query(0, n * BUCKET, 10), full.query(0, n * BUCKET, 10));
            assert_eq!(a[..9], b[..9]);
            assert_eq!(p.query(n * BUCKET, 1000 * BUCKET, 3)[1..], [[0.0; 2]; 2]);
        }
        // Cached: no partials.
        c.load_with(&a, 0, |_| panic!("cached")).unwrap();
    }

    #[test]
    fn second_load_hits_disk_cache_with_zero_decodes() {
        let d = tmp("hit");
        let a = d.join("a.wav");
        fs::write(&a, b"x").unwrap();
        let (first, n) = counting(d.join("cache"), u64::MAX);
        let made = first.load(&a, 0).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 1);
        let (second, n) = counting(d.join("cache"), u64::MAX);
        assert_eq!(second.load(&a, 0).unwrap(), made);
        assert_eq!(n.load(Ordering::SeqCst), 0);
        // Another stream of the same file is its own entry.
        second.load(&a, 1).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn size_or_mtime_change_invalidates() {
        let d = tmp("inval");
        let p = d.join("a.wav");
        fs::write(&p, b"one").unwrap();
        let (c, n) = counting(d.join("cache"), u64::MAX);
        c.load(&p, 0).unwrap();
        c.load(&p, 0).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 1);
        fs::write(&p, b"longer").unwrap();
        c.load(&p, 0).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 2);
        let f = fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        c.load(&p, 0).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn shares_the_lru_cap() {
        let d = tmp("lru");
        let cache = d.join("cache");
        let paths: Vec<_> = (0..3u8)
            .map(|i| {
                let p = d.join(format!("{i}.wav"));
                fs::write(&p, [i]).unwrap();
                p
            })
            .collect();
        // Room for two entries.
        let (c, n) = counting(cache.clone(), 2 * 1000 * 8);
        for p in &paths {
            c.load(p, 0).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 2);
        c.load(&paths[0], 0).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 4);
    }

    /// Acceptance: drawing a 1-hour waveform at any zoom takes under 1 ms per frame.
    #[test]
    fn one_hour_query_under_1ms_at_any_zoom() {
        if cfg!(debug_assertions) {
            eprintln!("skipping timing: run with --release");
            return;
        }
        let total = 3600 * crate::audio::SAMPLE_RATE as i64;
        let finest = (0..total / BUCKET)
            .map(|i| [-((i % 97) as f32) / 97.0, (i % 89) as f32 / 89.0])
            .collect();
        let p = Pyramid::new(finest);
        let s = crate::audio::SAMPLE_RATE as i64;
        for span in [total, 600 * s, 60 * s, s, s / 100] {
            let start = (total - span) / 2;
            let t = Instant::now();
            let mut out = p.query(start, start + span, 2000);
            for _ in 0..9 {
                out = p.query(start, start + span, 2000);
            }
            let each = t.elapsed() / 10;
            assert_eq!(out.len(), 2000);
            eprintln!("span {span} samples: {each:?} per 2000 px query");
            assert!(each < Duration::from_millis(1), "span {span}: {each:?}");
        }
    }

    #[test]
    fn tone_peaks_at_its_level() {
        let Some(dir) = crate::test_media::corpus() else {
            return;
        };
        crate::ffmpeg::init().unwrap();
        let c = Cache::new(tmp("tone"), u64::MAX);
        let p = c.load(&dir.join("tone-1khz.wav"), 0).unwrap();
        let s = crate::audio::SAMPLE_RATE as i64;
        for peak in p.query(s, 4 * s, 30) {
            assert!((peak[1] - 0.1).abs() < 0.005, "{peak:?}");
            assert!((peak[0] + 0.1).abs() < 0.005, "{peak:?}");
        }
    }
}
