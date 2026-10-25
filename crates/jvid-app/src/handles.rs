//! On-preview transform handles: a bounding box with corner and edge handles, a rotation
//! handle and the anchor crosshair over the selected clip. Edits go through the Inspector's
//! `SetClipProps`, so they key animated properties and undo the same way.

use std::collections::HashSet;

use egui::{Pos2, Rect, Sense, Stroke, Ui, Vec2, vec2};
use jvid_core::history::GestureKey;
use jvid_core::model::{Clip, ClipId, Project, Text, TrackKind, Transform};
use jvid_core::time::Time;
use jvid_render::text;
use suite_ui::themes::DARK;
use suite_ui::tokens::color;

use crate::timeline::Edit;

/// Undo gesture for handle drags, xored with the drag count.
const GESTURE: GestureKey = 0x6861_6e64_0000; // "hand"
const HANDLE: f32 = 8.0;
/// The rotation handle's distance above the top edge.
const STEM: f32 = 24.0;
/// Shift snaps rotation to this many degrees.
const SNAP: f64 = 15.0;
/// How much the viewer black covers the wide shot outside a reframe box.
const WIDE_DIM: f32 = 0.6;

/// What a drag changes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    Move,
    /// Scale from a handle; corners and edges both preserve the current X/Y ratio.
    Scale,
    Rotate,
    /// Drag the reframe box, the project frame, over a wide shot: the clip moves the other way.
    Reframe,
}

