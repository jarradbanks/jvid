use super::*;
use jvid_core::time::FLICKS_PER_SECOND;

/// Starts through the editor, without an imported asset or a pre-seeded take.
#[test]
fn empty_project_records_from_the_program_button_and_both_stop_buttons_finish_pcm() {
    for playback_stop in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mic = crate::record::tests::FakeMic::default();
        let output = crate::transport::tests::Fake::default();
        let mut h = builder([1280.0, 720.0], 1.0).build_eframe(|cc| {
            theme::install(&cc.egui_ctx);
            App::restore(None)
        });
        h.run();
        h.get_by_label(tr!("menu.new_project")).click();
        h.run();
        assert!(!h.state().on_welcome);
        assert!(h.state().history.project().assets().is_empty());
        assert_eq!(h.state().history.project().sequence().duration(), Time(0));
        let app = h.state_mut();
        app.path = Some(dir.path().join("empty.jvid"));
        app.input = Some(Box::new(mic.clone()));
        app.preview
            .set_output(jvid_audio::Output::new(Box::new(output.clone()), None));
        h.get_by_label("Record Voiceover (Shift+R)").click();
        h.run_steps(2);
        assert!(h.state().take.is_some());
        let wav = h.state().take.as_ref().unwrap().path.clone();
        let before = h.state().history.project().clone();
        let capture = mic.0.lock().unwrap().clone().unwrap();
        for _ in 0..6 {
            capture.push(&[0.25; 1_600]);
            output.pull(1_600);
            h.run_steps(1);
            assert!(h.state().take.is_some());
            assert!(h.state().preview.playing());
            assert!(h.state().timeline.level.unwrap() > 0.24);
            assert!(!h.state().timeline.live.as_ref().unwrap().peaks.is_empty());
        }
        assert!(h.state().timeline.playhead() > Time(0));
        let (elapsed, level) = h.state().preview.recording.unwrap();
        assert!(!elapsed.is_zero() && level > 0.24);
        assert!(h.state().history.project().assets().is_empty());
        h.get_by_label(if playback_stop {
            "Stop (Space)"
        } else {
            "Stop Recording (Shift+R)"
        })
        .click();
        h.run_steps(2);
        assert!(h.state().take.is_none());
        assert!(!h.state().preview.playing());
        assert!(h.state().preview.recording.is_none());
        assert!(h.state().timeline.live.is_none() && h.state().timeline.level.is_none());
        let project = h.state().history.project();
        let [asset] = project.assets() else {
            panic!("only the recorded WAV, no dummy media")
        };
        assert_eq!(asset.path, wav);
        assert_eq!(
            asset.probe.as_ref().unwrap().duration,
            Time(FLICKS_PER_SECOND / 5)
        );
        let clips: Vec<_> = project
            .sequence()
            .tracks()
            .iter()
            .flat_map(|t| t.clips())
            .collect();
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].start, Time(0));
        assert_eq!(clips[0].end(), Time(FLICKS_PER_SECOND / 5));
        let bytes = std::fs::read(&wav).unwrap();
        assert_eq!(bytes.len(), 44 + 9_600 * 2);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            48_000
        );
        assert_eq!(
            u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
            9_600 * 2
        );
        jvid_media::ffmpeg::init().unwrap();
        assert_eq!(
            jvid_media::probe::probe(&wav).unwrap().duration,
            Time(FLICKS_PER_SECOND / 5)
        );
        assert!(
            bytes[44..]
                .as_chunks::<2>()
                .0
                .iter()
                .all(|p| i16::from_le_bytes(*p) == 8192)
        );
        assert!(h.state_mut().history.undo());
        assert_eq!(h.state().history.project(), &before);
        assert!(h.state_mut().history.redo());
        assert_eq!(h.state().history.project().assets().len(), 1);
        // Seek through the real Program strip, then replay the completed audio-only project.
        let seeker = h.get_by_label(tr!("preview.position")).rect();
        let at = egui::pos2(seeker.left() + 1.0, seeker.center().y);
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(2);
        assert_eq!(h.state().timeline.playhead(), Time(0));
        h.get_by_label("Play (Space)").click();
        h.run_steps(2);
        output.pull(1_600);
        h.run_steps(1);
        assert!(h.state().preview.playing());
        assert!(h.state().timeline.playhead() > Time(0));
        h.get_by_label("Stop (Space)").click();
        h.run_steps(2);
        assert!(!h.state().preview.playing());
        assert_eq!(std::fs::read(&wav).unwrap(), bytes);
    }
}

