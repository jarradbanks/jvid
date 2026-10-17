//! Decodes an audio stream to interleaved f32 stereo at the project sample rate.

use std::path::Path;

use ffmpeg_next::format::{Sample, context::Input, sample::Type};
use ffmpeg_next::software::resampling;
use ffmpeg_next::{ChannelLayout, Error, Rational, codec, decoder, frame, media};

use crate::probe::ProbeError;

/// Project sample rate (PLAN.md: 48 kHz f32 stereo internally).
pub const SAMPLE_RATE: u32 = 48_000;

/// Decoding restarts this far before a seek target, so codec and resampler state settle first.
const PREROLL_SAMPLES: i64 = SAMPLE_RATE as i64 / 10;

/// Interleaved stereo samples starting at `start`, counted in project-rate sample frames.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub start: i64,
    pub samples: Vec<f32>,
}

pub struct AudioDecoder {
    input: Input,
    stream: usize,
    time_base: Rational,
    decoder: decoder::Audio,
    /// Built from the first frame after open or seek, since some codecs only report their format
    /// once decoding starts.
    resampler: Option<resampling::Context>,
    mono: bool,
    /// Sample frame of the next output; `None` until the first frame after open or seek sets it.
    pos: Option<i64>,
    /// Output before this sample frame is dropped (sample-accurate seek).
    target: i64,
    eof: bool,
    done: bool,
}

impl AudioDecoder {
    /// Opens the `nth` audio stream (0-based, in file order) of `path`. Call `ffmpeg::init` first.
    pub fn open(path: &Path, nth: usize) -> Result<Self, ProbeError> {
        let input = ffmpeg_next::format::input(path).map_err(ProbeError::Unreadable)?;
        let stream = input
            .streams()
            .filter(|s| s.parameters().medium() == media::Type::Audio)
            .nth(nth)
            .ok_or(ProbeError::NoMedia)?;
        let codec = stream.parameters().id().name().to_owned();
        let decoder = codec::Context::from_parameters(stream.parameters())
            .and_then(|c| c.decoder().audio())
            .map_err(|_| ProbeError::UnsupportedCodec(codec))?;
        Ok(Self {
            stream: stream.index(),
            time_base: stream.time_base(),
            input,
            decoder,
            resampler: None,
            mono: false,
            pos: None,
            target: 0,
            eof: false,
            done: false,
        })
    }

    /// Positions the decoder so the next chunk starts exactly at sample frame `sample`.
    pub fn seek(&mut self, sample: i64) -> Result<(), Error> {
        let from = (sample - PREROLL_SAMPLES).max(0) * 1_000_000 / SAMPLE_RATE as i64;
        self.input.seek(from, ..from)?;
        self.decoder.flush();
        self.resampler = None;
        self.pos = None;
        self.target = sample;
        self.eof = false;
        self.done = false;
        Ok(())
    }

    /// The next chunk of decoded audio, or `None` at the end of the stream.
    pub fn next_chunk(&mut self) -> Result<Option<Chunk>, Error> {
        let mut frame = frame::Audio::empty();
        loop {
            if self.decoder.receive_frame(&mut frame).is_ok() {
                if let Some(c) = self.convert(Some(&mut frame))? {
                    return Ok(Some(c));
                }
            } else if self.eof {
                if self.done || self.resampler.is_none() {
                    return Ok(None);
                }
                self.done = true;
                return self.convert(None);
            } else {
                match self.input.packets().next() {
                    Some((s, p)) if s.index() == self.stream => self.decoder.send_packet(&p)?,
                    Some(_) => {}
                    None => {
                        self.decoder.send_eof()?;
                        self.eof = true;
                    }
                }
            }
        }
    }

