use super::*;
use egui_kittest::{Harness, kittest::Queryable};

const S: i64 = FLICKS_PER_SECOND;

fn harness(project: Project) -> Harness<'static, (Timeline, History)> {
    let mut fresh = true;
    let mut h = Harness::builder()
        .with_size([720.0, 180.0])
        .with_step_dt(0.25)
        .with_max_steps(32)
        .build_ui_state(
            move |ui, (t, h): &mut (Timeline, History)| {
                if suite_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                let map = Keymap::default();
                for id in map.pressed(ui.ctx(), true) {
                    t.action(id, h.project());
                }
                if !map.held(ui.ctx(), "add_marker") {
                    t.release_marker();
                }
                if let Some((c, _)) = t.show(ui, h.project(), &HashSet::new(), &Keymap::default()) {
                    h.apply(c).unwrap();
                }
            },
            (Timeline::default(), History::new(project)),
        );
    h.run();
    h
}

#[test]
fn wheel_reaches_bottom_tracks_over_headers_clips_and_empty_lanes() {
    let mut hist = History::new(super::tests::project(2));
    for i in 2..=10 {
        hist.apply(Command::AddTrack {
            kind: TrackKind::Audio,
            name: format!("Audio {i}"),
        })
        .unwrap();
    }
    let asset = hist.project().assets()[0].id;
    let lower: Vec<_> = hist.project().sequence().tracks()[2..]
        .iter()
        .map(|t| t.id)
        .collect();
    for track in lower {
        hist.apply(Command::AddClip {
            track,
            clip: jvid_core::model::NewClip {
                asset,
                start: Time(0),
                source_in: Time(0),
                source_out: Time(2 * S),
                link: None,
            },
        })
        .unwrap();
    }
    for x in [30.0, 8.0 + size::TRACK_HEADER + 20.0, 650.0] {
        let mut h = harness(hist.project().clone());
        h.hover_at(pos2(x, 78.0));
        h.run();
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            phase: egui::TouchPhase::Move,
            delta: vec2(0.0, -1000.0),
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
        assert!(
            h.state().0.vscroll > 200.0,
            "wheel over {x}: {}",
            h.state().0.vscroll
        );
        let bottom = h
            .output()
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "Audio 10" => {
                    Some(Rect::from_min_size(t.pos, t.galley.size()))
                }
                _ => None,
            })
            .expect("bottom track painted");
        assert!(
            bottom.top() >= 48.0 && bottom.bottom() <= 172.0,
            "bottom header {bottom:?}"
        );
        let track = h
            .state()
            .1
            .project()
            .sequence()
            .tracks()
            .last()
            .unwrap()
            .clone();
        let lane = pos2(8.0 + size::TRACK_HEADER + 20.0, bottom.center().y);
        h.hover_at(lane);
        h.run();
        h.drag_at(lane);
        h.drop_at(lane);
        h.run();
        assert!(
            h.state().0.selected.contains(&track.clips()[0].id),
            "bottom lane lines up with its header"
        );
        h.get_all_by_label("Mute").last().unwrap().click();
        h.run();
        assert!(
            h.state()
                .1
                .project()
                .sequence()
                .track(track.id)
                .unwrap()
                .muted
        );
    }
}

#[test]
fn vertical_scrollbar_drag_and_trough_click_reach_the_bottom_lane() {
    let mut hist = History::new(super::tests::project(2));
    for i in 2..=10 {
        hist.apply(Command::AddTrack {
            kind: TrackKind::Audio,
            name: format!("Audio {i}"),
        })
        .unwrap();
    }
    for drag in [false, true] {
        let mut h = harness(hist.project().clone());
        let start = pos2(707.0, if drag { 55.0 } else { 154.0 });
        h.hover_at(start);
        h.run();
        h.drag_at(start);
        h.run();
        let end = if drag { pos2(707.0, 159.0) } else { start };
        h.hover_at(end);
        h.run();
        h.drop_at(end);
        h.run();
        assert!(
            h.state().0.vscroll >= 300.0,
            "scrollbar {drag}: {}",
            h.state().0.vscroll
        );
        assert_eq!(
            h.state().0.scroll,
            0.0,
            "a vertical scrollbar must not pan time"
        );
    }
}

#[test]
fn bin_drop_at_zero_accepts_the_left_edge_and_new_track_slot() {
    let mut hist = History::new(super::tests::project(2));
    hist.apply(Command::AddText {
        name: "Caption".into(),
        text: Default::default(),
    })
    .unwrap();
    let asset = hist.project().assets().last().unwrap().id;
    for (slot, dx) in [(false, 0.0), (false, -4.0), (true, 0.0), (true, -4.0)] {
        let mut h = harness(hist.project().clone());
        let before = h.state().1.project().clone();
        let at = pos2(
            8.0 + size::TRACK_HEADER + dx,
            if slot { 50.0 } else { 68.0 },
        );
        h.hover_at(at);
        h.run();
        egui::DragAndDrop::set_payload(&h.ctx, asset);
        h.drag_at(at);
        h.run_steps(2);
        assert_eq!(h.state().0.slot > 0.0, slot);
        assert!(
            h.output().shapes.iter().any(|s| matches!(&s.shape,
                egui::Shape::Rect(r) if r.fill == color::blue().gamma_multiply(0.3)
                    && (r.rect.left() - (8.0 + size::TRACK_HEADER + 1.0)).abs() < 0.1
            )),
            "zero-time ghost missing at {at:?}"
        );
        h.drop_at(at);
        h.run();
        let seq = h.state().1.project().sequence();
        let clip = seq
            .tracks()
            .iter()
            .flat_map(|t| t.clips())
            .find(|c| c.asset == asset)
            .expect("zero-time drop placed");
        assert_eq!(clip.start, Time(0));
        assert_eq!(seq.tracks().len(), 2 + usize::from(slot));
        assert!(h.state_mut().1.undo());
        assert_eq!(h.state().1.project(), &before);
    }
}

