//! Keyframe lanes: a clip with keyframes shows a toggle in its name strip that expands one
//! lane per animated property under it. Keys click and marquee select, drag (snapping to
//! frames, the playhead, edges and markers), delete, copy and paste at the playhead, and
//! open the easing editor on right-click. Edits are `SetClipProps`, as in the inspector.
//! The lanes' right-click menu opens the curve graph editor ([`super::graph`]).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use egui::{Id, Painter, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use jvid_core::history::{Command, GestureKey};
use jvid_core::keyframe::{Easing, Keyframe, SPEED, Value};
use jvid_core::model::{Clip, ClipId, Sequence, Track};
use jvid_core::time::Time;
use suite_ui::theme;
use suite_ui::tokens::color;

use crate::i18n::{CATALOG, tr};

use super::{Edit, Timeline, View};

/// Height of one property lane.
pub const LANE_H: f32 = 18.0;
/// Diamond half-size.
const KEY_R: f32 = 5.0;
/// Undo gesture for easing handle drags, xored with the clip and drag count.
const EASING_GESTURE: GestureKey = 0x6b66_6561_0000; // "kfea"

#[derive(Default)]
pub struct Keys {
    /// Clips showing their lanes.
    pub expanded: HashSet<ClipId>,
    /// Selected keyed clips already expanded once, so the toggle can collapse them again.
    auto: HashSet<ClipId>,
    /// Selected keys by clip, property and source time.
    pub selected: BTreeSet<(ClipId, String, Time)>,
    /// Copied keys with times relative to the earliest.
    pub clipboard: Vec<(String, Keyframe)>,
    /// Clipboard target metadata, retained even when the source clip is removed.
    clipboard_source: Option<Clip>,
    /// The focused key editor, cleared when clip or empty-lane selection takes focus.
    clip: Option<ClipId>,
    /// Timeline shift of the selected keys while they're dragged.
    pub(super) drag: Option<Time>,
    /// The curve graph editor, while it covers the lanes.
    pub(super) graph: Option<super::graph::Graph>,
}

/// The properties of `c` that have keyframes, in lane order.
pub(super) fn lanes(c: &Clip) -> impl Iterator<Item = (&String, &Vec<Keyframe>)> {
    c.keyframes.iter().filter(|(_, k)| !k.is_empty())
}

/// Lane `name`'s caption: a built-in property's `lane.<name>`, an effect's parameter after the
/// effect's name (`Film Grain · Amount`), anything else as stored in Title Case.
pub(super) fn caption(c: &Clip, name: &str) -> String {
    use crate::effects;
    if let Some(s) = CATALOG.get(&format!("lane.{name}")) {
        return s.to_owned();
    }
    if let Some((i, id)) = crate::inspector::fx_param(name)
        && let Some(fx) = c.transform.effects.get(i)
    {
        let (title, shipped) = effects::with(|l| {
            let shipped = l.shipped(&fx.shader);
            match l.get(&fx.shader) {
                Some(Ok(s)) => (effects::name(&fx.shader, shipped, s), shipped),
                _ => (fx.shader.clone(), shipped),
            }
        });
        return format!("{title} · {}", effects::param(&fx.shader, shipped, id));
    }
    effects::title_case_name(&name.replace(['_', '.'], " "))
}

/// Inserts `k`, replacing any key at the same time.
fn put(keys: &mut Vec<Keyframe>, k: Keyframe) {
    match keys.binary_search_by_key(&k.time, |x| x.time) {
        Ok(i) => keys[i] = k,
        Err(i) => keys.insert(i, k),
    }
}

/// `c`'s props after `f` rewrites its keyframes, as one undo step.
fn rewrite(c: &Clip, f: impl FnOnce(&mut BTreeMap<String, Vec<Keyframe>>)) -> Command {
    let mut keys = c.keyframes.clone();
    f(&mut keys);
    keys.retain(|_, k| !k.is_empty());
    Command::SetClipProps(vec![(c.id, c.transform.clone(), keys)])
}

fn picked<'a>(
    sel: &'a BTreeSet<(ClipId, String, Time)>,
    c: &'a Clip,
) -> impl Iterator<Item = (&'a String, Time)> {
    sel.iter().filter(move |s| s.0 == c.id).map(|s| (&s.1, s.2))
}

/// Drags the selected keys of `c` `delta` along the timeline, each to the source time the clip
/// then shows there; they replace keys they land on. A speed key retimes the clip after it, so
/// speed keys go first, in timeline order, each found on the clip it retimes. Returns the edit
/// and the keys where they landed.
fn shift(
    c: &Clip,
    sel: &BTreeSet<(ClipId, String, Time)>,
    delta: Time,
) -> (Command, BTreeSet<(ClipId, String, Time)>) {
    let mut x = c.clone();
    let mut moved: Vec<_> = picked(sel, c)
        .filter_map(|(p, t)| {
            let v = x.keyframes.get_mut(p)?;
            let i = v.iter().position(|k| k.time == t)?;
            Some((p.clone(), v.remove(i)))
        })
        .collect();
    moved.sort_by_key(|(p, k)| (p != SPEED, c.timeline_at(k.time)));
    let mut landed = BTreeSet::new();
    for (p, k) in moved {
        let to = c.timeline_at(k.time) + delta;
        let time = match () {
            // A freeze shows every key at its start, so its keys move in source time.
            _ if c.freeze.is_some() => (k.time + delta).max(Time(0)),
            _ if p == SPEED => land(&x, k, to),
            _ => x.source_beyond(to),
        };
        put(
            x.keyframes.entry(p.clone()).or_default(),
            Keyframe { time, ..k },
        );
        landed.insert((c.id, p, time));
    }
    (rewrite(c, |keys| *keys = x.keyframes), landed)
}

