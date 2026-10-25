//! Generate Subtitles: transcribes the selected clips, or the audio between In and Out, and
//! lays the words out as caption cues on a new subtitle track, as one undoable edit.
//! The model downloads on first use; its progress and Cancel show in the dialog, and closing
//! the dialog cancels whatever is running. The same dialog transcribes whole files for the
//! Transcript panel ([`Subtitles::open_transcribe`]).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use egui::{RichText, Ui};
use jvid_core::history::Command;
use jvid_core::model::{Clip, ClipId, Project, Sequence, Text, TrackKind};
use jvid_core::srt::Cue;
use jvid_core::time::{FLICKS_PER_SECOND as S, Rate, Time};
use jvid_jobs::{Handle, Pool, Priority};
use jvid_media::audio::SAMPLE_RATE;
use jvid_transcribe::models::{self, Model};
use jvid_transcribe::{Error, Options, Source, Transcriber, Transcript, Whisper, Word};
use suite_ui::i18n::{self, LANGUAGES};
use suite_ui::theme;
use suite_ui::tokens::{color, radius, space};
use suite_ui::widgets::{Button, Checkbox, Dropdown, State};

use crate::i18n::{tr, verbatim};

/// Caption limits: lines of about this many characters, at most two of them, on screen for
/// about this long.
const LINE: usize = 42;
const MAX_LEN: Time = Time(7 * S);
/// A gap this long between words ends the caption.
const PAUSE: Time = Time(S / 2);
/// A short caption stays up this long, unless the next word comes first.
const MIN_LEN: Time = Time(S);

/// Whisper's code for a shipped language, whose names the picker shows after Auto.
fn whisper_code(tag: &str) -> &str {
    match tag {
        "nb" => "no",
        _ => tag.split('-').next().unwrap_or(tag),
    }
}

/// Captions for `words` (sorted, in timeline time): a new caption at sentence ends and pauses,
/// and when the next word would pass two lines or [`MAX_LEN`], then breaking after the last
/// comma in its second half if there is one.
pub fn cues(words: &[Word]) -> Vec<Cue> {
    let mut groups: Vec<Vec<&Word>> = Vec::new();
    let mut cue: Vec<&Word> = Vec::new();
    for w in words {
        if let Some(last) = cue.last() {
            let over = |c: &[&Word]| {
                let texts: Vec<&str> = c.iter().chain([&w]).map(|w| w.text.as_str()).collect();
                lines(&texts).is_none() || w.end - c[0].start > MAX_LEN
            };
            if ends_sentence(&last.text) || w.start - last.end >= PAUSE {
                groups.push(std::mem::take(&mut cue));
            } else if over(&cue) {
                let half = cue.len().div_ceil(2);
                let at = cue
                    .iter()
                    .rposition(|w| soft_break(&w.text))
                    .map_or(cue.len(), |k| k + 1)
                    .max(half);
                let rest = cue.split_off(at);
                groups.push(std::mem::replace(&mut cue, rest));
                if !cue.is_empty() && over(&cue) {
                    groups.push(std::mem::take(&mut cue));
                }
            }
        }
        cue.push(w);
    }
    groups.push(cue);
    groups.retain(|g| !g.is_empty());
    let starts: Vec<Time> = groups.iter().skip(1).map(|g| g[0].start).collect();
    groups
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let texts: Vec<&str> = g.iter().map(|w| w.text.as_str()).collect();
            let start = g[0].start;
            let next = starts.get(i).copied().unwrap_or(Time(i64::MAX));
            let end = g[g.len() - 1].end.max((start + MIN_LEN).min(next));
            // shortcut: a single word over two lines (whisper gives Chinese and Japanese as
            // sentence-long words) stays one long line; split by characters if that's common.
            let text = lines(&texts).unwrap_or_else(|| texts.join(" "));
            Cue { start, end, text }
        })
        .collect()
}

/// `words` as one line, or two balanced lines, within [`LINE`] characters each; `None` if
/// they don't fit.
fn lines(words: &[&str]) -> Option<String> {
    let len = |w: &[&str]| w.iter().map(|w| w.chars().count()).sum::<usize>() + w.len() - 1;
    if len(words) <= LINE {
        return Some(words.join(" "));
    }
    (1..words.len())
        .map(|i| (len(&words[..i]).max(len(&words[i..])), i))
        .filter(|&(longest, _)| longest <= LINE)
        .min()
        .map(|(_, i)| words[..i].join(" ") + "\n" + &words[i..].join(" "))
}

/// The word, closing quotes aside, ends a sentence.
fn ends_sentence(w: &str) -> bool {
    let w = w.trim_end_matches(['"', '\'', '”', '’', ')', '»', '」']);
    w.ends_with(['.', '?', '!', '…', '。', '？', '！'])
}

