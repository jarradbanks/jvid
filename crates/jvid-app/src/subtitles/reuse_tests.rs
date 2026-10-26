//! End-to-end recognition reuse with real decoding and a counted fake speech engine: no
//! models, downloads, or GPU. Exercise both dialog modes and the actual Transcript placement.

use super::*;
use crate::transcript::{Cache, TranscriptPanel, fillers};
use jvid_core::history::{Created, History};
use jvid_core::model::{NewClip, ProbeInfo};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct Fake(Arc<AtomicUsize>);

impl Transcriber for Fake {
    fn transcribe(
        &self,
        audio: Vec<f32>,
        opts: &Options,
        _: &jvid_jobs::Ctx,
    ) -> Result<Transcript, Error> {
        self.0.fetch_add(1, Ordering::SeqCst);
        assert!(
            opts.initial_prompt.is_some(),
            "both spoken-word workflows preserve fillers"
        );
        Ok(Transcript {
            language: Some(opts.language.clone().unwrap_or_else(|| "en".into())),
            words: (0..audio.len() / jvid_media::audio::SPEECH_RATE as usize)
                .map(|i| Word {
                    text: "um".into(),
                    start: Time(i as i64 * S),
                    end: Time(i as i64 * S + S / 2),
                    confidence: 0.9,
                })
                .collect(),
        })
    }
}