    /// Resamples `frame` (or flushes the resampler on `None`) and trims output before `target`.
    fn convert(&mut self, mut frame: Option<&mut frame::Audio>) -> Result<Option<Chunk>, Error> {
        let mut cap = 0;
        if let Some(f) = frame.as_deref_mut() {
            if self.pos.is_none() {
                let ts = f.timestamp().or(f.pts()).unwrap_or(0) as i128;
                let (tb, rate) = (self.time_base, f.rate() as i128);
                let mut first =
                    (ts * tb.numerator() as i128 * rate / tb.denominator() as i128) as i64;
                // Coarse timestamps (MKV's milliseconds) can't place a frame to the sample, but
                // fixed-size codec frames sit on multiples of their size.
                let frame_size = self.decoder.frame_size() as i64;
                if frame_size > 0 && (tb.denominator() as i128) < rate * tb.numerator() as i128 {
                    first = (first + frame_size / 2).div_euclid(frame_size) * frame_size;
                }
                // Start on an input sample that lands exactly on an output sample, so output after
                // a seek lines up with output decoded from the start, not just to the nearest sample.
                let period = rate as i64 / gcd(rate as i64, SAMPLE_RATE as i64);
                let skip = (-first).rem_euclid(period) as usize;
                if skip >= f.samples() {
                    return Ok(None);
                }
                if skip > 0 {
                    *f = trim_start(f, skip);
                }
                self.pos = Some((first + skip as i64) * SAMPLE_RATE as i64 / rate as i64);
            }
            cap = f.samples() * SAMPLE_RATE as usize / f.rate() as usize;
        }
        let out_layout = match frame.as_deref() {
            Some(f) if self.resampler.is_none() => {
                // Mono goes to both sides at full level; swresample would pan it -3 dB.
                self.mono = f.channels() == 1;
                let layout = if self.mono {
                    ChannelLayout::MONO
                } else {
                    ChannelLayout::STEREO
                };
                self.resampler = Some(resampling::Context::get(
                    f.format(),
                    layout_of(f),
                    f.rate(),
                    Sample::F32(Type::Packed),
                    layout,
                    SAMPLE_RATE,
                )?);
                layout
            }
            _ => self.resampler.as_ref().unwrap().output().channel_layout,
        };
        // Headroom for the resampler's buffered delay.
        let mut out = frame::Audio::new(Sample::F32(Type::Packed), cap + 2048, out_layout);
        let resampler = self.resampler.as_mut().unwrap();
        match frame {
            Some(f) => {
                if f.channel_layout().is_empty() {
                    f.set_channel_layout(layout_of(f));
                }
                resampler.run(f, &mut out)?;
            }
            None => {
                resampler.flush(&mut out)?;
            }
        }
        let n = out.samples();
        let mut samples: Vec<f32> = out.data(0)[..n * 4 * out_layout.channels() as usize]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_ne_bytes(b))
            .collect();
        if self.mono {
            samples = samples.iter().flat_map(|&s| [s, s]).collect();
        }
        let start = self.pos.unwrap_or(0);
        self.pos = Some(start + n as i64);
        let skip = (self.target - start).clamp(0, n as i64);
        samples.drain(..skip as usize * 2);
        Ok((!samples.is_empty()).then_some(Chunk {
            start: start + skip,
            samples,
        }))
    }
}

/// The frame's layout, or FFmpeg's default for its channel count when the codec left it unset.
fn layout_of(f: &frame::Audio) -> ChannelLayout {
    let l = f.channel_layout();
    if l.is_empty() {
        ChannelLayout::default(f.channels().into())
    } else {
        l
    }
}

