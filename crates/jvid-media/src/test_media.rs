//! Locates the generated test media corpus and reads the frame index burned into its
//! `frame-index-1080p30.mp4` (feature `test-support`; dev-dependency use only).

use std::path::PathBuf;
use std::sync::OnceLock;

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