fn soft_break(w: &str) -> bool {
    w.ends_with([',', ';', ':', '—', '–', '，', '、', '；', '：'])
}

/// A clip to transcribe, as it was when Generate was clicked, and the part of the timeline to
/// caption.
#[derive(Clone)]
pub(crate) struct Target {
    pub clip: Clip,
    pub span: (Time, Time),
    pub source: Source,
}

impl Target {
    /// The `words` (in file time) that fall in the span, moved to timeline time.
    pub fn place<'a>(&'a self, words: &'a [Word]) -> impl Iterator<Item = Word> + 'a {
        let c = &self.clip;
        words.iter().filter_map(move |w| {
            let (start, end) = (c.timeline_at(w.start), c.timeline_at(w.end));
            (start >= self.span.0 && start < self.span.1).then(|| Word {
                start,
                end: end.min(self.span.1),
                ..w.clone()
            })
        })
    }
}

/// One deliberate audio source per linked group, shared by every speech/peak tool.
/// Explicit audio selection wins before linked partners are considered. Preferences only
/// resolve ambiguity among equally explicit candidates, so selecting Mic always wins.
#[derive(Default)]
pub(crate) struct AudioSources {
    preferred: HashSet<ClipId>,
}

impl AudioSources {
    fn groups<'a>(
        &self,
        seq: &'a Sequence,
        selected: &HashSet<ClipId>,
        reverse: bool,
    ) -> Vec<Vec<&'a Clip>> {
        let mut clips: Vec<_> = seq
            .tracks()
            .iter()
            .filter(|t| t.kind == TrackKind::Audio && (!selected.is_empty() || !t.muted))
            .flat_map(|t| t.clips())
            .filter(|c| c.enabled && c.freeze.is_none() && (reverse || !c.reverse))
            .filter(|c| {
                selected.is_empty()
                    || selected.contains(&c.id)
                    || selected
                        .iter()
                        .any(|&id| seq.linked(id).iter().any(|(_, p)| p.id == c.id))
            })
            .collect();
        clips.sort_by_key(|c| {
            (
                !selected.contains(&c.id),
                !self.preferred.contains(&c.id),
                c.start,
                c.id.0,
            )
        });
        let mut groups: Vec<Vec<&Clip>> = Vec::new();
        for c in clips {
            if let Some(g) = groups
                .iter_mut()
                .find(|g| c.link.is_some() && g[0].link == c.link)
            {
                g.push(c);
            } else {
                groups.push(vec![c]);
            }
        }
        for g in &mut groups {
            if g.iter().any(|c| selected.contains(&c.id)) {
                g.retain(|c| selected.contains(&c.id));
            }
        }
        groups.sort_by_key(|g| (g[0].start, g[0].id.0));
        groups
    }

    pub(crate) fn clips<'a>(
        &self,
        seq: &'a Sequence,
        selected: &HashSet<ClipId>,
        reverse: bool,
    ) -> Vec<&'a Clip> {
        self.groups(seq, selected, reverse)
            .into_iter()
            .map(|g| g[0])
            .collect()
    }

    pub(crate) fn selection(&self, seq: &Sequence, selected: &HashSet<ClipId>) -> HashSet<ClipId> {
        let ids: HashSet<_> = self
            .clips(seq, selected, false)
            .iter()
            .map(|c| c.id)
            .collect();
        if ids.is_empty() {
            selected.clone()
        } else {
            ids
        }
    }

    /// Shows the analyzed stream even when there is only one; ambiguous groups get a picker.
    /// Returns true when an old preview must be discarded.
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        seq: &Sequence,
        selected: &HashSet<ClipId>,
        reverse: bool,
    ) -> bool {
        let mut changed = false;
        for g in self.groups(seq, selected, reverse) {
            let labels: Vec<_> = g
                .iter()
                .map(|c| {
                    let name = seq
                        .clip(c.id)
                        .and_then(|(t, _)| seq.track(t))
                        .map(|t| t.name.as_str())
                        .unwrap_or_default();
                    tr!(
                        "analysis.source_stream",
                        name = name,
                        number = c.stream.unwrap_or(0) + 1
                    )
                })
                .collect();
            ui.horizontal(|ui| {
                ui.label(tr!("analysis.audio_source"));
                if g.len() == 1 {
                    ui.label(&labels[0]);
                } else {
                    let names: Vec<_> = labels.iter().map(String::as_str).collect();
                    let mut pick = 0;
                    ui.push_id(g[0].link, |ui| {
                        ui.add(Dropdown::new(&mut pick, &names).width(200.0));
                    });
                    if pick != 0 {
                        for c in &g {
                            self.preferred.remove(&c.id);
                        }
                        self.preferred.insert(g[pick].id);
                        changed = true;
                    }
                }
            });
        }
        changed
    }
}