/// The source time where speed key `k`, put there on `c`, shows at timeline time `to`. The key
/// retimes the clip as it moves, so it is searched for: its timeline time rises with its source
/// time, or falls on a reversed clip.
fn land(c: &Clip, k: Keyframe, to: Time) -> Time {
    let at = |s: Time| {
        let mut x = c.clone();
        put(
            x.keyframes.entry(SPEED.into()).or_default(),
            Keyframe { time: s, ..k },
        );
        x.timeline_at(s)
    };
    // True below the answer, and up to it on a reversed clip.
    let early = |s: Time| (at(s) < to) != c.reverse;
    let mut hi = c.source_beyond(to).max(Time(1));
    while early(hi) && hi.0 < i64::MAX / 4 {
        hi = hi + hi;
    }
    let (mut lo, mut hi) = (0, hi.0);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if early(Time(mid)) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Time(lo - i64::from(c.reverse)).max(Time(0))
}

/// Shifts the selected keys of `c` by `delta` in source time, as the graph editor drags them;
/// they replace keys they land on. `keys` is a copy of `c`'s.
pub(super) fn shift_keys(
    keys: &mut BTreeMap<String, Vec<Keyframe>>,
    c: &Clip,
    sel: &BTreeSet<(ClipId, String, Time)>,
    delta: Time,
) {
    let moved: Vec<_> = picked(sel, c)
        .filter_map(|(p, t)| {
            let v = keys.get_mut(p)?;
            let i = v.iter().position(|k| k.time == t)?;
            Some((p.clone(), v.remove(i)))
        })
        .collect();
    for (p, mut k) in moved {
        k.time = k.time + delta;
        put(keys.entry(p).or_default(), k);
    }
}

fn delete(c: &Clip, sel: &BTreeSet<(ClipId, String, Time)>) -> Command {
    rewrite(c, |keys| {
        for (p, t) in picked(sel, c) {
            if let Some(v) = keys.get_mut(p) {
                v.retain(|k| k.time != t);
            }
        }
    })
}

fn ease(c: &Clip, sel: &BTreeSet<(ClipId, String, Time)>, e: Easing) -> Command {
    rewrite(c, |keys| {
        for (p, t) in picked(sel, c) {
            for k in keys.get_mut(p).into_iter().flatten() {
                if k.time == t {
                    k.easing = e;
                }
            }
        }
    })
}

/// The selected keys of `c`, timed from the earliest.
fn copy(c: &Clip, sel: &BTreeSet<(ClipId, String, Time)>) -> Vec<(String, Keyframe)> {
    let keys: Vec<_> = picked(sel, c)
        .filter_map(|(p, t)| {
            let k = c.keyframes.get(p)?.iter().find(|k| k.time == t)?;
            Some((p.clone(), *k))
        })
        .collect();
    let first = keys.iter().map(|k| k.1.time).min().unwrap_or_default();
    keys.into_iter()
        .map(|(p, k)| {
            (
                p,
                Keyframe {
                    time: k.time - first,
                    ..k
                },
            )
        })
        .collect()
}

/// `copied` onto `c` with the earliest at timeline time `at`, but no earlier than source 0.
fn paste(c: &Clip, copied: &[(String, Keyframe)], at: Time) -> Command {
    let base = c.source_at(at).max(Time(0));
    rewrite(c, |keys| {
        for (p, k) in copied {
            let time = k.time + base;
            put(keys.entry(p.clone()).or_default(), Keyframe { time, ..*k });
        }
        // The clipboard can outlive an Inspector scale edit, or come from a clip that
        // already uses independent axes. Promote both retained and incoming scalar keys
        // together so Value::lerp never encounters mixed scale variants.
        if let Some(scale) = keys.get_mut("scale")
            && scale.iter().any(|k| matches!(k.value, Value::Vec2(_)))
        {
            for k in scale {
                if let Value::Scalar(s) = k.value {
                    k.value = Value::Vec2([s; 2]);
                }
            }
        }
    })
}

/// Validate property kinds and structural targets before offering a key paste. Effect
/// slots must refer to the same shader, and mask points must exist on the destination.
fn supports_key(c: &Clip, source: Option<&Clip>, name: &str, value: Value) -> bool {
    use jvid_core::model::{Curves, Secondary, Text, Wheels};
    if let Some((i, p)) = crate::inspector::fx_param(name) {
        return matches!(value, Value::Scalar(_))
            && source
                .and_then(|s| s.transform.effects.get(i))
                .zip(c.transform.effects.get(i))
                .is_some_and(|(a, b)| a.shader == b.shader && b.params.contains_key(p));
    }
    if let Some((i, p)) = crate::inspector::mask_param(name) {
        let Some(mask) = c.transform.masks.get(i) else {
            return false;
        };
        return match p {
            "feather" | "opacity" => matches!(value, Value::Scalar(_)),
            p => p
                .strip_prefix('p')
                .and_then(|n| n.parse::<usize>().ok())
                .is_some_and(|n| n < mask.points.len() && matches!(value, Value::Vec2(_))),
        };
    }
    if Curves::NAMES.contains(&name) {
        return matches!(value, Value::Curve(_));
    }
    if Wheels::NAMES.contains(&name) {
        return matches!(value, Value::Vec2(_));
    }
    if Secondary::KEYS.contains(&name) || Wheels::KEYS.contains(&name) {
        return matches!(value, Value::Scalar(_));
    }
    if let Some(expected) = Text::default().get(name) {
        // Sequence-only action checks cannot resolve an asset's text style. Retain text
        // keys on the same asset rather than silently animating an unrelated media type.
        return source.is_some_and(|s| s.asset == c.asset)
            && std::mem::discriminant(&value) == std::mem::discriminant(&expected);
    }
    match name {
        "position" | "anchor" => matches!(value, Value::Vec2(_)),
        "scale" => matches!(value, Value::Scalar(_) | Value::Vec2(_)),
        "color" => matches!(value, Value::Color(_)) && source.is_some_and(|s| s.asset == c.asset),
        SPEED => matches!(value, Value::Scalar(v) if jvid_core::keyframe::SPEEDS.contains(&v)),
        "rotation" | "opacity" | "crop_left" | "crop_top" | "crop_right" | "crop_bottom"
        | "volume" | "pan" | "exposure" | "contrast" | "saturation" | "temperature" | "tint"
        | "lut_intensity" => matches!(value, Value::Scalar(_)),
        _ => false,
    }
}

impl Keys {
    pub(super) fn clear_focus(&mut self) {
        self.clip = None;
        self.selected.clear();
    }

