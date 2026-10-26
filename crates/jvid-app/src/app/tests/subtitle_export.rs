use super::*;

fn captions_app() -> Harness<'static, App> {
    let mut h = plain_harness();
    let app = h.state_mut();
    app.on_welcome = false;
    app.history
        .apply(Command::ImportSubtitles {
            name: "Captions".into(),
            cues: vec![jvid_core::srt::Cue {
                start: Time(0),
                end: Time(jvid_core::time::FLICKS_PER_SECOND),
                text: "Edited café\n字幕".into(),
            }],
            style: Default::default(),
        })
        .unwrap();
    h.run();
    h
}

#[test]
fn discoverable_subtitle_export_saves_without_gpu_models_recognition_or_video_jobs() {
    let mut h = captions_app();
    assert!(h.state().preview.gpu().is_none());
    assert!(h.state().subtitles.engine.is_none());
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("captions.srt");
    let before = h.state().history.project().clone();
    h.get_by_label("Timeline").click();
    h.run();
    h.get_by_label("Export Subtitles (.srt)… ").click();
    h.run();
    assert!(h.state().subtitle_export.is_open());
    assert!(h.state().modal_owns_input(&h.ctx));
    h.state_mut().subtitle_export.test_destination = Some(Some(destination.clone()));
    h.get_by_label("Save SRT…").click();
    h.run_steps(2);
    assert_eq!(
        std::fs::read_to_string(destination).unwrap(),
        "1\n00:00:00,000 --> 00:00:01,000\nEdited café\n字幕\n\n"
    );
    assert!(!h.state().subtitle_export.is_open());
    assert!(!h.state().export_dialog.is_open());
    assert!(h.state().exports.is_idle());
    assert!(h.state().queue.is_empty());
    assert!(h.state().subtitles.jobs.is_none());
    assert_eq!(h.state().history.project(), &before);
}

#[test]
fn file_and_subtitle_context_routes_open_the_same_dialog_and_native_cancel_is_harmless() {
    let mut h = captions_app();
    let fake = std::sync::Arc::new(FakeTranscriber {
        slow: false,
        asked: Default::default(),
    });
    h.state_mut().subtitles.engine = Some((jvid_transcribe::models::Model::Base, fake.clone()));
    h.get_by_label("File").click();
    h.run();
    h.get_by_label("Export Subtitles (.srt)… ").click();
    h.run();
    h.state_mut().subtitle_export.test_destination = Some(None);
    h.get_by_label("Save SRT…").click();
    h.run_steps(2);
    assert!(h.state().subtitle_export.is_open());
    assert!(h.state().subtitles.jobs.is_none());
    assert!(fake.asked.lock().unwrap().is_empty());
    h.get_by_label("Cancel").click();
    h.run();
    // Right-click the subtitle track's name, just left of its Lock toggle.
    let lock = h.get_by_label("Lock").rect();
    let at = egui::pos2(lock.left() - 16.0, lock.center().y);
    h.hover_at(at);
    h.run();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
    }
    h.get_by_label("Export Subtitles (.srt)…").click();
    h.run();
    assert!(h.state().subtitle_export.is_open());
    assert!(!h.state().timeline.export_subtitles);
    h.get_by_label("Cancel").click();
    h.run();
    assert!(!h.state().subtitle_export.is_open());
}

#[test]
fn in_out_is_an_explicit_scope_choice_and_clips_the_saved_captions() {
    let mut h = captions_app();
    let s = jvid_core::time::FLICKS_PER_SECOND;
    h.state_mut()
        .history
        .apply(Command::SetInOut {
            in_point: Some(Time(s / 2)),
            out_point: Some(Time(s * 3 / 4)),
        })
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("range.srt");
    h.state_mut().run_action("export_subtitles", true);
    h.run();
    h.get_by_label(tr!("export.entire"));
    h.get_by_label(tr!("export.in_out")).click();
    h.run();
    h.state_mut().subtitle_export.test_destination = Some(Some(path.clone()));
    h.get_by_label("Save SRT…").click();
    h.run_steps(2);
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "1\n00:00:00,000 --> 00:00:00,250\nEdited café\n字幕\n\n"
    );
    assert!(h.state().exports.is_idle() && h.state().subtitles.jobs.is_none());
}

#[test]
fn empty_subtitle_export_explains_why_saving_is_disabled() {
    let mut h = plain_harness();
    h.state_mut().on_welcome = false;
    h.state_mut().run_action("export_subtitles", true);
    h.run();
    h.get_by_label("No subtitles in this range.");
    assert!(h.get_by_label("Save SRT…").accesskit_node().is_disabled());
    assert!(
        h.get_by_label(tr!("export.in_out"))
            .accesskit_node()
            .is_disabled()
    );
    h.get_by_label("Cancel").click();
    h.run();
    assert!(!h.state().subtitle_export.is_open());
}
