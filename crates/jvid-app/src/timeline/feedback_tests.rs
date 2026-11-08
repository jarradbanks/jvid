use super::*;
use egui_kittest::{Harness, kittest::Queryable};

const S: i64 = FLICKS_PER_SECOND;

fn harness(project: Project) -> Harness<'static, (Timeline, History)> {
    feedback_harness(project, [720.0, 180.0], 1.0)
}

fn feedback_harness(
    project: Project,
    size: [f32; 2],
    scale: f32,
) -> Harness<'static, (Timeline, History)> {
    let mut fresh = true;
    let mut h = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(scale)
        .with_step_dt(0.25)
        .with_max_steps(32)
        .build_ui_state(
            move |ui, (t, h): &mut (Timeline, History)| {
                if j_ui::test_support::install_theme(ui, &mut fresh) {
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
                        if j_ui::test_support::install_theme(ui, &mut fresh) {
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
            let from = h.get_by_label("Caption").rect().center();
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

fn begin_clip(h: &mut Harness<'_, (Timeline, History)>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run();
    h.drag_at(from);
    h.run();
    h.hover_at(to);
    h.run();
}

fn begin_asset(h: &mut Harness<'_, (Timeline, History)>, asset: AssetId, at: Pos2) {
    let from = pos2(20.0, 16.0);
    h.hover_at(from);
    h.run();
    h.drag_at(from);
    h.run();
    egui::DragAndDrop::set_payload(&h.ctx, asset);
    h.hover_at(at);
    h.run();
}

fn planned_result(h: &Harness<'_, (Timeline, History)>) -> Sequence {
    let plan = h
        .state()
        .0
        .feedback
        .as_ref()
        .expect("validated pointer plan");
    plan.command
        .preview_timeline(h.state().1.project())
        .unwrap()
        .unwrap()
}

fn assert_commit(
    h: &mut Harness<'_, (Timeline, History)>,
    at: Pos2,
    result: &Sequence,
    before: &Project,
) {
    assert_eq!(
        h.state().1.project(),
        before,
        "preview must not edit history"
    );
    h.drop_at(at);
    h.run();
    assert_eq!(
        h.state().1.project().sequence(),
        result,
        "commit must match preview exactly"
    );
    assert!(h.state().0.feedback.is_none());
    assert!(h.state().0.snapped.is_none());
    let committed = h.state().1.project().clone();
    let json = jvid_core::file::to_json(&committed, std::path::Path::new("/tmp")).unwrap();
    let reopened =
        jvid_core::file::from_json(&json, std::path::Path::new("/tmp"), |_| false).unwrap();
    assert_eq!(reopened, committed);
    assert!(h.state_mut().1.undo());
    assert_eq!(h.state().1.project(), before);
    assert!(!h.state().1.can_undo(), "one edit only");
    assert!(h.state_mut().1.redo());
    assert_eq!(h.state().1.project(), &committed);
}

#[test]
fn overwrite_feedback_matches_split_survivors_and_one_undoable_commit() {
    let before = super::tests::project(4);
    let moving = before.sequence().tracks()[0].clips()[0].id;
    let next = before.sequence().tracks()[0].clips()[1].id;
    let mut h = harness(before.clone());
    h.state_mut().0.snapping = false;
    let y = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
    let from = pos2(8.0 + size::TRACK_HEADER + 20.0, y);
    let to = from + vec2(20.0, 0.0);
    begin_clip(&mut h, from, to);
    let result = planned_result(&h);
    assert_eq!(result.clip(moving).unwrap().1.start, Time(S));
    assert_eq!(result.clip(next).unwrap().1.start, Time(3 * S));
    let plan = h.state().0.feedback.as_ref().unwrap();
    assert_eq!(plan.overwritten.len(), 1);
    assert_eq!(
        (plan.overwritten[0].start, plan.overwritten[0].end),
        (Time(2 * S), Time(3 * S))
    );
    assert!(
        plan.ghosts
            .iter()
            .any(|s| !s.primary && s.start == Time(3 * S))
    );
    assert_commit(&mut h, to, &result, &before);
}

#[test]
fn source_bin_shift_and_alt_update_insert_and_stream_result_while_stationary() {
    let mut setup = History::new(super::tests::project(4));
    let asset = setup.project().assets()[0].id;
    let mut probe = setup.project().asset(asset).unwrap().probe.clone().unwrap();
    probe.audio_streams = 2;
    setup
        .apply(Command::SetAssetProbe {
            id: asset,
            probe: Some(probe),
        })
        .unwrap();
    setup
        .apply(Command::SetAssetInOut {
            id: asset,
            in_point: Some(Time(0)),
            out_point: Some(Time(S)),
        })
        .unwrap();
    let before = setup.project().clone();
    let mut h = harness(before.clone());
    h.state_mut().0.snapping = false;
    let at = pos2(8.0 + size::TRACK_HEADER + 20.0, 68.0);
    begin_asset(&mut h, asset, at);
    let overwrite = planned_result(&h);
    assert!(h.state().0.feedback.as_ref().unwrap().overwritten.len() >= 2);
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::SHIFT));
    h.run();
    let inserted = planned_result(&h);
    assert_ne!(inserted, overwrite);
    assert!(
        h.state()
            .0
            .feedback
            .as_ref()
            .unwrap()
            .overwritten
            .is_empty()
    );
    assert!(
        h.state()
            .0
            .feedback
            .as_ref()
            .unwrap()
            .ghosts
            .iter()
            .any(|s| !s.primary && s.start >= Time(3 * S))
    );
    h.event(egui::Event::ModifiersChanged(egui::Modifiers {
        alt: true,
        shift: true,
        ..Default::default()
    }));
    h.run();
    let merged = planned_result(&h);
    assert_eq!(inserted.tracks().len(), merged.tracks().len() + 1);
    assert!(
        merged.tracks()[1]
            .clips()
            .iter()
            .any(|c| c.start == Time(S) && c.stream.is_none())
    );
    assert_commit(&mut h, at, &merged, &before);
}

#[test]
fn command_modifier_replans_trim_ripple_neighbors_and_linked_partners() {
    let mut setup = History::new(super::tests::project(4));
    let tracks = setup.project().sequence().tracks();
    let first: Vec<_> = tracks.iter().map(|t| t.clips()[0].id).collect();
    let later: Vec<_> = tracks.iter().map(|t| t.clips()[1].id).collect();
    setup
        .apply(Command::LinkClips {
            ids: first.clone(),
            linked: true,
        })
        .unwrap();
    setup
        .apply(Command::LinkClips {
            ids: later.clone(),
            linked: true,
        })
        .unwrap();
    let before = setup.project().clone();
    let mut h = harness(before.clone());
    h.state_mut().0.snapping = false;
    let from = pos2(8.0 + size::TRACK_HEADER + 36.0, 68.0);
    let to = from + vec2(20.0, 0.0);
    begin_clip(&mut h, from, to);
    assert_eq!(
        planned_result(&h),
        *before.sequence(),
        "neighbor bounds non-ripple trim"
    );
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
    h.run();
    let result = planned_result(&h);
    for id in &first {
        assert_eq!(result.clip(*id).unwrap().1.end(), Time(3 * S));
    }
    for id in &later {
        assert_eq!(result.clip(*id).unwrap().1.start, Time(3 * S));
    }
    assert_eq!(h.state().0.feedback.as_ref().unwrap().ghosts.len(), 4);
    assert!(
        h.state()
            .0
            .feedback
            .as_ref()
            .unwrap()
            .label(h.state().0.transport.rate)
            .contains("+00:00:01:00")
    );
    assert_commit(&mut h, to, &result, &before);
}

#[test]
fn snap_label_and_shift_bypass_describe_the_final_plan() {
    let before = super::tests::project(2);
    let mut h = harness(before.clone());
    h.state_mut().0.seek(Time(5 * S));
    let from = pos2(8.0 + size::TRACK_HEADER + 20.0, 68.0);
    let to = from + vec2(96.0, 0.0);
    begin_clip(&mut h, from, to);
    assert_eq!(h.state().0.snapped, Some(Time(5 * S)));
    assert_eq!(
        h.state().0.snap_target,
        Some(edit_feedback::SnapTarget::Playhead)
    );
    assert!(h.output().shapes.iter().any(|s| matches!(&s.shape,
        egui::Shape::Text(t) if t.galley.text().contains("Snap: Playhead"))));
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::SHIFT));
    h.run();
    assert!(h.state().0.snapped.is_none());
    assert_ne!(planned_result(&h).tracks()[0].clips()[0].start, Time(5 * S));
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run();
    let result = planned_result(&h);
    assert_eq!(result.tracks()[0].clips()[0].start, Time(5 * S));
    assert_commit(&mut h, to, &result, &before);
}