/// Selected audio (before linked expansion), or enabled audio between In and Out / whole
/// sequence. Reversed and frozen clips are left out. Source ranges keep their chosen stream.
pub(crate) fn targets(project: &Project, selected: &HashSet<ClipId>, whole: bool) -> Vec<Target> {
    targets_with_sources(project, selected, whole, &AudioSources::default())
}

fn targets_with_sources(
    project: &Project,
    selected: &HashSet<ClipId>,
    whole: bool,
    sources: &AudioSources,
) -> Vec<Target> {
    let seq = project.sequence();
    let span = if selected.is_empty() {
        let Some(span) = whole
            .then_some((Time(0), seq.duration()))
            .or_else(|| in_out(seq))
        else {
            return Vec::new();
        };
        Some(span)
    } else {
        None
    };
    let rate = Rate::new(SAMPLE_RATE, 1);
    sources
        .clips(seq, selected, false)
        .into_iter()
        .filter_map(|c| {
            let span = span.map_or((c.start, c.end()), |(a, b)| {
                (c.start.max(a), c.end().min(b))
            });
            if span.0 >= span.1 {
                return None;
            }
            let asset = project.asset(c.asset)?;
            let from = c.source_at(span.0);
            let to = if span.1 >= c.end() {
                c.source_out
            } else {
                c.source_at(span.1)
            };
            Some(Target {
                clip: c.clone(),
                span,
                source: Source {
                    path: asset.path.clone(),
                    nth: c.stream.unwrap_or(0) as usize,
                    range: rate.index_floor(from)..rate.index_floor(to),
                },
            })
        })
        .collect()
}

/// In to Out, either end defaulting to the sequence's; `None` with neither set.
fn in_out(seq: &Sequence) -> Option<(Time, Time)> {
    let (i, o) = (seq.in_point(), seq.out_point());
    (i.is_some() || o.is_some()).then(|| (i.unwrap_or(Time(0)), o.unwrap_or(seq.duration())))
}

/// What a job sends back.
enum Msg {
    Downloaded(Result<PathBuf, Error>),
    Loaded(Result<Arc<dyn Transcriber>, Error>),
    /// The transcript of `Run::sources[i]`.
    Transcribed(usize, Result<Transcript, Error>),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    Downloading,
    Loading,
    Transcribing,
}

/// A Generate in progress; dropping it cancels its jobs.
struct Run {
    model: Model,
    opts: Options,
    targets: Vec<Target>,
    /// Each source once: duplicated clips share a transcript, and the pool would drop a
    /// second identical job's result.
    sources: Vec<Source>,
    done: Vec<Option<Transcript>>,
    stage: Stage,
    handles: Vec<Handle>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl Drop for Run {
    fn drop(&mut self) {
        self.handles.iter().for_each(Handle::cancel);
    }
}

impl Run {
    fn load(&mut self, jobs: &Pool, path: PathBuf) {
        self.stage = Stage::Loading;
        let (tx, model) = (self.tx.clone(), self.model);
        let key = format!("whisper:{}", path.display());
        let job = jobs.submit(key, Priority::Interactive, move |_| {
            let r = Whisper::load(&path, model).map(|w| Arc::new(w) as Arc<dyn Transcriber>);
            let _ = tx.send(Msg::Loaded(r));
        });
        self.handles = vec![job];
    }

    fn transcribe(&mut self, jobs: &Pool, engine: &Arc<dyn Transcriber>) {
        self.stage = Stage::Transcribing;
        self.handles = (self.sources.iter().enumerate())
            .map(|(i, s)| {
                let tx = self.tx.clone();
                let done = move |r| {
                    let _ = tx.send(Msg::Transcribed(i, r));
                };
                jvid_transcribe::submit(jobs, engine.clone(), s.clone(), self.opts.clone(), done)
            })
            .collect();
    }

    /// The share done of the stage, if it can tell.
    fn progress(&self) -> Option<f32> {
        let sum: f32 = self.handles.iter().map(Handle::progress).sum();
        (self.stage != Stage::Loading).then(|| sum / self.handles.len().max(1) as f32)
    }

