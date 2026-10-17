//! Encodes RGBA frames and stereo f32 audio as an export preset says and muxes them into a file.

use std::path::Path;

use ffmpeg_next::format::{self, Pixel, Sample, sample};
use ffmpeg_next::software::scaling;
use ffmpeg_next::{ChannelLayout, Dictionary, Error, Packet, Rational, codec, encoder, frame};
use jvid_core::preset::{AudioCodec, Preset, VideoCodec};
use jvid_core::time::Rate;

use crate::audio::SAMPLE_RATE;

/// H.264 first (libx264 only with the `gpl-ffmpeg` feature, per the licensing ADR; OpenH264
/// otherwise), then MPEG-4 Part 2, which every FFmpeg build has.
/// shortcut: software only; hardware encoders (#55) choose later.
#[cfg(feature = "gpl-ffmpeg")]
const H264_CODECS: &[&str] = &["libx264", "libopenh264", "mpeg4"];
#[cfg(not(feature = "gpl-ffmpeg"))]
const H264_CODECS: &[&str] = &["libopenh264", "mpeg4"];

/// Encoder names to try and the pixel format they take.
/// shortcut: GIF uses a fixed 3-3-2 palette; a generated palette is a follow-up.
fn video_codec(c: VideoCodec) -> (&'static [&'static str], Pixel) {
    match c {
        VideoCodec::H264 => (H264_CODECS, Pixel::YUV420P),
        VideoCodec::ProRes => (&["prores_ks", "prores"], Pixel::YUV422P10LE),
        VideoCodec::Gif => (&["gif"], Pixel::RGB8),
    }
}

fn audio_codec(c: AudioCodec) -> (&'static str, Sample) {
    match c {
        AudioCodec::Aac => ("aac", Sample::F32(sample::Type::Planar)),
        AudioCodec::Mp3 => ("libmp3lame", Sample::F32(sample::Type::Planar)),
        AudioCodec::Pcm => ("pcm_s16le", Sample::I16(sample::Type::Packed)),
    }
}

struct Stream<E> {
    enc: E,
    index: usize,
    time_base: Rational,
}

/// One output file. Push every frame and sample in order, then [`Encoder::finish`].
pub struct Encoder {
    out: format::context::Output,
    video: Option<(Stream<encoder::Video>, scaling::Context)>,
    audio: Option<Stream<encoder::Audio>>,
    /// Size of the RGBA frames pushed in.
    src: (u32, u32),
    frames: i64,
    /// Interleaved samples not yet a whole encoder frame.
    pending: Vec<f32>,
    samples: i64,
}

impl Encoder {
    /// Creates `path` (container from its extension) for `src`-sized RGBA frames at `rate` and
    /// 48 kHz stereo audio, encoded as `preset` says (its size and rate override `src` and
    /// `rate`). Call `ffmpeg::init` first.
    pub fn create(
        path: &Path,
        src: (u32, u32),
        rate: Rate,
        preset: &Preset,
    ) -> Result<Self, Error> {
        let mut out = format::output(path)?;
        let global = out.format().flags().contains(format::Flags::GLOBAL_HEADER);
        let rate = preset.frame_rate.unwrap_or(rate);
        let (width, height) = preset.size.map_or(src, |[w, h]| (w, h));
        let frame_tb = Rational(rate.den as i32, rate.num as i32);
        let video = match preset.video {
            None => None,
            Some(vc) => {
                let (names, pixel) = video_codec(vc);
                let (codec, enc) = names
                    .iter()
                    .filter_map(|n| encoder::find_by_name(n))
                    .find_map(|c| {
                        let mut e = codec::Context::new_with_codec(c).encoder().video().ok()?;
                        e.set_width(width);
                        e.set_height(height);
                        e.set_format(pixel);
                        e.set_time_base(frame_tb);
                        e.set_frame_rate(Some(Rational(rate.num as i32, rate.den as i32)));
                        e.set_bit_rate((width * height) as usize * 6);
                        if global {
                            e.set_flags(codec::Flags::GLOBAL_HEADER);
                        }
                        // Encoders ignore options they don't have; libx264's crf overrides the
                        // bit rate.
                        let mut opts = Dictionary::new();
                        opts.set("crf", "20");
                        opts.set("preset", "veryfast");
                        for (k, v) in &preset.options {
                            opts.set(k, v);
                        }
                        Some((c, e.open_with(opts).ok()?))
                    })
                    .ok_or(Error::EncoderNotFound)?;
                let stream = Stream {
                    index: add(&mut out, codec, frame_tb, &enc)?,
                    enc,
                    time_base: frame_tb,
                };
                let flags = if (width, height) == src {
                    scaling::Flags::POINT
                } else {
                    scaling::Flags::BICUBIC
                };
                let scaler =
                    scaling::Context::get(Pixel::RGBA, src.0, src.1, pixel, width, height, flags)?;
                Some((stream, scaler))
            }
        };
        let audio = match preset.audio {
            None => None,
            Some(ac) => {
                let (name, format) = audio_codec(ac);
                let c = encoder::find_by_name(name).ok_or(Error::EncoderNotFound)?;
                let mut e = codec::Context::new_with_codec(c).encoder().audio()?;
                e.set_rate(SAMPLE_RATE as i32);
                e.set_channel_layout(ChannelLayout::STEREO);
                e.set_format(format);
                e.set_bit_rate(192_000);
                e.set_time_base(Rational(1, SAMPLE_RATE as i32));
                if global {
                    e.set_flags(codec::Flags::GLOBAL_HEADER);
                }
                let enc = e.open_as(c)?;
                let time_base = Rational(1, SAMPLE_RATE as i32);
                Some(Stream {
                    index: add(&mut out, c, time_base, &enc)?,
                    enc,
                    time_base,
                })
            }
        };
        out.write_header()?;
        Ok(Self {
            out,
            video,
            audio,
            src,
            frames: 0,
            pending: Vec::new(),
            samples: 0,
        })
    }

