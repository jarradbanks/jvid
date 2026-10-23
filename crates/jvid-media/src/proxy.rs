//! Low-res, intra-frame proxies for smooth preview of heavy sources (4K, HEVC). Preview decodes
//! through [`Proxies::resolve`]; export decodes the originals and never looks here.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ffmpeg_next::format::Pixel;
use ffmpeg_next::software::scaling;
use ffmpeg_next::{Error, frame};
use jvid_core::model::{ProbeInfo, StreamInfo};
use jvid_core::preset::{Preset, VideoCodec};
use jvid_core::time::{Rate, Time};
use jvid_jobs::{Ctx, Handle, Pool, Priority};

use crate::decode::{Frame, VideoDecoder};
use crate::encode::Encoder;

/// Proxy height in pixels; width follows the source aspect.
pub const HEIGHT: u32 = 540;

/// Whether `info` is heavy enough to suggest a proxy: wider than 1080p (4K and up) or HEVC.
pub fn suggested(info: &ProbeInfo) -> bool {
    info.streams.iter().any(|s| match s {
        StreamInfo::Video(v) => v.width.max(v.height) > 1920 || v.codec == "hevc",
        _ => false,
    })
}

/// The proxy store and the preview's proxy switch.
pub struct Proxies {
    dir: PathBuf,
    enabled: AtomicBool,
}

impl Proxies {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            enabled: AtomicBool::new(false),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    /// Where `src`'s proxy lives; it changes when `src` is rewritten, so a stale proxy misses.
    pub fn path(&self, src: &Path) -> Option<PathBuf> {
        let key = crate::thumbnails::key(src).ok()?;
        Some(self.dir.join(format!("{key}.proxy.mp4")))
    }

    /// `src`'s finished proxy, if there is one.
    pub fn existing(&self, src: &Path) -> Option<PathBuf> {
        self.path(src).filter(|p| p.exists())
    }

    /// The file preview should decode for `src`: its proxy when proxies are on and it's made.
    pub fn resolve(&self, src: &Path) -> PathBuf {
        self.enabled()
            .then(|| self.existing(src))
            .flatten()
            .unwrap_or_else(|| src.to_owned())
    }

    /// Makes `src`'s proxy on `jobs` at proxy priority unless it exists; de-duplicated by path.
    pub fn submit(self: &Arc<Self>, jobs: &Pool, src: PathBuf, info: &ProbeInfo) -> Handle {
        let me = self.clone();
        let rate = info.frame_rate.unwrap_or(Rate::new(30, 1));
        // Rounded up: a container's duration often ends at the last frame's start.
        let frames = rate.index_floor(Time(info.duration.0 - 1)).max(0) + 1;
        jobs.submit(
            format!("proxy:{}", src.display()),
            Priority::Proxy,
            move |ctx| {
                let Some(dst) = me.path(&src).filter(|p| !p.exists()) else {
                    return;
                };
                if let Err(e) = fs::create_dir_all(&me.dir)
                    .map_err(|e| e.to_string())
                    .and_then(|()| generate(&src, &dst, rate, frames, ctx))
                {
                    tracing::warn!("proxy for {}: {e}", src.display());
                }
            },
        )
    }
}

/// Transcodes `frames` frames of `src` at `rate` into an all-intra [`HEIGHT`]p H.264 `dst`, written
/// to a temporary file first so a cancelled or failed job never leaves a half proxy behind.
// shortcut: video only, since preview audio still decodes from the original.
pub fn generate(src: &Path, dst: &Path, rate: Rate, frames: i64, ctx: &Ctx) -> Result<(), String> {
    let tmp = dst.with_extension(format!("{}.tmp.mp4", std::process::id()));
    let r = match encode(src, &tmp, rate, frames, ctx) {
        Ok(true) => return fs::rename(&tmp, dst).map_err(|e| e.to_string()),
        Ok(false) => Ok(()),
        Err(e) => Err(e.to_string()),
    };
    let _ = fs::remove_file(&tmp);
    r
}

/// Returns false when cancelled.
fn encode(src: &Path, dst: &Path, rate: Rate, frames: i64, ctx: &Ctx) -> Result<bool, Error> {
    let mut dec = VideoDecoder::open(src)?;
    let first = dec.frame_at(rate.time_floor(0))?;
    let size = size((first.width, first.height));
    let preset = Preset {
        name: "Proxy".into(),
        extension: "mp4".into(),
        video: Some(VideoCodec::H264),
        audio: None,
        size: None,
        frame_rate: None,
        // GOP of 1 (all intra), so every scrub lands on a keyframe; software for the same output
        // everywhere, and lower quality is fine for preview.
        options: BTreeMap::from([
            ("g".into(), "1".into()),
            ("encoder".into(), "software".into()),
            ("crf".into(), "23".into()),
        ]),
    };
    let mut enc = Encoder::create(dst, size, rate, &preset)?;
    let mut scaler = None;
    let mut f = first;
    for i in 0..frames {
        if ctx.is_cancelled() {
            return Ok(false);
        }
        if i > 0 {
            f = dec.frame_at(rate.time_floor(i))?;
        }
        enc.video(&to_rgba(&f, size, &mut scaler)?)?;
        ctx.set_progress(i as f32 / frames as f32);
    }
    enc.finish()?;
    Ok(true)
}

