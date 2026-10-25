use super::*;

const S: i64 = FLICKS_PER_SECOND;

/// Real pointer events go through show(), including follow() and egui's click/drag decision.
/// No GPU or wall-clock sleeps are needed to reproduce these gesture lifecycle failures.
struct Gesture {
    ctx: egui::Context,
    timeline: Timeline,
    project: Project,
}

impl Gesture {
    fn new() -> Self {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut gesture = Self {
            ctx,
            timeline: Timeline::default(),
            project: tests::project(120),
        };
        gesture.frame(0.0, vec![]);
        assert_eq!(gesture.timeline.span(), 40.0);
        gesture
    }

    fn frame(&mut self, time: f64, events: Vec<egui::Event>) {
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    vec2(size::TRACK_HEADER + 800.0, 300.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                assert!(
                    self.timeline
                        .show(ui, &self.project, &HashSet::new(), &Keymap::default())
                        .is_none()
                );
            },
        );
        output.textures_delta.clear();
    }

    fn ruler(&self, seconds: f64) -> Pos2 {
        pos2(
            self.timeline
                .view(size::TRACK_HEADER)
                .x(Time((seconds * S as f64) as i64)),
            markers::LANE + size::RULER / 2.0,
        )
    }

    fn button(&mut self, time: f64, at: Pos2, pressed: bool) {
        self.frame(
            time,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }

    fn playback(&mut self, time: f64, head: Time, events: Vec<egui::Event>) {
        // Program writes its audio-clock head back into the timeline before show(). Any
        // change here would be read as a new seek by sync_program(), flushing its output.
        self.timeline.seek(head);
        self.frame(time, events);
        assert_eq!(
            self.timeline.playhead(),
            head,
            "held pointer submitted a seek"
        );
    }
}

#[test]
fn stationary_ruler_drag_does_not_page_and_remap_its_seek_target() {
    let mut g = Gesture::new();
    let from = g.ruler(38.5);
    let at = g.ruler(39.0);
    g.button(0.1, from, true);
    g.frame(0.2, vec![egui::Event::PointerMoved(at)]);
    assert_eq!(g.timeline.playhead(), Time(39 * S));
    for frame in 1..=90 {
        let head = Time(39 * S) + g.timeline.transport.rate.time_floor(frame);
        g.playback(0.2 + frame as f64 / 30.0, head, vec![]);
        assert_eq!(g.timeline.scroll, 0.0, "playback remapped the held pointer");
    }
    g.button(3.3, at, false);
    g.playback(3.4, Time(43 * S), vec![]);
    assert_eq!(
        g.timeline.scroll, 43.0,
        "normal paging resumes after release"
    );
}

#[test]
fn stationary_ruler_press_does_not_reseek_when_click_becomes_drag() {
    let mut g = Gesture::new();
    let at = g.ruler(10.0);
    g.button(0.1, at, true);
    assert_eq!(g.timeline.playhead(), Time(10 * S));
    g.playback(0.6, Time(10 * S + S / 2), vec![]);
    // Default max_click_duration is 0.8s: this unchanged press now becomes a drag.
    g.playback(1.0, Time(10 * S + 9 * S / 10), vec![]);
    g.playback(1.1, Time(11 * S), vec![]);
    g.button(1.2, at, false);
    assert_eq!(g.timeline.playhead(), Time(11 * S));
    g.button(1.3, at, true);
    assert_eq!(
        g.timeline.playhead(),
        Time(10 * S),
        "a new press can revisit the target"
    );
}

#[test]
fn ruler_drag_threshold_within_same_frame_does_not_reseek() {
    let mut g = Gesture::new();
    g.timeline.zoom = MAX_ZOOM; // One frame spans 80 points; cross the drag threshold within it.
    g.frame(0.05, vec![]);
    let at = g.ruler(0.1);
    g.button(0.1, at, true);
    assert_eq!(g.timeline.playhead(), Time(S / 10));
    let head = g.timeline.transport.rate.time_floor(4);
    g.playback(
        0.2,
        head,
        vec![egui::Event::PointerMoved(at + vec2(10.0, 0.0))],
    );
    g.playback(0.3, head + g.timeline.transport.rate.time_floor(1), vec![]);
}

#[test]
fn playhead_drag_does_not_page_and_remap_its_seek_target() {
    let mut g = Gesture::new();
    g.timeline.seek(Time(38 * S + S / 2));
    g.frame(0.05, vec![]);
    let from = pos2(g.ruler(38.5).x, 250.0);
    let at = pos2(g.ruler(39.0).x, from.y);
    g.button(0.1, from, true);
    g.frame(0.2, vec![egui::Event::PointerMoved(at)]);
    assert_eq!(g.timeline.playhead(), Time(39 * S));
    g.playback(1.2, Time(40 * S), vec![]);
    assert_eq!(g.timeline.scroll, 0.0);
    g.playback(1.3, Time(40 * S + S / 10), vec![]);
    g.button(1.4, at, false);
    g.playback(1.5, Time(41 * S), vec![]);
    assert_eq!(g.timeline.scroll, 41.0);
}

#[test]
fn ruler_seeks_forward_and_backward_while_paused_and_scrolls_past_edges() {
    let mut g = Gesture::new();
    let at = g.ruler(10.0);
    g.button(0.1, at, true);
    for (i, seconds) in [20.0, 5.0, 39.0].into_iter().enumerate() {
        let at = g.ruler(seconds);
        g.frame(0.2 + i as f64 / 10.0, vec![egui::Event::PointerMoved(at)]);
        assert_eq!(g.timeline.playhead(), Time((seconds * S as f64) as i64));
        assert_eq!(g.timeline.transport.speed(), 0);
    }
    let past_edge = pos2(size::TRACK_HEADER + 820.0, at.y);
    g.frame(0.5, vec![egui::Event::PointerMoved(past_edge)]);
    let scrolled = g.timeline.scroll;
    let head = g.timeline.playhead();
    assert!(scrolled > 0.0);
    assert!(head > Time(40 * S));
    g.frame(0.6, vec![]);
    assert!(
        g.timeline.scroll > scrolled,
        "intentional edge scrolling continues"
    );
    assert!(g.timeline.playhead() > head);
    g.button(0.7, past_edge, false);
    let head = g.timeline.playhead();
    let scrolled = g.timeline.scroll;
    g.frame(0.8, vec![]);
    assert_eq!(g.timeline.playhead(), head);
    assert_eq!(g.timeline.scroll, scrolled);
}
