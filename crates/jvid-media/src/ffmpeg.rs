//! FFmpeg startup: initialise the dynamically linked libraries and log what they can do.
//! The supported version range and licensing split are in docs/adr/0001-ffmpeg-binding.md.

use ffmpeg_next::codec::{self, Id};
use ffmpeg_next::{decoder, encoder};

/// Oldest supported libavcodec, packed as `major << 16 | minor << 8 | micro` (FFmpeg 4.4).
pub const MIN_AVCODEC: u32 = 58 << 16 | 134 << 8;
/// True when built with the `gpl-ffmpeg` feature, i.e. linking a GPL FFmpeg is intended.
pub const GPL_BUILD: bool = cfg!(feature = "gpl-ffmpeg");

const DECODERS: [(Id, &str); 6] = [
    (Id::H264, "h264"),
    (Id::HEVC, "hevc"),
    (Id::PRORES, "prores"),
    (Id::AAC, "aac"),
    (Id::MP3, "mp3"),
    (Id::OPUS, "opus"),
];
const ENCODERS: [&str; 8] = [
    "libx264",
    "libx265",
    "libopenh264",
    "h264_nvenc",
    "h264_qsv",
    "h264_amf",
    "h264_videotoolbox",
    "aac",
];

/// What the linked FFmpeg reports about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub avcodec: u32,
    pub license: &'static str,
    pub decoders: Vec<&'static str>,
    pub encoders: Vec<&'static str>,
}

/// Initialises FFmpeg and logs its version, licence and key codecs. Call once at startup,
/// after `jvid_log::init`.
pub fn init() -> Result<Info, ffmpeg_next::Error> {
    ffmpeg_next::init()?;
    let info = Info {
        avcodec: codec::version(),
        license: codec::license(),
        decoders: DECODERS
            .iter()
            .filter(|(id, _)| decoder::find(*id).is_some())
            .map(|(_, name)| *name)
            .collect(),
        encoders: ENCODERS
            .into_iter()
            .filter(|name| encoder::find_by_name(name).is_some())
            .collect(),
    };
    tracing::info!(
        avcodec = %version_string(info.avcodec),
        license = info.license,
        decoders = ?info.decoders,
        encoders = ?info.encoders,
        "ffmpeg loaded"
    );
    tracing::info!(hardware = ?crate::encode::hardware(), "hardware encoders");
    for problem in problems(info.avcodec, info.license, GPL_BUILD) {
        tracing::warn!("{problem}");
    }
    Ok(info)
}

fn version_string(v: u32) -> String {
    format!("{}.{}.{}", v >> 16, (v >> 8) & 0xff, v & 0xff)
}

/// Warnings about the linked FFmpeg: too old, or GPL in a build meant to stay LGPL.
pub fn problems(avcodec: u32, license: &str, gpl_build: bool) -> Vec<String> {
    let mut out = Vec::new();
    if avcodec < MIN_AVCODEC {
        out.push(format!(
            "libavcodec {} is older than the supported minimum {} (FFmpeg 4.4)",
            version_string(avcodec),
            version_string(MIN_AVCODEC)
        ));
    }
    let gpl = license.contains("GPL") && !license.contains("LGPL");
    if gpl && !gpl_build {
        out.push(format!(
            "linked FFmpeg is {license}; this build expects LGPL (enable `gpl-ffmpeg` for GPL)"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffmpeg_next::{format, frame, media};

    const NEW: u32 = 61 << 16 | 19 << 8;

    #[test]
    fn flags_old_ffmpeg_and_unexpected_gpl() {
        assert!(problems(NEW, "LGPL version 2.1 or later", false).is_empty());
        assert!(problems(NEW, "GPL version 2 or later", true).is_empty());
        assert_eq!(problems(NEW, "GPL version 2 or later", false).len(), 1);
        assert_eq!(
            problems(MIN_AVCODEC, "LGPL version 2.1 or later", false).len(),
            0
        );
        assert_eq!(
            problems(MIN_AVCODEC - 1, "LGPL version 2.1 or later", false).len(),
            1
        );
        assert_eq!(version_string(MIN_AVCODEC), "58.134.0");
    }

    #[test]
    fn linked_ffmpeg_is_supported() {
        let info = init().unwrap();
        assert!(
            info.avcodec >= MIN_AVCODEC,
            "{}",
            version_string(info.avcodec)
        );
    }

    /// Opens and decodes a 2x2, 3-frame Y4M clip written on the fly, so no fixture is needed.
    #[test]
    fn opens_and_decodes_a_file() {
        init().unwrap();
        let mut y4m = b"YUV4MPEG2 W2 H2 F25:1 Ip A1:1 C420jpeg\n".to_vec();
        for i in 0..3u8 {
            y4m.extend_from_slice(b"FRAME\n");
            y4m.extend_from_slice(&[i * 80; 6]); // 4 luma + 1 Cb + 1 Cr
        }
        let dir = std::env::temp_dir().join(format!("jvid-ffmpeg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tiny.y4m");
        std::fs::write(&path, y4m).unwrap();

        let mut input = format::input(&path).unwrap();
        let stream = input.streams().best(media::Type::Video).unwrap();
        let index = stream.index();
        let mut dec = codec::context::Context::from_parameters(stream.parameters())
            .unwrap()
            .decoder()
            .video()
            .unwrap();
        let mut frame = frame::Video::empty();
        let mut lumas = Vec::new();
        let mut drain = |dec: &mut decoder::Video| {
            while dec.receive_frame(&mut frame).is_ok() {
                assert_eq!((frame.width(), frame.height()), (2, 2));
                lumas.push(frame.data(0)[0]);
            }
        };
        for (s, packet) in input.packets() {
            if s.index() == index {
                dec.send_packet(&packet).unwrap();
                drain(&mut dec);
            }
        }
        dec.send_eof().unwrap();
        drain(&mut dec);
        assert_eq!(lumas, [0, 80, 160]);
    }
}