#[test]
fn escape_focus_loss_and_outside_bin_release_never_commit() {
    for case in 0..4 {
        let before = super::tests::project(2);
        let mut h = harness(before.clone());
        let from = pos2(
            8.0 + size::TRACK_HEADER + if case == 1 { 36.0 } else { 20.0 },
            68.0,
        );
        let to = from + vec2(40.0, 0.0);
        if case == 3 {
            begin_asset(&mut h, before.assets()[0].id, to);
            h.hover_at(pos2(20.0, 16.0));
            h.run();
            assert!(h.state().0.feedback.is_none());
            h.drop_at(pos2(20.0, 16.0));
        } else {
            begin_clip(&mut h, from, to);
            assert!(h.state().0.feedback.is_some());
            if case == 2 {
                h.event(egui::Event::WindowFocused(false));
            } else {
                h.key_press(Key::Escape);
            }
            h.run();
            assert!(
                h.state().0.feedback.is_none(),
                "cancel case {case}: {:?}",
                h.state().0.feedback
            );
            h.drop_at(to);
        }
        h.run();
        assert_eq!(h.state().1.project(), &before);
        assert!(!h.state().1.can_undo());
        assert!(h.state().0.trim.is_none() && h.state().0.snapped.is_none());
    }
}

#[test]
fn locked_targets_and_downstream_linked_locks_refuse_preview_and_commit() {
    for case in 0..3 {
        let mut setup = History::new(super::tests::project(4));
        let tracks = setup.project().sequence().tracks();
        let video = tracks[0].clips()[0].id;
        let audio = tracks[1].clips()[0].id;
        let later: Vec<_> = tracks.iter().map(|t| t.clips()[1].id).collect();
        let mut settings = tracks[1].settings();
        if case == 0 {
            setup
                .apply(Command::LinkClips {
                    ids: vec![video, audio],
                    linked: true,
                })
                .unwrap();
        }
        if case == 2 {
            setup
                .apply(Command::LinkClips {
                    ids: later,
                    linked: true,
                })
                .unwrap();
        }
        settings.locked = true;
        settings.target = case == 1;
        setup.apply(Command::SetTrack(settings)).unwrap();
        let before = setup.project().clone();
        let mut h = harness(before.clone());
        let from = pos2(
            8.0 + size::TRACK_HEADER + if case == 2 { 36.0 } else { 20.0 },
            68.0,
        );
        let to = from + vec2(20.0, 0.0);
        if case == 1 {
            begin_asset(&mut h, before.assets()[0].id, to);
        } else {
            if case == 2 {
                h.event(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
            }
            begin_clip(&mut h, from, to);
        }
        assert!(
            h.state().0.feedback.is_none(),
            "locked case {case}: {:?}",
            h.state().0.feedback
        );
        assert!(matches!(
            h.state().0.feedback_error,
            Some(ModelError::Locked(_))
        ));
        h.drop_at(to);
        h.run();
        assert_eq!(h.state().1.project(), &before);
        assert!(!h.state().1.can_undo());
    }
}

#[test]
fn autoscroll_stationary_pointer_keeps_preview_equal_to_commit() {
    let before = super::tests::project(160);
    let mut h = feedback_harness(before.clone(), [420.0, 200.0], 1.5);
    h.state_mut().0.snapping = false;
    let from = pos2(8.0 + size::TRACK_HEADER + 20.0, 68.0);
    let to = pos2(404.0, 68.0);
    h.hover_at(from);
    h.run();
    h.drag_at(from);
    h.step();
    h.hover_at(to);
    for _ in 0..12 {
        h.step();
    }
    assert!(h.state().0.scroll > 0.0);
    let result = planned_result(&h);
    assert_commit(&mut h, to, &result, &before);
}

#[test]
fn narrow_fractional_scale_labels_fit_and_do_not_steal_trim_hits() {
    for scale in [1.0, 1.5, 2.0] {
        let before = super::tests::project(2);
        let mut h = feedback_harness(before.clone(), [240.0, 160.0], scale);
        h.state_mut().0.snapping = false;
        let from = pos2(8.0 + size::TRACK_HEADER + 36.0, 68.0);
        let to = from + vec2(12.0, 0.0);
        begin_clip(&mut h, from, to);
        assert!(matches!(
            h.state().0.feedback.as_ref().unwrap().command,
            Command::Trim { .. }
        ));
        let label = h
            .output()
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text().starts_with("Trim ") => {
                    Some((s.clip_rect, t))
                }
                _ => None,
            })
            .expect("trim delta label");
        assert!(Rect::from_min_max(pos2(8.0, 8.0), pos2(232.0, 152.0)).contains_rect(label.0));
        assert!(
            label
                .0
                .contains_rect(Rect::from_min_size(label.1.pos, label.1.galley.size())),
            "scale {scale}: clip {:?}, text {:?}, size {:?}",
            label.0,
            label.1.pos,
            label.1.galley.size()
        );
        let result = planned_result(&h);
        assert_commit(&mut h, to, &result, &before);
    }
}