/// A drag's start: the pointer and transform when it began, and the drag count.
#[derive(Clone)]
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
    start: &Transform,
    from: Pos2,
    to: Pos2,
    anchor: Pos2,
    s: f32,
    shift: bool,
) -> Transform {
    let mut x = start.clone();
    match grab {
        Grab::Move | Grab::Reframe => {
            let mut d = (to - from) / s;
            if grab == Grab::Reframe {
                d = -d;
            }
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
pub(crate) fn corners(
    project: &Project,
    c: &Clip,
    x: &Transform,
    frame: Rect,
) -> Option<[Pos2; 4]> {
    let st = project.settings();
    let size = [st.width as f32, st.height as f32];
    let image = crate::masks::upright(project, c).unwrap_or(size);
    let q = jvid_render::composite::quad(image, size, x)?;
    Some(q.map(|[cx, cy, ..]| {
        frame.min
            + vec2(
                (cx + 1.0) / 2.0 * frame.width(),
                (1.0 - cy) / 2.0 * frame.height(),
            )
    }))
}

/// [`corners`] of the box around `c`'s drawn pixels: a title's text (`text`, as keyed at the
/// playhead) rather than its whole frame.
fn bounds(
    project: &Project,
    c: &Clip,
    x: &Transform,
    text: Option<&Text>,
    frame: Rect,
) -> Option<[Pos2; 4]> {
    let q @ [tl, tr, bl, _] = corners(project, c, x, frame)?;
    let st = project.settings();
    let Some([x0, y0, x1, y1]) = text.and_then(|t| text::drawn_bounds(t, (st.width, st.height)))
    else {
        return Some(q);
    };
    let (w, h) = (st.width as f32, st.height as f32);
    // The quad is a parallelogram, so image coordinates map linearly onto it.
    let at = |px: u32, py: u32| tl + (tr - tl) * (px as f32 / w) + (bl - tl) * (py as f32 / h);
    Some([at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1)])
}

/// The top visible clip whose transformed image contains `pos`. Generated titles use
/// their drawn text bounds, so empty title canvas does not hide the footage below.
pub fn clip_at(project: &Project, t: Time, frame: Rect, pos: Pos2) -> Option<ClipId> {
    jvid_core::eval::video_at(project.sequence(), t)
        .into_iter()
        .find_map(|l| {
            if l.transform.opacity <= 0.0 {
                return None;
            }
            let (_, c) = project.sequence().clip(l.clip)?;
            let text = project
                .asset(c.asset)?
                .text
                .as_ref()
                .map(|text| text.with(&l.text));
            let [tl, tr, bl, _] = bounds(project, c, &l.transform, text.as_ref(), frame)?;
            let (u, v, d) = (tr - tl, bl - tl, pos - tl);
            let det = u.x * v.y - u.y * v.x;
            if det.abs() < 1e-6 {
                return None;
            }
            let a = (d.x * v.y - d.y * v.x) / det;
            let b = (u.x * d.y - u.y * d.x) / det;
            ((0.0..=1.0).contains(&a) && (0.0..=1.0).contains(&b)).then_some(c.id)
        })
}

/// The top text clip with glyphs (or its box) near a double-click, and where it was, at playhead `t`, inside
/// `frame`, the project's on-screen rect.
// shortcut: maps the click through the layer's uncropped quad; a cropped text layer hits
// slightly off. Invert the crop if it matters.
pub fn double_clicked_text(
    ui: &Ui,
    project: &Project,
    t: Time,
    frame: Rect,
) -> Option<(ClipId, Pos2)> {
    let primary = egui::PointerButton::Primary;
    let pos = ui.input(|i| {
        let p = i.pointer.interact_pos();
        p.filter(|_| i.pointer.button_double_clicked(primary))
    });
    let pos = pos.filter(|p| frame.contains(*p) && crate::preview::viewer_rect(ui).contains(*p))?;
    let st = project.settings();
    let (w, h) = (st.width as i64, st.height as i64);
    let seq = project.sequence();
    jvid_core::eval::video_at(seq, t).into_iter().find_map(|l| {
        let text = project.asset(l.asset)?.text.as_ref()?;
        let (_, c) = seq.clip(l.clip)?;
        let [tl, tr, bl, _] = corners(project, c, &l.transform, frame)?;
        // Where `pos` sits across the layer's image, 0..1 on each axis.
        let (u, v, d) = (tr - tl, bl - tl, pos - tl);
        let det = u.x * v.y - u.y * v.x;
        let a = (d.x * v.y - d.y * v.x) / det;
        let b = (u.x * d.y - u.y * d.x) / det;
        if !(0.0..1.0).contains(&a) || !(0.0..1.0).contains(&b) {
            return None;
        }
        let img = jvid_render::text::rasterize(&text.with(&l.text), l.color, (st.width, st.height));
        let (x, y) = ((a * w as f32) as i64, (b * h as f32) as i64);
        // A few pixels of slack, so the gaps between glyphs still hit.
        let r = 8;
        let near = (y - r..=y + r).flat_map(|y| (x - r..=x + r).map(move |x| (x, y)));
        near.filter(|(x, y)| (0..w).contains(x) && (0..h).contains(y))
            .any(|(x, y)| img[((y * w + x) * 4 + 3) as usize] > 0)
            .then_some((l.clip, pos))
    })
}

/// Undo gesture for on-canvas text edits, xored with the asset and the session count.
const TYPING: GestureKey = 0x7479_7065_0000; // "type"

/// An on-canvas text edit: the clip, its text as typed, and the session count.
#[derive(Clone)]
struct Typing {
    clip: ClipId,
    content: String,
    session: u64,
}

fn typing_id() -> egui::Id {
    egui::Id::new("canvas text")
}

/// Whether a text clip is being edited on the canvas; transform handles hide meanwhile.
pub fn editing(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Typing>(typing_id()).is_some())
}

/// Starts editing `clip`'s text on the canvas next frame, the caret at screen point `at`.
pub fn start_editing(ctx: &egui::Context, project: &Project, clip: ClipId, at: Pos2) {
    let Some((_, c)) = project.sequence().clip(clip) else {
        return;
    };
    let Some(text) = project.asset(c.asset).and_then(|a| a.text.as_ref()) else {
        return;
    };
    let session = ctx.data(|d| d.get_temp::<u64>(typing_id().with("sessions")).unwrap_or(0)) + 1;
    ctx.data_mut(|d| {
        d.insert_temp(typing_id().with("sessions"), session);
        d.insert_temp(typing_id().with("at"), at);
        d.insert_temp(
            typing_id(),
            Typing {
                clip,
                content: text.content.clone(),
                session,
            },
        );
    });
}

/// The on-canvas text editor at playhead `t`, inside `frame`, the project's on-screen rect: a
/// hidden text field takes the keys (so the keymap stands down) and a caret is drawn into the
/// rendered text. Esc or a click elsewhere ends it; each session is one undo step.
// shortcut: mouse drag-selection and Up/Down follow the hidden field's own layout, not the
// rendered one; map them through `text::hit` too if they feel off.
pub fn edit_text(ui: &mut Ui, project: &Project, t: Time, frame: Rect) -> Option<Edit> {
    let id = typing_id();
    let mut typing: Typing = ui.data(|d| d.get_temp(id))?;
    let seq = project.sequence();
    let layer = jvid_core::eval::video_at(seq, t)
        .into_iter()
        .find(|l| l.clip == typing.clip);
    let found = layer.as_ref().and_then(|l| {
        let a = project.asset(l.asset)?;
        let (_, c) = seq.clip(l.clip)?;
        Some((
            a,
            a.text.as_ref()?,
            corners(project, c, &l.transform, frame)?,
        ))
    });
    let (Some(l), Some((asset, base, [tl, tr, bl, br]))) = (&layer, found) else {
        ui.data_mut(|d| d.remove::<Typing>(id));
        return None;
    };
    let st = project.settings();
    let size = (st.width, st.height);
    let (w, h) = (st.width as f32, st.height as f32);
    let at = |[x, y]: [f32; 2]| tl + (tr - tl) * (x / w) + (bl - tl) * (y / h);
    // Where screen point `p` sits in frame pixels: the inverse of `at`.
    let back = |p: Pos2| {
        let (u, v, d) = (tr - tl, bl - tl, p - tl);
        let det = u.x * v.y - u.y * v.x;
        let a = (d.x * v.y - d.y * v.x) / det;
        let b = (u.x * d.y - u.y * d.x) / det;
        [a * w, b * h]
    };
    let shown = |content: &str| Text {
        content: content.to_owned(),
        ..base.with(&l.text)
    };

    let field = id.with("field");
    let viewer = crate::preview::viewer_rect(ui);
    let area = Rect::from_points(&[tl, tr, bl, br]).intersect(viewer);
    let first = ui.data_mut(|d| d.remove_temp::<Pos2>(id.with("at")));
    let pressed = ui.input(|i| {
        let p = i.pointer.interact_pos();
        p.filter(|_| i.pointer.primary_pressed())
    });
    let click = first.or(pressed.filter(|p| area.contains(*p)));
    // Over the text, so a click there keeps focus; carets come from the rendered layout.
    let out = ui
        .scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            ui.set_opacity(0.0);
            egui::TextEdit::multiline(&mut typing.content)
                .id(field)
                .frame(egui::Frame::NONE)
                .desired_width(area.width())
                .show(ui)
        })
        .inner;
    if first.is_some() {
        ui.memory_mut(|m| m.request_focus(field));
    }
    if let Some(p) = click {
        let i = text::hit(&shown(&typing.content), size, back(p));
        let mut state = out.state.clone();
        let one = egui::text::CCursorRange::one(egui::text::CCursor::new(i));
        state.cursor.set_char_range(Some(one));
        state.store(ui.ctx(), field);
        ui.memory_mut(|m| m.request_focus(field));
    }
    let focused = ui.memory(|m| m.has_focus(field));
    if !focused && first.is_none() && click.is_none() {
        ui.data_mut(|d| d.remove::<Typing>(id));
        return None;
    }
    ui.data_mut(|d| d.insert_temp(id, typing.clone()));

    let shown = shown(&typing.content);
    let carets = text::carets(&shown, size);
    let range = egui::TextEdit::load_state(ui.ctx(), field).and_then(|s| s.cursor.char_range());
    let (a, b) = range.map_or((0, 0), |r| (r.primary.index.0, r.secondary.index.0));
    let p = ui.painter().with_clip_rect(viewer);
    let (lo, hi) = (
        a.min(b).min(carets.len() - 1),
        a.max(b).min(carets.len() - 1),
    );
    for i in lo..hi {
        let ([x0, top, bottom], [x1, ..]) = (carets[i], carets[i + 1]);
        // A wrap or newline jumps lines; that char's highlight is skipped.
        if x1 > x0 && carets[i + 1][1] == top {
            let q = vec![
                at([x0, top]),
                at([x1, top]),
                at([x1, bottom]),
                at([x0, bottom]),
            ];
            let fill = DARK.light_blue.gamma_multiply(0.35);
            p.add(egui::Shape::convex_polygon(q, fill, Stroke::NONE));
        }
    }
    let [x, top, bottom] = carets[a.min(carets.len() - 1)];
    p.line_segment([at([x, top]), at([x, bottom])], Stroke::new(2.0, DARK.ink));

    (typing.content != base.content).then(|| {
        let text = Text {
            content: typing.content.clone(),
            ..base.clone()
        };
        let cmd = jvid_core::history::Command::SetText { id: asset.id, text };
        (cmd, Some(TYPING ^ asset.id.0 ^ (typing.session << 24)))
    })
}

