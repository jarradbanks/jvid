//! Edit by Text: the Transcript panel. It shows the words spoken in the selected clips, or in
//! the whole sequence, from each file's transcript cached in the app's cache folder (never in
//! the project). Words are placed through the clips every frame, so they follow timeline edits
//! and undo. Clicking a word seeks to it, the word under the playhead is lit, Find picks out
//! matches, and Delete ripple deletes the selected words from their clips and linked clips as
//! one undo step. Fillers lists the um, uh and long pauses to cut the same way ([`fillers`]).

pub mod fillers;
pub(crate) mod text;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

#[cfg(test)]
use std::fs;
use std::rc::Rc;

use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{Color32, Key, Margin, Sense, Ui, vec2};
use jvid_core::history::Command;
use jvid_core::model::{ClipId, Project};
use jvid_core::time::{FLICKS_PER_SECOND as S, Time};
#[cfg(test)]
use jvid_transcribe::Transcript;
use jvid_transcribe::store::{Settings, Shared, Store};
use jvid_transcribe::{Source, Word};
use suite_ui::panel::Panel;
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{Button, Checkbox, State, TextInput};

use crate::i18n::tr;
use crate::subtitles::{AudioSources, targets};

/// A pause this long between words starts a new paragraph.
const PARAGRAPH: Time = Time(S);
/// Line height of the words, roomier than Body's for reading.
const LINE: f32 = 20.0;

/// The Transcript view of the shared source-time recognition store. Selection and placed-word
/// APIs stay local to this panel; caption edits never alter the store.
pub struct Cache {
    pub(crate) store: Shared,
    settings: Settings,
    heard: HashSet<(PathBuf, usize)>,
    jobs: Option<jvid_jobs::Pool>,
    verifying: HashMap<PathBuf, (u64, jvid_jobs::Handle)>,
    read_revision: u64,
    placed: Option<(Project, HashSet<ClipId>, u64, Rc<Placed>)>,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            store: Store::default().shared(),
            settings: crate::subtitles::Subtitles::default().transcript_settings(),
            heard: HashSet::new(),
            jobs: None,
            verifying: HashMap::new(),
            read_revision: 0,
            placed: None,
        }
    }
}

/// The words in timeline time, missing source intervals, and detected language per clip.
type Languages = HashMap<ClipId, Option<String>>;
pub(crate) type Placed = (Vec<(ClipId, Word)>, Vec<Source>, Languages);

impl Cache {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            store: Store::new(dir).shared(),
            ..Self::default()
        }
    }

    pub(crate) fn set_settings(&mut self, settings: Settings) {
        if self.settings != settings {
            self.settings = settings;
            self.placed = None;
            self.heard.clear();
        }
    }

    #[cfg(test)]
    pub fn get(&mut self, path: &Path, nth: usize) -> Option<Transcript> {
        let mut store = self.store.lock().unwrap();
        let request = store
            .request(
                &Source {
                    path: path.into(),
                    nth,
                    range: 0..i64::MAX,
                },
                &self.settings,
            )
            .ok()?;
        let found = store.lookup(&request);
        found.missing.is_empty().then_some(found.transcript)
    }

    /// Test fixtures insert recognized source words through the same persistent store.
    #[cfg(test)]
    pub fn insert(&mut self, source: &Source, transcript: Transcript) {
        let mut store = self.store.lock().unwrap();
        let request = store.request(source, &self.settings).unwrap();
        store.insert(&request, transcript).unwrap();
        self.placed = None;
    }

    /// Hash source content on a cancellable job before reading persistent words. The UI
    /// never waits for another tool's verification lock or reads a large media file itself.
    fn prepare(&mut self, project: &Project, selected: &HashSet<ClipId>) -> Option<u64> {
        let sources = targets(project, selected, true);
        let Ok(mut store) = self.store.try_lock() else {
            return None;
        };
        for source in &sources {
            store.observe(&source.source.path);
        }
        let paths: HashSet<_> = sources
            .iter()
            .map(|t| t.source.path.clone())
            .filter(|p| p.is_file() && !store.has_identity(p))
            .collect();
        let revision = store.revision();
        drop(store);
        if paths.is_empty() {
            return Some(revision);
        }
        for path in paths {
            if self
                .verifying
                .get(&path)
                .is_some_and(|(r, handle)| *r == revision || !handle.is_finished())
            {
                continue;
            }
            let shared = self.store.clone();
            let file = path.clone();
            let handle = self
                .jobs
                .get_or_insert_with(|| jvid_jobs::Pool::new(1))
                .submit(
                    format!("recognition:identity:{}:{revision}", path.display()),
                    jvid_jobs::Priority::Interactive,
                    move |ctx| {
                        if let Ok(identity) = Store::identify(&file, &|| ctx.is_cancelled()) {
                            let _ = shared.lock().unwrap().accept(identity);
                        }
                    },
                );
            self.verifying.insert(path, (revision, handle));
        }
        None
    }

    /// Memoize placement while observing source changes even when the project stays unchanged.
    pub(crate) fn placed(&mut self, project: &Project, selected: &HashSet<ClipId>) -> Rc<Placed> {
        let Some(revision) = self.prepare(project, selected) else {
            self.placed = None;
            self.heard.clear();
            return Rc::new((
                Vec::new(),
                jvid_transcribe::store::merge_sources(
                    targets(project, selected, true)
                        .into_iter()
                        .map(|t| t.source)
                        .collect(),
                ),
                Languages::new(),
            ));
        };
        match &self.placed {
            Some((p, s, r, w)) if p == project && s == selected && *r == revision => w.clone(),
            _ => {
                let w = Rc::new(words(project, selected, self));
                let revision = self.read_revision;
                self.placed = Some((project.clone(), selected.clone(), revision, w.clone()));
                w
            }
        }
    }
}

