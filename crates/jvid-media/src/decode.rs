//! Frame-accurate video decoding: the frame shown at time `t` is the last frame with pts ≤ t.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ffmpeg_next::color::{Range, Space};
use ffmpeg_next::format::{self, Pixel};
use ffmpeg_next::software::scaling;
use ffmpeg_next::{Error, Rational, decoder, frame, media};
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use jvid_jobs::{Handle, Priority};

/// An upright YUV 4:2:0 frame (I420): Y, U, V planes with tight rows, chroma `(w+1)/2 × (h+1)/2`.
#[derive(Debug, Clone)]
pub struct Frame {
    pub pts: Time,
    pub width: u32,
    pub height: u32,
    pub planes: [Vec<u8>; 3],
    /// YUV→RGB matrix: BT.709 when true, BT.601 when false.
    pub bt709: bool,
    /// Full (0–255) rather than limited (16–235) range.
    pub full_range: bool,
}

/// One open file. Not shared: take one from a [`DecoderPool`] per request.
pub struct VideoDecoder {
    input: format::context::Input,
    stream: usize,
    decoder: decoder::Video,
    time_base: Rational,
    rotation: u32,
    hwaccel: Option<&'static str>,
    /// Last decoded frame at or before the previous target, and the one after it.
    cur: Option<frame::Video>,
    next: Option<frame::Video>,
    eof_sent: bool,
}

impl VideoDecoder {
    /// Opens the best video stream of `path`. Call `ffmpeg::init` first.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let input = format::input(path)?;
        let stream = input
            .streams()
            .best(media::Type::Video)
            .ok_or(Error::StreamNotFound)?;
        let mut ctx = ffmpeg_next::codec::Context::from_parameters(stream.parameters())?;
        let hwaccel = crate::hwdec::attach(&mut ctx);
        Ok(Self {
            hwaccel,
            stream: stream.index(),
            time_base: stream.time_base(),
            rotation: crate::probe::rotation(&stream),
            decoder: ctx.decoder().video()?,
            input,
            cur: None,
            next: None,
            eof_sent: false,
        })
    }

    /// The hardware device decoding this file, if any (see [`crate::hwdec`]).
    pub fn hwaccel(&self) -> Option<&'static str> {
        self.hwaccel
    }

    /// The frame shown at `t`; before the first frame, the first frame.
    pub fn frame_at(&mut self, t: Time) -> Result<Frame, Error> {
        let target = self.to_ticks(t);
        // Decoding forward up to a second beats a seek, which restarts at a keyframe.
        let near = self.to_ticks(Time(FLICKS_PER_SECOND));
        match self.cur.as_ref().map(pts) {
            Some(p) if p <= target && target - p <= near => {}
            _ => self.seek(target)?,
        }
        loop {
            if self.next.is_none() {
                self.next = self.decode_one()?;
            }
            match &self.next {
                Some(n) if pts(n) <= target || self.cur.is_none() => self.cur = self.next.take(),
                _ => break,
            }
        }
        self.current()
    }

    /// The keyframe at or before `t`: one seek and one decode, so a stand-in shows at once while
    /// [`Self::frame_at`] decodes the rest of a long GOP.
    pub fn keyframe_at(&mut self, t: Time) -> Result<Frame, Error> {
        self.seek(self.to_ticks(t))?;
        self.cur = self.next.take();
        self.current()
    }

    fn current(&self) -> Result<Frame, Error> {
        let cur = self.cur.as_ref().ok_or(Error::Eof)?;
        let time = self.to_time(pts(cur));
        let downloaded;
        let cur = if crate::hwdec::is_hardware(cur) {
            downloaded = crate::hwdec::download(cur)?;
            &downloaded
        } else {
            cur
        };
        let mut frame = upright(&to_i420(cur)?, self.rotation);
        frame.pts = time;
        Ok(frame)
    }

    /// Lands on a keyframe at or before `target`, backing off when the container's index is coarse.
    fn seek(&mut self, target: i64) -> Result<(), Error> {
        let mut back = self.to_ticks(Time(FLICKS_PER_SECOND));
        let mut ts = target;
        loop {
            let micros = self.to_micros(ts);
            self.input.seek(micros, ..micros)?;
            self.decoder.flush();
            (self.cur, self.next, self.eof_sent) = (None, None, false);
            self.next = self.decode_one()?;
            match &self.next {
                Some(n) if pts(n) > target && micros > 0 => {
                    ts = target - back;
                    back *= 2;
                }
                _ => return Ok(()),
            }
        }
    }

    fn decode_one(&mut self) -> Result<Option<frame::Video>, Error> {
        let mut f = frame::Video::empty();
        loop {
            match self.decoder.receive_frame(&mut f) {
                Ok(()) => return Ok(Some(f)),
                Err(Error::Eof) => return Ok(None),
                Err(Error::Other { errno }) if errno == ffmpeg_next::util::error::EAGAIN => {}
                // Like the ffmpeg CLI: drop a corrupt frame and keep going.
                Err(Error::InvalidData) => tracing::warn!("skipping corrupt frame"),
                Err(e) => return Err(e),
            }
            let stream = self.stream;
            match self.input.packets().find(|(s, _)| s.index() == stream) {
                Some((_, packet)) => match self.decoder.send_packet(&packet) {
                    Err(Error::InvalidData) => tracing::warn!("skipping corrupt packet"),
                    r => r?,
                },
                None if !self.eof_sent => {
                    self.decoder.send_eof()?;
                    self.eof_sent = true;
                }
                None => return Ok(None),
            }
        }
    }

    /// Floors, so a frame at exactly `t` counts as shown.
    fn to_ticks(&self, t: Time) -> i64 {
        let (num, den) = (self.time_base.0 as i128, self.time_base.1 as i128);
        (t.0 as i128 * den).div_euclid(num * FLICKS_PER_SECOND as i128) as i64
    }

    fn to_time(&self, ticks: i64) -> Time {
        let (num, den) = (self.time_base.0 as i128, self.time_base.1 as i128);
        Time((ticks as i128 * num * FLICKS_PER_SECOND as i128).div_euclid(den) as i64)
    }

    fn to_micros(&self, ticks: i64) -> i64 {
        let (num, den) = (self.time_base.0 as i128, self.time_base.1 as i128);
        (ticks as i128 * num * 1_000_000).div_euclid(den).max(0) as i64
    }
}

