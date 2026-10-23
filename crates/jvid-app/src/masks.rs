//! On-preview mask handles over the selected clip: each mask's outline with handles on its
//! box or points, and drawing a bezier mask point by point. Edits go through the Inspector's
//! `SetClipProps`, so they key animated paths and undo the same way.

use std::collections::HashSet;

use egui::{Key, Modifiers, Pos2, Rect, Sense, Stroke, Ui, Vec2, vec2};
use jvid_core::history::{Command, GestureKey};
use jvid_core::model::{Clip, ClipId, Mask, MaskShape, Project, TrackKind, Transform};
use jvid_core::time::Time;
use suite_ui::theme;
use suite_ui::tokens::{color, space};

use crate::i18n::tr;
use crate::timeline::Edit;

/// Undo gesture for mask handle drags, xored with the drag count.
const GESTURE: GestureKey = 0x6d61_736b_0000; // "mask"
const HANDLE: f32 = 7.0;
/// How near the first point a click closes the path being drawn.
const CLOSE: f32 = 8.0;

/// `c`'s image size upright, as frames decode; `None` for a solid or title, which have no
/// probe and fill the frame.
pub fn upright(project: &Project, c: &Clip) -> Option<[f32; 2]> {
    let p = project.asset(c.asset)?.probe.as_ref()?;
    let turned = p
        .streams
        .iter()
        .any(|s| matches!(s, jvid_core::model::StreamInfo::Video(v) if v.rotation % 180 == 90));
    let [w, h] = [p.width? as f32, p.height? as f32];
    Some(if turned { [h, w] } else { [w, h] })
}

/// `c`'s image size fitted to the frame: the pixels its masks' points are in.
pub fn fitted(project: &Project, c: &Clip) -> [f64; 2] {
    let st = project.settings();
    let size = [st.width as f32, st.height as f32];
    let image = upright(project, c);
    let [_, _, w, h] =
        jvid_render::preview::place(image.unwrap_or(size), [0.0, 0.0, size[0], size[1]], None);
    [w as f64, h as f64]
}

/// Where image point `p` (see [`Mask::points`]) of a clip under `x` is on screen, with the
/// project's centre at `centre` and `s` screen points per project pixel.
fn screen(x: &Transform, centre: Pos2, s: f32, p: [f64; 2]) -> Pos2 {
    let (sin, cos) = x.rotation.to_radians().sin_cos();
    let u = (p[0] - x.anchor[0]) * x.scale[0];
    let v = (p[1] - x.anchor[1]) * x.scale[1];
    let at = [
        x.position[0] + u * cos - v * sin,
        x.position[1] + u * sin + v * cos,
    ];
    centre + vec2(at[0] as f32, at[1] as f32) * s
}

/// The image point at screen point `q`; [`screen`] inverted.
fn image(x: &Transform, centre: Pos2, s: f32, q: Pos2) -> [f64; 2] {
    let d = (q - centre) / s;
    let (px, py) = (d.x as f64 - x.position[0], d.y as f64 - x.position[1]);
    let (sin, cos) = x.rotation.to_radians().sin_cos();
    let k = |v: f64| if v.abs() < 1e-6 { 1e-6 } else { v };
    [
        (px * cos + py * sin) / k(x.scale[0]) + x.anchor[0],
        (py * cos - px * sin) / k(x.scale[1]) + x.anchor[1],
    ]
}

fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

/// What a drag changes on mask `mask`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    /// A rectangle's or ellipse's box handle, by which sides it moves (-1, 0 or 1 per axis);
    /// `[0, 0]` is the centre, which moves the whole box.
    Box { mask: usize, side: [i8; 2] },
    /// A bezier's point `n` (see [`Mask::points`]): a vertex, or its in or out handle.
    Point { mask: usize, n: usize },
}

/// The box handles' sides, corners first.
const SIDES: [[i8; 2]; 9] = [
    [-1, -1],
    [1, -1],
    [-1, 1],
    [1, 1],
    [0, -1],
    [1, 0],
    [0, 1],
    [-1, 0],
    [0, 0],
];

