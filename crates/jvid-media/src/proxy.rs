//! Low-res, intra-frame proxies for smooth preview of heavy sources (4K, HEVC). Preview decodes
//! through [`Proxies::resolve`]; export decodes the originals and never looks here.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ffmpeg_next::Error;
use jvid_core::model::{ProbeInfo, StreamInfo};
use jvid_core::preset::{Preset, VideoCodec};
use jvid_core::time::{Rate, Time};
use jvid_jobs::{Ctx, Handle, Pool, Priority};

use crate::decode::VideoDecoder;
use crate::encode::{CreateError, Encoder};

/// Proxy height in pixels; width follows the source aspect.
pub const HEIGHT: u32 = 540;
/// Conversion/encoding contract version; old disposable proxies must regenerate.
const VERSION: u32 = 2;

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
        Some(self.dir.join(format!("{key}.proxy-v{VERSION}.mp4")))
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
fn encode(src: &Path, dst: &Path, rate: Rate, frames: i64, ctx: &Ctx) -> Result<bool, CreateError> {
    let mut dec = VideoDecoder::open_hdr(src)?;
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
    let mut f = first;
    for i in 0..frames {
        if ctx.is_cancelled() {
            return Ok(false);
        }
        if i > 0 {
            f = dec.frame_at(rate.time_floor(i))?;
        }
        enc.video(f.view_rgba(size).as_raw()).map_err(Error::from)?;
        ctx.set_progress(i as f32 / frames as f32);
    }
    enc.finish().map_err(Error::from)?;
    Ok(true)
}

/// The proxy size for an upright `w × h` source: [`HEIGHT`] tall (never upscaled), even sides.
pub fn size((w, h): (u32, u32)) -> (u32, u32) {
    let ph = h.min(HEIGHT);
    let pw = (w as u64 * ph as u64 / h.max(1) as u64) as u32;
    ((pw & !1).max(2), (ph & !1).max(2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::Frame;
    use jvid_core::model::VideoInfo;

    #[test]
    fn proxy_conversion_honors_matrix_and_range_changes() {
        crate::ffmpeg::init().unwrap();
        // Keep dimensions fixed while changing metadata to catch stale cached coefficients.
        for (bt709, full_range, yuv) in [
            (true, false, [168, 44, 136]),
            (false, false, [161, 44, 141]),
            (true, true, [177, 33, 137]),
            (false, true, [169, 33, 144]),
        ] {
            let f = Frame {
                pts: Time(0),
                width: 32,
                height: 16,
                planes: [
                    vec![yuv[0]; 32 * 16],
                    vec![yuv[1]; 16 * 8],
                    vec![yuv[2]; 16 * 8],
                ],
                bt709,
                full_range,
                hdr: None,
            };
            let got = f.view_rgba((16, 8)).into_raw();
            assert!(
                got[..3]
                    .iter()
                    .zip([191, 191, 0])
                    .all(|(g, w)| g.abs_diff(w) <= 3),
                "709={bt709} full={full_range}: {:?}",
                &got[..3]
            );
        }
    }

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

    #[test]
    fn old_proxy_conversion_cache_misses() {
        use std::hash::{Hash, Hasher};
        let dir =
            std::env::temp_dir().join(format!("jvid-proxy-color-version-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.mp4");
        fs::write(&src, b"source remains unchanged").unwrap();
        let m = fs::metadata(&src).unwrap();
        let mut old = std::collections::hash_map::DefaultHasher::new();
        (1u32, src.as_path(), m.len(), m.modified().ok()).hash(&mut old);
        fs::write(
            dir.join(format!("{:016x}.proxy.mp4", old.finish())),
            b"old pixels",
        )
        .unwrap();
        // Also reject the old proxy suffix if the shared thumbnail key version changes alone.
        fs::write(
            dir.join(format!(
                "{}.proxy.mp4",
                crate::thumbnails::key(&src).unwrap()
            )),
            b"old pixels",
        )
        .unwrap();
        let proxies = Proxies::new(dir.clone());
        proxies.set_enabled(true);
        assert!(proxies.existing(&src).is_none());
        assert_eq!(proxies.resolve(&src), src);
        assert_eq!(fs::read(&src).unwrap(), b"source remains unchanged");
        fs::remove_dir_all(dir).unwrap();
    }

    /// A 720p source becomes an all-intra 540p proxy with every frame, and switching follows the toggle.
    #[test]
    fn generates_intra_proxy_and_switches() {
        crate::ffmpeg::init().unwrap();
        let dir = std::env::temp_dir().join(format!("jvid-proxy-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.mov");
        let rate = Rate::new(30, 1);
        let mut preset = jvid_core::preset::builtin()
            .into_iter()
            .find(|p| p.video == Some(VideoCodec::ProRes))
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
        info.frame_rate = Some(rate);
        let pool = Pool::new(1);
        let job = proxies.submit(&pool, src.clone(), &info);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !job.is_finished() {
            assert!(std::time::Instant::now() < deadline, "proxy job timed out");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let proxy_options = BTreeMap::from([("encoder".to_owned(), "software".to_owned())]);
        if crate::encode::video_encoders(VideoCodec::H264, &proxy_options).is_empty() {
            assert!(
                proxies.existing(&src).is_none(),
                "no mislabeled MPEG-4 proxy"
            );
            proxies.set_enabled(true);
            assert_eq!(proxies.resolve(&src), src, "original remains playable");
            // Exercise the public production path and retain the actionable creation error.
            let dst = dir.join("unavailable.mp4");
            let original = fs::read(&src).unwrap();
            fs::write(&dst, b"previous proxy").unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let (source, target) = (src.clone(), dst.clone());
            let _job = pool.submit("unavailable proxy", Priority::Proxy, move |ctx| {
                tx.send(generate(&source, &target, rate, 12, ctx)).unwrap();
            });
            let error = rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap()
                .unwrap_err();
            assert!(
                error.contains("h264")
                    && error.contains("libopenh264")
                    && error.contains("gpl-ffmpeg"),
                "{error}"
            );
            assert_eq!(fs::read(&dst).unwrap(), b"previous proxy");
            assert_eq!(fs::read(&src).unwrap(), original);
            assert!(
                !dst.with_extension(format!("{}.tmp.mp4", std::process::id()))
                    .exists()
            );
        } else {
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
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}