fn fixture() -> (
    tempfile::TempDir,
    History,
    ClipId,
    Subtitles,
    TranscriptPanel,
    Arc<AtomicUsize>,
) {
    jvid_media::ffmpeg::init().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("voice.wav");
    let samples = 8 * SAMPLE_RATE;
    let data = samples * 2;
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend((36 + data).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(SAMPLE_RATE.to_le_bytes());
    wav.extend((SAMPLE_RATE * 2).to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(data.to_le_bytes());
    for i in 0..samples {
        wav.extend((((i as f32 * 0.05).sin() * 8000.0) as i16).to_le_bytes());
    }
    std::fs::write(&path, wav).unwrap();
    let mut h = History::new(Project::default());
    h.apply(Command::Import(vec![(
        path,
        ProbeInfo {
            duration: Time(8 * S),
            audio_streams: 1,
            ..Default::default()
        },
    )]))
    .unwrap();
    let Created::Track(track) = h
        .apply(Command::AddTrack {
            kind: TrackKind::Audio,
            name: String::new(),
        })
        .unwrap()
    else {
        panic!()
    };
    let Created::Clip(clip) = h
        .apply(Command::AddClip {
            track,
            clip: NewClip {
                asset: h.project().assets()[0].id,
                source_in: Time(2 * S),
                source_out: Time(4 * S),
                start: Time(S),
                link: None,
            },
        })
        .unwrap()
    else {
        panic!()
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let cache = Cache::new(dir.path().join("cache"));
    let subtitles = Subtitles {
        store: cache.store.clone(),
        engine: Some((Model::Base, Arc::new(Fake(calls.clone())))),
        ..Default::default()
    };
    let mut panel = TranscriptPanel::default();
    panel.cache = cache;
    (dir, h, clip, subtitles, panel, calls)
}

fn finish(s: &mut Subtitles) -> Done {
    let start = Instant::now();
    loop {
        if let Some(done) = s.poll() {
            return done;
        }
        assert!(s.error.is_none(), "{:?}", s.error);
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "recognition never finished"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn complete_panel(panel: &mut TranscriptPanel, project: &Project, selected: &HashSet<ClipId>) {
    let start = Instant::now();
    while !panel.cache.placed(project, selected).1.is_empty() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "cached view never loaded"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn subtitles_then_transcript_reuses_covered_span_and_reopen_preserves_words() {
    let (dir, mut h, clip, mut s, mut panel, calls) = fixture();
    let selected = HashSet::from([clip]);
    s.open();
    s.start(h.project(), &selected);
    let Done::Edit(edit) = finish(&mut s) else {
        panic!()
    };
    h.apply(*edit).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    complete_panel(&mut panel, h.project(), &selected);
    let placed = panel.cache.placed(h.project(), &selected);
    assert_eq!(placed.0.len(), 2);
    assert_eq!(placed.0[0].1.start, Time(S));
    assert_eq!(fillers::find(&placed.0, &placed.2).len(), 2);
    let caption = h
        .project()
        .assets()
        .iter()
        .find(|a| a.text.is_some())
        .unwrap();
    let mut text = caption.text.clone().unwrap();
    text.content = "Edited caption".into();
    let id = caption.id;
    h.apply(Command::SetText { id, text }).unwrap();
    assert_eq!(panel.cache.placed(h.project(), &selected).0[0].1.text, "um");
    let source = targets(h.project(), &selected, false)[0].source.clone();
    s.open_transcribe(vec![source]);
    s.start(h.project(), &selected);
    let Done::Transcribed(done) = finish(&mut s) else {
        panic!()
    };
    assert_eq!(done[0].1.words[0].start, Time(2 * S));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(s.feedback.take().is_some());
    panel.cache = Cache::new(dir.path().join("cache"));
    s.store = panel.cache.store.clone();
    s.engine = None;
    complete_panel(&mut panel, h.project(), &selected);
    s.open();
    s.start(h.project(), &selected);
    assert!(matches!(finish(&mut s), Done::Edit(_)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn transcript_then_subtitles_reuses_words_after_moves_trims_speed_and_repeated_clips() {
    let (_dir, mut h, clip, mut s, mut panel, calls) = fixture();
    let selected = HashSet::from([clip]);
    s.open_transcribe(panel.missing(h.project(), &selected));
    s.start(h.project(), &selected);
    assert!(matches!(finish(&mut s), Done::Transcribed(_)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let track = h.project().sequence().clip(clip).unwrap().0;
    h.apply(Command::MoveClip {
        id: clip,
        track,
        start: Time(4 * S),
    })
    .unwrap();
    h.apply(Command::SetClipSpeed {
        id: clip,
        speed: 2.0,
        reverse: false,
        ripple: false,
    })
    .unwrap();
    let placed = panel.cache.placed(h.project(), &selected);
    assert_eq!(placed.0[0].1.start, Time(4 * S));
    assert_eq!(placed.0[1].1.start, Time(4 * S + S / 2));
    h.apply(Command::TrimClip {
        id: clip,
        start: Time(4 * S),
        source_in: Time(3 * S),
        source_out: Time(4 * S),
    })
    .unwrap();
    assert_eq!(panel.cache.placed(h.project(), &selected).0.len(), 1);
    let Created::Clip(repeat) = h
        .apply(Command::AddClip {
            track,
            clip: NewClip {
                asset: h.project().assets()[0].id,
                source_in: Time(2 * S),
                source_out: Time(4 * S),
                start: Time(6 * S),
                link: None,
            },
        })
        .unwrap()
    else {
        panic!()
    };
    let selected = HashSet::from([clip, repeat]);
    let placed = panel.cache.placed(h.project(), &selected);
    assert!(placed.1.is_empty());
    assert_eq!(placed.0.len(), 3);
    assert_eq!(placed.0[1].1.start, Time(6 * S));
    s.open();
    s.start(h.project(), &selected);
    let Done::Edit(edit) = finish(&mut s) else {
        panic!()
    };
    let Command::ImportSubtitles { cues, .. } = *edit else {
        panic!()
    };
    assert_eq!(cues[0].start, Time(4 * S));
    assert_eq!(cues[1].start, Time(6 * S));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn partial_subtitle_coverage_only_recognizes_the_two_missing_intervals() {
    let (_dir, mut h, clip, mut s, mut panel, calls) = fixture();
    let selected = HashSet::from([clip]);
    s.open();
    s.start(h.project(), &selected);
    finish(&mut s);
    h.apply(Command::TrimClip {
        id: clip,
        start: Time(S),
        source_in: Time(S),
        source_out: Time(6 * S),
    })
    .unwrap();
    let missing = panel.missing(h.project(), &selected);
    let p = SAMPLE_RATE as i64;
    assert_eq!(
        missing.iter().map(|s| s.range.clone()).collect::<Vec<_>>(),
        [p..2 * p, 4 * p..6 * p]
    );
    assert_eq!(panel.cache.placed(h.project(), &selected).0.len(), 2);
    s.engine = Some((Model::Base, Arc::new(Fake(calls.clone()))));
    s.open_transcribe(
        targets(h.project(), &selected, false)
            .into_iter()
            .map(|t| t.source)
            .collect(),
    );
    s.start(h.project(), &selected);
    finish(&mut s);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    complete_panel(&mut panel, h.project(), &selected);
    assert_eq!(panel.cache.placed(h.project(), &selected).0.len(), 5);
    s.open();
    s.start(h.project(), &selected);
    finish(&mut s);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    s.open_transcribe(
        targets(h.project(), &selected, false)
            .into_iter()
            .map(|t| t.source)
            .collect(),
    );
    s.force = true;
    s.engine = Some((Model::Base, Arc::new(Fake(calls.clone()))));
    s.start(h.project(), &selected);
    finish(&mut s);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    s.open_transcribe(
        targets(h.project(), &selected, false)
            .into_iter()
            .map(|t| t.source)
            .collect(),
    );
    s.language = 1;
    s.engine = Some((Model::Base, Arc::new(Fake(calls.clone()))));
    s.start(h.project(), &selected);
    finish(&mut s);
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    panel.cache.set_settings(s.transcript_settings());
    complete_panel(&mut panel, h.project(), &selected);
    let source = Source {
        nth: 1,
        ..targets(h.project(), &selected, false)[0].source.clone()
    };
    let request = s
        .store
        .lock()
        .unwrap()
        .request(&source, &s.transcript_settings())
        .unwrap();
    assert_eq!(s.store.lock().unwrap().lookup(&request).missing, [source]);
}

struct Empty(Arc<AtomicUsize>);

impl Transcriber for Empty {
    fn transcribe(
        &self,
        _: Vec<f32>,
        _: &Options,
        _: &jvid_jobs::Ctx,
    ) -> Result<Transcript, Error> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Transcript::default())
    }
}

#[test]
fn successful_empty_recognition_is_reused_as_silence() {
    let (dir, h, clip, mut s, mut panel, calls) = fixture();
    let selected = HashSet::from([clip]);
    let source = targets(h.project(), &selected, false)[0].source.clone();
    s.engine = Some((Model::Base, Arc::new(Empty(calls.clone()))));
    s.open_transcribe(vec![source.clone()]);
    s.start(h.project(), &selected);
    finish(&mut s);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    complete_panel(&mut panel, h.project(), &selected);
    assert!(panel.cache.placed(h.project(), &selected).0.is_empty());
    panel.cache = Cache::new(dir.path().join("cache"));
    s.store = panel.cache.store.clone();
    s.engine = None;
    complete_panel(&mut panel, h.project(), &selected);
    s.open_transcribe(vec![source]);
    s.start(h.project(), &selected);
    finish(&mut s);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
