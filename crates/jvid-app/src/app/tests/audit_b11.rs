//! Regressions for modal ownership, focus routing and transport input.
use super::*;
use egui::{Key, Modifiers};
use jvid_core::time::FLICKS_PER_SECOND as S;

fn editor() -> Harness<'static, App> {
    editor_with_gpu(false)
}

fn editor_with_gpu(with_gpu: bool) -> Harness<'static, App> {
    let builder = builder([1280.0, 720.0], 1.0).with_step_dt(1.0 / 60.0);
    let builder = if with_gpu { builder.wgpu() } else { builder };
    let mut h = builder.build_eframe(|cc| App::new(cc, &Default::default()));
    h.run();
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

/// RawInput keeps every press in the same egui frame. Harness::step otherwise
/// runs a separate frame for each Harness::event/key_press event.
fn shortcut_batch(h: &mut Harness<'_, App>, keys: &[(Modifiers, Key)]) {
    for &(modifiers, key) in keys {
        h.input_mut()
            .events
            .push(egui::Event::ModifiersChanged(modifiers));
        for pressed in [true, false] {
            h.input_mut().events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            });
        }
    }
    h.input_mut()
        .events
        .push(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

fn export_batch_blocks(modifiers: Modifiers, key: Key, focused: Option<&str>) {
    if !gpu() {
        return;
    }
    let mut h = editor_with_gpu(true);
    assert!(
        h.state().can_export(),
        "exercise the actual Export shortcut"
    );
    let dir = std::env::temp_dir().join(format!(
        "jvid-b11-batch-{key:?}-{}-{}",
        focused.unwrap_or("global"),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mic = crate::record::tests::FakeMic::default();
    h.state_mut().path = Some(dir.join("p.jvid"));
    h.state_mut().input = Some(Box::new(mic.clone()));
    if let Some(label) = focused {
        h.get_by_label(label).focus();
        h.run_steps(2);
        assert!(h.get_by_label(label).is_focused());
    } else {
        h.ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }
    let before = h.state().history.project().clone();
    let selected = h.state().timeline.selected().clone();
    let head = h.state().timeline.playhead();
    shortcut_batch(&mut h, &[(Modifiers::COMMAND, Key::M), (modifiers, key)]);
    let opened = h.state().export_dialog.is_open();
    let project_changed = h.state().history.project() != &before;
    let selection_changed = h.state().timeline.selected() != &selected;
    let head_changed = h.state().timeline.playhead() != head;
    let playing = h.state().preview.program_playing();
    let recording = h.state().take.is_some();
    let mic_opened = mic.0.lock().unwrap().is_some();
    drop(h);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(opened, "Export must open before {key:?}");
    assert!(
        !project_changed
            && !selection_changed
            && !head_changed
            && !playing
            && !recording
            && !mic_opened,
        "Export then {key:?}, focused={focused:?}, in one frame: project_changed={project_changed}, \
         selection_changed={selection_changed}, head_changed={head_changed}, \
         playing={playing}, recording={recording}, mic_opened={mic_opened}"
    );
}

#[test]
fn audit_452_export_batch_blocks_delete() {
    export_batch_blocks(Modifiers::NONE, Key::Delete, None);
}

#[test]
fn audit_452_export_batch_blocks_undo() {
    export_batch_blocks(Modifiers::COMMAND, Key::Z, None);
}

#[test]
fn audit_452_export_batch_blocks_space() {
    export_batch_blocks(Modifiers::NONE, Key::Space, None);
}

#[test]
fn audit_452_export_batch_blocks_record() {
    export_batch_blocks(Modifiers::SHIFT, Key::R, None);
}

#[test]
fn audit_452_export_batch_blocks_focused_play_space() {
    export_batch_blocks(Modifiers::NONE, Key::Space, Some("Play (Space)"));
}

#[test]
fn audit_452_export_batch_blocks_focused_play_enter() {
    export_batch_blocks(Modifiers::NONE, Key::Enter, Some("Play (Space)"));
}

#[test]
fn audit_452_export_batch_blocks_focused_record_space() {
    export_batch_blocks(
        Modifiers::NONE,
        Key::Space,
        Some("Record Voiceover (Shift+R)"),
    );
}

#[test]
fn audit_452_export_batch_blocks_focused_record_enter() {
    export_batch_blocks(
        Modifiers::NONE,
        Key::Enter,
        Some("Record Voiceover (Shift+R)"),
    );
}

#[test]
fn audit_452_nonmodal_batch_runs_every_shortcut() {
    let mut h = editor();
    assert!(!h.state().can_export(), "Export cannot open without a GPU");
    let before = h.state().history.project().clone();
    let head = h.state().timeline.playhead();
    shortcut_batch(
        &mut h,
        &[
            (Modifiers::COMMAND, Key::M),
            (Modifiers::COMMAND, Key::Z),
            (Modifiers::COMMAND, Key::Y),
            (Modifiers::NONE, Key::ArrowRight),
        ],
    );
    assert_eq!(h.state().history.project(), &before, "Undo then Redo");
    assert!(h.state().timeline.playhead() > head, "then step forward");
}

#[test]
fn audit_452_prompt_dismissal_keeps_batch_owned() {
    for dismiss in [Key::Escape, Key::Enter] {
        let mut h = editor();
        h.state_mut().prompts.push(Prompt::About);
        h.run_steps(3);
        let before = h.state().history.project().clone();
        shortcut_batch(
            &mut h,
            &[(Modifiers::NONE, dismiss), (Modifiers::COMMAND, Key::Z)],
        );
        assert!(h.state().prompts.is_empty(), "{dismiss:?}");
        assert_eq!(h.state().history.project(), &before, "{dismiss:?}");
        h.run_steps(3);
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run_steps(2);
        assert_ne!(h.state().history.project(), &before, "Undo resumes");
    }
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

fn source_editor() -> (Harness<'static, App>, AssetId) {
    let mut h = editor();
    let app = h.state_mut();
    let path = PathBuf::from("/nonexistent/long-recording.wav");
    app.history
        .apply(Command::Import(vec![(
            path.clone(),
            jvid_core::model::ProbeInfo {
                duration: Time(1800 * S),
                frame_rate: Some(jvid_core::time::Rate::new(30, 1)),
                width: Some(1920),
                height: Some(1080),
                ..Default::default()
            },
        )]))
        .unwrap();
    app.missing.insert(path);
    let id = app.history.project().assets().last().unwrap().id;
    app.run_action("focus_media", false);
    app.bin.list = true;
    app.bin.query = "long-recording.wav".into();
    h.run_steps(3);
    (h, id)
}

fn bin_asset_pos(h: &Harness<'_, App>) -> egui::Pos2 {
    let rect = h
        .state()
        .dock
        .iter_leaves()
        .find(|(_, l)| l.tabs.contains(&Tab::Media))
        .unwrap()
        .1
        .rect;
    let search = h
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .into_iter()
        .find(|n| rect.contains_rect(n.rect()))
        .unwrap()
        .rect();
    fn text_rect(shape: &egui::Shape, search: Rect, name: &str) -> Option<Rect> {
        match shape {
            egui::Shape::Text(t)
                if t.pos.y > search.bottom()
                    && t.galley.job.text == name
                    && t.pos.x >= search.left()
                    && t.pos.x < search.right() =>
            {
                Some(Rect::from_min_size(t.pos, t.galley.size()))
            }
            egui::Shape::Vec(v) => v.iter().find_map(|s| text_rect(s, search, name)),
            _ => None,
        }
    }
    let found = h
        .output()
        .shapes
        .iter()
        .filter(|s| s.clip_rect.intersects(rect))
        .find_map(|s| text_rect(&s.shape, search, &h.state().bin.query))
        .unwrap();
    found.center()
}

fn double_click(h: &mut Harness<'_, App>, label: &str) {
    h.state_mut().bin.query = label.into();
    h.input_mut().time = Some(h.ctx.input(|i| i.time) + 1.0);
    h.run_steps(2);
    let at = bin_asset_pos(h);
    h.hover_at(at);
    h.step();
    for pressed in [true, false, true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.step();
    }
    h.run_steps(2);
}

#[test]
fn audit_457_bin_double_click_focuses_source_and_routes_following_keys() {
    let (mut h, id) = source_editor();
    double_click(&mut h, "long-recording.wav");
    assert_eq!(h.state().preview.source_asset(), Some(id));
    assert_eq!(
        h.state_mut().dock.find_active_focused().map(|(_, t)| *t),
        Some(Tab::Preview)
    );
    let head = h.state().timeline.playhead();
    h.key_press(Key::Space);
    h.run_steps(2);
    assert_eq!(h.state().preview.source_speed(), Some(1));
    assert_eq!(h.state().preview.program_speed(), 0);
    h.key_press(Key::Space);
    h.key_press(Key::I);
    h.run_steps(2);
    h.key_press(Key::ArrowRight);
    h.run_steps(2);
    h.key_press(Key::O);
    h.run_steps(2);
    let p = h.state().history.project();
    let a = p.asset(id).unwrap();
    let rate = p.settings().frame_rate;
    assert_eq!(a.in_point, Some(Time(0)));
    assert_eq!(a.out_point, Some(rate.time_floor(2)));
    assert!(p.sequence().in_point().is_none() && p.sequence().out_point().is_none());
    assert_eq!(h.state().timeline.playhead(), head);
    // Source marks are history edits and survive the project-file round trip.
    let saved = jvid_core::file::to_json(p, Path::new("/")).unwrap();
    let restored = jvid_core::file::from_json(&saved, Path::new("/"), |_| true).unwrap();
    assert_eq!(restored.asset(id).unwrap().in_point, a.in_point);
    assert_eq!(restored.asset(id).unwrap().out_point, a.out_point);
    h.state_mut().history.undo();
    assert!(
        h.state()
            .history
            .project()
            .asset(id)
            .unwrap()
            .out_point
            .is_none()
    );
    h.state_mut().history.redo();
    let mut track = h.state().history.project().sequence().tracks()[0].settings();
    track.target = true;
    let target = track.id;
    h.state_mut()
        .history
        .apply(Command::SetTrack(track))
        .unwrap();
    for key in [Key::Comma, Key::Period] {
        h.key_press(key);
        h.run_steps(2);
        assert!(
            h.state()
                .history
                .project()
                .sequence()
                .track(target)
                .unwrap()
                .clips()
                .iter()
                .any(|c| c.asset == id && c.source_out == rate.time_floor(2))
        );
    }
}

#[test]
fn audit_457_single_click_and_nested_sequence_keep_existing_destinations() {
    let (mut h, _) = source_editor();
    let at = bin_asset_pos(&h);
    h.hover_at(at);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    h.run_steps(2);
    assert_eq!(
        h.state_mut().dock.find_active_focused().map(|(_, t)| *t),
        Some(Tab::Media)
    );
    assert!(h.state().preview.source_asset().is_none());
    let clip = h.state().history.project().sequence().tracks()[0].clips()[0].id;
    h.state_mut()
        .history
        .apply(Command::Nest {
            ids: vec![clip],
            name: "Nested".into(),
            outer: "Main".into(),
        })
        .unwrap();
    let seq = h
        .state()
        .history
        .project()
        .assets()
        .iter()
        .find(|a| a.display_name() == "Nested")
        .unwrap()
        .sequence
        .unwrap();
    h.run_steps(2);
    double_click(&mut h, "Nested");
    assert_eq!(h.state().history.project().sequence().id(), seq);
    assert!(h.state().preview.source_asset().is_none());
}

#[test]
fn audit_470_default_and_remapped_focus_effects_select_and_focus_bin() {
    for origin in ["focus_timeline", "focus_source", "focus_inspector"] {
        for remapped in [false, true] {
            let mut h = editor();
            h.state_mut().run_action(origin, false);
            h.state_mut().bin.tab = 2;
            let selected = h.state().timeline.selected().clone();
            if remapped {
                h.state_mut().keymap = Keymap::from_toml("focus_effects = \"Ctrl+Alt+E\"").unwrap();
            }
            h.key_press_modifiers(
                if remapped {
                    Modifiers::COMMAND | Modifiers::ALT
                } else {
                    Modifiers::SHIFT
                },
                if remapped { Key::E } else { Key::Num7 },
            );
            h.run_steps(3);
            assert_eq!(h.state().bin.tab, 1);
            assert_eq!(
                h.state_mut().dock.find_active_focused().map(|(_, t)| *t),
                Some(Tab::Media)
            );
            assert_eq!(h.state().timeline.selected(), &selected);
            h.state_mut().run_action("focus_timeline", false);
            h.key_press_modifiers(Modifiers::SHIFT, Key::Num1);
            h.run_steps(2);
            assert_eq!(
                h.state_mut().dock.find_active_focused().map(|(_, t)| *t),
                Some(Tab::Media)
            );
            assert_eq!(
                h.state().bin.tab,
                1,
                "Focus Media retains its existing tab policy"
            );
        }
    }
}

fn pointer_click(h: &mut Harness<'_, App>, at: egui::Pos2) {
    h.hover_at(at);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    h.run_steps(2);
}

#[test]
fn audit_463_source_strip_seeks_long_recording_and_keeps_marks_and_timeline_independent() {
    let (mut h, id) = source_editor();
    double_click(&mut h, "long-recording.wav");
    let strip = h.get_by_label(tr!("preview.position")).rect();
    assert!(strip.height() >= 12.0, "a practical scrub hit area");
    let before = h.state().history.project().clone();
    let head = h.state().timeline.playhead();
    pointer_click(&mut h, strip.center());
    assert!((h.state().preview.source_frame().unwrap() - 27000).abs() <= 1);
    assert_eq!(h.state().timeline.playhead(), head);
    assert_eq!(
        h.state().history.project(),
        &before,
        "seeking is not a history edit"
    );
    h.key_press(Key::I);
    h.run_steps(2);
    h.key_press(Key::ArrowRight);
    h.run_steps(2);
    h.key_press(Key::O);
    h.run_steps(2);
    let rate = h.state().history.project().settings().frame_rate;
    let marked = h.state().history.project().asset(id).unwrap();
    assert_eq!(marked.in_point, Some(rate.time_floor(27000)));
    assert_eq!(marked.out_point, Some(rate.time_floor(27002)));
    let before = h.state().history.project().clone();
    h.drag_at(strip.center());
    h.step();
    h.hover_at(egui::pos2(strip.left() - 100.0, strip.center().y));
    h.run_steps(2);
    assert_eq!(h.state().preview.source_frame(), Some(0));
    h.hover_at(egui::pos2(strip.right() + 100.0, strip.center().y));
    h.run_steps(2);
    assert_eq!(h.state().preview.source_frame(), Some(53999));
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(strip.right() + 100.0, strip.center().y),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    assert_eq!(h.state().history.project(), &before);
    assert_eq!(h.state().timeline.playhead(), head);
}

#[test]
fn audit_463_program_strip_uses_timeline_seek_without_a_history_edit() {
    let mut h = editor();
    h.state_mut().timeline.seek(Time(0));
    h.run_steps(2);
    let before = h.state().history.project().clone();
    let strip = h.get_by_label(tr!("preview.position")).rect();
    pointer_click(&mut h, strip.center());
    assert_eq!(h.state().timeline.playhead(), Time(S / 2));
    assert_eq!(h.state().history.project(), &before);
}