    /// Every target's words as captions, once every source is transcribed.
    fn cues(&self) -> Option<Vec<Cue>> {
        let mut words = Vec::new();
        for t in &self.targets {
            let i = self.sources.iter().position(|s| *s == t.source)?;
            words.extend(t.place(&self.done[i].as_ref()?.words));
        }
        words.sort_by_key(|w| w.start);
        Some(cues(&words))
    }
}

/// The value column's width.
const FIELD: f32 = 220.0;

/// What a finished run hands back.
pub enum Done {
    /// The subtitle track, to apply.
    Edit(Box<Command>),
    /// [`Subtitles::open_transcribe`]'s sources, each with its transcript.
    Transcribed(Vec<(Source, Transcript)>),
}

/// The Generate Subtitles dialog and the jobs it runs.
#[derive(Default)]
pub struct Subtitles {
    open: bool,
    /// 0 is Auto, then [`LANGUAGES`].
    language: usize,
    sources: AudioSources,
    /// 0 is Fast, 1 Accurate.
    quality: usize,
    translate: bool,
    /// The models on disk, as of opening.
    downloaded: Vec<Model>,
    run: Option<Run>,
    error: Option<String>,
    /// Set by [`Self::open_transcribe`]: transcribes these instead of making subtitles.
    transcribe: Option<Vec<Source>>,
    /// Started on the first Generate, so launching the editor spawns no threads for it.
    pub(crate) jobs: Option<Pool>,
    /// The model loaded for this dialog, freed when it closes; tests put a fake here.
    pub(crate) engine: Option<(Model, Arc<dyn Transcriber>)>,
}

impl Subtitles {
    /// Whether there's audio to caption: selected clips, or clips between In and Out.
    pub fn can_open(project: &Project, selected: &HashSet<ClipId>) -> bool {
        !targets(project, selected, false).is_empty()
    }

    pub fn open(&mut self) {
        self.transcribe = None;
        self.sources = AudioSources::default();
        self.show_dialog();
    }

    /// Opens as Transcribe: the same choices bar Translate, as cuts need the spoken words.
    pub fn open_transcribe(&mut self, sources: Vec<Source>) {
        self.show_dialog();
        self.translate = false;
        self.transcribe = Some(sources);
    }

