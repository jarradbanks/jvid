//! Real keyboard/canvas/preset events, and bounds at the supported minimum window size.

use egui::{Key, Pos2};
use jvid_core::history::Created;
use jvid_core::model::{ClipId, TrackKind};
use jvid_core::time::FLICKS_PER_SECOND as S;

use super::*;
use crate::timeline::Tool;

fn editor(size: [f32; 2], scale: f32) -> Harness<'static, App> {
    let mut h = builder(size, scale)
        .with_step_dt(0.05)
        .build_eframe(|cc| App::new(cc, &Default::default()));
    h.state_mut().on_welcome = false;
    h.run();
    h
}

fn click(h: &mut Harness<'_, App>, at: Pos2) {
    h.hover_at(at);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
    }
    h.run();
}

fn content(app: &App, clip: ClipId) -> String {
    let p = app.history.project();
    let (_, c) = p.sequence().clip(clip).unwrap();
    p.asset(c.asset)
        .unwrap()
        .text
        .as_ref()
        .unwrap()
        .content
        .clone()
}

#[test]
fn text_tool_click_creates_and_edits_one_real_clip_with_separate_undo_steps() {
    let mut h = editor([960.0, 600.0], 1.0);
    let before = h.state().history.project().clone();
    h.state_mut().timeline.seek(Time(2 * S));
    h.state_mut().run_action("focus_program", false);
    h.key_press(Key::T);
    h.run();
    assert_eq!(h.state().timeline.tool(), Tool::Text);
    let canvas = h
        .get_all_by_label("Text Tool")
        .find(|n| n.rect().width() > 32.0)
        .unwrap()
        .rect();
    let at = canvas.center();
    h.hover_at(at);
    h.run();
    assert_eq!(
        h.output().platform_output.cursor_icon,
        egui::CursorIcon::Text
    );
    click(&mut h, at);
    assert_eq!(h.state().timeline.selected().len(), 1);
    let id = *h.state().timeline.selected().iter().next().unwrap();
    let (_, clip) = h.state().history.project().sequence().clip(id).unwrap();
    assert_eq!(clip.start, Time(2 * S));
    assert_eq!(
        clip.duration(),
        Time(i64::from(jvid_core::settings::DEFAULT_STILL_SECS) * S)
    );
    assert!(crate::handles::editing(&h.ctx));
    h.key_press(Key::End);
    h.event(egui::Event::Text(" edited".into()));
    h.run();
    assert_eq!(content(h.state(), id), "Title edited");
    h.key_press(Key::Escape);
    h.run();
    assert!(!crate::handles::editing(&h.ctx));
    assert!(h.state_mut().history.undo());
    assert_eq!(content(h.state(), id), "Title");
    assert!(h.state_mut().history.undo());
    assert_eq!(h.state().history.project(), &before);
    assert!(h.state_mut().history.redo());
    assert_eq!(content(h.state(), id), "Title");
    assert!(h.state_mut().history.redo());
    assert_eq!(content(h.state(), id), "Title edited");
    h.state_mut().run_action("focus_program", false);
    h.key_press(Key::V);
    h.run();
    assert_eq!(h.state().timeline.tool(), Tool::Selection);
    h.key_press(Key::C);
    h.run();
    assert_eq!(h.state().timeline.tool(), Tool::Razor);
}

#[test]
fn program_effect_drop_takes_priority_over_the_active_text_tool() {
    let library = tempfile::tempdir().unwrap();
    crate::effects::scan_for_test(library.path());
    let mut h = editor([960.0, 600.0], 1.0);
    let id = crate::playback::tests::add_solid(
        &mut h.state_mut().history,
        [0.2, 0.3, 0.4, 1.0],
        Time(0),
    );
    {
        let app = h.state_mut();
        app.timeline
            .select_clip(app.history.project().sequence(), id);
        app.run_action("focus_program", false);
    }
    h.key_press(Key::T);
    h.run();
    assert_eq!(h.state().timeline.tool(), Tool::Text);
    let at = h
        .get_all_by_label("Text Tool")
        .find(|node| node.rect().width() > 32.0)
        .unwrap()
        .rect()
        .center();
    let before = h.state().history.project().clone();
    egui::DragAndDrop::set_payload(&h.ctx, crate::effects::Drag("vignette".into()));
    h.drag_at(at);
    h.run();
    h.drop_at(at);
    h.run();
    let project = h.state().history.project();
    assert_eq!(project.assets().len(), before.assets().len());
    let effects = &project.sequence().clip(id).unwrap().1.transform.effects;
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].shader, "vignette");
    assert_eq!(h.state().timeline.tool(), Tool::Text);
    assert!(!crate::handles::editing(&h.ctx));
    assert!(!egui::DragAndDrop::has_any_payload(&h.ctx));
    assert!(h.state_mut().history.undo());
    assert_eq!(h.state().history.project(), &before);
}

