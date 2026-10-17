//! Source and timeline playback: the playhead, J/K/L shuttle, and audio fed to jvid-audio's master clock.
//!
//! The playhead is derived from the clock, never from wall time, so the video frame shown is the
//! one whose audio is playing. Every speed change or seek flushes the output ring and re-anchors
//! at the clock, so audio already queued can't play on or pull the two apart.

use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel, sync_channel};
use std::thread::JoinHandle;

use jvid_audio::{Output, SAMPLE_RATE};
use jvid_core::time::{FLICKS_PER_SECOND, Rate, Time};
use jvid_media::audio::{AudioDecoder, Chunk};

/// Fastest shuttle, in either direction.
pub const MAX_SPEED: i32 = 8;
/// How far ahead of the clock audio is queued, in sample frames (50ms).
const LEAD: u64 = SAMPLE_RATE as u64 / 20;

pub struct Transport {
    /// Shuttle speed: 0 stopped, 1 play, negative reverse.
    speed: i32,
    /// The playhead at clock frame `anchor`.
    pos: Time,
    anchor: u64,
    /// The clock frame the next pushed sample plays at.
    queued: u64,
    /// The source sample frame the next pushed sample comes from, at 1x.
    next_sample: i64,
    pub duration: Time,
    pub rate: Rate,
    /// Forward play jumps from the end of this range back to its start.
    pub loop_range: Option<(Time, Time)>,
}

impl Transport {
    pub fn new(duration: Time, rate: Rate) -> Self {
        Self {
            speed: 0,
            pos: Time(0),
            anchor: 0,
            queued: 0,
            next_sample: 0,
            duration,
            rate,
            loop_range: None,
        }
    }

    pub fn speed(&self) -> i32 {
        self.speed
    }

    /// The playhead at master-clock frame `clock`, clamped to the clip.
    pub fn position(&self, clock: u64) -> Time {
        if self.speed == 0 || clock <= self.anchor {
            return self.pos;
        }
        let dt = (clock - self.anchor) as i128 * FLICKS_PER_SECOND as i128 * self.speed as i128
            / SAMPLE_RATE as i128;
        Time((self.pos.0 as i128 + dt).clamp(0, self.duration.0 as i128) as i64)
    }

    /// The frame index the playhead is in; at the very end, the last frame.
    pub fn frame(&self, clock: u64) -> i64 {
        let last = self.rate.index_floor(Time(self.duration.0 - 1)).max(0);
        self.rate.index_floor(self.position(clock)).min(last)
    }

    /// Re-anchors at `clock`; the caller flushes the output ring first, so nothing already
    /// queued plays on.
    fn set(&mut self, speed: i32, pos: Time, clock: u64) {
        self.queued = clock;
        self.pos = Time(pos.0.clamp(0, self.duration.0));
        self.anchor = self.queued;
        self.speed = speed;
        self.next_sample =
            (self.pos.0 as i128 * SAMPLE_RATE as i128 / FLICKS_PER_SECOND as i128) as i64;
    }

    pub fn seek(&mut self, t: Time, clock: u64) {
        self.set(self.speed, t, clock);
    }

    /// Space: play from stopped (from the start when parked at the end), stop otherwise.
    pub fn toggle(&mut self, clock: u64) {
        let pos = self.position(clock);
        match self.speed {
            0 if pos >= self.duration => self.set(1, Time(0), clock),
            0 => self.set(1, pos, clock),
            _ => self.set(0, pos, clock),
        }
    }

    /// J (`dir` -1) or L (`dir` 1): doubles speed in that direction up to `MAX_SPEED`,
    /// or starts at 1x when stopped or going the other way.
    pub fn shuttle(&mut self, dir: i32, clock: u64) {
        let speed = if self.speed * dir > 0 {
            (self.speed * 2).clamp(-MAX_SPEED, MAX_SPEED)
        } else {
            dir
        };
        self.set(speed, self.position(clock), clock);
    }

    /// K.
    pub fn stop(&mut self, clock: u64) {
        self.set(0, self.position(clock), clock);
    }

    /// ←/→: stops and moves `n` frames.
    pub fn step(&mut self, n: i64, clock: u64) {
        let frame = self.frame(clock) + n;
        self.set(0, self.rate.time_floor(frame), clock);
    }

