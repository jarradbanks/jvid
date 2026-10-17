//! Zoom-to-region: Alt-drag a rectangle on the preview over the selected video clip and it
//! zooms in on it from the playhead, holds, then zooms back out, as eased `scale` and
//! `position` keyframes in one `SetClipProps` step, editable afterwards like any keyframes.

use std::collections::BTreeMap;
use std::collections::HashSet;

use egui::{Pos2, Rect, Stroke, Ui, Vec2};
use jvid_core::history::Command;
use jvid_core::keyframe::{Easing, Keyframe, Value};
use jvid_core::model::{Clip, ClipId, Project, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND as S, Time};
use suite_ui::tokens::color;

/// Duration and easing of the zoom in and out.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Preset {
    Quick,
    #[default]
    Smooth,
}

impl Preset {
    const ALL: [Self; 2] = [Self::Quick, Self::Smooth];

    /// Each transition's length and easing.
    fn ramp(self) -> (Time, Easing) {
        match self {
            Self::Quick => (Time(S * 3 / 10), Easing::EaseOut),
            Self::Smooth => (Time(S * 8 / 10), Easing::EaseInOut),
        }
    }
}

/// How long the zoom holds on the region.
const HOLD: Time = Time(2 * S);

/// `r` (project pixels from the frame centre, y down) grown about its centre to the
/// `w`x`h` project's aspect, so the zoomed frame never letterboxes.
fn fit(r: Rect, w: f32, h: f32) -> Rect {
    let a = w / h;
    let size = if r.width() / r.height().max(1e-3) < a {
        Vec2::new(r.height() * a, r.height())
    } else {
        Vec2::new(r.width(), r.width() / a)
    };
    Rect::from_center_size(r.center(), size)
}

/// Keys zooming `c` from timeline time `t` onto `region` (project pixels from the frame
/// centre, y down) in a `w` wide project; replaces scale and position keys inside its span.
pub fn zoom(c: &Clip, t: Time, region: Rect, w: f32, h: f32, preset: Preset) -> Command {
    let r = fit(region, w, h);
    let k = (w / r.width().max(1.0)) as f64;
    let x0 = c.transform_at(t);
    // Scaling the whole output by k about the region's centre moves that centre to the middle.
    let (cx, cy) = (r.center().x as f64, r.center().y as f64);
    let pos = [(x0.position[0] - cx) * k, (x0.position[1] - cy) * k];
    let scale = x0.scale.map(|s| s * k);

    let (ramp, easing) = preset.ramp();
    let s0 = c.source_in + (t - c.start);
    let mut steps = [ramp, HOLD, ramp];
    // Squeeze it into what is left of the clip.
    let room = (c.source_out - s0).0 as f64 / (2 * ramp.0 + HOLD.0) as f64;
    if room < 1.0 {
        steps = steps.map(|d| Time((d.0 as f64 * room) as i64));
    }
    let times = [
        s0,
        s0 + steps[0],
        s0 + steps[0] + steps[1],
        s0 + steps[0] + steps[1] + steps[2],
    ];
    let eases = [easing, Easing::Linear, easing, Easing::Linear];
    let uniform = x0.scale[0] == x0.scale[1];
    let sv = |s: [f64; 2]| {
        if uniform {
            Value::Scalar(s[0])
        } else {
            Value::Vec2(s)
        }
    };
    let vals = [
        ("position", [Value::Vec2(x0.position), Value::Vec2(pos)]),
        ("scale", [sv(x0.scale), sv(scale)]),
    ];

    let mut keys: BTreeMap<String, Vec<Keyframe>> = c.keyframes.clone();
    for (name, [from, to]) in vals {
        let list = keys.entry(name.into()).or_default();
        list.retain(|k| k.time < times[0] || k.time > times[3]);
        for (i, time) in times.into_iter().enumerate() {
            let value = if i == 1 || i == 2 { to } else { from };
            let easing = eases[i];
            list.push(Keyframe {
                time,
                value,
                easing,
            });
        }
        list.sort_by_key(|k| k.time);
        list.dedup_by_key(|k| k.time);
    }
    Command::SetClipProps(vec![(c.id, c.transform.clone(), keys)])
}

/// A drag's start on screen.
#[derive(Clone, Copy)]
struct Draw(Pos2);

