//! Speech-cut proposals are view state until a validated, atomic Apply.

use std::path::PathBuf;
use std::time::SystemTime;

use jvid_core::history::{Command, History};
use jvid_core::model::{ClipId, Project};
use jvid_core::time::FLICKS_PER_SECOND as S;
use jvid_core::time::Time;
use std::collections::{HashMap, HashSet};

use crate::i18n::tr;
use crate::keymap::Applicability;
use crate::subtitles::AudioSources;
use crate::timeline::{Timeline, silence::Silence};
use crate::transcript::TranscriptPanel;
use suite_ui::widgets::{Button, Checkbox, DragField, State};

pub(crate) fn applicability(project: &Project, selected: &HashSet<ClipId>) -> Applicability {
    Applicability::when(
        !AudioSources::default()
            .clips(project.sequence(), selected, true)
            .is_empty(),
        "speech_review.unavailable",
    )
}

pub(crate) type Span = (ClipId, Time, Time);

/// Includes the project session: opening another project can reuse history revisions/IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Revision {
    pub session: u64,
    pub history: u64,
    pub transcript: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::history::History;
    use jvid_core::time::FLICKS_PER_SECOND as S;

    fn rev() -> Revision {
        Revision {
            session: 1,
            history: 0,
            transcript: 0,
        }
    }

    #[test]
    fn proposals_cancel_none_all_edges_and_undo() {
        let (p, mic, game) = crate::subtitles::test_support::two_streams(false);
        let mut plan = Plan::begin(&p, rev());
        plan.complete(vec![
            (mic, Time(S), Time(3 * S)),
            (game, Time(2 * S), Time(3 * S)),
            (mic, Time(5 * S), Time(7 * S)),
        ])
        .unwrap();
        assert_eq!(plan.saved(), Time(2 * S));
        assert_eq!(plan.audition(&p, rev(), false).unwrap(), p);
        let after = plan.audition(&p, rev(), true).unwrap();
        assert_eq!(after.sequence().duration(), Time(4 * S));
        let mut h = History::new(p.clone());
        assert_eq!(
            h.project(),
            &p,
            "analysis and audition never mutate history"
        );
        h.apply(plan.command(&p, rev()).unwrap().unwrap()).unwrap();
        assert_eq!(h.project(), &after);
        assert!(h.undo());
        assert_eq!(h.project(), &p);
        assert!(!h.can_undo());
        for proposal in &mut plan.proposals {
            proposal.accepted = false;
        }
        assert_eq!(plan.saved(), Time(0));
        assert_eq!(plan.command(&p, rev()).unwrap(), None);
        drop(plan);
        assert_eq!(h.project(), &p, "cancellation discards view state only");
    }

    #[test]
    fn edits_session_transcript_and_source_replacement_invalidate() {
        let (p, mic, _) = crate::subtitles::test_support::two_streams(false);
        let mut plan = Plan::begin(&p, rev());
        let mut h = History::new(p.clone());
        h.apply(Command::SetInOut {
            in_point: Some(Time(S)),
            out_point: None,
        })
        .unwrap();
        plan.complete(vec![(mic, Time(3 * S), Time(4 * S))])
            .unwrap();
        assert_eq!(plan.command(h.project(), rev()), Err(Refusal::Stale));
        for revision in [
            Revision {
                session: 2,
                ..rev()
            },
            Revision {
                history: 1,
                ..rev()
            },
            Revision {
                transcript: 1,
                ..rev()
            },
        ] {
            assert_eq!(plan.command(&p, revision), Err(Refusal::Stale));
        }
        std::fs::write(&p.assets()[0].path, b"replacement").unwrap();
        assert_eq!(plan.command(&p, rev()), Err(Refusal::Stale));
    }

    #[test]
    fn linked_av_captions_locks_marks_and_persistence() {
        use jvid_core::model::Text;
        let (p, mic, _) = crate::subtitles::test_support::two_streams(false);
        let mut h = History::new(p);
        h.apply(Command::ImportSubtitles {
            name: "Captions".into(),
            style: Text::default(),
            cues: vec![jvid_core::srt::Cue {
                start: Time(2 * S),
                end: Time(5 * S),
                text: "caption".into(),
            }],
        })
        .unwrap();
        h.apply(Command::SetInOut {
            in_point: Some(Time(S / 2)),
            out_point: Some(Time(5 * S)),
        })
        .unwrap();
        let before = h.project().clone();
        let mut plan = Plan::begin(&before, rev());
        plan.complete(vec![(mic, Time(3 * S), Time(4 * S))])
            .unwrap();
        h.apply(plan.command(&before, rev()).unwrap().unwrap())
            .unwrap();
        let seq = h.project().sequence();
        assert_eq!(
            (seq.in_point(), seq.out_point()),
            (Some(Time(S / 2)), Some(Time(5 * S)))
        );
        assert_eq!(seq.duration(), Time(5 * S));
        for track in seq
            .tracks()
            .iter()
            .filter(|t| !before.asset(t.clips()[0].asset).unwrap().generated_caption)
        {
            assert_eq!(track.clips().len(), 2);
            assert_eq!(track.clips()[0].end(), track.clips()[1].start);
            assert_eq!(track.clips()[1].source_at(Time(4 * S)), Time(4 * S));
        }
        assert_eq!(
            seq.tracks()
                .iter()
                .flat_map(|t| t.clips())
                .filter(|c| before.asset(c.asset).unwrap().generated_caption)
                .map(|c| c.end())
                .max()
                .unwrap(),
            Time(4 * S)
        );
        let saved = jvid_core::file::to_json(h.project(), std::path::Path::new("/")).unwrap();
        let loaded =
            jvid_core::file::from_json(&saved, std::path::Path::new("/"), |_| true).unwrap();
        assert_eq!(&loaded, h.project());
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        let mut settings = h.project().sequence().tracks()[0].settings();
        settings.locked = true;
        h.apply(Command::SetTrack(settings)).unwrap();
        let locked = h.project().clone();
        let mut plan = Plan::begin(&locked, rev());
        plan.complete(vec![(mic, Time(3 * S), Time(4 * S))])
            .unwrap();
        assert_eq!(plan.command(&locked, rev()), Err(Refusal::Locked));
        assert_eq!(plan.audition(&locked, rev(), false).unwrap(), locked);
        assert_eq!(h.project(), &locked);
    }

    #[test]
    fn cached_filler_review_uses_context_coverage_and_tunable_pauses() {
        let (p, mic, _) = crate::subtitles::test_support::two_streams(false);
        let selected = HashSet::from([mic]);
        let dir = tempfile::tempdir().unwrap();
        let mut panel = TranscriptPanel::default();
        panel.cache = crate::transcript::Cache::new(dir.path().into());
        let source = crate::subtitles::targets(&p, &selected, true)
            .remove(0)
            .source;
        let word = |text: &str, start, end| jvid_transcribe::Word {
            text: text.into(),
            start: Time(start),
            end: Time(end),
            confidence: 1.0,
        };
        panel.cache.insert(
            &source,
            jvid_transcribe::Transcript {
                language: Some("en".into()),
                words: vec![
                    word("hello", 3 * S / 2, 2 * S),
                    word("um", 2 * S, 9 * S / 4),
                    word("next", 4 * S, 17 * S / 4),
                ],
            },
        );
        let (hits, partial) = panel.review_hits(&p, &selected, Time(S), Time(S / 8));
        assert!(!partial);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().any(|(_, context)| context == "hello um next"));
        let revision = Revision {
            transcript: panel.review_revision(),
            ..rev()
        };
        let mut plan = Plan::begin(&p, revision);
        plan.complete(
            hits.into_iter()
                .map(|(h, _)| (h.clip, h.start, h.end))
                .collect(),
        )
        .unwrap();
        assert_eq!(plan.saved(), Time(7 * S / 4));
        plan.proposals[0].accepted = false;
        assert_eq!(plan.saved(), Time(3 * S / 2));
        assert!(matches!(
            plan.command(&p, revision).unwrap(),
            Some(Command::Batch(_))
        ));
        let (hits, _) = panel.review_hits(&p, &selected, Time(2 * S), Time(S / 8));
        assert_eq!(
            hits.len(),
            1,
            "longer pause threshold keeps only the filler word"
        );
    }

    #[test]
    fn source_snapshot_precedes_async_completion() {
        let (p, mic, _) = crate::subtitles::test_support::two_streams(false);
        let mut plan = Plan::begin(&p, rev());
        std::fs::write(&p.assets()[0].path, b"replaced while detector was running").unwrap();
        assert!(
            plan.current(&p, rev()),
            "frame checks use revisions; final Apply checks source metadata"
        );
        plan.complete(vec![(mic, Time(3 * S), Time(4 * S))])
            .unwrap();
        assert_eq!(plan.command(&p, rev()), Err(Refusal::Stale));
    }

    #[test]
    fn narrow_review_accept_reject_cancel_and_apply() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::{NodeT, Queryable};
        for size in [[320.0, 420.0], [640.0, 720.0]] {
            let (p, mic, _) = crate::subtitles::test_support::two_streams(false);
            let mut review = Review::open(&p, rev(), &HashSet::from([mic]));
            review
                .plan
                .complete(vec![(mic, Time(3 * S), Time(4 * S))])
                .unwrap();
            review.ready = true;
            let mut fresh = true;
            let mut h = Harness::builder().with_size(size).build_ui_state(
                move |ui, (review, timeline, transcript, output)| {
                    if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                        let next = review.show(ui.ctx(), &p, rev(), timeline, transcript);
                        if next.close {
                            *output = next;
                        }
                    }
                },
                (
                    review,
                    Timeline::default(),
                    TranscriptPanel::default(),
                    Output::default(),
                ),
            );
            h.run();
            let bounds = h.ctx.content_rect();
            for label in [
                tr!("speech_review.title"),
                tr!("prompt.cancel"),
                tr!("speech_review.apply"),
            ] {
                assert!(
                    bounds.contains_rect(h.get_by_label(label).rect()),
                    "{size:?}: {label}"
                );
            }
            h.get_by_label(tr!("speech_review.reject_all")).click();
            h.run();
            assert!(
                h.get_by_label(tr!("speech_review.apply"))
                    .accesskit_node()
                    .is_disabled()
            );
            assert_eq!(h.state().0.plan.saved(), Time(0));
            h.get_by_label(tr!("speech_review.accept_all")).click();
            h.run();
            h.get_by_label(tr!("speech_review.apply")).click();
            h.run();
            assert!(matches!(h.state().3.command, Some(Command::Batch(_))));
            assert!(h.state().3.close);
            h.state_mut().3 = Output::default();
            h.get_by_label(tr!("prompt.cancel")).click();
            h.run();
            assert!(h.state().3.close && h.state().3.command.is_none());
        }
    }

    #[test]
    #[ignore = "changes global locale and captures UI; run in isolation"]
    fn translated_review_artifacts() {
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                suite_ui::i18n::set_pseudo(false);
                suite_ui::i18n::set_language("en");
            }
        }
        let _restore = Restore;
        for language in ["en", "de", "fr", "ja"] {
            suite_ui::i18n::set_language(language);
            narrow_review_accept_reject_cancel_and_apply();
            if let Some(dir) = std::env::var_os("JVID_SPEECH_CAPTURE") {
                assert!(suite_ui::test_support::gpu());
                let (p, mic, _) = crate::subtitles::test_support::two_streams(false);
                let mut review = Review::open(&p, rev(), &HashSet::from([mic]));
                review
                    .plan
                    .complete(vec![(mic, Time(3 * S), Time(4 * S))])
                    .unwrap();
                review.ready = true;
                let mut fresh = true;
                let mut timeline = Timeline::default();
                let mut transcript = TranscriptPanel::default();
                let mut h = egui_kittest::Harness::builder()
                    .with_size([384.0, 520.0])
                    .wgpu()
                    .build_ui(move |ui| {
                        if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                            review.show(ui.ctx(), &p, rev(), &mut timeline, &mut transcript);
                        }
                    });
                h.run();
                std::fs::create_dir_all(&dir).unwrap();
                h.render()
                    .unwrap()
                    .save(PathBuf::from(dir).join(format!("speech-review-{language}.png")))
                    .unwrap();
            }
        }
        suite_ui::i18n::set_pseudo(true);
        narrow_review_accept_reject_cancel_and_apply();
    }

    #[test]
    fn audio_export_has_exact_samples_across_accepted_cut() {
        use jvid_core::history::Created;
        use jvid_core::model::{NewClip, ProbeInfo, TrackKind};
        use jvid_core::time::Rate;
        use jvid_jobs::{Pool, Priority};
        use jvid_media::audio::AudioDecoder;
        jvid_media::ffmpeg::init().unwrap();
        let fixture = jvid_media::audio::offset_fixture::Fixture::default();
        let mut h = History::new(Project::default());
        h.apply(Command::Import(vec![(
            fixture.0.clone(),
            ProbeInfo {
                duration: Time(2 * S),
                audio_streams: 2,
                ..Default::default()
            },
        )]))
        .unwrap();
        let Created::Track(track) = h
            .apply(Command::AddTrack {
                kind: TrackKind::Audio,
                name: "Audio".into(),
            })
            .unwrap()
        else {
            panic!()
        };
        let Created::Clip(id) = h
            .apply(Command::AddClip {
                track,
                clip: NewClip {
                    asset: h.project().assets()[0].id,
                    source_in: Time(0),
                    source_out: Time(2 * S),
                    start: Time(0),
                    link: None,
                },
            })
            .unwrap()
        else {
            panic!()
        };
        let mut plan = Plan::begin(h.project(), rev());
        plan.complete(vec![(id, Time(S / 2), Time(S))]).unwrap();
        h.apply(plan.command(h.project(), rev()).unwrap().unwrap())
            .unwrap();
        let p = h.project().clone();
        let path = fixture.0.parent().unwrap().join("speech-cut.wav");
        let request = crate::export::Request {
            path: path.clone(),
            range: (Time(0), Time(3 * S / 2)),
            preset: jvid_core::preset::builtin()
                .into_iter()
                .find(|p| p.name == "Audio Only (WAV)")
                .unwrap(),
            loudness: None,
            lossless: None,
            sidecar: false,
        };
        let pool = Pool::new(1);
        let (tx, rx) = std::sync::mpsc::channel();
        let _handle = pool.submit("speech_export", Priority::Interactive, move |ctx| {
            tx.send(crate::export::run_with_gpu(&p, &request, None, ctx))
                .unwrap();
        });
        rx.recv_timeout(std::time::Duration::from_secs(20))
            .unwrap()
            .unwrap();
        let mut decoder = AudioDecoder::open(&path, 0).unwrap();
        let mut samples = Vec::new();
        while let Some(chunk) = decoder.next_chunk().unwrap() {
            samples.extend(chunk.samples);
        }
        assert_eq!(samples.len(), 72_000 * 2);
        let gain = std::f32::consts::FRAC_1_SQRT_2;
        // Assert every sample, including both sides of the splice: no inserted silence or
        // duplicate samples; PCM quantization is the only allowed difference.
        for (i, pair) in samples.as_chunks::<2>().0.iter().enumerate() {
            let source = i as i64 + if i >= 24_000 { 24_000 } else { 0 };
            let want = jvid_media::audio::offset_fixture::Fixture::sample(0, source) * gain;
            assert!(
                pair.iter().all(|s| (*s - want).abs() < 3.0 / 32768.0),
                "sample {i}: {pair:?} vs {want}"
            );
        }
        assert_eq!(
            Rate::new(48_000, 1).time_floor(72_000),
            h.project().sequence().duration()
        );
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceStamp(PathBuf, Option<(u64, SystemTime)>);

fn source_stamp(path: &PathBuf) -> SourceStamp {
    let stamp = std::fs::metadata(path)
        .ok()
        .and_then(|m| Some((m.len(), m.modified().ok()?)));
    SourceStamp(path.clone(), stamp)
}

fn sources(project: &Project, selected: &HashSet<ClipId>) -> Vec<SourceStamp> {
    let mut paths: Vec<_> = project
        .sequence()
        .tracks()
        .iter()
        .flat_map(|t| t.clips())
        .filter(|c| selected.contains(&c.id))
        .filter_map(|c| project.asset(c.asset))
        .filter(|a| a.probe.is_some())
        .map(|a| a.path.clone())
        .collect();
    paths.sort();
    paths.dedup();
    paths.iter().map(source_stamp).collect()
}

#[derive(Clone, Debug)]
pub(crate) struct Proposal {
    pub span: Span,
    pub accepted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    Stale,
    Invalid,
    Locked,
}

/// Source/project snapshot is captured before analysis starts, never when it completes.
pub(crate) struct Plan {
    project: Project,
    revision: Revision,
    sources: Vec<SourceStamp>,
    pub proposals: Vec<Proposal>,
}

impl Plan {
    #[cfg(test)]
    pub fn begin(project: &Project, revision: Revision) -> Self {
        Self::begin_for(
            project,
            revision,
            &project
                .sequence()
                .tracks()
                .iter()
                .flat_map(|t| t.clips())
                .map(|c| c.id)
                .collect(),
        )
    }

    fn begin_for(project: &Project, revision: Revision, selected: &HashSet<ClipId>) -> Self {
        Self {
            project: project.clone(),
            revision,
            sources: sources(project, selected),
            proposals: Vec::new(),
        }
    }

    pub fn current(&self, project: &Project, revision: Revision) -> bool {
        self.revision == revision && self.project == *project
    }

    /// Reject malformed detector results and clamp edge proposals to actual clip bounds.
    pub fn complete(&mut self, spans: Vec<Span>) -> Result<(), Refusal> {
        let mut proposals = Vec::new();
        for (id, a, b) in spans {
            let (_, clip) = self.project.sequence().clip(id).ok_or(Refusal::Invalid)?;
            if a >= b {
                return Err(Refusal::Invalid);
            }
            let span = (id, a.max(clip.start), b.min(clip.end()));
            if span.1 < span.2 {
                proposals.push(Proposal {
                    span,
                    accepted: true,
                });
            }
        }
        proposals.sort_by_key(|p| (p.span.1, p.span.0));
        proposals.dedup_by_key(|p| p.span);
        self.proposals = proposals;
        Ok(())
    }

    /// Overlapping suggestions from multiple streams remove time only once.
    pub fn ranges(&self) -> Vec<(Time, Time)> {
        let mut spans: Vec<_> = self
            .proposals
            .iter()
            .filter(|p| p.accepted)
            .map(|p| (p.span.1, p.span.2))
            .collect();
        spans.sort_unstable();
        let mut ranges: Vec<(Time, Time)> = Vec::new();
        for (a, b) in spans {
            if let Some(last) = ranges.last_mut()
                && a <= last.1
            {
                last.1 = last.1.max(b);
            } else {
                ranges.push((a, b));
            }
        }
        ranges
    }

    pub fn saved(&self) -> Time {
        Time(self.ranges().iter().map(|(a, b)| (*b - *a).0).sum())
    }

    /// Same validated command drives audition and Apply. Existing sequence extraction cuts
    /// all tracks (captions included), preserving sync even for unlinked background media.
    pub fn command(
        &self,
        project: &Project,
        revision: Revision,
    ) -> Result<Option<Command>, Refusal> {
        if !self.current(project, revision) || self.sources.iter().any(|s| *s != source_stamp(&s.0))
        {
            return Err(Refusal::Stale);
        }
        let ranges = self.ranges();
        let Some(_) = ranges.first() else {
            return Ok(None);
        };
        if self.locked(project) {
            return Err(Refusal::Locked);
        }
        let mut commands = Vec::new();
        for &(a, b) in ranges.iter().rev() {
            commands.push(Command::SetInOut {
                in_point: Some(a),
                out_point: Some(b),
            });
            commands.push(Command::RemoveRange { ripple: true });
        }
        // Analysis and temporary extraction marks never replace the user's marks.
        commands.push(Command::SetInOut {
            in_point: project.sequence().in_point(),
            out_point: project.sequence().out_point(),
        });
        let command = Command::Batch(commands);
        let mut candidate = History::new(project.clone());
        candidate
            .apply(command.clone())
            .map_err(|_| Refusal::Invalid)?;
        Ok(Some(command))
    }

    fn locked(&self, project: &Project) -> bool {
        self.ranges().first().is_some_and(|(first, _)| {
            project
                .sequence()
                .tracks()
                .iter()
                .any(|t| t.locked && t.clips().iter().any(|c| c.end() > *first))
        })
    }

    pub fn audition(
        &self,
        project: &Project,
        revision: Revision,
        after: bool,
    ) -> Result<Project, Refusal> {
        if !after {
            if !self.current(project, revision)
                || self.sources.iter().any(|s| *s != source_stamp(&s.0))
            {
                return Err(Refusal::Stale);
            }
            return Ok(project.clone());
        }
        let command = self.command(project, revision)?;
        let mut candidate = History::new(project.clone());
        if after && let Some(command) = command {
            candidate.apply(command).map_err(|_| Refusal::Invalid)?;
        }
        Ok(candidate.project().clone())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Silence,
    Fillers,
}

/// Audio audition runs the same project mixer/export evaluation on a private candidate.
struct Audition {
    project: Project,
    audio: crate::playback::Audio,
    output: jvid_audio::Output,
    transport: crate::transport::Transport,
}

impl Audition {
    fn start(project: Project, start: Time, end: Time) -> Self {
        let mut audio = crate::playback::Audio::new(Box::new(|path, nth| {
            let mut source = crate::transport::AudioSource::stream(path, nth);
            Box::new(move |start, out| source.fill(start, out))
        }));
        audio.prime(&project, start);
        let output = jvid_audio::Output::new(Box::new(jvid_audio::CpalBackend::default()), None);
        let mut transport = crate::transport::Transport::new(end, project.settings().frame_rate);
        transport.seek(start, output.clock());
        transport.toggle(output.clock());
        Self {
            project,
            audio,
            output,
            transport,
        }
    }

    fn tick(&mut self) -> bool {
        self.output.poll();
        self.audio
            .prime(&self.project, self.transport.position(self.output.clock()));
        self.transport.tick(&mut self.output, &mut |start, out| {
            self.audio.mix(&self.project, start, out)
        });
        self.transport.speed() != 0
    }
}

impl Drop for Audition {
    fn drop(&mut self) {
        self.output.flush();
    }
}

pub(crate) struct Review {
    plan: Plan,
    input_selection: HashSet<ClipId>,
    selected: HashSet<ClipId>,
    sources: AudioSources,
    detector: Option<Silence>,
    mode: Mode,
    threshold: f64,
    pause: f64,
    breathing: f64,
    ready: bool,
    partial: bool,
    selected_proposal: usize,
    audition: Option<Audition>,
    error: Option<Refusal>,
    contexts: HashMap<Span, String>,
}

#[derive(Default)]
pub(crate) struct Output {
    pub command: Option<Command>,
    pub close: bool,
}

impl Review {
    pub fn open(project: &Project, revision: Revision, selected: &HashSet<ClipId>) -> Self {
        let ids = AudioSources::default()
            .clips(project.sequence(), selected, true)
            .iter()
            .map(|c| c.id)
            .collect();
        let mut review = Self {
            plan: Plan::begin_for(project, revision, &ids),
            input_selection: selected.clone(),
            selected: HashSet::new(),
            sources: AudioSources::default(),
            detector: None,
            mode: Mode::Silence,
            threshold: -40.0,
            pause: 1.0,
            breathing: 0.15,
            ready: false,
            partial: false,
            selected_proposal: 0,
            audition: None,
            error: None,
            contexts: HashMap::new(),
        };
        review.recompute(project, revision);
        review
    }

    fn recompute(&mut self, project: &Project, revision: Revision) {
        self.audition = None;
        self.selected = self
            .sources
            .clips(project.sequence(), &self.input_selection, true)
            .iter()
            .map(|c| c.id)
            .collect();
        self.plan = Plan::begin_for(project, revision, &self.selected);
        self.detector = Silence::open(project.sequence(), &self.selected);
        if let Some(detector) = &mut self.detector {
            detector.configure(self.threshold, self.pause, self.breathing);
        }
        self.ready = self.selected.is_empty();
        self.partial = false;
        self.error = None;
        self.contexts.clear();
        self.selected_proposal = 0;
    }

    /// Modal owns input; changing settings/recomputing only replaces this view state.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        project: &Project,
        revision: Revision,
        timeline: &mut Timeline,
        transcript: &mut TranscriptPanel,
    ) -> Output {
        use suite_ui::tokens::{color, space};
        let mut out = Output::default();
        let current = self.plan.current(project, revision) && self.error != Some(Refusal::Stale);
        if !current {
            self.audition = None;
            self.detector = None;
        }
        if current && !self.ready {
            let spans = match self.mode {
                Mode::Silence => self
                    .detector
                    .as_mut()
                    .and_then(|detector| timeline.review_silence_spans(detector, project, ctx)),
                Mode::Fillers => {
                    let (hits, partial) = transcript.review_hits(
                        project,
                        &self.selected,
                        Time((self.pause * S as f64) as i64),
                        Time((self.breathing * S as f64) as i64),
                    );
                    self.partial = partial;
                    Some(
                        hits.into_iter()
                            .map(|(h, context)| {
                                let span = (h.clip, h.start, h.end);
                                self.contexts.insert(span, context);
                                span
                            })
                            .collect(),
                    )
                }
            };
            if let Some(spans) = spans {
                self.error = self.plan.complete(spans).err();
                self.ready = self.error.is_none();
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
        if let Some(audition) = &mut self.audition {
            if !audition.tick() {
                self.audition = None;
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        let revision = Revision {
            transcript: transcript.review_revision(),
            ..revision
        };
        let current = self.plan.current(project, revision) && self.error != Some(Refusal::Stale);
        let modal = egui::Modal::new(egui::Id::new("speech_review")).show(ctx, |ui| {
            ui.set_width(440.0_f32.min((ctx.content_rect().width() - 48.0).max(160.0)));
            ui.label(
                egui::RichText::new(tr!("speech_review.title")).font(suite_ui::theme::headline()),
            );
            ui.add_space(space::S8);
            let mut changed = false;
            egui::ScrollArea::vertical()
                .id_salt("speech_review_body")
                .max_height((ctx.content_rect().height() - 150.0).max(80.0))
                .show(ui, |ui| {
                    ui.label(tr!("speech_review.scope"));
                    changed |=
                        self.sources
                            .show(ui, project.sequence(), &self.input_selection, true);
                    ui.horizontal_wrapped(|ui| {
                        changed |= ui
                            .selectable_value(&mut self.mode, Mode::Silence, tr!("silence.title"))
                            .changed();
                        changed |= ui
                            .selectable_value(
                                &mut self.mode,
                                Mode::Fillers,
                                tr!("transcript.fillers"),
                            )
                            .changed();
                    });
                    if self.mode == Mode::Silence {
                        ui.label(tr!("silence.threshold"));
                        changed |= ui
                            .add(
                                DragField::new(&mut self.threshold)
                                    .range(-80.0..=-10.0)
                                    .suffix(tr!("silence.db_suffix"))
                                    .width(100.0)
                                    .decimals(0)
                                    .speed(0.25),
                            )
                            .changed();
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tr!("silence.min_len"));
                        changed |= ui
                            .add(
                                DragField::new(&mut self.pause)
                                    .range(0.1..=30.0)
                                    .suffix(tr!("silence.seconds_suffix"))
                                    .width(100.0)
                                    .decimals(2)
                                    .speed(0.02),
                            )
                            .changed();
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tr!("silence.padding"));
                        changed |= ui
                            .add(
                                DragField::new(&mut self.breathing)
                                    .range(0.0..=2.0)
                                    .suffix(tr!("silence.seconds_suffix"))
                                    .width(100.0)
                                    .decimals(2)
                                    .speed(0.01),
                            )
                            .changed();
                    });
                    if !current {
                        ui.colored_label(color::amber(), tr!("speech_review.stale"));
                        changed |= ui
                            .add(Button::secondary(tr!("speech_review.recompute")))
                            .clicked();
                    } else if !self.ready {
                        ui.label(tr!("silence.reading"));
                    }
                    if self.partial {
                        ui.label(tr!("transcript.partial"));
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add(Button::secondary(tr!("speech_review.accept_all")))
                            .clicked()
                        {
                            self.audition = None;
                            for proposal in &mut self.plan.proposals {
                                proposal.accepted = true;
                            }
                        }
                        if ui
                            .add(Button::secondary(tr!("speech_review.reject_all")))
                            .clicked()
                        {
                            self.audition = None;
                            for proposal in &mut self.plan.proposals {
                                proposal.accepted = false;
                            }
                        }
                    });
                    for (i, proposal) in self.plan.proposals.iter_mut().enumerate() {
                        let start = format!("{:.2}", proposal.span.1.0 as f64 / S as f64);
                        let end = format!("{:.2}", proposal.span.2.0 as f64 / S as f64);
                        ui.push_id(i, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .add(Checkbox::new(
                                        &mut proposal.accepted,
                                        &tr!("speech_review.cut", start = start, end = end),
                                    ))
                                    .changed()
                                {
                                    self.audition = None;
                                }
                                ui.selectable_value(
                                    &mut self.selected_proposal,
                                    i,
                                    tr!("speech_review.audition"),
                                );
                            })
                        });
                        if let Some(context) = self.contexts.get(&proposal.span) {
                            ui.label(context);
                        }
                    }
                    if self.ready && self.plan.proposals.is_empty() {
                        ui.label(tr!("transcript.none"));
                    }
                    let seconds = format!("{:.2}", self.plan.saved().0 as f64 / S as f64);
                    ui.label(tr!("speech_review.saved", seconds = seconds));
                    let enabled =
                        current && self.ready && !changed && !self.plan.proposals.is_empty();
                    let state = if enabled {
                        State::Live
                    } else {
                        State::Disabled
                    };
                    ui.horizontal_wrapped(|ui| {
                        for (after, label) in [
                            (false, tr!("speech_review.before")),
                            (true, tr!("speech_review.after")),
                        ] {
                            if ui.add(Button::secondary(label).state(state)).clicked() {
                                match self.plan.audition(project, revision, after) {
                                    Ok(candidate) => {
                                        let at = self.plan.proposals[self.selected_proposal].span.1;
                                        let removed = if after {
                                            self.plan
                                                .ranges()
                                                .iter()
                                                .map(|(a, b)| (at.min(*b) - *a).0.max(0))
                                                .sum()
                                        } else {
                                            0
                                        };
                                        let start = Time((at.0 - removed - S).max(0));
                                        let end = (start + Time(6 * S))
                                            .min(candidate.sequence().duration());
                                        self.audition =
                                            Some(Audition::start(candidate, start, end));
                                    }
                                    Err(error) => self.error = Some(error),
                                }
                            }
                        }
                        if ui.add(Button::secondary(tr!("preview.stop"))).clicked() {
                            self.audition = None;
                        }
                    });
                });
            if changed {
                self.recompute(
                    project,
                    Revision {
                        transcript: transcript.review_revision(),
                        ..revision
                    },
                );
            }
            let locked = self.plan.locked(project);
            if let Some(error) = self.error.or(locked.then_some(Refusal::Locked)) {
                ui.colored_label(
                    color::amber(),
                    match error {
                        Refusal::Stale => tr!("speech_review.stale"),
                        Refusal::Locked => tr!("speech_review.locked"),
                        Refusal::Invalid => tr!("speech_review.invalid"),
                    },
                );
            }
            let actions = suite_ui::dialog::action_footer(
                ui,
                [
                    Button::secondary(tr!("prompt.cancel")),
                    Button::primary(tr!("speech_review.apply")).state(
                        if current
                            && self.ready
                            && !changed
                            && !locked
                            && self.plan.saved() > Time(0)
                        {
                            State::Live
                        } else {
                            State::Disabled
                        },
                    ),
                ],
            );
            out.close = actions[0].clicked();
            if actions[1].clicked() {
                match self.plan.command(project, revision) {
                    Ok(command) => {
                        out.command = command;
                        out.close = true;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        });
        out.close |= modal.should_close();
        out
    }
}
