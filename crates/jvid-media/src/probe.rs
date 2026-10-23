//! Reads everything we need to know about a media file before using it.

use std::path::Path;

use ffmpeg_next::codec::{self, packet::side_data};
use ffmpeg_next::format::{self, Pixel, stream::Stream};
use ffmpeg_next::{Rational, media};
use jvid_core::model::{AudioInfo, ProbeInfo, StreamInfo, VideoInfo};
use jvid_core::time::{FLICKS_PER_SECOND, Rate, Time};

#[derive(Debug)]
pub enum ProbeError {
    /// FFmpeg couldn't open or parse it: not media, truncated, or an unknown container.
    Unreadable(ffmpeg_next::Error),
    /// Opened, but has no video or audio stream.
    NoMedia,
    /// A video or audio stream whose codec this FFmpeg can't decode.
    UnsupportedCodec(String),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(e) => write!(f, "not a readable media file: {e}"),
            Self::NoMedia => f.write_str("no video or audio streams"),
            Self::UnsupportedCodec(c) => write!(f, "unsupported codec: {c}"),
        }
    }
}

impl std::error::Error for ProbeError {}

/// Probes `path`. Call `ffmpeg::init` first.
pub fn probe(path: &Path) -> Result<ProbeInfo, ProbeError> {
    let input = format::input(path).map_err(ProbeError::Unreadable)?;
    let streams = input
        .streams()
        .map(stream_info)
        .collect::<Result<Vec<_>, _>>()?;
    let video = streams.iter().find_map(|s| match s {
        StreamInfo::Video(v) => Some(v),
        _ => None,
    });
    let audio: Vec<_> = streams
        .iter()
        .filter_map(|s| match s {
            StreamInfo::Audio(a) => Some(a),
            _ => None,
        })
        .collect();
    if video.is_none() && audio.is_empty() {
        return Err(ProbeError::NoMedia);
    }
    // Stills report no duration.
    let micros = input.duration().max(0);
    Ok(ProbeInfo {
        // 705.6 flicks per microsecond: dividing first truncated every duration by 0.085%.
        duration: Time((i128::from(micros) * i128::from(FLICKS_PER_SECOND) / 1_000_000) as i64),
        width: video.map(|v| v.width),
        height: video.map(|v| v.height),
        frame_rate: video.and_then(|v| {
            if v.vfr {
                v.avg_frame_rate
            } else {
                v.real_frame_rate.or(v.avg_frame_rate)
            }
        }),
        sample_rate: audio.first().map(|a| a.sample_rate),
        audio_streams: audio.len() as u32,
        container: input.format().name().to_owned(),
        streams,
        file_size: std::fs::metadata(path).ok().map(|m| m.len()),
    })
}

fn stream_info(stream: Stream) -> Result<StreamInfo, ProbeError> {
    let params = stream.parameters();
    let codec = params.id().name().to_owned();
    let kind = params.medium();
    if !matches!(kind, media::Type::Video | media::Type::Audio) {
        return Ok(StreamInfo::Other { codec });
    }
    // Opening a decoder is the real support check; it also exposes size, aspect and colour.
    let decoder = codec::Context::from_parameters(params)
        .map_err(|_| ProbeError::UnsupportedCodec(codec.clone()))?
        .decoder();
    if kind == media::Type::Audio {
        let a = decoder
            .audio()
            .map_err(|_| ProbeError::UnsupportedCodec(codec.clone()))?;
        return Ok(StreamInfo::Audio(AudioInfo {
            codec,
            sample_rate: a.rate(),
            channels: a.channels().into(),
            channel_layout: a.channel_layout().bits(),
            title: stream.metadata().get("title").map(str::to_owned),
        }));
    }
    let v = decoder
        .video()
        .map_err(|_| ProbeError::UnsupportedCodec(codec.clone()))?;
    let (width, height) = (v.width(), v.height());
    let sample_aspect = rate(v.aspect_ratio());
    let avg_frame_rate = rate(stream.avg_frame_rate());
    let real_frame_rate = rate(stream.rate());
    Ok(StreamInfo::Video(VideoInfo {
        codec,
        width,
        height,
        sample_aspect,
        display_aspect: rate(
            Rational::new(width as i32, height as i32)
                * sample_aspect.map_or(Rational::new(1, 1), |s| {
                    Rational::new(s.num as i32, s.den as i32)
                }),
        ),
        avg_frame_rate,
        real_frame_rate,
        vfr: is_vfr(avg_frame_rate, real_frame_rate),
        rotation: rotation(&stream),
        color_primaries: name(v.color_primaries().name()),
        color_transfer: name(v.color_transfer_characteristic().name()),
        color_range: name(v.color_range().name()),
        params: crate::remux::params_hash(&stream.parameters()),
        bit_depth: bit_depth(v.format()),
    }))
}