#[test]
fn empty_welcome_shortcut_records_on_an_armed_track_while_preview_is_hidden_and_after_content() {
    let dir = tempfile::tempdir().unwrap();
    let mic = crate::record::tests::FakeMic::default();
    let output = crate::transport::tests::Fake::default();
    let mut app = App::restore(None);
    app.path = Some(dir.path().join("empty.jvid"));
    app.input = Some(Box::new(mic.clone()));
    app.preview
        .set_output(jvid_audio::Output::new(Box::new(output.clone()), None));
    app.history
        .apply(Command::AddTrack {
            kind: TrackKind::Audio,
            name: String::new(),
        })
        .unwrap();
    let track = app.history.project().sequence().tracks().last().unwrap().id;
    app.timeline.armed = Some(track);
    app.timeline.action("selection_tool", app.history.project());
    let start = Time(2 * FLICKS_PER_SECOND);
    app.timeline.seek(start);
    let before = app.history.project().clone();
    let mut h = builder([1280.0, 720.0], 1.0).build_eframe(move |cc| {
        theme::install(&cc.egui_ctx);
        app
    });
    h.run();
    assert!(h.state().on_welcome);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::R);
    h.run_steps(2);
    assert!(!h.state().on_welcome, "Record must reveal the editor");
    let wav = h.state().take.as_ref().unwrap().path.clone();
    let path = h.state().dock.find_tab(&Tab::Preview).unwrap();
    h.state_mut().dock.remove_tab(path);
    let capture = mic.0.lock().unwrap().clone().unwrap();
    // More than the empty timeline's ten-second scrollable tail.
    for _ in 0..250 {
        capture.push(&[0.25; 2_400]);
        output.pull(2_400);
        h.run_steps(1);
        assert!(h.state().take.is_some());
        assert!(h.state().preview.program_playing());
    }
    assert!(h.state().timeline.playhead() > Time(14 * FLICKS_PER_SECOND));
    assert!(h.state().history.project().assets().is_empty());
    assert!(
        h.state()
            .history
            .project()
            .sequence()
            .tracks()
            .iter()
            .all(|t| t.clips().is_empty())
    );
    h.state_mut().show_panel(Tab::Preview);
    h.run_steps(2);
    h.get_by_label("Stop (Space)").click();
    h.run_steps(2);
    assert!(h.state().take.is_none());
    let project = h.state().history.project();
    assert_eq!(project.assets().len(), 1);
    let [clip] = project.sequence().track(track).unwrap().clips() else {
        panic!("one take")
    };
    assert_eq!(clip.start, start);
    assert_eq!(clip.end() - start, Time(FLICKS_PER_SECOND * 25 / 2));
    let next_start = clip.end() + Time(2 * FLICKS_PER_SECOND);
    let bytes = std::fs::read(&wav).unwrap();
    assert_eq!(bytes.len(), 44 + 600_000 * 2);
    assert!(h.state_mut().history.undo());
    assert_eq!(h.state().history.project(), &before);
    assert!(h.state_mut().history.redo());
    h.run_steps(2);

    // A second take starts after real recorded content, even with looping enabled.
    let before_second = h.state().history.project().clone();
    h.state_mut().timeline.seek(next_start);
    h.state_mut().preview.program_action("loop_playback");
    h.run_steps(2);
    h.get_by_label("Record Voiceover (Shift+R)").click();
    h.run_steps(2);
    assert_eq!(h.state().take.as_ref().unwrap().start, next_start);
    let next_wav = h.state().take.as_ref().unwrap().path.clone();
    let capture = mic.0.lock().unwrap().clone().unwrap();
    for _ in 0..6 {
        capture.push(&[-0.25; 1_600]);
        output.pull(1_600);
        h.run_steps(1);
        assert!(h.state().take.is_some());
        assert!(h.state().timeline.playhead() >= next_start);
    }
    assert!(h.state().timeline.playhead() > next_start);
    h.get_by_label("Stop Recording (Shift+R)").click();
    h.run_steps(2);
    assert!(h.state().take.is_none());
    assert!(!h.state().preview.playing());
    let project = h.state().history.project();
    assert_eq!(project.assets().len(), 2);
    let clips = project.sequence().track(track).unwrap().clips();
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[1].start, next_start);
    assert_eq!(clips[1].end() - next_start, Time(FLICKS_PER_SECOND / 5));
    let next_bytes = std::fs::read(&next_wav).unwrap();
    assert_eq!(next_bytes.len(), 44 + 9_600 * 2);
    assert!(
        next_bytes[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .all(|p| i16::from_le_bytes(*p) == -8192)
    );
    assert!(h.state_mut().history.undo());
    assert_eq!(h.state().history.project(), &before_second);
    assert!(h.state_mut().history.redo());
    assert_eq!(std::fs::read(&wav).unwrap(), bytes);
}

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
            .as_chunks::<2>()
            .0
            .iter()
            .all(|p| i16::from_le_bytes(*p) == 8192)
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
