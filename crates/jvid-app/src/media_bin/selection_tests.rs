//! Exercises the same pointer and keyboard paths in both Media layouts, without a GPU.

use super::*;
use egui::{Event, Modifiers, PointerButton};
use egui_kittest::{Harness, kittest::Queryable};
use jvid_core::{
    file,
    history::{Created, History},
    model::{NewClip, ProbeInfo, TrackKind},
    time::Time,
};

struct State {
    bin: MediaBin,
    history: History,
    focused: bool,
    opened: Option<AssetId>,
}

fn harness(list: bool) -> Harness<'static, State> {
    let mut history = History::new(Project::default());
    history
        .apply(Command::Import(
            [
                "Zulu.mov",
                "Keep Bravo.mov",
                "Keep Alpha.mov",
                "Keep Charlie.mov",
                "Unrelated.wav",
            ]
            .into_iter()
            .map(|name| (name.into(), ProbeInfo::default()))
            .collect(),
        ))
        .unwrap();
    let mut fresh = true;
    let mut h = Harness::builder()
        .with_size([520.0, 620.0])
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            move |ui, state: &mut State| {
                if suite_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                // Keep interaction timing realistic without waiting for theme/modal fades.
                ui.ctx()
                    .global_style_mut(|style| style.animation_time = 0.0);
                ui.style_mut().animation_time = 0.0;
                let out = state.bin.show(
                    ui,
                    state.history.project(),
                    state.focused,
                    false,
                    &HashSet::new(),
                );
                if let Some(c) = out.command {
                    state.history.apply(c).unwrap();
                }
                state.opened = state.opened.or(out.open);
            },
            State {
                bin: MediaBin {
                    list,
                    ..Default::default()
                },
                history,
                focused: true,
                opened: None,
            },
        );
    h.run();
    h
}

fn click(h: &mut Harness<'_, State>, name: &str, modifiers: Modifiers) {
    let pos = h.get_by_label(name).rect().center();
    h.event(Event::ModifiersChanged(modifiers));
    h.hover_at(pos);
    h.run();
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers,
        });
        h.run();
    }
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run();
}

fn selection(h: &Harness<'_, State>) -> HashSet<AssetId> {
    h.state().bin.selected_ids.clone()
}

#[test]
fn preview_double_click_and_asset_drag_still_work_in_grid_and_list() {
    for list in [false, true] {
        let mut h = harness(list);
        h.get_by_label("Keep Alpha.mov").click();
        h.run();
        h.get_by_label("Keep Alpha.mov").click();
        h.run();
        assert_eq!(h.state().opened, Some(AssetId(3)));
        let pos = h.get_by_label("Zulu.mov").rect().center();
        h.hover_at(pos);
        h.run();
        h.drag_at(pos);
        h.run();
        h.hover_at(pos + vec2(30.0, 30.0));
        h.step();
        assert_eq!(
            egui::DragAndDrop::payload::<AssetId>(&h.ctx).as_deref(),
            Some(&AssetId(1))
        );
        h.drop_at(pos + vec2(50.0, 50.0));
        h.run();
        assert_eq!(h.state().history.project().assets().len(), 5);
    }
}