/// The proxy size for an upright `w × h` source: [`HEIGHT`] tall (never upscaled), even sides.
pub fn size((w, h): (u32, u32)) -> (u32, u32) {
    let ph = h.min(HEIGHT);
    let pw = (w as u64 * ph as u64 / h.max(1) as u64) as u32;
    ((pw & !1).max(2), (ph & !1).max(2))
}

/// Scales an I420 frame to `size` tight RGBA rows.
// shortcut: swscale's default BT.601 limited-range matrix, so BT.709 or full-range proxies shift
// colour slightly; fine for preview, pass the frame's colorspace if anyone notices.
fn to_rgba(
    f: &Frame,
    (w, h): (u32, u32),
    scaler: &mut Option<scaling::Context>,
) -> Result<Vec<u8>, Error> {
    let mut yuv = frame::Video::new(Pixel::YUV420P, f.width, f.height);
    for (i, plane) in f.planes.iter().enumerate() {
        let pw = if i == 0 { f.width } else { f.width.div_ceil(2) } as usize;
        let stride = yuv.stride(i);
        for (row, line) in plane.chunks_exact(pw).enumerate() {
            yuv.data_mut(i)[row * stride..][..pw].copy_from_slice(line);
        }
    }
    let s = match scaler {
        Some(s) => s,
        None => scaler.insert(scaling::Context::get(
            Pixel::YUV420P,
            f.width,
            f.height,
            Pixel::RGBA,
            w,
            h,
            scaling::Flags::BILINEAR,
        )?),
    };
    let mut rgba = frame::Video::empty();
    s.run(&yuv, &mut rgba)?;
    let stride = rgba.stride(0);
    Ok(rgba
        .data(0)
        .chunks(stride)
        .take(h as usize)
        .flat_map(|r| &r[..w as usize * 4])
        .copied()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::model::VideoInfo;

    #[test]
    fn proxy_size_keeps_aspect_and_is_even() {
        assert_eq!(size((3840, 2160)), (960, 540));
        assert_eq!(size((2160, 3840)), (302, 540));
        assert_eq!(size((640, 360)), (640, 360));
        assert_eq!(size((4096, 2160)), (1024, 540));
    }

    #[test]
    fn suggests_4k_and_hevc_only() {
        let info = |codec: &str, w, h| ProbeInfo {
            streams: vec![StreamInfo::Video(VideoInfo {
                codec: codec.into(),
                width: w,
                height: h,
                ..VideoInfo::default()
            })],
            ..ProbeInfo::default()
        };
        assert!(suggested(&info("h264", 3840, 2160)));
        assert!(suggested(&info("hevc", 1920, 1080)));
        assert!(!suggested(&info("h264", 1920, 1080)));
        assert!(!suggested(&ProbeInfo::default()));
    }

    /// A 720p source becomes an all-intra 540p proxy with every frame, and switching follows the toggle.
    #[test]
    fn generates_intra_proxy_and_switches() {
        crate::ffmpeg::init().unwrap();
        let dir = std::env::temp_dir().join(format!("jvid-proxy-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.mp4");
        let rate = Rate::new(30, 1);
        let mut preset = jvid_core::preset::builtin()
            .into_iter()
            .find(|p| p.video == Some(VideoCodec::H264))
            .unwrap();
        preset.frame_rate = None;
        let (w, h) = (1280, 720);
        let mut enc = Encoder::create(&src, (w, h), rate, &preset).unwrap();
        for i in 0..12u8 {
            enc.video(&vec![i * 20; (w * h * 4) as usize]).unwrap();
        }
        enc.finish().unwrap();

        let proxies = Arc::new(Proxies::new(dir.join("proxies")));
        assert_eq!(proxies.resolve(&src), src);
        let mut info = crate::probe::probe(&src).unwrap();
        // The average rate of a 12-frame clip is skewed by its last frame's missing duration.
        info.frame_rate = Some(rate);
        let pool = Pool::new(1);
        let job = proxies.submit(&pool, src.clone(), &info);
        while !job.is_finished() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let proxy = proxies.existing(&src).expect("proxy made");
        assert_eq!(proxies.resolve(&src), src, "off until toggled");
        proxies.set_enabled(true);
        assert_eq!(proxies.resolve(&src), proxy);

        let p = crate::probe::probe(&proxy).unwrap();
        assert_eq!((p.width, p.height), (Some(960), Some(540)));
        // All intra: the keyframe at or before every frame is that frame.
        let (mut key, mut exact) = (
            VideoDecoder::open(&proxy).unwrap(),
            VideoDecoder::open(&proxy).unwrap(),
        );
        for i in [0, 5, 11] {
            let t = rate.time_floor(i);
            assert_eq!(
                key.keyframe_at(t).unwrap().pts,
                exact.frame_at(t).unwrap().pts
            );
        }
        // The last frame made it, give or take lossy encoding (frames step 17 luma levels).
        let luma = |p: &Path| {
            VideoDecoder::open(p)
                .unwrap()
                .frame_at(rate.time_floor(11))
                .unwrap()
                .planes[0][0]
        };
        assert!(luma(&proxy).abs_diff(luma(&src)) < 4);
        // Windows can't delete a file still open.
        drop((key, exact));
        fs::remove_dir_all(&dir).unwrap();
    }
}