#[test]
fn each_preset_adds_a_selected_styled_editable_clip_at_the_playhead() {
    for (name, _, style) in crate::media_bin::presets() {
        let mut h = editor([960.0, 600.0], 1.0);
        h.state_mut().timeline.seek(Time(3 * S));
        h.state_mut().bin.tab = 2;
        let before = h.state().history.project().clone();
        h.run();
        h.get_by_label(name).click();
        h.run();
        assert_eq!(h.state().bin.tab, 2, "keep the preset browser discoverable");
        let id = *h.state().timeline.selected().iter().next().unwrap();
        let p = h.state().history.project();
        let (_, c) = p.sequence().clip(id).unwrap();
        assert_eq!(c.start, Time(3 * S));
        assert_eq!(p.asset(c.asset).unwrap().text.as_ref(), Some(&style));
        assert!(h.query_all_by_label(tr!("inspector.text")).next().is_some());
        h.get_by_role(egui::accesskit::Role::MultilineTextInput)
            .focus();
        h.run();
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
        h.run();
        h.event(egui::Event::Text(format!("{} edited", style.content)));
        h.run();
        assert!(content(h.state(), id).ends_with(" edited"));
        assert!(h.state_mut().history.undo());
        assert_eq!(content(h.state(), id), style.content);
        assert!(h.state_mut().history.undo());
        assert_eq!(h.state().history.project(), &before);
    }
}

#[test]
fn text_placement_preserves_occupied_locked_hidden_and_solo_video_tracks() {
    for (locked, hidden, solo) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let mut h = editor([960.0, 600.0], 1.0);
        crate::playback::tests::add_solid(
            &mut h.state_mut().history,
            [0.2, 0.3, 0.4, 1.0],
            Time(0),
        );
        let mut track = h.state().history.project().sequence().tracks()[0].settings();
        (track.locked, track.hidden, track.solo) = (locked, hidden, solo);
        h.state_mut()
            .history
            .apply(Command::SetTrack(track))
            .unwrap();
        let before = h.state().history.project().clone();
        let (name, _, text) = crate::media_bin::presets().into_iter().next().unwrap();
        let ctx = h.ctx.clone();
        let clip = h
            .state_mut()
            .create_text(
                &ctx,
                super::super::text_tools::Creation {
                    name: name.into(),
                    text,
                    at: None,
                },
            )
            .unwrap();
        let p = h.state().history.project();
        let (track, _) = p.sequence().clip(clip).unwrap();
        let overlay = p.sequence().track(track).unwrap();
        assert!(!overlay.hidden && !overlay.locked);
        assert_eq!(overlay.solo, solo);
        assert_eq!(overlay.id, p.sequence().tracks()[0].id);
        assert_eq!(&p.sequence().tracks()[1..], before.sequence().tracks());
        assert!(
            jvid_core::eval::video_at(p.sequence(), Time(0))
                .iter()
                .any(|l| l.clip == clip)
        );
        assert!(h.state_mut().history.undo());
        assert_eq!(h.state().history.project(), &before);
    }
}

#[test]
fn text_tool_ignores_active_drag_payloads() {
    let mut h = editor([960.0, 600.0], 1.0);
    h.state_mut().run_action("text_tool", true);
    h.run();
    let canvas = h
        .get_all_by_label("Text Tool")
        .find(|n| n.rect().width() > 32.0)
        .unwrap()
        .rect();
    let before = h.state().history.project().clone();
    // The canvas deliberately treats asset/effect payloads alike; each drag's own target
    // gets to handle it, including on its release frame.
    egui::DragAndDrop::set_payload(&h.ctx, "effect payload".to_owned());
    click(&mut h, canvas.center());
    assert_eq!(h.state().history.project(), &before);
    assert!(!crate::handles::editing(&h.ctx));
}

