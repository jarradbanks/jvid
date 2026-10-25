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

/// Keyframes closer than this (well under the dialog's least threshold, 0.2) skip the frames
/// between them.
const QUIET: f32 = 0.1;

/// Frames `step` apart across source times `[from, to)`, each with how much the picture
/// changed since the one before (0–1), so a caller can try thresholds without rescanning.
/// `progress` gets the fraction done and returns false to stop, keeping what was found.
///
/// Decodes keyframes first and only the frames between two that differ; a stretch that
/// changes and changes back between keyframes reads as unchanged.
pub fn scenes(
    path: &Path,
    from: Time,
    to: Time,
    step: Time,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Vec<(Time, f32)>, Error> {
    let mut keys = VideoDecoder::open(path)?;
    keys.keyframes_only();
    let mut full = VideoDecoder::open(path)?;
    let mut out = Vec::new();
    let f = match keys.frame_at(from) {
        Err(Error::Eof) => return Ok(out),
        f => f?,
    };
    let (mut t, mut prev) = (f.pts, histogram(&f.planes[0]));
    while t < to && progress((t - from).0 as f32 / (to - from).0 as f32) {
        // The first keyframe at least `step` on; none past the last, so scan the tail in full.
        let k = match keys.frame_after(t + step - Time(1)) {
            Err(Error::Eof) => None,
            k => Some(k?),
        };
        let h = k.as_ref().map(|k| histogram(&k.planes[0]));
        if h.is_none_or(|h| distance(&prev, &h) > QUIET) {
            let (mut s, end) = (t + step, k.as_ref().map_or(to, |k| k.pts.min(to)));
            while s < end {
                let f = full.frame_at(s)?;
                // Past the end of the file, frame_at repeats the last frame.
                if out.last().is_some_and(|l: &(Time, f32)| f.pts <= l.0) {
                    break;
                }
                let fh = histogram(&f.planes[0]);
                out.push((f.pts, distance(&prev, &fh)));
                prev = fh;
                s = s + step;
            }
        }
        let (Some(k), Some(h)) = (k, h) else { break };
        if k.pts < to {
            out.push((k.pts, distance(&prev, &h)));
        }
        (t, prev) = (k.pts, h);
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

    /// `n` frames at 10 fps, dark before frame `cut` and bright from it, a keyframe every `gop`.
    fn cut_clip(path: &Path, codec: codec::Id, n: i64, cut: i64, gop: u32) {
        let mut out = format::output(path).unwrap();
        let c = encoder::find(codec).unwrap();
        let mut e = codec::Context::new_with_codec(c).encoder().video().unwrap();
        let tb = Rational(1, 10);
        e.set_width(32);
        e.set_height(32);
        e.set_format(Pixel::YUV420P);
        e.set_time_base(tb);
        e.set_gop(gop);
        e.set_max_b_frames(0);
        // No extra keyframe at the cut.
        let opts = ffmpeg_next::Dictionary::from_iter([("sc_threshold", "1000000000")]);
        let mut enc = e.open_as_with(c, opts).unwrap();
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
        for i in 0..n {
            f.data_mut(0).fill(if i < cut { 30 } else { 220 });
            f.data_mut(1).fill(128);
            f.data_mut(2).fill(128);
            f.set_pts(Some(i));
            enc.send_frame(&f).unwrap();
            write(&mut enc, &mut out);
        }
        enc.send_eof().unwrap();
        write(&mut enc, &mut out);
        out.write_trailer().unwrap();
    }

    fn cuts(found: &[(Time, f32)]) -> Vec<Time> {
        found
            .iter()
            .filter(|(_, d)| *d > 0.5)
            .map(|(t, _)| *t)
            .collect()
    }

    /// Ten dark frames at 10 fps, then ten bright ones: one change, at 1 s.
    #[test]
    fn scenes_finds_the_cut() {
        crate::ffmpeg::init().unwrap();
        let dir = std::env::temp_dir().join(format!("jvid-scenes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cut.mkv");
        cut_clip(&path, codec::Id::FFV1, 20, 10, 1);

        let step = Time(S / 10);
        let found = scenes(&path, Time(0), Time(2 * S), step, &mut |_| true).unwrap();
        assert_eq!(found.len(), 19, "{found:?}");
        assert_eq!(cuts(&found), [Time(S)]);
        // Stopping at once finds nothing.
        let none = scenes(&path, Time(0), Time(2 * S), step, &mut |_| false).unwrap();
        assert!(none.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Keyframes each second and a cut between two of them, at 1.5 s: the quiet first second
    /// is one sample, the second with the cut is scanned frame by frame, and so is the tail
    /// after the last keyframe.
    #[test]
    fn scenes_scans_between_keyframes_only_where_they_differ() {
        crate::ffmpeg::init().unwrap();
        let dir = std::env::temp_dir().join(format!("jvid-scenes-gop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cut.mkv");
        cut_clip(&path, codec::Id::MPEG4, 30, 15, 10);

        let found = scenes(&path, Time(0), Time(3 * S), Time(S / 10), &mut |_| true).unwrap();
        let times: Vec<_> = found.iter().map(|(t, _)| t.0 * 10 / S).collect();
        let mut want = vec![10];
        want.extend(11..30);
        assert_eq!(times, want, "{found:?}");
        assert_eq!(cuts(&found), [Time(3 * S / 2)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Times a whole-file scan of the 2-hour corpus recording, as Find Highlights runs it.
    #[test]
    #[ignore = "takes minutes; run with --ignored --nocapture to time the scan"]
    fn scenes_long_recording_timing() {
        let Some(dir) = crate::test_media::corpus() else {
            return;
        };
        crate::ffmpeg::init().unwrap();
        let start = std::time::Instant::now();
        let found = scenes(
            &dir.join("long-2h-1440p60.mp4"),
            Time(0),
            Time(7200 * S),
            Time(S / 5),
            &mut |_| true,
        )
        .unwrap();
        eprintln!("{} samples in {:.1?}", found.len(), start.elapsed());
    }
}
