//! Audio input: a cpal capture stream feeding a lock-free ring, drained by the app into a WAV.
//!
//! The device's own rate and channel count are kept; decoding the file later resamples it.

use std::any::Any;
use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::{Consumer, Producer, RingBuffer};

/// Ring capacity in seconds: how long the UI may stall before samples drop.
const RING_SECS: usize = 4;
/// Sample frames per [`Recorder::peaks`] entry; jvid-media's waveform `BUCKET`.
pub const PEAK_FRAMES: u64 = 256;

/// State shared between the capture callback and the [`Recorder`].
pub struct Capture {
    /// Locked only by the callback, so its `try_lock` never waits.
    ring: Mutex<Producer<f32>>,
    /// Peak absolute sample since the last [`Recorder::level`], as f32 bits.
    peak: AtomicU32,
    /// Samples lost to a full ring.
    dropped: AtomicU64,
    lost: AtomicBool,
}

impl Capture {
    /// Queues interleaved samples from a device buffer.
    pub fn push(&self, samples: &[f32]) {
        let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        self.peak.fetch_max(peak.to_bits(), Ordering::Relaxed); // non-negative f32 bits order like u32
        let Ok(mut ring) = self.ring.try_lock() else {
            return;
        };
        let n = samples.len().min(ring.slots());
        for &s in &samples[..n] {
            let _ = ring.push(s);
        }
        let lost = (samples.len() - n) as u64;
        if lost > 0 {
            self.dropped.fetch_add(lost, Ordering::Relaxed);
        }
    }

    /// Marks the device gone; the app ends the take.
    pub fn lose(&self) {
        self.lost.store(true, Ordering::Release);
    }
}

/// A capture stream's format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub rate: u32,
    pub channels: u16,
}

/// Opens input streams. `CpalInput` in the app; a fake in tests.
pub trait InputBackend {
    /// Input device names.
    fn devices(&self) -> Vec<String>;
    /// The format `device` (the default when `None`) would record in, without opening it.
    fn format(&self, device: Option<&str>) -> Result<Format, String>;
    /// Starts a stream on `device` that calls `capture.push` from its callback and
    /// `capture.lose` when the device goes away. The handle stops the stream when dropped.
    fn open(&mut self, device: Option<&str>, capture: Arc<Capture>)
    -> Result<Box<dyn Any>, String>;
}

/// One take: captures from a device into a 16-bit PCM WAV, written as it goes.
pub struct Recorder {
    ring: Consumer<f32>,
    capture: Arc<Capture>,
    stream: Box<dyn Any>,
    file: BufWriter<File>,
    pub format: Format,
    /// Samples written so far.
    samples: u64,
    /// Lowest and highest sample of each [`PEAK_FRAMES`] frames written, the last one partial.
    peaks: Vec<[f32; 2]>,
}

impl Recorder {
    /// Creates `path` and starts recording into it. On failure nothing is left behind.
    pub fn start(
        backend: &mut dyn InputBackend,
        device: Option<&str>,
        path: &Path,
    ) -> Result<Self, String> {
        let format = backend.format(device)?;
        let mut file = BufWriter::new(File::create_new(path).map_err(|e| e.to_string())?);
        let started = file
            .write_all(&wav_header(format, 0))
            .map_err(|e| e.to_string())
            .and_then(|()| {
                let len = format.rate as usize * format.channels.max(1) as usize * RING_SECS;
                let (producer, consumer) = RingBuffer::new(len);
                let capture = Arc::new(Capture {
                    ring: Mutex::new(producer),
                    peak: AtomicU32::new(0),
                    dropped: AtomicU64::new(0),
                    lost: AtomicBool::new(false),
                });
                let stream = backend.open(device, capture.clone())?;
                Ok((consumer, capture, stream))
            });
        match started {
            Ok((ring, capture, stream)) => Ok(Self {
                ring,
                capture,
                stream,
                file,
                format,
                samples: 0,
                peaks: Vec::new(),
            }),
            Err(e) => {
                drop(file);
                let _ = std::fs::remove_file(path);
                Err(e)
            }
        }
    }