/// A copy of `f` without its first `n` samples.
fn trim_start(f: &frame::Audio, n: usize) -> frame::Audio {
    let mut out = frame::Audio::new(f.format(), f.samples() - n, layout_of(f));
    out.set_rate(f.rate());
    let width = f.format().bytes()
        * if f.is_planar() {
            1
        } else {
            f.channels() as usize
        };
    let len = out.samples() * width;
    for p in 0..f.planes() {
        out.data_mut(p)[..len].copy_from_slice(&f.data(p)[n * width..n * width + len]);
    }
    out
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_media::corpus;

    fn decode_all(path: &Path, nth: usize) -> Vec<f32> {
        let mut d = AudioDecoder::open(path, nth).unwrap();
        let mut out = Vec::new();
        while let Some(c) = d.next_chunk().unwrap() {
            assert_eq!(c.start as usize, out.len() / 2, "chunks are contiguous");
            out.extend(c.samples);
        }
        out
    }

    /// One channel (0 = left, 1 = right) of interleaved stereo, between `from` and `to` seconds.
    fn channel(s: &[f32], ch: usize, from: f64, to: f64) -> Vec<f32> {
        let at = |t: f64| (t * SAMPLE_RATE as f64) as usize;
        s.iter().skip(ch).step_by(2).copied().collect::<Vec<_>>()[at(from)..at(to)].to_vec()
    }

    /// Frequency from rising zero crossings; FFT-free and accurate to ~1 Hz over a second.
    fn frequency(s: &[f32]) -> f64 {
        let crossings: Vec<usize> = (1..s.len())
            .filter(|&i| s[i - 1] < 0.0 && s[i] >= 0.0)
            .collect();
        let (first, last) = (crossings[0], *crossings.last().unwrap());
        (crossings.len() - 1) as f64 * SAMPLE_RATE as f64 / (last - first) as f64
    }

    /// Peak level of a sine, from its RMS.
    fn peak(s: &[f32]) -> f64 {
        (s.iter().map(|&x| x as f64 * x as f64).sum::<f64>() / s.len() as f64).sqrt()
            * std::f64::consts::SQRT_2
    }

    #[test]
    fn tone_resamples_to_48k_at_right_frequency_and_level() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        for file in [
            "tone-1khz-44k.wav",
            "tone-1khz.wav",
            "tone-1khz.mp3",
            "tone-1khz.m4a",
        ] {
            let s = decode_all(&dir.join(file), 0);
            let secs = s.len() as f64 / 2.0 / SAMPLE_RATE as f64;
            assert!((secs - 5.0).abs() < 0.1, "{file}: {secs} s");
            for ch in 0..2 {
                let c = channel(&s, ch, 1.0, 4.0);
                let (hz, level) = (frequency(&c), peak(&c));
                assert!((hz - 1000.0).abs() < 1.0, "{file}: {hz} Hz");
                assert!((level - 0.1).abs() < 0.005, "{file}: peak {level}");
            }
        }
    }

    #[test]
    fn each_mkv_stream_decodes_to_stereo() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        for (nth, hz) in [(0, 440.0), (1, 1000.0), (2, 2500.0)] {
            let s = decode_all(&dir.join("three-audio.mkv"), nth);
            let (l, r) = (channel(&s, 0, 1.0, 4.0), channel(&s, 1, 1.0, 4.0));
            assert_eq!(l, r, "stream {nth}: mono source on both sides");
            assert!((frequency(&l) - hz).abs() < 1.0, "stream {nth}");
        }
        assert!(AudioDecoder::open(&dir.join("three-audio.mkv"), 3).is_err());
    }

    #[test]
    fn surround_downmixes_to_stereo() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        // Tone in front-left and centre only; centre reaches both sides, LFE and rears are silent.
        for file in ["tone-5.1.wav", "tone-7.1.wav"] {
            let s = decode_all(&dir.join(file), 0);
            let (l, r) = (channel(&s, 0, 1.0, 4.0), channel(&s, 1, 1.0, 4.0));
            let (pl, pr) = (peak(&l), peak(&r));
            assert!(pl > pr * 1.5 && pr > 0.02, "{file}: L {pl} R {pr}");
            assert!(pl <= 1.0, "{file}: clipped");
            assert!((frequency(&r) - 1000.0).abs() < 1.0, "{file}");
        }
    }

    // shortcut: MP3 is left out; FFmpeg can't shift its timestamps for gapless trimming, so it lands
    // about a sample off. Fix with the stream's skip_samples when someone needs MP3 seeking.
    #[test]
    fn seek_is_sample_accurate() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        for file in ["tone-1khz-44k.wav", "tone-1khz.m4a", "three-audio.mkv"] {
            let path = dir.join(file);
            let all = decode_all(&path, 0);
            let mut d = AudioDecoder::open(&path, 0).unwrap();
            for target in [123_457, 48_000, 200_001] {
                d.seek(target).unwrap();
                let c = d.next_chunk().unwrap().unwrap();
                assert_eq!(c.start, target, "{file}");
                let at = target as usize * 2;
                let want = &all[at..at + c.samples.len().min(200)];
                let max = want
                    .iter()
                    .zip(&c.samples)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0, f32::max);
                // Off by even one sample at 1 kHz would differ by ~0.013.
                assert!(max < 0.002, "{file} @ {target}: {max}");
            }
        }
    }
}