    /// Call every UI frame: stops at either end and keeps `LEAD` of audio queued while playing.
    /// `audio(start, out)` fills `out` with interleaved stereo from source sample frame `start`.
    pub fn tick(&mut self, out: &mut Output, audio: &mut dyn FnMut(i64, &mut [f32])) {
        let clock = out.clock();
        if let Some((start, end)) = self.loop_range
            && self.speed > 0
            && self.position(clock) >= end
        {
            out.flush();
            self.set(self.speed, start, clock);
        }
        let pos = self.position(clock);
        if (self.speed > 0 && pos >= self.duration) || (self.speed < 0 && pos.0 <= 0) {
            out.flush();
            self.set(0, pos, clock);
        }
        if self.speed == 0 {
            return;
        }
        self.queued = self.queued.max(clock);
        let want = (clock + LEAD).saturating_sub(self.queued) as usize;
        let mut buf = vec![0.0; want * 2];
        // shortcut: shuttle speeds other than 1x play silence; add varispeed audio with the mixer.
        if self.speed == 1 {
            audio(self.next_sample, &mut buf);
        }
        let taken = (out.push(&buf) / 2) as u64;
        self.queued += taken;
        self.next_sample += taken as i64;
    }
}

/// Reads one audio stream for `Transport::tick`. A feeder thread decodes ahead of the playhead,
/// so the UI thread only copies samples and asks for seeks; it exits when this is dropped.
pub struct AudioSource {
    feeder: Option<Feeder>,
    /// Decoded samples starting at sample frame `at`, contiguous from the last seek.
    buf: Vec<f32>,
    at: i64,
}

struct Feeder {
    seek: Sender<(u64, i64)>,
    chunks: Receiver<(u64, Vec<f32>)>,
    /// The seek generation chunks must carry to be used.
    generation: u64,
    thread: Option<JoinHandle<()>>,
}

/// Chunks decoded ahead (a few hundred ms) before the feeder waits for the UI.
const AHEAD: usize = 16;
/// A forward jump further than this seeks instead of waiting for the feeder to catch up.
const MAX_GAP: i64 = SAMPLE_RATE as i64;

impl AudioSource {
    /// Silent when the file has no audio or it can't be opened.
    pub fn open(path: &Path, has_audio: bool) -> Self {
        let dec = has_audio
            .then(|| AudioDecoder::open(path, 0))
            .and_then(|r| {
                r.map_err(|e| tracing::warn!("source audio of {}: {e}", path.display()))
                    .ok()
            });
        Self {
            feeder: dec.map(Feeder::spawn),
            buf: Vec::new(),
            // Forces a seek on first use.
            at: i64::MAX,
        }
    }

    /// Fills `out` with interleaved stereo from sample frame `start`, as far as it's decoded
    /// (silence past that). Samples before `start` are dropped; nothing at or after it is, so a
    /// caller that only queued part of `out` asks again from where it got to.
    pub fn fill(&mut self, start: i64, out: &mut [f32]) {
        let Some(f) = &mut self.feeder else { return };
        while let Ok((g, samples)) = f.chunks.try_recv() {
            if g == f.generation {
                self.buf.extend(samples);
            }
        }
        if start < self.at || start > self.at + self.buf.len() as i64 / 2 + MAX_GAP {
            f.generation += 1;
            if f.seek.send((f.generation, start)).is_err() {
                self.feeder = None;
                return;
            }
            self.buf.clear();
            self.at = start;
        }
        // Behind the playhead after an underrun: drop what's arrived, the rest as it comes.
        let skip = ((start - self.at) as usize * 2).min(self.buf.len());
        self.buf.drain(..skip);
        self.at += skip as i64 / 2;
        if self.at == start {
            let n = out.len().min(self.buf.len());
            out[..n].copy_from_slice(&self.buf[..n]);
        }
    }
}

impl Feeder {
    fn spawn(mut dec: AudioDecoder) -> Self {
        let (seek, seeks) = channel::<(u64, i64)>();
        let (tx, chunks) = sync_channel(AHEAD);
        let thread = std::thread::Builder::new()
            .name("source-audio".into())
            .spawn(move || {
                // Idle until the first seek; exits when the source is dropped.
                let Ok(mut want) = seeks.recv() else { return };
                loop {
                    let (generation, start) = want;
                    if let Err(e) = dec.seek(start) {
                        tracing::warn!("source audio seek: {e}");
                        return;
                    }
                    loop {
                        match seeks.try_recv() {
                            Ok(w) => {
                                want = w;
                                break;
                            }
                            Err(TryRecvError::Disconnected) => return,
                            Err(TryRecvError::Empty) => {}
                        }
                        let samples = match dec.next_chunk() {
                            Ok(Some(Chunk { samples, .. })) => samples,
                            Ok(None) => match seeks.recv() {
                                Ok(w) => {
                                    want = w;
                                    break;
                                }
                                Err(_) => return,
                            },
                            Err(e) => {
                                tracing::warn!("source audio decode: {e}");
                                return;
                            }
                        };
                        if tx.send((generation, samples)).is_err() {
                            return;
                        }
                    }
                }
            })
            .map_err(|e| tracing::warn!("source audio thread: {e}"))
            .ok();
        Self {
            seek,
            chunks,
            generation: 0,
            thread,
        }
    }
}

