//! Encodes RGBA frames and stereo f32 audio as an export preset says and muxes them into a file.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use ffmpeg_next::format::{self, Pixel, Sample, sample};
use ffmpeg_next::software::scaling;
use ffmpeg_next::{ChannelLayout, Dictionary, Error, Packet, Rational, codec, encoder, ffi, frame};
use jvid_core::preset::{AudioCodec, Preset, VideoCodec};
use jvid_core::time::Rate;

use crate::audio::SAMPLE_RATE;

/// Software H.264: libx264 only with the `gpl-ffmpeg` feature, per the licensing ADR; OpenH264
/// otherwise; then MPEG-4 Part 2, which every FFmpeg build has.
#[cfg(feature = "gpl-ffmpeg")]
const H264_CODECS: &[&str] = &["libx264", "libopenh264", "mpeg4"];
#[cfg(not(feature = "gpl-ffmpeg"))]
const H264_CODECS: &[&str] = &["libopenh264", "mpeg4"];
/// Software HEVC is x265 only, so a permissive build needs a hardware encoder for HEVC.
#[cfg(feature = "gpl-ffmpeg")]
const HEVC_CODECS: &[&str] = &["libx265"];
#[cfg(not(feature = "gpl-ffmpeg"))]
const HEVC_CODECS: &[&str] = &[];

/// Hardware encoders, best first. They all take NV12 (VAAPI after an upload).
const HW_H264: &[&str] = &[
    "h264_nvenc",
    "h264_qsv",
    "h264_amf",
    "h264_videotoolbox",
    "h264_vaapi",
];
const HW_HEVC: &[&str] = &[
    "hevc_nvenc",
    "hevc_qsv",
    "hevc_amf",
    "hevc_videotoolbox",
    "hevc_vaapi",
];

/// Software encoder names to try, the pixel format they take, and the hardware encoders.
/// shortcut: GIF uses a fixed 3-3-2 palette; a generated palette is a follow-up.
fn video_codec(c: VideoCodec) -> (&'static [&'static str], Pixel, &'static [&'static str]) {
    match c {
        VideoCodec::H264 => (H264_CODECS, Pixel::YUV420P, HW_H264),
        VideoCodec::Hevc => (HEVC_CODECS, Pixel::YUV420P, HW_HEVC),
        VideoCodec::ProRes => (&["prores_ks", "prores"], Pixel::YUV422P10LE, &[]),
        VideoCodec::Gif => (&["gif"], Pixel::RGB8, &[]),
    }
}

/// The hardware encoders that open on this machine, found by test-opening each one once.
/// Call `ffmpeg::init` first.
pub fn hardware() -> &'static [&'static str] {
    static HW: OnceLock<Vec<&'static str>> = OnceLock::new();
    HW.get_or_init(|| {
        HW_H264
            .iter()
            .chain(HW_HEVC)
            .copied()
            .filter(|n| {
                open_video(
                    n,
                    Pixel::NV12,
                    (256, 256),
                    Rate::new(30, 1),
                    false,
                    &BTreeMap::new(),
                )
                .is_some()
            })
            .collect()
    })
}

/// Encoders to try for `codec`, best first: the `available` hardware ones (unless the preset's
/// `encoder` option is `software`), then software.
fn candidates(
    codec: VideoCodec,
    available: &[&str],
    options: &BTreeMap<String, String>,
) -> Vec<(&'static str, Pixel)> {
    let (software, pixel, hw) = video_codec(codec);
    let use_hw = options.get("encoder").is_none_or(|e| e != "software");
    hw.iter()
        .filter(|n| use_hw && available.contains(n))
        .map(|n| (*n, Pixel::NV12))
        .chain(software.iter().map(|n| (*n, pixel)))
        .collect()
}

/// The preset's quality (`crf`, x264's 0–51 scale, default 20) as `name`'s own rate control.
/// Encoders without a constant-quality mode get a bit rate that halves every 6 steps, like crf.
fn rate_control(
    name: &str,
    crf: Option<&String>,
    (w, h): (u32, u32),
) -> Vec<(&'static str, String)> {
    let crf = crf
        .and_then(|c| c.parse::<f64>().ok())
        .unwrap_or(20.0)
        .clamp(0.0, 51.0);
    let q = format!("{}", crf.round());
    let bits = (w * h) as f64 * 6.0 * 2f64.powf((20.0 - crf) / 6.0);
    // Constant-quality modes get `b = 0`, or the encoder would cap or target that rate instead.
    let zero = ("b", "0".to_string());
    match name.rsplit('_').next() {
        Some("nvenc") => vec![("rc", "vbr".into()), ("cq", q), zero],
        Some("qsv") => vec![("global_quality", q), zero],
        Some("amf") => vec![
            ("rc", "cqp".into()),
            ("qp_i", q.clone()),
            ("qp_p", q.clone()),
            ("qp_b", q),
            zero,
        ],
        Some("vaapi") => vec![("rc_mode", "CQP".into()), ("qp", q), zero],
        // x264/x265 use crf; VideoToolbox, OpenH264 and MPEG-4 use the bit rate.
        _ => vec![("crf", q), ("b", format!("{}", bits as u64))],
    }
}

