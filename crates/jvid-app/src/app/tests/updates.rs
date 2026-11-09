use super::*;

fn app() -> App {
    let mut app = App::restore(None);
    // Keep the font scan out of these install guards; no GPU or real installer is needed.
    app.exports = jvid_jobs::Pool::new(1);
    app.updates = crate::updates::Updates::for_test(
        j_update::state::State::Ready(j_update::network::Download {
            update: j_update::network::Available {
                version: "1.2.0".into(),
                notes: "Update fixture".into(),
                asset: j_update::metadata::Asset {
                    target: jvid_update_config::TARGET.into(),
                    distribution: "windows-installer".into(),
                    name: "jvid-1.2.0-windows-x86_64-setup.exe".into(),
                    size: 100,
                    sha256: "00".repeat(32),
                },
                asset_id: 1,
                tag: "v1.2.0".into(),
                manifest: vec![],
                signature: vec![],
            },
            dir: tempfile::tempdir().unwrap(),
        }),
        "windows-installer",
    );
    app.updates.open = true;
    app
}

fn dirty(app: &mut App) {
    app.history
        .apply(Command::AddTrack {
            kind: TrackKind::Audio,
            name: "Voice".into(),
        })
        .unwrap();
}

fn ready_path(app: &App) -> PathBuf {
    let j_update::state::State::Ready(download) = app.updates.state() else {
        panic!("expected the verified download to remain ready");
    };
    download.dir.path().to_owned()
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
    dirty(&mut app);
    // Saving to a directory must fail, without opening a native file dialog.
    app.path = Some(dir.path().to_owned());
    let before = app.history.project().clone();
    assert!(app.update_work().unsaved);
    app.install_update(&egui::Context::default());
    assert!(app.history.is_dirty() && !app.closing);
    assert_eq!(*app.history.project(), before);
    assert!(app.update_install.lock().unwrap().is_none());
    assert!(matches!(app.prompts.last(), Some(Prompt::Error(_))));
    assert!(!app.updates.open && ready_path(&app).exists());
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

#[test]
fn cancelling_save_keeps_project_and_verified_download_ready_without_shutdown() {
    let mut app = app();
    dirty(&mut app);
    let before = app.history.project().clone();
    let stage = ready_path(&app);
    let mut asked_to_save = false;
    app.install_update_with(&egui::Context::default(), |_| {
        // The native save dialog's Cancel result, without opening an OS dialog.
        asked_to_save = true;
        false
    });
    assert!(asked_to_save && app.history.is_dirty() && app.path.is_none());
    assert_eq!(*app.history.project(), before);
    assert!(app.updates.open && ready_path(&app) == stage && stage.exists());
    assert!(!app.closing && app.update_install.lock().unwrap().is_none());
    assert!(app.prompts.is_empty());
}

#[test]
fn saving_for_update_writes_project_before_preparation_and_keeps_editor_on_prepare_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    dirty(&mut app);
    let before = app.history.project().clone();
    let path = dir.path().join("saved.jvid");
    app.path = Some(path.clone());
    app.install_update(&egui::Context::default());
    assert!(!app.history.is_dirty());
    assert_eq!(project_file::load(&path).unwrap(), before);
    // This test executable has no installed Windows layout or real signed payload.
    // Preparation fails closed before shutdown; nothing can launch an installer.
    assert!(matches!(
        app.updates.state(),
        j_update::state::State::Failed(_)
    ));
    assert!(!app.closing && app.update_install.lock().unwrap().is_none());
}

#[test]
fn active_prompt_blocks_save_and_work_started_during_save_blocks_preparation() {
    let mut app = app();
    dirty(&mut app);
    app.error("Existing prompt".into());
    app.install_update_with(&egui::Context::default(), |_| panic!("save while busy"));
    assert!(app.history.is_dirty() && ready_path(&app).exists());
    assert!(!app.closing && app.update_install.lock().unwrap().is_none());
    app.prompts.clear();
    app.install_update_with(&egui::Context::default(), |app| {
        app.history.mark_saved();
        app.error("Work appeared during save".into());
        true
    });
    assert!(ready_path(&app).exists());
    assert!(!app.closing && app.update_install.lock().unwrap().is_none());
}

#[test]
fn an_install_request_without_a_ready_payload_never_opens_save() {
    let mut app = app();
    dirty(&mut app);
    *app.updates.state_mut() = j_update::state::State::Current;
    app.install_update_with(&egui::Context::default(), |_| panic!("save without update"));
    assert!(app.history.is_dirty() && !app.closing);
    assert!(app.update_install.lock().unwrap().is_none());
}