impl Drop for Feeder {
    fn drop(&mut self) {
        // Disconnect both channels so the thread wakes from `send` or `recv` and exits.
        (self.seek, _) = channel();
        (_, self.chunks) = sync_channel(0);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use jvid_audio::{Backend, Feed};
    use std::any::Any;
    use std::sync::{Arc, Mutex};

    const FPS30: Rate = Rate::new(30, 1);

    fn secs(s: i64) -> Time {
        Time(s * FLICKS_PER_SECOND)
    }

    #[test]
    fn jkl_shuttle_state_machine() {
        let mut t = Transport::new(secs(60), FPS30);
        t.shuttle(1, 0);
        assert_eq!(t.speed(), 1);
        for want in [2, 4, 8, 8] {
            t.shuttle(1, 0);
            assert_eq!(t.speed(), want);
        }
        t.shuttle(-1, 0);
        assert_eq!(t.speed(), -1, "J while going forward reverses at 1x");
        t.shuttle(-1, 0);
        assert_eq!(t.speed(), -2);
        t.stop(0);
        assert_eq!(t.speed(), 0);
        t.toggle(0);
        assert_eq!(t.speed(), 1);
        t.toggle(0);
        assert_eq!(t.speed(), 0);
    }

    #[test]
    fn position_follows_clock_and_speed() {
        let mut t = Transport::new(secs(60), FPS30);
        t.seek(secs(10), 0);
        t.shuttle(1, 0);
        assert_eq!(t.position(SAMPLE_RATE as u64), secs(11));
        t.shuttle(1, SAMPLE_RATE as u64);
        assert_eq!(t.position(2 * SAMPLE_RATE as u64), secs(13), "2x");
        t.shuttle(-1, 2 * SAMPLE_RATE as u64);
        assert_eq!(t.position(5 * SAMPLE_RATE as u64), secs(10), "reverse");
        assert_eq!(t.position(100 * SAMPLE_RATE as u64), Time(0), "clamped");
    }

    #[test]
    fn steps_home_end_and_play_at_end_restarts() {
        let mut t = Transport::new(secs(2), FPS30);
        t.step(1, 0);
        assert_eq!(t.frame(0), 1);
        t.step(-5, 0);
        assert_eq!(t.frame(0), 0, "clamped at the start");
        t.seek(t.duration, 0);
        assert_eq!(t.frame(0), 59, "End shows the last frame");
        t.toggle(0);
        assert_eq!((t.speed(), t.position(0)), (1, Time(0)));
    }

    /// Forward play wraps from the loop's end to its start, flushing what was queued past it.
    #[test]
    fn loop_range_wraps_forward_play() {
        let fake = Fake::default();
        let mut out = Output::new(Box::new(fake.clone()), None);
        let mut t = Transport::new(secs(10), FPS30);
        t.loop_range = Some((secs(1), secs(2)));
        t.seek(secs(1), 0);
        t.toggle(0);
        let mut wraps = 0;
        let mut prev = t.position(0);
        for _ in 0..1_000 {
            t.tick(&mut out, &mut |_, _| {});
            fake.pull(480);
            let pos = t.position(out.clock());
            assert!(
                pos >= secs(1) && pos <= secs(2) + Time(FLICKS_PER_SECOND / 20),
                "{pos:?}"
            );
            wraps += (pos < prev) as u32;
            prev = pos;
        }
        assert!(wraps >= 3, "{wraps} wraps");
        assert_eq!(t.speed(), 1);
    }

    /// `AudioSource` matches a direct decode across a first seek, continued reads, a partial
    /// read, a backward seek and a forward jump, then shuts its thread down on drop.
    #[test]
    fn audio_source_fills_seeks_and_drains() {
        let Some(dir) = jvid_media::test_media::corpus() else {
            return;
        };
        jvid_media::ffmpeg::init().unwrap();
        let path = dir.join("tone-1khz.m4a");
        let reference = |start: i64, n: usize| {
            let mut d = AudioDecoder::open(&path, 0).unwrap();
            d.seek(start).unwrap();
            let mut v = Vec::new();
            while v.len() < n {
                v.extend(d.next_chunk().unwrap().unwrap().samples);
            }
            v.truncate(n);
            v
        };
        let mut src = AudioSource::open(&path, true);
        // The feeder decodes asynchronously; poll as the UI would until it has caught up.
        let mut read = |start: i64, n: usize| {
            let mut out = vec![0.0; n];
            for _ in 0..500 {
                out.fill(0.0);
                src.fill(start, &mut out);
                if out[n - 2..].iter().any(|&s| s != 0.0) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            out
        };
        let close = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4);
        for (start, n) in [
            (48_000, 4800),
            (50_400, 4800),
            (51_000, 9600),
            (24_000, 4800),
        ] {
            let (g, w) = (read(start, n), reference(start, n));
            let bad = g.iter().zip(&w).position(|(x, y)| (x - y).abs() >= 1e-4);
            assert!(
                bad.is_none(),
                "at {start}: {bad:?} {:?} {:?}",
                &g[..8],
                &w[..8]
            );
        }
        assert!(
            close(&read(200_000, 4800), &reference(200_000, 4800)),
            "jump"
        );
        drop(src);
    }

    /// One device; tests call `pull` as its callback.
    #[derive(Clone, Default)]
    pub(crate) struct Fake(Arc<Mutex<Option<Arc<Feed>>>>);

    impl Backend for Fake {
        fn devices(&self) -> Vec<String> {
            vec!["fake".into()]
        }
        fn open(
            &mut self,
            _: Option<&str>,
            feed: Arc<Feed>,
        ) -> Result<(String, Box<dyn Any>), String> {
            *self.0.lock().unwrap() = Some(feed);
            Ok(("fake".into(), Box::new(())))
        }
    }

    impl Fake {
        pub(crate) fn pull(&self, frames: usize) -> Vec<f32> {
            let mut out = vec![0.0; frames * 2];
            self.0.lock().unwrap().as_ref().unwrap().fill(&mut out, 2);
            out
        }
    }

    /// Five minutes of 30fps playback where every sample holds its own source index + 1. After
    /// each device callback, the frame the viewer would show for the clock is compared with the
    /// last sample heard; they must stay within one frame throughout, across pauses, seeks,
    /// shuttle and uneven UI/device timing.
    #[test]
    fn video_frame_stays_within_one_frame_of_audio_over_five_minutes() {
        let fake = Fake::default();
        let mut out = Output::new(Box::new(fake.clone()), None);
        let mut t = Transport::new(secs(300), FPS30);
        let mut audio = |start: i64, buf: &mut [f32]| {
            // The clip's audio ends with it, as a decoder's would.
            let end = 300 * SAMPLE_RATE as i64;
            for (i, f) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let n = start + i as i64;
                f.fill(if n < end { (n + 1) as f32 } else { 0.0 });
            }
        };
        let frame = FPS30.time_floor(1).0 as f64 / FLICKS_PER_SECOND as f64;
        let (mut checked, mut worst) = (0, 0.0f64);
        t.toggle(out.clock());
        for step in 0u64.. {
            // UI frames at ~60Hz with jitter; device callbacks of 480 or 512 frames.
            if step % 3 != 2 {
                t.tick(&mut out, &mut audio);
            }
            let pulled = fake.pull(if step % 2 == 0 { 480 } else { 512 });
            if matches!(step, 3_000 | 3_100 | 6_000 | 6_200 | 6_300 | 6_301) {
                out.flush();
            }
            match step {
                3_000 => t.stop(out.clock()),
                3_100 => t.toggle(out.clock()),
                6_000 => t.shuttle(1, out.clock()),
                6_200 => t.shuttle(-1, out.clock()),
                6_300 => t.seek(secs(120), out.clock()),
                6_301 => t.shuttle(1, out.clock()),
                _ => {}
            }
            if t.speed() == 0 && t.position(out.clock()) >= t.duration {
                break;
            }
            // Only 1x audio carries source positions; the rest is silence or underrun.
            let heard = pulled.last().copied().unwrap_or(0.0);
            if t.speed() != 1 || heard == 0.0 {
                continue;
            }
            let audio_secs = (heard as f64 - 1.0) / SAMPLE_RATE as f64;
            // The sample at the last clock frame played.
            let shown = t.rate.time_floor(t.frame(out.clock() - 1));
            let video_secs = shown.0 as f64 / FLICKS_PER_SECOND as f64;
            let drift = (video_secs - audio_secs).abs();
            worst = worst.max(drift);
            assert!(drift <= frame, "drift {drift}s at step {step}");
            checked += 1;
        }
        assert!(checked > 20_000, "only {checked} checks");
        assert!(worst <= frame, "{worst}");
    }
}