    /// Call regularly (e.g. once per UI frame): writes what the device captured.
    pub fn poll(&mut self) -> io::Result<()> {
        let n = self.ring.slots();
        let Ok(chunk) = self.ring.read_chunk(n) else {
            return Ok(());
        };
        let (a, b) = chunk.as_slices();
        let per_peak = PEAK_FRAMES * u64::from(self.format.channels.max(1));
        let mut bytes = Vec::with_capacity(n * 2);
        for (i, &s) in a.iter().chain(b).enumerate() {
            let s = s.clamp(-1.0, 1.0);
            let at = ((self.samples + i as u64) / per_peak) as usize;
            if at == self.peaks.len() {
                self.peaks.push([f32::MAX, f32::MIN]);
            }
            let p = &mut self.peaks[at];
            *p = [p[0].min(s), p[1].max(s)];
            bytes.extend_from_slice(&pcm16(s));
        }
        chunk.commit_all();
        self.samples += n as u64;
        self.file.write_all(&bytes)
    }

    /// The waveform of what's been written, for drawing the take as it records.
    pub fn peaks(&self) -> &[[f32; 2]] {
        &self.peaks
    }

    /// Peak absolute sample since the last call, 0..=1 (1 is full scale).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.capture.peak.swap(0, Ordering::Relaxed))
    }

    /// The device went away mid-take.
    pub fn lost(&self) -> bool {
        self.capture.lost.load(Ordering::Acquire)
    }

    /// Stops the stream, writes the rest and the final sizes. Returns the sample frames recorded
    /// and their [`Self::peaks`].
    pub fn finish(mut self) -> io::Result<(u64, Vec<[f32; 2]>)> {
        drop(std::mem::replace(&mut self.stream, Box::new(())));
        self.poll()?;
        let dropped = self.capture.dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            tracing::warn!(dropped, "recording: input ring overflowed; samples lost");
        }
        // A frame cut by an overflow would shift every channel after it; drop the stray samples.
        let channels = u64::from(self.format.channels.max(1));
        let frames = self.samples / channels;
        let bytes = frames * channels * 2;
        let mut file = self.file.into_inner().map_err(|e| e.into_error())?;
        file.set_len(44 + bytes)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&wav_header(self.format, bytes))?;
        file.sync_all()?;
        Ok((frames, self.peaks))
    }
}

