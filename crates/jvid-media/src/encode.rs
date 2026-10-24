//! Encodes RGBA frames and stereo f32 audio as an export preset says and muxes them into a file.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use ffmpeg_next::color::{Primaries, Range, Space, TransferCharacteristic};
use ffmpeg_next::format::{self, Pixel, Sample, sample};
use ffmpeg_next::software::scaling;
use ffmpeg_next::{
    ChannelLayout, Dictionary, Error, Packet, Rational, codec, encoder, filter, frame,
};
use jvid_core::hdr::{BT709_TO_BT2020, REFERENCE_WHITE, nits_to_pq};
use jvid_core::preset::{AudioCodec, Preset, VideoCodec};
use jvid_core::time::Rate;

use crate::audio::SAMPLE_RATE;
use crate::hwenc;

/// Keeps VAAPI upload/codec failures distinct from conversion and output I/O errors, so a
/// caller that can replay its input may retry once with a fresh software encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    Vaapi(Error),
    Other(Error),
}

impl EncodeError {
    fn video(error: Error, vaapi: bool) -> Self {
        if vaapi {
            Self::Vaapi(error)
        } else {
            Self::Other(error)
        }
    }
}

impl From<Error> for EncodeError {
    fn from(error: Error) -> Self {
        Self::Other(error)
    }
}

// Callers without replay support (e.g. software-only proxy generation) can keep using FFmpeg
// errors. Shared export retains EncodeError until it decides whether to retry.
impl From<EncodeError> for Error {
    fn from(error: EncodeError) -> Self {
        match error {
            EncodeError::Vaapi(e) | EncodeError::Other(e) => e,
        }
    }
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vaapi(e) => write!(f, "VAAPI upload/encode: {e}"),
            Self::Other(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for EncodeError {}

/// Software H.264: libx264 only with the `gpl-ffmpeg` feature, per the licensing ADR; OpenH264
/// otherwise; then MPEG-4 Part 2, which every FFmpeg build has.
#[cfg(feature = "gpl-ffmpeg")]
const H264_CODECS: &[&str] = &["libx264", "libopenh264", "mpeg4"];
#[cfg(not(feature = "gpl-ffmpeg"))]
const H264_CODECS: &[&str] = &["libopenh264", "mpeg4"];
/// Software HEVC is x265 only, so a permissive build needs a hardware encoder for HEVC. Tests
/// try x265 too, so HDR export runs where FFmpeg has it.
#[cfg(any(feature = "gpl-ffmpeg", test))]
const HEVC_CODECS: &[&str] = &["libx265"];
#[cfg(not(any(feature = "gpl-ffmpeg", test)))]
const HEVC_CODECS: &[&str] = &[];

/// Hardware encoders, best first. They all take NV12 (VAAPI after an upload; see `hwenc`).
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
fn video_codec(c: VideoCodec) -> (&'static [&'static str], Pixel, &'static [&'static str]) {
    match c {
        VideoCodec::H264 => (H264_CODECS, Pixel::YUV420P, HW_H264),
        VideoCodec::Hevc => (HEVC_CODECS, Pixel::YUV420P, HW_HEVC),
        VideoCodec::ProRes => (&["prores_ks", "prores"], Pixel::YUV422P10LE, &[]),
        VideoCodec::Gif => (&["gif"], Pixel::PAL8, &[]),
    }
}

/// The hardware encoders that open on this machine, including a VAAPI upload probe.
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
    // HDR10 is 10-bit: P010 into hardware, except VAAPI, whose upload pool is NV12.
    let hdr = is_hdr10(codec, options);
    let (hw_pixel, pixel) = if hdr {
        (Pixel::P010LE, Pixel::YUV420P10LE)
    } else {
        (Pixel::NV12, pixel)
    };
    hw.iter()
        .filter(|n| use_hw && available.contains(n) && !(hdr && hwenc::is_vaapi(n)))
        .map(|n| (*n, hw_pixel))
        .chain(software.iter().map(|n| (*n, pixel)))
        .collect()
}

/// Whether an HDR10 export has an encoder to try: x265 in this build, or a working hardware HEVC
/// encoder other than VAAPI. Call `ffmpeg::init` first.
pub fn hdr10_available() -> bool {
    let pq = BTreeMap::from([("hdr".to_string(), "pq".to_string())]);
    candidates(VideoCodec::Hevc, hardware(), &pq)
        .iter()
        .any(|(n, _)| encoder::find_by_name(n).is_some())
}

/// The preset asks for HDR10 (`hdr=pq`, set by `Preset::hdr`), which is HEVC only.
fn is_hdr10(codec: VideoCodec, options: &BTreeMap<String, String>) -> bool {
    codec == VideoCodec::Hevc && options.get("hdr").is_some_and(|v| v == "pq")
}

