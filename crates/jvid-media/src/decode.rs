//! Frame-accurate video decoding: the frame shown at time `t` is the last frame with pts ≤ t.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
}

/// One open file. Not shared: take one from a [`DecoderPool`] per request.
pub struct VideoDecoder {
    input: format::context::Input,
    stream: usize,
    decoder: decoder::Video,
    time_base: Rational,
    rotation: u32,
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
        let ctx = ffmpeg_next::codec::Context::from_parameters(stream.parameters())?;
        Ok(Self {
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
        let cur = self.cur.as_ref().ok_or(Error::Eof)?;
        let time = self.to_time(pts(cur));
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
                Err(e) => return Err(e),
            }
            let stream = self.stream;
            match self.input.packets().find(|(s, _)| s.index() == stream) {
                Some((_, packet)) => self.decoder.send_packet(&packet)?,
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
    let f = if f.format() == Pixel::YUV420P {
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
    }
}

/// Idle decoders kept per asset, so scrubbing one clip doesn't reopen the file every frame.
#[derive(Default)]
pub struct DecoderPool {
    idle: Mutex<HashMap<PathBuf, Vec<VideoDecoder>>>,
}

impl DecoderPool {
    /// Decodes on the calling thread with an idle decoder for `path`, opening one if none is free.
    pub fn frame_at(&self, path: &Path, t: Time) -> Result<Frame, Error> {
        let idle = self.idle.lock().unwrap().get_mut(path).and_then(Vec::pop);
        let mut dec = match idle {
            Some(d) => d,
            None => VideoDecoder::open(path)?,
        };
        let frame = dec.frame_at(t);
        let mut idle = self.idle.lock().unwrap();
        idle.entry(path.to_owned()).or_default().push(dec);
        frame
    }

    /// Decodes on `jobs` and hands the result to `done` there. A request matching a live one
    /// (same path and time) is dropped along with its `done`; the returned handle is the live one's.
    pub fn submit(
        self: &Arc<Self>,
        jobs: &jvid_jobs::Pool,
        path: PathBuf,
        t: Time,
        priority: Priority,
        done: impl FnOnce(Result<Frame, Error>) + Send + 'static,
    ) -> Handle {
        let pool = self.clone();
        let key = format!("decode:{}:{}", path.display(), t.0);
        jobs.submit(key, priority, move |ctx| {
            if !ctx.is_cancelled() {
                done(pool.frame_at(&path, t));
            }
        })
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

    /// Every frame's best-effort pts in flicks, decoded straight through by FFmpeg.
    fn reference_pts(path: &Path) -> Vec<Time> {
        let mut dec = VideoDecoder::open(path).unwrap();
        let mut out = Vec::new();
        while let Some(f) = dec.decode_one().unwrap() {
            out.push(dec.to_time(pts(&f)));
        }
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
                .values()
                .map(Vec::len)
                .sum::<usize>()
                >= 1
        );
    }
}
