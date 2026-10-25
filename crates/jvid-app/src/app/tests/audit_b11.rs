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
    let value = |h: &Harness<'_, App>| {
        h.get_all_by_role(egui::accesskit::Role::TextInput)
            .into_iter()
            .find(|n| area.contains_rect(n.rect()))
            .unwrap()
            .value()
    };
    assert_eq!(value(&h).as_deref(), Some("hello"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(value(&h).as_deref(), Some(""));
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

fn tab_to(h: &mut Harness<'_, App>, label: &str) {
    for _ in 0..160 {
        if h.get_by_label(label).is_focused() {
            return;
        }
        h.key_press(Key::Tab);
        h.run_steps(2);
    }
    panic!("Tab never reached {label}");
}

#[test]
fn audit_453_tab_space_and_pointer_each_run_one_transport_action() {
    for (label, expected) in [("Play (Space)", 1), ("Shuttle Right (L)", 1)] {
        let mut h = editor();
        h.state_mut().timeline.seek(Time(0));
        h.state_mut().run_action("focus_program", false);
        h.run_steps(2);
        tab_to(&mut h, label);
        h.key_press(Key::Space);
        h.run_steps(2);
        assert_eq!(h.state().preview.program_speed(), expected, "{label}");
    }
    let mut h = editor();
    h.state_mut().timeline.seek(Time(0));
    h.run_steps(2);
    tab_to(&mut h, "Play (Space)");
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().preview.program_speed(), 1);
    h.get_by_label("Stop (Space)").click();
    h.run_steps(2);
    h.get_by_label("Play (Space)").click();
    h.run_steps(2);
    assert_eq!(h.state().preview.program_speed(), 1);
    h.get_by_label("Stop (Space)").click();
    h.run_steps(2);
    assert_eq!(h.state().preview.program_speed(), 0);
    // With no widget focused, the global default toggles once; remapping it does too.
    h.ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
    h.state_mut().run_action("focus_timeline", false);
    h.key_press(Key::Space);
    h.run_steps(2);
    assert_eq!(h.state().preview.program_speed(), 1);
    h.state_mut().keymap = Keymap::from_toml("play_stop = \"P\"").unwrap();
    h.key_press(Key::P);
    h.run_steps(2);
    assert_eq!(h.state().preview.program_speed(), 0);
    h.key_press(Key::Space);
    h.run_steps(2);
    assert_eq!(h.state().preview.program_speed(), 0);
    h.key_press(Key::P);
    h.run_steps(2);
    assert_eq!(h.state().preview.program_speed(), 1);
}

#[test]
fn audit_453_tab_space_record_and_stop_do_not_restart_take() {
    let mut h = editor();
    let dir = std::env::temp_dir().join(format!("jvid-b11-record-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mic = crate::record::tests::FakeMic::default();
    h.state_mut().path = Some(dir.join("p.jvid"));
    h.state_mut().input = Some(Box::new(mic.clone()));
    h.state_mut().timeline.seek(Time(0));
    h.run_steps(2);
    tab_to(&mut h, "Record Voiceover (Shift+R)");
    h.key_press(Key::Space);
    h.run_steps(2);
    assert!(h.state().take.is_some());
    mic.0.lock().unwrap().as_ref().unwrap().push(&[0.25; 4800]);
    assert!(h.get_by_label("Stop Recording (Shift+R)").is_focused());
    h.key_press(Key::Space);
    h.run_steps(2);
    assert!(
        h.state().take.is_none(),
        "Stop must not immediately start a new take"
    );
    assert_eq!(h.state().preview.program_speed(), 0);
    drop(h);
    std::fs::remove_dir_all(dir).unwrap();
}