/// `points` (a mask's, when the drag began) after `grab` went from image point `from` to
/// `to`. A box side follows the pointer with the opposite side fixed; a vertex brings its
/// handles; a handle mirrors the other unless `alt`, and Alt on a vertex pulls out handles.
fn dragged(
    grab: Grab,
    points: &[[f64; 2]],
    from: [f64; 2],
    to: [f64; 2],
    alt: bool,
) -> Vec<[f64; 2]> {
    let mut p = points.to_vec();
    let d = sub(to, from);
    match grab {
        Grab::Box { side: [0, 0], .. } => p[0] = add(points[0], d),
        Grab::Box { side, .. } => {
            for a in 0..2 {
                let s = f64::from(side[a]);
                if s == 0.0 {
                    continue;
                }
                let (c, size) = (points[0][a], points[1][a]);
                let fixed = c - s * size / 2.0;
                let moved = c + s * size / 2.0 + d[a];
                p[0][a] = (fixed + moved) / 2.0;
                // Past the opposite side the box flips rather than going negative.
                p[1][a] = ((moved - fixed) * s).abs();
            }
        }
        Grab::Point { n, .. } if n % 3 == 0 && alt => {
            let out = sub(to, points[n]);
            p[n + 1] = out.map(|v| -v);
            p[n + 2] = out;
        }
        Grab::Point { n, .. } if n % 3 == 0 => p[n] = add(points[n], d),
        Grab::Point { n, .. } => {
            p[n] = add(points[n], d);
            if !alt {
                let other = if n % 3 == 1 { n + 1 } else { n - 1 };
                p[other] = p[n].map(|v| -v);
            }
        }
    }
    p
}

/// Each handle of `m` (mask `i`) as its grab and image point; zero bezier handles are left
/// out, as they sit on their vertex.
fn handles(i: usize, m: &Mask) -> Vec<(Grab, [f64; 2])> {
    match m.shape {
        MaskShape::Bezier => (0..m.points.len())
            .filter(|n| n % 3 == 0 || m.points[*n] != [0.0; 2])
            .map(|n| {
                let at = match n % 3 {
                    0 => m.points[n],
                    _ => add(m.points[n - n % 3], m.points[n]),
                };
                (Grab::Point { mask: i, n }, at)
            })
            .collect(),
        _ => SIDES
            .iter()
            .map(|&side| {
                let [c, size] = [m.points[0], m.points[1]];
                let at = [0, 1].map(|a| c[a] + f64::from(side[a]) * size[a] / 2.0);
                (Grab::Box { mask: i, side }, at)
            })
            .collect(),
    }
}

/// `m`'s outline in image points.
fn outline(m: &Mask) -> Vec<[f64; 2]> {
    match m.shape {
        MaskShape::Bezier => jvid_render::mask::flatten(&m.points, jvid_render::mask::STEPS)
            .into_iter()
            .map(|p| p.map(f64::from))
            .collect(),
        MaskShape::Rectangle => [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]]
            .map(|k| [0, 1].map(|a| m.points[0][a] + k[a] * m.points[1][a] / 2.0))
            .to_vec(),
        MaskShape::Ellipse => (0..64)
            .map(|k| {
                let (sin, cos) = (k as f64 / 64.0 * std::f64::consts::TAU).sin_cos();
                [0, 1].map(|a| m.points[0][a] + [cos, sin][a] * m.points[1][a] / 2.0)
            })
            .collect(),
    }
}

/// A drag's start: the grab, the pointer in image points, the mask's points, the drag count.
#[derive(Clone)]
struct Start {
    grab: Grab,
    from: [f64; 2],
    points: Vec<[f64; 2]>,
    drag: u64,
}

/// A bezier being drawn on `clip`: its points so far.
#[derive(Clone)]
struct Pen {
    clip: ClipId,
    points: Vec<[f64; 2]>,
}

fn pen_id() -> egui::Id {
    egui::Id::new("mask pen")
}

/// Starts drawing a bezier mask on `clip` on the Program view.
pub fn draw(ctx: &egui::Context, clip: ClipId) {
    let pen = Pen {
        clip,
        points: Vec::new(),
    };
    ctx.data_mut(|d| d.insert_temp(pen_id(), pen));
}