    pub(super) fn clear_clipboard(&mut self) {
        self.clipboard.clear();
        self.clipboard_source = None;
    }

    /// Picks keys on `clip`, dropping any picked on other clips: edits go to one clip.
    pub(super) fn pick_clip(&mut self, clip: ClipId) {
        self.clip = Some(clip);
        self.selected.retain(|s| s.0 == clip);
    }

    /// Follows `clip`'s selected keys after they shift by `d`.
    pub(super) fn moved(&mut self, clip: ClipId, d: Time) {
        let s = std::mem::take(&mut self.selected);
        self.selected = s
            .into_iter()
            .map(|(c, p, t)| if c == clip { (c, p, t + d) } else { (c, p, t) })
            .collect();
    }

    /// Extra height under `track`'s clips for its tallest expanded clip; none while locked.
    pub fn extra(&self, track: &Track) -> f32 {
        if track.locked {
            return 0.0;
        }
        let open = track
            .clips()
            .iter()
            .filter(|c| self.expanded.contains(&c.id));
        let n = open.map(|c| lanes(c).count()).max().unwrap_or(0);
        n as f32 * LANE_H
    }
}

impl Timeline {
    /// Whether the curve graph editor is open.
    pub fn graph_open(&self) -> bool {
        self.keys.graph.is_some()
    }

    /// Opens the curve graph editor on `clip`, as the lanes' right-click menu does.
    pub fn open_graph(&mut self, clip: ClipId) {
        self.keys.graph = Some(super::graph::Graph::new(clip));
    }

