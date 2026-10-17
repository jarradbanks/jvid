//! On-preview transform handles: a bounding box with corner and edge handles, a rotation
//! handle and the anchor crosshair over the selected clip. Edits go through the Inspector's
//! `SetClipProps`, so they key animated properties and undo the same way.

use std::collections::HashSet;

use egui::{Pos2, Rect, Sense, Stroke, Ui, Vec2, vec2};
use jvid_core::history::GestureKey;
use jvid_core::model::{Clip, ClipId, Project, TrackKind, Transform};
use jvid_core::time::Time;
use suite_ui::tokens::color;

use crate::timeline::Edit;

/// Undo gesture for handle drags, xored with the drag count.
const GESTURE: GestureKey = 0x6861_6e64_0000; // "hand"
const HANDLE: f32 = 8.0;
/// The rotation handle's distance above the top edge.
const STEM: f32 = 24.0;
/// Shift snaps rotation to this many degrees.
const SNAP: f64 = 15.0;

/// What a drag changes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    Move,
    /// Scale from a handle; corners and edges both scale uniformly, as the Inspector links scale.
    Scale,
    Rotate,
}

/// A drag's start: the pointer and transform when it began, and the drag count.
#[derive(Clone, Copy)]
struct Start {
    grab: Grab,
    from: Pos2,
    x: Transform,
    drag: u64,
}

/// `start` dragged from `from` to `to` (screen points) with the anchor at `anchor` on screen and
/// `s` screen points per project pixel; Shift constrains move to one axis and snaps rotation.
fn dragged(
    grab: Grab,
    start: Transform,
    from: Pos2,
    to: Pos2,
    anchor: Pos2,
    s: f32,
    shift: bool,
) -> Transform {
    let mut x = start;
    match grab {
        Grab::Move => {
            let mut d = (to - from) / s;
            if shift {
                if d.x.abs() > d.y.abs() {
                    d.y = 0.0;
                } else {
                    d.x = 0.0;
                }
            }
            x.position = [
                start.position[0] + d.x as f64,
                start.position[1] + d.y as f64,
            ];
        }
        Grab::Scale => {
            // How far along its start direction from the anchor the handle went.
            let d = from - anchor;
            let k = (((to - anchor).dot(d) / d.length_sq().max(1e-6)) as f64).max(0.01);
            x.scale = start.scale.map(|v| v * k);
        }
        Grab::Rotate => {
            let angle = |p: Pos2| ((p - anchor).angle() as f64).to_degrees();
            let r = start.rotation + angle(to) - angle(from);
            x.rotation = if shift { (r / SNAP).round() * SNAP } else { r };
        }
    }
    x
}

/// The four corners (top-left, top-right, bottom-left, bottom-right) of `c` under `x` on
/// screen, with `frame` the project's on-screen rect; `None` when cropped away.
fn corners(project: &Project, c: &Clip, x: &Transform, frame: Rect) -> Option<[Pos2; 4]> {
    let st = project.settings();
    let size = [st.width as f32, st.height as f32];
    let probe = project.asset(c.asset).and_then(|a| a.probe.as_ref());
    // A solid has no probe and fills the frame.
    let image = match probe.and_then(|p| Some([p.width? as f32, p.height? as f32])) {
        Some(i) => i,
        None => size,
    };
    let q = jvid_render::composite::quad(image, size, x)?;
    Some(q.map(|[cx, cy, ..]| {
        frame.min
            + vec2(
                (cx + 1.0) / 2.0 * frame.width(),
                (1.0 - cy) / 2.0 * frame.height(),
            )
    }))
}

