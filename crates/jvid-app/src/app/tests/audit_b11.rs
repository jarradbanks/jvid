//! Regressions for modal ownership, focus routing and transport input.
use super::*;
use egui::{Key, Modifiers};
use jvid_core::time::FLICKS_PER_SECOND as S;

fn editor() -> Harness<'static, App> {
    let mut h = plain_harness();
    let app = h.state_mut();
    app.on_welcome = false;
    crate::playback::tests::add_solid(&mut app.history, [0.2, 0.3, 0.4, 1.0], Time(0));
    let clip = app.history.project().sequence().tracks()[0].clips()[0].id;
    app.timeline
        .select_clip(app.history.project().sequence(), clip);
    app.timeline.seek(Time(S));
    app.preview
        .set_output(jvid_audio::Output::new(Box::new(LifecycleOutput), None));
    app.run_action("focus_timeline", false);
    h.run_steps(3);
    h
}

#[test]
fn audit_452_modals_own_first_frame_and_later_shortcuts() {
    for modal in ["settings", "preferences", "export", "prompt"] {
        for first_frame in [true, false] {
            let mut h = editor();
            let app = h.state_mut();
            match modal {
                "settings" => {
                    app.settings_dialog.draft = Some(app.history.project().settings().clone())
                }
                "preferences" => app.preferences.open(Vec::new(), None),
                "export" => app
                    .export_dialog
                    .open(app.history.project().sequence(), "test"),
                _ => app.prompts.push(Prompt::About),
            }
            assert!(
                h.state().modal_owns_input(&h.ctx),
                "{modal} owns input before drawing"
            );
            if !first_frame {
                h.run_steps(3);
            }
            let before = h.state().history.project().clone();
            let head = h.state().timeline.playhead();
            for (mods, key) in [
                (Modifiers::COMMAND, Key::Z),
                (Modifiers::NONE, Key::Delete),
                (Modifiers::NONE, Key::Space),
                (Modifiers::COMMAND, Key::M),
                (Modifiers::SHIFT, Key::R),
            ] {
                h.key_press_modifiers(mods, key);
                h.run_steps(2);
                assert_eq!(
                    h.state().history.project(),
                    &before,
                    "{modal} {key:?} first={first_frame}"
                );
                assert_eq!(h.state().timeline.playhead(), head);
                assert!(!h.state().preview.program_playing());
                assert!(h.state().take.is_none());
                // Space may activate the prompt's own focused OK button.
                if modal == "prompt" && key == Key::Space && h.state().prompts.is_empty() {
                    h.state_mut().prompts.push(Prompt::About);
                    h.run_steps(3);
                }
            }
            assert_eq!(
                h.state().settings_dialog.draft.is_some(),
                modal == "settings"
            );
            assert_eq!(h.state().preferences.is_open(), modal == "preferences");
            assert_eq!(h.state().export_dialog.is_open(), modal == "export");
            assert_eq!(h.state().prompts.len(), usize::from(modal == "prompt"));
            h.key_press(Key::Escape);
            h.run_steps(4);
            assert!(h.state().settings_dialog.draft.is_none());
            assert!(!h.state().preferences.is_open());
            assert!(!h.state().export_dialog.is_open());
            assert!(h.state().prompts.is_empty());
            h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
            h.run_steps(2);
            assert_ne!(
                h.state().history.project(),
                &before,
                "shortcuts resume: {modal}"
            );
        }
    }
}

#[test]
fn audit_452_modal_text_edit_keeps_typing_and_local_undo() {
    let mut h = editor();
    h.state_mut().keymap_editor.open();
    h.run_steps(3);
    let before = h.state().history.project().clone();
    let area = h
        .ctx
        .memory(|m| m.area_rect(egui::Id::new("keymap_editor")))
        .unwrap();
    let field = h
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .into_iter()
        .find(|n| area.contains_rect(n.rect()))
        .unwrap();
    field.focus();
    field.type_text("hello");
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(h.state().history.project(), &before);
    assert!(h.state().keymap_editor.is_open());
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(!h.state().keymap_editor.is_open());
}

#[test]
fn audit_452_modal_does_not_stop_an_active_take() {
    let (mut app, dir, _, _) = recording_app("b11-modal", true);
    app.on_welcome = false;
    app.preferences.open(Vec::new(), None);
    let mut h = builder([1280.0, 720.0], 1.0).build_eframe(move |cc| {
        theme::install(&cc.egui_ctx);
        app
    });
    h.run_steps(3);
    let before = h.state().history.project().clone();
    let wav = h.state().take.as_ref().unwrap().path.clone();
    for (mods, key) in [
        (Modifiers::NONE, Key::Space),
        (Modifiers::SHIFT, Key::R),
        (Modifiers::COMMAND, Key::Z),
        (Modifiers::NONE, Key::Delete),
    ] {
        h.key_press_modifiers(mods, key);
        h.run_steps(2);
        assert_eq!(h.state().take.as_ref().unwrap().path, wav);
        assert_eq!(h.state().history.project(), &before);
    }
    h.state_mut().end_take();
    drop(h);
    std::fs::remove_dir_all(dir).unwrap();
}