/// Handles over the lowest-id selected video clip at playhead `t`, inside `frame`, the
/// project's on-screen rect; a drag's edit.
pub fn show(
    ui: &mut Ui,
    project: &Project,
    selected: &HashSet<ClipId>,
    t: Time,
    frame: Rect,
    wide: &dyn Fn(&Clip) -> Option<egui::TextureId>,
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
    let text = project
        .asset(c.asset)
        .and_then(|a| a.text.as_ref())
        .map(|text| {
            let layer = jvid_core::eval::video_at(seq, t)
                .into_iter()
                .find(|l| l.clip == c.id);
            layer.map_or_else(|| text.clone(), |l| text.with(&l.text))
        });
    let [tl, tr, bl, br] = bounds(project, c, &x, text.as_ref(), frame)?;
    let st = project.settings();
    let s = frame.width() / st.width as f32;
    // A shot of another shape covering the frame, as Reframe leaves one: the frame is a box
    // to aim over it.
    let drawn = Rect::from_points(&[tl, tr, bl, br]);
    let reframe = text.is_none()
        && crate::masks::upright(project, c)
            .is_some_and(|[w, h]| (w / h - st.width as f32 / st.height as f32).abs() > 0.01)
        && drawn.expand(0.5).contains_rect(frame)
        // Stretch can match the frame exactly. Only overflow is a wide shot to reframe;
        // an exact match retains the normal drag direction for the clip.
        && (drawn.width() > frame.width() + 0.5 || drawn.height() > frame.height() + 0.5);
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
    if reframe {
        grabs.push((Grab::Reframe, frame));
    }
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
    // Off the viewer, nothing draws or takes clicks; Fit zoom brings off-frame handles back.
    // A reframed shot's outline runs on across the viewer.
    let viewer = crate::preview::viewer_rect(ui);
    let clip = if reframe {
        viewer
    } else {
        frame.expand(STEM + HANDLE).intersect(viewer)
    };
    let mut edit = None;
    for (i, (grab, r)) in grabs.into_iter().enumerate() {
        let resp = ui.interact(r.intersect(clip), id.with(i), Sense::drag());
        let icon = match grab {
            Grab::Move | Grab::Reframe => egui::CursorIcon::Move,
            Grab::Scale => egui::CursorIcon::ResizeNwSe,
            Grab::Rotate => egui::CursorIcon::Alias,
        };
        let resp = resp.on_hover_cursor(icon);
        if resp.drag_started()
            && let Some(from) = resp.interact_pointer_pos()
        {
            let drag = start.as_ref().map_or(0, |s| s.drag) + 1;
            ui.data_mut(|d| {
                d.insert_temp(
                    id,
                    Start {
                        grab,
                        from,
                        x: x.clone(),
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
            let new = dragged(grab, &st.x, st.from, to, anchor, s, shift);
            if new != x {
                let names: &[&str] = match grab {
                    Grab::Move | Grab::Reframe => &["position"],
                    Grab::Scale => &["scale"],
                    Grab::Rotate => &["rotation"],
                };
                let cmd = crate::inspector::set_props(&[c], names, &new, t);
                edit = Some((cmd, Some(GESTURE ^ (st.drag << 16))));
            }
        }
    }

    let p = ui.painter().with_clip_rect(clip);
    // The viewer is black in every theme, so its overlay keeps the dark palette.
    let stroke = Stroke::new(1.0, DARK.ink);
    p.add(egui::Shape::closed_line(vec![tl, tr, br, bl], stroke));
    if reframe {
        // The wide shot outside the frame, dimmed, to show what the frame crops away.
        let wide = wide(c);
        for r in crate::app::tutorial::shaded(viewer, &[frame]) {
            if let Some(tex) = wide {
                let mut mesh = egui::Mesh::with_texture(tex);
                for (p, uv) in [
                    (tl, [0.0, 0.0]),
                    (tr, [1.0, 0.0]),
                    (br, [1.0, 1.0]),
                    (bl, [0.0, 1.0]),
                ] {
                    mesh.vertices.push(egui::epaint::Vertex {
                        pos: p,
                        uv: uv.into(),
                        color: egui::Color32::WHITE,
                    });
                }
                mesh.add_triangle(0, 1, 2);
                mesh.add_triangle(0, 2, 3);
                ui.painter().with_clip_rect(r).add(mesh);
            }
            p.rect_filled(r, 0.0, color::VIEWER.gamma_multiply(WIDE_DIM));
        }
        p.rect_stroke(
            frame,
            0.0,
            Stroke::new(1.0, DARK.light_blue),
            egui::StrokeKind::Inside,
        );
    }
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
            DARK.ink,
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
pub(crate) mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn move_scale_rotate() {
        let x = Transform::default();
        let a = pos2(100.0, 100.0);
        // Two screen points per project pixel.
        let m = dragged(Grab::Move, &x, a, pos2(120.0, 106.0), a, 2.0, false);
        assert_eq!(m.position, [10.0, 3.0]);
        let m = dragged(Grab::Move, &x, a, pos2(120.0, 106.0), a, 2.0, true);
        assert_eq!(m.position, [10.0, 0.0]);
        let m = dragged(Grab::Move, &x, a, pos2(104.0, 140.0), a, 2.0, true);
        assert_eq!(m.position, [0.0, 20.0]);
        // The reframe box moves right over the shot, so the shot moves left.
        let m = dragged(Grab::Reframe, &x, a, pos2(120.0, 106.0), a, 2.0, false);
        assert_eq!(m.position, [-10.0, -3.0]);
        // A corner twice as far from the anchor doubles scale; past the anchor it floors.
        let s = dragged(
            Grab::Scale,
            &x,
            pos2(150.0, 150.0),
            pos2(200.0, 200.0),
            a,
            2.0,
            false,
        );
        assert_eq!(s.scale, [2.0, 2.0]);
        let s = dragged(
            Grab::Scale,
            &x,
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
            &x,
            pos2(200.0, 100.0),
            pos2(100.0, 200.0),
            a,
            2.0,
            false,
        );
        assert!((r.rotation - 90.0).abs() < 1e-4);
        let to = a + Vec2::angled(50f32.to_radians()) * 100.0;
        let r = dragged(Grab::Rotate, &x, pos2(200.0, 100.0), to, a, 2.0, true);
        assert_eq!(r.rotation, 45.0);
    }

    /// A clip filling a 1920x1080 project with position animated from a key at 0.
    pub(crate) fn history() -> (jvid_core::history::History, ClipId) {
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
                    let edit = show(
                        ui,
                        h.project(),
                        &HashSet::from([id]),
                        Time(S),
                        frame,
                        &|_| None,
                    );
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

    fn has_amber(img: &image::RgbaImage) -> bool {
        let amber = color::amber();
        img.pixels().any(|p| {
            [amber.r(), amber.g(), amber.b()]
                .iter()
                .zip(p.0)
                .all(|(a, b)| a.abs_diff(b) < 40)
        })
    }

    #[test]
    fn the_overlay_draws_no_amber() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let (h, id) = history();
        let mut harness = egui_kittest::Harness::builder()
            .with_size([960.0, 540.0])
            .build_ui(move |ui| {
                let frame = Rect::from_min_size(Pos2::ZERO, vec2(960.0, 540.0));
                show(
                    ui,
                    h.project(),
                    &HashSet::from([id]),
                    Time(0),
                    frame,
                    &|_| None,
                );
            });
        harness.run();
        let img = harness.render().unwrap();
        assert!(!has_amber(&img), "amber in the overlay");
        // The Ink outline is drawn.
        assert!(img.pixels().any(|p| p.0[..3].iter().all(|c| *c > 200)));
    }

    #[test]
    fn a_reframe_box_dims_the_wide_shot_outside_it() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let (h, id) = history();
        let tall = h.project().reframed(1080, 1920, |_, _| None);
        let frame = Rect::from_center_size(pos2(480.0, 270.0), vec2(303.75, 540.0));
        let mut tex = None;
        let mut harness = egui_kittest::Harness::builder()
            .with_size([960.0, 540.0])
            .wgpu()
            .build_ui(move |ui| {
                // A white wide shot, so its dimmed edges read against the black viewer.
                let t = tex.get_or_insert_with(|| {
                    let img = egui::ColorImage::new([16, 9], vec![egui::Color32::WHITE; 144]);
                    ui.ctx().load_texture("wide", img, Default::default())
                });
                let id_ = t.id();
                show(ui, &tall, &HashSet::from([id]), Time(0), frame, &|_| {
                    Some(id_)
                });
            });
        harness.run();
        let img = harness.render().unwrap();
        // Outside the box, the shot shows dimmed: neither black nor white.
        let outside = img.get_pixel(100, 270).0;
        assert!(
            outside[..3].iter().all(|c| (40..200).contains(c)),
            "{outside:?}"
        );
        // Inside, the overlay leaves the program frame alone.
        assert_eq!(
            img.get_pixel(400, 200).0[3],
            0,
            "nothing painted in the frame"
        );
        harness.snapshot("reframe_wide_dimmed");
    }

    #[test]
    fn dragging_the_reframe_box_at_two_times_keys_twice_and_undoes_each() {
        use jvid_core::history::History;
        use jvid_core::time::FLICKS_PER_SECOND as S;
        let (h, id) = history();
        let tall = h.project().reframed(1080, 1920, |_, _| None);
        // The 9:16 frame fitted in a 960x540 viewer.
        let frame = Rect::from_center_size(pos2(480.0, 270.0), vec2(303.75, 540.0));
        let mut harness = egui_kittest::Harness::builder()
            .with_size([960.0, 540.0])
            .build_ui_state(
                move |ui, (h, t): &mut (History, Time)| {
                    let edit = show(ui, h.project(), &HashSet::from([id]), *t, frame, &|_| None);
                    if let Some((c, Some(k))) = edit {
                        h.apply_coalesced(k, c).unwrap();
                    }
                },
                (History::new(tall), Time(0)),
            );
        let keys = |h: &egui_kittest::Harness<'_, (History, Time)>| {
            h.state().0.project().sequence().tracks()[0].clips()[0].keyframes["position"].clone()
        };
        assert_eq!(keys(&harness).len(), 1, "Reframe keys the start");
        for secs in [1, 2] {
            harness.state_mut().1 = Time(secs * S);
            harness.run();
            let mut p = pos2(450.0, 270.0);
            harness.hover_at(p);
            harness.run();
            harness.drag_at(p);
            harness.run();
            for _ in 0..3 {
                p += vec2(10.0, 0.0);
                harness.hover_at(p);
                harness.run();
            }
            harness.drop_at(p);
            harness.run();
        }
        let after = keys(&harness);
        assert_eq!(after.len(), 3);
        assert_eq!([after[1].time, after[2].time], [Time(S), Time(2 * S)]);
        // The box went right, so the shot went left: 30 points at 0.28 points a pixel.
        let jvid_core::keyframe::Value::Vec2([x, _]) = after[1].value else {
            panic!()
        };
        assert!((x + 30.0 * 1920.0 / 540.0).abs() < 0.5, "{x}");
        assert!(harness.state_mut().0.undo());
        assert_eq!(keys(&harness).len(), 2);
        assert!(harness.state_mut().0.undo());
        assert_eq!(keys(&harness).len(), 1);
    }

    /// A title clip filling 0..1s of a default project, with a box so its hit and bounds
    /// don't depend on the installed fonts.
    fn title() -> (jvid_core::history::History, ClipId) {
        use jvid_core::history::{Command, Created, History};
        use jvid_core::model::NewClip;
        use jvid_core::time::FLICKS_PER_SECOND as S;
        let mut h = History::new(Project::default());
        let text = Text {
            box_color: [0.0, 0.0, 0.0, 1.0],
            ..Text::default()
        };
        let Ok(Created::Asset(asset)) = h.apply(Command::AddText {
            name: "Title".into(),
            text,
        }) else {
            panic!("text")
        };
        let Ok(Created::Track(track)) = h.apply(Command::AddTrack {
            kind: TrackKind::Video,
            name: "V1".into(),
        }) else {
            panic!("track")
        };
        let clip = NewClip {
            asset,
            source_in: Time(0),
            source_out: Time(S),
            start: Time(0),
            link: None,
        };
        h.apply(Command::AddClip { track, clip }).unwrap();
        let id = h.project().sequence().tracks()[0].clips()[0].id;
        (h, id)
    }

    #[test]
    fn a_titles_handles_fit_its_text() {
        use jvid_core::history::History;
        let (h, id) = title();
        let frame = Rect::from_min_size(Pos2::ZERO, vec2(960.0, 540.0));
        let p = h.project();
        let (_, c) = p.sequence().clip(id).unwrap();
        let text = p.asset(c.asset).unwrap().text.clone();
        let b = bounds(p, c, &c.transform, text.as_ref(), frame).unwrap();
        let r = Rect::from_points(&b);
        if r == frame {
            return; // No font, nothing drawn.
        }
        assert!(
            r.contains(frame.center()) && frame.contains_rect(r),
            "{r:?}"
        );
        assert!(r.width() < frame.width() / 2.0, "{r:?}");
        // A drag in the empty corner no longer moves the title.
        let mut harness = egui_kittest::Harness::builder()
            .with_size([960.0, 540.0])
            .build_ui_state(
                move |ui, h: &mut History| {
                    let edit = show(
                        ui,
                        h.project(),
                        &HashSet::from([id]),
                        Time(0),
                        frame,
                        &|_| None,
                    );
                    if let Some((c, _)) = edit {
                        h.apply(c).unwrap();
                    }
                },
                h,
            );
        let mut drag = |from: Pos2| {
            harness.hover_at(from);
            harness.run();
            harness.drag_at(from);
            harness.run();
            harness.hover_at(from + vec2(40.0, 0.0));
            harness.run();
            harness.drop_at(from + vec2(40.0, 0.0));
            harness.run();
            let p = harness.state().project();
            p.sequence().tracks()[0].clips()[0].transform.position
        };
        assert_eq!(drag(pos2(60.0, 60.0)), [0.0, 0.0]);
        assert_ne!(drag(frame.center()), [0.0, 0.0]);
    }

    #[test]
    fn double_click_finds_the_text_under_it() {
        let (h, id) = title();
        let mut harness = egui_kittest::Harness::builder()
            .with_size([960.0, 540.0])
            .with_step_dt(0.05)
            .build_ui_state(
                |ui, (p, hit): &mut (Project, Option<ClipId>)| {
                    let frame = Rect::from_min_size(Pos2::ZERO, vec2(960.0, 540.0));
                    *hit = hit.or(double_clicked_text(ui, p, Time(0), frame).map(|(c, _)| c));
                },
                (h.project().clone(), None),
            );
        harness.run();
        let click = |harness: &mut egui_kittest::Harness<'_, _>, p| {
            for _ in 0..2 {
                harness.hover_at(p);
                harness.event(egui::Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                });
                harness.event(egui::Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                });
                harness.step();
            }
        };
        click(&mut harness, pos2(20.0, 20.0));
        assert_eq!(harness.state().1, None, "a corner is empty");
        // Past the triple-click window.
        (0..20).for_each(|_| harness.step());
        click(&mut harness, pos2(480.0, 270.0));
        assert_eq!(harness.state().1, Some(id));
    }

    /// Double-clicks `p` within one step.
    fn double_click<S>(harness: &mut egui_kittest::Harness<'_, S>, p: Pos2) {
        harness.hover_at(p);
        harness.step();
        for pressed in [true, false, true, false] {
            harness.event(egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
        }
        harness.run();
    }

    #[test]
    fn typing_on_a_rotated_title_is_live_and_one_undo_step() {
        use jvid_core::history::{Command, History};
        let (mut h, id) = title();
        let c = h.project().sequence().tracks()[0].clips()[0].clone();
        let x = Transform {
            rotation: 30.0,
            ..c.transform.clone()
        };
        h.apply(Command::SetClipProps(vec![(id, x, c.keyframes.clone())]))
            .unwrap();
        let content = |h: &History| {
            let a = h.project().assets().iter().find(|a| a.text.is_some());
            a.and_then(|a| a.text.clone()).unwrap().content
        };
        let mut harness = egui_kittest::Harness::builder()
            .with_size([960.0, 540.0])
            .with_step_dt(0.05)
            .build_ui_state(
                |ui, h: &mut History| {
                    let frame = Rect::from_min_size(Pos2::ZERO, vec2(960.0, 540.0));
                    let edit = if editing(ui.ctx()) {
                        edit_text(ui, h.project(), Time(0), frame)
                    } else {
                        if let Some((c, at)) = double_clicked_text(ui, h.project(), Time(0), frame)
                        {
                            start_editing(ui.ctx(), h.project(), c, at);
                        }
                        None
                    };
                    if let Some((cmd, Some(k))) = edit {
                        h.apply_coalesced(k, cmd).unwrap();
                    }
                },
                h,
            );
        harness.run();
        double_click(&mut harness, pos2(480.0, 270.0));
        assert!(editing(&harness.ctx), "a double-click starts editing");
        harness.key_press(egui::Key::End);
        for s in [" ", "2"] {
            harness.event(egui::Event::Text(s.into()));
            harness.run();
        }
        assert_eq!(content(harness.state()), "Title 2", "the text updates live");
        if suite_ui::test_support::gpu() {
            harness.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Home);
            harness.run();
            let img = harness.render().unwrap();
            assert!(!has_amber(&img), "amber in the caret or selection");
        }
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(!editing(&harness.ctx), "Esc commits");
        assert!(harness.state_mut().undo());
        assert_eq!(content(harness.state()), "Title", "one undo restores it");
    }

    #[test]
    fn an_oversized_clip_leaves_the_transport_row_clickable() {
        use jvid_core::history::{Command, History};
        use suite_ui::tokens::size;
        let (mut h, id) = history();
        let c = h.project().sequence().tracks()[0].clips()[0].clone();
        let x = Transform {
            scale: [1.5, 1.5],
            ..c.transform.clone()
        };
        h.apply(Command::SetClipProps(vec![(id, x, c.keyframes.clone())]))
            .unwrap();
        let mut harness = egui_kittest::Harness::builder()
            .with_size([960.0, 540.0])
            .build_ui_state(
                move |ui, (h, clicks): &mut (History, u32)| {
                    let row = Rect::from_min_max(
                        pos2(0.0, ui.max_rect().bottom() - size::HEADER),
                        ui.max_rect().max,
                    );
                    // A stand-in for the play button, drawn first as the transport row is.
                    if ui.put(row, egui::Button::new("Play")).clicked() {
                        *clicks += 1;
                    }
                    let frame = Rect::from_min_size(Pos2::ZERO, vec2(960.0, 540.0 - size::HEADER));
                    if let Some((c, _)) = show(
                        ui,
                        h.project(),
                        &HashSet::from([id]),
                        Time(0),
                        frame,
                        &|_| None,
                    ) {
                        h.apply(c).unwrap();
                    }
                },
                (h, 0),
            );
        harness.run();
        // Under the box body, which a 150% clip stretches over the transport row.
        let p = pos2(480.0, 540.0 - size::HEADER / 2.0);
        harness.hover_at(p);
        harness.run();
        harness.event(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        });
        harness.event(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        });
        harness.run();
        assert_eq!(harness.state().1, 1);
    }
}