#[test]
fn grid_and_list_toggle_range_and_select_all_use_the_visible_sorted_results() {
    for list in [false, true] {
        let mut h = harness(list);
        let alpha_rect = h.get_by_label("Keep Alpha.mov").rect();
        click(&mut h, "Zulu.mov", Modifiers::NONE);
        click(&mut h, "Keep Alpha.mov", Modifiers::COMMAND);
        assert_eq!(selection(&h), HashSet::from([AssetId(1), AssetId(3)]));
        assert_eq!(h.get_by_label("Keep Alpha.mov").rect(), alpha_rect);
        click(&mut h, "Zulu.mov", Modifiers::COMMAND);
        assert_eq!(selection(&h), HashSet::from([AssetId(3)]));

        h.state_mut().bin.query = "Keep".into();
        h.state_mut().bin.sort = 1;
        h.run();
        click(&mut h, "Keep Bravo.mov", Modifiers::NONE);
        click(&mut h, "Keep Charlie.mov", Modifiers::SHIFT);
        assert_eq!(selection(&h), HashSet::from([AssetId(2), AssetId(4)]));
        click(&mut h, "Keep Alpha.mov", Modifiers::NONE);
        click(&mut h, "Keep Charlie.mov", Modifiers::SHIFT);
        assert_eq!(
            selection(&h),
            HashSet::from([AssetId(3), AssetId(2), AssetId(4)])
        );
        // A view change retains the same logical selection.
        h.state_mut().bin.list = !list;
        h.run();
        assert_eq!(
            selection(&h),
            HashSet::from([AssetId(3), AssetId(2), AssetId(4)])
        );
        // A plain click replaces, and select-all selects matches rather than the project.
        click(&mut h, "Keep Bravo.mov", Modifiers::NONE);
        assert_eq!(selection(&h), HashSet::from([AssetId(2)]));
        h.key_press_modifiers(Modifiers::COMMAND, Key::A);
        h.run();
        assert_eq!(
            selection(&h),
            HashSet::from([AssetId(2), AssetId(3), AssetId(4)])
        );
        h.state_mut().bin.query = "Charlie".into();
        h.run();
        assert_eq!(selection(&h), HashSet::from([AssetId(4)]));
        h.key_press(Key::Delete);
        h.run();
        assert!(h.state().history.project().asset(AssetId(4)).is_none());
        assert_eq!(h.state().history.project().assets().len(), 4);
        assert!(selection(&h).is_empty());
        assert!(h.state_mut().history.undo());
        h.run();
        assert_eq!(h.state().history.project().assets().len(), 5);
    }
}

#[test]
fn filtered_ranges_exclude_hidden_assets_between_the_endpoints() {
    for list in [false, true] {
        let mut h = harness(list);
        h.state_mut()
            .history
            .apply(Command::AddAsset("Keep Echo.mov".into()))
            .unwrap();
        h.state_mut().bin.query = "Keep".into();
        h.run();
        click(&mut h, "Keep Charlie.mov", Modifiers::NONE);
        click(&mut h, "Keep Echo.mov", Modifiers::SHIFT);
        assert_eq!(selection(&h), HashSet::from([AssetId(4), AssetId(6)]));
        h.key_press(Key::Delete);
        h.run();
        assert!(h.state().history.project().asset(AssetId(5)).is_some());
        assert!(h.state().history.project().asset(AssetId(4)).is_none());
        assert!(h.state().history.project().asset(AssetId(6)).is_none());
    }
}

#[test]
fn media_shortcuts_require_panel_focus_and_leave_search_and_rename_editing_alone() {
    for list in [false, true] {
        let mut h = harness(list);
        click(&mut h, "Keep Alpha.mov", Modifiers::NONE);
        h.state_mut().focused = false;
        h.key_press_modifiers(Modifiers::COMMAND, Key::A);
        h.key_press(Key::Delete);
        h.run();
        assert_eq!(selection(&h), HashSet::from([AssetId(3)]));
        assert_eq!(h.state().history.project().assets().len(), 5);

        h.state_mut().focused = true;
        h.get_by_role(egui::accesskit::Role::TextInput).focus();
        h.run();
        h.key_press_modifiers(Modifiers::COMMAND, Key::A);
        h.key_press(Key::Delete);
        h.run();
        assert_eq!(selection(&h), HashSet::from([AssetId(3)]));
        assert_eq!(h.state().history.project().assets().len(), 5);
        h.state_mut().bin.renaming = Some((AssetId(3), "Keep Alpha".into()));
        h.run();
        h.key_press(Key::Delete);
        h.run();
        assert_eq!(h.state().history.project().assets().len(), 5);
        assert!(h.state().bin.confirm_remove.is_none());

        h.state_mut().bin.renaming = None;
        h.state_mut().bin.tab = 2;
        h.key_press_modifiers(Modifiers::COMMAND, Key::A);
        h.key_press(Key::Delete);
        h.run();
        assert_eq!(h.state().history.project().assets().len(), 5);
    }
}

