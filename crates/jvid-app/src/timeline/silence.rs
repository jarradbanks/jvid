//! Detect Silence: finds stretches under a level in the selected audio clips' waveform peaks,
//! shades them on the timeline while its dialog is open, and ripple deletes them or speeds
//! them up as one edit.

use std::collections::HashSet;

use egui::{Align2, Key, Pos2, Ui};
use jvid_core::history::Command;
use jvid_core::model::{Clip, ClipId, Sequence, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use jvid_media::audio::SAMPLE_RATE;
use jvid_media::silence;
use jvid_media::waveforms::Peak;
use suite_ui::theme;
use suite_ui::tokens::{radius, space};
use suite_ui::widgets::{Button, DragField, State};

use crate::i18n::{tr, trn};

/// A timeline span to cut from a clip and its linked clips.
pub(super) type Span = (ClipId, Time, Time);

/// The open dialog: its settings and the clips it reads.
pub(super) struct Silence {
    /// dBFS a stretch has to stay under.
    threshold: f64,
    /// Seconds a stretch has to last.
    min_len: f64,
    /// Seconds of each stretch kept at both ends.
    pad: f64,
    /// Speed Up's rate in percent, as in the inspector.
    speed: f64,
    /// One audio clip per link group; cuts carry its linked clips along.
    clips: Vec<ClipId>,
}

impl Silence {
    /// Reads the audio clips among `selected` and their linked partners; `None` if there are
    /// none.
    pub(super) fn open(seq: &Sequence, selected: &HashSet<ClipId>) -> Option<Self> {
        let audio = |id: ClipId| {
            seq.linked(id).into_iter().filter(|(t, c)| {
                c.freeze.is_none() && seq.track(*t).is_some_and(|t| t.kind == TrackKind::Audio)
            })
        };
        let mut clips: Vec<&Clip> = selected
            .iter()
            .flat_map(|&id| audio(id))
            .map(|(_, c)| c)
            .collect();
        clips.sort_by_key(|c| (c.start, c.id.0));
        clips.dedup_by_key(|c| c.id);
        // shortcut: a group with several audio clips (game and mic streams) reads only its
        // first; let the dialog pick the stream if mixed recordings miss silences.
        let mut links = HashSet::new();
        clips.retain(|c| c.link.is_none_or(|l| links.insert(l)));
        (!clips.is_empty()).then(|| Self {
            threshold: -40.0,
            min_len: 1.0,
            pad: 0.15,
            speed: 400.0,
            clips: clips.iter().map(|c| c.id).collect(),
        })
    }

    /// The clips whose peaks it needs.
    pub(super) fn clips(&self) -> &[ClipId] {
        &self.clips
    }

    /// The silent spans on the timeline, from each clip's finest peaks; `None` while any
    /// clip's peaks are still loading.
    pub(super) fn spans<'a>(
        &self,
        seq: &Sequence,
        peaks: &dyn Fn(&Clip) -> Option<&'a [Peak]>,
    ) -> Option<Vec<Span>> {
        let rate = SAMPLE_RATE as i64;
        let samples = |secs: f64| (secs * rate as f64) as i64;
        let time = |n: i64| Time(n * (FLICKS_PER_SECOND / rate));
        let mut out = Vec::new();
        for &id in &self.clips {
            let Some((_, c)) = seq.clip(id) else { continue };
            let found = silence::detect(
                peaks(c)?,
                self.threshold as f32,
                samples(self.min_len),
                samples(self.pad),
            );
            for (a, b) in found {
                let (a, b) = (time(a).max(c.source_in), time(b).min(c.source_out));
                if a < b {
                    // Reversed clips map the source range back to front.
                    let (x, y) = (c.timeline_at(a), c.timeline_at(b));
                    out.push((id, x.min(y), x.max(y)));
                }
            }
        }
        Some(out)
    }

    /// The dialog, floating at `at` above the lanes so the shaded spans stay in view. Returns
    /// the edit once Cut or Speed Up is picked, and false once it should close.
    pub(super) fn show(
        &mut self,
        ui: &Ui,
        at: Pos2,
        spans: Option<&[Span]>,
    ) -> (Option<Command>, bool) {
        let mut command = None;
        let mut open = !ui.input(|i| i.key_pressed(Key::Escape));
        egui::Window::new(tr!("silence.title"))
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
                ui.label(egui::RichText::new(tr!("silence.title")).font(theme::headline()));
                ui.add_space(space::S8);
                egui::Grid::new("silence_grid")
                    .spacing([space::S8, space::S8])
                    .show(ui, |ui| {
                        let rows = [
                            (
                                tr!("silence.threshold"),
                                &mut self.threshold,
                                tr!("silence.db_suffix"),
                                -80.0..=-10.0,
                                0,
                                0.25,
                            ),
                            (
                                tr!("silence.min_len"),
                                &mut self.min_len,
                                tr!("silence.seconds_suffix"),
                                0.1..=30.0,
                                2,
                                0.02,
                            ),
                            (
                                tr!("silence.padding"),
                                &mut self.pad,
                                tr!("silence.seconds_suffix"),
                                0.0..=2.0,
                                2,
                                0.01,
                            ),
                            (
                                tr!("silence.speed"),
                                &mut self.speed,
                                "%",
                                110.0..=10000.0,
                                0,
                                5.0,
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
                let found = spans.map(|s| {
                    let total: i64 = s.iter().map(|(_, a, b)| (*b - *a).0).sum();
                    let seconds = format!("{:.1}", total as f64 / FLICKS_PER_SECOND as f64);
                    match s.len() {
                        0 => tr!("silence.none").to_owned(),
                        n => trn!("silence.found", n, seconds = seconds),
                    }
                });
                ui.label(found.unwrap_or_else(|| tr!("silence.reading").to_owned()));
                ui.add_space(space::S16);
                let any = spans.is_some_and(|s| !s.is_empty());
                let state = if any { State::Live } else { State::Disabled };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = space::S8;
                    let mut pick = |ui: &mut Ui, b: Button, tip: &str, speed: Option<f64>| {
                        if ui.add(b.state(state)).on_hover_text(tip).clicked() {
                            let spans = spans.unwrap_or_default().to_vec();
                            command = Some(Command::CutSpans { spans, speed });
                            open = false;
                        }
                    };
                    pick(
                        ui,
                        Button::primary(tr!("silence.cut")),
                        tr!("silence.cut_tooltip"),
                        None,
                    );
                    let speed = Some(self.speed / 100.0);
                    pick(
                        ui,
                        Button::secondary(tr!("silence.speed_up")),
                        tr!("silence.speed_up_tooltip"),
                        speed,
                    );
                    if ui.add(Button::ghost(tr!("prompt.cancel"))).clicked() {
                        open = false;
                    }
                });
            });
        (command, open)
    }
}

