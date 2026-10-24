//! The curve graph editor: one clip's keyframed values over source time, each number of
//! each property normalised to its own range so they share one plot. Keys click, box
//! select and drag in time and value; the selected keys show their easing's bezier
//! handles, and dragging one makes that easing the matching custom curve. Drags preview
//! until release, then land as one `SetClipProps`, so each is one undo step. The key
//! selection is the lanes'. Opened from the lanes' right-click menu, it covers the
//! timeline until closed.

use std::collections::{BTreeMap, BTreeSet};

use egui::{Align2, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use jvid_core::history::Command;
use jvid_core::keyframe::{Easing, Keyframe, Value, value_at};
use jvid_core::model::{Clip, ClipId, Sequence};
use jvid_core::time::{Time, format_timecode};
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::Button;

use super::keyframes::{caption, diamond, lanes, shift_keys};
use super::{Edit, Timeline, View, secs, tick_steps};
use crate::i18n::tr;

/// Key and handle hit size.
const HIT: f32 = 12.0;

type Selection = BTreeSet<(ClipId, String, Time)>;
type Keys = BTreeMap<String, Vec<Keyframe>>;

pub struct Graph {
    pub clip: ClipId,
    /// Source seconds at the plot's left edge, and points per second.
    scroll: f64,
    zoom: f64,
    /// Normalised values at the plot's bottom and top.
    lo: f64,
    hi: f64,
    /// Frame every key on the next frame, once the plot's size is known.
    pub fit: bool,
    drag: Option<Drag>,
}

impl Graph {
    pub fn new(clip: ClipId) -> Self {
        Self {
            clip,
            scroll: 0.0,
            zoom: 100.0,
            lo: 0.0,
            hi: 1.0,
            fit: true,
            drag: None,
        }
    }

    fn y(&self, plot: Rect, n: f64) -> f32 {
        plot.bottom() - ((n - self.lo) / (self.hi - self.lo)) as f32 * plot.height()
    }

    fn n(&self, plot: Rect, y: f32) -> f64 {
        self.lo + f64::from((plot.bottom() - y) / plot.height()) * (self.hi - self.lo)
    }

    /// Fits every key of `c` into a plot `width` wide, with a margin all round.
    fn frame(&mut self, c: &Clip, width: f32) {
        let times = lanes(c).flat_map(|(_, k)| k.iter().map(|k| secs(k.time)));
        let (a, b) = times.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), t| {
            (a.min(t), b.max(t))
        });
        let (a, b) = if b - a < 1e-6 {
            (a - 1.0, b + 1.0)
        } else {
            (a, b)
        };
        let pad = (b - a) * 0.05;
        self.zoom = f64::from(width.max(1.0)) / (b - a + 2.0 * pad);
        self.scroll = a - pad;
        (self.lo, self.hi) = (-0.1, 1.1);
    }

    /// Zooms both axes by `f` around plot point `p`.
    fn zoom_by(&mut self, plot: Rect, p: Pos2, f: f64) {
        let t = self.scroll + f64::from(p.x - plot.left()) / self.zoom;
        self.zoom = (self.zoom * f).clamp(1.0, 1e5);
        self.scroll = t - f64::from(p.x - plot.left()) / self.zoom;
        let n = self.n(plot, p.y);
        let span = ((self.hi - self.lo) / f).clamp(1e-3, 1e3);
        self.lo = n - (n - self.lo) / (self.hi - self.lo) * span;
        self.hi = self.lo + span;
    }
}

/// A drag in progress, shown as a preview until release.
#[derive(Clone, Debug)]
enum Drag {
    /// The selected keys shift by `by`, and number `part` of the grabbed key becomes `value`.
    Key {
        prop: String,
        time: Time,
        part: usize,
        by: Time,
        value: f64,
    },
    /// The easing out of `prop`'s key at `time` becomes this bezier.
    Handle {
        prop: String,
        time: Time,
        points: [f64; 4],
    },
}