/// The masks of the clip [`crate::handles::show`] edits, with handles, or the path being
/// drawn on it; an edit once one moves or the path closes. `frame` is the project's
/// on-screen rect.
pub fn show(
    ui: &mut Ui,
    project: &Project,
    selected: &HashSet<ClipId>,
    t: Time,
    frame: Rect,
) -> Option<Edit> {
    let pen: Option<Pen> = ui.data(|d| d.get_temp(pen_id()));
    if pen.as_ref().is_some_and(|p| !selected.contains(&p.clip)) {
        ui.data_mut(|d| d.remove::<Pen>(pen_id()));
    }
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
    let s = frame.width() / project.settings().width as f32;
    let (to_screen, to_image) = (
        |p| screen(&x, frame.center(), s, p),
        |q| image(&x, frame.center(), s, q),
    );
    let clip = frame
        .expand(HANDLE * 2.0)
        .intersect(crate::preview::viewer_rect(ui));
    let painter = ui.painter().with_clip_rect(clip);
    let line = Stroke::new(1.0, color::LIGHT_BLUE);
    if let Some(pen) = pen.filter(|p| p.clip == c.id) {
        return pen_tool(ui, c, pen, clip, (to_screen, to_image));
    }

    let id = ui.id().with("mask handles");
    let start: Option<Start> = ui.data(|d| d.get_temp(id));
    let mut edit = None;
    for (i, m) in x.masks.iter().enumerate() {
        let path: Vec<Pos2> = outline(m).into_iter().map(to_screen).collect();
        painter.add(egui::Shape::closed_line(path, line));
        for (k, (grab, at)) in handles(i, m).into_iter().enumerate() {
            let p = to_screen(at);
            let r = Rect::from_center_size(p, Vec2::splat(HANDLE * 1.5)).intersect(clip);
            let resp = ui.interact(r, id.with((i, k)), Sense::drag());
            let resp = resp.on_hover_cursor(egui::CursorIcon::Crosshair);
            if resp.drag_started()
                && let Some(from) = resp.interact_pointer_pos()
            {
                let drag = start.as_ref().map_or(0, |s| s.drag) + 1;
                let st = Start {
                    grab,
                    from: to_image(from),
                    points: m.points.clone(),
                    drag,
                };
                ui.data_mut(|d| d.insert_temp(id, st));
            }
            if resp.dragged()
                && let Some(to) = resp.interact_pointer_pos()
                && let Some(st) = ui
                    .data(|d| d.get_temp::<Start>(id))
                    .filter(|s| s.grab == grab)
            {
                let alt = ui.input(|i| i.modifiers.alt);
                let points = dragged(grab, &st.points, st.from, to_image(to), alt);
                if points != m.points {
                    let mut new = x.clone();
                    new.masks[i].points = points;
                    let names: Vec<String> =
                        (0..m.points.len()).map(|n| Mask::point(i, n)).collect();
                    let names: Vec<&str> = names.iter().map(String::as_str).collect();
                    let cmd = crate::inspector::set_props(&[c], &names, &new, t);
                    edit = Some((cmd, Some(GESTURE ^ (st.drag << 16))));
                }
            }
            if let Grab::Point { n, .. } = grab
                && n % 3 != 0
            {
                let vertex = to_screen(m.points[n - n % 3]);
                painter.line_segment([vertex, p], line);
                painter.circle(p, HANDLE / 2.0, color::VIEWER, line);
            } else {
                let square = Rect::from_center_size(p, Vec2::splat(HANDLE));
                painter.rect_filled(square, 0.0, color::LIGHT_BLUE);
            }
        }
    }
    edit
}