#[test]
fn holding_m_paints_a_growing_range_then_commits_one_marker_on_release() {
    let mut h = harness(super::tests::project(8));
    h.state_mut().0.seek(Time(S));
    h.state_mut().0.transport.toggle(0);
    h.key_down(Key::M);
    h.run();
    let ink = markers::PALETTE[0];
    assert!(
        h.output().shapes.iter().any(|s| matches!(&s.shape,
            egui::Shape::Path(p) if p.fill == ink
        )),
        "held marker start missing"
    );
    for seconds in [2, 3, 4] {
        h.state_mut().0.seek(Time(seconds * S));
        h.run();
        assert!(h.state().1.project().sequence().markers().is_empty());
        assert!(
            h.output().shapes.iter().any(|s| matches!(&s.shape,
                egui::Shape::Rect(r) if r.fill == ink.gamma_multiply(0.3)
                    && (r.rect.width() - (seconds - 1) as f32 * 20.0).abs() < 0.1
            )),
            "live range missing at {seconds}s"
        );
    }
    h.key_up(Key::M);
    h.run();
    assert_eq!(h.state().0.transport.speed(), 1);
    let [marker] = h.state().1.project().sequence().markers() else {
        panic!("one marker")
    };
    assert_eq!((marker.time, marker.duration), (Time(S), Time(3 * S)));
    assert!(h.state_mut().1.undo());
    assert!(h.state().1.project().sequence().markers().is_empty());
    assert!(!h.state_mut().1.undo());
}

#[test]
fn dragging_a_media_tile_or_row_to_the_zero_seam_places_it_undoably() {
    use crate::media_bin::MediaBin;
    let mut setup = History::new(super::tests::project(2));
    setup
        .apply(Command::AddText {
            name: "Caption".into(),
            text: Default::default(),
        })
        .unwrap();
    let asset = setup.project().assets().last().unwrap().id;
    for list in [false, true] {
        for slot in [false, true] {
            let mut fresh = true;
            let mut bin = MediaBin::default();
            bin.list = list;
            bin.query = "Caption".into();
            let mut h = Harness::builder()
                .with_size([1000.0, 300.0])
                .build_ui_state(
                    move |ui, (bin, t, hist): &mut (MediaBin, Timeline, History)| {
                        if suite_ui::test_support::install_theme(ui, &mut fresh) {
                            return;
                        }
                        let (left, right) = ui.max_rect().split_left_right_at_x(300.0);
                        ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
                            bin.show(ui, hist.project(), true, false, &HashSet::new());
                        });
                        let edit = ui
                            .scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
                                t.show(ui, hist.project(), &HashSet::new(), &Keymap::default())
                            })
                            .inner;
                        if let Some((c, _)) = edit {
                            hist.apply(c).unwrap();
                        }
                    },
                    (
                        bin,
                        Timeline::default(),
                        History::new(setup.project().clone()),
                    ),
                );
            h.run();
            let before = h.state().2.project().clone();
            let from = pos2(80.0, if list { 90.0 } else { 100.0 });
            let to = pos2(
                300.0 + size::TRACK_HEADER - 4.0,
                if slot { 50.0 } else { 68.0 },
            );
            h.hover_at(from);
            h.run();
            h.drag_at(from);
            h.run();
            for fraction in [0.1, 0.5, 1.0] {
                h.hover_at(from + (to - from) * fraction);
                h.run();
            }
            assert!(
                egui::DragAndDrop::has_payload_of_type::<AssetId>(&h.ctx),
                "Media drag list={list}"
            );
            assert!(
                h.output().shapes.iter().any(|s| matches!(&s.shape,
                    egui::Shape::Rect(r) if r.fill == color::blue().gamma_multiply(0.3)
                )),
                "zero-time ghost list={list}, slot={slot}"
            );
            h.drop_at(to);
            h.run();
            let seq = h.state().2.project().sequence();
            let clip = seq
                .tracks()
                .iter()
                .flat_map(|t| t.clips())
                .find(|c| c.asset == asset)
                .expect("caption placed");
            assert_eq!(clip.start, Time(0));
            assert_eq!(seq.tracks().len(), 2 + usize::from(slot));
            assert!(h.state_mut().2.undo());
            assert_eq!(h.state().2.project(), &before);
        }
    }
}