#[test]
fn clipped_clip_ends_are_not_phantom_trim_handles() {
    let mut setup = History::new(super::tests::project(0));
    setup
        .apply(Command::AddClip {
            track: setup.project().sequence().tracks()[0].id,
            clip: jvid_core::model::NewClip {
                asset: setup.project().assets()[0].id,
                start: Time(0),
                source_in: Time(0),
                source_out: Time(30 * S),
                link: None,
            },
        })
        .unwrap();
    for scale in [1.0, 1.5] {
        let mut h = feedback_harness(setup.project().clone(), [240.0, 160.0], scale);
        h.state_mut().0.scroll = 1.0;
        h.run();
        let from = pos2(8.0 + size::TRACK_HEADER + 2.0, 68.0);
        begin_clip(&mut h, from, from + vec2(12.0, 0.0));
        assert!(h.state().0.trim.is_none());
        assert!(matches!(
            h.state().0.feedback.as_ref().unwrap().command,
            Command::MoveSelection { .. }
        ));
    }
}

#[test]
fn snap_targets_name_clip_edges_markers_and_in_out_with_stable_ties() {
    let mut setup = History::new(super::tests::project(2));
    setup
        .apply(Command::RenameAsset {
            id: setup.project().assets()[0].id,
            name: Some("Game".into()),
        })
        .unwrap();
    setup
        .apply(Command::AddMarker {
            time: Time(4 * S),
            duration: Time(S),
            name: "Beat".into(),
        })
        .unwrap();
    setup
        .apply(Command::SetInOut {
            in_point: Some(Time(6 * S)),
            out_point: Some(Time(8 * S)),
        })
        .unwrap();
    let mut timeline = Timeline::default();
    timeline.sync(setup.project());
    timeline.seek(Time(20 * S));
    for (seconds, text) in [
        (0, "Game Start"),
        (2, "Game End"),
        (4, "Marker Beat"),
        (5, "Marker End Beat"),
        (6, "In Point"),
        (8, "Out Point"),
    ] {
        let at = Time(seconds * S);
        let shift = timeline.snap(setup.project().sequence(), &[], &[at + Time(S / 30)]);
        assert_eq!(shift, Time(-S / 30));
        assert_eq!(timeline.snapped, Some(at));
        assert!(
            timeline
                .snap_target
                .unwrap()
                .label(setup.project())
                .contains(text)
        );
    }
    // At a shared clip edge, label and time must come from the same winning target.
    timeline.seek(Time(2 * S));
    timeline.snap(setup.project().sequence(), &[], &[Time(2 * S)]);
    assert_eq!(
        timeline.snap_target,
        Some(edit_feedback::SnapTarget::Playhead)
    );
}