    fn key_paste_clip<'a>(&self, seq: &'a Sequence) -> Option<&'a Clip> {
        let id = if self.selected.len() == 1 {
            *self.selected.iter().next()?
        } else if self.selected.is_empty() {
            self.keys
                .graph
                .as_ref()
                .map(|g| g.clip)
                .or(self.keys.clip)?
        } else {
            return None;
        };
        let (track, c) = seq.clip(id)?;
        if seq.track(track)?.locked
            || !self
                .keys
                .clipboard
                .iter()
                .all(|(p, k)| supports_key(c, self.keys.clipboard_source.as_ref(), p, k.value))
        {
            return None;
        }
        Some(c)
    }

    /// Some means the key editor owns this action, even when an invalid destination
    /// disables it; None lets the clip editor handle it. Menus and shortcuts share this.
    pub(super) fn key_capability(&self, id: &str, seq: &Sequence) -> Option<bool> {
        match id {
            "paste" if !self.keys.clipboard.is_empty() => Some(self.key_paste_clip(seq).is_some()),
            "copy" | "cut" | "delete"
                if self.selected.is_empty()
                    || (self.selected.len() == 1
                        && self.keys.clip.is_some_and(|id| self.selected.contains(&id))) =>
            {
                let (track, c) = seq.clip(self.keys.clip?)?;
                picked(&self.keys.selected, c)
                    .any(|(p, t)| {
                        c.keyframes
                            .get(p)
                            .is_some_and(|keys| keys.iter().any(|k| k.time == t))
                    })
                    .then(|| id == "copy" || seq.track(track).is_some_and(|t| !t.locked))
            }
            _ => None,
        }
    }

    /// Delete, copy, cut and paste for keys; false leaves the action to clips.
    pub(super) fn key_action(&mut self, id: &str, seq: &Sequence) -> bool {
        match self.key_capability(id, seq) {
            None => return false,
            Some(false) => return true,
            Some(true) => {}
        }
        if id == "paste" {
            let c = self.key_paste_clip(seq).expect("capability checked");
            self.pending = Some(paste(c, &self.keys.clipboard, self.playhead()));
        } else {
            let c = seq
                .clip(self.keys.clip.expect("capability checked"))
                .expect("checked")
                .1;
            if id != "delete" {
                self.keys.clipboard = copy(c, &self.keys.selected);
                self.keys.clipboard_source = Some(c.clone());
                self.clipboard.clear();
                self.copied = true;
            }
            if id != "copy" {
                self.pending = Some(delete(c, &self.keys.selected));
                self.keys.selected.clear();
            }
        }
        true
    }

    /// The expand toggle in `clip`'s name strip, when it has keyframes, pinned to the strip's
    /// visible left edge. Selecting a keyed clip, or keying a selected one, expands it once;
    /// collapsed, its key times show as ticks on the body.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn key_toggle(
        &mut self,
        ui: &Ui,
        painter: &Painter,
        view: View,
        clip: &Clip,
        r: Rect,
        id: Id,
    ) {
        let keyed = lanes(clip).next().is_some();
        if keyed && self.selected.contains(&clip.id) {
            if self.keys.auto.insert(clip.id) {
                self.keys.expanded.insert(clip.id);
            }
        } else {
            self.keys.auto.remove(&clip.id);
        }
        if !keyed || r.width() < 24.0 {
            return;
        }
        let open = self.keys.expanded.contains(&clip.id);
        let x = r
            .left()
            .max(painter.clip_rect().left())
            .min(r.right() - 20.0)
            + 10.0;
        let c = pos2(x, r.top() + super::NAME_STRIP / 2.0);
        let hit = Rect::from_center_size(c, vec2(16.0, 16.0));
        let resp = ui
            .interact(hit, id.with(("kf toggle", clip.id)), Sense::click())
            .on_hover_text(if open {
                tr!("timeline.hide_keyframes")
            } else {
                tr!("timeline.show_keyframes")
            });
        diamond(
            painter,
            c,
            if open {
                color::light_blue()
            } else {
                color::ink_2()
            },
            open,
        );
        if resp.clicked() && !self.keys.expanded.remove(&clip.id) {
            self.keys.expanded.insert(clip.id);
        }
        if !open {
            let painter = painter.with_clip_rect(r.intersect(painter.clip_rect()));
            let y = r.bottom() - 4.0;
            let times: BTreeSet<Time> = lanes(clip)
                .flat_map(|(_, k)| k.iter().map(|k| k.time))
                .collect();
            for t in times {
                let x = view.x(clip.timeline_at(t));
                let pts = [
                    vec2(0.0, -3.0),
                    vec2(3.0, 0.0),
                    vec2(0.0, 3.0),
                    vec2(-3.0, 0.0),
                ];
                let pts = pts.map(|d| pos2(x, y) + d).to_vec();
                painter.add(Shape::convex_polygon(pts, color::ink(), Stroke::NONE));
            }
        }
    }

    /// Draws and edits `clip`'s lanes in `area`, the strip under it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn key_lanes(
        &mut self,
        ui: &Ui,
        painter: &Painter,
        seq: &Sequence,
        view: View,
        clip: &Clip,
        area: Rect,
        id: Id,
    ) -> Option<Edit> {
        let id = id.with(("kf", clip.id));
        let painter = painter.with_clip_rect(area.intersect(painter.clip_rect()));
        painter.rect_filled(area, 0.0, color::raised());
        let bg = ui.interact(area, id, Sense::click_and_drag());
        let tl = |s: Time| clip.timeline_at(s);
        let mods = ui.input(|i| i.modifiers);
        let mut edit = None;
        let marquee = bg.dragged() || bg.drag_stopped();
        let mut centers = Vec::new();
        let (left, right) = (painter.clip_rect().left(), painter.clip_rect().right());
        let shift_x = self.keys.drag.map_or(0.0, |d| view.x(d) - view.x(Time(0)));
        for (row, (name, keys)) in lanes(clip).enumerate() {
            let y = area.top() + (row as f32 + 0.5) * LANE_H;
            if let (Some(a), Some(b)) = (keys.first(), keys.last()) {
                let (a, b) = (view.x(tl(a.time)), view.x(tl(b.time)));
                painter.line_segment([pos2(a, y), pos2(b, y)], Stroke::new(1.0, color::ink_4()));
            }
            // The caption sits on a plate in the lane's fill, so the key line passes behind it.
            let galley =
                painter.layout_no_wrap(caption(clip, name), theme::small(), color::ink_3());
            let at = pos2(area.left().max(left) + 6.0, y - galley.size().y / 2.0);
            let plate = Rect::from_min_size(at, galley.size()).expand2(vec2(3.0, 1.0));
            painter.rect_filled(plate, 4.0, color::raised());
            painter.galley(at, galley, color::ink_3());
            let on_times: Vec<Time> = picked(&self.keys.selected, clip)
                .filter(|s| s.0 == name)
                .map(|s| s.1)
                .collect();
            for k in keys {
                let on = on_times.contains(&k.time);
                let x = view.x(tl(k.time)) + if on { shift_x } else { 0.0 };
                // Off-screen keys need no hit box, except the one being dragged.
                let dragging = on && self.keys.drag.is_some();
                if (x < left - KEY_R || x > right + KEY_R) && !dragging {
                    continue;
                }
                let sel = || (clip.id, name.clone(), k.time);
                let c = pos2(x, y);
                if marquee {
                    centers.push((sel(), c));
                }
                let r = ui.interact(
                    Rect::from_center_size(c, vec2(2.0 * KEY_R + 2.0, LANE_H)),
                    id.with((name, k.time.0)),
                    Sense::click_and_drag(),
                );
                diamond(
                    &painter,
                    c,
                    if on {
                        color::light_blue()
                    } else {
                        color::ink_2()
                    },
                    true,
                );
                if r.clicked() || r.drag_started() || r.secondary_clicked() {
                    self.keys.pick_clip(clip.id);
                    self.selected.clear();
                    if mods.shift || mods.command {
                        if r.clicked() && !self.keys.selected.remove(&sel()) {
                            self.keys.selected.insert(sel());
                        }
                    } else if !on {
                        self.keys.selected = BTreeSet::from([sel()]);
                    }
                }
                if r.dragged()
                    && let (Some(o), Some(p)) = (
                        ui.input(|i| i.pointer.press_origin()),
                        r.interact_pointer_pos(),
                    )
                {
                    // The grabbed key lands on a frame, then snaps to nearby edges.
                    let rate = self.transport.rate;
                    let to = tl(k.time) + (view.time(p.x) - view.time(o.x));
                    let to = rate.time_floor(rate.index_nearest(to));
                    let to = to + self.snap(seq, &[], &[to]);
                    // No key goes before source 0, which is to the right on a reversed clip.
                    let min = picked(&self.keys.selected, clip).map(|s| s.1).min();
                    let lim = min.map_or(Time(0), |m| tl(Time(0)) - tl(m));
                    let d = to - tl(k.time);
                    let held = if clip.reverse { d.min(lim) } else { d.max(lim) };
                    if held != d {
                        self.snapped = None;
                    }
                    self.keys.drag = Some(held);
                }
                if r.drag_stopped()
                    && let Some(d) = self.keys.drag.take()
                {
                    self.snapped = None;
                    if d != Time(0) {
                        let (cmd, landed) = shift(clip, &self.keys.selected, d);
                        edit = Some((cmd, None));
                        self.keys.selected.retain(|s| s.0 != clip.id);
                        self.keys.selected.extend(landed);
                    }
                }
                r.context_menu(|ui| {
                    let Some((e, dragging)) = crate::easing::editor(ui, k.easing) else {
                        return;
                    };
                    // One undo step per handle drag.
                    let drags = ui.id().with("kf easing drags");
                    let n = ui.data_mut(|d| {
                        let n = d.get_temp_mut_or(drags, (0u64, false));
                        n.0 += u64::from(dragging && !n.1);
                        n.1 = dragging;
                        n.0
                    });
                    let key = EASING_GESTURE ^ clip.id.0 ^ (n << 16);
                    edit = Some((ease(clip, &self.keys.selected, e), dragging.then_some(key)));
                });
            }
        }
        if bg.clicked() && !mods.shift && !mods.command {
            self.keys.selected.clear();
        }
        bg.context_menu(|ui| {
            if ui.button(tr!("timeline.open_graph")).clicked() {
                self.keys.graph = Some(super::graph::Graph::new(clip.id));
                ui.close();
            }
        });
        if let (Some(a), Some(b)) = (
            ui.input(|i| i.pointer.press_origin()),
            bg.interact_pointer_pos(),
        ) && (bg.dragged() || bg.drag_stopped())
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
                painter.rect_filled(band, 0.0, color::blue().gamma_multiply(0.15));
                painter.rect_stroke(
                    band,
                    0.0,
                    Stroke::new(1.0, color::light_blue()),
                    egui::StrokeKind::Inside,
                );
            }
        }
        edit
    }
}

