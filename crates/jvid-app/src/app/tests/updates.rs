use super::*;

fn app() -> App {
    let mut app = App::restore(None);
    // Keep the font scan out of these install guards; no GPU or real installer is needed.
    app.exports = jvid_jobs::Pool::new(1);
    app
}

#[test]
fn updates_never_finalize_an_active_recording_or_close_the_editor() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.path = Some(dir.path().join("p.jvid"));
    app.input = Some(Box::new(crate::record::tests::FakeMic::default()));
    app.start_take();
    assert!(app.take.is_some());
    let recorded = app.take.as_ref().unwrap().path.clone();
    assert!(app.update_work().recording);
    app.install_update(&egui::Context::default());
    assert!(app.take.is_some() && !app.closing);
    assert!(recorded.exists());
    app.finalize_take();
}

#[test]
fn failed_save_retains_unsaved_work_and_prevents_update_handoff() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.history
        .apply(Command::AddTrack {
            kind: TrackKind::Audio,
            name: "Voice".into(),
        })
        .unwrap();
    // Saving to a directory must fail, without opening a native file dialog.
    app.path = Some(dir.path().to_owned());
    let before = app.history.project().clone();
    assert!(app.update_work().unsaved);
    app.install_update(&egui::Context::default());
    assert!(app.history.is_dirty() && !app.closing);
    assert_eq!(*app.history.project(), before);
    assert!(app.update_install.lock().unwrap().is_none());
    assert!(matches!(app.prompts.last(), Some(Prompt::Error(_))));
}

#[test]
fn active_export_job_blocks_update_even_after_cancellation() {
    let mut app = app();
    let (go, wait) = std::sync::mpsc::channel::<()>();
    let (started, start) = std::sync::mpsc::channel();
    let job = app.exports.submit(
        "update-gate-export",
        jvid_jobs::Priority::Interactive,
        move |_| {
            started.send(()).unwrap();
            let _ = wait.recv();
        },
    );
    start.recv().unwrap();
    job.cancel();
    assert!(app.update_work().exporting);
    app.install_update(&egui::Context::default());
    assert!(!app.closing && app.update_install.lock().unwrap().is_none());
    go.send(()).unwrap();
}