fn pcm16(s: f32) -> [u8; 2] {
    ((s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16).to_le_bytes()
}

/// A canonical 44-byte header for 16-bit PCM with `data` bytes of samples.
fn wav_header(f: Format, data: u64) -> [u8; 44] {
    // shortcut: WAV sizes are 32-bit, so a take past ~6 hours of stereo 48 kHz overflows; use RF64 then.
    let data = data.min(u32::MAX as u64 - 36) as u32;
    let block = f.channels * 2;
    let mut h = [0u8; 44];
    let fields: [&[u8]; 13] = [
        b"RIFF",
        &(36 + data).to_le_bytes(),
        b"WAVE",
        b"fmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &f.channels.to_le_bytes(),
        &f.rate.to_le_bytes(),
        &(f.rate * u32::from(block)).to_le_bytes(),
        &block.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &data.to_le_bytes(),
    ];
    let mut at = 0;
    for b in fields {
        h[at..at + b.len()].copy_from_slice(b);
        at += b.len();
    }
    h
}

/// The platform's default cpal host, for input.
pub struct CpalInput(cpal::Host);

impl Default for CpalInput {
    fn default() -> Self {
        Self(cpal::default_host())
    }
}

impl CpalInput {
    fn device(&self, name: Option<&str>) -> Result<(cpal::Device, cpal::StreamConfig), String> {
        let device = match name {
            Some(name) => self
                .0
                .input_devices()
                .ok()
                .and_then(|mut d| d.find(|d| d.to_string() == name)),
            None => self.0.default_input_device(),
        }
        .ok_or("no such input device")?;
        let default = device.default_input_config().map_err(|e| e.to_string())?;
        // shortcut: needs f32 capture at the device's default rate; convert other sample formats when a device lacks it.
        let config = if default.sample_format() == cpal::SampleFormat::F32 {
            default.config()
        } else {
            device
                .supported_input_configs()
                .map_err(|e| e.to_string())?
                .filter(|c| c.sample_format() == cpal::SampleFormat::F32)
                .find_map(|c| c.try_with_sample_rate(default.sample_rate()))
                .ok_or("device has no f32 input")?
                .config()
        };
        Ok((device, config))
    }
}

impl InputBackend for CpalInput {
    fn devices(&self) -> Vec<String> {
        self.0
            .input_devices()
            .map(|d| d.map(|d| d.to_string()).collect())
            .unwrap_or_default()
    }

    fn format(&self, device: Option<&str>) -> Result<Format, String> {
        let (_, c) = self.device(device)?;
        Ok(Format {
            rate: c.sample_rate,
            channels: c.channels,
        })
    }

    fn open(
        &mut self,
        device: Option<&str>,
        capture: Arc<Capture>,
    ) -> Result<Box<dyn Any>, String> {
        let (device, config) = self.device(device)?;
        let lose = capture.clone();
        let stream = device
            .build_input_stream(
                config,
                move |data: &[f32], _: &_| capture.push(data),
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
        Ok(Box::new(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A microphone fed by the test: `feed` runs one device callback.
    #[derive(Default, Clone)]
    struct FakeMic {
        capture: Arc<Mutex<Option<Arc<Capture>>>>,
        missing: bool,
    }

    impl FakeMic {
        fn feed(&self, samples: &[f32]) {
            self.capture.lock().unwrap().as_ref().unwrap().push(samples);
        }
    }

    impl InputBackend for FakeMic {
        fn devices(&self) -> Vec<String> {
            vec!["mic".into()]
        }
        fn format(&self, _: Option<&str>) -> Result<Format, String> {
            if self.missing {
                return Err("no input device".into());
            }
            Ok(Format {
                rate: 8_000,
                channels: 2,
            })
        }
        fn open(&mut self, _: Option<&str>, c: Arc<Capture>) -> Result<Box<dyn Any>, String> {
            *self.capture.lock().unwrap() = Some(c);
            Ok(Box::new(()))
        }
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jvid-input-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn writes_the_fed_samples_as_pcm16_wav() {
        let path = temp("take.wav");
        let mic = FakeMic::default();
        let mut r = Recorder::start(&mut mic.clone(), None, &path).unwrap();
        mic.feed(&[0.0, 0.5, -0.5, 1.0]);
        r.poll().unwrap();
        assert_eq!(r.level(), 1.0);
        assert_eq!(r.level(), 0.0, "the peak resets once read");
        // Out of range is clipped; the trailing half frame is dropped.
        mic.feed(&[-2.0, 0.25, 0.75]);
        let (frames, peaks) = r.finish().unwrap();
        assert_eq!(frames, 3);
        assert_eq!(peaks, [[-1.0, 1.0]], "clipped like the file");

        let b = std::fs::read(&path).unwrap();
        assert_eq!(b.len(), 44 + 3 * 2 * 2);
        assert_eq!(&b[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()), 36 + 12);
        assert_eq!(&b[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([b[22], b[23]]), 2, "channels");
        assert_eq!(u32::from_le_bytes(b[24..28].try_into().unwrap()), 8_000);
        assert_eq!(u32::from_le_bytes(b[40..44].try_into().unwrap()), 12);
        let samples: Vec<i16> = b[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect();
        assert_eq!(samples, [0, 16384, -16384, 32767, -32767, 8192]);
    }

    #[test]
    fn a_missing_device_leaves_no_file() {
        let path = temp("missing.wav");
        let mut mic = FakeMic {
            missing: true,
            ..Default::default()
        };
        assert!(Recorder::start(&mut mic, None, &path).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn a_full_ring_drops_samples_instead_of_blocking() {
        let path = temp("full.wav");
        let mic = FakeMic::default();
        let r = Recorder::start(&mut mic.clone(), None, &path).unwrap();
        let cap = 8_000 * 2 * RING_SECS;
        mic.feed(&vec![0.1; cap + 10]);
        mic.capture.lock().unwrap().as_ref().unwrap().lose();
        assert!(r.lost());
        assert_eq!(r.finish().unwrap().0, cap as u64 / 2);
    }

    #[test]
    fn peaks_cover_peak_frames_across_channels_and_polls() {
        let path = temp("peaks.wav");
        let mic = FakeMic::default();
        let mut r = Recorder::start(&mut mic.clone(), None, &path).unwrap();
        // Stereo: a peak is 512 samples. Fed in odd sizes so polls split peaks.
        mic.feed(&vec![0.5; 300]);
        r.poll().unwrap();
        assert_eq!(r.peaks(), [[0.5, 0.5]]);
        mic.feed(&vec![-0.25; 300]);
        r.poll().unwrap();
        assert_eq!(r.peaks(), [[-0.25, 0.5], [-0.25, -0.25]]);
        assert_eq!(PEAK_FRAMES as i64, jvid_media::waveforms::BUCKET);
        r.finish().unwrap();
    }
}
