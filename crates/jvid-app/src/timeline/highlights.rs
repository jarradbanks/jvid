//! Find Highlights: scans the selected clips for loud stretches (from their waveform peaks)
//! and scene changes (decoded on a job), lists the ranges it finds, and adds them as ranged
//! markers in one undo step.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{Align2, Key, Pos2, Ui};
use jvid_core::history::Command;
use jvid_core::model::{Clip, ClipId, Sequence, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND, Rate, Time, format_timecode};
use jvid_jobs::{Handle, Pool, Priority};
use jvid_media::audio::SAMPLE_RATE;
use jvid_media::highlights;
use jvid_media::waveforms::Peak;
use suite_ui::theme;
use suite_ui::tokens::{color, radius, size, space};
use suite_ui::widgets::{Button, Checkbox, DragField, State};

use crate::i18n::{tr, trn};
use crate::subtitles::AudioSources;

/// How far apart the scene scan samples frames.
const STEP: Time = Time(FLICKS_PER_SECOND / 5);
/// Loud peaks closer than this make one range.
const GAP_SECONDS: i64 = 2;

/// Each sampled frame's distance from the one before: the clip, its source time, 0–1.
type Distances = Vec<(ClipId, Time, f32)>;

/// The scene scan: running while `handle` is, done once `found` is filled.
pub(super) struct Scan {
    handle: Option<Handle>,
    found: Arc<Mutex<Option<Distances>>>,
}

/// The open dialog: its settings, the clips it reads and its scene scan.
pub(super) struct Highlights {
    /// 0–100: higher finds quieter peaks and smaller picture changes.
    sensitivity: f64,
    /// Seconds added at both ends of each range.
    pad: f64,
    loud: bool,
    scenes: bool,
    /// One audio clip per link group, as Detect Silence reads.
    audio: Vec<ClipId>,
    selected: HashSet<ClipId>,
    sources: AudioSources,
    video: Vec<ClipId>,
    pub(super) scan: Option<Scan>,
}

impl Drop for Highlights {
    /// Closing the dialog, however it closes, stops the scan.
    fn drop(&mut self) {
        if let Some(h) = self.scan.as_ref().and_then(|s| s.handle.as_ref()) {
            h.cancel();
        }
    }
}

impl Highlights {
    /// Reads the clips among `selected` and their linked partners; `None` if none has audio
    /// or video to scan.
    pub(super) fn open(seq: &Sequence, selected: &HashSet<ClipId>) -> Option<Self> {
        if selected.is_empty() {
            return None;
        }
        let mut clips: Vec<(TrackKind, &Clip)> = selected
            .iter()
            .flat_map(|&id| seq.linked(id))
            .filter(|(_, c)| c.freeze.is_none())
            .filter_map(|(t, c)| Some((seq.track(t)?.kind, c)))
            .collect();
        clips.sort_by_key(|(_, c)| (c.start, c.id.0));
        clips.dedup_by_key(|(_, c)| c.id);
        let sources = AudioSources::default();
        let audio: Vec<_> = sources
            .clips(seq, selected, true)
            .iter()
            .map(|c| c.id)
            .collect();
        let video: Vec<_> = clips
            .iter()
            .filter(|(k, _)| *k == TrackKind::Video)
            .map(|(_, c)| c.id)
            .collect();
        (!audio.is_empty() || !video.is_empty()).then(|| Self {
            sensitivity: 50.0,
            pad: 2.0,
            loud: !audio.is_empty(),
            scenes: audio.is_empty(),
            audio,
            selected: selected.clone(),
            sources,
            video,
            scan: None,
        })
    }

    /// The audio clips whose peaks it needs.
    pub(super) fn audio(&self) -> &[ClipId] {
        if self.loud { &self.audio } else { &[] }
    }

