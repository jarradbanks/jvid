//! Locates the generated test media corpus and reads the frame index burned into its
//! `frame-index-1080p30.mp4`, and writes PQ and HLG test clips (feature `test-support`; dev-dependency
//! use only).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ffmpeg_next::color::{Primaries, Range, Space, TransferCharacteristic};
use ffmpeg_next::format::{self, Pixel};
use ffmpeg_next::{Error, Packet, Rational, codec, encoder, frame};
use jvid_core::hdr::{Hdr, nits_to_pq};

/// Blocks across the top of each frame, one bit each, bit 0 leftmost. Matches scripts/gen-test-media.
const BITS: usize = 16;
const BLOCK_W: usize = 120;
const BLOCK_H: usize = 64;

/// Call first in every test that needs the corpus; `None` means skip it.
///
/// Looks in `$JVID_TEST_MEDIA` or `test-media/` at the repo root, written by scripts/gen-test-media.
/// Without it the tests skip, unless `JVID_TEST_MEDIA_REQUIRED=1`, which panics so CI can't pass
/// by skipping.
pub fn corpus() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::var_os("JVID_TEST_MEDIA").map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-media"),
            PathBuf::from,
        );
        if dir.join(".complete").exists() {
            return Some(dir);
        }
        assert!(
            !std::env::var("JVID_TEST_MEDIA_REQUIRED").is_ok_and(|v| v == "1"),
            "test media corpus missing at {}",
            dir.display()
        );
        eprintln!("skipping test media tests: run scripts/gen-test-media to generate the corpus.");
        None
    })
    .clone()
}

/// Reads the frame index from an 8-bit luma plane (`stride` bytes per row).
pub fn frame_index(luma: &[u8], stride: usize) -> u32 {
    (0..BITS)
        .filter(|bit| luma[BLOCK_H / 2 * stride + bit * BLOCK_W + BLOCK_W / 2] > 128)
        .map(|bit| 1 << bit)
        .sum()
}

/// The 10-bit limited-range code of the PQ signal for `nits`.
pub fn pq_code(nits: f32) -> u16 {
    64 + (876.0 * nits_to_pq(nits)).round() as u16
}

/// A neutral grey patch for [`hdr_clip`] at `nits`, PQ-encoded.
pub fn pq_grey(nits: f32) -> [f32; 3] {
    [nits_to_pq(nits); 3]
}

/// Writes a BT.2020 10-bit `hdr` clip to `path` (Matroska, lossless FFV1, which every FFmpeg
/// build has): three frames of 16×16 patches, left to right, each a non-linear R'G'B' signal
/// (0–1). Call `ffmpeg::init` first.
pub fn hdr_clip(path: &Path, hdr: Hdr, patches: &[[f32; 3]]) -> Result<(), Error> {
    let mut out = format::output(path)?;
    let c = encoder::find(codec::Id::FFV1).ok_or(Error::EncoderNotFound)?;
    let mut e = codec::Context::new_with_codec(c).encoder().video()?;
    let (w, h) = (16 * patches.len() as u32, 16);
    let tb = Rational(1, 30);
    e.set_width(w);
    e.set_height(h);
    e.set_format(Pixel::YUV420P10LE);
    e.set_time_base(tb);
    e.set_color_primaries(Primaries::BT2020);
    e.set_color_transfer_characteristic(match hdr {
        Hdr::Pq => TransferCharacteristic::SMPTE2084,
        Hdr::Hlg => TransferCharacteristic::ARIB_STD_B67,
    });
    e.set_colorspace(Space::BT2020NCL);
    e.set_color_range(Range::MPEG);
    if out.format().flags().contains(format::Flags::GLOBAL_HEADER) {
        e.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let mut enc = e.open_as(c)?;
    let mut s = out.add_stream(c)?;
    s.set_parameters(&enc);
    s.set_time_base(tb);
    out.write_header()?;
    // BT.2020 non-constant luminance Y'CbCr, limited range.
    let ycbcr = |[r, g, b]: [f32; 3]| {
        let y = 0.2627 * r + 0.6780 * g + 0.0593 * b;
        [
            64.0 + 876.0 * y,
            512.0 + 896.0 * (b - y) / 1.8814,
            512.0 + 896.0 * (r - y) / 1.4746,
        ]
        .map(|v: f32| v.round() as u16)
    };
    let mut f = frame::Video::new(Pixel::YUV420P10LE, w, h);
    for plane in 0..3 {
        let (stride, sub) = (f.stride(plane), if plane == 0 { 1 } else { 2 });
        for y in 0..(h / sub) as usize {
            for x in 0..(w / sub) as usize {
                let v = ycbcr(patches[x * sub as usize / 16])[plane];
                f.data_mut(plane)[y * stride + x * 2..][..2].copy_from_slice(&v.to_le_bytes());
            }
        }
    }
    for pts in 0..3 {
        f.set_pts(Some(pts));
        enc.send_frame(&f)?;
        write(&mut out, &mut enc, tb)?;
    }
    enc.send_eof()?;
    write(&mut out, &mut enc, tb)?;
    out.write_trailer()
}

/// Writes every packet `enc` has ready to stream 0.
fn write(
    out: &mut format::context::Output,
    enc: &mut encoder::Video,
    tb: Rational,
) -> Result<(), Error> {
    let mut p = Packet::empty();
    while enc.receive_packet(&mut p).is_ok() {
        p.set_stream(0);
        p.rescale_ts(tb, out.stream(0).expect("added").time_base());
        p.write_interleaved(out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffmpeg_next::{format, frame, media};

    #[test]
    fn frame_index_reads_bit_blocks() {
        let stride = 1920;
        for n in [0, 1, 0b1010_0101, 299, u16::MAX as u32] {
            let luma: Vec<u8> = (0..BLOCK_H * stride)
                .map(|i| {
                    if n >> ((i % stride) / BLOCK_W) & 1 == 1 {
                        235
                    } else {
                        16
                    }
                })
                .collect();
            assert_eq!(frame_index(&luma, stride), n);
        }
    }

    #[test]
    fn decoded_frames_carry_their_index() {
        let Some(dir) = corpus() else { return };
        ffmpeg_next::init().unwrap();
        let mut input = format::input(&dir.join("frame-index-1080p30.mp4")).unwrap();
        let stream = input.streams().best(media::Type::Video).unwrap();
        let index = stream.index();
        let ctx = ffmpeg_next::codec::Context::from_parameters(stream.parameters()).unwrap();
        let mut decoder = ctx.decoder().video().unwrap();
        let mut frame = frame::Video::empty();
        let mut n = 0;
        let mut check = |decoder: &mut ffmpeg_next::decoder::Video| {
            while decoder.receive_frame(&mut frame).is_ok() {
                assert_eq!(frame_index(frame.data(0), frame.stride(0)), n);
                n += 1;
            }
        };
        for (s, packet) in input.packets() {
            if s.index() == index {
                decoder.send_packet(&packet).unwrap();
                check(&mut decoder);
            }
        }
        decoder.send_eof().unwrap();
        check(&mut decoder);
        assert_eq!(n, 300);
    }
}
