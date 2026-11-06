//! Viewer-only geometry. Project points are pixels from frame centre, with y down;
//! screen points are logical egui points. None of this state belongs in a project.

use egui::{Pos2, Rect, Vec2, pos2, vec2};

/// Room for a 24-point rotation stem, its hit target and breathing space, at any DPI.
pub const EDITING_GUTTER: f32 = 40.0;

/// Explicit view recovery and panning. Selection bounds are captured once, never followed
/// while a transform handle moves. Fit clears both fields.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct View {
    pub selection: Option<Rect>,
    pub pan: Vec2,
}

/// The clipped body excludes headers/transport; editing reserves screen-space controls;
/// frame is the complete project picture, including portions outside the clipped body.
#[derive(Clone, Copy, Debug)]
pub struct ViewerGeometry {
    pub body: Rect,
    pub editing: Rect,
    pub frame: Rect,
    pub project_size: Vec2,
}

impl ViewerGeometry {
    /// `zoom` is physical screen pixels per project pixel (100% = 1), or Fit.
    /// Selection recovery takes precedence until Fit or a percentage is chosen.
    pub fn new(body: Rect, size: Vec2, zoom: Option<f32>, dpi: f32, view: View) -> Self {
        let size = size.max(Vec2::splat(1.0));
        // Tiny dock panels keep finite geometry rather than inverting the inner rect.
        let gutter = vec2(
            EDITING_GUTTER.min((body.width() - 1.0).max(0.0) / 2.0),
            EDITING_GUTTER.min((body.height() - 1.0).max(0.0) / 2.0),
        );
        let editing = body.shrink2(gutter);
        let target = view
            .selection
            .unwrap_or_else(|| Rect::from_center_size(Pos2::ZERO, size));
        let fitted = || {
            (editing.width() / target.width().max(1.0))
                .min(editing.height() / target.height().max(1.0))
                .max(f32::EPSILON)
        };
        let scale = if view.selection.is_some() {
            fitted()
        } else {
            zoom.map_or_else(fitted, |z| z / dpi.max(f32::EPSILON))
        };
        let centre = editing.center() - target.center().to_vec2() * scale + view.pan;
        Self {
            body,
            editing,
            frame: Rect::from_center_size(centre, size * scale),
            project_size: size,
        }
    }

    /// Adapt an existing complete frame rect. Overlays must use this rect, never the
    /// frame/body intersection, otherwise zoomed and panned coordinates change.
    pub fn from_frame(body: Rect, frame: Rect, size: Vec2) -> Self {
        Self {
            body,
            editing: body,
            frame,
            project_size: size.max(Vec2::splat(1.0)),
        }
    }

    pub fn scale(self) -> f32 {
        self.frame.width() / self.project_size.x
    }

    pub fn project_to_screen(self, point: Pos2) -> Pos2 {
        self.frame.center() + point.to_vec2() * self.scale()
    }

    pub fn screen_to_project(self, point: Pos2) -> Pos2 {
        ((point - self.frame.center()) / self.scale()).to_pos2()
    }

    /// Frame-relative normalized point, for title placement and other anchored content.
    pub fn screen_to_normalized(self, point: Pos2) -> Pos2 {
        let p = self.screen_to_project(point);
        pos2(
            p.x / self.project_size.x + 0.5,
            p.y / self.project_size.y + 0.5,
        )
    }

    pub fn screen_rect_to_project(self, rect: Rect) -> Rect {
        Rect::from_min_max(
            self.screen_to_project(rect.min),
            self.screen_to_project(rect.max),
        )
    }

    pub fn project_rect_to_screen(self, rect: Rect) -> Rect {
        Rect::from_min_max(
            self.project_to_screen(rect.min),
            self.project_to_screen(rect.max),
        )
    }

    /// Drawing/clicking the picture excludes the gutter, transport and adjacent panels.
    pub fn picture(self) -> Rect {
        self.frame.intersect(self.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_reserves_logical_controls_for_every_shape_and_dpi() {
        for body_size in [vec2(960.0, 492.0), vec2(240.0, 240.0)] {
            let body = Rect::from_min_size(pos2(17.0, 23.0), body_size);
            for size in [
                vec2(1920.0, 1080.0),
                vec2(1080.0, 1920.0),
                Vec2::splat(1080.0),
            ] {
                let mut previous = None;
                for dpi in [1.0, 1.5, 2.0] {
                    let g = ViewerGeometry::new(body, size, None, dpi, View::default());
                    assert!(body.contains_rect(g.frame.expand(32.0)));
                    assert_eq!(g.picture(), g.frame);
                    if let Some(frame) = previous {
                        assert_eq!(g.frame, frame, "Fit uses logical points at every DPI");
                    }
                    previous = Some(g.frame);
                }
            }
        }
    }

    #[test]
    fn mapping_roundtrips_fit_zoom_pan_and_off_frame_recovery() {
        let body = Rect::from_min_size(pos2(60.0, 90.0), vec2(480.0, 300.0));
        let size = vec2(1920.0, 1080.0);
        let selection = Rect::from_min_max(pos2(4000.0, -3000.0), pos2(9000.0, 2000.0));
        for view in [
            View::default(),
            View {
                selection: None,
                pan: vec2(-77.0, 120.0),
            },
            View {
                selection: Some(selection),
                pan: Vec2::ZERO,
            },
        ] {
            for zoom in [None, Some(1.0), Some(2.0)] {
                let g = ViewerGeometry::new(body, size, zoom, 1.5, view);
                for p in [Pos2::ZERO, pos2(-960.0, -540.0), pos2(9000.0, -3000.0)] {
                    assert!(g.screen_to_project(g.project_to_screen(p)).distance(p) < 0.002);
                }
                let r = Rect::from_center_size(Pos2::ZERO, vec2(340.0, 140.0));
                let back = g.screen_rect_to_project(g.project_rect_to_screen(r));
                assert!(back.min.distance(r.min) < 0.002);
                assert!(back.max.distance(r.max) < 0.002);
                if view.selection.is_some() {
                    assert!(body.contains_rect(g.project_rect_to_screen(selection).expand(32.0)));
                }
                let origin = g.screen_to_normalized(g.frame.min);
                assert!(origin.distance(Pos2::ZERO) < 1e-5);
            }
        }
    }

    #[test]
    fn percentage_zoom_uses_physical_pixels_and_clips_only_hits() {
        let body = Rect::from_min_size(Pos2::ZERO, vec2(240.0, 160.0));
        let g = ViewerGeometry::new(body, vec2(1920.0, 1080.0), Some(1.0), 2.0, View::default());
        assert_eq!(g.scale(), 0.5);
        assert_eq!(g.frame.size(), vec2(960.0, 540.0));
        assert_eq!(g.picture(), body);
        assert_eq!(g.screen_to_project(g.frame.min), pos2(-960.0, -540.0));
        let tiny = ViewerGeometry::new(
            Rect::from_min_size(Pos2::ZERO, vec2(40.0, 20.0)),
            Vec2::splat(1080.0),
            None,
            1.0,
            View::default(),
        );
        assert!(tiny.scale().is_finite() && tiny.scale() > 0.0);
    }
}