/// Cached partial words are visible immediately; only genuinely missing intervals are asked
/// for. A covered silent interval has neither words nor missing coverage.
fn words(project: &Project, selected: &HashSet<ClipId>, cache: &mut Cache) -> Placed {
    let (mut words, mut missing) = (Vec::new(), Vec::new());
    let mut languages = Languages::new();
    cache.heard.clear();
    let mut store = cache.store.lock().unwrap();
    for t in targets(project, selected, true) {
        match store.request_known(&t.source, &cache.settings) {
            Ok(request) => {
                let found = store.lookup(&request);
                if found.missing.is_empty() {
                    cache.heard.insert((t.source.path.clone(), t.source.nth));
                }
                languages.insert(t.clip.id, found.transcript.language.clone());
                words.extend(t.place(&found.transcript.words).map(|w| (t.clip.id, w)));
                missing.extend(found.missing);
            }
            Err(_) => missing.push(t.source),
        }
    }
    words.sort_by_key(|(c, w)| (w.start, c.0));
    cache.read_revision = store.revision();
    (
        words,
        jvid_transcribe::store::merge_sources(missing),
        languages,
    )
}

/// An unrecognized gap is not a detected pause. Keep filler words available while refusing
/// automatic pause cuts across any missing source coverage.
fn covered_hits(
    words: &[(ClipId, Word)],
    languages: &Languages,
    missing: &[Source],
    project: &Project,
    selected: &HashSet<ClipId>,
) -> Vec<fillers::Hit> {
    let mut hits = fillers::find(words, languages);
    let rate = jvid_core::time::Rate::new(jvid_media::audio::SAMPLE_RATE, 1);
    for target in targets(project, selected, true) {
        for source in missing
            .iter()
            .filter(|s| s.path == target.source.path && s.nth == target.source.nth)
        {
            let a = target.clip.timeline_at(rate.time_floor(source.range.start));
            let b = target.clip.timeline_at(rate.time_floor(source.range.end));
            hits.retain(|h| !h.pause || h.clip != target.clip.id || h.end <= a || h.start >= b);
        }
    }
    hits
}

/// Ripple deletes `words[picked]`: each run of neighbouring words in one clip is one span,
/// the pauses between them included.
fn cut(words: &[(ClipId, Word)], picked: Range<usize>) -> Option<Command> {
    let mut spans: Vec<(ClipId, Time, Time)> = Vec::new();
    for (c, w) in &words[picked] {
        match spans.last_mut() {
            Some((last, _, end)) if last == c => *end = (*end).max(w.end),
            _ => spans.push((*c, w.start, w.end)),
        }
    }
    (!spans.is_empty()).then_some(Command::CutSpans { spans, speed: None })
}

/// What the panel asks of the app this frame.
#[derive(Default)]
pub struct Output {
    pub seek: Option<Time>,
    pub command: Option<Command>,
    /// Transcribe was clicked: this source scope can be reused, completed or recognized again.
    pub transcribe: Option<Vec<Source>>,
    /// Snapshot of the current displayed scope to save as plain text.
    pub export: Option<String>,
    pub close: bool,
}

#[derive(Default)]
pub struct TranscriptPanel {
    sources: AudioSources,
    pub cache: Cache,
    pub(crate) query: String,
    /// The first and last words picked, by start time, so it outlives edits elsewhere.
    selection: Option<(Time, Time)>,
    /// The start of the word last lit, to scroll to the next one.
    lit: Option<Time>,
    /// While Fillers lists the hits: the starts of those unticked.
    unticked: Option<HashSet<Time>>,
}

impl TranscriptPanel {
    /// Called before app key routing. Find retains its native clipboard behavior;
    /// Copy in the focused transcript belongs only to its selected words.
    pub(crate) fn copy_shortcut(
        &mut self,
        ctx: &egui::Context,
        project: &Project,
        selected: &HashSet<ClipId>,
    ) {
        if ctx.text_edit_focused() {
            return;
        }
        let copy = ctx.input_mut(|input| {
            let event = input.events.iter().any(|e| matches!(e, egui::Event::Copy));
            input.events.retain(|e| !matches!(e, egui::Event::Copy));
            input.consume_key(egui::Modifiers::COMMAND, Key::C) || event
        });
        if copy {
            let selected = self.sources.selection(project.sequence(), selected);
            let placed = self.cache.placed(project, &selected);
            if let Some(picked) = self.picked(&placed.0) {
                ctx.copy_text(text::plain(&placed.0[picked]));
            }
        }
    }

    fn text_actions(&self, ui: &mut Ui, words: &[(ClipId, Word)], out: &mut Output) {
        let ready = |yes| if yes { State::Live } else { State::Disabled };
        if ui
            .add(Button::secondary(tr!("action.copy")).state(ready(self.picked(words).is_some())))
            .clicked()
            && let Some(picked) = self.picked(words)
        {
            ui.ctx().copy_text(text::plain(&words[picked]));
        }
        if ui
            .add(Button::secondary(tr!("transcript.copy_all")).state(ready(!words.is_empty())))
            .clicked()
        {
            ui.ctx().copy_text(text::plain(words));
        }
        if ui
            .add(Button::secondary(tr!("transcript.export_text")).state(ready(!words.is_empty())))
            .clicked()
        {
            out.export = Some(text::plain(words));
        }
    }

    /// Source-time recognition is reusable; word selections and searches are not.
    pub fn reset_session(&mut self) {
        self.sources = AudioSources::default();
        self.query.clear();
        self.selection = None;
        self.lit = None;
        self.unticked = None;
    }