/// While Alt is held over the selected video clip at playhead `t`, draws the zoom region
/// inside `frame`, the project's on-screen rect, and returns the zoom on release.
pub fn show(
    ui: &mut Ui,
    project: &Project,
    selected: &HashSet<ClipId>,
    t: Time,
    frame: Rect,
) -> Option<Command> {
    let seq = project.sequence();
    let c = selected
        .iter()
        .filter_map(|id| seq.clip(*id))
        .filter(|(tr, c)| {
            seq.track(*tr).is_some_and(|tr| tr.kind == TrackKind::Video)
                && c.start <= t
                && t < c.end()
        })
        .map(|(_, c)| c)
        .min_by_key(|c| c.id.0)?;
    let id = ui.id().with("zoom region");
    let start: Option<Draw> = ui.data(|d| d.get_temp(id));
    if !ui.input(|i| i.modifiers.alt) && start.is_none() {
        return None;
    }
    let mut preset: Preset = ui
        .data(|d| d.get_temp(id.with("preset")))
        .unwrap_or_default();
    let bar = Rect::from_min_size(frame.min + Vec2::splat(8.0), Vec2::new(160.0, 20.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
        ui.horizontal(|ui| {
            ui.label("Zoom");
            for p in Preset::ALL {
                ui.selectable_value(&mut preset, p, format!("{p:?}"));
            }
        });
    });
    ui.data_mut(|d| d.insert_temp(id.with("preset"), preset));

    let resp = ui.interact(frame, id, egui::Sense::drag());
    let resp = resp.on_hover_cursor(egui::CursorIcon::Crosshair);
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        ui.data_mut(|d| d.insert_temp(id, Draw(p)));
    }
    let Draw(from) = ui.data(|d| d.get_temp(id))?;
    let to = resp.interact_pointer_pos().unwrap_or(from);
    let st = project.settings();
    let (w, h) = (st.width as f32, st.height as f32);
    let s = frame.width() / w;
    let px = |p: Pos2| ((p - frame.center()) / s).to_pos2();
    let region = Rect::from_two_pos(px(from), px(to));
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<Draw>(id));
        // A click, not a drag.
        return (region.width() > 4.0 && region.height() > 4.0)
            .then(|| zoom(c, t, region, w, h, preset));
    }
    let shown = fit(region, w, h);
    let screen = Rect::from_min_max(
        frame.center() + shown.min.to_vec2() * s,
        frame.center() + shown.max.to_vec2() * s,
    );
    let p = ui.painter().with_clip_rect(frame);
    p.rect_stroke(
        screen,
        0.0,
        Stroke::new(1.0, color::AMBER),
        egui::StrokeKind::Inside,
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    /// A 10s clip filling a 1920x1080 project, with a position key at 0.
    fn clip() -> (jvid_core::history::History, Clip) {
        let (h, _) = crate::handles::tests::history();
        let c = h.project().sequence().tracks()[0].clips()[0].clone();
        (h, c)
    }

    #[test]
    fn fit_keeps_the_project_aspect() {
        let r = fit(
            Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0)),
            1920.0,
            1080.0,
        );
        assert!((r.width() / r.height() - 16.0 / 9.0).abs() < 1e-4);
        assert!((r.center() - pos2(50.0, 50.0)).length() < 1e-3);
        assert_eq!(r.height(), 100.0);
    }

    #[test]
    fn zooms_in_holds_and_out() {
        let (mut h, c) = clip();
        // The top-left quarter of a 1920x1080 frame.
        let region = Rect::from_min_max(pos2(-960.0, -540.0), pos2(0.0, 0.0));
        let Command::SetClipProps(v) = zoom(&c, Time(S), region, 1920.0, 1080.0, Preset::Smooth)
        else {
            panic!()
        };
        let keys = &v[0].2;
        let scale = &keys["scale"];
        let pos = &keys["position"];
        let t: Vec<_> = scale.iter().map(|k| k.time).collect();
        assert_eq!(
            t,
            [
                Time(S),
                Time(S * 18 / 10),
                Time(S * 38 / 10),
                Time(S * 46 / 10)
            ]
        );
        let vals: Vec<_> = scale.iter().map(|k| k.value).collect();
        let s = |v| Value::Scalar(v);
        assert_eq!(vals, [s(1.0), s(2.0), s(2.0), s(1.0)]);
        assert_eq!(pos[2].value, Value::Vec2([960.0, 540.0]));
        assert_eq!(scale[0].easing, Easing::EaseInOut);
        assert_eq!(scale[2].easing, Easing::EaseInOut);
        // Halfway into the hold, the region's centre sits mid-frame at twice the size.
        let mut z = c.clone();
        z.keyframes = keys.clone();
        let x = z.transform_at(Time(S * 28 / 10));
        assert_eq!(x.scale, [2.0, 2.0]);
        assert_eq!(x.position, [960.0, 540.0]);
        // One undo step, the earlier key kept.
        let before = c.keyframes.clone();
        h.apply(Command::SetClipProps(v)).unwrap();
        assert_eq!(
            h.project().sequence().tracks()[0].clips()[0].keyframes["position"].len(),
            5
        );
        assert!(h.undo());
        assert_eq!(
            h.project().sequence().tracks()[0].clips()[0].keyframes,
            before
        );
    }

    #[test]
    fn squeezes_into_the_clip_end() {
        let (_, c) = clip();
        let region = Rect::from_min_max(pos2(-100.0, -100.0), pos2(100.0, 100.0));
        let Command::SetClipProps(v) = zoom(&c, Time(9 * S), region, 1920.0, 1080.0, Preset::Quick)
        else {
            panic!()
        };
        let last = v[0].2["scale"].last().unwrap().time;
        assert!(last <= Time(10 * S) && last > Time(9 * S));
        assert_eq!(v[0].2["scale"].len(), 4);
    }
}
