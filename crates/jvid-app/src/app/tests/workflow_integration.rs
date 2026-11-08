//! Boundaries exercised only when all workflow packages are composed.

use super::*;
use crate::i18n::tr;
use jvid_core::annotations::AnnotationKind;
use jvid_core::captions::{Edit as CaptionEdit, Scope};
use jvid_core::media_library::LibraryEdit;
use jvid_core::model::Text;
use jvid_core::time::FLICKS_PER_SECOND;

fn annotated(app: &mut App) -> [jvid_core::model::ClipId; 2] {
    app.on_welcome = false;
    app.run_action("focus_program", false);
    app.run_action("add_arrow", false);
    let arrow = *app.timeline.selected().iter().next().unwrap();
    app.run_action("add_callout", false);
    let callout = *app.timeline.selected().iter().next().unwrap();
    [arrow, callout]
}

#[test]
fn annotation_inspector_paste_library_filter_and_complete_collection_compose() {
    let mut h = plain_harness();
    let [arrow, callout] = annotated(h.state_mut());
    let app = h.state_mut();
    let (_, clip) = app.history.project().sequence().clip(arrow).unwrap();
    let mut transform = clip.transform.clone();
    transform.position = [80.0, -45.0];
    app.history
        .apply(Command::SetClipProps(vec![(
            arrow,
            transform,
            clip.keyframes.clone(),
        )]))
        .unwrap();
    app.timeline.select_program_clip(arrow);
    app.run_action("copy_settings", true);
    app.timeline.select_program_clip(callout);
    app.focus_panel(Tab::Inspector);
    let before = app.history.project().clone();
    h.run();
    h.get_by_label(tr!("action.paste_settings")).click();
    h.run();
    let app = h.state_mut();
    let (_, clip) = app.history.project().sequence().clip(callout).unwrap();
    assert_eq!(clip.transform.position, [80.0, -45.0]);
    assert_eq!(
        app.history
            .project()
            .asset(clip.asset)
            .unwrap()
            .annotation
            .as_ref()
            .unwrap()
            .kind,
        AnnotationKind::Callout
    );
    let pasted = app.history.project().clone();
    assert!(app.history.undo());
    assert_eq!(app.history.project(), &before);
    assert!(app.history.redo());
    assert_eq!(app.history.project(), &pasted);
    let ids = app
        .history
        .project()
        .assets()
        .iter()
        .map(|a| a.id)
        .collect();
    app.history
        .apply(Command::EditMediaLibrary(LibraryEdit::SetFavorites {
            ids,
            favorite: true,
        }))
        .unwrap();
    let organized = app.history.project().clone();
    app.run_action("focus_media", false);
    app.bin.list = true;
    h.run();
    h.get_by_label(tr!("bin.filters")).click();
    h.run();
    h.get_by_role_and_label(egui::accesskit::Role::RadioButton, tr!("bin.video"))
        .click();
    h.key_press(egui::Key::Escape);
    h.run();
    for asset in organized.assets() {
        assert!(h.query_by_label(&asset.display_name()).is_some());
    }
    assert_eq!(h.state().history.project(), &organized);
    assert!(
        jvid_core::file::dependencies(&organized)
            .unwrap()
            .is_empty()
    );
    let dir = tempfile::tempdir().unwrap();
    let request =
        crate::collect_project::Request::new(organized.clone(), None, dir.path().join("collected"));
    let collected = crate::collect_project::collect(&request, || false, |_| {}).unwrap();
    assert!(collected.manifest.files.is_empty());
    assert!(collected.manifest.unresolved.is_empty());
    let reopened = crate::project_file::load(&collected.project).unwrap();
    assert_eq!(reopened, organized);
    let root: serde_json::Value =
        serde_json::from_slice(&std::fs::read(collected.project).unwrap()).unwrap();
    assert_eq!(root["schema_version"], 2);
}

fn captions_and_highlight(app: &mut App) {
    app.history
        .apply(Command::ImportSubtitles {
            name: "Untertitel für die vollständige Integrationsprüfung".into(),
            cues: vec![jvid_core::srt::Cue {
                start: Time(0),
                end: Time(2 * FLICKS_PER_SECOND),
                text: "café 字幕 日本語 漢字 한글".into(),
            }],
            style: Text::default(),
        })
        .unwrap();
    app.history
        .apply(Command::AddMarker {
            time: Time(0),
            duration: Time(FLICKS_PER_SECOND),
            name: "Ausgewählter Höhepunkt mit unveränderlichen Untertiteln".into(),
        })
        .unwrap();
}

