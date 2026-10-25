//! Edit by Text: the Transcript panel. It shows the words spoken in the selected clips, or in
//! the whole sequence, from each file's transcript cached in the app's cache folder (never in
//! the project). Words are placed through the clips every frame, so they follow timeline edits
//! and undo. Clicking a word seeks to it, the word under the playhead is lit, Find picks out
//! matches, and Delete ripple deletes the selected words from their clips and linked clips as
//! one undo step.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};

use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{Color32, Key, Margin, Sense, Ui, vec2};
use jvid_core::history::Command;
use jvid_core::model::{ClipId, Project};
use jvid_core::time::{FLICKS_PER_SECOND as S, Time};
use jvid_transcribe::{Source, Transcript, Word};
use suite_ui::panel::Panel;
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{Button, TextInput};

use crate::i18n::tr;
use crate::subtitles::targets;

/// Bumped when the file format or the word timing changes, so older transcripts are redone.
const VERSION: u32 = 1;
/// A pause this long between words starts a new paragraph.
const PARAGRAPH: Time = Time(S);
/// Line height of the words, roomier than Body's for reading.
const LINE: f32 = 20.0;

/// Whole audio streams' transcripts, in a folder and loaded as needed.
#[derive(Default)]
pub struct Cache {
    dir: Option<PathBuf>,
    /// By path and stream; `None` when not transcribed.
    loaded: HashMap<(PathBuf, usize), Option<Vec<Word>>>,
}

impl Cache {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            loaded: HashMap::new(),
        }
    }

    /// Changes whenever the file is moved, resized or rewritten.
    // shortcut: DefaultHasher may change between Rust releases, which only costs a re-transcribe.
    fn file(&self, path: &Path, nth: usize) -> Option<PathBuf> {
        let m = fs::metadata(path).ok()?;
        let mut h = DefaultHasher::new();
        (VERSION, path, m.len(), m.modified().ok()).hash(&mut h);
        Some(
            self.dir
                .as_ref()?
                .join(format!("{:016x}.{nth}.txt", h.finish())),
        )
    }

    /// The words of audio stream `nth` of `path`, timed from the file's start; `None` until
    /// it's transcribed.
    pub fn get(&mut self, path: &Path, nth: usize) -> Option<&[Word]> {
        let key = (path.to_owned(), nth);
        if !self.loaded.contains_key(&key) {
            let text = self
                .file(path, nth)
                .and_then(|f| fs::read_to_string(f).ok());
            self.loaded.insert(key.clone(), text.map(|t| parse(&t)));
        }
        self.loaded[&key].as_deref()
    }

    /// Keeps `t`, the transcript of the whole of `source`, and saves it.
    pub fn insert(&mut self, source: &Source, t: Transcript) {
        if let Some(file) = self.file(&source.path, source.nth) {
            let saved = file.parent().map_or(Ok(()), fs::create_dir_all);
            if let Err(e) = saved.and_then(|_| fs::write(&file, format(&t.words))) {
                tracing::warn!("transcript: can't save {}: {e}", file.display());
            }
        }
        let key = (source.path.clone(), source.nth);
        self.loaded.insert(key, Some(t.words));
    }
}

/// One word per line: start and end in flicks, then the text, tab separated.
fn format(words: &[Word]) -> String {
    let line = |w: &Word| {
        let text = w.text.replace(['\t', '\n', '\r'], " ");
        format!("{}\t{}\t{text}\n", w.start.0, w.end.0)
    };
    words.iter().map(line).collect()
}

fn parse(text: &str) -> Vec<Word> {
    let word = |line: &str| {
        let mut f = line.splitn(3, '\t');
        Some(Word {
            start: Time(f.next()?.parse().ok()?),
            end: Time(f.next()?.parse().ok()?),
            text: f.next()?.to_owned(),
            confidence: 1.0,
        })
    };
    text.lines().filter_map(word).collect()
}

/// The words to show, in timeline time with the clip each is in, sorted; and the whole
/// streams not transcribed yet.
fn words(
    project: &Project,
    selected: &HashSet<ClipId>,
    cache: &mut Cache,
) -> (Vec<(ClipId, Word)>, Vec<Source>) {
    let (mut words, mut missing) = (Vec::new(), Vec::<Source>::new());
    for t in targets(project, selected, true) {
        let whole = Source {
            range: 0..i64::MAX,
            ..t.source.clone()
        };
        match cache.get(&whole.path, whole.nth) {
            Some(w) => words.extend(t.place(w).map(|w| (t.clip.id, w))),
            None if !missing.contains(&whole) => missing.push(whole),
            None => {}
        }
    }
    words.sort_by_key(|(c, w)| (w.start, c.0));
    (words, missing)
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
    /// Transcribe was clicked: these streams have no transcript yet.
    pub transcribe: Option<Vec<Source>>,
    pub close: bool,
}