#[cfg(test)]
mod tests {
    use jvid_core::history::History;
    use jvid_core::time::FLICKS_PER_SECOND as S;
    use jvid_media::waveforms::{BUCKET, Pyramid};

    use super::super::Timeline;
    use super::super::tests::project;
    use super::*;

    /// Acceptance: Cut leaves two adjacent clips with the silence gone, undone in one step,
    /// and Speed Up shortens the timeline by what the speed saves.
    #[test]
    fn cut_or_speed_up_the_detected_silence() {
        let p = project(2);
        let track = &p.sequence().tracks()[1];
        assert_eq!(track.kind, TrackKind::Audio);
        let audio = track.clips()[0].id;
        // Two seconds: loud, then silent from 0.5 s to 1.5 s, then loud.
        let rate = SAMPLE_RATE as i64;
        let peaks = (0..2 * rate / BUCKET)
            .map(|i| {
                let quiet = i * BUCKET >= rate / 2 && (i + 1) * BUCKET <= 3 * rate / 2;
                if quiet { [0.0; 2] } else { [-0.5, 0.5] }
            })
            .collect();
        let mut t = Timeline::default();
        let path = (p.assets()[0].path.clone(), 0);
        t.media.peaks.insert(path, Some(Pyramid::new(peaks)));
        t.sync(&p);
        assert!(!t.can("detect_silence", p.sequence()));
        t.selected.insert(audio);
        assert!(t.can("detect_silence", p.sequence()));
        assert!(t.action("detect_silence", &p));
        let s = t.silence.as_mut().unwrap();
        // Whole quiet buckets only: the 1 s silence reads a few ms short.
        (s.pad, s.min_len) = (0.0, 0.5);

        let spans = t.silence_spans(&p, &egui::Context::default()).unwrap();
        let [(id, a, b)] = spans[..] else {
            panic!("{spans:?}")
        };
        assert_eq!(id, audio);
        let frame = S / 30;
        assert!((a.0 - S / 2).abs() <= frame && (b.0 - 3 * S / 2).abs() <= frame);

        let mut h = History::new(p.clone());
        let cut = Command::CutSpans {
            spans: spans.clone(),
            speed: None,
        };
        h.apply(cut).unwrap();
        let clips = h.project().sequence().tracks()[1].clips();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].end(), clips[1].start);
        assert_eq!(clips[1].end(), Time(2 * S) - (b - a));
        assert!(h.undo());
        assert_eq!(h.project(), &p);

        let speed = Some(4.0);
        h.apply(Command::CutSpans { spans, speed }).unwrap();
        let clips = h.project().sequence().tracks()[1].clips();
        let saved = (b - a).0 - (b - a).0 / 4;
        assert!((clips.last().unwrap().end().0 - (2 * S - saved)).abs() <= 1);
        assert!(h.undo());
        assert_eq!(h.project(), &p);
    }
}
