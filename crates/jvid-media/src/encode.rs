//! Encodes RGBA frames and stereo f32 audio and muxes them into an MP4, MOV or MKV file.

use std::path::Path;

use ffmpeg_next::format::{self, Pixel, Sample, sample};
use ffmpeg_next::software::scaling;
use ffmpeg_next::{ChannelLayout, Dictionary, Error, Packet, Rational, codec, encoder, frame};
use jvid_core::time::Rate;

use crate::audio::SAMPLE_RATE;

/// H.264 first (libx264 only with the `gpl-ffmpeg` feature, per the licensing ADR; OpenH264
/// otherwise), then MPEG-4 Part 2, which every FFmpeg build has.
/// shortcut: fixed codecs and quality; presets (#53) and hardware encoders (#55) choose later.
#[cfg(feature = "gpl-ffmpeg")]
const VIDEO_CODECS: &[&str] = &["libx264", "libopenh264", "mpeg4"];
#[cfg(not(feature = "gpl-ffmpeg"))]
const VIDEO_CODECS: &[&str] = &["libopenh264", "mpeg4"];

struct Stream<E> {
    enc: E,
    index: usize,
    time_base: Rational,
}

/// One output file. Push every frame and sample in order, then [`Encoder::finish`].
pub struct Encoder {
    out: format::context::Output,
    video: Stream<encoder::Video>,
    audio: Option<Stream<encoder::Audio>>,
    scaler: scaling::Context,
    frames: i64,
    /// Interleaved samples not yet a whole encoder frame.
    pending: Vec<f32>,
    samples: i64,
}

impl Encoder {
    /// Creates `path` (container from its extension) for `width`×`height` RGBA frames at `rate`,
    /// with 48 kHz stereo AAC when `audio`. Call `ffmpeg::init` first.
    pub fn create(
        path: &Path,
        (width, height): (u32, u32),
        rate: Rate,
        audio: bool,
    ) -> Result<Self, Error> {
        let mut out = format::output(path)?;
        let global = out.format().flags().contains(format::Flags::GLOBAL_HEADER);
        let frame_tb = Rational(rate.den as i32, rate.num as i32);
        let (video_codec, video_enc) = VIDEO_CODECS
            .iter()
            .filter_map(|n| encoder::find_by_name(n))
            .find_map(|c| {
                let mut e = codec::Context::new_with_codec(c).encoder().video().ok()?;
                e.set_width(width);
                e.set_height(height);
                e.set_format(Pixel::YUV420P);
                e.set_time_base(frame_tb);
                e.set_frame_rate(Some(Rational(rate.num as i32, rate.den as i32)));
                e.set_bit_rate((width * height) as usize * 6);
                if global {
                    e.set_flags(codec::Flags::GLOBAL_HEADER);
                }
                // Encoders ignore options they don't have; libx264's crf overrides the bit rate.
                let mut opts = Dictionary::new();
                opts.set("crf", "20");
                opts.set("preset", "veryfast");
                Some((c, e.open_with(opts).ok()?))
            })
            .ok_or(Error::EncoderNotFound)?;
        let video = Stream {
            index: add(&mut out, video_codec, frame_tb, &video_enc)?,
            enc: video_enc,
            time_base: frame_tb,
        };
        let audio = if audio {
            let c = encoder::find(codec::Id::AAC).ok_or(Error::EncoderNotFound)?;
            let mut e = codec::Context::new_with_codec(c).encoder().audio()?;
            e.set_rate(SAMPLE_RATE as i32);
            e.set_channel_layout(ChannelLayout::STEREO);
            e.set_format(Sample::F32(sample::Type::Planar));
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
        } else {
            None
        };
        out.write_header()?;
        let scaler = scaling::Context::get(
            Pixel::RGBA,
            width,
            height,
            Pixel::YUV420P,
            width,
            height,
            scaling::Flags::BILINEAR,
        )?;
        Ok(Self {
            out,
            video,
            audio,
            scaler,
            frames: 0,
            pending: Vec::new(),
            samples: 0,
        })
    }

    /// Encodes the next frame: tight RGBA rows at the size given to [`Encoder::create`].
    pub fn video(&mut self, rgba: &[u8]) -> Result<(), Error> {
        let (w, h) = (self.video.enc.width(), self.video.enc.height());
        let mut src = frame::Video::new(Pixel::RGBA, w, h);
        let stride = src.stride(0);
        for (row, line) in rgba.chunks_exact(w as usize * 4).enumerate() {
            src.data_mut(0)[row * stride..][..line.len()].copy_from_slice(line);
        }
        let mut yuv = frame::Video::empty();
        self.scaler.run(&src, &mut yuv)?;
        yuv.set_pts(Some(self.frames));
        self.frames += 1;
        self.video.enc.send_frame(&yuv)?;
        drain(&mut self.out, &mut self.video, |e, p| e.receive_packet(p))
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
            for (i, s) in self.pending.drain(..n * 2).enumerate() {
                f.plane_mut::<f32>(i % 2)[i / 2] = s;
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
        self.video.enc.send_eof()?;
        drain(&mut self.out, &mut self.video, |e, p| e.receive_packet(p))?;
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
