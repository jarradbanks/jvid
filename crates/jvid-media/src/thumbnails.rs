//! Poster and filmstrip thumbnails with a disk cache keyed by path, size and mtime, capped by
//! total bytes with least-recently-used eviction.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use image::{GenericImageView, ImageFormat, RgbaImage, imageops};
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use jvid_jobs::{Handle, Priority};

use crate::decode::{DecoderPool, Frame};

/// Thumbnail width in pixels; height follows the source aspect.
pub const WIDTH: u32 = 160;
/// Most filmstrip frames per asset; short clips get one per second.
pub const STRIP_MAX: i64 = 100;
/// Bump when the thumbnail format changes so old entries miss.
const VERSION: u32 = 2;

/// Filmstrip steps at least this long sample the keyframe before each step rather than the exact
/// frame: a whole GOP decode per thumbnail makes a long recording's strip take minutes, and a GOP
/// is a small part of such a step.
pub const KEYFRAME_STEP: Time = Time(10 * FLICKS_PER_SECOND);

/// Returns a `WIDTH`-wide RGBA thumbnail of `path` at `t`: the exact frame, or with `exact` false
/// the keyframe before it.
pub type Grab = dyn Fn(&Path, Time, bool) -> Result<RgbaImage, String> + Send + Sync;

pub struct Cache {
    dir: PathBuf,
    cap_bytes: u64,
    grab: Box<Grab>,
}

impl Cache {
    /// A cache in `dir` decoding through FFmpeg (videos) or `decode_still` (images).
    pub fn new(dir: PathBuf, cap_bytes: u64) -> Self {
        let pool = DecoderPool::hdr();
        Self::with_grab(
            dir,
            cap_bytes,
            Box::new(move |p, t, exact| grab(&pool, p, t, exact)),
        )
    }

    pub fn with_grab(dir: PathBuf, cap_bytes: u64, grab: Box<Grab>) -> Self {
        Self {
            dir,
            cap_bytes,
            grab,
        }
    }

    /// The poster frame: one second in, or halfway through a shorter clip.
    pub fn poster(&self, path: &Path, duration: Time) -> Result<RgbaImage, String> {
        let t = Time(FLICKS_PER_SECOND.min(duration.0 / 2).max(0));
        self.cached(path, "poster", || (self.grab)(path, t, true))
    }

    /// Frames at a fixed interval: every second, widened so there are at most [`STRIP_MAX`].
    pub fn filmstrip(&self, path: &Path, duration: Time) -> Result<Vec<RgbaImage>, String> {
        self.filmstrip_with(path, duration, |_| {})
    }

    /// [`Self::filmstrip`], handing each frame to `progress` as it's made when not cached, so a
    /// long recording's strip fills in rather than appearing all at once.
    pub fn filmstrip_with(
        &self,
        path: &Path,
        duration: Time,
        mut progress: impl FnMut(&RgbaImage),
    ) -> Result<Vec<RgbaImage>, String> {
        let step = FLICKS_PER_SECOND.max(duration.0 / STRIP_MAX);
        let n = (duration.0 / step).max(1);
        let exact = step < KEYFRAME_STEP.0;
        let strip = self.cached(path, "strip", || {
            let frames = (0..n)
                .map(|i| {
                    let f = (self.grab)(path, Time(i * step), exact)?;
                    progress(&f);
                    Ok(f)
                })
                .collect::<Result<Vec<_>, String>>()?;
            let h = frames.iter().map(|f| f.height()).max().unwrap_or(1);
            let mut strip = RgbaImage::new(WIDTH * frames.len() as u32, h);
            for (i, f) in frames.iter().enumerate() {
                imageops::overlay(&mut strip, f, i as i64 * WIDTH as i64, 0);
            }
            Ok(strip)
        })?;
        Ok((0..strip.width() / WIDTH)
            .map(|i| strip.view(i * WIDTH, 0, WIDTH, strip.height()).to_image())
            .collect())
    }

    /// Makes the poster on `jobs` at preview priority and hands it to `done` there.
    pub fn submit_poster(
        self: &Arc<Self>,
        jobs: &jvid_jobs::Pool,
        path: PathBuf,
        duration: Time,
        done: impl FnOnce(Result<RgbaImage, String>) + Send + 'static,
    ) -> Handle {
        let cache = self.clone();
        let key = format!("thumb:{}", path.display());
        jobs.submit(key, Priority::Preview, move |ctx| {
            if !ctx.is_cancelled() {
                done(cache.poster(&path, duration));
            }
        })
    }

    fn cached(
        &self,
        path: &Path,
        kind: &str,
        make: impl FnOnce() -> Result<RgbaImage, String>,
    ) -> Result<RgbaImage, String> {
        let file = self.dir.join(format!("{}.{kind}.png", key(path)?));
        if let Ok(img) = image::open(&file) {
            // The file's mtime is its last use, for LRU eviction.
            let _ = fs::File::options()
                .write(true)
                .open(&file)
                .and_then(|f| f.set_modified(SystemTime::now()));
            return Ok(img.into_rgba8());
        }
        let img = make()?;
        if let Err(e) = self.store(&file, &img) {
            tracing::warn!("thumbnails: can't write {}: {e}", file.display());
        }
        Ok(img)
    }