/// A value's numbers: one for a scalar, two for a point, four for a colour.
fn parts(v: &mut Value) -> &mut [f64] {
    match v {
        Value::Scalar(x) => std::slice::from_mut(x),
        Value::Vec2(a) => a,
        Value::Color(a) => a,
    }
}

fn part(mut v: Value, i: usize) -> f64 {
    parts(&mut v).get(i).copied().unwrap_or_default()
}

fn key<'a>(keys: &'a mut Keys, prop: &str, t: Time) -> Option<&'a mut Keyframe> {
    keys.get_mut(prop)?.iter_mut().find(|k| k.time == t)
}

/// `c`'s keys with drag `d` applied.
fn dragged(d: &Drag, c: &Clip, sel: &Selection) -> Keys {
    let mut keys = c.keyframes.clone();
    match d {
        Drag::Key {
            prop,
            time,
            part,
            by,
            value,
        } => {
            shift_keys(&mut keys, c, sel, *by);
            if let Some(k) = key(&mut keys, prop, *time + *by)
                && let Some(x) = parts(&mut k.value).get_mut(*part)
            {
                *x = *value;
            }
        }
        Drag::Handle { prop, time, points } => {
            if let Some(k) = key(&mut keys, prop, *time) {
                k.easing = Easing::Bezier(*points);
            }
        }
    }
    keys
}

/// One curve: number `part` of property `prop`, and the value range that maps to 0..=1.
struct Curve<'a> {
    prop: &'a String,
    part: usize,
    keys: &'a [Keyframe],
    range: (f64, f64),
}

impl Curve<'_> {
    fn norm(&self, v: f64) -> f64 {
        (v - self.range.0) / (self.range.1 - self.range.0)
    }
}

/// Every number of every keyed property of `c`, in lane order.
fn curves(c: &Clip) -> Vec<Curve<'_>> {
    lanes(c)
        .flat_map(|(prop, keys)| {
            let n = parts(&mut keys[0].value.clone()).len();
            (0..n).map(move |i| {
                let (a, b) = keys
                    .iter()
                    .map(|k| part(k.value, i))
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                        (a.min(v), b.max(v))
                    });
                // A flat curve sits mid-height.
                let range = if b - a > 1e-9 {
                    (a, b)
                } else {
                    (a - 1.0, a + 1.0)
                };
                Curve {
                    prop,
                    part: i,
                    keys,
                    range,
                }
            })
        })
        .collect()
}

/// A curve's legend name: the lane caption, then which number for a point or colour.
fn label(c: &Clip, curve: &Curve) -> String {
    let name = caption(c, curve.prop);
    let n = parts(&mut curve.keys[0].value.clone()).len();
    let part = match (n, curve.part) {
        (1, _) => return name,
        (2, 0) => tr!("timeline.part_x"),
        (2, _) => tr!("timeline.part_y"),
        (_, 0) => tr!("timeline.part_r"),
        (_, 1) => tr!("timeline.part_g"),
        (_, 2) => tr!("timeline.part_b"),
        _ => tr!("timeline.part_a"),
    };
    format!("{name} {part}")
}

impl Timeline {
    /// Draws the graph editor over the whole timeline `area`; closing it, or its clip
    /// going away, brings the lanes back.
    pub(super) fn graph(&mut self, ui: &mut Ui, area: Rect, seq: &Sequence) -> Option<Edit> {
        let mut g = self.keys.graph.take()?;
        let clip = seq.clip(g.clip).map(|c| c.1)?;
        // Its last key deleted, the clip has nothing to graph.
        lanes(clip).next()?;
        let (edit, open) = self.graph_ui(ui, area, &mut g, clip);
        if open {
            self.keys.graph = Some(g);
        }
        edit
    }