#[test]
fn both_optional_panels_restore_without_changing_caption_export_snapshot() {
    let mut app = App::restore(None);
    annotated(&mut app);
    captions_and_highlight(&mut app);
    for action in ["show_captions", "review_highlights"] {
        assert!(app.action_applicability(action, false).is_enabled());
        app.run_action(action, false);
    }
    let project = app.history.project().clone();
    let marker = project.sequence().markers()[0].id;
    let snapshot = crate::highlight_review::HighlightSelectionSnapshot::capture(
        app.project_session,
        &app.history,
        &HashSet::from([marker]),
        (Time(0), Time(0)),
    )
    .unwrap();
    let source = crate::export::versions::VersionSource::highlights(
        app.project_session,
        &app.history,
        &snapshot,
    )
    .unwrap();
    app.run_action("maximize_panel", false);
    assert!(app.workspaces.is_maximized());
    app.close_panel(Tab::Captions);
    assert!(!app.workspaces.is_maximized());
    let saved = crate::workspaces::Snapshot::capture(&app.dock, Panels::default());
    let restored = saved.restore(Panels::default());
    assert!(restored.find_tab(&Tab::Captions).is_none());
    assert!(restored.find_tab(&Tab::Highlights).is_some());
    let unavailable = saved.restore(Panels(&[]));
    assert!(unavailable.find_tab(&Tab::Captions).is_none());
    assert!(unavailable.find_tab(&Tab::Highlights).is_none());
    app.dock = restored;
    app.run_action("show_captions", false);
    assert!(app.dock.find_tab(&Tab::Captions).is_some());
    assert!(source.validate(app.project_session, &app.history).is_ok());
    assert_eq!(app.history.project(), &project);
    app.history
        .apply(Command::Caption(CaptionEdit::Replace {
            scope: Scope::Sequence,
            find: "café".into(),
            replacement: "你好 café".into(),
        }))
        .unwrap();
    assert!(source.validate(app.project_session, &app.history).is_err());
    assert!(
        snapshot
            .validate(app.project_session, &app.history)
            .is_err()
    );
    assert_eq!(source.project(), &project);
    assert_eq!(source.ranges()[0].span, (Time(0), Time(FLICKS_PER_SECOND)));
    assert!(app.history.undo());
    assert_eq!(app.history.project(), &project);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("字幕と図形.jvid");
    crate::project_file::save(app.history.project(), &path).unwrap();
    assert_eq!(crate::project_file::load(&path).unwrap(), project);
}

