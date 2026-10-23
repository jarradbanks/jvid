//! The easing picker: presets plus a curve with draggable bezier handles.

use egui::{Pos2, Rect, Sense, Shape, Stroke, Ui, vec2};
use jvid_core::keyframe::Easing;
use suite_ui::tokens::{color, space};
use suite_ui::widgets::Segmented;

use crate::i18n::tr;

/// The presets' names, in [`index`] order.
fn names() -> [&'static str; 6] {
    [
        tr!("easing.linear"),
        tr!("easing.ease_in"),
        tr!("easing.ease_out"),
        tr!("easing.ease_in_out"),
        tr!("easing.hold"),
        tr!("easing.bezier"),
    ]
}

/// The curve widget's side.
const CURVE: f32 = 140.0;

fn index(e: Easing) -> usize {
    match e {
        Easing::Linear => 0,
        Easing::EaseIn => 1,
        Easing::EaseOut => 2,
        Easing::EaseInOut => 3,
        Easing::Hold => 4,
        Easing::Bezier(_) => 5,
    }
}

/// The bezier control points `e` is, if it is one; linear is the straight bezier.
fn points(e: Easing) -> Option<[f64; 4]> {
    match e {
        Easing::Linear => Some([0.0, 0.0, 1.0, 1.0]),
        Easing::EaseIn => Some([0.42, 0.0, 1.0, 1.0]),
        Easing::EaseOut => Some([0.0, 0.0, 0.58, 1.0]),
        Easing::EaseInOut => Some([0.42, 0.0, 0.58, 1.0]),
        Easing::Hold => None,
        Easing::Bezier(p) => Some(p),
    }
}

fn preset(i: usize, old: Easing) -> Easing {
    match i {
        0 => Easing::Linear,
        1 => Easing::EaseIn,
        2 => Easing::EaseOut,
        3 => Easing::EaseInOut,
        4 => Easing::Hold,
        _ => Easing::Bezier(points(old).unwrap_or([0.25, 0.1, 0.25, 1.0])),
    }
}

/// Moves handle `h` (0 or 1) of `p` to curve-space `(x, y)`; x stays in 0..=1 so the curve
/// stays a function of time, y may overshoot.
fn drag(mut p: [f64; 4], h: usize, x: f64, y: f64) -> [f64; 4] {
    p[2 * h] = x.clamp(0.0, 1.0);
    p[2 * h + 1] = y.clamp(-1.0, 2.0);
    p
}

/// Edits `easing`; the new easing and whether a handle drag is still going, once it changes.
pub fn editor(ui: &mut Ui, easing: Easing) -> Option<(Easing, bool)> {
    let mut out = None;
    let mut i = index(easing);
    if ui.add(Segmented::new(&mut i, &names())).changed() && i != index(easing) {
        out = Some((preset(i, easing), false));
    }
    ui.add_space(space::S8);
    let shown = out.map_or(easing, |(e, _)| e);
    // Curve space: y from -0.25 to 1.25 so overshoot shows.
    let (rect, _) = ui.allocate_exact_size(vec2(CURVE, CURVE), Sense::hover());
    let inner = Rect::from_min_max(
        rect.min + vec2(0.0, CURVE / 6.0),
        rect.max - vec2(0.0, CURVE / 6.0),
    );
    let to = |x: f64, y: f64| inner.lerp_inside(vec2(x as f32, 1.0 - y as f32));
    let from = |p: Pos2| {
        let v = (p - inner.min) / inner.size();
        (f64::from(v.x), 1.0 - f64::from(v.y))
    };
    let painter = ui.painter_at(rect);
    painter.rect_stroke(
        inner,
        2.0,
        Stroke::new(1.0, color::INK_3),
        egui::StrokeKind::Inside,
    );
    let curve: Vec<Pos2> = (0..=48)
        .map(|s| {
            let t = f64::from(s) / 48.0;
            to(t, shown.apply(t))
        })
        .collect();
    painter.add(Shape::line(curve, Stroke::new(2.0, color::LIGHT_BLUE)));
    if let Some(p) = points(shown) {
        for h in 0..2 {
            let (anchor, handle) = (to(h as f64, h as f64), to(p[2 * h], p[2 * h + 1]));
            painter.line_segment([anchor, handle], Stroke::new(1.0, color::INK_2));
            let id = ui.id().with(("easing handle", h));
            let r = ui.interact(
                Rect::from_center_size(handle, vec2(12.0, 12.0)),
                id,
                Sense::drag(),
            );
            let r = r.on_hover_cursor(egui::CursorIcon::Grab);
            let fill = if r.hovered() || r.dragged() {
                color::INK
            } else {
                color::LIGHT_BLUE
            };
            painter.circle_filled(handle, 4.0, fill);
            if let Some(at) = r.interact_pointer_pos().filter(|_| r.dragged()) {
                let (x, y) = from(at);
                out = Some((Easing::Bezier(drag(p, h, x, y)), true));
            } else if r.drag_stopped() {
                out = Some((Easing::Bezier(p), false));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_round_trip_and_bezier_starts_from_the_old_curve() {
        for i in 0..5 {
            assert_eq!(index(preset(i, Easing::Linear)), i);
        }
        assert_eq!(
            preset(5, Easing::EaseIn),
            Easing::Bezier([0.42, 0.0, 1.0, 1.0])
        );
        assert_eq!(index(preset(5, Easing::Hold)), 5);
    }

    #[test]
    fn handles_keep_x_in_range() {
        let p = drag([0.0, 0.0, 1.0, 1.0], 1, 1.5, 3.0);
        assert_eq!(p, [0.0, 0.0, 1.0, 2.0]);
        let p = drag(p, 0, -0.2, 0.5);
        assert_eq!(p, [0.0, 0.5, 1.0, 2.0]);
    }
}