    fn graph_ui(
        &mut self,
        ui: &mut Ui,
        area: Rect,
        g: &mut Graph,
        clip: &Clip,
    ) -> (Option<Edit>, bool) {
        let id = ui.id().with(("graph", clip.id));
        let line = 1.0 / ui.pixels_per_point();
        let left = area.left() + size::TRACK_HEADER;
        let ruler =
            Rect::from_x_y_ranges(left..=area.right(), area.top()..=area.top() + size::RULER);
        let corner = Rect::from_x_y_ranges(area.left()..=left, ruler.y_range());
        let legend = Rect::from_x_y_ranges(area.left()..=left, ruler.bottom()..=area.bottom());
        let plot = Rect::from_x_y_ranges(left..=area.right(), ruler.bottom()..=area.bottom());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, color::panel());
        painter.rect_filled(plot, 0.0, color::field());

        // Corner cell: the title, then Frame All and Close at the right.
        let mut open = true;
        painter.text(
            pos2(corner.left() + space::INSET, corner.center().y),
            Align2::LEFT_CENTER,
            tr!("timeline.graph_editor"),
            theme::body_strong(),
            color::ink(),
        );
        let button = |i: f32| {
            let right = corner.right() - space::S8 - i * (size::CONTROL + space::S4);
            Rect::from_x_y_ranges(right - size::CONTROL..=right, corner.y_range())
        };
        let close = ui
            .put(
                button(0.0),
                Button::icon(icon::X, tr!("timeline.close_graph")),
            )
            .on_hover_text(tr!("timeline.close_graph"));
        if close.clicked() {
            open = false;
        }
        let fit = ui
            .put(
                button(1.0),
                Button::icon(icon::SCAN, tr!("timeline.frame_all")),
            )
            .on_hover_text(tr!("timeline.frame_all"));
        if std::mem::take(&mut g.fit) || fit.clicked() {
            g.frame(clip, plot.width());
        }

        // Wheel pans, pinch or Ctrl+wheel zooms.
        let bg = ui.interact(plot, id.with("bg"), Sense::click_and_drag());
        if ui.rect_contains_pointer(plot)
            && let Some(p) = ui.input(|i| i.pointer.hover_pos())
        {
            let (z, s) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
            if z != 1.0 {
                g.zoom_by(plot, p, f64::from(z));
            } else {
                g.scroll -= f64::from(s.x) / g.zoom;
                let dn = f64::from(s.y / plot.height()) * (g.hi - g.lo);
                (g.lo, g.hi) = (g.lo + dn, g.hi + dn);
            }
        }
        let view = View {
            left,
            scroll: g.scroll,
            zoom: g.zoom,
        };

        // Ruler in source timecode, the clip's bounds, the 0 and 1 guides and the playhead.
        let rate = self.transport.rate;
        let (major, minor) = tick_steps(g.zoom, rate);
        let rp = painter.with_clip_rect(ruler);
        let first = rate.index_floor(view.time(plot.left())).max(0) / minor * minor;
        let mut f = first;
        while f <= rate.index_floor(view.time(plot.right())) + 1 {
            let x = view.x(rate.time_floor(f));
            let h = if f % major == 0 { 8.0 } else { 4.0 };
            rp.rect_filled(
                Rect::from_x_y_ranges(x..=x + line, ruler.bottom() - h..=ruler.bottom()),
                0.0,
                color::border_strong(),
            );
            if f % major == 0 {
                rp.text(
                    pos2(x + 3.0, ruler.top() + 2.0),
                    Align2::LEFT_TOP,
                    format_timecode(f as u64, rate),
                    theme::small(),
                    color::ink_3(),
                );
            }
            f += minor;
        }
        let pp = painter.with_clip_rect(plot);
        for t in [clip.source_in, clip.source_out] {
            let x = view.x(t);
            pp.rect_filled(
                Rect::from_x_y_ranges(x..=x + line, plot.y_range()),
                0.0,
                color::border_strong(),
            );
        }
        for n in [0.0, 1.0] {
            let y = g.y(plot, n);
            pp.rect_filled(
                Rect::from_x_y_ranges(plot.x_range(), y..=y + line),
                0.0,
                color::border(),
            );
        }
        let head = self.playhead();
        if (clip.start..clip.end()).contains(&head) {
            let x = view.x(clip.source_at(head));
            pp.rect_filled(
                Rect::from_x_y_ranges(x..=x + line, plot.y_range()),
                0.0,
                color::light_blue(),
            );
        }
        painter.rect_filled(
            Rect::from_x_y_ranges(area.x_range(), ruler.bottom() - line..=ruler.bottom()),
            0.0,
            color::border(),
        );
        painter.rect_filled(
            Rect::from_x_y_ranges(left - line..=left, area.y_range()),
            0.0,
            color::border(),
        );