/// HDR10 export carries only the SDR composite (see [`Hdr10`]), so no pixel is brighter than
/// reference white: MaxCLL and MaxFALL.
const MAX_CLL: u32 = REFERENCE_WHITE as u32;

/// x265's HDR10 SEI: the mastering display [`hwenc::hdr10_metadata`] gives hardware encoders.
const X265_HDR10: &str = "master-display=G(13250,34500)B(7500,3000)R(34000,16000)\
                          WP(15635,16450)L(10000000,1):max-cll=203,203";

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
        Some("vaapi") => vec![("rc_mode", "CQP".into()), ("qp", q), zero],
        Some("amf") => vec![
            ("rc", "cqp".into()),
            ("qp_i", q.clone()),
            ("qp_p", q.clone()),
            ("qp_b", q),
            zero,
        ],
        // x264/x265 use crf; VideoToolbox, OpenH264 and MPEG-4 use the bit rate.
        _ => vec![("crf", q), ("b", format!("{}", bits as u64))],
    }
}

/// Opens encoder `name` taking `pixel` frames, with the preset's options mapped onto it,
/// tagged and configured for HDR10 when `hdr`.
pub(crate) fn open_video(
    name: &str,
    pixel: Pixel,
    (width, height): (u32, u32),
    rate: Rate,
    global: bool,
    hdr: bool,
    options: &BTreeMap<String, String>,
) -> Option<(ffmpeg_next::Codec, encoder::Video)> {
    let c = encoder::find_by_name(name)?;
    let mut e = codec::Context::new_with_codec(c).encoder().video().ok()?;
    e.set_width(width);
    e.set_height(height);
    e.set_time_base(Rational(rate.den as i32, rate.num as i32));
    e.set_frame_rate(Some(Rational(rate.num as i32, rate.den as i32)));
    if global {
        e.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    if hwenc::is_vaapi(name) {
        if !hwenc::attach(&mut e, (width, height)) {
            return None;
        }
        e.set_format(ffmpeg_next::ffi::AVPixelFormat::AV_PIX_FMT_VAAPI.into());
    } else {
        e.set_format(pixel);
    }
    let mut opts = Dictionary::new();
    if hdr {
        e.set_color_primaries(Primaries::BT2020);
        e.set_color_transfer_characteristic(TransferCharacteristic::SMPTE2084);
        e.set_colorspace(Space::BT2020NCL);
        e.set_color_range(Range::MPEG);
        if name == "libx265" {
            opts.set("x265-params", X265_HDR10);
        }
    }
    let software = HW_H264.iter().chain(HW_HEVC).all(|n| *n != name);
    if software {
        opts.set("preset", "veryfast");
    }
    // An explicit `b` (target-size export) is average bit rate mode, so no constant quality.
    if !options.contains_key("b") {
        for (k, v) in rate_control(name, options.get("crf"), (width, height)) {
            opts.set(k, &v);
        }
    }
    // x264's preset and tune values aren't valid for hardware encoders and would fail the open;
    // other encoders ignore options they don't have.
    for (k, v) in options {
        if k != "crf" && k != "encoder" && (software || k != "preset" && k != "tune") {
            opts.set(k, v);
        }
    }
    let e = e.open_with(opts).ok()?;
    ready_video(name, e, |e| hwenc::probe_upload(e, (width, height))).map(|e| (c, e))
}

/// Reject a candidate before its stream parameters or header are committed. Probe the actual
/// output dimensions/options on every open, even when the cached 256px hardware probe passed.
fn ready_video<T>(
    name: &str,
    encoder: T,
    probe: impl FnOnce(&T) -> Result<(), Error>,
) -> Option<T> {
    if hwenc::is_vaapi(name)
        && let Err(error) = probe(&encoder)
    {
        tracing::warn!(name, %error, "VAAPI upload probe failed; trying next encoder");
        return None;
    }
    Some(encoder)
}

fn audio_codec(c: AudioCodec) -> (&'static str, Sample) {
    match c {
        AudioCodec::Aac => ("aac", Sample::F32(sample::Type::Planar)),
        AudioCodec::Mp3 => ("libmp3lame", Sample::F32(sample::Type::Planar)),
        AudioCodec::Pcm => ("pcm_s16le", Sample::I16(sample::Type::Packed)),
        AudioCodec::Pcm24 => ("pcm_s24le", Sample::I32(sample::Type::Packed)),
    }
}

struct Video {
    stream: Stream<encoder::Video>,
    convert: Convert,
    name: &'static str,
}

/// How pushed RGBA frames become the encoder's frames.
enum Convert {
    Scale(scaling::Context),
    /// GIF: scales, then palettegen/paletteuse. The palette comes from every frame, so the
    /// graph holds them all until [`Encoder::finish`].
    /// shortcut: buffers the whole export in memory (~1.6 GB for 10 s of 1080p at 25 fps); switch
    /// to a two-pass or per-frame palette if long GIFs matter.
    Palette(filter::Graph),
    /// HDR10: resized as RGBA first when the output size differs, then mapped on the CPU.
    Pq(Option<scaling::Context>, Box<Hdr10>),
}

/// Maps the SDR composite into HDR10 (PQ, BT.2020, limited-range 10-bit) per BT.2408: BT.1886
/// decode (gamma 2.4), SDR 100% at the 203 cd/m² reference white, BT.709 to BT.2020 primaries in
/// linear light, PQ encode. Highlights stay at reference white: the compositor is 8-bit SDR.
struct Hdr10 {
    /// 8-bit code to linear light, 0–1.
    linear: [f32; 256],
    /// PQ signal at linear light `(i / LUT)²`: square-root steps keep the shadows fine.
    pq: Vec<f32>,
}

const LUT: usize = 4095;

impl Hdr10 {
    fn new() -> Self {
        Self {
            linear: std::array::from_fn(|c| (c as f32 / 255.0).powf(2.4)),
            pq: (0..=LUT)
                .map(|i| nits_to_pq((i as f32 / LUT as f32).powi(2) * REFERENCE_WHITE))
                .collect(),
        }
    }

    /// One SDR pixel's PQ BT.2020 R'G'B'.
    fn rgb(&self, px: &[u8]) -> [f32; 3] {
        let l = [0, 1, 2].map(|c| self.linear[px[c] as usize]);
        BT709_TO_BT2020.map(|m| {
            let v = m[0] * l[0] + m[1] * l[1] + m[2] * l[2];
            self.pq[(v.clamp(0.0, 1.0).sqrt() * LUT as f32 + 0.5) as usize]
        })
    }

    /// `w`×`h` RGBA rows `stride` bytes apart as 10-bit codes: Y, then Cb and Cr at
    /// `(w+1)/2 × (h+1)/2`, each chroma sample from its 2×2 block's mean R'G'B'.
    fn planes(&self, rgba: &[u8], stride: usize, w: usize, h: usize) -> [Vec<u16>; 3] {
        // BT.2020 non-constant luminance.
        let (kr, kb) = (0.2627, 0.0593);
        let luma = |[r, g, b]: [f32; 3]| kr * r + (1.0 - kr - kb) * g + kb * b;
        // Codes are positive, so adding a half and truncating rounds; `round` is a libm call.
        let code = |v: f32, scale: f32, zero: f32| (zero + scale * v + 0.5) as u16;
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        // Chroma rows from `cy0` on, into their luma rows `y` and chroma rows `u` and `v`.
        let band = |cy0: usize, y: &mut [u16], u: &mut [u16], v: &mut [u16]| {
            for (cy, (u, v)) in (cy0..).zip(u.chunks_mut(cw).zip(v.chunks_mut(cw))) {
                for cx in 0..cw {
                    let mut sum = [0.0; 3];
                    for (x, y0) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let (x, row) = ((2 * cx + x).min(w - 1), (2 * cy + y0).min(h - 1));
                        let p = self.rgb(&rgba[row * stride + x * 4..]);
                        y[(row - 2 * cy0) * w + x] = code(luma(p), 876.0, 64.0);
                        sum = [0, 1, 2].map(|c| sum[c] + p[c] / 4.0);
                    }
                    let l = luma(sum);
                    u[cx] = code((sum[2] - l) / (2.0 * (1.0 - kb)), 896.0, 512.0);
                    v[cx] = code((sum[0] - l) / (2.0 * (1.0 - kr)), 896.0, 512.0);
                }
            }
        };
        let mut y = vec![0; w * h];
        let (mut u, mut v) = (vec![0; cw * ch], vec![0; cw * ch]);
        // One band per core: on one, 1080p takes ~30 ms a frame.
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let rows = ch.div_ceil(cores).max(1);
        let (band, chroma) = (&band, rows * cw);
        std::thread::scope(|s| {
            let bands = y
                .chunks_mut(2 * rows * w)
                .zip(u.chunks_mut(chroma).zip(v.chunks_mut(chroma)));
            for (i, (y, (u, v))) in bands.enumerate() {
                s.spawn(move || band(i * rows, y, u, v));
            }
        });
        [y, u, v]
    }
}