/// FFmpeg's best-effort timestamp, the same one `ffprobe -show_frames` reports.
fn pts(f: &frame::Video) -> i64 {
    f.timestamp().or(f.pts()).unwrap_or(0)
}

fn to_i420(f: &frame::Video) -> Result<Frame, Error> {
    let (w, h) = (f.width(), f.height());
    let converted;
    // Untagged: BT.709 for HD, BT.601 for SD, as players guess.
    let bt709 = match f.color_space() {
        Space::BT470BG | Space::SMPTE170M | Space::FCC => false,
        Space::Unspecified => h > 576,
        _ => true,
    };
    // swscale turns the JPEG (yuvj) formats into limited range; other formats keep their values.
    let jpeg = matches!(
        f.format(),
        Pixel::YUVJ422P | Pixel::YUVJ444P | Pixel::YUVJ440P | Pixel::YUVJ411P
    );
    let full_range = f.color_range() == Range::JPEG && !jpeg;
    // yuvj420p is I420 already, just tagged full range.
    let f = if matches!(f.format(), Pixel::YUV420P | Pixel::YUVJ420P) {
        f
    } else {
        // shortcut: a scaler per frame (it isn't Send, so can't live in the pooled decoder);
        // cache one per decode call if non-4:2:0 sources (ProRes) show up hot in profiles.
        let mut sws = scaling::Context::get(
            f.format(),
            w,
            h,
            Pixel::YUV420P,
            w,
            h,
            scaling::Flags::POINT,
        )?;
        let mut out = frame::Video::empty();
        sws.run(f, &mut out)?;
        converted = out;
        &converted
    };
    let plane = |i: usize, pw: usize, ph: usize| {
        let stride = f.stride(i);
        (0..ph)
            .flat_map(|y| &f.data(i)[y * stride..y * stride + pw])
            .copied()
            .collect()
    };
    let (cw, ch) = (w.div_ceil(2) as usize, h.div_ceil(2) as usize);
    Ok(Frame {
        pts: Time(0),
        width: w,
        height: h,
        planes: [
            plane(0, w as usize, h as usize),
            plane(1, cw, ch),
            plane(2, cw, ch),
        ],
        bt709,
        full_range,
    })
}

