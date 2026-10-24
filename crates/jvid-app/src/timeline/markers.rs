//! Markers: M drops one at the playhead without touching playback, or marks a range while
//! held during playback; Alt+M turns in/out into a
//! ranged one, Shift+M / Ctrl+Shift+M jump between them. Flags sit on the ruler; click one to
//! seek, right-click to edit its name, color and note.

use egui::{Color32, Id, Rect, Sense, Shape, Stroke, Ui, pos2};
use jvid_core::history::Command;
use jvid_core::model::Sequence;
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use suite_ui::themes::DARK;
use suite_ui::tokens::color;

use crate::i18n::tr;

use super::{Edit, Timeline, View};

/// `Marker::color` indexes this. Like clip colours, they're the user's and don't follow the theme.
pub const PALETTE: &[Color32] = &[
    DARK.amber,
    DARK.red,
    DARK.green,
    DARK.light_blue,
    color::CLIP_PLUM,
    color::CLIP_VIOLET,
];

fn swatch(i: u8) -> Color32 {
    PALETTE.get(i as usize).copied().unwrap_or(DARK.amber)
}

impl Timeline {
    /// The edit or seek for a marker action, or false if `id` isn't one.
    pub(super) fn marker_action(&mut self, id: &str, seq: &Sequence) -> bool {
        let here = self.playhead();
        match id {
            // Key repeats while held land here too; the first press is the start.
            "add_marker" => {
                self.marker_hold.get_or_insert(here);
            }
            "add_range_marker" => {
                let (i, o) = (seq.in_point(), seq.out_point());
                self.pending = (i.is_some() || o.is_some()).then(|| {
                    let start = i.unwrap_or_default();
                    Command::AddMarker {
                        time: start,
                        duration: o.unwrap_or(seq.duration()) - start,
                        name: String::new(),
                    }
                });
            }
            "next_marker" | "prev_marker" => {
                let times = seq.markers().iter().map(|m| m.time);
                let to = if id == "next_marker" {
                    times.filter(|t| *t > here).min()
                } else {
                    times.filter(|t| *t < here).max()
                };
                if let Some(t) = to {
                    self.seek(t);
                }
            }
            _ => return false,
        }
        true
    }

    /// M is up: a point marker where it went down, or a range to here if the playhead moved
    /// more than a tap's worth. Does nothing unless M went down.
    pub fn release_marker(&mut self) {
        let Some(start) = self.marker_hold.take() else {
            return;
        };
        // shortcut: a tap is measured in timeline time, exact at 1x; use wall time if
        // fast shuttling makes taps come out as ranges.
        let here = self.playhead();
        let tap = (here - start).0.abs() < FLICKS_PER_SECOND * 3 / 10;
        let (time, end) = if tap {
            (start, start)
        } else {
            (start.min(here), start.max(here))
        };
        self.pending = Some(Command::AddMarker {
            time,
            duration: end - time,
            name: String::new(),
        });
    }

