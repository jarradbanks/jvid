//! Fade handles: a square in each top corner of a clip, dragged inward to set its fade in or
//! out; the ramp is drawn as a line from the corner. Double-clicking a handle toggles linear
//! and equal-power. Edits are `SetClipProps`, as in the inspector, so they undo the same way.

use egui::{CursorIcon, Id, Painter, Rect, Sense, Stroke, Ui, pos2, vec2};
use jvid_core::history::{Command, GestureKey};
use jvid_core::model::{Clip, FadeCurve};
use jvid_core::time::Time;
use suite_ui::tokens::color;

use super::{Edit, Timeline, View};

/// Handle size.
const S: f32 = 7.0;
/// Undo gesture for handle drags, xored with the clip, side and drag count.
const GESTURE: GestureKey = 0x6661_6465_0000; // "fade"

/// Sets the fade in (`out` false) or out of `c` to `len`, held to the clip's length.
fn set(c: &Clip, out: bool, len: Time) -> Command {
    let mut x = c.transform.clone();
    let len = len.clamp(Time(0), c.duration());
    *if out { &mut x.fade_out } else { &mut x.fade_in } = len;
    Command::SetClipProps(vec![(c.id, x, c.keyframes.clone())])
}

fn toggle(c: &Clip) -> Command {
    let mut x = c.transform.clone();
    x.fade_curve = match x.fade_curve {
        FadeCurve::Linear => FadeCurve::EqualPower,
        FadeCurve::EqualPower => FadeCurve::Linear,
    };
    Command::SetClipProps(vec![(c.id, x, c.keyframes.clone())])
}

impl Timeline {
    /// Draws and edits `clip`'s fade handles inside `r`, its rect on screen; a `locked` clip
    /// shows its fades without handles.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn fade_handles(
        &mut self,
        ui: &Ui,
        painter: &Painter,
        view: View,
        clip: &Clip,
        r: Rect,
        id: Id,
        locked: bool,
    ) -> Option<Edit> {
        // Too narrow for two handles and the trim zones.
        if r.width() < 4.0 * S {
            return None;
        }
        let mut edit = None;
        let drags = id.with(("fade drags", clip.id));
        for out in [false, true] {
            let x = &clip.transform;
            let (len, edge) = if out {
                (x.fade_out, view.x(clip.end()))
            } else {
                (x.fade_in, view.x(clip.start))
            };
            let sign = if out { -1.0 } else { 1.0 };
            let at = (edge + sign * (view.x(clip.start + len) - view.x(clip.start)))
                .clamp(r.left() + S / 2.0, r.right() - S / 2.0);
            if len > Time(0) {
                let corner = pos2(edge.clamp(r.left(), r.right()), r.bottom());
                painter.line_segment(
                    [corner, pos2(at, r.top())],
                    Stroke::new(1.5, color::amber()),
                );
            }
            if locked {
                continue;
            }
            let h = Rect::from_center_size(pos2(at, r.top() + S / 2.0), vec2(S, S));
            let resp = ui.interact(h, id.with(("fade", clip.id, out)), Sense::click_and_drag());
            let hot = resp.hovered() || resp.dragged();
            if hot {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            painter.rect_filled(h, 1.0, if hot { color::amber() } else { color::border() });
            if resp.double_clicked() {
                edit = Some((toggle(clip), None));
            }
            if resp.drag_started() {
                ui.data_mut(|d| *d.get_temp_mut_or(drags, 0u64) += 1);
            }
            if resp.dragged()
                && let Some(p) = resp.interact_pointer_pos()
            {
                let rate = self.transport.rate;
                let t = rate.time_floor(rate.index_nearest(view.time(p.x)));
                let len = if out { clip.end() - t } else { t - clip.start };
                let n = ui.data(|d| d.get_temp(drags).unwrap_or(0u64));
                let key = GESTURE ^ clip.id.0 ^ (out as u64) << 8 ^ (n << 16);
                edit = Some((set(clip, out, len), Some(key)));
            }
        }
        edit
    }
}

#[cfg(test)]
mod tests {
    use jvid_core::history::{Command, History};
    use jvid_core::model::Edge;

    use super::super::tests::project;
    use super::*;

    fn clip(h: &History) -> Clip {
        h.project().sequence().tracks()[0].clips()[0].clone()
    }

    #[test]
    fn fades_are_undoable_and_survive_trims() {
        let mut h = History::new(project(2));
        let d = clip(&h).duration();
        h.apply(set(&clip(&h), false, Time(-5))).unwrap();
        assert_eq!(clip(&h).transform.fade_in, Time(0), "held to zero");
        h.apply(set(&clip(&h), true, Time(d.0 * 2))).unwrap();
        assert_eq!(clip(&h).transform.fade_out, d, "held to the clip");
        h.apply(set(&clip(&h), true, Time(d.0 / 4))).unwrap();
        h.apply(toggle(&clip(&h))).unwrap();
        assert_eq!(clip(&h).transform.fade_curve, FadeCurve::EqualPower);
        let id = clip(&h).id;
        let trims = vec![(id, Edge::Tail, Time(-d.0 / 2))];
        h.apply(Command::Trim {
            trims,
            ripple: false,
        })
        .unwrap();
        let c = clip(&h);
        assert_eq!(
            (c.transform.fade_out, c.duration()),
            (Time(d.0 / 4), Time(d.0 / 2))
        );
        assert_eq!(c.fade_at(c.end()), 0.0);
        assert!(h.undo() && h.undo());
        assert_eq!(clip(&h).transform.fade_curve, FadeCurve::Linear);
        assert!(h.undo() && h.undo());
        assert_eq!(clip(&h).transform.fade_out, Time(0));
    }
}