    fn show_dialog(&mut self) {
        self.open = true;
        self.error = None;
        let dir = models::dir();
        self.downloaded = (Model::ALL.into_iter())
            .filter(|m| dir.as_deref().is_some_and(|d| m.is_downloaded(d)))
            .collect();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Cancels anything running and frees the model.
    pub(crate) fn close(&mut self) {
        self.open = false;
        self.run = None;
        self.engine = None;
    }

    fn model(&self) -> Model {
        match (self.quality, self.translate) {
            (0, _) => Model::Base,
            // large-v3-turbo wasn't trained to translate.
            (_, true) => Model::Small,
            _ => Model::LargeV3Turbo,
        }
    }

    /// Clicks Generate, or Transcribe, as a lesson's Show Me does: only once the model is on
    /// disk or loaded, so a download waits for the user to choose it in the dialog.
    pub(crate) fn start_if_ready(&mut self, project: &Project, selected: &HashSet<ClipId>) {
        let m = self.model();
        if self.engine.as_ref().is_some_and(|(e, _)| *e == m) || self.downloaded.contains(&m) {
            self.start(project, selected);
        }
    }

    fn start(&mut self, project: &Project, selected: &HashSet<ClipId>) {
        let targets = match self.transcribe {
            Some(_) => Vec::new(),
            None => targets_with_sources(project, selected, false, &self.sources),
        };
        let mut sources: Vec<Source> = self.transcribe.clone().unwrap_or_default();
        for t in &targets {
            if !sources.contains(&t.source) {
                sources.push(t.source.clone());
            }
        }
        let (tx, rx) = channel();
        let language = (self.language.checked_sub(1)).map(|i| whisper_code(LANGUAGES[i]));
        // Edit by Text keeps the fillers so Fillers can find them; Auto guesses the UI language's.
        let fillers = (self.transcribe.as_ref()).map(|_| {
            crate::transcript::fillers::prompt(language.unwrap_or(whisper_code(i18n::language())))
        });
        let mut run = Run {
            model: self.model(),
            opts: Options {
                language: language.map(str::to_owned),
                initial_prompt: fillers.map(str::to_owned),
                translate: self.translate,
            },
            done: vec![None; sources.len()],
            targets,
            sources,
            stage: Stage::Loading,
            handles: Vec::new(),
            tx,
            rx,
        };
        let jobs = self.jobs.get_or_insert_with(|| Pool::new(1));
        self.error = None;
        match (&self.engine, models::dir()) {
            (Some((m, engine)), _) if *m == run.model => run.transcribe(jobs, engine),
            (_, None) => return self.error = Some(tr!("subtitles.no_folder").into()),
            (_, Some(dir)) if run.model.is_downloaded(&dir) => run.load(jobs, run.model.path(&dir)),
            (_, Some(dir)) => {
                run.stage = Stage::Downloading;
                let tx = run.tx.clone();
                let job = run.model.download(jobs, &dir, move |r| {
                    let _ = tx.send(Msg::Downloaded(r));
                });
                run.handles = vec![job];
            }
        }
        self.run = Some(run);
    }

    /// Moves the run on with what its jobs sent; the edit once every transcript is in.
    fn poll(&mut self) -> Option<Done> {
        let Self {
            run: Some(run),
            jobs: Some(jobs),
            ..
        } = self
        else {
            return None;
        };
        // Taken before reading, as a job sends its result before it's marked finished.
        let (stage, finished) = (run.stage, run.handles.iter().map(Handle::is_finished));
        let finished: Vec<bool> = finished.collect();
        let failed = loop {
            match run.rx.try_recv() {
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => {
                    // A job that ended without a word (it panicked) would otherwise spin forever.
                    let lost = |(i, &f): (usize, &bool)| {
                        f && (stage != Stage::Transcribing || run.done[i].is_none())
                    };
                    if run.stage == stage && finished.iter().enumerate().any(lost) {
                        break self.failed(Error::Failed("crashed".into()));
                    }
                    return None;
                }
                Ok(Msg::Downloaded(Ok(path))) => {
                    self.downloaded.push(run.model);
                    run.load(jobs, path);
                }
                Ok(Msg::Loaded(Ok(engine))) => {
                    run.transcribe(jobs, &engine);
                    self.engine = Some((run.model, engine));
                }
                Ok(Msg::Transcribed(i, Ok(t))) => {
                    run.done[i] = Some(t);
                    if self.transcribe.is_some() {
                        if run.done.iter().all(Option::is_some) {
                            let done = run.done.iter_mut().map(|t| t.take().unwrap_or_default());
                            let done = run.sources.iter().cloned().zip(done).collect();
                            self.close();
                            return Some(Done::Transcribed(done));
                        }
                    } else if let Some(cues) = run.cues() {
                        if cues.is_empty() {
                            break tr!("subtitles.no_speech").to_owned();
                        }
                        let [.., (_, style)] = Text::presets();
                        let name = tr!("app.subtitles").to_owned();
                        self.close();
                        let c = Command::ImportSubtitles { name, cues, style };
                        return Some(Done::Edit(Box::new(c)));
                    }
                }
                Ok(Msg::Downloaded(Err(e)) | Msg::Loaded(Err(e)) | Msg::Transcribed(_, Err(e))) => {
                    break self.failed(e);
                }
            }
        };
        self.error = Some(failed);
        self.run = None;
        None
    }

    fn failed(&self, e: Error) -> String {
        match self.transcribe {
            Some(_) => tr!("transcript.failed", error = e),
            None => tr!("subtitles.failed", error = e),
        }
    }

    /// Shows the dialog while open; returns the result once it's ready.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        selected: &HashSet<ClipId>,
    ) -> Option<Done> {
        if !self.open {
            return None;
        }
        if let Some(c) = self.poll() {
            return Some(c);
        }
        if self.run.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        let can = match &self.transcribe {
            Some(sources) => !sources.is_empty(),
            None => Self::can_open(project, selected),
        };
        let (title, action) = match self.transcribe {
            Some(_) => (tr!("transcript.transcribe"), tr!("transcript.transcribe")),
            None => (tr!("subtitles.title"), tr!("subtitles.generate")),
        };
        let (mut start, mut close) = (false, false);
        let modal = egui::Modal::new(egui::Id::new("generate_subtitles"))
            .frame(
                egui::Frame::popup(ui.style())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(space::S16),
            )
            .show(ui.ctx(), |ui| {
                ui.set_width(360.0);
                // Menus stay inside the dialog, as it was last frame.
                let bounds = (ui.ctx())
                    .memory(|m| m.area_rect(egui::Id::new("generate_subtitles")))
                    .unwrap_or(ui.ctx().content_rect());
                ui.label(RichText::new(title).font(theme::headline()));
                ui.add_space(space::S8);
                let idle = if self.run.is_none() {
                    State::Live
                } else {
                    State::Disabled
                };
                ui.add_enabled_ui(self.run.is_none(), |ui| {
                    if let Some(sources) = &self.transcribe {
                        for source in sources {
                            let name = source
                                .path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy();
                            ui.label(tr!(
                                "analysis.source_stream",
                                name = name,
                                number = source.nth + 1
                            ));
                        }
                    } else {
                        self.sources.show(ui, project.sequence(), selected, false);
                    }
                });
                egui::Grid::new("subtitles_grid")
                    .spacing([space::S8, space::S8])
                    .show(ui, |ui| {
                        ui.label(tr!("subtitles.language"));
                        let names: Vec<&str> = std::iter::once(tr!("subtitles.auto"))
                            .chain(LANGUAGES.iter().map(|l| i18n::autonym(l)))
                            .collect();
                        let d = Dropdown::new(&mut self.language, &names);
                        ui.add(d.width(FIELD).bounds(bounds).state(idle));
                        ui.end_row();

                        ui.label(tr!("subtitles.model"));
                        let accurate = match self.translate {
                            true => Model::Small,
                            false => Model::LargeV3Turbo,
                        };
                        let names = [
                            (Model::Base, tr!("subtitles.fast")),
                            (accurate, tr!("subtitles.accurate")),
                        ]
                        .map(|(m, name)| {
                            match self.downloaded.contains(&m) {
                                true => name.to_owned(),
                                false => {
                                    let size = (m.size() as f64 / 1e6).round().to_string();
                                    tr!("subtitles.with_size", name = name, size = size)
                                }
                            }
                        });
                        let names = names.each_ref().map(String::as_str);
                        let d = Dropdown::new(&mut self.quality, &names);
                        ui.add(d.width(FIELD).bounds(bounds).state(idle));
                        ui.end_row();

                        if self.transcribe.is_none() {
                            ui.label(verbatim(""));
                            let translate = tr!("subtitles.translate");
                            ui.add(Checkbox::new(&mut self.translate, translate).state(idle));
                            ui.end_row();
                        }
                    });
                if let Some(run) = &self.run {
                    ui.add_space(space::S16);
                    let status = match run.stage {
                        Stage::Downloading => tr!("subtitles.downloading"),
                        Stage::Loading => tr!("subtitles.loading"),
                        Stage::Transcribing => tr!("subtitles.transcribing"),
                    };
                    let progress = run.progress();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(status).font(theme::body_strong()));
                        if let Some(p) = progress {
                            let percent = format!("{:.0}", p * 100.0);
                            let percent = tr!("subtitles.percent", percent = percent);
                            let rtl = egui::Layout::right_to_left(egui::Align::Center);
                            ui.with_layout(rtl, |ui| {
                                ui.label(RichText::new(percent).color(color::ink_2()))
                            });
                        }
                    });
                    ui.add_space(space::S4);
                    let w = ui.available_width();
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(w, 6.0), egui::Sense::hover());
                    crate::export::queue::bar(ui, rect, progress.unwrap_or(0.0));
                } else if let Some(e) = &self.error {
                    ui.add_space(space::S8);
                    ui.label(RichText::new(e).color(color::red()));
                }
                ui.add_space(space::S16);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = space::S8;
                    let ready = if can && self.run.is_none() {
                        State::Live
                    } else {
                        State::Disabled
                    };
                    let generate = Button::primary(action)
                        .loading(self.run.is_some())
                        .state(ready);
                    start = ui.add(generate).clicked();
                    close = ui.add(Button::secondary(tr!("prompt.cancel"))).clicked();
                });
            });
        if close || modal.should_close() {
            self.close();
        } else if start {
            self.start(project, selected);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use jvid_core::history::{Created, History};
    use jvid_core::model::{NewClip, ProbeInfo};

    use super::*;

    fn word(text: &str, start: f64, end: f64) -> Word {
        let t = |s: f64| Time((s * S as f64) as i64);
        Word {
            text: text.into(),
            start: t(start),
            end: t(end),
            confidence: 1.0,
        }
    }

    /// `text` spoken without a break, a word every `step` seconds from `at`.
    fn spoken(text: &str, at: f64, step: f64) -> Vec<Word> {
        let words = text.split(' ').enumerate();
        words
            .map(|(i, w)| word(w, at + i as f64 * step, at + (i + 1) as f64 * step))
            .collect()
    }

    fn texts(cues: &[Cue]) -> Vec<&str> {
        cues.iter().map(|c| c.text.as_str()).collect()
    }

    struct Silent;

    impl Transcriber for Silent {
        fn transcribe(
            &self,
            _: Vec<f32>,
            _: &Options,
            _: &jvid_jobs::Ctx,
        ) -> Result<Transcript, Error> {
            Ok(Transcript::default())
        }
    }

    /// Transcribe for Edit by Text keeps the fillers, in the chosen language; subtitles don't.
    #[test]
    fn transcribe_prompts_for_fillers_and_subtitles_do_not() {
        let mut s = Subtitles {
            engine: Some((Model::Base, Arc::new(Silent))),
            ..Subtitles::default()
        };
        let source = Source {
            path: PathBuf::from("missing.mp4"),
            nth: 0,
            range: 0..i64::MAX,
        };
        s.open_transcribe(vec![source]);
        s.language = 1 + LANGUAGES.iter().position(|&l| l == "de").unwrap();
        let none = HashSet::new();
        s.start(&Project::default(), &none);
        let prompt = s.run.as_ref().unwrap().opts.initial_prompt.clone();
        assert_eq!(
            prompt.as_deref(),
            Some(crate::transcript::fillers::prompt("de"))
        );
        s.open();
        s.start(&Project::default(), &none);
        assert!(s.run.as_ref().unwrap().opts.initial_prompt.is_none());
    }

    /// Acceptance: lines of at most 42 characters, at most two, at most about 7 s, breaking
    /// at punctuation and pauses.
    #[test]
    fn a_fake_transcript_becomes_short_caption_lines() {
        let mut words = spoken(
            "So today we're going to look at the new export dialog, which finally lets you \
             queue several renders at once and keep editing while they run in the background.",
            0.0,
            0.25,
        );
        // A pause, then a short sentence and another straight after it.
        words.extend(spoken("Let's start. Open the project.", 9.0, 0.25));
        let cues = cues(&words);
        for c in &cues {
            let lines: Vec<&str> = c.text.lines().collect();
            assert!(lines.len() <= 2, "{:?}", c.text);
            assert!(
                lines.iter().all(|l| l.chars().count() <= LINE),
                "{:?}",
                c.text
            );
            assert!(c.end - c.start <= MAX_LEN + MIN_LEN, "{c:?}");
            assert!(c.start < c.end);
        }
        assert_eq!(
            texts(&cues),
            [
                "So today we're going to look\nat the new export dialog,",
                "which finally lets you queue several\nrenders at once and keep editing while",
                "they run in the background.",
                "Let's start.",
                "Open the project.",
            ]
        );
        // Nothing overlaps, and every word is kept, in order.
        assert!(cues.windows(2).all(|w| w[0].end <= w[1].start));
        let all: Vec<String> = cues.iter().map(|c| c.text.replace('\n', " ")).collect();
        let spoken: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(all.join(" "), spoken.join(" "));
    }

    #[test]
    fn pauses_and_length_end_a_caption_and_short_ones_stay_up_a_second() {
        // "Hi" then 0.6 s of quiet.
        let mut words = vec![word("Hi", 0.0, 0.2), word("there", 0.8, 1.0)];
        let cues_ = cues(&words);
        assert_eq!(texts(&cues_), ["Hi", "there"]);
        // Held to the next word, not a full second.
        assert_eq!(cues_[0].end, words[1].start);
        assert_eq!(cues_[1].end, words[1].start + MIN_LEN);
        // Slow speech: 7 s fills before two lines do.
        words = spoken("one two three four five six seven eight nine ten", 0.0, 0.9);
        let cues_ = cues(&words);
        assert_eq!(
            texts(&cues_),
            ["one two three four five six seven", "eight nine ten"]
        );
        assert!(cues_[0].end - cues_[0].start <= MAX_LEN);
        assert!(cues(&[]).is_empty());
    }

    #[test]
    fn lines_balance_and_long_words_stand_alone() {
        assert_eq!(lines(&["a", "b"]).as_deref(), Some("a b"));
        let w: Vec<&str> = "aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii"
            .split(' ')
            .collect();
        assert_eq!(
            lines(&w).as_deref(),
            Some("aaaa bbbb cccc dddd\neeee ffff gggg hhhh iiii")
        );
        let long = "長い文章".repeat(12);
        assert_eq!(lines(&[&long]), None);
        let c = cues(&[word(&long, 0.0, 3.0), word("next", 3.0, 3.2)]);
        assert_eq!(texts(&c), [long.as_str(), "next"]);
        assert!(ends_sentence("done.\"") && ends_sentence("終わり。") && !ends_sentence("Mr"));
        let codes = LANGUAGES.map(whisper_code);
        assert_eq!(
            codes[5..],
            [
                "pt", "nl", "pl", "cs", "tr", "ru", "uk", "sv", "da", "no", "ca", "ja", "zh", "ko"
            ]
        );
    }

    /// One audio clip of 10–20 s of a file, at 2 s on the timeline, and a video clip.
    fn project() -> (History, ClipId) {
        let mut h = History::new(Project::default());
        let probe = ProbeInfo {
            duration: Time(60 * S),
            audio_streams: 1,
            ..Default::default()
        };
        h.apply(Command::Import(vec![(PathBuf::from("/m/talk.m4a"), probe)]))
            .unwrap();
        let asset = h.project().assets()[0].id;
        let mut clip = None;
        for kind in [TrackKind::Video, TrackKind::Audio] {
            let name = String::new();
            let Ok(Created::Track(track)) = h.apply(Command::AddTrack { kind, name }) else {
                panic!()
            };
            let new = NewClip {
                asset,
                source_in: Time(10 * S),
                source_out: Time(20 * S),
                start: Time(2 * S),
                link: None,
            };
            if let Ok(Created::Clip(id)) = h.apply(Command::AddClip { track, clip: new }) {
                clip = Some(id);
            }
        }
        (h, clip.unwrap())
    }

    #[test]
    fn audit_442_selected_microphone_beats_linked_creation_order() {
        let (mut h, first) = project();
        let Created::Track(track) = h
            .apply(Command::AddTrack {
                kind: TrackKind::Audio,
                name: "Mic".into(),
            })
            .unwrap()
        else {
            panic!()
        };
        let asset = h.project().assets()[0].id;
        let Created::Clip(mic) = h
            .apply(Command::AddClip {
                track,
                clip: NewClip {
                    asset,
                    source_in: Time(10 * S),
                    source_out: Time(20 * S),
                    start: Time(2 * S),
                    link: Some(jvid_core::model::LinkId(7)),
                },
            })
            .unwrap()
        else {
            panic!()
        };
        h.apply(Command::SetClipLink {
            id: first,
            link: Some(jvid_core::model::LinkId(7)),
        })
        .unwrap();
        let result = targets(h.project(), &HashSet::from([mic]), false);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].clip.id, mic);
    }

    #[test]
    fn selected_audio_or_in_to_out_is_transcribed_and_placed_on_the_timeline() {
        let (mut h, audio) = project();
        let none = HashSet::new();
        assert!(targets(h.project(), &none, false).is_empty());
        let video = h.project().sequence().tracks()[0].clips()[0].id;
        // A video clip alone has no audio to read.
        assert!(targets(h.project(), &HashSet::from([video]), false).is_empty());

        let [t] = &targets(h.project(), &HashSet::from([audio]), false)[..] else {
            panic!()
        };
        let rate = SAMPLE_RATE as i64;
        assert_eq!(t.source.range, 10 * rate..20 * rate);
        assert_eq!(t.span, (Time(2 * S), Time(12 * S)));
        // Words come back in file time; 11 s of the file plays at 3 s.
        let heard = Transcript {
            language: Some("en".into()),
            words: vec![word("before", 9.0, 9.5), word("hello", 11.0, 11.5)],
        };
        let placed: Vec<Word> = t.place(&heard.words).collect();
        assert_eq!(placed, [word("hello", 3.0, 3.5)]);

        h.apply(Command::SetInOut {
            in_point: Some(Time(5 * S)),
            out_point: Some(Time(8 * S)),
        })
        .unwrap();
        let [t] = &targets(h.project(), &none, false)[..] else {
            panic!()
        };
        assert_eq!(t.source.range, 13 * rate..16 * rate);
        assert_eq!(t.span, (Time(5 * S), Time(8 * S)));
        assert!(t.place(&heard.words).next().is_none());
        // The whole sequence, for the Transcript panel.
        h.apply(Command::SetInOut {
            in_point: None,
            out_point: None,
        })
        .unwrap();
        let [t] = &targets(h.project(), &none, true)[..] else {
            panic!()
        };
        assert_eq!(t.span, (Time(2 * S), Time(12 * S)));
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use jvid_core::history::{Command, History};
    use jvid_core::model::{Placement, ProbeInfo};

    pub(crate) fn two_streams(mic_first: bool) -> (Project, ClipId, ClipId) {
        let mut h = History::new(Project::default());
        h.apply(Command::Import(vec![(
            PathBuf::from("/fake/two-streams.mkv"),
            ProbeInfo {
                duration: Time(6 * S),
                width: Some(640),
                height: Some(360),
                audio_streams: 2,
                ..Default::default()
            },
        )]))
        .unwrap();
        let mut streams = vec![(Some(0), "Game".into()), (Some(1), "Mic".into())];
        if mic_first {
            streams.reverse();
        }
        h.apply(Command::Place(Placement {
            asset: h.project().assets()[0].id,
            source_in: Time(S),
            source_out: Time(5 * S),
            start: Time(2 * S),
            video: None,
            audio: None,
            streams,
            add_video: true,
            add_audio: true,
            insert: false,
        }))
        .unwrap();
        let audio = |nth| {
            h.project()
                .sequence()
                .tracks()
                .iter()
                .filter(|t| t.kind == TrackKind::Audio)
                .flat_map(|t| t.clips())
                .find(|c| c.stream == Some(nth))
                .unwrap()
                .id
        };
        (h.project().clone(), audio(1), audio(0))
    }
}