pub(super) fn diamond(painter: &Painter, c: egui::Pos2, fill: egui::Color32, solid: bool) {
    let pts = vec![
        c + vec2(0.0, -KEY_R),
        c + vec2(KEY_R, 0.0),
        c + vec2(0.0, KEY_R),
        c + vec2(-KEY_R, 0.0),
    ];
    if solid {
        painter.add(Shape::convex_polygon(
            pts,
            fill,
            Stroke::new(1.0, color::field()),
        ));
    } else {
        painter.add(Shape::closed_line(pts, Stroke::new(1.0, fill)));
    }
}

#[cfg(test)]
mod tests {
    use jvid_core::history::History;
    use jvid_core::keyframe::Value;
    use jvid_core::model::Transform;
    use jvid_core::time::FLICKS_PER_SECOND as S;

    use super::super::tests::project;
    use super::*;

    #[test]
    fn lane_captions_cover_every_built_in_key_and_read_as_before_in_english() {
        let mut keys = vec![
            "position",
            "scale",
            "rotation",
            "anchor",
            "opacity",
            "crop_left",
            "crop_top",
            "crop_right",
            "crop_bottom",
            "exposure",
            "contrast",
            "saturation",
            "temperature",
            "tint",
            "lut_intensity",
            "volume",
            "pan",
            "speed",
            "color",
        ];
        keys.extend(jvid_core::model::Text::ANIMATED);
        keys.extend(jvid_core::model::Wheels::KEYS);
        keys.extend(jvid_core::model::Curves::NAMES);
        keys.extend(jvid_core::model::Secondary::KEYS);
        let lanes: Vec<&str> = CATALOG
            .keys("en")
            .into_iter()
            .filter_map(|k| k.strip_prefix("lane."))
            .collect();
        for key in &keys {
            assert!(lanes.contains(key), "no lane.{key}");
        }
        let c = project(1).sequence().tracks()[0].clips()[0].clone();
        for key in lanes {
            assert!(keys.contains(&key), "lane.{key} isn't a built-in key");
            let words = caption(&c, key);
            assert!(
                words.split(' ').all(|w| w.starts_with(char::is_uppercase)),
                "lane.{key} = {words:?} isn't Title Case"
            );
        }
        assert_eq!(caption(&c, "crop_bottom"), "Crop Bottom");
        // An effect the clip doesn't have, or any other name, shows as stored in Title Case.
        assert_eq!(caption(&c, "fx0.grain_size"), "Fx0 Grain Size");
        assert_eq!(caption(&c, "mask0.feather"), "Mask0 Feather");
    }

    fn times(h: &History, p: &str) -> Vec<(i64, Easing)> {
        let c = &h.project().sequence().tracks()[0].clips()[1];
        let k = c.keyframes.get(p).map_or(&[][..], |v| v);
        k.iter().map(|k| (k.time.0 / (S / 10), k.easing)).collect()
    }

    #[test]
    fn keys_move_ease_delete_copy_and_paste_as_undoable_props() {
        let mut h = History::new(project(4));
        let c = h.project().sequence().tracks()[0].clips()[1].clone();
        for (p, t) in [("opacity", 0), ("opacity", 5), ("scale", 5), ("scale", 10)] {
            h.apply(Command::AddKeyframe {
                id: c.id,
                param: p.into(),
                key: Keyframe {
                    time: Time(t * S / 10),
                    value: Value::Scalar(1.0),
                    easing: Easing::Linear,
                },
            })
            .unwrap();
        }
        let c = |h: &History| h.project().sequence().tracks()[0].clips()[1].clone();
        let mut keys = Keys::default();
        keys.expanded.insert(c(&h).id);
        assert_eq!(
            keys.extra(&h.project().sequence().tracks()[0]),
            2.0 * LANE_H
        );
        let sel = |p: &str, t| (c(&h).id, p.to_owned(), Time(t * S / 10));
        keys.selected = BTreeSet::from([sel("opacity", 0), sel("scale", 5)]);

        // Moving onto an existing key replaces it.
        let (cmd, _) = shift(&c(&h), &keys.selected, Time(5 * S / 10));
        h.apply(cmd).unwrap();
        assert_eq!(times(&h, "opacity"), [(5, Easing::Linear)]);
        assert_eq!(times(&h, "scale"), [(10, Easing::Linear)]);
        h.undo();
        assert_eq!(times(&h, "opacity").len(), 2);

        let cmd = ease(&c(&h), &keys.selected, Easing::Hold);
        h.apply(cmd).unwrap();
        assert_eq!(
            times(&h, "scale"),
            [(5, Easing::Hold), (10, Easing::Linear)]
        );

        // Copied relative to the earliest, pasted at the playhead (clip starts at 2s).
        let copied = copy(&c(&h), &keys.selected);
        h.apply(paste(&c(&h), &copied, Time(3 * S))).unwrap();
        assert_eq!(
            times(&h, "opacity"),
            [(0, Easing::Hold), (5, Easing::Linear), (10, Easing::Hold)]
        );
        assert_eq!(
            times(&h, "scale"),
            [(5, Easing::Hold), (10, Easing::Linear), (15, Easing::Hold)]
        );

        h.apply(delete(&c(&h), &keys.selected)).unwrap();
        assert_eq!(
            times(&h, "scale"),
            [(10, Easing::Linear), (15, Easing::Hold)]
        );
    }