#[test]
fn mixed_used_and_unused_selection_has_one_warning_and_one_undo_step() {
    for list in [false, true] {
        let mut h = harness(list);
        let history = &mut h.state_mut().history;
        let Created::Track(track) = history
            .apply(Command::AddTrack {
                kind: TrackKind::Video,
                name: "V1".into(),
            })
            .unwrap()
        else {
            panic!()
        };
        for start in [0, 20] {
            history
                .apply(Command::AddClip {
                    track,
                    clip: NewClip {
                        asset: AssetId(3),
                        source_in: Time(0),
                        source_out: Time(10),
                        start: Time(start),
                        link: None,
                    },
                })
                .unwrap();
        }
        let before = history.project().clone();
        h.run();
        click(&mut h, "Keep Alpha.mov", Modifiers::NONE);
        click(&mut h, "Unrelated.wav", Modifiers::COMMAND);
        h.get_by_label("Remove Selected").click();
        h.run();
        assert_eq!(h.state().bin.pending_remove, vec![AssetId(3), AssetId(5)]);
        assert_eq!(h.state().history.project(), &before);
        h.get_by_label("Original files on disk are kept.");
        h.get_by_label("Remove 2 selected assets?");
        h.get_by_label("In-use assets: 1. Removing the selection also removes 2 timeline clips. You can undo this.");
        h.get_by_label("Cancel").click();
        h.run();
        assert_eq!(h.state().history.project(), &before);
        assert_eq!(selection(&h), HashSet::from([AssetId(3), AssetId(5)]));
        h.key_press(Key::Delete);
        h.run();
        h.get_by_label("Remove").click();
        h.run();
        assert_eq!(h.state().history.project().assets().len(), 3);
        assert!(
            h.state()
                .history
                .project()
                .sequence()
                .track(track)
                .unwrap()
                .clips()
                .is_empty()
        );
        assert!(selection(&h).is_empty());
        assert!(h.state().bin.selected.is_none());
        assert!(h.state_mut().history.undo());
        assert_eq!(h.state().history.project(), &before);
        assert!(h.state_mut().history.redo());
        h.run();
        assert_eq!(h.state().history.project().assets().len(), 3);
    }
}

#[test]
fn unused_batch_removal_preserves_disk_files_and_undo_restores_all_assets() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), b"original footage").unwrap();
    let mut h = harness(false);
    h.state_mut()
        .history
        .apply(Command::Import(vec![(
            file.path().to_owned(),
            ProbeInfo::default(),
        )]))
        .unwrap();
    let before = h.state().history.project().clone();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run();
    h.key_press(Key::Delete);
    h.run();
    assert!(h.state().history.project().assets().is_empty());
    assert!(h.state().bin.confirm_remove.is_none());
    assert_eq!(std::fs::read(file.path()).unwrap(), b"original footage");
    assert!(h.state_mut().history.undo());
    assert_eq!(h.state().history.project(), &before);
}

#[test]
fn subtitle_provenance_hides_only_captions_and_survives_save_load_and_undo() {
    let mut h = harness(false);
    for command in [
        Command::AddText {
            name: "Reusable title".into(),
            text: Text::default(),
        },
        Command::AddSolid {
            name: "Reusable solid".into(),
            color: [0.0, 0.0, 0.0, 1.0],
        },
    ] {
        h.state_mut().history.apply(command).unwrap();
    }
    let before = h.state().history.project().clone();
    h.state_mut().history.apply(Command::ImportSubtitles {
        name: "Captions".into(),
        cues: jvid_core::srt::parse("1\n00:00:01,000 --> 00:00:03,000\nCaption one\n\n2\n00:00:04,000 --> 00:00:06,000\nCaption two\n"),
        style: Text::default(),
    }).unwrap();
    h.run();
    let project = h.state().history.project();
    assert_eq!(project.assets().len(), 9);
    assert_eq!(visible(project.assets(), "", 0).len(), 7);
    assert_eq!(project.subtitle_cues().len(), 2);
    assert!(h.query_by_label("Caption one").is_none());
    h.get_by_label("Reusable title");
    h.get_by_label("Reusable solid");
    let json = file::to_json(project, std::path::Path::new("/project")).unwrap();
    let loaded = file::from_json(&json, std::path::Path::new("/project"), |_| false).unwrap();
    assert_eq!(&loaded, project);
    assert_eq!(visible(loaded.assets(), "", 0).len(), 7);
    assert!(h.state_mut().history.undo());
    assert_eq!(h.state().history.project(), &before);
    assert!(h.state_mut().history.redo());
    h.run();
    assert_eq!(
        visible(h.state().history.project().assets(), "", 0).len(),
        7
    );
    // Legacy files lack provenance; ordinary titles and imports stay visible.
    let legacy = json.replace("\"generated_caption\": true", "\"legacy_caption\": true");
    let legacy = file::from_json(&legacy, std::path::Path::new("/project"), |_| false).unwrap();
    assert_eq!(visible(legacy.assets(), "", 0).len(), 9);
    assert_eq!(legacy.subtitle_cues().len(), 2);
}