/// Handles over the lowest-id selected video clip at playhead `t`, inside `frame`, the
/// project's on-screen rect; a drag's edit.
pub fn show(
    ui: &mut Ui,
    project: &Project,
    selected: &HashSet<ClipId>,
    t: Time,
    frame: Rect,
) -> Option<Edit> {
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
    let x = c.transform_at(t);
    let [tl, tr, bl, br] = corners(project, c, &x, frame)?;
    let s = frame.width() / project.settings().width as f32;
    let anchor = frame.center() + vec2(x.position[0] as f32, x.position[1] as f32) * s;
    let up = (tl - bl).normalized();
    let top = tl.lerp(tr, 0.5);
    let rot = top + up * STEM;

    let id = ui.id().with("transform handles");
    let start: Option<Start> = ui.data(|d| d.get_temp(id));
    let mut grabs: Vec<(Grab, Rect)> = Vec::new();
    // Later grabs sit on top: the body first, so handles win where they overlap it.
    let bounds = Rect::from_points(&[tl, tr, bl, br]);
    grabs.push((Grab::Move, bounds));
    for p in [
        tl,
        tr,
        bl,
        br,
        top,
        tr.lerp(br, 0.5),
        bl.lerp(br, 0.5),
        tl.lerp(bl, 0.5),
    ] {
        grabs.push((
            Grab::Scale,
            Rect::from_center_size(p, Vec2::splat(HANDLE * 1.5)),
        ));
    }
    grabs.push((
        Grab::Rotate,
        Rect::from_center_size(rot, Vec2::splat(HANDLE * 1.5)),
    ));
    let mut edit = None;
    for (i, (grab, r)) in grabs.into_iter().enumerate() {
        let resp = ui.interact(
            r.intersect(frame.expand(STEM + HANDLE)),
            id.with(i),
            Sense::drag(),
        );
        let icon = match grab {
            Grab::Move => egui::CursorIcon::Move,
            Grab::Scale => egui::CursorIcon::ResizeNwSe,
            Grab::Rotate => egui::CursorIcon::Alias,
        };
        let resp = resp.on_hover_cursor(icon);
        if resp.drag_started()
            && let Some(from) = resp.interact_pointer_pos()
        {
            let drag = start.map_or(0, |s| s.drag) + 1;
            ui.data_mut(|d| {
                d.insert_temp(
                    id,
                    Start {
                        grab,
                        from,
                        x,
                        drag,
                    },
                )
            });
        }
        let Some(st) = ui
            .data(|d| d.get_temp::<Start>(id))
            .filter(|st| st.grab == grab)
        else {
            continue;
        };
        if resp.dragged()
            && let Some(to) = resp.interact_pointer_pos()
        {
            let shift = ui.input(|i| i.modifiers.shift);
            let anchor =
                frame.center() + vec2(st.x.position[0] as f32, st.x.position[1] as f32) * s;
            let new = dragged(grab, st.x, st.from, to, anchor, s, shift);
            if new != x {
                let names: &[&str] = match grab {
                    Grab::Move => &["position"],
                    Grab::Scale => &["scale"],
                    Grab::Rotate => &["rotation"],
                };
                let cmd = crate::inspector::set_props(&[c], names, &new, t);
                edit = Some((cmd, Some(GESTURE ^ (st.drag << 16))));
            }
        }
    }

    let p = ui.painter().with_clip_rect(frame.expand(STEM + HANDLE));
    let stroke = Stroke::new(1.0, color::AMBER);
    p.add(egui::Shape::closed_line(vec![tl, tr, br, bl], stroke));
    p.line_segment([top, rot], stroke);
    p.circle(rot, HANDLE / 2.0, color::VIEWER, stroke);
    for h in [
        tl,
        tr,
        bl,
        br,
        top,
        tr.lerp(br, 0.5),
        bl.lerp(br, 0.5),
        tl.lerp(bl, 0.5),
    ] {
        p.rect_filled(
            Rect::from_center_size(h, Vec2::splat(HANDLE)),
            0.0,
            color::AMBER,
        );
    }
    p.circle_stroke(anchor, HANDLE, stroke);
    p.line_segment(
        [
            anchor - vec2(HANDLE * 1.5, 0.0),
            anchor + vec2(HANDLE * 1.5, 0.0),
        ],
        stroke,
    );
    p.line_segment(
        [
            anchor - vec2(0.0, HANDLE * 1.5),
            anchor + vec2(0.0, HANDLE * 1.5),
        ],
        stroke,
    );
    edit
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn move_scale_rotate() {
        let x = Transform::default();
        let a = pos2(100.0, 100.0);
        // Two screen points per project pixel.
        let m = dragged(Grab::Move, x, a, pos2(120.0, 106.0), a, 2.0, false);
        assert_eq!(m.position, [10.0, 3.0]);
        let m = dragged(Grab::Move, x, a, pos2(120.0, 106.0), a, 2.0, true);
        assert_eq!(m.position, [10.0, 0.0]);
        let m = dragged(Grab::Move, x, a, pos2(104.0, 140.0), a, 2.0, true);
        assert_eq!(m.position, [0.0, 20.0]);
        // A corner twice as far from the anchor doubles scale; past the anchor it floors.
        let s = dragged(
            Grab::Scale,
            x,
            pos2(150.0, 150.0),
            pos2(200.0, 200.0),
            a,
            2.0,
            false,
        );
        assert_eq!(s.scale, [2.0, 2.0]);
        let s = dragged(
            Grab::Scale,
            x,
            pos2(150.0, 150.0),
            pos2(50.0, 50.0),
            a,
            2.0,
            false,
        );
        assert_eq!(s.scale, [0.01, 0.01]);
        // A quarter turn clockwise on screen (y down), and Shift snaps 50° to 45°.
        let r = dragged(
            Grab::Rotate,
            x,
            pos2(200.0, 100.0),
            pos2(100.0, 200.0),
            a,
            2.0,
            false,
        );
        assert!((r.rotation - 90.0).abs() < 1e-4);
        let to = a + Vec2::angled(50f32.to_radians()) * 100.0;
        let r = dragged(Grab::Rotate, x, pos2(200.0, 100.0), to, a, 2.0, true);
        assert_eq!(r.rotation, 45.0);
    }

    /// A clip filling a 1920x1080 project with position animated from a key at 0.
    fn history() -> (jvid_core::history::History, ClipId) {
        use jvid_core::history::{Command, History};
        use jvid_core::keyframe::{Easing, Keyframe, Value};
        use jvid_core::model::{NewClip, ProbeInfo};
        use jvid_core::time::FLICKS_PER_SECOND as S;
        let mut h = History::new(Project::default());
        let probe = ProbeInfo {
            duration: Time(60 * S),
            width: Some(1920),
            height: Some(1080),
            ..Default::default()
        };
        h.apply(Command::Import(vec![("/nonexistent/a.mp4".into(), probe)]))
            .unwrap();
        let asset = h.project().assets()[0].id;
        h.apply(Command::AddTrack {
            kind: TrackKind::Video,
            name: "V1".into(),
        })
        .unwrap();
        let track = h.project().sequence().tracks()[0].id;
        let clip = NewClip {
            asset,
            source_in: Time(0),
            source_out: Time(10 * S),
            start: Time(0),
            link: None,
        };
        h.apply(Command::AddClip { track, clip }).unwrap();
        let c = h.project().sequence().tracks()[0].clips()[0].clone();
        let key = Keyframe {
            time: Time(0),
            value: Value::Vec2([0.0; 2]),
            easing: Easing::Linear,
        };
        let keys = [("position".to_string(), vec![key])].into();
        h.apply(Command::SetClipProps(vec![(c.id, c.transform, keys)]))
            .unwrap();
        (h, c.id)
    }

    #[test]
    fn a_drag_keys_and_is_one_undo_step() {
        use jvid_core::history::History;
        use jvid_core::time::FLICKS_PER_SECOND as S;
        let (h, id) = history();
        let mut harness = egui_kittest::Harness::builder()
            .with_size([960.0, 540.0])
            .build_ui_state(
                move |ui, h: &mut History| {
                    let frame = Rect::from_min_size(Pos2::ZERO, vec2(960.0, 540.0));
                    let edit = show(ui, h.project(), &HashSet::from([id]), Time(S), frame);
                    if let Some((c, Some(k))) = edit {
                        h.apply_coalesced(k, c).unwrap();
                    }
                },
                h,
            );
        harness.run();
        let keys = |h: &egui_kittest::Harness<'_, History>| {
            h.state().project().sequence().tracks()[0].clips()[0].keyframes["position"].clone()
        };
        let before = keys(&harness);
        let mut p = pos2(300.0, 200.0);
        harness.hover_at(p);
        harness.run();
        harness.drag_at(p);
        harness.run();
        for _ in 0..3 {
            p += vec2(20.0, 0.0);
            harness.hover_at(p);
            harness.run();
        }
        harness.drop_at(p);
        harness.run();
        // Half a screen point per project pixel: 60 points is 120 pixels, keyed at 1s.
        let after = keys(&harness);
        assert_eq!(after.len(), 2);
        assert_eq!(after[1].time, Time(S));
        assert_eq!(
            after[1].value,
            jvid_core::keyframe::Value::Vec2([120.0, 0.0])
        );
        assert!(harness.state_mut().undo());
        assert_eq!(keys(&harness), before);
    }
}