    /// On a speed ramp a dragged key lands on the frame it was dropped on, forwards and
    /// reversed: a speed key too, though moving it retimes the clip (#82).
    #[test]
    fn dragged_keys_land_on_the_frame_they_were_dropped_on_over_a_ramp() {
        use jvid_core::time::Rate;
        let rate = Rate::new(30, 1);
        for reverse in [false, true] {
            let mut h = History::new(project(1));
            let id = h.project().sequence().tracks()[0].clips()[0].id;
            let speed = 1.0;
            h.apply(Command::SetClipSpeed {
                id,
                speed,
                reverse,
                ripple: true,
            })
            .unwrap();
            let keys = [(SPEED, 0, 1.0), (SPEED, 10, 3.0), (SPEED, 20, 0.5)];
            for (param, t, v) in keys.into_iter().chain([("volume", 15, 1.0)]) {
                let key = Keyframe {
                    time: Time(t * S / 10),
                    value: Value::Scalar(v),
                    easing: Easing::EaseInOut,
                };
                let param = param.into();
                h.apply(Command::AddKeyframe { id, param, key }).unwrap();
            }
            // Drags `param`'s key at `s` by `frames` and checks it lands on that frame.
            let drag = |h: &mut History, param: &str, s: Time, frames: i64| {
                let c = |h: &History| h.project().sequence().tracks()[0].clips()[0].clone();
                let from = c(h).timeline_at(s);
                let to = rate.time_floor(rate.index_nearest(from) + frames);
                let sel = BTreeSet::from([(id, param.to_owned(), s)]);
                let (cmd, landed) = shift(&c(h), &sel, to - from);
                h.apply(cmd).unwrap();
                let [(_, p, s)] = <[_; 1]>::try_from(Vec::from_iter(landed)).unwrap();
                assert!(p == param && c(h).keyframes[param].iter().any(|k| k.time == s));
                let at = c(h).timeline_at(s);
                assert_eq!(
                    rate.index_floor(at),
                    rate.index_floor(to),
                    "{param} {reverse}"
                );
                s
            };
            let s = drag(&mut h, SPEED, Time(S), 7);
            drag(&mut h, "volume", Time(15 * S / 10), -5);
            drag(&mut h, SPEED, s, -9);
        }
    }

    #[test]
    fn delete_and_paste_actions_go_to_keys_when_some_are_picked() {
        let mut h = History::new(project(2));
        let c = h.project().sequence().tracks()[0].clips()[0].clone();
        h.apply(Command::AddKeyframe {
            id: c.id,
            param: "opacity".into(),
            key: Keyframe {
                time: Time(0),
                value: Value::Scalar(1.0),
                easing: Easing::Linear,
            },
        })
        .unwrap();
        let mut t = Timeline::default();
        t.sync(h.project());
        t.selected.insert(c.id);
        t.keys.clip = Some(c.id);
        t.keys.selected.insert((c.id, "opacity".into(), Time(0)));
        assert!(t.action("copy", h.project()));
        assert!(t.action("delete", h.project()));
        h.apply(t.pending.take().unwrap()).unwrap();
        let clip = |h: &History| h.project().sequence().tracks()[0].clips()[0].clone();
        assert!(
            clip(&h).keyframes.is_empty(),
            "the clip stays, its key goes"
        );
        t.seek(Time(S));
        assert!(t.action("paste", h.project()));
        h.apply(t.pending.take().unwrap()).unwrap();
        assert_eq!(clip(&h).keyframes["opacity"][0].time, Time(S));
    }

    #[test]
    fn expanded_lanes_drag_a_key_and_snapshot() {
        use egui::pos2;
        use egui_kittest::kittest::Queryable;
        use jvid_core::model::Project;
        use suite_ui::tokens::size;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut p = History::new(project(1));
        let id = p.project().sequence().tracks()[0].clips()[0].id;
        for (param, t) in [("opacity", 0), ("opacity", 1), ("scale", 1)] {
            p.apply(Command::AddKeyframe {
                id,
                param: param.into(),
                key: Keyframe {
                    time: Time(t * S),
                    value: Value::Scalar(1.0),
                    easing: Easing::Linear,
                },
            })
            .unwrap();
        }
        let height = p.project().sequence().tracks()[0].height;
        let mut h = super::super::tests::harness(p.project().clone());
        h.state_mut().0.keys.expanded.insert(id);
        for _ in 0..3 {
            h.state_mut().0.action("zoom_in", &Project::default());
        }
        h.run();
        h.snapshot("timeline_keyframes");
        // The opacity lane is the first under the clip; the panel sits 8pt in.
        let lock = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
        let y = lock - height / 2.0 + height + 2.0 + LANE_H / 2.0;
        let view = h.state().0.view(8.0 + size::TRACK_HEADER);
        // The opacity line runs from the clip's start, but stops at its caption's plate.
        let img = h.render().expect("render");
        let words = view.x(Time(0)) + 10.0;
        let through = (0..20).any(|dx| {
            (-3..=3).any(|dy| {
                let p = img.get_pixel((words + dx as f32) as u32, (y + dy as f32) as u32);
                egui::Color32::from_rgb(p[0], p[1], p[2]) == suite_ui::tokens::color::ink_4()
            })
        });
        assert!(!through, "the key line shows through the caption");
        let (x, dx) = (view.x(Time(S)), view.x(Time(2 * S)) - view.x(Time(S)));
        h.hover_at(pos2(x, y));
        h.run();
        h.drag_at(pos2(x, y));
        h.run();
        for d in [5.0, dx + 1.0] {
            h.hover_at(pos2(x + d, y));
            h.run();
        }
        h.drop_at(pos2(x + dx + 1.0, y));
        h.run();
        let c = h.state().1.project().sequence().tracks()[0].clips()[0].clone();
        let t: Vec<_> = c.keyframes["opacity"].iter().map(|k| k.time).collect();
        assert_eq!(t, [Time(0), Time(2 * S)], "lands on the 2s frame");
        assert_eq!(c.keyframes["scale"][0].time, Time(S), "unpicked keys stay");
        h.state_mut().1.undo();
        let c = h.state().1.project().sequence().tracks()[0].clips()[0].clone();
        assert_eq!(c.keyframes["opacity"][1].time, Time(S));
    }

    fn keyed(n: usize) -> (History, ClipId) {
        let mut h = History::new(project(n));
        let id = h.project().sequence().tracks()[0].clips()[(n - 1) / 2].id;
        h.apply(Command::AddKeyframe {
            id,
            param: "opacity".into(),
            key: Keyframe {
                time: Time(0),
                value: Value::Scalar(1.0),
                easing: Easing::Linear,
            },
        })
        .unwrap();
        (h, id)
    }