    /// Draws the flags (and ranges) on the ruler and handles clicks on them.
    pub(super) fn markers(
        &mut self,
        ui: &Ui,
        seq: &Sequence,
        view: View,
        ruler: Rect,
        id: Id,
    ) -> Option<Edit> {
        let painter = ui.painter().with_clip_rect(ruler);
        let mut edit = None;
        for m in seq.markers() {
            let (x, c) = (view.x(m.time), swatch(m.color));
            if m.duration > Time(0) {
                let band = Rect::from_x_y_ranges(x..=view.x(m.time + m.duration), ruler.y_range());
                painter.rect_filled(band, 0.0, c.gamma_multiply(0.3));
            }
            let top = ruler.top() + 1.0;
            painter.add(Shape::convex_polygon(
                vec![pos2(x - 5.0, top), pos2(x + 5.0, top), pos2(x, top + 8.0)],
                c,
                Stroke::NONE,
            ));
            painter.line_segment([pos2(x, top), pos2(x, ruler.bottom())], Stroke::new(1.0, c));
            // The upper part only, so scrubbing still works under a flag.
            let hit = Rect::from_x_y_ranges(x - 5.0..=x + 5.0, top..=top + 10.0);
            let r = ui.interact(hit, id.with(("marker", m.id.0)), Sense::click());
            if r.clicked() {
                self.seek(m.time);
            }
            let tip = match (m.name.is_empty(), m.note.is_empty()) {
                (true, true) => tr!("timeline.marker").to_owned(),
                (false, true) => m.name.clone(),
                (true, false) => m.note.clone(),
                (false, false) => format!("{}\n{}", m.name, m.note),
            };
            let r = r.on_hover_text(tip);
            let gesture = r.id.value();
            // Default close-on-click would shut it on the first click into Name.
            egui::Popup::context_menu(&r)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    let mut e = m.clone();
                    ui.label(tr!("timeline.marker_name"));
                    let mut changed = ui.text_edit_singleline(&mut e.name).changed();
                    ui.horizontal(|ui| {
                        for (i, c) in PALETTE.iter().enumerate() {
                            let (rect, r) =
                                ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::click());
                            ui.painter().rect_filled(rect, 3.0, *c);
                            if i as u8 == e.color {
                                ui.painter().rect_stroke(
                                    rect,
                                    3.0,
                                    Stroke::new(1.5, color::ink()),
                                    egui::StrokeKind::Outside,
                                );
                            }
                            if r.clicked() {
                                (e.color, changed) = (i as u8, true);
                            }
                        }
                    });
                    ui.label(tr!("timeline.marker_note"));
                    changed |= ui.text_edit_multiline(&mut e.note).changed();
                    if changed {
                        edit = Some((Command::SetMarker(e), Some(gesture)));
                    }
                    if ui.button(tr!("timeline.delete_marker")).clicked() {
                        edit = Some((Command::RemoveMarker(m.id), None));
                        ui.close();
                    }
                });
        }
        edit
    }
}

#[cfg(test)]
mod tests {
    use jvid_core::history::History;
    use jvid_core::time::FLICKS_PER_SECOND as S;

    use super::super::tests::project;
    use super::*;

    #[test]
    fn m_adds_markers_and_jumps_between_them() {
        let mut h = History::new(project(4));
        let mut t = Timeline::default();
        let run = |t: &mut Timeline, h: &mut History, id| {
            assert!(t.action(id, h.project()));
            t.release_marker();
            if let Some(c) = t.pending.take() {
                h.apply(c).unwrap();
            }
        };
        let marks = |h: &History| -> Vec<_> {
            let m = h.project().sequence().markers().iter();
            m.map(|m| (m.time.0 / S, m.duration.0 / S)).collect()
        };
        t.sync(h.project());
        for s in [1, 3] {
            t.seek(Time(s * S));
            run(&mut t, &mut h, "add_marker");
        }
        assert_eq!(marks(&h), [(1, 0), (3, 0)]);
        // No in/out: nothing to range.
        run(&mut t, &mut h, "add_range_marker");
        assert_eq!(marks(&h).len(), 2);
        h.apply(Command::SetInOut {
            in_point: Some(Time(5 * S)),
            out_point: Some(Time(7 * S)),
        })
        .unwrap();
        run(&mut t, &mut h, "add_range_marker");
        assert_eq!(marks(&h)[2], (5, 2));
        run(&mut t, &mut h, "prev_marker");
        assert_eq!(t.playhead(), Time(S));
        run(&mut t, &mut h, "next_marker");
        assert_eq!(t.playhead(), Time(3 * S));
        run(&mut t, &mut h, "next_marker");
        assert_eq!(t.playhead(), Time(5 * S));
        run(&mut t, &mut h, "next_marker");
        assert_eq!(t.playhead(), Time(5 * S));
    }

