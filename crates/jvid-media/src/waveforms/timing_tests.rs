use super::*;
use crate::audio::SAMPLE_RATE;
use std::process::Command;

#[test]
fn timestamp_gaps_keep_silence_and_pulse_positions() {
    crate::ffmpeg::init().unwrap();
    for (name, filter, pulse, rate, codec) in [
        ("late", "asetpts=PTS+3/TB", 7.0, 48000, "pcm_f32le"),
        (
            "interrupted",
            "aselect=not(between(t\\,1\\,3))",
            4.0,
            48000,
            "pcm_f32le",
        ),
        (
            "interrupted44",
            "aselect=not(between(t\\,1\\,3))",
            4.0,
            44100,
            "pcm_f32le",
        ),
        (
            "interrupted-aac44",
            "aselect=not(between(t\\,1\\,3))",
            4.0,
            44100,
            "aac",
        ),
    ] {
        let root =
            std::env::temp_dir().join(format!("jvid-wave-timing-{name}-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join(if codec == "aac" {
            "audio.m4a"
        } else {
            "audio.mkv"
        });
        let program = if cfg!(windows) {
            "C:/ffmpeg/bin/ffmpeg.exe"
        } else {
            "ffmpeg"
        };
        let output = Command::new(program)
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                &format!(r"aevalsrc=if(between(t\,4\,4.1)\,0.8*sin(2*PI*1000*t)\,0):s={rate}:d=5"),
                "-af",
                filter,
                "-c:a",
                codec,
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut decoder = AudioDecoder::open(&path, 0).unwrap();
        let mut first_pulse = None;
        while let Some(chunk) = decoder.next_chunk().unwrap() {
            if let Some(i) = chunk
                .samples
                .as_chunks::<2>()
                .0
                .iter()
                .position(|s| s[0].abs() > 0.2)
            {
                first_pulse.get_or_insert((chunk.start + i as i64) as f64 / SAMPLE_RATE as f64);
            }
        }
        assert!(
            (first_pulse.unwrap() - pulse).abs() < if codec == "aac" { 0.012 } else { 0.002 },
            "{name}: {first_pulse:?}"
        );
        let cache = Cache::new(root.join("cache"), 1024 * 1024);
        for _ in 0..2 {
            let pyramid = cache.load(&path, 0).unwrap();
            assert!(
                pyramid
                    .finest()
                    .iter()
                    .all(|p| p[0] <= p[1] && p.iter().all(|v| v.is_finite() && v.abs() <= 1.0)),
                "{name}: invalid gap peaks"
            );
            let s = SAMPLE_RATE as i64;
            assert_eq!(
                pyramid.query(2 * s, 5 * s / 2, 20),
                vec![[0.0; 2]; 20],
                "{name}: gap is silent"
            );
            assert!(
                pyramid
                    .query(
                        ((pulse + 0.02) * s as f64) as i64,
                        ((pulse + 0.08) * s as f64) as i64,
                        10
                    )
                    .iter()
                    .all(|p| p[1] > 0.6)
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn contiguous_resampling_and_seek_still_match_full_decode() {
    crate::ffmpeg::init().unwrap();
    let root = std::env::temp_dir().join(format!("jvid-wave-contiguous-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    for (name, rate, codec) in [
        ("pcm48.wav", 48_000, "pcm_f32le"),
        ("pcm44.wav", 44_100, "pcm_f32le"),
        ("aac44.m4a", 44_100, "aac"),
        ("aac48.mkv", 48_000, "aac"),
    ] {
        let path = root.join(name);
        let program = if cfg!(windows) {
            "C:/ffmpeg/bin/ffmpeg.exe"
        } else {
            "ffmpeg"
        };
        let output = Command::new(program)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!("sine=frequency=1000:sample_rate={rate}:duration=5"))
            .args(["-c:a", codec])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut decoder = AudioDecoder::open(&path, 0).unwrap();
        let mut samples = Vec::new();
        let mut origin = None;
        while let Some(chunk) = decoder.next_chunk().unwrap() {
            let first = *origin.get_or_insert(chunk.start);
            assert_eq!(
                chunk.start,
                first + samples.len() as i64 / 2,
                "{name}: contiguous chunks"
            );
            samples.extend(chunk.samples);
        }
        assert!(
            (samples.len() as i64 / 2 - 5 * SAMPLE_RATE as i64).abs() < 2048,
            "{name}: complete tail"
        );
        // Coarse MKV timestamps lack sample-accurate seek guarantees; continuity above
        // ensures timestamp rounding does not invent a discontinuity at each AAC frame.
        if name.ends_with("mkv") {
            continue;
        }
        for target in [48_000, 123_457, 200_001] {
            decoder.seek(target).unwrap();
            let chunk = decoder.next_chunk().unwrap().unwrap();
            assert_eq!(chunk.start, target, "{name}: seek position");
            let at = (target - origin.unwrap()) as usize * 2;
            let max = chunk
                .samples
                .iter()
                .take(200)
                .zip(&samples[at..])
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(max < 0.002, "{name}: seek sample difference {max}");
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn old_timestamp_collapsing_cache_is_not_reused() {
    let root = std::env::temp_dir().join(format!("jvid-wave-old-cache-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("source.wav");
    fs::write(&source, b"test source").unwrap();
    fs::write(
        root.join(format!("{}.0.v1.peaks", key(&source).unwrap())),
        [f32::MAX, f32::MIN]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let cache = Cache::with_scan(root.clone(), 1024, Box::new(|_, _, _| Ok(vec![[0.0; 2]])));
    assert_eq!(cache.load(&source, 0).unwrap().finest(), &[[0.0; 2]]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extreme_timestamp_returns_error_without_allocating_the_gap() {
    crate::ffmpeg::init().unwrap();
    let root = std::env::temp_dir().join(format!("jvid-wave-limit-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("audio.mkv");
    let program = if cfg!(windows) {
        "C:/ffmpeg/bin/ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let output = Command::new(program)
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=48000:cl=mono:d=0.01",
            "-af",
            "asetpts=PTS+100000/TB",
            "-c:a",
            "pcm_f32le",
        ])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let error = Cache::new(root.join("cache"), 1024)
        .load(&path, 0)
        .unwrap_err();
    assert!(error.contains("peak memory limit"), "{error}");
    fs::remove_dir_all(root).unwrap();
}