/// Copies 10-bit `planes` into `f`, a yuv420p10le or p010le frame of their size.
fn fill(f: &mut frame::Video, [y, u, v]: [Vec<u16>; 3]) {
    let h = f.height() as usize;
    // P010 keeps the 10 bits at the top of each sample and interleaves Cb and Cr.
    let planes = if f.format() == Pixel::P010LE {
        let uv = u
            .iter()
            .zip(&v)
            .flat_map(|(a, b)| [a << 6, b << 6])
            .collect();
        vec![y.iter().map(|s| s << 6).collect(), uv]
    } else {
        vec![y, u, v]
    };
    for (i, p) in planes.iter().enumerate() {
        let rows = if i == 0 { h } else { h.div_ceil(2) };
        let (stride, row) = (f.stride(i), p.len() / rows);
        let data = f.data_mut(i);
        for (r, line) in p.chunks_exact(row).enumerate() {
            let dst = data[r * stride..][..row * 2].as_chunks_mut::<2>().0;
            for (d, s) in dst.iter_mut().zip(line) {
                *d = s.to_le_bytes();
            }
        }
    }
}

/// The GIF filter graph: `src`-sized RGBA in at `tb`, a `size` PAL8 frame out per frame.
fn palette_graph(src: (u32, u32), size: (u32, u32), tb: Rational) -> Result<filter::Graph, Error> {
    let mut g = filter::Graph::new();
    let args = format!(
        "video_size={}x{}:pix_fmt=rgba:time_base={}/{}:pixel_aspect=1/1",
        src.0, src.1, tb.0, tb.1
    );
    g.add(
        &filter::find("buffer").ok_or(Error::FilterNotFound)?,
        "in",
        &args,
    )?;
    g.add(
        &filter::find("buffersink").ok_or(Error::FilterNotFound)?,
        "out",
        "",
    )?;
    // A palette from the whole clip plus error-diffusion dithering avoids banding in gradients;
    // rectangle diff mode only re-dithers what changed, which keeps static areas from crawling.
    let spec = format!(
        "scale={}:{}:flags=lanczos,split[a][b];[a]palettegen[p];\
         [b][p]paletteuse=dither=sierra2_4a:diff_mode=rectangle",
        size.0, size.1
    );
    g.output("in", 0)?.input("out", 0)?.parse(&spec)?;
    g.validate()?;
    Ok(g)
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
        Self::create_with(path, src, rate, preset, hardware(), open_video)
    }

    fn create_with(
        path: &Path,
        src: (u32, u32),
        rate: Rate,
        preset: &Preset,
        available: &[&str],
        mut open: impl FnMut(
            &str,
            Pixel,
            (u32, u32),
            Rate,
            bool,
            bool,
            &BTreeMap<String, String>,
        ) -> Option<(ffmpeg_next::Codec, encoder::Video)>,
    ) -> Result<Self, Error> {
        let mut out = format::output(path)?;
        let global = out.format().flags().contains(format::Flags::GLOBAL_HEADER);
        let rate = preset.frame_rate.unwrap_or(rate);
        let (width, height) = preset.output_size(src);
        let frame_tb = Rational(rate.den as i32, rate.num as i32);
        let video = match preset.video {
            None => None,
            Some(vc) => {
                let hdr = is_hdr10(vc, &preset.options);
                let (name, pixel, codec, enc) = candidates(vc, available, &preset.options)
                    .into_iter()
                    .find_map(|(n, pixel)| {
                        let (c, e) = open(
                            n,
                            pixel,
                            (width, height),
                            rate,
                            global,
                            hdr,
                            &preset.options,
                        )?;
                        Some((n, pixel, c, e))
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
                let scale =
                    |to| scaling::Context::get(Pixel::RGBA, src.0, src.1, to, width, height, flags);
                let convert = if vc == VideoCodec::Gif {
                    Convert::Palette(palette_graph(src, (width, height), frame_tb)?)
                } else if hdr {
                    let resize = (src != (width, height)).then(|| scale(Pixel::RGBA));
                    Convert::Pq(resize.transpose()?, Box::new(Hdr10::new()))
                } else {
                    Convert::Scale(scale(pixel)?)
                };
                Some(Video {
                    stream,
                    convert,
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
        let mut opts = Dictionary::new();
        if preset.video == Some(VideoCodec::Gif) {
            opts.set("loop", preset.options.get("loop").map_or("0", |l| l));
        }
        out.write_header_with(opts)?;
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
    pub fn video(&mut self, rgba: &[u8]) -> Result<(), EncodeError> {
        let Some(Video {
            stream,
            convert,
            name,
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
        src.set_pts(Some(self.frames));
        self.frames += 1;
        match convert {
            Convert::Scale(scaler) => {
                let mut yuv = frame::Video::empty();
                scaler.run(&src, &mut yuv)?;
                yuv.set_pts(src.pts());
                let vaapi = hwenc::is_vaapi(name);
                push_video(&mut self.out, stream, yuv, vaapi, hwenc::upload)
            }
            Convert::Palette(g) => {
                g.get("in").expect("added").source().add(&src)?;
                filtered(&mut self.out, stream, g)
            }
            Convert::Pq(resize, hdr10) => {
                let mut resized = frame::Video::empty();
                let rgba = match resize {
                    Some(scaler) => {
                        scaler.run(&src, &mut resized)?;
                        &resized
                    }
                    None => &src,
                };
                let (w, h) = (rgba.width() as usize, rgba.height() as usize);
                let planes = hdr10.planes(rgba.data(0), rgba.stride(0), w, h);
                let mut out = frame::Video::new(stream.enc.format(), w as u32, h as u32);
                fill(&mut out, planes);
                hwenc::hdr10_metadata(&mut out, MAX_CLL);
                out.set_pts(src.pts());
                push_video(&mut self.out, stream, out, false, hwenc::upload)
            }
        }
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
                // Packed is only ever PCM; plane_mut would only cover one channel.
                let wide = f.format() == Sample::I32(sample::Type::Packed);
                let data = f.data_mut(0);
                for (i, s) in samples.enumerate() {
                    let s = s.clamp(-1.0, 1.0);
                    if wide {
                        // pcm_s24le reads the top 24 bits of each s32.
                        let v = ((s as f64 * 8_388_607.0) as i32) << 8;
                        data[i * 4..][..4].copy_from_slice(&v.to_le_bytes());
                    } else {
                        let v = (s * i16::MAX as f32) as i16;
                        data[i * 2..][..2].copy_from_slice(&v.to_le_bytes());
                    }
                }
            }
            f.set_pts(Some(self.samples));
            self.samples += n as i64;
            a.enc.send_frame(&f)?;
            drain(&mut self.out, a, |e, p| e.receive_packet(p), false)?;
        }
        Ok(())
    }

    /// Flushes the encoders and writes the trailer.
    pub fn finish(mut self) -> Result<(), EncodeError> {
        self.flush_audio(true)?;
        if let Some(Video {
            stream: v,
            convert,
            name,
        }) = &mut self.video
        {
            if let Convert::Palette(g) = convert {
                g.get("in").expect("added").source().flush()?;
                filtered(&mut self.out, v, g)?;
            }
            let vaapi = hwenc::is_vaapi(name);
            v.enc.send_eof().map_err(|e| EncodeError::video(e, vaapi))?;
            drain(&mut self.out, v, |e, p| e.receive_packet(p), vaapi)?;
        }
        if let Some(a) = &mut self.audio {
            a.enc.send_eof()?;
            drain(&mut self.out, a, |e, p| e.receive_packet(p), false)?;
        }
        self.out.write_trailer().map_err(EncodeError::Other)
    }
}

fn push_video(
    out: &mut format::context::Output,
    stream: &mut Stream<encoder::Video>,
    mut frame: frame::Video,
    vaapi: bool,
    upload: impl FnOnce(&codec::Context, &frame::Video) -> Result<frame::Video, Error>,
) -> Result<(), EncodeError> {
    if vaapi {
        frame = upload(&stream.enc, &frame).map_err(EncodeError::Vaapi)?;
    }
    stream
        .enc
        .send_frame(&frame)
        .map_err(|e| EncodeError::video(e, vaapi))?;
    drain(out, stream, |e, p| e.receive_packet(p), vaapi)
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

/// Encodes every frame the GIF graph has ready.
fn filtered(
    out: &mut format::context::Output,
    s: &mut Stream<encoder::Video>,
    g: &mut filter::Graph,
) -> Result<(), EncodeError> {
    let mut f = frame::Video::empty();
    loop {
        match g.get("out").expect("added").sink().frame(&mut f) {
            Ok(()) => {}
            Err(Error::Eof) => return Ok(()),
            Err(Error::Other { errno }) if errno == ffmpeg_next::util::error::EAGAIN => {
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        }
        s.enc.send_frame(&f)?;
        drain(out, s, |e, p| e.receive_packet(p), false)?;
    }
}

/// Writes every packet the encoder has ready.
fn drain<E>(
    out: &mut format::context::Output,
    s: &mut Stream<E>,
    receive: impl Fn(&mut E, &mut Packet) -> Result<(), Error>,
    vaapi: bool,
) -> Result<(), EncodeError> {
    let mut p = Packet::empty();
    loop {
        match receive(&mut s.enc, &mut p) {
            Ok(()) => {}
            Err(Error::Eof) => return Ok(()),
            Err(Error::Other { errno }) if errno == ffmpeg_next::util::error::EAGAIN => {
                return Ok(());
            }
            Err(e) => return Err(EncodeError::video(e, vaapi)),
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

    fn small_preset() -> Preset {
        Preset {
            name: "injected failure".into(),
            extension: "mp4".into(),
            video: Some(VideoCodec::H264),
            audio: None,
            size: None,
            frame_rate: None,
            options: BTreeMap::new(),
        }
    }

    #[test]
    fn startup_upload_failure_selects_software_before_header() {
        crate::ffmpeg::init().unwrap();
        let path =
            std::env::temp_dir().join(format!("jvid-startup-upload-{}.mp4", std::process::id()));
        let mut probed = false;
        let mut enc = Encoder::create_with(
            &path,
            (64, 64),
            Rate::new(30, 1),
            &small_preset(),
            &["h264_vaapi"],
            |name, pixel, size, rate, global, hdr, options| {
                if !hwenc::is_vaapi(name) {
                    return open_video(name, pixel, size, rate, global, hdr, options);
                }
                // Use a real software codec context to inject the upload result on any host.
                let (codec, encoder) =
                    open_video("mpeg4", Pixel::YUV420P, size, rate, global, hdr, options)?;
                ready_video(name, encoder, |_| {
                    probed = true;
                    assert_eq!(
                        std::fs::metadata(&path).unwrap().len(),
                        0,
                        "header is not committed"
                    );
                    Err(Error::External)
                })
                .map(|encoder| (codec, encoder))
            },
        )
        .unwrap();
        assert!(probed);
        assert!(H264_CODECS.contains(&enc.video_encoder().unwrap()));
        for _ in 0..5 {
            enc.video(&[128; 64 * 64 * 4]).unwrap();
        }
        enc.finish().unwrap();
        let info = crate::probe::probe(&path).unwrap();
        assert_eq!(info.width, Some(64));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn in_progress_upload_and_receive_failures_keep_their_origin() {
        crate::ffmpeg::init().unwrap();
        let path =
            std::env::temp_dir().join(format!("jvid-progress-upload-{}.mp4", std::process::id()));
        let mut preset = small_preset();
        preset.options.insert("encoder".into(), "software".into());
        let mut enc = Encoder::create(&path, (64, 64), Rate::new(30, 1), &preset).unwrap();
        for _ in 0..3 {
            enc.video(&[128; 64 * 64 * 4]).unwrap();
        }
        let video = enc.video.as_mut().unwrap();
        let frame = frame::Video::new(video.stream.enc.format(), 64, 64);
        let result = push_video(&mut enc.out, &mut video.stream, frame, true, |_, _| {
            Err(Error::External)
        });
        assert_eq!(result, Err(EncodeError::Vaapi(Error::External)));
        assert_eq!(
            drain(
                &mut enc.out,
                &mut video.stream,
                |_, _| Err(Error::External),
                true
            ),
            Err(EncodeError::Vaapi(Error::External))
        );
        assert_eq!(
            drain(
                &mut enc.out,
                &mut video.stream,
                |_, _| Err(Error::External),
                false
            ),
            Err(EncodeError::Other(Error::External))
        );
        drop(enc);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn hardware_first_then_software() {
        let names = |c, hw: &[&str], o| -> Vec<_> {
            candidates(c, hw, &o).into_iter().map(|(n, _)| n).collect()
        };
        let hw = ["h264_qsv", "h264_nvenc", "hevc_qsv"];
        let got = names(VideoCodec::H264, &hw, opts(&[]));
        assert_eq!(got[..2], ["h264_nvenc", "h264_qsv"]);
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
        // HDR10 is HEVC only, 10-bit, and skips VAAPI's NV12 pool.
        let pq = opts(&[("hdr", "pq")]);
        assert_eq!(
            candidates(VideoCodec::Hevc, &["hevc_vaapi", "hevc_qsv"], &pq),
            [("hevc_qsv", Pixel::P010LE), ("libx265", Pixel::YUV420P10LE)]
        );
        assert_eq!(candidates(VideoCodec::H264, &hw, &pq)[0].1, Pixel::NV12);
    }

    #[test]
    fn sdr_maps_to_hdr10_at_reference_white() {
        let hdr10 = Hdr10::new();
        let code = |nits| 64 + (876.0 * nits_to_pq(nits)).round() as u16;
        // 2×2 blocks: white, black, 50% grey (BT.1886: 0.5^2.4 of 203 cd/m²), and BT.709 red.
        for (px, y, cb, cr) in [
            ([255, 255, 255], code(REFERENCE_WHITE), 512, 512),
            ([0, 0, 0], 64, 512, 512),
            (
                [128, 128, 128],
                code((128f32 / 255.0).powf(2.4) * 203.0),
                512,
                512,
            ),
        ] {
            let rgba: Vec<u8> = (0..4).flat_map(|_| [px[0], px[1], px[2], 255]).collect();
            let [ys, u, v] = hdr10.planes(&rgba, 8, 2, 2);
            for got in ys {
                assert!(got.abs_diff(y) <= 1, "{px:?}: Y {got}, want {y}");
            }
            assert_eq!((u[0], v[0]), (cb, cr), "{px:?}");
        }
        // Red: Cr up, Cb down, inside the 10-bit legal range; 3×1 pads the odd column.
        let [y, u, v] = hdr10.planes(&[255, 0, 0, 255].repeat(3), 12, 3, 1);
        assert_eq!((y.len(), u.len()), (3, 2));
        assert!(
            v[0] > 512 && v[0] <= 960 && u[0] < 512 && u[0] >= 64,
            "{u:?} {v:?}"
        );
        // Bands across cores: 37 rows of different greys land on their own rows.
        let grey = |r: usize| (r * 7) as u8;
        let rgba: Vec<u8> = (0..37 * 5).flat_map(|i| [grey(i / 5); 4]).collect();
        let [y, ..] = hdr10.planes(&rgba, 20, 5, 37);
        for r in 0..37 {
            let [one, ..] = hdr10.planes(&[grey(r); 4], 4, 1, 1);
            assert!(y[r * 5..][..5].iter().all(|c| *c == one[0]), "row {r}");
        }
    }

    /// The export check from #86: HDR10 comes out HEVC 10-bit PQ BT.2020 with mastering
    /// display and light level SEI. Skips without x265, unless `JVID_X265_TESTS=required`.
    #[test]
    fn hdr_export_probes_as_hevc_10bit_pq() {
        crate::ffmpeg::init().unwrap();
        let path = std::env::temp_dir().join(format!("jvid-hdr10-{}.mp4", std::process::id()));
        let preset = Preset {
            name: "HDR10".into(),
            video: Some(VideoCodec::Hevc),
            options: opts(&[("hdr", "pq"), ("encoder", "software")]),
            ..small_preset()
        };
        let mut enc = match Encoder::create(&path, (64, 64), Rate::new(30, 1), &preset) {
            Err(Error::EncoderNotFound) => {
                // CI sets this where its FFmpeg has x265, so it can't pass by skipping.
                assert!(
                    !std::env::var("JVID_X265_TESTS").is_ok_and(|v| v == "required"),
                    "JVID_X265_TESTS=required but this FFmpeg has no libx265"
                );
                eprintln!("skipping HDR export test: this FFmpeg has no libx265");
                return;
            }
            r => r.unwrap(),
        };
        assert_eq!(enc.video_encoder(), Some("libx265"));
        for _ in 0..5 {
            enc.video(&[255; 64 * 64 * 4]).unwrap();
        }
        enc.finish().unwrap();
        let info = crate::probe::probe(&path).unwrap();
        let Some(jvid_core::model::StreamInfo::Video(v)) = info.streams.first() else {
            panic!("no video: {info:?}");
        };
        assert_eq!(
            (v.codec.as_str(), v.bit_depth, v.hdr()),
            ("hevc", 10, Some(jvid_core::hdr::Hdr::Pq))
        );
        assert_eq!(v.color_primaries.as_deref(), Some("bt2020"));
        // The SEI comes back as side data on decoded frames; white sits at reference white.
        let mut input = format::input(&path).unwrap();
        let stream = input
            .streams()
            .best(ffmpeg_next::media::Type::Video)
            .unwrap();
        let ctx = codec::Context::from_parameters(stream.parameters()).unwrap();
        let mut dec = ctx.decoder().video().unwrap();
        let mut f = frame::Video::empty();
        for (_, p) in input.packets() {
            dec.send_packet(&p).unwrap();
            if dec.receive_frame(&mut f).is_ok() {
                break;
            }
        }
        drop(input);
        std::fs::remove_file(&path).unwrap();
        use ffmpeg_next::frame::side_data::Type;
        assert!(f.side_data(Type::MasteringDisplayMetadata).is_some());
        assert!(f.side_data(Type::ContentLightLevel).is_some());
        let y = u16::from_le_bytes([f.data(0)[0], f.data(0)[1]]);
        let want = 64 + (876.0 * nits_to_pq(REFERENCE_WHITE)).round() as u16;
        assert!(y.abs_diff(want) <= 4, "white Y {y}, want {want}");
    }

    /// Without a render node (CI) VAAPI fails at the probe, so exports fall back; with one, it
    /// opens with a frame pool.
    #[test]
    fn vaapi_opens_only_with_a_device() {
        crate::ffmpeg::init().unwrap();
        let rate = Rate::new(30, 1);
        for name in ["h264_vaapi", "hevc_vaapi"] {
            let opened = open_video(
                name,
                Pixel::NV12,
                (256, 256),
                rate,
                false,
                false,
                &opts(&[]),
            );
            assert_eq!(opened.is_some(), hardware().contains(&name), "{name}");
            if std::fs::read_dir("/dev/dri").is_err() {
                assert!(opened.is_none(), "{name} without a render node");
            }
        }
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
            get(&rc("h264_amf", Some("99")), "qp_i").as_deref(),
            Some("51")
        );
        assert_eq!(
            get(&rc("hevc_vaapi", Some("23")), "qp").as_deref(),
            Some("23")
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

    /// #380: an MP3 lasts the samples pushed in, not its whole 1152-sample frames with the
    /// encoder delay and padding (up to ~45 ms more).
    #[test]
    fn mp3_lasts_its_samples() {
        crate::ffmpeg::init().unwrap();
        let preset = jvid_core::preset::builtin()
            .into_iter()
            .find(|p| p.audio == Some(AudioCodec::Mp3))
            .unwrap();
        let path = std::env::temp_dir().join(format!("jvid-mp3-{}.mp3", std::process::id()));
        for n in [1_600, 4_800, 7_000, 96_007] {
            let mut enc = Encoder::create(&path, (64, 64), Rate::new(30, 1), &preset).unwrap();
            enc.audio(&vec![0.1; n * 2]).unwrap();
            enc.finish().unwrap();
            let got = crate::probe::probe(&path).unwrap().duration.0;
            let flicks = jvid_core::time::FLICKS_PER_SECOND;
            let want = n as i64 * flicks / SAMPLE_RATE as i64;
            assert!((got - want).abs() < flicks / 1000, "{n}: {got} vs {want}");
        }
        std::fs::remove_file(&path).unwrap();
    }

    /// The acceptance test from #60: 5 s of a grey ramp (plus a moving box so frames differ)
    /// through the GIF preset comes out 480p with no visible banding: averaged over 9 px, every
    /// pixel of the ramp is within 3 levels of the source, where the old 3-3-2 palette missed by ~11.
    #[test]
    fn gif_has_no_banding() {
        crate::ffmpeg::init().unwrap();
        let path = std::env::temp_dir().join(format!("jvid-gif-{}.gif", std::process::id()));
        let preset = jvid_core::preset::builtin()
            .into_iter()
            .find(|p| p.video == Some(VideoCodec::Gif))
            .unwrap();
        let (w, h) = (854usize, 480usize);
        let mut enc =
            Encoder::create(&path, (w as u32, h as u32), Rate::new(15, 1), &preset).unwrap();
        for i in 0..75 {
            let mut rgba = vec![255u8; w * h * 4];
            for (n, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let (x, y) = (n % w, n / w);
                let g = (x * 255 / (w - 1)) as u8;
                let boxed = y < 40 && (i * 10..i * 10 + 40).contains(&x);
                px[..3].fill(if boxed { 255 - g } else { g });
            }
            enc.video(&rgba).unwrap();
        }
        enc.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.windows(11).any(|b| b == b"NETSCAPE2.0"), "loops");
        let info = crate::probe::probe(&path).unwrap();
        assert_eq!((info.width, info.height), (Some(854), Some(480)));
        let mut dec = crate::decode::VideoDecoder::open(&path).unwrap();
        let f = dec.frame_at(jvid_core::time::Time(0)).unwrap();
        drop(dec); // Windows can't delete a file still open.
        std::fs::remove_file(&path).unwrap();
        let (lo, span) = if f.full_range {
            (0.0, 255.0)
        } else {
            (16.0, 219.0)
        };
        let row = &f.planes[0][300 * w..][..w];
        let worst = (4..w - 4)
            .map(|x| {
                let mean = row[x - 4..=x + 4].iter().map(|&v| v as f64).sum::<f64>() / 9.0;
                let want = lo + (x * 255 / (w - 1)) as f64 * span / 255.0;
                (mean - want).abs()
            })
            .fold(0.0, f64::max);
        assert!(worst < 3.0, "banding: off by {worst:.1}");
    }
}
