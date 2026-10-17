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
const VERSION: u32 = 1;

/// Returns a `WIDTH`-wide RGBA thumbnail of `path` at `t`.
pub type Grab = dyn Fn(&Path, Time) -> Result<RgbaImage, String> + Send + Sync;

pub struct Cache {
    dir: PathBuf,
    cap_bytes: u64,
    grab: Box<Grab>,
}

impl Cache {
    /// A cache in `dir` decoding through FFmpeg (videos) or `decode_still` (images).
    pub fn new(dir: PathBuf, cap_bytes: u64) -> Self {
        let pool = DecoderPool::default();
        Self::with_grab(dir, cap_bytes, Box::new(move |p, t| grab(&pool, p, t)))
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
        self.cached(path, "poster", || (self.grab)(path, t))
    }

    /// Frames at a fixed interval: every second, widened so there are at most [`STRIP_MAX`].
    pub fn filmstrip(&self, path: &Path, duration: Time) -> Result<Vec<RgbaImage>, String> {
        let step = FLICKS_PER_SECOND.max(duration.0 / STRIP_MAX);
        let n = (duration.0 / step).max(1);
        let strip = self.cached(path, "strip", || {
            let frames = (0..n)
                .map(|i| (self.grab)(path, Time(i * step)))
                .collect::<Result<Vec<_>, _>>()?;
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

fn grab(pool: &DecoderPool, path: &Path, t: Time) -> Result<RgbaImage, String> {
    if ImageFormat::from_path(path).is_ok() {
        let img = crate::stills::decode_still(path).map_err(|e| e.to_string())?;
        let h = (WIDTH as u64 * img.height() as u64 / img.width().max(1) as u64).max(1) as u32;
        return Ok(imageops::thumbnail(&img, WIDTH, h));
    }
    let f = pool.frame_at(path, t).map_err(|e| e.to_string())?;
    Ok(to_rgba(&f, WIDTH))
}

/// Nearest-neighbour scale to `w` wide and BT.601 limited-range YUV to RGB.
// shortcut: assumes BT.601 limited range; use the frame's colour info once Frame carries it (#23).
fn to_rgba(f: &Frame, w: u32) -> RgbaImage {
    let h = (w as u64 * f.height as u64 / f.width.max(1) as u64).max(1) as u32;
    let (fw, cw) = (f.width as usize, f.width.div_ceil(2) as usize);
    RgbaImage::from_fn(w, h, |x, y| {
        let sx = (x as u64 * f.width as u64 / w as u64) as usize;
        let sy = (y as u64 * f.height as u64 / h as u64) as usize;
        let c = 298 * (f.planes[0][sy * fw + sx] as i32 - 16);
        let d = f.planes[1][sy / 2 * cw + sx / 2] as i32 - 128;
        let e = f.planes[2][sy / 2 * cw + sx / 2] as i32 - 128;
        let px = |v: i32| ((v + 128) >> 8).clamp(0, 255) as u8;
        image::Rgba([
            px(c + 409 * e),
            px(c - 100 * d - 208 * e),
            px(c + 516 * d),
            255,
        ])
    })
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
        let grab = Box::new(move |_: &Path, t: Time| {
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
        };
        let img = to_rgba(&f, 4);
        assert_eq!(img.get_pixel(0, 0).0, [255, 255, 255, 255]);
        assert_eq!(img.get_pixel(3, 1).0, [0, 0, 0, 255]);
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
}