        let curves = curves(clip);
        let shown = match &g.drag {
            Some(d) => dragged(d, clip, &self.keys.selected),
            None => clip.keyframes.clone(),
        };
        let mods = ui.input(|i| i.modifiers);
        let origin = ui.input(|i| i.pointer.press_origin());
        let mut edit = None;
        let mut centers = Vec::new();
        let lp = painter.with_clip_rect(legend);
        for (i, curve) in curves.iter().enumerate() {
            let ink = color::CLIPS[i % color::CLIPS.len()];
            let row = legend.top() + (i as f32 + 0.5) * size::CONTROL;
            let x = legend.left() + space::INSET;
            lp.rect_filled(
                Rect::from_x_y_ranges(x..=x + 8.0, row - 1.0..=row + 1.0),
                1.0,
                ink,
            );
            lp.text(
                pos2(x + 8.0 + space::S8, row),
                Align2::LEFT_CENTER,
                label(clip, curve),
                theme::body(),
                color::ink_2(),
            );

            // Sampled every 2pt, so holds and overshoot draw as they play.
            let keys = shown.get(curve.prop).map_or(&[][..], |v| v);
            let pts: Vec<Pos2> = (0..=(plot.width() / 2.0) as usize)
                .filter_map(|s| {
                    let x = plot.left() + s as f32 * 2.0;
                    let v = part(value_at(keys, view.time(x))?, curve.part);
                    Some(pos2(x, g.y(plot, curve.norm(v))))
                })
                .collect();
            pp.add(Shape::line(pts, Stroke::new(1.5, ink)));

            // The selected keys' bezier handles, hidden while keys move. Before the keys,
            // so a key under a handle still takes the pointer.
            let moving = matches!(g.drag, Some(Drag::Key { .. }));
            for w in keys.windows(2).filter(|_| !moving) {
                let (a, b) = (&w[0], &w[1]);
                // Linear's own handles sit on its keys; thirds draw the same line.
                let p = match a.easing {
                    Easing::Linear => Some([1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0]),
                    e => crate::easing::points(e),
                };
                let Some(p) = p else { continue };
                let (xa, xb) = (view.x(a.time), view.x(b.time));
                let na = curve.norm(part(a.value, curve.part));
                let nb = curve.norm(part(b.value, curve.part));
                for h in 0..2 {
                    let end = if h == 0 { a } else { b };
                    if !self
                        .keys
                        .selected
                        .contains(&(clip.id, curve.prop.clone(), end.time))
                    {
                        continue;
                    }
                    let anchor = pos2(
                        if h == 0 { xa } else { xb },
                        g.y(plot, if h == 0 { na } else { nb }),
                    );
                    let at = pos2(
                        xa + p[2 * h] as f32 * (xb - xa),
                        g.y(plot, na + p[2 * h + 1] * (nb - na)),
                    );
                    pp.line_segment([anchor, at], Stroke::new(1.0, color::ink_3()));
                    let r = ui
                        .interact(
                            Rect::from_center_size(at, vec2(HIT, HIT)),
                            id.with(("handle", curve.prop, curve.part, a.time.0, h)),
                            Sense::drag(),
                        )
                        .on_hover_cursor(egui::CursorIcon::Grab);
                    let fill = if r.hovered() || r.dragged() {
                        color::ink()
                    } else {
                        color::ink_2()
                    };
                    pp.circle_filled(at, 4.0, fill);
                    if r.dragged()
                        && let Some(q) = r.interact_pointer_pos()
                    {
                        let x = f64::from((q.x - xa) / (xb - xa));
                        // A flat segment's value doesn't change, so neither does its y.
                        let y = if (nb - na).abs() > 1e-9 {
                            (g.n(plot, q.y) - na) / (nb - na)
                        } else {
                            p[2 * h + 1]
                        };
                        g.drag = Some(Drag::Handle {
                            prop: curve.prop.clone(),
                            time: a.time,
                            points: crate::easing::drag(p, h, x, y),
                        });
                    }
                    if r.drag_stopped()
                        && let Some(d) = g.drag.take()
                    {
                        edit = self.commit(clip, &d);
                    }
                }
            }

            for k in curve.keys {
                let sel = (clip.id, curve.prop.clone(), k.time);
                let on = self.keys.selected.contains(&sel);
                let (mut t, mut v) = (k.time, part(k.value, curve.part));
                if let Some(Drag::Key {
                    prop,
                    time,
                    part,
                    by,
                    value,
                }) = &g.drag
                    && on
                {
                    t = t + *by;
                    if (prop, *time, *part) == (curve.prop, k.time, curve.part) {
                        v = *value;
                    }
                }
                let c = pos2(view.x(t), g.y(plot, curve.norm(v)));
                centers.push((sel.clone(), c));
                let r = ui.interact(
                    Rect::from_center_size(c, vec2(HIT, HIT)),
                    id.with((curve.prop, curve.part, k.time.0)),
                    Sense::click_and_drag(),
                );
                diamond(
                    &pp,
                    c,
                    if on {
                        color::light_blue()
                    } else {
                        color::ink()
                    },
                    true,
                );
                if r.hovered() || r.dragged() {
                    pp.text(
                        c + vec2(HIT, -HIT),
                        Align2::LEFT_BOTTOM,
                        format!("{v:.3}"),
                        theme::small(),
                        color::ink(),
                    );
                }
                if r.clicked() || r.drag_started() {
                    self.keys.pick_clip(clip.id);
                    self.selected.clear();
                    if mods.shift || mods.command {
                        if r.clicked() && !self.keys.selected.remove(&sel) {
                            self.keys.selected.insert(sel.clone());
                        }
                    } else if !on {
                        self.keys.selected = BTreeSet::from([sel.clone()]);
                    }
                    if r.drag_started() {
                        self.keys.selected.insert(sel);
                    }
                }
                if r.dragged()
                    && let (Some(o), Some(p)) = (origin, r.interact_pointer_pos())
                {
                    // In time the key lands on a frame and no key goes before source 0.
                    let to = k.time + (view.time(p.x) - view.time(o.x));
                    let to = rate.time_floor(rate.index_nearest(to));
                    let picked = self.keys.selected.iter().filter(|s| s.0 == clip.id);
                    let floor = picked.map(|s| s.2).min().map_or(Time(0), |m| Time(0) - m);
                    let (a, b) = curve.range;
                    g.drag = Some(Drag::Key {
                        prop: curve.prop.clone(),
                        time: k.time,
                        part: curve.part,
                        by: (to - k.time).max(floor),
                        value: part(k.value, curve.part)
                            + (g.n(plot, p.y) - g.n(plot, o.y)) * (b - a),
                    });
                }
                if r.drag_stopped()
                    && let Some(d) = g.drag.take()
                {
                    edit = self.commit(clip, &d);
                }
            }
        }