    pub fn has_video(&self) -> bool {
        self.video.is_some()
    }

    /// Encodes the next frame: tight RGBA rows at the source size given to [`Encoder::create`].
    /// Ignored without a video stream.
    pub fn video(&mut self, rgba: &[u8]) -> Result<(), Error> {
        let Some((stream, scaler)) = &mut self.video else {
            return Ok(());
        };
        let (w, h) = self.src;
        let mut src = frame::Video::new(Pixel::RGBA, w, h);
        let stride = src.stride(0);
        for (row, line) in rgba.chunks_exact(w as usize * 4).enumerate() {
            src.data_mut(0)[row * stride..][..line.len()].copy_from_slice(line);
        }
        let mut yuv = frame::Video::empty();
        scaler.run(&src, &mut yuv)?;
        yuv.set_pts(Some(self.frames));
        self.frames += 1;
        stream.enc.send_frame(&yuv)?;
        drain(&mut self.out, stream, |e, p| e.receive_packet(p))
    }

    /// Encodes the next interleaved stereo samples; ignored without an audio stream.
    pub fn audio(&mut self, samples: &[f32]) -> Result<(), Error> {
        self.pending.extend_from_slice(samples);
        self.flush_audio(false)
    }

    /// Sends whole encoder frames of the pending samples, and the remainder too when `last`.
    fn flush_audio(&mut self, last: bool) -> Result<(), Error> {
        let Some(a) = &mut self.audio else {
            self.pending.clear();
            return Ok(());
        };
        // A frame size of 0 means the codec takes any size.
        let size = match a.enc.frame_size() as usize {
            0 => self.pending.len() / 2,
            n => n,
        };
        while size > 0 && (self.pending.len() >= size * 2 || last && !self.pending.is_empty()) {
            let n = size.min(self.pending.len() / 2);
            let mut f = frame::Audio::new(a.enc.format(), n, ChannelLayout::STEREO);
            f.set_rate(SAMPLE_RATE);
            let samples = self.pending.drain(..n * 2);
            if f.is_planar() {
                for (i, s) in samples.enumerate() {
                    f.plane_mut::<f32>(i % 2)[i / 2] = s;
                }
            } else {
                // Packed is only ever 16-bit PCM; plane_mut would only cover one channel.
                let data = f.data_mut(0);
                for (i, s) in samples.enumerate() {
                    let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    data[i * 2..][..2].copy_from_slice(&v.to_le_bytes());
                }
            }
            f.set_pts(Some(self.samples));
            self.samples += n as i64;
            a.enc.send_frame(&f)?;
            drain(&mut self.out, a, |e, p| e.receive_packet(p))?;
        }
        Ok(())
    }

    /// Flushes the encoders and writes the trailer.
    pub fn finish(mut self) -> Result<(), Error> {
        self.flush_audio(true)?;
        if let Some((v, _)) = &mut self.video {
            v.enc.send_eof()?;
            drain(&mut self.out, v, |e, p| e.receive_packet(p))?;
        }
        if let Some(a) = &mut self.audio {
            a.enc.send_eof()?;
            drain(&mut self.out, a, |e, p| e.receive_packet(p))?;
        }
        self.out.write_trailer()
    }
}

/// Adds a stream for an opened encoder; returns its index.
fn add(
    out: &mut format::context::Output,
    codec: ffmpeg_next::Codec,
    time_base: Rational,
    enc: impl Into<codec::Parameters>,
) -> Result<usize, Error> {
    let mut s = out.add_stream(codec)?;
    s.set_parameters(enc);
    s.set_time_base(time_base);
    Ok(s.index())
}

/// Writes every packet the encoder has ready.
fn drain<E>(
    out: &mut format::context::Output,
    s: &mut Stream<E>,
    receive: impl Fn(&mut E, &mut Packet) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut p = Packet::empty();
    loop {
        match receive(&mut s.enc, &mut p) {
            Ok(()) => {}
            Err(Error::Eof) => return Ok(()),
            Err(Error::Other { errno }) if errno == ffmpeg_next::util::error::EAGAIN => {
                return Ok(());
            }
            Err(e) => return Err(e),
        }
        p.set_stream(s.index);
        // The muxer may have changed the stream's time base in write_header.
        let to = out.stream(s.index).expect("added").time_base();
        p.rescale_ts(s.time_base, to);
        p.write_interleaved(out)?;
    }
}
