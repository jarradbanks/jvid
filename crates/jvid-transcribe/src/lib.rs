//! Local, offline speech-to-text with word timings: a clip's audio goes through FFmpeg to 16 kHz
//! mono, quiet stretches are skipped, and whisper.cpp ([`Whisper`]) returns timed [`Word`]s.
//!
//! Models are downloaded on request into the data folder ([`models`]); none is bundled.

pub mod models;
pub mod store;
mod whisper;

use std::fmt;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use jvid_core::time::{Rate, Time};
use jvid_jobs::{Ctx, Handle, Pool, Priority};
use jvid_media::audio::{SAMPLE_RATE, SPEECH_RATE, decode_speech};
use jvid_media::silence;
use jvid_media::waveforms::BUCKET;

pub use whisper::Whisper;

/// A recognised word and when it was spoken.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Word {
    /// As spoken, with any punctuation whisper attached ("Hello," or "world.").
    pub text: String,
    pub start: Time,
    pub end: Time,
    /// Mean token probability in `0.0..=1.0`.
    pub confidence: f32,
}

/// What a transcription heard.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Transcript {
    /// ISO 639-1 code of the language spoken (the one asked for, or the one detected); `None` when
    /// the audio was all silence.
    pub language: Option<String>,
    pub words: Vec<Word>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Options {
    /// ISO 639-1 code such as "en"; `None` detects the language.
    pub language: Option<String>,
    /// Text whisper continues from, so its style carries over: a prompt like "Um, uh, so, like"
    /// keeps the filler words whisper otherwise drops.
    pub initial_prompt: Option<String>,
    /// Gives the words in English, whatever language is spoken.
    pub translate: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Cancelled,
    /// A download didn't match its SHA-256, so it was discarded.
    Corrupt,
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Cancelled => f.write_str("cancelled"),
            Error::Corrupt => f.write_str("download is corrupt"),
            Error::Failed(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for Error {}

/// Speech to words, so jobs and tests can swap whisper for a fake.
pub trait Transcriber: Send + Sync {
    /// Words in `audio` (mono at [`SPEECH_RATE`]), timed from its start, and its language. Reports progress to `ctx`
    /// and returns [`Error::Cancelled`] within a second of it being cancelled; [`Whisper`] frees
    /// the CPU later, at whisper.cpp's next compute-graph boundary.
    fn transcribe(&self, audio: Vec<f32>, opts: &Options, ctx: &Ctx) -> Result<Transcript, Error>;
}

/// What to transcribe: audio stream `nth` of `path` over project-rate sample frames `range`
/// (`0..i64::MAX` for all of it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub path: PathBuf,
    pub nth: usize,
    pub range: Range<i64>,
}

/// Transcribes `source` on `jobs` and hands the transcript to `done` there, its words, timed from the start of the
/// file. Cancel through the handle; `done` isn't called for a cancelled job.
pub fn submit(
    jobs: &Pool,
    engine: Arc<dyn Transcriber>,
    source: Source,
    opts: Options,
    done: impl FnOnce(Result<Transcript, Error>) + Send + 'static,
) -> Handle {
    // Same audio with other options or another model is another job: the pool hands a duplicate
    // the live job's handle and drops its `done`.
    let key = format!(
        "transcribe:{:?}:{}:{:?}:{:?}:{:p}",
        source.path,
        source.nth,
        source.range,
        opts,
        Arc::as_ptr(&engine) as *const ()
    );
    jobs.submit(key, Priority::Interactive, move |ctx| {
        match run(&*engine, &source, &opts, ctx) {
            Err(Error::Cancelled) => {}
            r => done(r),
        }
    })
}

fn run(
    engine: &dyn Transcriber,
    s: &Source,
    opts: &Options,
    ctx: &Ctx,
) -> Result<Transcript, Error> {
    let audio = decode_speech(&s.path, s.nth, s.range.clone(), &|| ctx.is_cancelled())
        .map_err(Error::Failed)?
        .ok_or(Error::Cancelled)?;
    let (speech, spans) = skip_silence(&audio);
    let voiced = voiced(&audio);
    // An hour of speech-rate audio is about 230 MB.
    drop(audio);
    let mut t = if speech.is_empty() {
        Transcript::default()
    } else {
        engine.transcribe(speech, opts, ctx)?
    };
    let offset = Rate::new(SAMPLE_RATE, 1).time_floor(s.range.start.max(0));
    for w in &mut t.words {
        (w.start, w.end) = to_source(w.start, w.end, &spans);
    }
    snap(&mut t.words, &voiced);
    for w in &mut t.words {
        (w.start, w.end) = (offset + w.start, offset + w.end);
    }
    ctx.set_progress(1.0);
    Ok(t)
}

/// Quiet below this many dBFS for [`MIN_SILENCE`] samples or more is cut, keeping [`PAD`] samples
/// each side so words aren't clipped. Whisper tends to invent text over long silence.
const SILENCE_DB: f32 = -45.0;
const MIN_SILENCE: i64 = 2 * SPEECH_RATE as i64;
const PAD: i64 = SPEECH_RATE as i64 / 4;
const _: () = assert!(
    PAD % BUCKET != 0,
    "skip_silence tells a leading cut by a == PAD"
);

/// Frames per second of [`voiced`].
const FRAMES: u32 = 100;
/// Kept before a word's first voiced frame and after its last: consonants start and trail off
/// under the threshold.
const LEAD: Time = Time(jvid_core::time::FLICKS_PER_SECOND * 3 / 100);
const TAIL: Time = Time(jvid_core::time::FLICKS_PER_SECOND * 5 / 100);

/// Whether each 10 ms of `audio` is speech: 15 dB over the noise floor (the quietest tenth of
/// it), but at least [`SILENCE_DB`].
// shortcut: the threshold is capped at -35 dBFS so loud backgrounds count as speech and leave
// whisper's edges alone; use a voice-activity model if music beds trim words.
fn voiced(audio: &[f32]) -> Vec<bool> {
    let db: Vec<f32> = (audio.chunks((SPEECH_RATE / FRAMES) as usize))
        .map(|c| {
            let ms = c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32;
            10.0 * ms.max(1e-12).log10()
        })
        .collect();
    let mut sorted = db.clone();
    sorted.sort_by(f32::total_cmp);
    let floor = sorted.get(sorted.len() / 10).copied().unwrap_or(SILENCE_DB);
    let threshold = (floor + 15.0).clamp(SILENCE_DB, -35.0);
    db.into_iter().map(|d| d > threshold).collect()
}

/// Pulls each word's edges in onto its speech, so a cut on them neither clips the word nor
/// keeps the pause beside it: whisper's ends run into the next pause and its starts can sit
/// early in the one before. A word with no voiced frame keeps its times.
fn snap(words: &mut [Word], voiced: &[bool]) {
    let rate = Rate::new(FRAMES, 1);
    for w in words {
        let frame = |t: Time| (rate.index_floor(t).max(0) as usize).min(voiced.len());
        let span = frame(w.start)..frame(w.end);
        let (Some(first), Some(last)) = (
            span.clone().find(|&i| voiced[i]),
            span.rev().find(|&i| voiced[i]),
        ) else {
            continue;
        };
        w.start = w.start.max(rate.time_floor(first as i64) - LEAD);
        w.end = w.end.min(rate.time_floor(last as i64 + 1) + TAIL);
    }
}

/// `audio` with long silences spliced out, and where each kept span starts: `(in the result, in
/// audio)`, both as [`Time`] from the start.
fn skip_silence(audio: &[f32]) -> (Vec<f32>, Vec<(Time, Time)>) {
    let peaks: Vec<_> = audio
        .chunks(BUCKET as usize)
        .map(|c| {
            c.iter()
                .fold([0f32; 2], |[lo, hi], &s| [lo.min(s), hi.max(s)])
        })
        .collect();
    let len = audio.len() as i64;
    // No pad at the ends of the audio: there's no speech beyond them to keep a breath for.
    let quiet = silence::detect(&peaks, SILENCE_DB, MIN_SILENCE, PAD)
        .into_iter()
        .map(|(a, b)| {
            (
                if a == PAD { 0 } else { a },
                if b + PAD >= len { len } else { b },
            )
        });
    let rate = Rate::new(SPEECH_RATE, 1);
    let (mut speech, mut spans, mut from) = (Vec::new(), Vec::new(), 0);
    for (a, b) in quiet.chain([(len, len)]) {
        let a = a.min(len) as usize;
        if a > from {
            spans.push((
                rate.time_floor(speech.len() as i64),
                rate.time_floor(from as i64),
            ));
            speech.extend_from_slice(&audio[from..a]);
        }
        from = b as usize;
    }
    (speech, spans)
}

/// Maps a word's times in spliced speech back to the audio it came from, through the kept span
/// holding its start. Whisper often stretches a word's end to the next word's start, which can be
/// past a cut; clamping it to the span keeps the removed silence out of the word.
fn to_source(start: Time, end: Time, spans: &[(Time, Time)]) -> (Time, Time) {
    let i = spans.partition_point(|s| s.0 <= start).saturating_sub(1);
    let Some(&(at, src)) = spans.get(i) else {
        return (start, end);
    };
    let len = spans.get(i + 1).map_or(Time(i64::MAX), |n| n.0 - at);
    (src + (start - at), src + (end - at).min(len))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    const S: usize = SPEECH_RATE as usize;

    fn secs(t: f64) -> Time {
        Time((t * jvid_core::time::FLICKS_PER_SECOND as f64) as i64)
    }

    fn tone(n: usize) -> impl Iterator<Item = f32> {
        (0..n).map(|i| 0.3 * (i as f32 * 0.2).sin())
    }

    #[test]
    fn long_silence_is_spliced_out_and_times_map_back() {
        // 1 s speech, 3 s silence, 1 s speech, 1 s silence (too short to cut).
        let audio: Vec<f32> = tone(S)
            .chain(std::iter::repeat_n(0.0, 3 * S))
            .chain(tone(S))
            .chain(std::iter::repeat_n(0.0, S))
            .collect();
        let (speech, spans) = skip_silence(&audio);
        // Silence detection works in 256-sample buckets, so cuts land within one of the pads.
        let cut = 3 * S - 2 * PAD as usize;
        assert!(
            speech.len().abs_diff(audio.len() - cut) < 256,
            "{}",
            speech.len()
        );
        assert_eq!(spans.len(), 2, "{spans:?}");
        assert_eq!(spans[0], (Time(0), Time(0)));
        // Before the cut maps to itself; after it, shifted by the cut.
        let at = |t| to_source(t, t, &spans).0;
        assert_eq!(at(secs(0.5)), secs(0.5));
        let shift = spans[1].1 - spans[1].0;
        assert!((shift.0 - secs(cut as f64 / S as f64).0).abs() < secs(0.02).0);
        assert_eq!(at(spans[1].0 + secs(0.1)), spans[1].1 + secs(0.1));
        // Exactly at the splice is the far side; a flick before it is the near side.
        assert_eq!(at(spans[1].0), spans[1].1);
        assert_eq!(at(spans[1].0 - Time(1)), spans[1].0 - Time(1));
    }

    #[test]
    fn a_word_ending_past_a_splice_ends_at_its_own_span() {
        // Spans: speech 0–1 s is source 0–1 s, speech from 1 s is source from 4 s.
        let spans = [(Time(0), Time(0)), (secs(1.0), secs(4.0))];
        let edge = secs(1.0);
        // Ends at or past the splice clamp to the end of the word's span, not across the cut.
        assert_eq!(to_source(secs(0.8), edge, &spans), (secs(0.8), edge));
        assert_eq!(to_source(secs(0.8), secs(1.3), &spans), (secs(0.8), edge));
        assert_eq!(
            to_source(secs(0.8), edge - Time(1), &spans),
            (secs(0.8), edge - Time(1))
        );
        // A word starting at the splice belongs to the far side, start and end.
        assert_eq!(to_source(edge, secs(1.3), &spans), (secs(4.0), secs(4.3)));
        // The last span has no end to clamp to.
        assert_eq!(
            to_source(secs(2.0), secs(9.0), &spans),
            (secs(5.0), secs(12.0))
        );
    }

    #[test]
    fn leading_silence_and_all_silence() {
        let audio: Vec<f32> = std::iter::repeat_n(0.0, 3 * S).chain(tone(S)).collect();
        let (speech, spans) = skip_silence(&audio);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].0, Time(0));
        assert_eq!(to_source(Time(0), Time(0), &spans).0, spans[0].1);
        assert!(speech.len() < 2 * S);
        assert!(skip_silence(&vec![0.0; 3 * S]).0.is_empty());
        // Too short to hold a silence, even the length of a pad.
        assert_eq!(skip_silence(&vec![0.0; PAD as usize]).0.len(), PAD as usize);
    }

    /// Acceptance for word timing: on tone bursts in quiet noise, sloppy word edges (a start in
    /// the pause before, an end running through the pause after) land on the bursts.
    #[test]
    fn word_edges_snap_to_speech_energy() {
        // 0.5 s quiet, 0.5 s "word", 0.7 s quiet, 0.4 s "word", 0.4 s quiet; noise at -60 dBFS.
        let noise = |n: usize| (0..n).map(|i| if i % 2 == 0 { 0.001 } else { -0.001 });
        let audio: Vec<f32> = noise(S / 2)
            .chain(tone(S / 2))
            .chain(noise(7 * S / 10))
            .chain(tone(2 * S / 5))
            .chain(noise(2 * S / 5))
            .collect();
        let word = |start, end| Word {
            text: String::new(),
            start: secs(start),
            end: secs(end),
            confidence: 1.0,
        };
        let mut words = [
            word(0.2, 1.4),
            word(1.4, 2.4),
            word(0.6, 0.9),
            word(2.2, 2.4),
        ];
        snap(&mut words, &voiced(&audio));
        let near = |t: Time, want: f64| (t.0 - secs(want).0).abs() <= secs(0.011).0;
        let edges = |w: &Word| (w.start, w.end);
        assert!(
            near(words[0].start, 0.47) && near(words[0].end, 1.05),
            "{:?}",
            edges(&words[0])
        );
        assert!(
            near(words[1].start, 1.67) && near(words[1].end, 2.15),
            "{:?}",
            edges(&words[1])
        );
        // Inside speech already: unchanged. All quiet: unchanged.
        assert_eq!(edges(&words[2]), (secs(0.6), secs(0.9)));
        assert_eq!(edges(&words[3]), (secs(2.2), secs(2.4)));
    }

    /// Hears one word per kept second, and with `slow` keeps running until cancelled.
    struct Fake {
        slow: bool,
    }

    impl Transcriber for Fake {
        fn transcribe(
            &self,
            audio: Vec<f32>,
            opts: &Options,
            ctx: &Ctx,
        ) -> Result<Transcript, Error> {
            if self.slow {
                while !ctx.is_cancelled() {
                    ctx.set_progress(0.5);
                    std::thread::sleep(Duration::from_millis(10));
                }
                return Err(Error::Cancelled);
            }
            let text = opts.initial_prompt.clone().unwrap_or_default();
            Ok(Transcript {
                language: Some(opts.language.clone().unwrap_or("en".into())),
                words: (0..audio.len() / S)
                    .map(|i| Word {
                        text: text.clone(),
                        start: secs(i as f64),
                        end: secs(i as f64 + 0.5),
                        confidence: 1.0,
                    })
                    .collect(),
            })
        }
    }

    fn wav(dir: &std::path::Path) -> PathBuf {
        // 2 s tone, 3 s silence, 2 s tone; stereo 16-bit at the project rate.
        let p = SAMPLE_RATE as usize;
        let samples: Vec<f32> = (0..2 * p)
            .map(|i| 0.3 * (i as f32 * 0.05).sin())
            .chain(std::iter::repeat_n(0.0, 3 * p))
            .chain((0..2 * p).map(|i| 0.3 * (i as f32 * 0.05).sin()))
            .collect();
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
        let path = dir.join("tone-gap-tone.wav");
        std::fs::write(&path, b).unwrap();
        path
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jvid-transcribe-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn job_returns_words_timed_from_the_file_start() {
        jvid_media::ffmpeg::init().unwrap();
        let path = wav(&tmp("job"));
        let jobs = Pool::new(1);
        let (tx, rx) = std::sync::mpsc::channel();
        let opts = Options {
            initial_prompt: Some("um".into()),
            ..Options::default()
        };
        // From 1 s in: 1 s of tone, the silence (cut), then 2 s of tone.
        let p = SAMPLE_RATE as i64;
        let source = Source {
            path,
            nth: 0,
            range: p..i64::MAX,
        };
        let h = submit(
            &jobs,
            Arc::new(Fake { slow: false }),
            source,
            opts,
            move |r| {
                tx.send(r).unwrap();
            },
        );
        let t = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        assert_eq!(h.progress(), 1.0);
        assert_eq!(t.language.as_deref(), Some("en"));
        let words = t.words;
        let starts: Vec<f64> = words
            .iter()
            .map(|w| w.start.0 as f64 / jvid_core::time::FLICKS_PER_SECOND as f64)
            .collect();
        assert!(
            words.iter().all(|w| w.text == "um"),
            "options reach the engine"
        );
        // Kept: 1–2.25 s, then ~4.75–7 s, about 3.5 s of speech, so three words.
        assert_eq!(starts.len(), 3, "{starts:?}");
        assert!((starts[0] - 1.0).abs() < 0.02, "{starts:?}");
        assert!((starts[1] - 2.0).abs() < 0.02, "{starts:?}");
        // The third second of speech starts 0.75 s past the cut's far edge.
        assert!((starts[2] - 5.5).abs() < 0.05, "{starts:?}");
    }

    #[test]
    fn same_audio_with_other_options_or_engine_is_its_own_job() {
        jvid_media::ffmpeg::init().unwrap();
        let path = wav(&tmp("dedupe"));
        let jobs = Pool::new(1);
        let (tx, rx) = std::sync::mpsc::channel();
        let source = Source {
            path,
            nth: 0,
            range: 0..i64::MAX,
        };
        let engine: Arc<dyn Transcriber> = Arc::new(Fake { slow: false });
        let runs = [
            (engine.clone(), "plain"),
            (engine, "um"),
            (Arc::new(Fake { slow: false }) as Arc<dyn Transcriber>, "um"),
        ];
        // Submitted back to back on one worker, so each is still live as the next arrives.
        for (i, (engine, prompt)) in runs.into_iter().enumerate() {
            let opts = Options {
                initial_prompt: Some(prompt.into()),
                ..Options::default()
            };
            let tx = tx.clone();
            submit(&jobs, engine, source.clone(), opts, move |r| {
                tx.send((i, r.unwrap().words[0].text.clone())).unwrap();
            });
        }
        let mut got: Vec<_> = (0..3)
            .map(|_| rx.recv_timeout(Duration::from_secs(10)).unwrap())
            .collect();
        got.sort();
        assert_eq!(
            got,
            [(0, "plain".into()), (1, "um".into()), (2, "um".into())]
        );
    }

    /// Acceptance: Cancel stops within a second.
    #[test]
    fn cancel_stops_within_a_second() {
        jvid_media::ffmpeg::init().unwrap();
        let path = wav(&tmp("cancel"));
        let jobs = Pool::new(1);
        let source = Source {
            path,
            nth: 0,
            range: 0..i64::MAX,
        };
        let h = submit(
            &jobs,
            Arc::new(Fake { slow: true }),
            source,
            Options::default(),
            |_| panic!("done is not called after cancel"),
        );
        let start = Instant::now();
        while h.progress() < 0.5 {
            assert!(start.elapsed() < Duration::from_secs(10), "never started");
            std::thread::sleep(Duration::from_millis(5));
        }
        let at = Instant::now();
        h.cancel();
        while !h.is_finished() {
            assert!(at.elapsed() < Duration::from_secs(1), "still running");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