    #[test]
    fn m_while_playing_keeps_playing() {
        let mut h = History::new(project(4));
        let mut t = Timeline::default();
        t.sync(h.project());
        t.transport.toggle(0);
        for _ in 0..5 {
            assert!(t.action("add_marker", h.project()));
            t.release_marker();
            h.apply(t.pending.take().unwrap()).unwrap();
            assert_eq!(t.transport.speed(), 1);
        }
        assert_eq!(h.project().sequence().markers().len(), 5);
    }

    /// #331: holding M while playing marks from press to release; a tap stays a point.
    #[test]
    fn holding_m_marks_a_range() {
        use crate::keymap::Keymap;
        use egui::Key;
        use egui_kittest::Harness;
        let mut hist = History::new(project(4));
        let p = hist.project().clone();
        let map = Keymap::default();
        let mut t = Timeline::default();
        t.sync(&p);
        t.transport.toggle(0);
        let mut h = Harness::new_ui_state(
            |ui, t: &mut Timeline| {
                for id in map.pressed(ui.ctx(), true) {
                    assert!(t.action(id, &p), "{id}");
                }
                if !map.held(ui.ctx(), "add_marker") {
                    t.release_marker();
                }
            },
            t,
        );
        // Press at `from`, the playhead plays on to `to`, release.
        let mut hold = |from: i64, to: i64| {
            h.state_mut().seek(Time(from));
            h.key_down(Key::M);
            h.run();
            // Key repeats don't move the start.
            h.key_down(Key::M);
            h.run();
            assert!(h.state().pending.is_none(), "added before release");
            h.state_mut().seek(Time(to));
            h.key_up(Key::M);
            h.run();
            assert_eq!(h.state().transport.speed(), 1);
            let c = h.state_mut().pending.take().expect("a marker");
            hist.apply(c).unwrap();
            let m = hist.project().sequence().markers().last().unwrap();
            (m.time, m.duration)
        };
        assert_eq!(hold(S, 4 * S), (Time(S), Time(3 * S)));
        assert_eq!(hold(6 * S, 6 * S + S / 10), (Time(6 * S), Time(0)));
        // Shuttling backwards still gives a forward range.
        assert_eq!(hold(9 * S, 8 * S), (Time(8 * S), Time(S)));
        // Paused: one press, ten auto-repeats and a release make one marker.
        h.state_mut().transport.toggle(0);
        h.key_down(Key::M);
        for _ in 0..10 {
            h.event(egui::Event::Key {
                key: Key::M,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: egui::Modifiers::NONE,
            });
            h.run();
        }
        h.key_up(Key::M);
        h.run();
        hist.apply(h.state_mut().pending.take().unwrap()).unwrap();
        assert!(h.state().pending.is_none());
        assert_eq!(hist.project().sequence().markers().len(), 4);
        assert!(hist.undo());
        for n in [2, 1, 0] {
            assert!(hist.undo());
            assert_eq!(hist.project().sequence().markers().len(), n);
        }
    }

    /// #331: clicking into the right-click popup to name a marker keeps it open.
    #[test]
    fn marker_popup_takes_a_name() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = super::super::tests::harness(project(4));
        let c = Command::AddMarker {
            time: Time(S),
            duration: Time(0),
            name: String::new(),
        };
        h.state_mut().1.apply(c).unwrap();
        h.run();
        // The flag's grip, past the 8pt margin and the headers.
        let left = 8.0 + suite_ui::tokens::size::TRACK_HEADER;
        let at = pos2(h.state().0.view(left).x(Time(S)), 8.0 + 5.0);
        h.hover_at(at);
        h.run();
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            h.run();
        }
        let name = h.get_all_by_role(Role::TextInput).next().expect("popup");
        name.click();
        h.run();
        h.get_all_by_role(Role::TextInput)
            .next()
            .unwrap()
            .type_text("Intro");
        h.run();
        let marker = &h.state().1.project().sequence().markers()[0];
        assert_eq!(marker.name, "Intro");
        assert!(h.query_by_label("Note").is_some(), "popup closed");
    }
}
