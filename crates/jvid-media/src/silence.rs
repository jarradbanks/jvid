//! Finds silent stretches in a waveform's finest peaks, for cutting dead air.

use crate::waveforms::{BUCKET, Peak};

/// Sample-frame ranges `[start, end)` where every peak stays under `threshold_db` dBFS for at
/// least `min_len` frames, each shrunk by `pad` frames at both ends so speech keeps a breath.
pub fn detect(peaks: &[Peak], threshold_db: f32, min_len: i64, pad: i64) -> Vec<(i64, i64)> {
    let limit = 10f32.powf(threshold_db / 20.0);
    let mut out = Vec::new();
    let mut run = None;
    // A loud sentinel closes a run that reaches the end.
    for (i, p) in peaks.iter().chain([&[1.0, 1.0]]).enumerate() {
        let quiet = p[0].abs().max(p[1].abs()) < limit;
        match (quiet, run) {
            (true, None) => run = Some(i as i64 * BUCKET),
            (false, Some(start)) => {
                run = None;
                let end = i as i64 * BUCKET;
                if end - start >= min_len && start + pad < end - pad {
                    out.push((start + pad, end - pad));
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::audio::SAMPLE_RATE;
    use crate::waveforms::Cache;

    const S: i64 = SAMPLE_RATE as i64;

    /// A 16-bit stereo WAV at [`SAMPLE_RATE`] of `samples`, both channels alike.
    fn wav(samples: &[f32]) -> Vec<u8> {
        let data = (samples.len() * 4) as u32;
        let mut b = Vec::new();
        b.extend(b"RIFF");
        b.extend((36 + data).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(SAMPLE_RATE.to_le_bytes());
        b.extend((SAMPLE_RATE * 4).to_le_bytes());
        b.extend(4u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(b"data");
        b.extend(data.to_le_bytes());
        for v in samples {
            let v = (v * i16::MAX as f32) as i16;
            b.extend(v.to_le_bytes());
            b.extend(v.to_le_bytes());
        }
        b
    }

    /// Acceptance: tone, two seconds of silence, tone gives one region within a frame of it.
    #[test]
    fn tone_silence_tone_finds_the_silence() {
        crate::ffmpeg::init().unwrap();
        let dir = std::env::temp_dir().join(format!("jvid-silence-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let tone = |n: i64| {
            (0..n).map(|i| 0.5 * (i as f32 * 440.0 / S as f32 * std::f32::consts::TAU).sin())
        };
        let samples: Vec<f32> = tone(S)
            .chain(std::iter::repeat_n(0.0, 2 * S as usize))
            .chain(tone(S))
            .collect();
        let path = dir.join("tone-silence-tone.wav");
        fs::write(&path, wav(&samples)).unwrap();
        let p = Cache::new(dir.join("cache"), u64::MAX)
            .load(&path, 0)
            .unwrap();

        let regions = detect(p.finest(), -40.0, S / 2, 0);
        let frame = S / 30;
        assert_eq!(regions.len(), 1, "{regions:?}");
        let (a, b) = regions[0];
        assert!(
            (a - S).abs() <= frame && (b - 3 * S).abs() <= frame,
            "{a}..{b}"
        );
        // Padding eats into both ends; a minimum longer than the gap finds nothing.
        assert_eq!(
            detect(p.finest(), -40.0, S / 2, S / 4),
            [(a + S / 4, b - S / 4)]
        );
        assert!(detect(p.finest(), -40.0, 3 * S, 0).is_empty());
        // A threshold above the tone's level hears everything as silence.
        assert_eq!(detect(p.finest(), 0.0, S / 2, 0).len(), 1);
    }
}