/// Bits per sample, e.g. 8 for nv12 and 10 for yuv420p10le or p010le; 0 when unknown.
fn bit_depth(p: Pixel) -> u32 {
    let Some(d) = p.descriptor() else {
        return 0;
    };
    // FFmpeg's bits per pixel counts luma and alpha once and each chroma sample once per `sub`
    // pixels: bpp = depth × (n - 2 + 2 / sub).
    let (n, sub) = (
        d.nb_components() as u32,
        1 << (d.log2_chroma_w() + d.log2_chroma_h()),
    );
    let samples = (n * sub).saturating_sub(2 * (sub - 1)).max(1);
    d.bits_per_pixel().max(0) as u32 * sub / samples
}

/// `None` for 0/x, x/0 or negative rationals. Reduces, so 2/4 becomes 1/2.
fn rate(r: Rational) -> Option<Rate> {
    let r = r.reduce();
    (r.numerator() > 0 && r.denominator() > 0)
        .then(|| Rate::new(r.numerator() as u32, r.denominator() as u32))
}

fn name(n: Option<&str>) -> Option<String> {
    n.filter(|n| !matches!(*n, "unknown" | "unspecified" | "reserved"))
        .map(str::to_owned)
}

/// More than 1% apart: rounding in MP4 durations makes CFR averages slightly off.
fn is_vfr(avg: Option<Rate>, real: Option<Rate>) -> bool {
    let (Some(a), Some(r)) = (avg, real) else {
        return false;
    };
    let (a, r) = (a.num as f64 / a.den as f64, r.num as f64 / r.den as f64);
    (a - r).abs() > r * 0.01
}

/// From the display matrix, falling back to the legacy `rotate` tag.
pub(crate) fn rotation(stream: &Stream) -> u32 {
    let degrees = stream
        .side_data()
        .find(|d| d.kind() == side_data::Type::DisplayMatrix && d.data().len() >= 36)
        .map(|d| {
            let m = |i: usize| i32::from_le_bytes(d.data()[i * 4..i * 4 + 4].try_into().unwrap());
            matrix_rotation(m(0), m(1))
        })
        .or_else(|| stream.metadata().get("rotate")?.parse::<f64>().ok())
        .unwrap_or(0.0);
    ((degrees / 90.0).round() as i64 * 90).rem_euclid(360) as u32
}