    /// The picked words, as a range of `words`.
    fn picked(&self, words: &[(ClipId, Word)]) -> Option<Range<usize>> {
        let (a, b) = self.selection?;
        let (a, b) = (a.min(b), a.max(b));
        let from = words.partition_point(|(_, w)| w.start < a);
        let to = words.partition_point(|(_, w)| w.start <= b);
        (from < to).then_some(from..to)
    }

    #[cfg(test)]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        selected: &HashSet<ClipId>,
        playhead: Time,
        focused: bool,
    ) -> Output {
        self.show_inner(ui, project, selected, playhead, focused, false)
    }

    pub(crate) fn show_docked(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        selected: &HashSet<ClipId>,
        playhead: Time,
        focused: bool,
    ) -> Output {
        self.show_inner(ui, project, selected, playhead, focused, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn show_inner(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        selected: &HashSet<ClipId>,
        playhead: Time,
        focused: bool,
        docked: bool,
    ) -> Output {
        if self.cache.jobs.as_ref().is_some_and(|p| !p.is_idle()) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        let selected_input = selected;
        let mut out = Output::default();
        // Before Find draws, which may let go of the focus this frame.
        let typing = ui.ctx().text_edit_focused();
        let selected = self.sources.selection(project.sequence(), selected);
        let placed = self.cache.placed(project, &selected);
        if self.cache.jobs.as_ref().is_some_and(|p| !p.is_idle()) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        let (words, missing, languages) = &*placed;
        let hits = self
            .unticked
            .as_ref()
            .map(|_| covered_hits(words, languages, missing, project, &selected));
        let close = [(icon::X, tr!("transcript.close"))];
        let panel = if docked {
            Panel::body().embedded(true)
        } else {
            Panel::titled(tr!("panel.transcript")).actions(&close)
        };
        let r = panel.focused(focused).show(ui, |ui| {
            if self
                .sources
                .show(ui, project.sequence(), selected_input, false)
            {
                self.selection = None;
                self.unticked = None;
                ui.ctx().request_repaint();
                return;
            }
            let bar = egui::Frame::NONE
                .inner_margin(Margin::symmetric(space::INSET as i8, space::S8 as i8));
            bar.show(ui, |ui| {
                ui.horizontal_wrapped(|ui| self.text_actions(ui, words, &mut out));
                let scope = if selected_input.is_empty() {
                    tr!("export.entire")
                } else {
                    tr!("export.selected")
                };
                ui.label(
                    egui::RichText::new(tr!("transcript.scope", scope = scope))
                        .color(color::ink_3()),
                );
                if !missing.is_empty() {
                    ui.label(egui::RichText::new(tr!("transcript.partial")).color(color::ink_3()));
                }
                // Right to left, so Find takes what Transcribe leaves.
                let rtl = egui::Layout::right_to_left(egui::Align::Center);
                let row = vec2(ui.available_width(), size::CONTROL);
                ui.allocate_ui_with_layout(row, rtl, |ui| {
                    ui.spacing_mut().item_spacing.x = space::S8;
                    let listing = self.unticked.is_some();
                    let button = Button::secondary(tr!("transcript.fillers")).on(listing);
                    if ui.add(button).clicked() {
                        self.unticked = (!listing).then(HashSet::new);
                    }
                    let button = Button::secondary(tr!("transcript.transcribe"));
                    if ui.add(button).clicked() {
                        out.transcribe = Some(
                            targets(project, &selected, true)
                                .into_iter()
                                .map(|t| t.source)
                                .collect(),
                        );
                    }
                    if let Some(unticked) = &self.unticked {
                        let hits = hits.as_deref().unwrap_or_default();
                        let ready = match hits.is_empty() {
                            true => State::Disabled,
                            false => State::Live,
                        };
                        let remove = Button::primary(tr!("transcript.remove"));
                        let remove = ui.add(remove.state(ready));
                        let all = Button::secondary(tr!("transcript.remove_all"));
                        let all = ui.add(all.state(ready));
                        if all.clicked() {
                            out.command = fillers::cut(hits, &HashSet::new());
                        } else if remove.clicked() {
                            out.command = fillers::cut(hits, unticked);
                        }
                        if remove.clicked() || all.clicked() {
                            self.unticked = None;
                        }
                        return;
                    }
                    let find = TextInput::search(&mut self.query).hint(tr!("transcript.find"));
                    let find = ui.add(find.width(ui.available_width()));
                    if find.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        self.find_next(words, playhead, &mut out);
                        // Keeps typing in Find, so Enter goes on to the next match.
                        find.request_focus();
                    }
                });
            });
            if missing.is_empty() && words.is_empty() {
                ui.label(egui::RichText::new(tr!("subtitles.no_speech")).color(color::ink_3()));
            }
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    let frame = egui::Frame::NONE
                        .inner_margin(Margin::symmetric(space::INSET as i8, space::S4 as i8));
                    frame.show(ui, |ui| match self.unticked.is_some() {
                        true => self.hits(ui, words, hits.as_deref().unwrap_or_default(), &mut out),
                        false => self.text(ui, words, playhead, &mut out),
                    });
                });
        });
        out.close = r.action == Some(0);
        if focused && !typing && !ui.ctx().text_edit_focused() {
            let delete = ui.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace));
            if let Some(picked) = self.picked(words).filter(|_| delete) {
                out.command = cut(words, picked);
                self.selection = None;
            }
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                self.selection = None;
            }
        }
        out
    }

    /// Fillers' list: a row per hit with its context, ticked unless unticked; ticking one
    /// seeks to it.
    fn hits(
        &mut self,
        ui: &mut Ui,
        words: &[(ClipId, Word)],
        hits: &[fillers::Hit],
        out: &mut Output,
    ) {
        let Some(unticked) = &mut self.unticked else {
            return;
        };
        if hits.is_empty() {
            ui.label(egui::RichText::new(tr!("transcript.none")).color(color::ink_3()));
        }
        for h in hits {
            let seconds = format!("{:.1}", (h.end - h.start).0 as f64 / S as f64);
            let pause = tr!("transcript.pause", seconds = seconds);
            let label = fillers::context(words, h, &pause);
            let mut on = !unticked.contains(&h.start);
            if ui.add(Checkbox::new(&mut on, &label)).clicked() {
                match on {
                    true => unticked.remove(&h.start),
                    false => unticked.insert(h.start),
                };
                out.seek = Some(h.start);
            }
        }
    }

    /// The streams Transcribe would transcribe, for a lesson's Show Me.
    pub(crate) fn missing(&mut self, project: &Project, selected: &HashSet<ClipId>) -> Vec<Source> {
        let selected = self.sources.selection(project.sequence(), selected);
        self.cache.placed(project, &selected).1.clone()
    }

    /// Whether displayed intervals of audio stream `nth` of `path` have recognition loaded.
    pub(crate) fn heard(&self, path: &Path, nth: usize) -> bool {
        self.cache.heard.contains(&(path.to_owned(), nth))
    }

    /// What Delete does with words `picked` (clamped to the words there are), for a lesson's
    /// Show Me.
    pub(crate) fn cut(
        &mut self,
        project: &Project,
        selected: &HashSet<ClipId>,
        picked: Range<usize>,
    ) -> Option<Command> {
        let selected = self.sources.selection(project.sequence(), selected);
        let placed = self.cache.placed(project, &selected);
        let n = placed.0.len();
        cut(&placed.0, picked.start.min(n)..picked.end.min(n))
    }

    /// Enter in Find: picks and seeks to the next match after the playhead, from the top
    /// after the last.
    fn find_next(&mut self, words: &[(ClipId, Word)], playhead: Time, out: &mut Output) {
        let query = self.query.trim().to_lowercase();
        if query.is_empty() {
            return;
        }
        let matches = |w: &&(ClipId, Word)| w.1.text.to_lowercase().contains(&query);
        let mut found = words.iter().filter(matches);
        let next = found.clone().find(|(_, w)| w.start > playhead);
        if let Some((_, w)) = next.or_else(|| found.next()) {
            self.selection = Some((w.start, w.start));
            out.seek = Some(w.start);
        }
    }

    /// The words as one wrapped text, picked words on Blue, the playhead's in Light Blue and,
    /// while Find has text, the others dimmed.
    fn text(&mut self, ui: &mut Ui, words: &[(ClipId, Word)], playhead: Time, out: &mut Output) {
        let query = self.query.trim().to_lowercase();
        let picked = self.picked(words).unwrap_or_default();
        let lit = words
            .iter()
            .position(|(_, w)| w.start <= playhead && playhead < w.end);
        let style = |on: bool, lit: bool, dim: bool| TextFormat {
            font_id: theme::body(),
            line_height: Some(LINE),
            color: match (lit, dim) {
                (true, _) => color::light_blue(),
                (_, true) => color::ink_3(),
                _ => color::ink(),
            },
            background: if on {
                color::blue().gamma_multiply(0.2)
            } else {
                Color32::TRANSPARENT
            },
            ..TextFormat::default()
        };
        let mut job = LayoutJob::default();
        job.wrap.max_width = ui.available_width();
        // Where each word starts, in characters.
        let (mut starts, mut chars) = (Vec::with_capacity(words.len()), 0);
        for (i, (_, w)) in words.iter().enumerate() {
            let on = picked.contains(&i);
            if let Some((_, prev)) = i.checked_sub(1).map(|p| &words[p]) {
                let gap = if w.start - prev.end >= PARAGRAPH {
                    "\n\n"
                } else {
                    " "
                };
                job.append(
                    gap,
                    0.0,
                    style(on && picked.contains(&(i - 1)), false, false),
                );
                chars += gap.chars().count();
            }
            starts.push(chars);
            let dim = !query.is_empty() && !w.text.to_lowercase().contains(&query);
            job.append(&w.text, 0.0, style(on, lit == Some(i), dim));
            chars += w.text.chars().count();
        }
        let galley = ui.painter().layout_job(job);
        let size = vec2(ui.available_width(), galley.size().y);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let label = || egui::WidgetInfo::labeled(egui::WidgetType::Label, true, galley.text());
        response.widget_info(label);
        let at = |pos: egui::Pos2| {
            let c = galley.cursor_from_pos(pos - rect.min).index.0;
            let i = starts.partition_point(|&s| s <= c).checked_sub(1)?;
            Some(words[i].1.start)
        };
        let shift = ui.input(|i| i.modifiers.shift);
        let pressed = if response.drag_started() {
            ui.input(|i| i.pointer.press_origin())
        } else {
            response
                .interact_pointer_pos()
                .filter(|_| response.clicked())
        };
        if let Some(t) = pressed.and_then(at) {
            match self.selection {
                Some((anchor, _)) if shift => self.selection = Some((anchor, t)),
                _ => {
                    self.selection = Some((t, t));
                    out.seek = Some(t);
                }
            }
        } else if response.dragged()
            && let (Some(t), Some((anchor, _))) =
                (response.interact_pointer_pos().and_then(at), self.selection)
        {
            self.selection = Some((anchor, t));
        }
        // Follows the playhead as it moves on to another word.
        let lit_at = lit.map(|i| words[i].1.start);
        if lit_at != self.lit {
            self.lit = lit_at;
            if let Some(i) = lit.filter(|_| !response.is_pointer_button_down_on()) {
                let r = galley.pos_from_cursor(CCursor::new(starts[i]));
                ui.scroll_to_rect(r.translate(rect.min.to_vec2()).expand(LINE), None);
            }
        }
        ui.painter().galley(rect.min, galley, color::ink());
        response.context_menu(|ui| self.text_actions(ui, words, out));
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use jvid_core::history::History;
    use jvid_core::model::{NewClip, ProbeInfo, TrackKind};

    use super::*;

    fn t(s: f64) -> Time {
        Time((s * S as f64) as i64)
    }

    fn word(text: &str, start: f64, end: f64) -> Word {
        Word {
            text: text.into(),
            start: t(start),
            end: t(end),
            confidence: 1.0,
        }
    }

    /// A file's video and audio, linked, 10–20 s of it at 2 s on the timeline, with a fake
    /// transcript of the file cached: a word a second from 10 s.
    pub(super) fn project(dir: &Path) -> (History, TranscriptPanel) {
        let path = dir.join("talk.mp4");
        fs::write(&path, b"stand-in").unwrap();
        let mut h = History::new(Project::default());
        let probe = ProbeInfo {
            duration: Time(60 * S),
            width: Some(1920),
            height: Some(1080),
            audio_streams: 1,
            ..Default::default()
        };
        h.apply(Command::Import(vec![(path.clone(), probe)]))
            .unwrap();
        let asset = h.project().assets()[0].id;
        let link = Some(jvid_core::model::LinkId(1));
        for kind in [TrackKind::Video, TrackKind::Audio] {
            let name = String::new();
            let Ok(jvid_core::history::Created::Track(track)) =
                h.apply(Command::AddTrack { kind, name })
            else {
                panic!()
            };
            let clip = NewClip {
                asset,
                source_in: t(10.0),
                source_out: t(20.0),
                start: t(2.0),
                link,
            };
            h.apply(Command::AddClip { track, clip }).unwrap();
        }
        let mut panel = TranscriptPanel {
            cache: Cache::new(dir.join("cache")),
            ..Default::default()
        };
        let spoken = "zero one two three four five six seven eight nine";
        let words = spoken.split(' ').enumerate();
        let words = words.map(|(i, w)| word(w, 10.0 + i as f64, 10.6 + i as f64));
        let source = Source {
            path,
            nth: 0,
            range: 0..i64::MAX,
        };
        let heard = Transcript {
            language: Some("en".into()),
            words: words.collect(),
        };
        panel.cache.insert(&source, heard);
        (h, panel)
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jvid-transcript-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn texts(words: &[(ClipId, Word)]) -> Vec<&str> {
        words.iter().map(|(_, w)| w.text.as_str()).collect()
    }

    #[test]
    fn copy_shortcut_claims_native_and_key_events_and_only_copies_picked_timeline_words() {
        let dir = tmp("copy-shortcut");
        let (history, mut panel) = project(&dir);
        let ctx = egui::Context::default();
        let events = [
            egui::Event::Copy,
            egui::Event::Key {
                key: Key::C,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            },
        ];
        let selected = HashSet::new();
        let placed = panel.cache.placed(history.project(), &selected);
        panel.selection = Some((placed.0[1].1.start, placed.0[3].1.start));
        let original = history.project().clone();
        for event in events {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events: vec![event],
                    ..Default::default()
                },
                |ui| {
                    let ctx = ui.ctx();
                    panel.copy_shortcut(ctx, history.project(), &selected);
                    assert!(ctx.input(|i| i.events.is_empty()));
                },
            );
            assert!(output.platform_output.commands.iter().any(|command| {
                matches!(command, egui::OutputCommand::CopyText(text) if text == "one two three")
            }));
            assert_eq!(history.project(), &original);
            output.textures_delta.clear();
        }
        panel.selection = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Copy],
                ..Default::default()
            },
            |ui| {
                panel.copy_shortcut(ui.ctx(), history.project(), &selected);
            },
        );
        assert!(
            !output
                .platform_output
                .commands
                .iter()
                .any(|c| matches!(c, egui::OutputCommand::CopyText(_)))
        );
        output.textures_delta.clear();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn copy_all_text_uses_trimmed_reordered_and_repeated_timeline_placements() {
        let dir = tmp("text-scope");
        let (mut history, mut panel) = project(&dir);
        let project = history.project();
        let audio = project
            .sequence()
            .tracks()
            .iter()
            .find(|t| t.kind == TrackKind::Audio)
            .unwrap();
        let original = audio.clips()[0].clone();
        let track = audio.id;
        history
            .apply(Command::AddClip {
                track,
                clip: NewClip {
                    asset: original.asset,
                    source_in: t(12.0),
                    source_out: t(14.0),
                    start: t(20.0),
                    link: None,
                },
            })
            .unwrap();
        let selected = HashSet::from([original.id]);
        let placed = panel.cache.placed(history.project(), &selected);
        assert_eq!(
            text::plain(&placed.0),
            "zero one two three four five six seven eight nine"
        );
        let all = panel.cache.placed(history.project(), &HashSet::new());
        assert!(text::plain(&all.0).ends_with("\n\ntwo three"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn transcript_copy_and_export_controls_are_non_destructive_and_find_keeps_copy() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        struct State {
            history: History,
            panel: TranscriptPanel,
            copied: Vec<String>,
            exported: Vec<String>,
        }
        let dir = tmp("text-controls");
        let (history, mut panel) = project(&dir);
        panel.selection = Some((t(3.0), t(5.0)));
        let original = history.project().clone();
        let mut fresh = true;
        let mut h = Harness::builder().with_size([900.0, 480.0]).build_ui_state(
            move |ui, state: &mut State| {
                if fresh {
                    theme::install(ui.ctx());
                    fresh = false;
                    ui.ctx().request_repaint();
                    return;
                }
                let selected = HashSet::new();
                state
                    .panel
                    .copy_shortcut(ui.ctx(), state.history.project(), &selected);
                let out = state
                    .panel
                    .show(ui, state.history.project(), &selected, Time(0), true);
                assert!(out.seek.is_none() && out.command.is_none() && out.transcribe.is_none());
                if let Some(text) = out.export {
                    state.exported.push(text);
                }
                state.copied.extend(ui.ctx().output(|out| {
                    out.commands
                        .iter()
                        .filter_map(|command| match command {
                            egui::OutputCommand::CopyText(text) => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                }));
            },
            State {
                history,
                panel,
                copied: Vec::new(),
                exported: Vec::new(),
            },
        );
        h.run();
        h.get_by_label(tr!("action.copy")).click();
        h.run();
        assert_eq!(h.state().copied.last().unwrap(), "one two three");
        h.get_by_label(tr!("transcript.copy_all")).click();
        h.run();
        let all = "zero one two three four five six seven eight nine";
        assert_eq!(h.state().copied.last().unwrap(), all);
        h.get_by_label(tr!("transcript.export_text")).click();
        h.run();
        assert_eq!(h.state().exported, [all]);
        assert_eq!(h.state().history.project(), &original);

        h.get_by_role(egui::accesskit::Role::TextInput).click();
        h.run();
        h.event(egui::Event::Text("search words".into()));
        h.run();
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
        h.run();
        h.event(egui::Event::Copy);
        h.run();
        assert_eq!(h.state().copied.last().unwrap(), "search words");
        assert_eq!(h.state().panel.selection, Some((t(3.0), t(5.0))));
        assert_eq!(h.state().history.project(), &original);
        fs::remove_dir_all(dir).unwrap();
    }

    /// Each clip's (timeline start, source in, source out), track by track.
    fn ranges(h: &History) -> Vec<Vec<(Time, Time, Time)>> {
        let seq = h.project().sequence();
        let clip = |c: &jvid_core::model::Clip| (c.start, c.source_in, c.source_out);
        let track = |tr: &jvid_core::model::Track| tr.clips().iter().map(clip).collect();
        seq.tracks().iter().map(track).collect()
    }

    /// Acceptance: deleting three words from a fake transcript removes exactly their ranges
    /// from video and audio, as one undo step that brings the clip and the words back.
    #[test]
    fn deleting_three_words_cuts_their_ranges_from_video_and_audio() {
        let dir = tmp("cut");
        let (mut h, mut panel) = project(&dir);
        let none = HashSet::new();
        let (words, missing, _) = words(h.project(), &none, &mut panel.cache);
        assert!(missing.is_empty());
        assert_eq!(words.len(), 10);
        // On the timeline: "two" is at 4 s.
        assert_eq!(words[2].1.start, t(4.0));
        // "two three four": 4 s to 6.6 s, from source 12 s to 14.6 s.
        panel.selection = Some((words[4].1.start, words[2].1.start));
        let picked = panel.picked(&words).unwrap();
        assert_eq!(picked, 2..5);
        let before = ranges(&h);
        h.apply(cut(&words, picked).unwrap()).unwrap();
        let want = vec![(t(2.0), t(10.0), t(12.0)), (t(4.0), t(14.6), t(20.0))];
        assert_eq!(ranges(&h), [want.clone(), want]);
        // The words follow the edit.
        let (after, _, _) = super::words(h.project(), &none, &mut panel.cache);
        let spoken = "zero one five six seven eight nine";
        assert_eq!(texts(&after), spoken.split(' ').collect::<Vec<_>>());
        assert_eq!(after[2].1.start, t(4.4));

        h.undo();
        assert_eq!(ranges(&h), before);
        let (back, _, _) = super::words(h.project(), &none, &mut panel.cache);
        assert_eq!(back, words);
    }

    #[test]
    fn transcripts_are_cached_per_file_on_disk() {
        let dir = tmp("cache");
        let (h, _panel) = project(&dir);
        let path = &h.project().assets()[0].path;
        // A fresh cache reads what the first saved.
        let mut cache = Cache::new(dir.join("cache"));
        let words = cache.get(path, 0).unwrap().words.clone();
        assert_eq!(words.len(), 10);
        assert_eq!(words[3], word("three", 13.0, 13.6));
        assert!(cache.get(path, 1).is_none());
        // Rewriting the file makes it another file.
        fs::write(path, b"another take").unwrap();
        assert!(Cache::new(dir.join("cache")).get(path, 0).is_none());
        // Nothing transcribed: the panel asks only for the visible source span.
        let mut empty = Cache::new(dir.join("other"));
        let (none, missing, _) = super::words(h.project(), &HashSet::new(), &mut empty);
        assert!(none.is_empty());
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].range, 10 * 48_000..20 * 48_000);
    }

    /// Placing runs again only when the project, the selection or a transcript changes.
    #[test]
    fn placed_words_are_kept_until_something_changes() {
        let dir = tmp("placed");
        let (mut h, mut panel) = project(&dir);
        let none = HashSet::new();
        let first = panel.cache.placed(h.project(), &none);
        assert_eq!(first.0.len(), 10);
        assert!(Rc::ptr_eq(&first, &panel.cache.placed(h.project(), &none)));
        let (words, _, _) = &*first;
        h.apply(cut(words, 2..5).unwrap()).unwrap();
        let cut = panel.cache.placed(h.project(), &none);
        assert_eq!(cut.0.len(), 7);
        let one = HashSet::from([words[0].0]);
        assert!(!Rc::ptr_eq(&cut, &panel.cache.placed(h.project(), &one)));
        let again = panel.cache.placed(h.project(), &one);
        let source = Source {
            path: h.project().assets()[0].path.clone(),
            nth: 0,
            range: 0..i64::MAX,
        };
        let heard = Transcript {
            language: None,
            words: vec![word("only", 10.0, 10.5)],
        };
        panel.cache.insert(&source, heard);
        let after = panel.cache.placed(h.project(), &one);
        assert!(!Rc::ptr_eq(&again, &after));
        assert_eq!(texts(&after.0), ["only"]);
    }

    /// Game and mic streams, linked: the whole sequence shows the group's words once.
    #[test]
    fn a_link_group_with_two_audio_clips_shows_its_words_once() {
        let dir = tmp("streams");
        let (mut h, mut panel) = project(&dir);
        let kind = TrackKind::Audio;
        let Ok(jvid_core::history::Created::Track(track)) = h.apply(Command::AddTrack {
            kind,
            name: String::new(),
        }) else {
            panic!()
        };
        let clip = NewClip {
            asset: h.project().assets()[0].id,
            source_in: t(10.0),
            source_out: t(20.0),
            start: t(2.0),
            link: Some(jvid_core::model::LinkId(1)),
        };
        h.apply(Command::AddClip { track, clip }).unwrap();
        let (words, _, _) = super::words(h.project(), &HashSet::new(), &mut panel.cache);
        assert_eq!(words.len(), 10);
    }

    #[test]
    fn audit_443_language_specific_fillers_survive_reopen_and_one_undo() {
        for (language, sentence, expected) in [
            ("de", "Er kommt ähm", vec!["ähm"]),
            ("pt", "um livro hum", vec!["hum"]),
            ("en", "um umbrella uhhh", vec!["um", "uhhh"]),
        ] {
            let dir = tmp(language);
            let (mut h, mut panel) = project(&dir);
            let source = Source {
                path: h.project().assets()[0].path.clone(),
                nth: 0,
                range: 0..i64::MAX,
            };
            let words = sentence
                .split(' ')
                .enumerate()
                .map(|(i, w)| word(w, 10.0 + i as f64 * 0.5, 10.3 + i as f64 * 0.5))
                .collect();
            panel.cache.insert(
                &source,
                Transcript {
                    language: Some(language.into()),
                    words,
                },
            );
            let before = h.project().clone();
            for reopen in [false, true] {
                if reopen {
                    panel.cache = Cache::new(dir.join("cache"));
                    panel.cache.get(&source.path, source.nth).unwrap();
                }
                let (placed, _, languages) =
                    super::words(h.project(), &HashSet::new(), &mut panel.cache);
                let hits = fillers::find(&placed, &languages);
                let actual: Vec<_> = hits
                    .iter()
                    .filter(|h| !h.pause)
                    .map(|h| placed[h.at].1.text.as_str())
                    .collect();
                assert_eq!(actual, expected, "{language}, reopen={reopen}");
                h.apply(fillers::cut(&hits, &HashSet::new()).unwrap())
                    .unwrap();
                assert!(h.undo());
                assert_eq!(h.project(), &before);
            }
        }
    }

    /// Acceptance: a fake transcript with four fillers and one long pause loses all five
    /// ranges from video and audio in one undo step; an unticked hit stays in the edit.
    #[test]
    fn four_fillers_and_a_long_pause_are_cut_unless_unticked() {
        let dir = tmp("fillers");
        let (mut h, mut panel) = project(&dir);
        let spoken = [
            ("So", 10.0),
            ("um,", 10.4),
            ("I", 10.8),
            ("uh", 11.1),
            ("think.", 11.5),
            ("Er,", 14.0),
            ("this", 14.4),
            ("Hmm,", 14.8),
            ("works.", 15.2),
        ];
        let source = Source {
            path: h.project().assets()[0].path.clone(),
            nth: 0,
            range: 0..i64::MAX,
        };
        let words = spoken.iter().map(|&(w, s)| word(w, s, s + 0.3)).collect();
        let heard = Transcript {
            language: Some("en".into()),
            words,
        };
        panel.cache.insert(&source, heard);
        let none = HashSet::new();
        let (words, _, languages) = super::words(h.project(), &none, &mut panel.cache);
        let hits = fillers::find(&words, &languages);
        let fillers: Vec<_> = hits
            .iter()
            .filter(|h| !h.pause)
            .map(|h| words[h.at].1.text.as_str())
            .collect();
        assert_eq!(fillers, ["um,", "uh", "Er,", "Hmm,"]);
        // The pause, 11.8 s to 14 s in the source, keeps an eighth of a second each side.
        let pause: Vec<_> = hits
            .iter()
            .filter(|h| h.pause)
            .map(|h| (h.start, h.end))
            .collect();
        assert_eq!(pause, [(t(3.8) + Time(S / 8), t(6.0) - Time(S / 8))]);
        let length = |h: &History| {
            ranges(h)
                .iter()
                .map(|tr| tr.iter().fold(Time(0), |a, c| a + (c.2 - c.1)))
                .collect::<Vec<_>>()
        };
        let before = h.project().clone();

        h.apply(fillers::cut(&hits, &HashSet::new()).unwrap())
            .unwrap();
        let (after, _, languages) = super::words(h.project(), &none, &mut panel.cache);
        assert_eq!(texts(&after), ["So", "I", "think.", "this", "works."]);
        // Ten seconds less four 0.3 s fillers and 1.95 s of pause.
        assert_eq!(length(&h), [t(6.85), t(6.85)]);
        assert!(fillers::find(&after, &languages).is_empty());
        h.undo();
        assert_eq!(h.project(), &before);

        let uh = HashSet::from([hits[1].start]);
        h.apply(fillers::cut(&hits, &uh).unwrap()).unwrap();
        let (after, _, _) = super::words(h.project(), &none, &mut panel.cache);
        assert_eq!(texts(&after), ["So", "I", "uh", "think.", "this", "works."]);
        assert_eq!(length(&h), [t(7.15), t(7.15)]);
    }

    #[test]
    fn words_in_two_clips_cut_as_two_spans() {
        let w = |c, s| (ClipId(c), word("w", s, s + 0.5));
        let words = [w(1, 0.0), w(1, 1.0), w(2, 2.0), w(2, 3.0)];
        let Some(Command::CutSpans { spans, speed: None }) = cut(&words, 0..4) else {
            panic!()
        };
        assert_eq!(
            spans,
            [(ClipId(1), t(0.0), t(1.5)), (ClipId(2), t(2.0), t(3.5))]
        );
        assert!(cut(&words, 1..1).is_none());
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;

    #[test]
    fn partial_cached_words_can_be_copied_and_exported_without_transcribing_gaps() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        let dir = tempfile::tempdir().unwrap();
        let (history, mut panel) = super::tests::project(dir.path());
        let path = history.project().assets()[0].path.clone();
        let transcript = panel.cache.get(&path, 0).unwrap();
        panel.cache.store = Store::default().shared();
        let rate = jvid_media::audio::SAMPLE_RATE as i64;
        panel.cache.insert(
            &Source {
                path,
                nth: 0,
                range: 10 * rate..14 * rate,
            },
            transcript,
        );
        let original = history.project().clone();
        let mut fresh = true;
        let mut h = Harness::builder().with_size([900.0, 480.0]).build_ui_state(
            move |ui,
                  (history, panel, copied, exported): &mut (
                jvid_core::history::History,
                TranscriptPanel,
                Vec<String>,
                Option<String>,
            )| {
                if suite_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                let out = panel.show(ui, history.project(), &HashSet::new(), Time(0), true);
                assert!(out.transcribe.is_none() && out.command.is_none());
                if out.export.is_some() {
                    *exported = out.export;
                }
                copied.extend(ui.ctx().output(|out| {
                    out.commands
                        .iter()
                        .filter_map(|command| match command {
                            egui::OutputCommand::CopyText(text) => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                }));
            },
            (history, panel, Vec::new(), None),
        );
        h.run();
        assert_eq!(h.get_all_by_label(tr!("transcript.partial")).count(), 1);
        h.get_by_label(tr!("transcript.copy_all")).click();
        h.run();
        assert_eq!(h.state().2, ["zero one two three"]);
        h.get_by_label(tr!("transcript.export_text")).click();
        h.run();
        assert_eq!(h.state().3.as_deref(), Some("zero one two three"));
        let (history, panel, _, _) = h.state_mut();
        let missing = panel.missing(history.project(), &HashSet::new());
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].range, 14 * rate..20 * rate);
        assert_eq!(history.project(), &original);
    }

    #[test]
    fn missing_coverage_never_becomes_a_remove_fillers_pause() {
        let dir = tempfile::tempdir().unwrap();
        let (h, mut panel) = super::tests::project(dir.path());
        // Replace the fixture with two recognized ranges separated by an unheard gap.
        panel.cache.store = Store::default().shared();
        let path = h.project().assets()[0].path.clone();
        let rate = jvid_media::audio::SAMPLE_RATE as i64;
        let settings = panel.cache.settings.clone();
        for (from, to, at) in [(10, 12, 11), (16, 20, 17)] {
            let source = Source {
                path: path.clone(),
                nth: 0,
                range: from * rate..to * rate,
            };
            let mut store = panel.cache.store.lock().unwrap();
            let request = store.request(&source, &settings).unwrap();
            store
                .insert(
                    &request,
                    Transcript {
                        language: Some("en".into()),
                        words: vec![Word {
                            text: "um".into(),
                            start: Time(at * S),
                            end: Time(at * S + S / 2),
                            confidence: 1.0,
                        }],
                    },
                )
                .unwrap();
        }
        let selected = HashSet::new();
        let (words, missing, languages) = words(h.project(), &selected, &mut panel.cache);
        assert_eq!(
            fillers::find(&words, &languages)
                .iter()
                .filter(|h| h.pause)
                .count(),
            1
        );
        let honest = covered_hits(&words, &languages, &missing, h.project(), &selected);
        assert_eq!(honest.len(), 2, "the filler words can still be removed");
        assert!(honest.iter().all(|h| !h.pause));
        // A successful silent result closes the gap and makes the pause meaningful.
        let source = missing[0].clone();
        panel.cache.insert(&source, Transcript::default());
        let (words, missing, languages) = super::words(h.project(), &selected, &mut panel.cache);
        assert!(missing.is_empty());
        assert_eq!(
            covered_hits(&words, &languages, &missing, h.project(), &selected).len(),
            3
        );
    }

    #[test]
    fn source_rewrite_invalidates_a_live_placed_view_even_without_project_edits() {
        let dir = tempfile::tempdir().unwrap();
        let (h, mut panel) = super::tests::project(dir.path());
        let selected = HashSet::new();
        assert_eq!(panel.cache.placed(h.project(), &selected).0.len(), 10);
        std::fs::write(&h.project().assets()[0].path, b"new-take").unwrap();
        let placed = panel.cache.placed(h.project(), &selected);
        assert!(placed.0.is_empty());
        assert_eq!(placed.1.len(), 1);
    }
}
