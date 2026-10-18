//! Markers: M drops one at the playhead without touching playback, Alt+M turns in/out into a
//! ranged one, Shift+M / Ctrl+Shift+M jump between them. Flags sit on the ruler; click one to
//! seek, right-click to edit its name, color and note.

use egui::{Color32, Id, Rect, Sense, Shape, Stroke, Ui, pos2};
use jvid_core::history::Command;
use jvid_core::model::Sequence;
use jvid_core::time::Time;
use suite_ui::tokens::color;

use super::{Edit, Timeline, View};

/// `Marker::color` indexes this.
pub const PALETTE: &[Color32] = &[
    color::AMBER,
    color::RED,
    color::GREEN,
    color::LIGHT_BLUE,
    color::CLIP_PLUM,
    color::CLIP_VIOLET,
];

fn swatch(i: u8) -> Color32 {
    PALETTE.get(i as usize).copied().unwrap_or(color::AMBER)
}

impl Timeline {
    /// The edit or seek for a marker action, or false if `id` isn't one.
    pub(super) fn marker_action(&mut self, id: &str, seq: &Sequence) -> bool {
        let here = self.playhead();
        match id {
            "add_marker" => {
                self.pending = Some(Command::AddMarker {
                    time: here,
                    duration: Time(0),
                    name: String::new(),
                });
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
                (true, true) => "Marker".to_owned(),
                (false, true) => m.name.clone(),
                (true, false) => m.note.clone(),
                (false, false) => format!("{}\n{}", m.name, m.note),
            };
            let r = r.on_hover_text(tip);
            let gesture = r.id.value();
            r.context_menu(|ui| {
                let mut e = m.clone();
                ui.label("Name");
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
                                Stroke::new(1.5, color::INK),
                                egui::StrokeKind::Outside,
                            );
                        }
                        if r.clicked() {
                            (e.color, changed) = (i as u8, true);
                        }
                    }
                });
                ui.label("Note");
                changed |= ui.text_edit_multiline(&mut e.note).changed();
                if changed {
                    edit = Some((Command::SetMarker(e), Some(gesture)));
                }
                if ui.button("Delete Marker").clicked() {
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
            h.apply(t.pending.take().unwrap()).unwrap();
            assert_eq!(t.transport.speed(), 1);
        }
        assert_eq!(h.project().sequence().markers().len(), 5);
    }
}
