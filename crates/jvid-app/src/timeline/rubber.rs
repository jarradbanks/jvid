//! The volume rubber band: a line over an audio clip's waveform at its gain, 0 at the bottom
//! to [`MAX`] at the top. Ctrl+click on the line adds a `volume` key there; dragging a point
//! moves it between its neighbours. Edits are `SetClipProps`, as in the inspector.

use egui::{CursorIcon, Id, Painter, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use jvid_core::history::{Command, GestureKey};
use jvid_core::keyframe::{Easing, Keyframe, Value};
use jvid_core::model::Clip;
use jvid_core::time::Time;
use suite_ui::tokens::color;

use super::{Edit, Timeline, View};

/// Gain at the top of the clip, as the inspector's range.
const MAX: f64 = 2.0;
/// Point radius, and how near the line a Ctrl+click must land.
const R: f32 = 4.0;
/// Undo gesture for point drags, xored with the clip and drag count.
const GESTURE: GestureKey = 0x7275_6262_0000; // "rubb"

fn y_of(v: f64, r: Rect) -> f32 {
    r.bottom() - (v / MAX).clamp(0.0, 1.0) as f32 * r.height()
}

fn v_of(y: f32, r: Rect) -> f64 {
    ((r.bottom() - y) / r.height()) as f64 * MAX
}

fn props(c: &Clip, keys: Vec<Keyframe>) -> Command {
    let mut all = c.keyframes.clone();
    all.insert("volume".into(), keys);
    Command::SetClipProps(vec![(c.id, c.transform.clone(), all)])
}

/// Adds a volume key at source time `at`. The first key starts from the static volume, so
/// the line doesn't jump.
fn add(c: &Clip, at: Time, v: f64) -> Command {
    let mut keys = c.keyframes.get("volume").cloned().unwrap_or_default();
    let k = Keyframe {
        time: at,
        value: Value::Scalar(v.clamp(0.0, MAX)),
        easing: Easing::Linear,
    };
    match keys.binary_search_by_key(&at, |k| k.time) {
        Ok(i) => keys[i] = k,
        Err(i) => keys.insert(i, k),
    }
    props(c, keys)
}

/// Moves volume key `i` to source time `at`, kept strictly between its neighbours so the
/// order (and the dragged point's index) holds.
fn shift(c: &Clip, i: usize, at: Time, v: f64) -> Command {
    let mut keys = c.keyframes["volume"].clone();
    let lo = i
        .checked_sub(1)
        .map_or(Time(i64::MIN), |j| keys[j].time + Time(1));
    let hi = keys.get(i + 1).map_or(Time(i64::MAX), |k| k.time - Time(1));
    keys[i].time = at.clamp(lo, hi);
    keys[i].value = Value::Scalar(v.clamp(0.0, MAX));
    props(c, keys)
}

impl Timeline {
    /// Draws `clip`'s volume line over `body`, its waveform area, and edits it. `cr` is the
    /// clip's own response: a Ctrl+click on it near the line adds a point.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn rubber_band(
        &mut self,
        ui: &Ui,
        painter: &Painter,
        view: View,
        clip: &Clip,
        body: Rect,
        cr: &Response,
        id: Id,
    ) -> Option<Edit> {
        if body.height() < 8.0 {
            return None;
        }
        let painter = painter.with_clip_rect(body.intersect(painter.clip_rect()));
        let shown = painter.clip_rect();
        let src = |t: Time| clip.source_at(t);
        let tl = |s: Time| clip.timeline_at(s);
        let gain = |x: f32| clip.gain_at(view.time(x)).0;
        // Sampled every few points, so easing curves show.
        let mut line = Vec::new();
        let mut x = shown.left();
        while x < shown.right() + 3.0 {
            line.push(pos2(x, y_of(gain(x), body)));
            x += 3.0;
        }
        painter.add(egui::Shape::line(line, Stroke::new(1.5, color::AMBER)));

        let mut edit = None;
        let keys = clip.keyframes.get("volume").map_or(&[][..], Vec::as_slice);
        let drags = id.with(("rubber drags", clip.id));
        for (i, k) in keys.iter().enumerate() {
            let Value::Scalar(v) = k.value else { continue };
            let c = pos2(view.x(tl(k.time)), y_of(v, body));
            if c.x < shown.left() - R || c.x > shown.right() + R {
                continue;
            }
            let r = ui.interact(
                Rect::from_center_size(c, vec2(3.0 * R, 3.0 * R)),
                id.with(("rubber", clip.id, i)),
                Sense::drag(),
            );
            let hot = r.hovered() || r.dragged();
            painter.circle_filled(c, if hot { R + 1.0 } else { R }, color::AMBER);
            if hot {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            }
            if r.drag_started() {
                ui.data_mut(|d| *d.get_temp_mut_or(drags, 0u64) += 1);
            }
            if r.dragged()
                && let Some(p) = r.interact_pointer_pos()
            {
                let rate = self.transport.rate;
                let at = rate.time_floor(rate.index_nearest(view.time(p.x)));
                let at = src(at).clamp(clip.source_in, clip.source_out);
                let n = ui.data(|d| d.get_temp(drags).unwrap_or(0u64));
                let cmd = shift(clip, i, at, v_of(p.y, body));
                edit = Some((cmd, Some(GESTURE ^ clip.id.0 ^ (n << 16))));
            }
        }
        if cr.clicked()
            && ui.input(|i| i.modifiers.command)
            && let Some(p) = cr.interact_pointer_pos()
            && body.contains(p)
            && (p.y - y_of(gain(p.x), body)).abs() <= 2.0 * R
        {
            edit = Some((add(clip, src(view.time(p.x)), gain(p.x)), None));
        }
        edit
    }
}

#[cfg(test)]
mod tests {
    use jvid_core::history::History;

    use super::super::tests::project;
    use super::*;

    fn clip(h: &History) -> Clip {
        h.project().sequence().tracks()[0].clips()[1].clone()
    }

    #[test]
    fn add_then_drag_points_as_undoable_props() {
        let mut h = History::new(project(4));
        let (s0, start) = (clip(&h).source_in, clip(&h).start);
        let at = |n: i64| s0 + Time(n);
        h.apply(add(&clip(&h), at(10), 1.0)).unwrap();
        h.apply(add(&clip(&h), at(50), 0.5)).unwrap();
        h.apply(add(&clip(&h), at(90), 9.0)).unwrap();
        let c = clip(&h);
        assert_eq!(c.gain_at(start + Time(30)).0, 0.75);
        assert_eq!(c.gain_at(start + Time(95)).0, MAX, "clamped to the top");
        // Dragging the middle point past its neighbour stops just short of it.
        h.apply(shift(&c, 1, at(500), 0.0)).unwrap();
        let keys: Vec<_> = clip(&h).keyframes["volume"]
            .iter()
            .map(|k| (k.time, k.value))
            .collect();
        assert_eq!(
            keys,
            [
                (at(10), Value::Scalar(1.0)),
                (at(89), Value::Scalar(0.0)),
                (at(90), Value::Scalar(MAX))
            ]
        );
        assert!(h.undo());
        assert_eq!(clip(&h).keyframes["volume"][1].time, at(50));
    }

    #[test]
    fn line_maps_gain_to_height() {
        let r = Rect::from_min_max(pos2(0.0, 10.0), pos2(10.0, 50.0));
        assert_eq!(y_of(0.0, r), 50.0);
        assert_eq!(y_of(1.0, r), 30.0);
        assert_eq!(y_of(5.0, r), 10.0);
        assert_eq!(v_of(30.0, r), 1.0);
    }
}