    #[test]
    fn selecting_a_keyed_clip_expands_it_and_the_toggle_stays_on_screen() {
        use egui::pos2;
        use egui_kittest::kittest::Queryable;
        use jvid_core::model::Project;
        use suite_ui::tokens::size;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let (p, id) = keyed(1);
        let height = p.project().sequence().tracks()[0].height;
        let mut h = super::super::tests::harness(p.project().clone());
        let left = 8.0 + size::TRACK_HEADER;
        // Long enough to run past the right edge.
        while {
            let v = h.state().0.view(left);
            v.x(Time(2 * S)) - v.x(Time(0)) < 2000.0
        } {
            h.state_mut().0.action("zoom_in", &Project::default());
        }
        h.run();
        assert!(!h.state().0.keys.expanded.contains(&id));
        h.state_mut().0.selected.insert(id);
        h.run();
        assert!(h.state().0.keys.expanded.contains(&id), "selecting expands");

        // The toggle sits at the clip's visible left edge, not its far-off right end.
        let lock = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
        let y = lock - height / 2.0 + super::super::NAME_STRIP / 2.0;
        let x = h.state().0.view(left).x(Time(0)).max(left) + 10.0;
        assert!(x < 1000.0);
        h.hover_at(pos2(x, y));
        h.run();
        h.drag_at(pos2(x, y));
        h.run();
        h.drop_at(pos2(x, y));
        h.run();
        h.run();
        assert!(
            !h.state().0.keys.expanded.contains(&id),
            "toggle collapses, and it sticks"
        );
    }

    #[test]
    fn paste_before_the_clip_lands_at_source_zero() {
        let (mut h, _) = keyed(4);
        let c = h.project().sequence().tracks()[0].clips()[1].clone();
        assert!(c.start > Time(0) && c.source_in < c.start);
        let copied = copy(&c, &BTreeSet::from([(c.id, "opacity".into(), Time(0))]));
        h.apply(paste(&c, &copied, Time(0))).unwrap();
        let c = &h.project().sequence().tracks()[0].clips()[1];
        assert_eq!(c.keyframes["opacity"][0].time, Time(0), "never negative");
    }

    #[test]
    fn picking_on_another_clip_drops_the_first_clips_keys() {
        let (h, id) = keyed(4);
        let other = h.project().sequence().tracks()[0].clips()[0].id;
        let mut keys = Keys::default();
        keys.pick_clip(other);
        keys.selected.insert((other, "opacity".into(), Time(0)));
        keys.pick_clip(id);
        keys.selected.insert((id, "opacity".into(), Time(0)));
        assert_eq!(keys.selected.len(), 1);
        assert_eq!(keys.clip, Some(id));
        assert!(keys.selected.iter().all(|s| s.0 == id));
    }

    #[test]
    fn height_grip_on_an_expanded_track_sizes_the_clip_row() {
        use egui::pos2;
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let (p, id) = keyed(1);
        let height = p.project().sequence().tracks()[0].height;
        let mut h = super::super::tests::harness(p.project().clone());
        h.state_mut().0.keys.expanded.insert(id);
        h.run();
        // The grip sits at the bottom of the lanes, one lane under the clip row.
        let lock = h.get_all_by_label("Lock").next().unwrap().rect();
        let (x, edge) = (40.0, lock.center().y - 16.0 + height + LANE_H);
        h.hover_at(pos2(x, edge));
        h.run();
        h.drag_at(pos2(x, edge));
        h.run();
        for dy in [5.0, 10.0, 21.0] {
            h.hover_at(pos2(x, edge + dy));
            h.run();
        }
        h.drop_at(pos2(x, edge + 21.0));
        h.run();
        let t = &h.state().1.project().sequence().tracks()[0];
        assert_eq!(t.height, height + 20.0, "grows with the pointer, no jump");
    }

    fn clipboard_fixture() -> (History, Timeline, ClipId, ClipId) {
        let mut h = History::new(project(4));
        let clips = h.project().sequence().tracks()[0].clips();
        let (a, b) = (clips[0].id, clips[1].id);
        for time in [Time(0), Time(S / 2)] {
            h.apply(Command::AddKeyframe {
                id: a,
                param: "opacity".into(),
                key: Keyframe {
                    time,
                    value: Value::Scalar(0.5),
                    easing: Easing::Linear,
                },
            })
            .unwrap();
        }
        let mut t = Timeline::default();
        t.sync(h.project());
        t.keys.pick_clip(a);
        t.keys.selected = [
            (a, "opacity".into(), Time(0)),
            (a, "opacity".into(), Time(S / 2)),
        ]
        .into();
        assert!(t.key_action("copy", h.project().sequence()));
        (History::new(h.project().clone()), t, a, b)
    }