        // Click empty space to deselect, drag it to box-select.
        if bg.clicked() && !mods.shift && !mods.command {
            self.keys.selected.clear();
        }
        if let (Some(a), Some(b)) = (origin, bg.interact_pointer_pos())
            && (bg.dragged() || bg.drag_stopped())
        {
            let band = Rect::from_two_pos(a, b);
            if bg.drag_stopped() {
                if !mods.shift && !mods.command {
                    self.keys.selected.clear();
                }
                self.keys.pick_clip(clip.id);
                self.selected.clear();
                let hit = centers.into_iter().filter(|(_, c)| band.contains(*c));
                self.keys.selected.extend(hit.map(|h| h.0));
            } else {
                pp.rect_filled(band, 0.0, color::blue().gamma_multiply(0.15));
                pp.rect_stroke(
                    band,
                    0.0,
                    Stroke::new(1.0, color::light_blue()),
                    egui::StrokeKind::Inside,
                );
            }
        }
        if !ui.input(|i| i.pointer.any_down()) {
            g.drag = None;
        }
        (edit, open)
    }

    /// Drag `d` on `clip` as one undo step, if it changed anything; picked keys follow.
    fn commit(&mut self, clip: &Clip, d: &Drag) -> Option<Edit> {
        let keys = dragged(d, clip, &self.keys.selected);
        if keys == clip.keyframes {
            return None;
        }
        if let Drag::Key { by, .. } = d {
            self.keys.moved(clip.id, *by);
        }
        Some((
            Command::SetClipProps(vec![(clip.id, clip.transform.clone(), keys)]),
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use egui::pos2;
    use egui_kittest::kittest::Queryable;
    use jvid_core::history::History;
    use jvid_core::model::Project;
    use jvid_core::time::FLICKS_PER_SECOND as S;

    use super::super::keyframes::LANE_H;
    use super::super::tests::{harness, project};
    use super::*;

    fn keyed(keys: &[(&str, i64, Value)]) -> (History, ClipId) {
        let mut h = History::new(project(1));
        let id = h.project().sequence().tracks()[0].clips()[0].id;
        for (param, t, value) in keys {
            h.apply(Command::AddKeyframe {
                id,
                param: (*param).into(),
                key: Keyframe {
                    time: Time(t * S / 2),
                    value: *value,
                    easing: Easing::Linear,
                },
            })
            .unwrap();
        }
        (h, id)
    }

    #[test]
    fn each_number_gets_its_own_range_and_a_key_drag_is_one_step() {
        let (mut h, id) = keyed(&[
            ("position", 0, Value::Vec2([0.0, 5.0])),
            ("position", 2, Value::Vec2([100.0, 5.0])),
        ]);
        let clip = |h: &History| h.project().sequence().clip(id).unwrap().1.clone();
        let c = clip(&h);
        let ranges: Vec<_> = curves(&c).iter().map(|c| (c.part, c.range)).collect();
        assert_eq!(
            ranges,
            [(0, (0.0, 100.0)), (1, (4.0, 6.0))],
            "flat sits mid-height"
        );
        assert_eq!(label(&c, &curves(&c)[1]), "Position Y");

        // The later key moves back half a second and its x drops; y stays.
        let mut t = Timeline::default();
        t.keys.selected.insert((id, "position".into(), Time(S)));
        let d = Drag::Key {
            prop: "position".into(),
            time: Time(S),
            part: 0,
            by: Time(-S / 2),
            value: 40.0,
        };
        let (cmd, gesture) = t.commit(&c, &d).unwrap();
        assert_eq!(gesture, None);
        assert!(
            t.keys
                .selected
                .contains(&(id, "position".into(), Time(S / 2)))
        );
        h.apply(cmd).unwrap();
        let k = clip(&h).keyframes["position"][1];
        assert_eq!((k.time, k.value), (Time(S / 2), Value::Vec2([40.0, 5.0])));
        h.undo();
        assert_eq!(clip(&h), c);
        let still = Drag::Key {
            prop: "position".into(),
            time: Time(S),
            part: 0,
            by: Time(0),
            value: 100.0,
        };
        assert!(
            t.commit(&c, &still).is_none(),
            "a drag back to the start is no edit"
        );
    }

    /// A graph open on one clip keyed 0 at 0s and 1 at 1s, opened from its lanes' menu.
    fn opened() -> (egui_kittest::Harness<'static, (Timeline, History)>, ClipId) {
        let (p, id) = keyed(&[
            ("opacity", 0, Value::Scalar(0.0)),
            ("opacity", 2, Value::Scalar(1.0)),
        ]);
        let height = p.project().sequence().tracks()[0].height;
        let mut h = harness(p.project().clone());
        h.state_mut().0.keys.expanded.insert(id);
        for _ in 0..3 {
            h.state_mut().0.action("zoom_in", &Project::default());
        }
        h.run();
        let lock = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
        let y = lock - height / 2.0 + height + 2.0 + LANE_H / 2.0;
        let x = h.state().0.view(8.0 + size::TRACK_HEADER).x(Time(S / 2));
        h.hover_at(pos2(x, y));
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: pos2(x, y),
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: Default::default(),
            });
            h.run();
        }
        h.get_by_label("Open Graph Editor").click();
        h.run();
        h.run();
        assert!(h.state().0.keys.graph.is_some());
        (h, id)
    }

    /// Screen point of source time `t` and normalised value `n`.
    fn at(h: &egui_kittest::Harness<'static, (Timeline, History)>, t: f64, n: f64) -> Pos2 {
        let close = h.get_by_label("Close Graph Editor").rect();
        let plot = Rect::from_x_y_ranges(close.right() + space::S8..=992.0, close.bottom()..=192.0);
        let g = h.state().0.keys.graph.as_ref().unwrap();
        let x = plot.left() + ((t - g.scroll) * g.zoom) as f32;
        pos2(x, g.y(plot, n))
    }

    fn drag(h: &mut egui_kittest::Harness<'static, (Timeline, History)>, from: Pos2, to: Pos2) {
        h.hover_at(from);
        h.run();
        h.drag_at(from);
        h.run();
        for p in [from + vec2(4.0, 4.0), to] {
            h.hover_at(p);
            h.run();
        }
        h.drop_at(to);
        h.run();
    }

    fn opacity(
        h: &egui_kittest::Harness<'static, (Timeline, History)>,
        id: ClipId,
    ) -> Vec<Keyframe> {
        h.state()
            .1
            .project()
            .sequence()
            .clip(id)
            .unwrap()
            .1
            .keyframes["opacity"]
            .clone()
    }

    #[test]
    fn dragging_a_key_moves_time_and_value_and_undoes_in_one_step() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let (mut h, id) = opened();
        h.snapshot("timeline_graph");
        let (from, to) = (at(&h, 1.0, 1.0), at(&h, 0.5, 0.5));
        drag(&mut h, from, to);
        let k = opacity(&h, id);
        assert_eq!(k[1].time, Time(S / 2), "lands on the half-second frame");
        let Value::Scalar(v) = k[1].value else {
            panic!()
        };
        assert!((v - 0.5).abs() < 0.02, "{v}");
        assert_eq!(k[0].time, Time(0), "unpicked keys stay");
        assert!(h.state_mut().1.undo());
        let k = opacity(&h, id);
        assert_eq!((k[1].time, k[1].value), (Time(S), Value::Scalar(1.0)));
        assert!(!h.state().1.can_undo(), "one step");
    }

    #[test]
    fn dragging_a_handle_sets_the_matching_bezier_and_undoes() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let (mut h, id) = opened();
        let key = at(&h, 0.0, 0.0);
        drag(&mut h, key, key);
        assert!(
            h.state()
                .0
                .keys
                .selected
                .contains(&(id, "opacity".into(), Time(0)))
        );
        // Linear's out handle shows a third of the way along.
        let (from, to) = (at(&h, 1.0 / 3.0, 1.0 / 3.0), at(&h, 0.5, 0.0));
        drag(&mut h, from, to);
        let Easing::Bezier(p) = opacity(&h, id)[0].easing else {
            panic!("{:?}", opacity(&h, id)[0].easing)
        };
        let want = [0.5, 0.0, 2.0 / 3.0, 2.0 / 3.0];
        assert!(
            p.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.02),
            "{p:?}"
        );
        assert!(h.state_mut().1.undo());
        assert_eq!(opacity(&h, id)[0].easing, Easing::Linear);
        assert!(!h.state().1.can_undo(), "one step");

        // Close goes back to the lanes.
        h.get_by_label("Close Graph Editor").click();
        h.run();
        assert!(h.state().0.keys.graph.is_none());
    }
}