#[test]
fn trim_bounds_clear_snap_labels_when_neighbor_prevents_the_promised_edge() {
    let mut h = harness(super::tests::project(4));
    h.state_mut().0.seek(Time(3 * S));
    let from = pos2(8.0 + size::TRACK_HEADER + 36.0, 68.0);
    begin_clip(&mut h, from, from + vec2(20.0, 0.0));
    assert!(h.state().0.snapped.is_none());
    assert!(h.state().0.snap_target.is_none());
    assert_eq!(planned_result(&h), *h.state().1.project().sequence());
}

#[test]
fn render_feedback_artifacts() {
    if !j_ui::test_support::gpu() {
        return;
    }
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/timeline-feedback");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, width, scale, trim, snap) in [
        ("overwrite", 720.0, 1.0, false, false),
        ("ripple", 720.0, 1.0, true, false),
        ("snap", 720.0, 1.5, false, true),
        ("narrow-trim-150", 240.0, 1.5, true, false),
    ] {
        let mut fresh = true;
        let mut h = Harness::builder()
            .with_size([width, 180.0])
            .with_pixels_per_point(scale)
            .with_step_dt(0.25)
            .with_max_steps(32)
            .wgpu()
            .build_ui_state(
                move |ui, (timeline, history): &mut (Timeline, History)| {
                    if j_ui::test_support::install_theme(ui, &mut fresh) {
                        return;
                    }
                    let _ =
                        timeline.show(ui, history.project(), &HashSet::new(), &Keymap::default());
                },
                (Timeline::default(), History::new(super::tests::project(4))),
            );
        h.run();
        h.state_mut().0.snapping = snap;
        if snap {
            h.state_mut().0.seek(Time(5 * S));
        }
        if trim {
            h.event(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
        }
        let from = pos2(
            8.0 + size::TRACK_HEADER + if trim { 36.0 } else { 20.0 },
            68.0,
        );
        let to = from
            + vec2(
                if snap {
                    96.0
                } else if width < 300.0 {
                    8.0
                } else {
                    20.0
                },
                0.0,
            );
        begin_clip(&mut h, from, to);
        assert!(h.state().0.feedback.is_some());
        h.render()
            .unwrap()
            .save(dir.join(format!("{name}.png")))
            .unwrap();
    }
}