    /// Starts the scene scan on `jobs` if it's wanted and hasn't started; `path` finds each
    /// clip's file. The job repaints `ctx` as it goes.
    pub(super) fn start_scan(
        &mut self,
        seq: &Sequence,
        jobs: &Pool,
        path: &dyn Fn(&Clip) -> Option<PathBuf>,
        ctx: &egui::Context,
    ) {
        if !self.scenes || self.scan.is_some() || self.video.is_empty() {
            return;
        }
        let clips: Vec<_> = self
            .video
            .iter()
            .filter_map(|&id| seq.clip(id))
            .filter_map(|(_, c)| Some((c.id, path(c)?, c.source_in, c.source_out)))
            .collect();
        let found = Arc::new(Mutex::new(None));
        let (out, ctx) = (found.clone(), ctx.clone());
        static RUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let key = format!(
            "highlights-{}",
            RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let handle = jobs.submit(key, Priority::Interactive, move |job| {
            let n = clips.len() as f32;
            let mut all = Vec::new();
            for (i, (id, path, from, to)) in clips.into_iter().enumerate() {
                let mut progress = |p: f32| {
                    job.set_progress((i as f32 + p) / n);
                    ctx.request_repaint();
                    !job.is_cancelled()
                };
                match highlights::scenes(&path, from, to, STEP, &mut progress) {
                    Ok(d) => all.extend(d.into_iter().map(|(t, d)| (id, t, d))),
                    Err(e) => tracing::warn!("scene scan of {}: {e}", path.display()),
                }
            }
            if !job.is_cancelled() {
                *out.lock().unwrap() = Some(all);
                ctx.request_repaint();
            }
        });
        self.scan = Some(Scan {
            handle: Some(handle),
            found,
        });
    }

    /// The highlight ranges on the timeline, sorted and merged; `None` while peaks load or the
    /// scene scan runs.
    pub(super) fn ranges<'a>(
        &self,
        seq: &Sequence,
        peaks: &dyn Fn(&Clip) -> Option<&'a [Peak]>,
    ) -> Option<Vec<(Time, Time)>> {
        let s = self.sensitivity as f32 / 100.0;
        let rate = SAMPLE_RATE as i64;
        let time = |n: i64| Time(n * (FLICKS_PER_SECOND / rate));
        let pad = Time((self.pad * FLICKS_PER_SECOND as f64) as i64);
        let mut out = Vec::new();
        // A source range of `c` on the timeline, padded; reversed clips map back to front.
        let mut push = |c: &Clip, a: Time, b: Time| {
            let (a, b) = (a.max(c.source_in), b.min(c.source_out));
            if a <= b {
                let (x, y) = (c.timeline_at(a), c.timeline_at(b));
                out.push(((x.min(y) - pad).max(Time(0)), x.max(y) + pad));
            }
        };
        for &id in self.audio() {
            let Some((_, c)) = seq.clip(id) else { continue };
            let p = peaks(c)?;
            // Relative to the clip's loudest peak, so a quiet recording finds its moments too.
            let top = p
                .iter()
                .map(|p| p[0].abs().max(p[1].abs()))
                .fold(0.0, f32::max);
            let db = 20.0 * top.max(1e-6).log10() - (1.0 + 20.0 * s);
            for (a, b) in highlights::loud(p, db, GAP_SECONDS * rate) {
                push(c, time(a), time(b));
            }
        }
        if self.scenes && !self.video.is_empty() {
            let found = self.scan.as_ref()?.found.lock().unwrap();
            let threshold = 0.8 - 0.6 * s;
            for &(id, t, d) in found.as_ref()? {
                if d > threshold
                    && let Some((_, c)) = seq.clip(id)
                {
                    push(c, t, t);
                }
            }
        }
        out.sort();
        let mut merged: Vec<(Time, Time)> = Vec::new();
        for (a, b) in out {
            match merged.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        Some(merged)
    }

    /// The scan's progress, 0–1, while it runs.
    fn scanning(&self) -> Option<f32> {
        let h = self.scan.as_ref()?.handle.as_ref()?;
        (self.scenes && !h.is_finished()).then(|| h.progress())
    }

    /// The dialog, floating at `at` above the lanes. Returns the markers to add once Add
    /// Markers is picked, and false once it should close.
    pub(super) fn show(
        &mut self,
        ui: &Ui,
        at: Pos2,
        seq: &Sequence,
        rate: Rate,
        ranges: Option<&[(Time, Time)]>,
    ) -> (Option<Command>, bool) {
        let mut command = None;
        let mut open = !ui.input(|i| i.key_pressed(Key::Escape));
        egui::Window::new(tr!("highlights.title"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .pivot(Align2::RIGHT_BOTTOM)
            .default_pos(at)
            .frame(
                egui::Frame::popup(ui.style())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(space::S16),
            )
            .show(ui.ctx(), |ui| {
                ui.set_width(280.0);
                ui.label(egui::RichText::new(tr!("highlights.title")).font(theme::headline()));
                ui.add_space(space::S8);
                let changed = self.sources.show(ui, seq, &self.selected, true);
                if changed {
                    self.audio = self
                        .sources
                        .clips(seq, &self.selected, true)
                        .iter()
                        .map(|c| c.id)
                        .collect();
                }
                let ranges = if changed { None } else { ranges };
                let audio = if self.audio.is_empty() {
                    State::Disabled
                } else {
                    State::Live
                };
                ui.add(Checkbox::new(&mut self.loud, tr!("highlights.loud")).state(audio));
                let video = if self.video.is_empty() {
                    State::Disabled
                } else {
                    State::Live
                };
                ui.add(Checkbox::new(&mut self.scenes, tr!("highlights.scenes")).state(video));
                ui.add_space(space::S8);
                egui::Grid::new("highlights_grid")
                    .spacing([space::S8, space::S8])
                    .show(ui, |ui| {
                        let rows = [
                            (
                                tr!("highlights.sensitivity"),
                                &mut self.sensitivity,
                                "%",
                                0.0..=100.0,
                                0,
                                0.5,
                            ),
                            (
                                tr!("highlights.padding"),
                                &mut self.pad,
                                tr!("silence.seconds_suffix"),
                                0.0..=30.0,
                                1,
                                0.05,
                            ),
                        ];
                        for (label, v, suffix, range, decimals, speed) in rows {
                            ui.label(label);
                            ui.add(
                                DragField::new(v)
                                    .suffix(suffix)
                                    .range(range)
                                    .decimals(decimals)
                                    .speed(speed)
                                    .width(96.0),
                            );
                            ui.end_row();
                        }
                    });
                ui.add_space(space::S8);
                let status = match (self.scanning(), ranges) {
                    (Some(p), _) => tr!("highlights.scanning", percent = (p * 100.0) as u32),
                    (None, None) => tr!("silence.reading").to_owned(),
                    (None, Some([])) => tr!("highlights.none").to_owned(),
                    (None, Some(r)) => trn!("highlights.found", r.len()),
                };
                ui.label(status);
                if let Some(p) = self.scanning() {
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 4.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(rect, 2.0, color::border_strong());
                    let done = rect.with_max_x(rect.left() + rect.width() * p);
                    ui.painter().rect_filled(done, 2.0, color::ink());
                }
                if let Some(r) = ranges.filter(|r| !r.is_empty()) {
                    ui.add_space(space::S4);
                    let tc = |t: Time| format_timecode(rate.index_floor(t).max(0) as u64, rate);
                    egui::ScrollArea::vertical()
                        .max_height(size::CONTROL * 6.0)
                        .show_rows(ui, size::CONTROL, r.len(), |ui, rows| {
                            for (i, &(a, b)) in
                                r.iter().enumerate().skip(rows.start).take(rows.len())
                            {
                                ui.allocate_ui(
                                    egui::vec2(ui.available_width(), size::CONTROL),
                                    |ui| {
                                        ui.horizontal_centered(|ui| {
                                            ui.label(tr!("highlights.marker_name", n = i + 1));
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    let text = format!("{} – {}", tc(a), tc(b));
                                                    ui.label(
                                                        egui::RichText::new(text)
                                                            .color(color::ink_2()),
                                                    );
                                                },
                                            );
                                        });
                                    },
                                );
                            }
                        });
                }
                ui.add_space(space::S16);
                let any = ranges.is_some_and(|r| !r.is_empty()) && self.scanning().is_none();
                let state = if any { State::Live } else { State::Disabled };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = space::S8;
                    let add = Button::primary(tr!("highlights.add")).state(state);
                    if ui.add(add).clicked() {
                        command = Some(markers(ranges.unwrap_or_default()));
                        open = false;
                    }
                    if ui.add(Button::ghost(tr!("prompt.cancel"))).clicked() {
                        open = false;
                    }
                });
            });
        (command, open)
    }
}