#[test]
fn rail_marker_ruler_and_track_bounds_stay_separate_at_minimum_and_large_sizes() {
    for size in [[960.0, 600.0], [1920.0, 1080.0]] {
        for scale in [1.0, 1.5] {
            let mut h = editor(size, scale);
            let Created::Track(_) = h
                .state_mut()
                .history
                .apply(Command::AddTrack {
                    kind: TrackKind::Video,
                    name: "Video".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            h.run();
            let leaf = h.state().dock.find_tab(&Tab::Timeline).unwrap().node_path();
            let panel = h.state().dock.leaf(leaf).unwrap().rect;
            let tools: Vec<_> = ["Selection Tool", "Razor Tool", "Text Tool"]
                .map(|name| h.get_by_label(name).rect())
                .into();
            let marker = h.get_by_label(tr!("timeline.marker_lane")).rect();
            let lock = h.get_all_by_label("Lock").next().unwrap().rect();
            assert_eq!(marker.height(), 16.0);
            for r in &tools {
                assert!(
                    panel.contains_rect(*r),
                    "{size:?} {scale}: {r:?} outside {panel:?}"
                );
                assert!(r.width() <= 24.1 && r.height() <= 24.1);
                assert!(r.right() < lock.left() && r.right() < marker.left());
                assert!(!r.intersects(marker) && !r.intersects(lock));
            }
            assert!(tools.windows(2).all(|w| w[0].bottom() < w[1].top()));
            assert!(lock.center().y > marker.bottom() + size::RULER);
            // The rail begins within the body immediately below the dock tab strip.
            assert!((tools[0].top() - marker.top() - space::S4).abs() < 1.0);
            h.get_by_label("Text Tool").click();
            h.run();
            assert_eq!(h.state().timeline.tool(), Tool::Text);
            // A header click remains a header click beside the rail.
            click(&mut h, lock.center());
            assert!(h.state().history.project().sequence().tracks()[0].locked);
            // Every remaining tool is visible or reachable from the rail's overflow.
            if h.query_by_label("Zoom to Fit").is_none() {
                let more = h
                    .get_all_by_label(tr!("preview.more"))
                    .find(|n| panel.contains_rect(n.rect()))
                    .unwrap();
                more.click();
                h.run();
            }
            assert!(h.get_all_by_role(egui::accesskit::Role::Button).any(|n| {
                n.accesskit_node()
                    .label()
                    .is_some_and(|label| label.starts_with("Zoom to Fit"))
            }));
        }
    }
}

#[test]
fn empty_marker_lane_adds_then_moves_a_marker_without_changing_geometry_or_seek() {
    let mut h = editor([960.0, 600.0], 1.0);
    let lane = h.get_by_label(tr!("timeline.marker_lane")).rect();
    let at = Pos2::new(lane.left() + 60.0, lane.center().y);
    h.hover_at(at);
    h.step();
    for pressed in [true, false, true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
    }
    h.run();
    assert_eq!(h.get_by_label(tr!("timeline.marker_lane")).rect(), lane);
    assert_eq!(h.state().history.project().sequence().markers().len(), 1);
    let before = h.state().history.project().sequence().markers()[0].time;
    assert_eq!(before, Time(3 * S));
    assert_eq!(h.state().timeline.playhead(), Time(0));
    h.drag_at(at);
    h.run();
    let to = at + vec2(40.0, 0.0);
    h.hover_at(to);
    h.run();
    h.drop_at(to);
    h.run();
    assert_eq!(h.get_by_label(tr!("timeline.marker_lane")).rect(), lane);
    assert_eq!(
        h.state().history.project().sequence().markers()[0].time,
        Time(5 * S)
    );
    assert_eq!(h.state().timeline.playhead(), Time(0));
    assert!(h.state_mut().history.undo());
    assert_eq!(
        h.state().history.project().sequence().markers()[0].time,
        before
    );
}

#[test]
fn rail_and_text_presets_snapshots() {
    if !gpu() {
        return;
    }
    let mut results = egui_kittest::SnapshotResults::new();
    for (size, name) in [
        ([960.0, 600.0], "timeline_tools_minimum"),
        ([1920.0, 1080.0], "timeline_tools_large"),
    ] {
        let mut h = builder(size, 1.0)
            .wgpu()
            .build_eframe(|cc| App::new(cc, &Default::default()));
        let app = h.state_mut();
        app.on_welcome = false;
        app.bin.tab = 2;
        crate::playback::tests::add_solid(&mut app.history, [0.2, 0.3, 0.4, 1.0], Time(0));
        h.run();
        results.add(h.try_snapshot(name));
    }
    results.unwrap();
}