    fn scale_clipboard_migration(vector_clipboard: bool) {
        let mut h = History::new(project(3));
        let clips = h.project().sequence().tracks()[0].clips();
        let (a, b) = (clips[0].id, clips[1].id);
        let clip = |h: &History, id| h.project().sequence().clip(id).unwrap().1.clone();
        for id in [a, b] {
            for (time, value, easing) in [
                (Time(0), Value::Scalar(1.0), Easing::Linear),
                (Time(S), Value::Scalar(3.0), Easing::EaseOut),
            ] {
                h.apply(Command::AddKeyframe {
                    id,
                    param: "scale".into(),
                    key: Keyframe {
                        time,
                        value,
                        easing,
                    },
                })
                .unwrap();
            }
        }
        let promote = |h: &mut History, time| {
            let c = clip(h, a);
            let x = Transform {
                scale: [3.0, 5.0],
                ..c.transform.clone()
            };
            h.apply(crate::inspector::set_props(&[&c], &["scale"], &x, time))
                .unwrap();
        };
        if vector_clipboard {
            promote(&mut h, Time(S));
        }
        let mut t = Timeline::default();
        t.sync(h.project());
        t.select_clip(h.project().sequence(), a);
        t.keys.pick_clip(a);
        t.keys.selected = [(a, "scale".into(), Time(S))].into();
        assert!(t.action("copy", h.project()));
        let clipboard = t.keys.clipboard.clone();
        let (destination, first, incoming) = if vector_clipboard {
            // A vector copied from another clip onto an untouched legacy animation.
            (b, [1.0; 2], [3.0, 5.0])
        } else {
            // The timeline retains a scalar copy across an Inspector X/Y edit.
            promote(&mut h, Time(0));
            (a, [3.0, 5.0], [3.0; 2])
        };
        assert_eq!(
            t.keys.clipboard, clipboard,
            "promotion retains the clipboard"
        );
        let before = h.project().clone();
        t.select_clip(h.project().sequence(), destination);
        let c = clip(&h, destination);
        t.seek(c.timeline_at(Time(S / 2)));
        assert!(t.action("paste", h.project()));
        h.apply(t.pending.take().unwrap()).unwrap();
        let c = clip(&h, destination);
        assert_eq!(
            c.keyframes["scale"],
            [
                Keyframe {
                    time: Time(0),
                    value: Value::Vec2(first),
                    easing: Easing::Linear
                },
                Keyframe {
                    time: Time(S / 2),
                    value: Value::Vec2(incoming),
                    easing: Easing::EaseOut
                },
                Keyframe {
                    time: Time(S),
                    value: Value::Vec2([3.0; 2]),
                    easing: Easing::EaseOut
                },
            ],
            "values, source times and easing survive promotion and paste"
        );
        let expected =
            |fraction: f64| std::array::from_fn(|i| first[i] + (incoming[i] - first[i]) * fraction);
        assert_eq!(
            c.transform_at(c.timeline_at(Time(S / 4))).scale,
            expected(0.5)
        );
        let fraction = Easing::EaseOut.apply(0.5);
        let expected = std::array::from_fn(|i| incoming[i] + (3.0 - incoming[i]) * fraction);
        assert_eq!(
            c.transform_at(c.timeline_at(Time(3 * S / 4))).scale,
            expected
        );
        let after = h.project().clone();
        assert!(h.undo());
        assert_eq!(
            h.project(),
            &before,
            "one paste undo restores legacy variants too"
        );
        assert!(h.redo());
        assert_eq!(h.project(), &after);
    }

    #[test]
    fn retained_scalar_scale_clipboard_pastes_into_promoted_animation() {
        scale_clipboard_migration(false);
    }

    #[test]
    fn vector_scale_clipboard_pastes_into_legacy_scalar_animation() {
        scale_clipboard_migration(true);
    }

    #[test]
    fn key_clipboard_pastes_into_selected_unanimated_clip_and_undoes_only_it() {
        let (mut h, mut t, a, b) = clipboard_fixture();
        let before = h.project().clone();
        t.select_clip(h.project().sequence(), b);
        t.seek(Time(5 * S / 2));
        assert!(t.key_action("paste", h.project().sequence()));
        h.apply(t.pending.take().unwrap()).unwrap();
        assert_eq!(h.project().sequence().clip(a), before.sequence().clip(a));
        let c = h.project().sequence().clip(b).unwrap().1;
        let times: Vec<_> = c.keyframes["opacity"].iter().map(|k| k.time).collect();
        assert_eq!(times, [Time(S / 2), Time(S)]);
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        assert!(!h.can_undo());
    }

    #[test]
    fn key_clipboard_and_menus_share_capabilities_and_copy_marker() {
        let (h, mut t, _, b) = clipboard_fixture();
        assert!(t.copied, "key copies trigger the system clipboard marker");
        t.select_clip(h.project().sequence(), b);
        assert!(t.can("paste", h.project().sequence()));
        assert!(
            t.clip_action("paste", h.project().sequence()).is_none(),
            "keys never enable clip paste"
        );
    }

    #[test]
    fn key_paste_refuses_ambiguous_stale_locked_or_unsupported_destinations() {
        let (mut h, mut t, a, b) = clipboard_fixture();
        t.selected = [a, b].into();
        assert!(!t.can("paste", h.project().sequence()));
        assert!(t.key_action("paste", h.project().sequence()));
        assert!(t.pending.is_none());
        t.select_clip(h.project().sequence(), b);
        t.selected.clear();
        assert!(
            !t.can("paste", h.project().sequence()),
            "no stale owner after explicit selection"
        );
        t.select_clip(h.project().sequence(), b);
        t.keys.clipboard[0].1.value = Value::Vec2([0.5, 0.5]);
        assert!(
            !t.can("paste", h.project().sequence()),
            "opacity needs scalar values"
        );
        t.keys.clipboard[0].1.value = Value::Scalar(0.5);
        let mut settings = h.project().sequence().tracks()[0].settings();
        settings.locked = true;
        h.apply(Command::SetTrack(settings)).unwrap();
        assert!(!t.can("paste", h.project().sequence()));
    }

    #[test]
    fn effect_key_paste_requires_matching_shader_and_parameter_target() {
        use jvid_core::model::Effect;
        let (mut h, mut t, _, b) = clipboard_fixture();
        t.select_clip(h.project().sequence(), b);
        t.keys.clipboard = vec![(
            "fx0.amount".into(),
            Keyframe {
                time: Time(0),
                value: Value::Scalar(0.5),
                easing: Easing::Linear,
            },
        )];
        let effect = |shader: &str| Effect {
            shader: shader.into(),
            params: [("amount".into(), 1.0)].into(),
            enabled: true,
            mask: None,
        };
        t.keys.clipboard_source.as_mut().unwrap().transform.effects = vec![effect("grain")];
        assert!(!t.can("paste", h.project().sequence()), "missing target");
        for (shader, expected) in [("blur", false), ("grain", true)] {
            let c = h.project().sequence().clip(b).unwrap().1;
            let mut x = c.transform.clone();
            x.effects = vec![effect(shader)];
            h.apply(Command::SetClipProps(vec![(b, x, c.keyframes.clone())]))
                .unwrap();
            assert_eq!(t.can("paste", h.project().sequence()), expected);
        }
    }
}
