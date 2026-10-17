//! Audio output: a lock-free ring buffer feeding a cpal stream, and the master clock.
//!
//! The clock is the count of sample frames the device has consumed from the ring. With no device
//! the ring is drained at wall-clock rate instead, so the clock and the feed behave the same.

use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::{Consumer, Producer, RingBuffer};

/// Project rate; the feed is interleaved f32 stereo at this rate (jvid_media::audio).
pub const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: usize = 2;
/// Ring capacity: half a second of audio.
const RING_FRAMES: usize = SAMPLE_RATE as usize / 2;
/// How often a missing or non-preferred device is retried.
const RETRY: Duration = Duration::from_secs(1);

/// State shared between the app thread and the audio callback.
pub struct Feed {
    /// Locked only by the callback, and by the app while it drains with no device, so the
    /// callback's `try_lock` never waits.
    ring: Mutex<Consumer<f32>>,
    played: AtomicU64,
    lost: AtomicBool,
}

impl Feed {
    /// Fills a device buffer of `channels` interleaved channels from the ring, padding with
    /// silence on underrun. Only frames taken from the ring advance the clock.
    pub fn fill(&self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        let Ok(mut ring) = self.ring.try_lock() else {
            return;
        };
        let frames = (out.len() / channels).min(ring.slots() / CHANNELS);
        for frame in out.chunks_exact_mut(channels).take(frames) {
            let l = ring.pop().unwrap_or(0.0);
            let r = ring.pop().unwrap_or(0.0);
            match frame {
                [m] => *m = (l + r) * 0.5,
                [a, b, ..] => (*a, *b) = (l, r),
                [] => {}
            }
        }
        self.played.fetch_add(frames as u64, Ordering::Release);
    }

    /// Marks the stream dead; the next `Output::poll` reopens a device.
    pub fn lose(&self) {
        self.lost.store(true, Ordering::Release);
    }
}

/// Opens output streams. `CpalBackend` in the app; a fake in tests.
pub trait Backend {
    /// Output device names.
    fn devices(&self) -> Vec<String>;
    /// Starts a stream on `device` (the default when `None`) that calls `feed.fill` from its
    /// callback and `feed.lose` when the device goes away. Returns the stream's name and a handle
    /// that stops the stream when dropped.
    fn open(
        &mut self,
        device: Option<&str>,
        feed: Arc<Feed>,
    ) -> Result<(String, Box<dyn Any>), String>;
}

/// Plays the feed and keeps the master clock.
pub struct Output {
    backend: Box<dyn Backend>,
    producer: Producer<f32>,
    feed: Arc<Feed>,
    /// User's chosen device; `None` follows the system default.
    preferred: Option<String>,
    stream: Option<(String, Box<dyn Any>)>,
    last_try: Instant,
    /// Wall-clock drain position while there is no device.
    wall: Option<Instant>,
}

impl Output {
    pub fn new(backend: Box<dyn Backend>, preferred: Option<String>) -> Self {
        Self::at(backend, preferred, Instant::now())
    }

    fn at(backend: Box<dyn Backend>, preferred: Option<String>, now: Instant) -> Self {
        let (producer, consumer) = RingBuffer::new(RING_FRAMES * CHANNELS);
        let feed = Arc::new(Feed {
            ring: Mutex::new(consumer),
            played: AtomicU64::new(0),
            lost: AtomicBool::new(false),
        });
        let mut out = Self {
            backend,
            producer,
            feed,
            preferred,
            stream: None,
            last_try: now,
            wall: None,
        };
        out.reopen(now);
        out
    }