    fn store(&self, file: &Path, img: &RgbaImage) -> Result<(), String> {
        fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        // Write then rename, so a crash never leaves a half-written entry.
        let tmp = file.with_extension(format!("{}.tmp", std::process::id()));
        img.save_with_format(&tmp, ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        fs::rename(&tmp, file).map_err(|e| e.to_string())?;
        evict(&self.dir, self.cap_bytes);
        Ok(())
    }
}

/// Deletes least recently used entries (thumbnails and waveforms) until `dir` fits in `cap_bytes`.
pub(crate) fn evict(dir: &Path, cap_bytes: u64) {
    let Ok(dir) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = dir
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x == "png" || x == "peaks")
        })
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = entries.iter().map(|e| e.1).sum();
    entries.sort();
    for (_, len, path) in entries {
        if total <= cap_bytes {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// Changes whenever the file is moved, resized or rewritten.
// shortcut: DefaultHasher may change between Rust releases, which only costs a regeneration.
pub(crate) fn key(path: &Path) -> Result<String, String> {
    let m = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = DefaultHasher::new();
    (VERSION, path, m.len(), m.modified().ok()).hash(&mut h);
    Ok(format!("{:016x}", h.finish()))
}

fn grab(pool: &DecoderPool, path: &Path, t: Time, exact: bool) -> Result<RgbaImage, String> {
    if ImageFormat::from_path(path).is_ok() {
        let img = crate::stills::decode_still(path).map_err(|e| e.to_string())?;
        let h = (WIDTH as u64 * img.height() as u64 / img.width().max(1) as u64).max(1) as u32;
        return Ok(imageops::thumbnail(&img, WIDTH, h));
    }
    let f = if exact {
        pool.frame_at(path, t)
    } else {
        pool.keyframe_at(path, t)
    };
    let f = f.map_err(|e| e.to_string())?;
    Ok(to_rgba(&f, WIDTH))
}

/// Scale to `w` wide using the same metadata-aware SDR view as preview and proxies.
fn to_rgba(f: &Frame, w: u32) -> RgbaImage {
    let h = (w as u64 * f.height as u64 / f.width.max(1) as u64).max(1) as u32;
    f.view_rgba((w, h))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jvid-thumbs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A cache whose decoder makes flat 160×90 frames and counts its calls.
    fn counting(dir: PathBuf, cap: u64) -> (Cache, Arc<AtomicUsize>) {
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        let grab = Box::new(move |_: &Path, t: Time, _| {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(RgbaImage::from_pixel(
                WIDTH,
                90,
                image::Rgba([(t.0 / FLICKS_PER_SECOND) as u8, 0, 0, 255]),
            ))
        });
        (Cache::with_grab(dir, cap, grab), n)
    }

    const SECS: Time = Time(10 * FLICKS_PER_SECOND);

    #[test]
    fn second_load_hits_disk_cache_with_zero_decodes() {
        let d = tmp("hit");
        let assets: Vec<_> = (0..3)
            .map(|i| {
                let p = d.join(format!("clip{i}.mp4"));
                fs::write(&p, [i]).unwrap();
                p
            })
            .collect();
        let (first, n) = counting(d.join("cache"), u64::MAX);
        for a in &assets {
            first.poster(a, SECS).unwrap();
            first.filmstrip(a, SECS).unwrap();
        }
        assert_eq!(n.load(Ordering::SeqCst), 3 * 11);
        // A fresh cache over the same dir is a re-opened project.
        let (second, n) = counting(d.join("cache"), u64::MAX);
        for a in &assets {
            second.poster(a, SECS).unwrap();
            let strip = second.filmstrip(a, SECS).unwrap();
            assert_eq!(strip.len(), 10);
            assert_eq!(strip[3].get_pixel(0, 0)[0], 3);
        }
        assert_eq!(n.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn size_or_mtime_change_invalidates() {
        let d = tmp("inval");
        let p = d.join("a.mp4");
        fs::write(&p, b"one").unwrap();
        let (c, n) = counting(d.join("cache"), u64::MAX);
        c.poster(&p, SECS).unwrap();
        c.poster(&p, SECS).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 1);
        fs::write(&p, b"longer").unwrap();
        c.poster(&p, SECS).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 2);
        let f = fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        c.poster(&p, SECS).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn old_color_conversion_cache_misses() {
        let d = tmp("color-version");
        let p = d.join("clip.mp4");
        fs::write(&p, b"source remains unchanged").unwrap();
        let m = fs::metadata(&p).unwrap();
        let mut old = DefaultHasher::new();
        (1u32, p.as_path(), m.len(), m.modified().ok()).hash(&mut old);
        let old_key = format!("{:016x}", old.finish());
        assert_ne!(key(&p).unwrap(), old_key);
        let cache = d.join("cache");
        fs::create_dir_all(&cache).unwrap();
        RgbaImage::new(WIDTH, 90)
            .save(cache.join(format!("{old_key}.poster.png")))
            .unwrap();
        let (c, n) = counting(cache, u64::MAX);
        c.poster(&p, SECS).unwrap();
        assert_eq!(
            n.load(Ordering::SeqCst),
            1,
            "old conversion must regenerate"
        );
        assert_eq!(fs::read(&p).unwrap(), b"source remains unchanged");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn evicts_least_recently_used_over_the_cap() {
        let d = tmp("lru");
        let cache = d.join("cache");
        let paths: Vec<_> = (0..3)
            .map(|i| {
                let p = d.join(format!("{i}.mp4"));
                fs::write(&p, [i]).unwrap();
                p
            })
            .collect();
        let (c, _) = counting(cache.clone(), u64::MAX);
        c.poster(&paths[0], SECS).unwrap();
        let one = fs::read_dir(&cache).unwrap().next().unwrap().unwrap();
        let entry = one.metadata().unwrap().len();
        // Room for two entries.
        let (c, n) = counting(cache.clone(), entry * 2);
        std::thread::sleep(Duration::from_millis(20));
        c.poster(&paths[1], SECS).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        c.poster(&paths[0], SECS).unwrap(); // touch: 1 is now the oldest
        std::thread::sleep(Duration::from_millis(20));
        c.poster(&paths[2], SECS).unwrap();
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 2);
        assert_eq!(n.load(Ordering::SeqCst), 2);
        c.poster(&paths[0], SECS).unwrap();
        c.poster(&paths[2], SECS).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 2);
        c.poster(&paths[1], SECS).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn white_and_black_convert() {
        let f = Frame {
            pts: Time(0),
            width: 4,
            height: 2,
            planes: [
                vec![235, 235, 16, 16, 235, 235, 16, 16],
                vec![128; 2],
                vec![128; 2],
            ],
            bt709: false,
            full_range: false,
            hdr: None,
        };
        let img = to_rgba(&f, 4);
        assert_eq!(img.get_pixel(0, 0).0, [255, 255, 255, 255]);
        assert_eq!(img.get_pixel(3, 1).0, [0, 0, 0, 255]);
    }

    #[test]
    fn poster_conversion_honors_sdr_matrix_and_range() {
        // Independently tabulated 75% yellow, rounded to 8-bit YUV codes. The 709/full patch
        // fails by over ten codes with the old fixed 601/limited conversion.
        for (bt709, full_range, yuv) in [
            (false, false, [161, 44, 141]),
            (false, true, [169, 33, 144]),
            (true, false, [168, 44, 136]),
            (true, true, [177, 33, 137]),
        ] {
            let f = Frame {
                pts: Time(0),
                width: 2,
                height: 2,
                planes: [vec![yuv[0]; 4], vec![yuv[1]], vec![yuv[2]]],
                bt709,
                full_range,
                hdr: None,
            };
            let got = to_rgba(&f, 2).get_pixel(0, 0).0;
            assert!(
                got[..3]
                    .iter()
                    .zip([191, 191, 0])
                    .all(|(g, w)| g.abs_diff(w) <= 3),
                "709={bt709} full={full_range}: {got:?}"
            );
        }
    }

    #[test]
    fn real_decode_poster_and_strip() {
        let Some(dir) = crate::test_media::corpus() else {
            return;
        };
        crate::ffmpeg::init().unwrap();
        let c = Cache::new(tmp("real"), u64::MAX);
        let p = dir.join("frame-index-1080p30.mp4");
        let poster = c.poster(&p, SECS).unwrap();
        assert_eq!((poster.width(), poster.height()), (WIDTH, 90));
        assert_eq!(c.filmstrip(&p, SECS).unwrap().len(), 10);
    }

    #[test]
    fn filmstrip_reports_progress_then_hits_cache() {
        let d = tmp("progress");
        let p = d.join("a.mp4");
        fs::write(&p, b"x").unwrap();
        let (c, _) = counting(d.join("cache"), u64::MAX);
        let mut seen = Vec::new();
        let strip = c
            .filmstrip_with(&p, SECS, |f| seen.push(f.get_pixel(0, 0)[0]))
            .unwrap();
        assert_eq!(seen, (0..10).collect::<Vec<u8>>());
        assert_eq!(strip.len(), 10);
        // Cached: all at once, nothing to report.
        c.filmstrip_with(&p, SECS, |_| panic!("cached")).unwrap();
    }

    #[test]
    fn long_filmstrips_sample_keyframes() {
        let d = tmp("keyframes");
        let p = d.join("a.mp4");
        fs::write(&p, b"x").unwrap();
        let modes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let m = modes.clone();
        let grab = Box::new(move |_: &Path, _: Time, exact: bool| {
            m.lock().unwrap().push(exact);
            Ok(RgbaImage::new(WIDTH, 90))
        });
        let c = Cache::with_grab(d.join("cache"), u64::MAX, grab);
        c.poster(&p, Time(7200 * FLICKS_PER_SECOND)).unwrap();
        c.filmstrip(&p, Time(7200 * FLICKS_PER_SECOND)).unwrap();
        let modes = modes.lock().unwrap();
        assert_eq!(modes.len(), 1 + STRIP_MAX as usize);
        assert!(modes[0] && !modes[1..].contains(&true));
    }
}
