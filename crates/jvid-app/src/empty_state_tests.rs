//! Empty panels keep their existing commands and distinguish no selection from work in flight.
use crate::media_bin::MediaBin;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use j_ui::test_support::install_theme;
use jvid_core::history::{Command, History};
use jvid_core::model::{ProbeInfo, Project};
use jvid_core::time::Time;
use std::collections::HashSet;

fn bin_harness(importing: bool) -> Harness<'static, (MediaBin, bool)> {
    let mut fresh = true;
    Harness::builder().with_size([240.0, 360.0]).build_ui_state(
        move |ui, (bin, imported): &mut (MediaBin, bool)| {
            if install_theme(ui, &mut fresh) {
                return;
            }
            *imported |= bin
                .show_docked(ui, &Project::default(), true, importing, &HashSet::new())
                .import;
        },
        (MediaBin::default(), false),
    )
}

#[test]
fn media_empty_action_imports_and_importing_keeps_its_own_state() {
    let mut h = bin_harness(false);
    h.run();
    assert!(h.query_by_label("No Media Yet").is_some());
    h.get_by_label("Import Media…").click();
    h.run();
    assert!(
        h.state().1,
        "uses the same import response as the pinned footer"
    );
    let mut h = bin_harness(true);
    h.run();
    assert!(h.query_by_label("No Media Yet").is_none());
    assert!(
        h.query_by_label(crate::i18n::tr!("bin.importing"))
            .is_some()
    );
}

#[test]
fn media_and_effect_no_results_actions_clear_the_existing_search() {
    for tab in [0, 1] {
        let mut h = bin_harness(false);
        h.state_mut().0.tab = tab;
        h.state_mut().0.query = "no-such-media".into();
        h.state_mut().0.fx_query = "no-such-effect".into();
        h.run();
        let title = if tab == 0 {
            "No Matching Media"
        } else {
            "No Matching Effects"
        };
        assert!(h.query_by_label(title).is_some());
        h.get_by_label("Clear Search").click();
        h.run();
        let bin = &h.state().0;
        assert!(if tab == 0 {
            bin.query.is_empty()
        } else {
            bin.fx_query.is_empty()
        });
        assert!(h.query_by_label(title).is_none());
    }
}

#[test]
fn empty_queue_action_opens_the_existing_export_flow() {
    for can_export in [false, true] {
        let mut fresh = true;
        let mut queue = crate::export::queue::Queue::default();
        let mut h = Harness::builder().with_size([240.0, 360.0]).build_ui_state(
            move |ui, asked: &mut bool| {
                if install_theme(ui, &mut fresh) {
                    return;
                }
                *asked |= queue.panel_docked(ui, true, can_export)
                    == Some(crate::export::queue::Action::New);
            },
            false,
        );
        h.run();
        if can_export {
            h.get_by_label("New Export…").click();
            h.run();
            assert!(*h.state());
        } else {
            assert!(h.query_by_label("New Export…").is_none());
        }
    }
}

#[test]
fn transcript_only_offers_transcription_with_an_audio_target() {
    for audio in [false, true] {
        let mut history = History::new(Project::default());
        if audio {
            history
                .apply(Command::Import(vec![(
                    "/nonexistent/empty-state-talk.wav".into(),
                    ProbeInfo {
                        duration: Time(jvid_core::time::FLICKS_PER_SECOND * 5),
                        audio_streams: 1,
                        ..Default::default()
                    },
                )]))
                .unwrap();
            let asset = history.project().assets()[0].id;
            let placement = history
                .project()
                .placement(asset, Time(0), false, None, false)
                .unwrap();
            history.apply(Command::Place(placement)).unwrap();
        }
        let mut fresh = true;
        let project = history.project().clone();
        let mut h = Harness::builder().with_size([240.0, 360.0]).build_ui_state(
            move |ui,
                  (panel, asked): &mut (
                crate::transcript::TranscriptPanel,
                Option<Vec<jvid_transcribe::Source>>,
            )| {
                if install_theme(ui, &mut fresh) {
                    return;
                }
                let out = panel.show_docked(ui, &project, &HashSet::new(), Time(0), true);
                if out.transcribe.is_some() {
                    *asked = out.transcribe;
                }
            },
            (crate::transcript::TranscriptPanel::default(), None),
        );
        h.run();
        if audio {
            assert!(h.query_by_label("No Transcript Yet").is_some());
            h.get_by_label("Transcribe").click();
            h.run();
            let sources = h.state().1.as_ref().unwrap();
            assert_eq!(sources.len(), 1);
            assert_eq!(
                sources[0].path,
                std::path::Path::new("/nonexistent/empty-state-talk.wav")
            );
        } else {
            assert!(h.query_by_label("No Audio to Transcribe").is_some());
            assert!(h.query_by_label("Transcribe").is_none());
        }
    }
}