    /// Queues interleaved stereo samples; returns how many were taken (whole frames only).
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let n = samples.len().min(self.producer.slots()) / CHANNELS * CHANNELS;
        for &s in &samples[..n] {
            let _ = self.producer.push(s);
        }
        n
    }

    /// Master clock: sample frames played since the output was created.
    pub fn clock(&self) -> u64 {
        self.feed.played.load(Ordering::Acquire)
    }

    /// Name of the device playing, or `None` on the wall-clock fallback.
    pub fn device(&self) -> Option<&str> {
        self.stream.as_ref().map(|(name, _)| name.as_str())
    }

    pub fn devices(&self) -> Vec<String> {
        self.backend.devices()
    }

    /// Switches to `device` (`None` for the system default).
    pub fn set_device(&mut self, device: Option<String>) {
        self.preferred = device;
        self.reopen(Instant::now());
    }

    /// Call regularly (e.g. once per UI frame): recovers lost devices and runs the fallback clock.
    pub fn poll(&mut self) {
        self.poll_at(Instant::now());
    }

    fn poll_at(&mut self, now: Instant) {
        let lost = self.feed.lost.load(Ordering::Acquire);
        let off_preferred = self.preferred.is_some() && self.device() != self.preferred.as_deref();
        if lost || ((self.stream.is_none() || off_preferred) && now - self.last_try >= RETRY) {
            self.reopen(now);
        }
        if let Some(from) = self.wall {
            let due = ((now - from).as_secs_f64() * SAMPLE_RATE as f64) as usize;
            let mut ring = self.feed.ring.lock().unwrap_or_else(|e| e.into_inner());
            let frames = due.min(ring.slots() / CHANNELS);
            if let Ok(chunk) = ring.read_chunk(frames * CHANNELS) {
                chunk.commit_all();
            }
            self.feed.played.fetch_add(frames as u64, Ordering::Release);
            // Advance only by what was drained, so the clock never runs ahead of the feed.
            self.wall = Some(from + Duration::from_secs_f64(due as f64 / SAMPLE_RATE as f64));
            if frames < due {
                self.wall = Some(now);
            }
        }
    }

    fn reopen(&mut self, now: Instant) {
        self.last_try = now;
        self.stream = None;
        self.feed.lost.store(false, Ordering::Release);
        let preferred = self.preferred.clone();
        let opened = preferred
            .as_deref()
            .and_then(|d| self.backend.open(Some(d), self.feed.clone()).ok())
            .or_else(|| self.backend.open(None, self.feed.clone()).ok());
        match opened {
            Some(stream) => {
                tracing::info!(device = %stream.0, "audio output opened");
                self.stream = Some(stream);
                self.wall = None;
            }
            None => {
                if self.wall.is_none() {
                    tracing::warn!("no audio output device; using wall clock");
                    self.wall = Some(now);
                }
            }
        }
    }
}

/// The platform's default cpal host.
pub struct CpalBackend(cpal::Host);

impl Default for CpalBackend {
    fn default() -> Self {
        Self(cpal::default_host())
    }
}

impl Backend for CpalBackend {
    fn devices(&self) -> Vec<String> {
        self.0
            .output_devices()
            .map(|d| d.map(|d| d.to_string()).collect())
            .unwrap_or_default()
    }