#[derive(Default)]
pub struct TranscriptPanel {
    pub cache: Cache,
    query: String,
    /// The first and last words picked, by start time, so it outlives edits elsewhere.
    selection: Option<(Time, Time)>,
    /// The start of the word last lit, to scroll to the next one.
    lit: Option<Time>,
}

impl TranscriptPanel {
    /// The picked words, as a range of `words`.
    fn picked(&self, words: &[(ClipId, Word)]) -> Option<Range<usize>> {
        let (a, b) = self.selection?;
        let (a, b) = (a.min(b), a.max(b));
        let from = words.partition_point(|(_, w)| w.start < a);
        let to = words.partition_point(|(_, w)| w.start <= b);
        (from < to).then_some(from..to)
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        selected: &HashSet<ClipId>,
        playhead: Time,
        focused: bool,
    ) -> Output {
        let mut out = Output::default();
        let (words, missing) = words(project, selected, &mut self.cache);
        let close = [(icon::X, tr!("transcript.close"))];
        let r = Panel::titled(tr!("panel.transcript"))
            .actions(&close)
            .focused(focused)
            .show(ui, |ui| {
                let bar = egui::Frame::NONE
                    .inner_margin(Margin::symmetric(space::INSET as i8, space::S8 as i8));
                bar.show(ui, |ui| {
                    // Right to left, so Find takes what Transcribe leaves.
                    let rtl = egui::Layout::right_to_left(egui::Align::Center);
                    let row = vec2(ui.available_width(), size::CONTROL);
                    ui.allocate_ui_with_layout(row, rtl, |ui| {
                        ui.spacing_mut().item_spacing.x = space::S8;
                        let button = Button::secondary(tr!("transcript.transcribe"));
                        if !missing.is_empty() && ui.add(button).clicked() {
                            out.transcribe = Some(missing.clone());
                        }
                        let find = TextInput::search(&mut self.query).hint(tr!("transcript.find"));
                        let find = ui.add(find.width(ui.available_width()));
                        if find.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            self.find_next(&words, playhead, &mut out);
                        }
                    });
                });
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let frame = egui::Frame::NONE
                            .inner_margin(Margin::symmetric(space::INSET as i8, space::S4 as i8));
                        frame.show(ui, |ui| self.text(ui, &words, playhead, &mut out));
                    });
            });
        out.close = r.action == Some(0);
        if focused && !ui.ctx().text_edit_focused() {
            let delete = ui.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace));
            if let Some(picked) = self.picked(&words).filter(|_| delete) {
                out.command = cut(&words, picked);
                self.selection = None;
            }
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                self.selection = None;
            }
        }
        out
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
        let all = galley.text().to_owned();
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &all));
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
    fn project(dir: &Path) -> (History, TranscriptPanel) {
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
        let (words, missing) = words(h.project(), &none, &mut panel.cache);
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
        let (after, _) = super::words(h.project(), &none, &mut panel.cache);
        let spoken = "zero one five six seven eight nine";
        assert_eq!(texts(&after), spoken.split(' ').collect::<Vec<_>>());
        assert_eq!(after[2].1.start, t(4.4));

        h.undo();
        assert_eq!(ranges(&h), before);
        let (back, _) = super::words(h.project(), &none, &mut panel.cache);
        assert_eq!(back, words);
    }

    #[test]
    fn transcripts_are_cached_per_file_on_disk() {
        let dir = tmp("cache");
        let (h, _panel) = project(&dir);
        let path = &h.project().assets()[0].path;
        // A fresh cache reads what the first saved.
        let mut cache = Cache::new(dir.join("cache"));
        let words = cache.get(path, 0).unwrap().to_vec();
        assert_eq!(words.len(), 10);
        assert_eq!(words[3], word("three", 13.0, 13.6));
        assert!(cache.get(path, 1).is_none());
        // Rewriting the file makes it another file.
        fs::write(path, b"another take").unwrap();
        assert!(Cache::new(dir.join("cache")).get(path, 0).is_none());
        // Nothing transcribed: the panel asks for the whole stream.
        let mut empty = Cache::new(dir.join("other"));
        let (none, missing) = super::words(h.project(), &HashSet::new(), &mut empty);
        assert!(none.is_empty());
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].range, 0..i64::MAX);
        assert_eq!(parse(&format(&[word("a\tb", 1.0, 2.0)]))[0].text, "a b");
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