/// One ranged marker per range, numbered in order, as one undo step.
pub(super) fn markers(ranges: &[(Time, Time)]) -> Command {
    Command::Batch(
        ranges
            .iter()
            .enumerate()
            .map(|(i, &(a, b))| Command::AddMarker {
                time: a,
                duration: b - a,
                name: tr!("highlights.marker_name", n = i + 1),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use jvid_core::history::History;
    use jvid_core::model::NewClip;
    use jvid_core::time::FLICKS_PER_SECOND as S;
    use jvid_media::waveforms::{BUCKET, Pyramid};

    use super::super::Timeline;
    use super::super::tests::project;
    use super::*;

    /// Acceptance: a ten-second clip with loud bursts at 2–2.5 s and 6–7 s gets a ranged
    /// marker on each, within a frame, added and undone in one step; a scene change near the
    /// second burst merges into it once padding makes them overlap.
    #[test]
    fn loud_bursts_become_markers() {
        let mut h = History::new(project(0));
        let asset = h.project().assets()[0].id;
        let tracks: Vec<_> = h
            .project()
            .sequence()
            .tracks()
            .iter()
            .map(|t| t.id)
            .collect();
        for &track in &tracks {
            let clip = NewClip {
                asset,
                source_in: Time(0),
                source_out: Time(10 * S),
                start: Time(0),
                link: None,
            };
            h.apply(Command::AddClip { track, clip }).unwrap();
        }
        let p = h.project().clone();
        let id = |i: usize| p.sequence().tracks()[i].clips()[0].id;
        let rate = SAMPLE_RATE as i64;
        let burst = |s: f64| (s * rate as f64) as i64;
        let peaks = (0..10 * rate / BUCKET)
            .map(|i| {
                let at = i * BUCKET;
                let loud = (burst(2.0)..burst(2.5)).contains(&at)
                    || (burst(6.0)..burst(7.0)).contains(&at);
                if loud { [-0.9, 0.9] } else { [-0.01, 0.01] }
            })
            .collect();
        let mut t = Timeline::default();
        t.media.peaks.insert(
            (p.assets()[0].path.clone(), 0),
            Some(Arc::new(Pyramid::new(peaks))),
        );
        t.sync(&p);
        assert!(!t.can("find_highlights", p.sequence()));
        t.selected.insert(id(1));
        assert!(t.action("find_highlights", &p));
        t.highlights.as_mut().unwrap().pad = 0.0;
        let ctx = egui::Context::default();

        let ranges = t.highlight_ranges(&p, &ctx).unwrap();
        let frame = S / 30;
        let near = |a: Time, s: f64| (a.0 - (s * S as f64) as i64).abs() <= frame;
        let [(a, b), (c, d)] = ranges[..] else {
            panic!("{ranges:?}")
        };
        assert!(
            near(a, 2.0) && near(b, 2.5) && near(c, 6.0) && near(d, 7.0),
            "{ranges:?}"
        );

        h.apply(markers(&ranges)).unwrap();
        let m = h.project().sequence().markers();
        assert_eq!(m.len(), 2);
        assert_eq!((m[1].time, m[1].time + m[1].duration), (c, d));
        assert_eq!(m[1].name, "Highlight 2");
        assert!(h.undo());
        assert_eq!(h.project(), &p);

        // A scene change at 5 s, with a second of padding, joins the second burst.
        t.selected.insert(id(0));
        assert!(t.action("find_highlights", &p));
        let hl = t.highlights.as_mut().unwrap();
        (hl.pad, hl.scenes) = (1.0, true);
        assert!(t.highlight_ranges(&p, &ctx).is_none(), "waits for the scan");
        let found = vec![(id(0), Time(5 * S), 0.9), (id(0), Time(9 * S), 0.05)];
        t.highlights.as_mut().unwrap().scan = Some(Scan {
            handle: None,
            found: Arc::new(Mutex::new(Some(found))),
        });
        let ranges = t.highlight_ranges(&p, &ctx).unwrap();
        assert_eq!(ranges.len(), 2, "{ranges:?}");
        assert!(
            near(ranges[1].0, 4.0) && near(ranges[1].1, 8.0),
            "{ranges:?}"
        );
    }
}