/// Opens encoder `name` taking `pixel` frames, with the preset's options mapped onto it.
fn open_video(
    name: &str,
    pixel: Pixel,
    (width, height): (u32, u32),
    rate: Rate,
    global: bool,
    options: &BTreeMap<String, String>,
) -> Option<(ffmpeg_next::Codec, encoder::Video, Option<HwFrames>)> {
    let c = encoder::find_by_name(name)?;
    let mut e = codec::Context::new_with_codec(c).encoder().video().ok()?;
    e.set_width(width);
    e.set_height(height);
    e.set_time_base(Rational(rate.den as i32, rate.num as i32));
    e.set_frame_rate(Some(Rational(rate.num as i32, rate.den as i32)));
    if global {
        e.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let hw = name.ends_with("_vaapi");
    let frames = if hw {
        let f = HwFrames::new(width, height)?;
        e.set_format(Pixel::from(ffi::AVPixelFormat::AV_PIX_FMT_VAAPI));
        f.attach(&mut e);
        Some(f)
    } else {
        e.set_format(pixel);
        None
    };
    let mut opts = Dictionary::new();
    let software = HW_H264.iter().chain(HW_HEVC).all(|n| *n != name);
    if software {
        opts.set("preset", "veryfast");
    }
    for (k, v) in rate_control(name, options.get("crf"), (width, height)) {
        opts.set(k, &v);
    }
    // x264's preset and tune values aren't valid for hardware encoders and would fail the open;
    // other encoders ignore options they don't have.
    for (k, v) in options {
        if k != "crf" && k != "encoder" && (software || k != "preset" && k != "tune") {
            opts.set(k, v);
        }
    }
    Some((c, e.open_with(opts).ok()?, frames))
}

/// A VAAPI frame pool: VAAPI encoders take GPU frames, so NV12 frames are uploaded first.
/// ffmpeg-next has no hardware-frame API, so this is the one `unsafe` corner of the encoder.
struct HwFrames(*mut ffi::AVBufferRef);

#[allow(unsafe_code)]
// SAFETY: the pool is only used by the encoder that owns it, one thread at a time.
unsafe impl Send for HwFrames {}

#[allow(unsafe_code)]
impl HwFrames {
    /// Gives the encoder its own reference to the pool.
    fn attach(&self, e: &mut encoder::video::Video) {
        // SAFETY: the context owns the new reference and unrefs it when freed.
        unsafe { (*e.as_mut_ptr()).hw_frames_ctx = ffi::av_buffer_ref(self.0) };
    }

    fn new(width: u32, height: u32) -> Option<Self> {
        // SAFETY: plain FFmpeg calls; every failure path unrefs what it allocated.
        unsafe {
            let mut device = std::ptr::null_mut();
            let t = ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI;
            if ffi::av_hwdevice_ctx_create(
                &mut device,
                t,
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
            ) < 0
            {
                return None;
            }
            let frames = Self(ffi::av_hwframe_ctx_alloc(device));
            ffi::av_buffer_unref(&mut device);
            if frames.0.is_null() {
                return None;
            }
            let fc = (*frames.0).data as *mut ffi::AVHWFramesContext;
            (*fc).format = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
            (*fc).sw_format = ffi::AVPixelFormat::AV_PIX_FMT_NV12;
            (*fc).width = width as i32;
            (*fc).height = height as i32;
            (*fc).initial_pool_size = 20;
            (ffi::av_hwframe_ctx_init(frames.0) >= 0).then_some(frames)
        }
    }

    fn upload(&self, src: &frame::Video) -> Result<frame::Video, Error> {
        let mut dst = frame::Video::empty();
        // SAFETY: `dst` gets a pool frame, then `src`'s pixels are copied into it.
        unsafe {
            for r in [
                ffi::av_hwframe_get_buffer(self.0, dst.as_mut_ptr(), 0),
                ffi::av_hwframe_transfer_data(dst.as_mut_ptr(), src.as_ptr(), 0),
            ] {
                if r < 0 {
                    return Err(Error::from(r));
                }
            }
        }
        Ok(dst)
    }
}

#[allow(unsafe_code)]
impl Drop for HwFrames {
    fn drop(&mut self) {
        // SAFETY: we own this reference.
        unsafe { ffi::av_buffer_unref(&mut self.0) };
    }
}

fn audio_codec(c: AudioCodec) -> (&'static str, Sample) {
    match c {
        AudioCodec::Aac => ("aac", Sample::F32(sample::Type::Planar)),
        AudioCodec::Mp3 => ("libmp3lame", Sample::F32(sample::Type::Planar)),
        AudioCodec::Pcm => ("pcm_s16le", Sample::I16(sample::Type::Packed)),
    }
}

struct Video {
    stream: Stream<encoder::Video>,
    scaler: scaling::Context,
    hw: Option<HwFrames>,
    name: &'static str,
}

struct Stream<E> {
    enc: E,
    index: usize,
    time_base: Rational,
}

/// One output file. Push every frame and sample in order, then [`Encoder::finish`].
pub struct Encoder {
    out: format::context::Output,
    video: Option<Video>,
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
                let (name, pixel, codec, enc, hw) = candidates(vc, hardware(), &preset.options)
                    .into_iter()
                    .find_map(|(n, pixel)| {
                        let (c, e, hw) =
                            open_video(n, pixel, (width, height), rate, global, &preset.options)?;
                        Some((n, pixel, c, e, hw))
                    })
                    .ok_or(Error::EncoderNotFound)?;
                tracing::info!(encoder = name, "video encoder");
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
                Some(Video {
                    stream,
                    scaler,
                    hw,
                    name,
                })
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

    /// The FFmpeg encoder picked for video, e.g. `h264_nvenc` or `libx264`.
    pub fn video_encoder(&self) -> Option<&'static str> {
        self.video.as_ref().map(|v| v.name)
    }

    /// Encodes the next frame: tight RGBA rows at the source size given to [`Encoder::create`].
    /// Ignored without a video stream.
    pub fn video(&mut self, rgba: &[u8]) -> Result<(), Error> {
        let Some(Video {
            stream, scaler, hw, ..
        }) = &mut self.video
        else {
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
        if let Some(hw) = hw {
            yuv = hw.upload(&yuv)?;
        }
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
        if let Some(Video { stream: v, .. }) = &mut self.video {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn hardware_first_then_software() {
        let names = |c, hw: &[&str], o| -> Vec<_> {
            candidates(c, hw, &o).into_iter().map(|(n, _)| n).collect()
        };
        let hw = ["h264_vaapi", "h264_nvenc", "hevc_qsv"];
        let got = names(VideoCodec::H264, &hw, opts(&[]));
        assert_eq!(got[..2], ["h264_nvenc", "h264_vaapi"]);
        assert_eq!(got[2..], *H264_CODECS);
        assert_eq!(names(VideoCodec::H264, &[], opts(&[])), H264_CODECS);
        let soft = opts(&[("encoder", "software")]);
        assert_eq!(names(VideoCodec::H264, &hw, soft), H264_CODECS);
        assert_eq!(names(VideoCodec::Hevc, &hw, opts(&[]))[0], "hevc_qsv");
        assert_eq!(names(VideoCodec::ProRes, &hw, opts(&[]))[0], "prores_ks");
        assert_eq!(
            candidates(VideoCodec::H264, &hw, &opts(&[]))[0].1,
            Pixel::NV12
        );
    }

    #[test]
    fn quality_maps_to_each_rate_control() {
        let rc = |n, crf: Option<&str>| rate_control(n, crf.map(String::from).as_ref(), (100, 100));
        let get = |o: &[(&str, String)], k| o.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        let nv = rc("h264_nvenc", Some("18"));
        assert_eq!(get(&nv, "cq").as_deref(), Some("18"));
        assert_eq!(get(&nv, "b").as_deref(), Some("0"));
        assert_eq!(
            get(&rc("hevc_qsv", None), "global_quality").as_deref(),
            Some("20")
        );
        assert_eq!(
            get(&rc("h264_amf", Some("14")), "qp_p").as_deref(),
            Some("14")
        );
        assert_eq!(
            get(&rc("h264_vaapi", Some("99")), "qp").as_deref(),
            Some("51")
        );
        // Bit-rate encoders: 6 bits per pixel at crf 20, half that 6 steps worse.
        assert_eq!(
            get(&rc("h264_videotoolbox", None), "b").as_deref(),
            Some("60000")
        );
        assert_eq!(
            get(&rc("libopenh264", Some("26")), "b").as_deref(),
            Some("30000")
        );
        assert_eq!(
            get(&rc("libx264", Some("18")), "crf").as_deref(),
            Some("18")
        );
    }

    /// CI has no hardware encoder, so this exports through the software fallback.
    #[test]
    fn exports_without_hardware() {
        crate::ffmpeg::init().unwrap();
        let path = std::env::temp_dir().join(format!("jvid-hwenc-{}.mp4", std::process::id()));
        let preset = jvid_core::preset::builtin()
            .into_iter()
            .find(|p| p.video == Some(VideoCodec::H264))
            .unwrap();
        let mut enc = Encoder::create(&path, (64, 64), Rate::new(30, 1), &preset).unwrap();
        let name = enc.video_encoder().unwrap();
        assert!(
            hardware().contains(&name) || H264_CODECS.contains(&name),
            "{name}"
        );
        for _ in 0..10 {
            enc.video(&[128; 64 * 64 * 4]).unwrap();
        }
        enc.finish().unwrap();
        let info = crate::probe::probe(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(info.width.is_some());
    }
}