/// Turns `f` clockwise by `rotation` degrees (0, 90, 180 or 270).
fn upright(f: &Frame, rotation: u32) -> Frame {
    if rotation == 0 {
        return f.clone();
    }
    let (w, h) = (f.width as usize, f.height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let turn = |p: &[u8], w: usize, h: usize| -> Vec<u8> {
        let (ow, oh) = if rotation == 180 { (w, h) } else { (h, w) };
        (0..oh)
            .flat_map(|y| (0..ow).map(move |x| (x, y)))
            .map(|(x, y)| match rotation {
                90 => p[(h - 1 - x) * w + y],
                180 => p[(h - 1 - y) * w + (w - 1 - x)],
                _ => p[x * w + (w - 1 - y)],
            })
            .collect()
    };
    let (width, height) = if rotation == 180 {
        (f.width, f.height)
    } else {
        (f.height, f.width)
    };
    Frame {
        pts: f.pts,
        width,
        height,
        planes: [
            turn(&f.planes[0], w, h),
            turn(&f.planes[1], cw, ch),
            turn(&f.planes[2], cw, ch),
        ],
        ..*f
    }
}

// shortcut: fixed caps of 2 idle decoders per asset and 8 assets (LRU), so a long timeline doesn't
// hold every file open; size them from memory profiles once multicam/4K timelines exist.
const IDLE_PER_ASSET: usize = 2;
const IDLE_ASSETS: usize = 8;

type Done = Box<dyn FnOnce(Result<Frame, Error>) + Send>;
/// Job sequence number, handle, and every caller's `done`.
type Request = (u64, Handle, Vec<Done>);

/// Idle decoders kept per asset, so scrubbing one clip doesn't reopen the file every frame.
#[derive(Default)]
pub struct DecoderPool {
    /// Least recently used asset first.
    idle: Mutex<Vec<(PathBuf, Vec<VideoDecoder>)>>,
    /// Submitted requests by path and time.
    live: Mutex<HashMap<String, Request>>,
    seq: std::sync::atomic::AtomicU64,
}

impl DecoderPool {
    /// Decodes on the calling thread with an idle decoder for `path`, opening one if none is free.
    pub fn frame_at(&self, path: &Path, t: Time) -> Result<Frame, Error> {
        self.with(path, |d| d.frame_at(t))
    }

    /// [`VideoDecoder::keyframe_at`] with a pooled decoder.
    pub fn keyframe_at(&self, path: &Path, t: Time) -> Result<Frame, Error> {
        self.with(path, |d| d.keyframe_at(t))
    }

    fn with(
        &self,
        path: &Path,
        f: impl FnOnce(&mut VideoDecoder) -> Result<Frame, Error>,
    ) -> Result<Frame, Error> {
        let idle = self
            .idle
            .lock()
            .unwrap()
            .iter_mut()
            .find(|(p, _)| p == path)
            .and_then(|(_, v)| v.pop());
        let mut dec = match idle {
            Some(d) => d,
            None => VideoDecoder::open(path)?,
        };
        let frame = f(&mut dec);
        // After a hard error the decoder may be half-advanced, so it isn't reused.
        if frame.is_ok() {
            let mut idle = self.idle.lock().unwrap();
            let mut entry = match idle.iter().position(|(p, _)| p == path) {
                Some(i) => idle.remove(i),
                None => (path.to_owned(), Vec::new()),
            };
            if entry.1.len() < IDLE_PER_ASSET {
                entry.1.push(dec);
            }
            idle.push(entry);
            if idle.len() > IDLE_ASSETS {
                idle.remove(0);
            }
        }
        frame
    }

    /// Decodes on `jobs` and hands the result to `done` there. A request matching a live one
    /// (same path and time) joins it: every caller's `done` gets the result, and all share one handle.
    pub fn submit(
        self: &Arc<Self>,
        jobs: &jvid_jobs::Pool,
        path: PathBuf,
        t: Time,
        priority: Priority,
        done: impl FnOnce(Result<Frame, Error>) + Send + 'static,
    ) -> Handle {
        let key = format!("decode:{}:{}", path.display(), t.0);
        let mut live = self.live.lock().unwrap();
        // A cancelled or finished (e.g. panicked) entry is stale; its waiters are dropped.
        if let Some((_, handle, waiters)) = live.get_mut(&key)
            && !handle.is_cancelled()
            && !handle.is_finished()
        {
            waiters.push(Box::new(done));
            return handle.clone();
        }
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (pool, k) = (self.clone(), key.clone());
        // Unique job key: this table, not the job pool, de-duplicates, so no `done` can be dropped.
        let handle = jobs.submit(format!("{key}#{seq}"), priority, move |ctx| {
            let result = (!ctx.is_cancelled()).then(|| pool.frame_at(&path, t));
            let mut live = pool.live.lock().unwrap();
            let waiters = match live.get(&k) {
                Some((s, ..)) if *s == seq => live.remove(&k).unwrap().2,
                _ => Vec::new(),
            };
            drop(live);
            if let Some(r) = result {
                for done in waiters {
                    done(r.clone());
                }
            }
        });
        live.insert(key, (seq, handle.clone(), vec![Box::new(done)]));
        handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_media::{corpus, frame_index};

    #[test]
    fn upright_turns_planes_clockwise() {
        // 2x2 luma [a b; c d], 1x1 chroma.
        let f = Frame {
            pts: Time(0),
            width: 2,
            height: 2,
            planes: [vec![1, 2, 3, 4], vec![5], vec![6]],
            bt709: true,
            full_range: false,
        };
        assert_eq!(upright(&f, 90).planes[0], [3, 1, 4, 2]);
        assert_eq!(upright(&f, 180).planes[0], [4, 3, 2, 1]);
        assert_eq!(upright(&f, 270).planes[0], [2, 4, 1, 3]);
        let tall = Frame {
            width: 4,
            height: 2,
            planes: [(0..8).collect(), vec![0; 2], vec![0; 2]],
            ..f
        };
        let r = upright(&tall, 90);
        assert_eq!((r.width, r.height), (2, 4));
        assert_eq!(r.planes[0], [4, 0, 5, 1, 6, 2, 7, 3]);
        assert_eq!(r.planes[1].len(), 2);
    }

    /// Deterministic xorshift, so a failure reproduces.
    fn rng(mut s: u64) -> impl FnMut() -> u64 {
        move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        }
    }

    /// Every frame's best-effort pts in flicks, from `ffprobe -show_frames` (scripts/gen-test-media).
    fn reference_pts(path: &Path) -> Vec<Time> {
        let dec = VideoDecoder::open(path).unwrap();
        let mut name = path.as_os_str().to_owned();
        name.push(".pts");
        let mut out: Vec<Time> = std::fs::read_to_string(name)
            .unwrap()
            .split_whitespace()
            .map(|ticks| dec.to_time(ticks.trim_matches(',').parse().unwrap()))
            .collect();
        out.sort();
        out
    }

    /// The last reference pts ≤ t, or the first.
    fn expected(reference: &[Time], t: Time) -> Time {
        reference[reference.partition_point(|&p| p <= t).max(1) - 1]
    }

    fn random_times(next: &mut impl FnMut() -> u64, end: Time, n: usize) -> Vec<Time> {
        let mut times: Vec<Time> = (0..n)
            .map(|_| Time((next() % end.0 as u64) as i64))
            .collect();
        // Also some short forward steps, which take the no-seek path.
        let step = FLICKS_PER_SECOND / 50;
        let start = times[0].0;
        times.extend((0..10).map(|i| Time(start + i * step)));
        times
    }

    #[test]
    fn random_seeks_return_the_burned_in_frame() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        let mut dec = VideoDecoder::open(&dir.join("frame-index-1080p30.mp4")).unwrap();
        let mut next = rng(0x15);
        for t in random_times(&mut next, Time(10 * FLICKS_PER_SECOND), 60) {
            let f = dec.frame_at(t).unwrap();
            let want = (t.0 * 30 / FLICKS_PER_SECOND) as u32;
            assert_eq!(frame_index(&f.planes[0], f.width as usize), want, "t={t:?}");
        }
    }

    #[test]
    fn random_seeks_match_ffmpeg_pts_across_the_corpus() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        // shortcut: long-2h-1440p60.mp4 skipped, a full reference decode takes minutes.
        for file in [
            "vfr-phone.mp4",
            "23.976fps.mp4",
            "59.94fps.mp4",
            "144fps.mp4",
            "4k-hevc.mp4",
            "prores.mov",
            "9x16.mp4",
            "no-audio.mp4",
            "rotated-phone.mp4",
            "three-audio.mkv",
            "screen-1440p60.mp4",
        ] {
            let path = dir.join(file);
            let reference = reference_pts(&path);
            let end = *reference.last().unwrap() + Time(FLICKS_PER_SECOND / 10);
            let mut dec = VideoDecoder::open(&path).unwrap();
            let mut next = rng(0x15 + file.len() as u64);
            for t in random_times(&mut next, end, 20) {
                let got = dec.frame_at(t).unwrap().pts;
                assert_eq!(got, expected(&reference, t), "{file} t={t:?}");
            }
        }
    }

    #[test]
    fn vfr_frames_match_ffmpeg_timestamps() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        let path = dir.join("vfr-phone.mp4");
        let reference = reference_pts(&path);
        let mut dec = VideoDecoder::open(&path).unwrap();
        // Every 1/120 s, then each frame's exact pts and the flick before it, backwards.
        let grid = (0..600).map(|i| Time(i * FLICKS_PER_SECOND / 120));
        let edges = reference.iter().rev().flat_map(|&p| [p, p - Time(1)]);
        for t in grid.chain(edges) {
            assert_eq!(
                dec.frame_at(t).unwrap().pts,
                expected(&reference, t),
                "t={t:?}"
            );
        }
    }

    #[test]
    fn keyframe_at_lands_on_the_keyframe_before() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        // Keyframes every 30 frames (-g 30); 3.5 s is frame 105, so keyframe 90.
        let mut dec = VideoDecoder::open(&dir.join("frame-index-1080p30.mp4")).unwrap();
        let f = dec.keyframe_at(Time(7 * FLICKS_PER_SECOND / 2)).unwrap();
        assert_eq!(frame_index(&f.planes[0], f.width as usize), 90);
        assert_eq!(f.pts, Time(3 * FLICKS_PER_SECOND));
        // The same decoder then finds the exact frame.
        let f = dec.frame_at(Time(7 * FLICKS_PER_SECOND / 2)).unwrap();
        assert_eq!(frame_index(&f.planes[0], f.width as usize), 105);
    }

    #[test]
    fn rotated_clip_matches_ffmpeg_autorotate() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        let f = VideoDecoder::open(&dir.join("rotated-phone.mp4"))
            .unwrap()
            .frame_at(Time(0))
            .unwrap();
        // The first frame as the ffmpeg CLI shows it, I420; luma comes first.
        let want = std::fs::read(dir.join("rotated-phone.first.yuv")).unwrap();
        let luma = &want[..1080 * 1920];
        let diff: u64 = (luma.iter().zip(&f.planes[0]))
            .map(|(a, b)| a.abs_diff(*b) as u64)
            .sum();
        assert!(diff / luma.len() as u64 <= 1, "mean luma diff {diff}");
        // Mirror and other turns of testsrc2 differ far more; check the test can tell.
        let wrong = upright(&upright(&f, 90), 90);
        let wdiff: u64 = (luma.iter().zip(&wrong.planes[0]))
            .map(|(a, b)| a.abs_diff(*b) as u64)
            .sum();
        assert!(wdiff / luma.len() as u64 > 10, "mean luma diff {wdiff}");
    }

    #[test]
    fn duplicate_submits_all_get_done() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        let pool = Arc::new(DecoderPool::default());
        let jobs = jvid_jobs::Pool::new(1);
        let (tx, rx) = std::sync::mpsc::channel();
        for _ in 0..3 {
            let tx = tx.clone();
            pool.submit(
                &jobs,
                dir.join("no-audio.mp4"),
                Time(FLICKS_PER_SECOND),
                Priority::Interactive,
                move |r| tx.send(r.unwrap().pts).unwrap(),
            );
        }
        drop(tx);
        assert_eq!(rx.iter().count(), 3);
    }

    #[test]
    fn pool_caps_idle_decoders() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        let pool = DecoderPool::default();
        let files = [
            "no-audio.mp4",
            "9x16.mp4",
            "144fps.mp4",
            "23.976fps.mp4",
            "59.94fps.mp4",
        ];
        for f in files.iter().cycle().take(IDLE_ASSETS + 2) {
            pool.frame_at(&dir.join(f), Time(0)).unwrap();
        }
        // Concurrent requests on one asset open extra decoders; only IDLE_PER_ASSET go back.
        std::thread::scope(|s| {
            for _ in 0..3 {
                s.spawn(|| pool.frame_at(&dir.join("no-audio.mp4"), Time(0)).unwrap());
            }
        });
        let idle = pool.idle.lock().unwrap();
        assert!(idle.len() <= IDLE_ASSETS);
        assert!(idle.iter().all(|(_, v)| v.len() <= IDLE_PER_ASSET));
    }

    #[test]
    fn corrupt_packets_are_skipped() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        // Scribble over the middle of the file: mdat payload, so some packets decode as garbage.
        let mut bytes = std::fs::read(dir.join("frame-index-1080p30.mp4")).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid..mid + 20_000].fill(0xAB);
        let path = std::env::temp_dir().join(format!("jvid-corrupt-{}.mp4", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let mut dec = VideoDecoder::open(&path).unwrap();
        for i in 0..10 {
            dec.frame_at(Time(i * FLICKS_PER_SECOND)).unwrap();
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn rotated_clip_decodes_upright_off_thread() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        let pool = Arc::new(DecoderPool::default());
        let jobs = jvid_jobs::Pool::new(2);
        let (tx, rx) = std::sync::mpsc::channel();
        for t in [0, 1] {
            let tx = tx.clone();
            pool.submit(
                &jobs,
                dir.join("rotated-phone.mp4"),
                Time(t * FLICKS_PER_SECOND),
                Priority::Interactive,
                move |r| tx.send(r).unwrap(),
            );
        }
        for _ in 0..2 {
            let f = rx.recv().unwrap().unwrap();
            assert_eq!((f.width, f.height), (1080, 1920));
            assert_eq!(f.planes[0].len(), 1080 * 1920);
            assert_eq!(f.planes[1].len(), 540 * 960);
        }
        assert!(
            pool.idle
                .lock()
                .unwrap()
                .iter()
                .map(|(_, v)| v.len())
                .sum::<usize>()
                >= 1
        );
    }

    #[test]
    fn frames_carry_matrix_and_range() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        for (file, bt709, full_range) in [
            ("bars-bt601-limited.mp4", false, false),
            ("bars-bt601-full.mp4", false, true),
            ("bars-bt709-limited.mp4", true, false),
            ("bars-bt709-full.mp4", true, true),
        ] {
            let f = VideoDecoder::open(&dir.join(file))
                .unwrap()
                .frame_at(Time(0))
                .unwrap();
            assert_eq!((f.bt709, f.full_range), (bt709, full_range), "{file}");
            // Full range keeps 75% white at 191 rather than squeezing it to 180.
            assert_eq!(
                f.planes[0][40],
                if full_range { 191 } else { 180 },
                "{file}"
            );
        }
    }
}
