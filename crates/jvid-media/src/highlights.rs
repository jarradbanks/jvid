//! Finds highlight candidates in a long recording: loud stretches in a waveform's finest peaks,
//! and scene changes in its video.

use std::path::Path;

use ffmpeg_next::Error;
use jvid_core::time::Time;

use crate::decode::VideoDecoder;
use crate::waveforms::{BUCKET, Peak};

/// Sample-frame ranges `[start, end)` where peaks pass `threshold_db` dBFS, with quiet gaps
/// shorter than `gap` frames bridged so one burst of shots reads as one range.
pub fn loud(peaks: &[Peak], threshold_db: f32, gap: i64) -> Vec<(i64, i64)> {
    let limit = 10f32.powf(threshold_db / 20.0);
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (i, p) in peaks.iter().enumerate() {
        if p[0].abs().max(p[1].abs()) < limit {
            continue;
        }
        let (a, b) = (i as i64 * BUCKET, (i as i64 + 1) * BUCKET);
        match out.last_mut() {
            Some(last) if a - last.1 < gap => last.1 = b,
            _ => out.push((a, b)),
        }
    }
    out
}

/// How different two luma histograms are, `0.0` (alike) to `1.0` (nothing shared).
fn distance(a: &[u32; 32], b: &[u32; 32]) -> f32 {
    let n: u32 = a.iter().sum();
    let d: u32 = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).sum();
    d as f32 / (2 * n.max(1)) as f32
}

/// A 32-bin histogram of every 8th luma sample.
fn histogram(luma: &[u8]) -> [u32; 32] {
    let mut h = [0; 32];
    for v in luma.iter().step_by(8) {
        h[(*v >> 3) as usize] += 1;
    }
    h
}

/// Frames `step` apart across source times `[from, to)`, each with how much the picture
/// changed since the one before (0–1), so a caller can try thresholds without rescanning.
/// `progress` gets the fraction done and returns false to stop, keeping what was found.
pub fn scenes(
    path: &Path,
    from: Time,
    to: Time,
    step: Time,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Vec<(Time, f32)>, Error> {
    let mut d = VideoDecoder::open(path)?;
    let mut out = Vec::new();
    let mut prev = None;
    let mut t = from;
    while t < to && progress((t - from).0 as f32 / (to - from).0 as f32) {
        let f = match d.frame_at(t) {
            Err(Error::Eof) => break,
            f => f?,
        };
        let h = histogram(&f.planes[0]);
        if let Some(p) = prev {
            out.push((f.pts, distance(&p, &h)));
        }
        prev = Some(h);
        t = t + step;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use ffmpeg_next::format::{self, Pixel};
    use ffmpeg_next::{Packet, Rational, codec, encoder, frame};
    use jvid_core::time::FLICKS_PER_SECOND as S;

    use super::*;

    #[test]
    fn loud_bridges_short_gaps_only() {
        let q = [0.0, 0.0];
        let l = [-0.9, 0.9];
        let peaks = [q, l, q, l, q, q, q, l];
        assert_eq!(
            loud(&peaks, -6.0, 2 * BUCKET),
            [(BUCKET, 4 * BUCKET), (7 * BUCKET, 8 * BUCKET)]
        );
        assert_eq!(loud(&peaks, -6.0, 0).len(), 3);
        assert!(loud(&peaks, 0.0, 0).is_empty());
    }

    /// Ten dark frames at 10 fps, then ten bright ones: one change, at 1 s.
    #[test]
    fn scenes_finds_the_cut() {
        crate::ffmpeg::init().unwrap();
        let dir = std::env::temp_dir().join(format!("jvid-scenes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cut.mkv");
        let mut out = format::output(&path).unwrap();
        let c = encoder::find(codec::Id::FFV1).unwrap();
        let mut e = codec::Context::new_with_codec(c).encoder().video().unwrap();
        let tb = Rational(1, 10);
        e.set_width(32);
        e.set_height(32);
        e.set_format(Pixel::YUV420P);
        e.set_time_base(tb);
        let mut enc = e.open_as(c).unwrap();
        out.add_stream(c).unwrap().set_parameters(&enc);
        out.stream_mut(0).unwrap().set_time_base(tb);
        out.write_header().unwrap();
        let mut f = frame::Video::new(Pixel::YUV420P, 32, 32);
        let write = |enc: &mut encoder::Video, out: &mut format::context::Output| {
            let mut p = Packet::empty();
            while enc.receive_packet(&mut p).is_ok() {
                p.set_stream(0);
                p.rescale_ts(tb, out.stream(0).unwrap().time_base());
                p.write_interleaved(out).unwrap();
            }
        };
        for i in 0..20 {
            f.data_mut(0).fill(if i < 10 { 30 } else { 220 });
            f.data_mut(1).fill(128);
            f.data_mut(2).fill(128);
            f.set_pts(Some(i));
            enc.send_frame(&f).unwrap();
            write(&mut enc, &mut out);
        }
        enc.send_eof().unwrap();
        write(&mut enc, &mut out);
        out.write_trailer().unwrap();

        let step = Time(S / 10);
        let found = scenes(&path, Time(0), Time(2 * S), step, &mut |_| true).unwrap();
        assert_eq!(found.len(), 19, "{found:?}");
        let cuts: Vec<_> = found
            .iter()
            .filter(|(_, d)| *d > 0.5)
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(cuts, [Time(S)]);
        // Stopping at once finds nothing.
        let none = scenes(&path, Time(0), Time(2 * S), step, &mut |_| false).unwrap();
        assert!(none.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