/// Drawing a bezier on `c`: a press adds a vertex and dragging pulls out its handles; a
/// press on the first vertex or Enter closes the path, one undo step, and Esc stops.
fn pen_tool(
    ui: &mut Ui,
    c: &Clip,
    mut pen: Pen,
    clip: Rect,
    (to_screen, to_image): (impl Fn([f64; 2]) -> Pos2, impl Fn(Pos2) -> [f64; 2]),
) -> Option<Edit> {
    let r = ui.interact(clip, ui.id().with("mask pen"), Sense::click_and_drag());
    let r = r.on_hover_cursor(egui::CursorIcon::Crosshair);
    let n = pen.points.len() / 3;
    let pressed = r.hovered() && ui.input(|i| i.pointer.primary_pressed());
    let at = ui.input(|i| i.pointer.interact_pos());
    let near_first = n >= 3 && at.is_some_and(|p| p.distance(to_screen(pen.points[0])) <= CLOSE);
    // Keys go to a focused text field first, and Enter only closes a path of three points.
    let free = ui.memory(|m| m.focused().is_none());
    let mut close = free && n >= 3 && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
    if free && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        ui.data_mut(|d| d.remove::<Pen>(pen_id()));
        return None;
    }
    if pressed && near_first {
        close = true;
    } else if pressed && let Some(p) = at {
        pen.points.extend([to_image(p), [0.0; 2], [0.0; 2]]);
    } else if r.dragged()
        && let Some(p) = at
        && n > 0
    {
        let k = 3 * (n - 1);
        let out = sub(to_image(p), pen.points[k]);
        pen.points[k + 1] = out.map(|v| -v);
        pen.points[k + 2] = out;
    }
    if close {
        ui.data_mut(|d| d.remove::<Pen>(pen_id()));
        let mut new = c.transform.clone();
        new.masks.push(Mask::new(MaskShape::Bezier, pen.points));
        return Some((
            Command::SetClipProps(vec![(c.id, new, c.keyframes.clone())]),
            None,
        ));
    }
    let painter = ui.painter().with_clip_rect(clip);
    let line = Stroke::new(1.0, color::LIGHT_BLUE);
    if n >= 2 {
        let path = jvid_render::mask::flatten(&pen.points, jvid_render::mask::STEPS);
        let path = path
            .into_iter()
            .map(|p| to_screen(p.map(f64::from)))
            .collect();
        painter.add(egui::Shape::closed_line(path, line));
    }
    for (k, v) in pen.points.iter().step_by(3).enumerate() {
        let square = Rect::from_center_size(to_screen(*v), Vec2::splat(HANDLE));
        // Ringed when a press there closes the path.
        if k == 0 && near_first {
            painter.rect_stroke(square.expand(2.0), 0.0, line, egui::StrokeKind::Outside);
        }
        painter.rect_filled(square, 0.0, color::LIGHT_BLUE);
    }
    let hint = painter.layout(
        tr!("inspector.draw_path").to_owned(),
        theme::body(),
        color::INK,
        clip.width() - 2.0 * space::S16,
    );
    let at = clip.center_top() + vec2(-hint.size().x / 2.0, space::S8);
    let back = Rect::from_min_size(at, hint.size()).expand(space::S4);
    painter.rect_filled(back, suite_ui::tokens::radius::CONTROL, color::PANEL);
    painter.galley(at, hint, color::INK);
    ui.data_mut(|d| d.insert_temp(pen_id(), pen));
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_and_screen_points_round_trip_through_the_transform() {
        let x = Transform {
            position: [30.0, -20.0],
            anchor: [5.0, 10.0],
            scale: [2.0, 0.5],
            rotation: 30.0,
            ..Default::default()
        };
        let centre = Pos2::new(480.0, 270.0);
        let p = [100.0, -40.0];
        let q = screen(&x, centre, 0.5, p);
        let back = image(&x, centre, 0.5, q);
        assert!((back[0] - p[0]).abs() < 1e-3 && (back[1] - p[1]).abs() < 1e-3);
        // The identity puts an image point at the centre plus its offset, scaled to screen.
        let q = screen(&Transform::default(), centre, 0.5, [100.0, -40.0]);
        assert_eq!(q, Pos2::new(530.0, 250.0));
    }

    #[test]
    fn a_box_side_moves_with_the_opposite_fixed_and_handles_mirror() {
        let rect = [[0.0, 0.0], [100.0, 50.0]];
        let side = |side| Grab::Box { mask: 0, side };
        // The right edge out by 20: the left stays at -50.
        let p = dragged(side([1, 0]), &rect, [50.0, 0.0], [70.0, 9.0], false);
        assert_eq!(p, [[10.0, 0.0], [120.0, 50.0]]);
        // The top-left corner in by (10, 5).
        let p = dragged(side([-1, -1]), &rect, [-50.0, -25.0], [-40.0, -20.0], false);
        assert_eq!(p, [[5.0, 2.5], [90.0, 45.0]]);
        // The right edge dragged past the left flips the box instead of going negative.
        let p = dragged(side([1, 0]), &rect, [50.0, 0.0], [-70.0, 0.0], false);
        assert_eq!(p, [[-60.0, 0.0], [20.0, 50.0]]);
        let p = dragged(side([0, 0]), &rect, [0.0, 0.0], [3.0, 4.0], false);
        assert_eq!(p, [[3.0, 4.0], [100.0, 50.0]]);

        let path = [
            [0.0, 0.0],
            [-5.0, 0.0],
            [5.0, 0.0],
            [10.0, 10.0],
            [0.0; 2],
            [0.0; 2],
        ];
        let point = |n| Grab::Point { mask: 0, n };
        // A vertex brings its handles, as they're relative to it.
        let p = dragged(point(0), &path, [0.0, 0.0], [1.0, 2.0], false);
        assert_eq!(p[..3], [[1.0, 2.0], [-5.0, 0.0], [5.0, 0.0]]);
        // An out handle mirrors the in handle, unless Alt breaks them.
        let p = dragged(point(2), &path, [5.0, 0.0], [5.0, 3.0], false);
        assert_eq!(p[1..3], [[-5.0, -3.0], [5.0, 3.0]]);
        let p = dragged(point(2), &path, [5.0, 0.0], [5.0, 3.0], true);
        assert_eq!(p[1..3], [[-5.0, 0.0], [5.0, 3.0]]);
        // Alt on a straight vertex pulls handles out to the pointer.
        let p = dragged(point(3), &path, [10.0, 10.0], [14.0, 10.0], true);
        assert_eq!(p[3..], [[10.0, 10.0], [-4.0, 0.0], [4.0, 0.0]]);
    }

    /// [`crate::handles::tests::history`]'s clip with `mask` on it, shown at 1s in a 960x540
    /// frame: half a screen point per project pixel. `pen` starts drawing on it.
    fn harness(
        mask: Option<Mask>,
        pen: bool,
    ) -> egui_kittest::Harness<'static, jvid_core::history::History> {
        use jvid_core::history::History;
        use jvid_core::time::FLICKS_PER_SECOND as S;
        let (mut h, id) = crate::handles::tests::history();
        let c = h.project().sequence().tracks()[0].clips()[0].clone();
        let x = Transform {
            masks: mask.into_iter().collect(),
            ..c.transform
        };
        h.apply(Command::SetClipProps(vec![(id, x, c.keyframes)]))
            .unwrap();
        let mut first = pen;
        let mut harness = egui_kittest::Harness::builder()
            .with_size([960.0, 540.0])
            .build_ui_state(
                move |ui, h: &mut History| {
                    if std::mem::take(&mut first) {
                        draw(ui.ctx(), id);
                    }
                    let frame = Rect::from_min_size(Pos2::ZERO, vec2(960.0, 540.0));
                    match show(ui, h.project(), &HashSet::from([id]), Time(S), frame) {
                        Some((c, Some(k))) => h.apply_coalesced(k, c).unwrap(),
                        Some((c, None)) => h.apply(c).unwrap(),
                        None => jvid_core::history::Created::Nothing,
                    };
                },
                h,
            );
        harness.run();
        harness
    }

    fn masks(h: &egui_kittest::Harness<'_, jvid_core::history::History>) -> Vec<Mask> {
        h.state().project().sequence().tracks()[0].clips()[0]
            .transform
            .masks
            .clone()
    }

    fn drag(h: &mut egui_kittest::Harness<'_, jvid_core::history::History>, from: Pos2, to: Pos2) {
        h.hover_at(from);
        h.run();
        h.drag_at(from);
        h.run();
        for k in 1..=3 {
            h.hover_at(from.lerp(to, k as f32 / 3.0));
            h.run();
        }
        h.drop_at(to);
        h.run();
    }

    #[test]
    fn dragging_a_mask_edge_is_one_undo_step() {
        let rect = Mask::new(MaskShape::Rectangle, vec![[0.0, 0.0], [960.0, 540.0]]);
        let mut h = harness(Some(rect.clone()), false);
        // The right edge's handle, at image x 480: screen 480 + 240.
        drag(&mut h, egui::pos2(720.0, 270.0), egui::pos2(780.0, 270.0));
        assert_eq!(masks(&h)[0].points, [[60.0, 0.0], [1080.0, 540.0]]);
        // A second drag is a step of its own.
        drag(&mut h, egui::pos2(780.0, 270.0), egui::pos2(800.0, 270.0));
        assert_eq!(masks(&h)[0].points, [[80.0, 0.0], [1120.0, 540.0]]);
        assert!(h.state_mut().undo());
        assert_eq!(masks(&h)[0].points, [[60.0, 0.0], [1080.0, 540.0]]);
        assert!(h.state_mut().undo());
        assert_eq!(masks(&h), [rect]);
    }

    #[test]
    fn an_edge_drag_with_only_size_keyed_keeps_the_centre_at_other_keys() {
        use jvid_core::keyframe::{Easing, Keyframe, Value};
        use jvid_core::time::FLICKS_PER_SECOND as S;
        let rect = Mask::new(MaskShape::Rectangle, vec![[0.0, 0.0], [960.0, 540.0]]);
        let mut h = harness(Some(rect), false);
        let id = h.state().project().sequence().tracks()[0].clips()[0].id;
        let c = h.state().project().sequence().clip(id).unwrap().1.clone();
        let key = |time, v| Keyframe {
            time: Time(time),
            value: Value::Vec2(v),
            easing: Easing::Linear,
        };
        let size = vec![key(0, [960.0, 540.0]), key(2 * S, [960.0, 540.0])];
        let mut keys = c.keyframes.clone();
        keys.insert(Mask::point(0, 1), size);
        let cmd = Command::SetClipProps(vec![(id, c.transform.clone(), keys)]);
        h.state_mut().apply(cmd).unwrap();
        h.run();
        drag(&mut h, egui::pos2(720.0, 270.0), egui::pos2(780.0, 270.0));
        let c = h.state().project().sequence().clip(id).unwrap().1;
        let centre: Vec<_> = c.keyframes[&Mask::point(0, 0)]
            .iter()
            .map(|k| (k.time.0, k.value))
            .collect();
        let v = Value::Vec2;
        assert_eq!(
            centre,
            [
                (0, v([0.0, 0.0])),
                (S, v([60.0, 0.0])),
                (2 * S, v([0.0, 0.0]))
            ]
        );
    }

    #[test]
    fn a_drawn_path_closes_into_one_mask_in_one_step() {
        let mut h = harness(None, true);
        let click = |h: &mut egui_kittest::Harness<'_, _>, p| drag(h, p, p);
        click(&mut h, egui::pos2(380.0, 170.0));
        // Dragging the second point out pulls its handles.
        drag(&mut h, egui::pos2(580.0, 170.0), egui::pos2(600.0, 170.0));
        click(&mut h, egui::pos2(480.0, 370.0));
        assert!(masks(&h).is_empty(), "nothing until it closes");
        click(&mut h, egui::pos2(381.0, 171.0));
        let m = &masks(&h)[0];
        assert_eq!(m.shape, MaskShape::Bezier);
        assert_eq!(m.points.len(), 9);
        assert_eq!(m.points[0], [-200.0, -200.0]);
        assert_eq!(m.points[3..6], [[200.0, -200.0], [-40.0, 0.0], [40.0, 0.0]]);
        assert!(h.state_mut().undo());
        assert!(masks(&h).is_empty());
    }

    #[test]
    fn masks_survive_save_and_reopen() {
        use jvid_core::keyframe::{Easing, Keyframe, Value};
        let (mut h, id) = crate::handles::tests::history();
        let c = h.project().sequence().tracks()[0].clips()[0].clone();
        let ellipse = Mask {
            feather: 12.0,
            invert: true,
            ..Mask::new(MaskShape::Ellipse, vec![[10.0, 0.0], [300.0, 200.0]])
        };
        let mut keys = c.keyframes.clone();
        let key = |v| Keyframe {
            time: Time(0),
            value: Value::Vec2(v),
            easing: Easing::EaseIn,
        };
        keys.insert(Mask::point(0, 1), vec![key([300.0, 200.0])]);
        let mut x = c.transform.clone();
        x.masks.push(ellipse);
        h.apply(Command::SetClipProps(vec![(id, x, keys)])).unwrap();
        let path = std::env::temp_dir().join(format!("jvid-masks-{}.jvid", std::process::id()));
        crate::project_file::save(h.project(), &path).unwrap();
        let back = crate::project_file::load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(&back, h.project());
    }
}