#[test]
fn generated_annotations_and_edited_unicode_captions_encode_all_policies() {
    let Some((device, queue)) = crate::playback::tests::device() else {
        return;
    };
    let mut app = App::restore(None);
    let mut settings = app.history.project().settings().clone();
    settings.width = 320;
    settings.height = 180;
    app.history.apply(Command::SetSettings(settings)).unwrap();
    annotated(&mut app);
    app.history
        .apply(Command::ImportSubtitles {
            name: "Captions".into(),
            cues: vec![jvid_core::srt::Cue {
                start: Time(0),
                end: Time(FLICKS_PER_SECOND),
                text: "Before".into(),
            }],
            style: Text {
                size: 24.0,
                ..Default::default()
            },
        })
        .unwrap();
    app.history
        .apply(Command::Caption(CaptionEdit::Replace {
            scope: Scope::Sequence,
            find: "Before".into(),
            replacement: "café 字幕".into(),
        }))
        .unwrap();
    let original = app.history.project().clone();
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("edited.jvid");
    crate::project_file::save(&original, &source).unwrap();
    let reopened = crate::project_file::load(&source).unwrap();
    let preset = jvid_core::preset::builtin()
        .into_iter()
        .find(|p| p.video == Some(jvid_core::preset::VideoCodec::ProRes))
        .unwrap();
    let pool = jvid_jobs::Pool::new(1);
    let mut frames = Vec::new();
    for policy in ["burn", "sidecar", "omit"] {
        let path = dir.path().join(format!("{policy}.mov"));
        let project = if policy == "omit" {
            reopened.without_subtitles()
        } else {
            reopened.clone()
        };
        let (_, result) = crate::export::submit(
            &pool,
            project,
            crate::export::Request {
                path: path.clone(),
                range: (Time(0), Time(FLICKS_PER_SECOND / 2)),
                preset: preset.clone(),
                loudness: None,
                lossless: None,
                sidecar: policy == "sidecar",
            },
            Some((device.clone(), queue.clone())),
        );
        let probe = result
            .recv_timeout(std::time::Duration::from_secs(90))
            .unwrap()
            .unwrap();
        assert_eq!((probe.width, probe.height), (Some(320), Some(180)));
        assert!(
            (probe.duration - Time(FLICKS_PER_SECOND / 2)).0.abs()
                <= original.settings().frame_rate.time_floor(1).0
        );
        let mut decoder = jvid_media::decode::VideoDecoder::open(&path).unwrap();
        let frame = decoder.frame_at(Time(0)).unwrap();
        assert!(
            frame.planes[0].iter().any(|y| *y > 100),
            "annotation pixels encoded"
        );
        frames.push(frame.planes[0].clone());
        if policy == "sidecar" {
            let text = std::fs::read_to_string(path.with_extension("srt")).unwrap();
            assert!(text.contains("00:00:00,000 --> 00:00:00,500"));
            assert!(text.contains("café 字幕"));
        } else {
            assert!(!path.with_extension("srt").exists());
        }
        if let Some(output) = std::env::var_os("JVID_INTEGRATION_CAPTURES") {
            let output = PathBuf::from(output);
            std::fs::create_dir_all(&output).unwrap();
            std::fs::copy(&path, output.join(path.file_name().unwrap())).unwrap();
            if policy == "sidecar" {
                std::fs::copy(path.with_extension("srt"), output.join("sidecar.srt")).unwrap();
            }
        }
    }
    assert_eq!(
        frames[1], frames[2],
        "sidecar and omission encode identical caption-free pixels"
    );
    assert!(
        frames[0]
            .iter()
            .zip(&frames[2])
            .filter(|(a, b)| a != b)
            .count()
            > 100,
        "edited caption contributes visible encoded pixels"
    );
    assert_eq!(app.history.project(), &original);
}

#[test]
#[ignore = "changes global locale and writes review captures; run in isolation"]
fn german_workflow_captures_at_required_sizes_and_scales() {
    if !gpu() {
        return;
    }
    struct Restore(&'static str);
    impl Drop for Restore {
        fn drop(&mut self) {
            j_ui::i18n::set_language(self.0);
        }
    }
    let _restore = Restore(j_ui::i18n::language());
    j_ui::i18n::set_language("de");
    let dir =
        PathBuf::from(std::env::var_os("JVID_INTEGRATION_CAPTURES").expect("capture directory"));
    std::fs::create_dir_all(&dir).unwrap();
    for (size, scale) in configs() {
        let mut h = harness(size, scale);
        annotated(h.state_mut());
        captions_and_highlight(h.state_mut());
        h.state_mut().run_action("show_captions", false);
        h.state_mut().run_action("review_highlights", false);
        let project = h.state().history.project().clone();
        h.run_steps(4);
        h.render()
            .unwrap()
            .save(dir.join(format!("editor-{}x{}-{scale}x-de.png", size[0], size[1])))
            .unwrap();
        h.state_mut().run_action("show_captions", false);
        h.run_steps(2);
        h.state_mut().run_action("command_palette", false);
        h.run_steps(2);
        assert!(h.state().command_palette.is_open());
        h.render()
            .unwrap()
            .save(dir.join(format!("palette-{}x{}-{scale}x-de.png", size[0], size[1])))
            .unwrap();
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
        assert!(!h.state().command_palette.is_open());
        assert_eq!(
            h.state_mut()
                .dock
                .find_active_focused()
                .map(|(_, tab)| *tab),
            Some(Tab::Captions)
        );
        h.state_mut().run_action("maximize_panel", false);
        h.run_steps(2);
        assert!(h.state().workspaces.is_maximized());
        h.state_mut().run_action("maximize_panel", false);
        h.state_mut().close_panel(Tab::Captions);
        h.state_mut().run_action("show_captions", false);
        h.run_steps(2);
        assert!(h.state().dock.find_tab(&Tab::Captions).is_some());
        assert_eq!(h.state().history.project(), &project);
        h.state_mut().run_action("add_rectangle", true);
        assert_ne!(h.state().history.project(), &project);
        h.state_mut().run_action("undo", false);
        assert_eq!(h.state().history.project(), &project);
    }
}