#[test]
fn inspector_no_selection_has_guidance_without_a_fake_action() {
    let mut fresh = true;
    let mut h = Harness::builder()
        .with_size([240.0, 220.0])
        .build_ui(move |ui| {
            if install_theme(ui, &mut fresh) {
                return;
            }
            let project = Project::default();
            let selected = crate::inspector::Selection::new(&project, &HashSet::new(), None);
            let (edit, seek) = crate::inspector::show(ui, &selected, None, Time(0));
            assert!(edit.is_none() && seek.is_none());
        });
    h.run();
    let title = h.get_by_label("Select a Clip").rect();
    let help = h
        .get_by_label("Select a clip in the timeline to edit its properties.")
        .rect();
    assert!(title.bottom() < help.top());
    assert!(
        h.query_all_by_role(egui::accesskit::Role::Button)
            .next()
            .is_none()
    );
}

#[test]
fn viewer_empty_guidance_stays_above_transport_and_scopes_has_its_own_message() {
    for source in [false, true] {
        let mut fresh = true;
        let mut h = Harness::builder().with_size([240.0, 260.0]).build_ui_state(
            move |ui, (preview, viewer): &mut (crate::preview::Preview, egui::Rect)| {
                if install_theme(ui, &mut fresh) {
                    return;
                }
                *viewer = crate::preview::viewer_rect(ui);
                if source {
                    preview.show_source(
                        ui,
                        &Project::default(),
                        &HashSet::new(),
                        &crate::keymap::Keymap::default(),
                    );
                } else {
                    preview.show_program(
                        ui,
                        &Project::default(),
                        Time(0),
                        None,
                        false,
                        &crate::keymap::Keymap::default(),
                    );
                }
            },
            (crate::preview::Preview::new(None), egui::Rect::NOTHING),
        );
        h.run();
        let title = if source {
            "Open a Clip"
        } else {
            "Nothing to Preview"
        };
        let rect = h.get_by_label(title).rect();
        assert!(h.state().1.contains_rect(rect));
    }
    let mut fresh = true;
    let mut h = Harness::builder()
        .with_size([240.0, 260.0])
        .build_ui(move |ui| {
            if install_theme(ui, &mut fresh) {
                return;
            }
            crate::scopes::show(ui);
        });
    h.run();
    assert!(h.query_by_label("No Frame to Measure").is_some());
}

#[test]
fn a_completed_transcript_without_speech_is_not_an_untranscribed_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("silence.wav");
    std::fs::write(&path, b"recognition identity fixture").unwrap();
    let mut history = History::new(Project::default());
    history
        .apply(Command::Import(vec![(
            path.clone(),
            ProbeInfo {
                duration: Time(jvid_core::time::FLICKS_PER_SECOND * 5),
                audio_streams: 1,
                ..Default::default()
            },
        )]))
        .unwrap();
    let asset = history.project().assets()[0].id;
    let placement = history
        .project()
        .placement(asset, Time(0), false, None, false)
        .unwrap();
    history.apply(Command::Place(placement)).unwrap();
    let project = history.project().clone();
    let mut panel = crate::transcript::TranscriptPanel::default();
    panel.cache = crate::transcript::Cache::new(dir.path().join("cache"));
    panel.cache.insert(
        &jvid_transcribe::Source {
            path,
            nth: 0,
            range: 0..i64::MAX,
        },
        jvid_transcribe::Transcript {
            language: Some("en".into()),
            words: Vec::new(),
        },
    );
    let mut fresh = true;
    let mut h = Harness::builder()
        .with_size([320.0, 400.0])
        .build_ui(move |ui| {
            if install_theme(ui, &mut fresh) {
                return;
            }
            panel.show_docked(ui, &project, &HashSet::new(), Time(0), false);
        });
    h.run();
    assert!(h.query_by_label("No Transcript Yet").is_none());
    assert!(
        h.query_by_label(crate::i18n::tr!("subtitles.no_speech"))
            .is_some()
    );
}

#[test]
fn scopes_waiting_for_the_first_measurement_is_not_empty() {
    let mut fresh = true;
    let mut h = Harness::builder()
        .with_size([240.0, 220.0])
        .build_ui(move |ui| {
            if install_theme(ui, &mut fresh) {
                return;
            }
            crate::scopes::pending(ui.ctx(), true);
            crate::scopes::show(ui);
        });
    h.run_steps(3);
    assert!(h.query_by_label("No Frame to Measure").is_none());
    assert!(h.query_by_label("Measuring Frame…").is_some());
}
