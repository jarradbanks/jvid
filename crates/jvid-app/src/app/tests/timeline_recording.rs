use super::*;

fn recording_with_picture(name: &str) -> (App, PathBuf, PathBuf, Time) {
    let (mut app, dir, wav, start) = recording_app(name, true);
    app.on_welcome = false;
    app.history
        .apply(Command::AddSolid {
            name: "Background".into(),
            color: [0.1, 0.1, 0.1, 1.0],
        })
        .unwrap();
    let asset = app.history.project().assets().last().unwrap().id;
    let placement = app
        .history
        .project()
        .placement(asset, Time(0), false, None, false)
        .unwrap();
    app.history.apply(Command::Place(placement)).unwrap();
    (app, dir, wav, start)
}

#[test]
fn short_docked_timeline_wheel_reaches_the_last_track() {
    for header in [false, true] {
        let mut app = App::restore(None);
        app.on_welcome = false;
        for n in 1..=12 {
            app.history
                .apply(Command::AddTrack {
                    kind: TrackKind::Audio,
                    name: format!("Track {n}"),
                })
                .unwrap();
        }
        let mut h = builder([1280.0, 720.0], 1.0)
            .with_step_dt(0.25)
            .with_max_steps(32)
            .build_eframe(move |cc| {
                theme::install(&cc.egui_ctx);
                app
            });
        h.run();
        let path = h.state().dock.find_tab(&Tab::Timeline).unwrap();
        let viewport = h.state().dock.leaf(path.node_path()).unwrap().viewport;
        let at = egui::pos2(
            viewport.left() + if header { 24.0 } else { 250.0 },
            viewport.bottom() - 40.0,
        );
        h.hover_at(at);
        h.run();
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            phase: egui::TouchPhase::Move,
            delta: vec2(0.0, -2000.0),
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
        let bottom = h
            .output()
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "Track 12" => {
                    Some(Rect::from_min_size(t.pos, t.galley.size()))
                }
                _ => None,
            })
            .expect("last track header painted");
        assert!(
            bottom.top() > viewport.top() && bottom.bottom() < viewport.bottom(),
            "{bottom:?} outside {viewport:?}"
        );
    }
}

#[test]
fn clicked_playback_stop_finalizes_before_seek_and_preserves_pcm_and_undo() {
    let (app, dir, wav, start) = recording_with_picture("clicked-stop");
    let before = app.history.project().clone();
    let mut h = builder([1280.0, 720.0], 1.0).build_eframe(move |cc| {
        theme::install(&cc.egui_ctx);
        app
    });
    h.run_steps(2);
    h.get_by_label("Stop (Space)").click();
    h.run_steps(2);
    assert!(
        h.state().take.is_none(),
        "playback Stop must finish capture"
    );
    assert!(!h.state().preview.playing());
    assert_finished_take(h.state().history.project(), &wav, start);
    let bytes = std::fs::read(&wav).unwrap();
    assert!(
        bytes[44..]
            .chunks_exact(2)
            .all(|p| i16::from_le_bytes([p[0], p[1]]) == 8192)
    );
    // The Program seeker, after Stop, must leave the completed edit intact.
    let seeker = h.get_by_label(tr!("preview.position")).rect();
    let at = egui::pos2(seeker.left() + 1.0, seeker.center().y);
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(2);
    assert!(h.state().timeline.playhead() < start);
    assert_finished_take(h.state().history.project(), &wav, start);
    h.state_mut().run_action("play_stop", true);
    h.run_steps(2);
    h.state_mut().run_action("shuttle_stop", true);
    assert!(h.state_mut().history.undo());
    assert_eq!(h.state().history.project(), &before);
    assert_eq!(std::fs::read(&wav).unwrap(), bytes);
    assert!(h.state_mut().history.redo());
    assert_finished_take(h.state().history.project(), &wav, start);
    drop(h);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn seeking_during_capture_finalizes_the_take_at_its_original_start() {
    for program in [false, true] {
        let (app, dir, wav, start) = recording_with_picture(if program {
            "program-seek"
        } else {
            "ruler-seek"
        });
        let before = app.history.project().clone();
        let mut h = builder([1280.0, 720.0], 1.0).build_eframe(move |cc| {
            theme::install(&cc.egui_ctx);
            app
        });
        h.run_steps(2);
        let at = if program {
            let seeker = h.get_by_label(tr!("preview.position")).rect();
            egui::pos2(seeker.left() + 1.0, seeker.center().y)
        } else {
            let path = h.state().dock.find_tab(&Tab::Timeline).unwrap();
            let viewport = h.state().dock.leaf(path.node_path()).unwrap().viewport;
            egui::pos2(
                viewport.left() + 12.0 + suite_ui::tokens::size::TRACK_HEADER + 10.0,
                viewport.top() + suite_ui::tokens::size::HEADER + 8.0 + 16.0 + 12.0,
            )
        };
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        // The seek strip distinguishes a press from a drag; move far enough to begin it.
        let to = at + vec2(12.0, 0.0);
        h.hover_at(to);
        h.run_steps(1);
        assert!(
            h.state().take.is_none(),
            "seeking must finish capture: program={program}"
        );
        h.drop_at(to);
        h.run_steps(2);
        assert!(h.state().timeline.playhead() < start);
        assert_finished_take(h.state().history.project(), &wav, start);
        assert!(h.state_mut().history.undo());
        assert_eq!(h.state().history.project(), &before);
        drop(h);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