    fn open(
        &mut self,
        device: Option<&str>,
        feed: Arc<Feed>,
    ) -> Result<(String, Box<dyn Any>), String> {
        let device = match device {
            Some(name) => self
                .0
                .output_devices()
                .ok()
                .and_then(|mut d| d.find(|d| d.to_string() == name)),
            None => self.0.default_output_device(),
        }
        .ok_or("no such output device")?;
        // shortcut: needs a device that runs f32 at 48 kHz; add rubato resampling when one doesn't.
        let config = device
            .supported_output_configs()
            .map_err(|e| e.to_string())?
            .filter(|c| c.sample_format() == cpal::SampleFormat::F32)
            .filter_map(|c| c.try_with_sample_rate(SAMPLE_RATE))
            .min_by_key(|c| c.channels().abs_diff(CHANNELS as u16))
            .ok_or("device has no f32 48 kHz output")?
            .config();
        let channels = config.channels as usize;
        let lose = feed.clone();
        let stream = device
            .build_output_stream(
                config,
                move |out: &mut [f32], _: &_| feed.fill(out, channels),
                move |e: cpal::Error| {
                    use cpal::ErrorKind::*;
                    if matches!(
                        e.kind(),
                        DeviceNotAvailable | DeviceChanged | StreamInvalidated | HostUnavailable
                    ) {
                        lose.lose();
                    }
                },
                None,
            )
            .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok((device.to_string(), Box::new(stream)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Devices that can be plugged and unplugged; streams capture the feed so tests can pull audio.
    #[derive(Default, Clone)]
    struct Fake(Arc<Mutex<FakeState>>);

    #[derive(Default)]
    struct FakeState {
        plugged: Vec<String>,
        feed: Option<Arc<Feed>>,
    }

    impl Fake {
        fn with(devices: &[&str]) -> Self {
            let fake = Self::default();
            fake.0.lock().unwrap().plugged = devices.iter().map(|d| d.to_string()).collect();
            fake
        }
        fn unplug(&self, name: &str) {
            let mut s = self.0.lock().unwrap();
            s.plugged.retain(|d| d != name);
            if let Some(feed) = &s.feed {
                feed.lose();
            }
        }
        fn plug(&self, name: &str) {
            self.0.lock().unwrap().plugged.push(name.into());
        }
        /// Runs one device callback of `frames` stereo frames.
        fn pull(&self, frames: usize) -> Vec<f32> {
            let mut out = vec![9.0; frames * CHANNELS];
            self.0
                .lock()
                .unwrap()
                .feed
                .as_ref()
                .unwrap()
                .fill(&mut out, CHANNELS);
            out
        }
    }

    impl Backend for Fake {
        fn devices(&self) -> Vec<String> {
            self.0.lock().unwrap().plugged.clone()
        }
        fn open(
            &mut self,
            device: Option<&str>,
            feed: Arc<Feed>,
        ) -> Result<(String, Box<dyn Any>), String> {
            let mut s = self.0.lock().unwrap();
            let name = match device {
                Some(d) => s.plugged.iter().find(|p| *p == d),
                None => s.plugged.first(),
            }
            .cloned()
            .ok_or("unplugged")?;
            s.feed = Some(feed);
            Ok((name, Box::new(())))
        }
    }

    #[test]
    fn clock_advances_by_frames_consumed() {
        let fake = Fake::with(&["speakers"]);
        let mut out = Output::new(Box::new(fake.clone()), None);
        assert_eq!(out.push(&[0.1, 0.2, 0.3, 0.4, 0.5]), 4, "whole frames only");
        assert_eq!(fake.pull(1), [0.1, 0.2]);
        assert_eq!(out.clock(), 1);
        // Underrun pads with silence and does not advance the clock.
        assert_eq!(fake.pull(3), [0.3, 0.4, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(out.clock(), 2);
    }

    #[test]
    fn ring_full_takes_what_fits() {
        let mut out = Output::new(Box::new(Fake::with(&["a"])), None);
        let big = vec![0.5; RING_FRAMES * CHANNELS + 6];
        assert_eq!(out.push(&big), RING_FRAMES * CHANNELS);
        assert_eq!(out.push(&big), 0);
    }

    #[test]
    fn mono_and_surround_devices() {
        let fake = Fake::with(&["a"]);
        let mut out = Output::new(Box::new(fake.clone()), None);
        out.push(&[0.2, 0.4, 0.6, 0.8]);
        let feed = fake.0.lock().unwrap().feed.clone().unwrap();
        let mut mono = [9.0; 1];
        feed.fill(&mut mono, 1);
        assert_eq!(mono, [0.3]);
        let mut six = [9.0; 6];
        feed.fill(&mut six, 6);
        assert_eq!(six, [0.6, 0.8, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn wall_clock_fallback_without_device() {
        let t0 = Instant::now();
        let mut out = Output::at(Box::new(Fake::default()), None, t0);
        assert_eq!(out.device(), None);
        out.push(&vec![0.0; 48_000 * CHANNELS / 4]);
        out.poll_at(t0 + Duration::from_millis(100));
        assert_eq!(out.clock(), 4_800);
        // The feed runs dry at 12 000 frames; the clock stops there rather than running ahead.
        out.poll_at(t0 + Duration::from_millis(900));
        assert_eq!(out.clock(), 12_000);
        out.push(&[0.0; 960]);
        out.poll_at(t0 + Duration::from_millis(910));
        assert_eq!(out.clock(), 12_480);
    }

    #[test]
    fn unplug_switches_to_default_then_back_on_replug() {
        let fake = Fake::with(&["speakers", "headphones"]);
        let t0 = Instant::now();
        let mut out = Output::at(Box::new(fake.clone()), Some("headphones".into()), t0);
        assert_eq!(out.device(), Some("headphones"));
        out.push(&[0.0; 8]);
        fake.pull(1);

        fake.unplug("headphones");
        out.poll_at(t0);
        assert_eq!(out.device(), Some("speakers"));
        fake.pull(1);
        assert_eq!(out.clock(), 2, "clock carries across the switch");

        fake.plug("headphones");
        out.poll_at(t0 + RETRY);
        assert_eq!(out.device(), Some("headphones"));
    }

    #[test]
    fn losing_every_device_falls_back_to_wall_clock_and_recovers() {
        let fake = Fake::with(&["speakers"]);
        let t0 = Instant::now();
        let mut out = Output::at(Box::new(fake.clone()), None, t0);
        out.push(&[0.0; 4_800]);
        fake.unplug("speakers");
        out.poll_at(t0);
        assert_eq!(out.device(), None);
        out.poll_at(t0 + Duration::from_millis(10));
        assert_eq!(out.clock(), 480);

        fake.plug("speakers");
        out.poll_at(t0 + RETRY / 2);
        assert_eq!(out.device(), None, "not retried before RETRY");
        let clock = out.clock();
        out.poll_at(t0 + RETRY);
        assert_eq!(out.device(), Some("speakers"));
        out.poll_at(t0 + RETRY * 3);
        assert_eq!(out.clock(), clock, "wall clock stops once a device plays");
    }
}