/// Clockwise degrees from the first row of a display matrix (FFmpeg's `av_display_rotation_get`,
/// negated, since that returns counter-clockwise).
fn matrix_rotation(a: i32, b: i32) -> f64 {
    (b as f64).atan2(a as f64).to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_and_vfr_helpers() {
        let one = 1 << 16;
        assert_eq!(matrix_rotation(0, one).round(), 90.0);
        assert_eq!(matrix_rotation(0, -one).round(), -90.0);
        assert_eq!(matrix_rotation(-one, 0).round(), 180.0);
        let r = |n, d| Some(Rate::new(n, d));
        assert!(!is_vfr(r(2997, 100), r(30000, 1001)));
        assert!(is_vfr(r(20, 1), r(30, 1)));
        assert!(!is_vfr(None, r(30, 1)));
        assert_eq!(rate(Rational::new(2, 4)), r(1, 2));
        assert_eq!(rate(Rational::new(0, 1)), None);
        for (p, bits) in [
            (Pixel::YUV420P, 8),
            (Pixel::NV12, 8),
            (Pixel::YUVJ422P, 8),
            (Pixel::RGBA, 8),
            (Pixel::GRAY8, 8),
            (Pixel::YUV420P10LE, 10),
            (Pixel::P010LE, 10),
            (Pixel::YUV422P10LE, 10),
            (Pixel::YUV444P12LE, 12),
            (Pixel::None, 0),
        ] {
            assert_eq!(bit_depth(p), bits, "{p:?}");
        }
    }

    fn video(info: &ProbeInfo) -> &VideoInfo {
        info.streams
            .iter()
            .find_map(|s| match s {
                StreamInfo::Video(v) => Some(v),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn probes_the_corpus() {
        let Some(dir) = crate::test_media::corpus() else {
            return;
        };
        crate::ffmpeg::init().unwrap();
        let none = None::<(u32, u32)>;
        // file, video codec, size, frame rate, audio streams, seconds
        #[rustfmt::skip]
        let cases = [
            ("frame-index-1080p30.mp4", Some("h264"), Some((1920, 1080)), Some((30, 1)), 0, 10.0),
            ("vfr-phone.mp4", Some("h264"), Some((1280, 720)), none, 1, 5.0),
            ("23.976fps.mp4", Some("h264"), Some((1920, 1080)), Some((24000, 1001)), 1, 5.0),
            ("59.94fps.mp4", Some("h264"), Some((1920, 1080)), Some((60000, 1001)), 1, 5.0),
            ("144fps.mp4", Some("h264"), Some((1920, 1080)), Some((144, 1)), 0, 3.0),
            ("4k-hevc.mp4", Some("hevc"), Some((3840, 2160)), Some((30, 1)), 0, 2.0),
            ("prores.mov", Some("prores"), Some((1920, 1080)), Some((25, 1)), 1, 2.0),
            ("9x16.mp4", Some("h264"), Some((1080, 1920)), Some((30, 1)), 1, 3.0),
            ("no-audio.mp4", Some("h264"), Some((1280, 720)), Some((30, 1)), 0, 3.0),
            ("rotated-phone.mp4", Some("h264"), Some((1920, 1080)), Some((30, 1)), 1, 3.0),
            ("tone-1khz.wav", None, None, None, 1, 5.0),
            ("tone-1khz.mp3", None, None, None, 1, 5.0),
            ("tone-1khz.m4a", None, None, None, 1, 5.0),
            ("still.png", Some("png"), Some((1920, 1080)), Some((25, 1)), 0, 0.0),
            ("still.jpg", Some("mjpeg"), Some((1920, 1080)), Some((25, 1)), 0, 0.0),
            ("still.webp", Some("webp"), Some((1920, 1080)), Some((25, 1)), 0, 0.0),
            ("three-audio.mkv", Some("h264"), Some((1280, 720)), Some((30, 1)), 3, 5.0),
            ("screen-1440p60.mp4", Some("h264"), Some((2560, 1440)), Some((60, 1)), 0, 3.0),
            ("long-2h-1440p60.mp4", Some("h264"), Some((2560, 1440)), Some((60, 1)), 1, 7200.0),
        ];
        for (file, codec, size, fps, audio, secs) in cases {
            let info = probe(&dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
            let got = info.duration.0 as f64 / FLICKS_PER_SECOND as f64;
            if file == "tone-1khz.wav" {
                assert_eq!(info.duration, Time(5 * FLICKS_PER_SECOND), "{file}");
            }
            // Concat drifts a few ms per segment, so the 2 h file gets 0.1% slack.
            assert!(
                (got - secs).abs() < f64::max(0.2, secs / 1000.0),
                "{file}: {got} s"
            );
            assert_eq!(info.width.zip(info.height), size, "{file}");
            assert_eq!(info.audio_streams, audio, "{file}");
            if file != "vfr-phone.mp4" {
                assert_eq!(info.frame_rate, fps.map(|(n, d)| Rate::new(n, d)), "{file}");
            }
            for s in &info.streams {
                match s {
                    StreamInfo::Video(v) => {
                        assert_eq!(Some(v.codec.as_str()), codec, "{file}");
                        assert_eq!(v.vfr, file == "vfr-phone.mp4", "{file}");
                        let bits = if file == "prores.mov" { 10 } else { 8 };
                        assert_eq!(v.bit_depth, bits, "{file}");
                        if file != "rotated-phone.mp4" {
                            assert_eq!(v.rotation, 0, "{file}");
                        }
                    }
                    StreamInfo::Audio(a) => {
                        assert_eq!((a.sample_rate, a.channels), (48_000, 1), "{file}");
                    }
                    StreamInfo::Other { .. } => panic!("{file}: unexpected stream"),
                }
            }
        }

        let vfr = probe(&dir.join("vfr-phone.mp4")).unwrap();
        let v = video(&vfr);
        // Gaps alternate 1/30 and 2/30 s: 20 fps on average, 30 fps timestamps.
        assert_eq!(v.real_frame_rate, Some(Rate::new(30, 1)));
        let avg = v.avg_frame_rate.unwrap();
        assert!(
            (avg.num as f64 / avg.den as f64 - 20.0).abs() < 0.5,
            "{avg:?}"
        );
        assert_eq!(vfr.frame_rate, Some(avg));

        // FFmpeg 6.1+ writes a 90 degree turn; 4.4's `rotate=90` tag reads back as 270.
        let rotated = probe(&dir.join("rotated-phone.mp4")).unwrap();
        assert_eq!(video(&rotated).rotation % 180, 90);

        let v = video(&probe(&dir.join("9x16.mp4")).unwrap()).clone();
        assert_eq!(v.sample_aspect, Some(Rate::new(1, 1)));
        assert_eq!(v.display_aspect, Some(Rate::new(9, 16)));
        assert_eq!(
            video(&probe(&dir.join("9x16.mp4")).unwrap())
                .color_range
                .as_deref(),
            Some("tv")
        );
        let titles: Vec<_> = probe(&dir.join("three-audio.mkv"))
            .unwrap()
            .streams
            .into_iter()
            .filter_map(|s| match s {
                StreamInfo::Audio(a) => a.title,
                _ => None,
            })
            .collect();
        assert_eq!(titles, ["Game", "Mic", "Voice chat"]);
    }

    /// The probe check from #86: a generated PQ clip probes as PQ BT.2020 10-bit.
    #[test]
    fn probes_a_pq_clip() {
        crate::ffmpeg::init().unwrap();
        let path = std::env::temp_dir().join(format!("jvid-probe-pq-{}.mkv", std::process::id()));
        crate::test_media::pq_clip(&path, &[26.0, 203.0]).unwrap();
        let info = probe(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let v = video(&info);
        assert_eq!(v.hdr(), Some(jvid_core::hdr::Hdr::Pq));
        assert_eq!(v.color_transfer.as_deref(), Some("smpte2084"));
        assert_eq!(v.color_primaries.as_deref(), Some("bt2020"));
        assert_eq!(v.bit_depth, 10);
    }

    #[test]
    fn bad_inputs_are_typed_errors() {
        crate::ffmpeg::init().unwrap();
        let dir = std::env::temp_dir().join(format!("jvid-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("notes.mp4");
        std::fs::write(&text, "not a video, just some text\n".repeat(100)).unwrap();
        assert!(matches!(probe(&text), Err(ProbeError::Unreadable(_))));
        assert!(matches!(
            probe(&dir.join("missing.mp4")),
            Err(ProbeError::Unreadable(_))
        ));

        let Some(corpus) = crate::test_media::corpus() else {
            return;
        };
        // No faststart, so the moov index is at the end and truncation loses it.
        let bytes = std::fs::read(corpus.join("no-audio.mp4")).unwrap();
        let cut = dir.join("cut.mp4");
        std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(probe(&cut), Err(ProbeError::Unreadable(_))));
    }
}
