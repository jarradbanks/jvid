//! Real dock gestures and their persisted layout; no recognition model or media downloads.

use egui::accesskit::Role;
use egui::{Key, Modifiers, Pos2};
use egui_dock::{NodePath, Split, TabDestination, TabIndex, TabInsert};

use super::*;

fn ui_harness(size: [f32; 2], scale: f32) -> Harness<'static, App> {
    let mut h = builder(size, scale).build_eframe(|cc| App::new(cc, &Default::default()));
    h.state_mut().on_welcome = false;
    h.run();
    h
}

fn tab_rect(h: &Harness<'_, App>, tab: Tab) -> Rect {
    h.get_all_by_role(Role::Tab)
        .find(|n| n.accesskit_node().label().as_deref() == Some(tab.title()))
        .unwrap_or_else(|| panic!("missing dock tab {tab:?}"))
        .rect()
}

fn drag(h: &mut Harness<'_, App>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(2);
    h.drag_at(from);
    h.run_steps(2);
    for n in 1..=12 {
        h.hover_at(from + (to - from) * (n as f32 / 12.0));
        h.run_steps(2);
    }
    h.drop_at(to);
    h.run();
}

fn group(app: &mut App, tab: Tab, target: Tab, index: usize) {
    let from = app.dock.find_tab(&tab).unwrap();
    let to = app.dock.find_tab(&target).unwrap().node_path();
    app.dock.move_tab(
        from,
        TabDestination::Node(to, TabInsert::Insert(TabIndex(index))),
    );
}

fn node(app: &App, tab: Tab) -> NodePath {
    app.dock.find_tab(&tab).unwrap().node_path()
}

fn save(app: &mut App) -> Memory {
    let mut storage = Memory::default();
    eframe::App::save(app, &mut storage);
    storage
}

#[test]
fn root_bottom_target_requires_a_tab_drag_and_escape_dismisses_it() {
    let mut h = ui_harness([1280.0, 720.0], 1.0);
    let label = tr!("panel.dock_bottom");
    h.get_all_by_role(Role::Tab)
        .find(|n| n.accesskit_node().label().as_deref() == Some(Tab::Media.title()))
        .unwrap()
        .click();
    h.run();
    assert!(
        h.query_by_label(label).is_none(),
        "clicking a tab never opens a drop target"
    );
    let before = node(h.state(), Tab::Media);
    let from = tab_rect(&h, Tab::Media).center();
    h.hover_at(from);
    h.run_steps(2);
    h.drag_at(from);
    h.run_steps(2);
    h.hover_at(from + vec2(0.0, 60.0));
    h.run_steps(2);
    assert!(h.query_by_label(label).is_some());
    h.key_press(Key::Escape);
    h.run();
    assert!(
        h.query_by_label(label).is_none(),
        "Escape dismisses the root destination"
    );
    h.drop_at(from);
    h.run();
    assert_eq!(node(h.state(), Tab::Media), before);
    assert_eq!(h.state().dock.iter_all_tabs().count(), 4);
}

#[test]
fn drag_middle_column_timeline_to_root_bottom_and_preserve_open_panels() {
    let mut snapshots = egui_kittest::SnapshotResults::new();
    for scale in SCALES {
        let mut h = ui_harness([1280.0, 720.0], scale);
        crate::playback::tests::add_solid(
            &mut h.state_mut().history,
            [0.2, 0.3, 0.4, 1.0],
            Time(0),
        );
        h.state_mut().show_panel(Tab::Transcript);
        h.state_mut().show_queue();
        group(h.state_mut(), Tab::Transcript, Tab::Media, 1);
        group(h.state_mut(), Tab::RenderQueue, Tab::Inspector, 1);
        h.run();
        let project = h.state().history.project().clone();
        let preview = h
            .state()
            .dock
            .leaf(node(h.state(), Tab::Preview))
            .unwrap()
            .rect;
        let from = tab_rect(&h, Tab::Timeline).center();
        drag(
            &mut h,
            from,
            egui::pos2(preview.center().x, preview.bottom() - 20.0),
        );
        let timeline = h
            .state()
            .dock
            .leaf(node(h.state(), Tab::Timeline))
            .unwrap()
            .rect;
        assert!(
            timeline.width() < 1000.0,
            "reproduce Timeline below Preview only"
        );
        let root = h.state().dock.main_surface()[NodeIndex::root()]
            .rect()
            .unwrap();
        let from = tab_rect(&h, Tab::Timeline).center();
        h.hover_at(from);
        h.run_steps(2);
        h.drag_at(from);
        h.run_steps(2);
        h.hover_at(from + vec2(0.0, 40.0));
        h.run_steps(2);
        let target = h.get_by_label(tr!("panel.dock_bottom")).rect();
        assert!((target.width() - root.width()).abs() < 2.0);
        h.hover_at(target.center());
        h.run_steps(2);
        if gpu() {
            snapshots.add(h.try_snapshot(format!("dock_timeline_root_bottom_{scale}x")));
        }
        h.drop_at(target.center());
        h.run();
        let timeline = h
            .state()
            .dock
            .leaf(node(h.state(), Tab::Timeline))
            .unwrap()
            .rect;
        assert!(
            (timeline.width() - root.width()).abs() < 2.0,
            "{timeline:?} vs {root:?}"
        );
        assert_eq!(
            node(h.state(), Tab::Transcript),
            node(h.state(), Tab::Media)
        );
        assert_eq!(
            node(h.state(), Tab::RenderQueue),
            node(h.state(), Tab::Inspector)
        );
        assert_eq!(h.state().dock.iter_all_tabs().count(), 6);
        assert_eq!(h.state().history.project(), &project);
        let saved = save(h.state_mut());
        let restored = App::restore(Some(&saved));
        let mut actual = Memory::default();
        eframe::set_value(&mut actual, DOCK_KEY, &restored.dock);
        assert_eq!(actual.0[DOCK_KEY], saved.0[DOCK_KEY]);
        h.state_mut().reset_layout();
        assert_eq!(h.state().dock.iter_all_tabs().count(), 6);
        assert_eq!(h.state().history.project(), &project);
        assert_eq!(
            h.state_mut().dock.find_active_focused().map(|(_, t)| *t),
            Some(Tab::Timeline)
        );
    }
}

#[test]
fn drag_optional_tabs_into_timeline_reorder_and_split_out() {
    for scale in SCALES {
        let mut h = ui_harness([1280.0, 720.0], scale);
        for tab in [Tab::Transcript, Tab::RenderQueue] {
            h.state_mut().show_panel(tab);
            h.run();
            let from = tab_rect(&h, tab).center();
            let timeline = tab_rect(&h, Tab::Timeline);
            // The strip's empty space appends the dropped panel to its group.
            let to = timeline.right_center() + vec2(12.0, 0.0);
            drag(&mut h, from, to);
            assert_eq!(node(h.state(), tab), node(h.state(), Tab::Timeline));
        }
        let timeline = node(h.state(), Tab::Timeline);
        assert_eq!(h.state().dock[timeline].tabs().unwrap().len(), 3);
        let leaf = h.state().dock.leaf(timeline).unwrap();
        assert!(
            leaf.rect.width() > 1200.0,
            "group must regain full timeline width"
        );

        // Drag Render Queue before Timeline, within the same group.
        let from = tab_rect(&h, Tab::RenderQueue).center();
        let to = tab_rect(&h, Tab::Timeline).left_center() + vec2(2.0, 0.0);
        drag(&mut h, from, to);
        let timeline = node(h.state(), Tab::Timeline);
        assert_eq!(
            h.state().dock[timeline].tabs().unwrap()[0],
            Tab::RenderQueue
        );

        // Clicking a tab switches content and routes editing shortcuts to it.
        h.hover_at(tab_rect(&h, Tab::Timeline).center());
        h.get_all_by_role(Role::Tab)
            .find(|n| n.accesskit_node().label().as_deref() == Some(Tab::Timeline.title()))
            .unwrap()
            .click();
        h.run();
        assert!(h.query_by_label("Razor Tool").is_some());
        h.state_mut()
            .timeline
            .seek(Time(jvid_core::time::FLICKS_PER_SECOND));
        h.key_press(Key::Home);
        h.run();
        assert_eq!(h.state().timeline.playhead(), Time(0));

        // Splitting onto the right edge uses the actual visible drop overlay.
        let from = tab_rect(&h, Tab::Transcript).center();
        let rect = h.state().dock.leaf(timeline).unwrap().rect;
        let to = egui::pos2(rect.right() - 20.0, rect.center().y);
        drag(&mut h, from, to);
        assert_ne!(
            node(h.state(), Tab::Transcript),
            node(h.state(), Tab::Timeline)
        );
        assert_eq!(h.state().dock.iter_all_tabs().count(), 6);
    }
}

#[test]
fn custom_groups_order_splits_and_active_tabs_survive_restart() {
    let mut app = App::restore(None);
    app.show_panel(Tab::Transcript);
    app.show_queue();
    group(&mut app, Tab::Transcript, Tab::Timeline, 0);
    group(&mut app, Tab::RenderQueue, Tab::Timeline, 1);
    app.focus_panel(Tab::Transcript);
    let media = node(&app, Tab::Media);
    let inspector = app.dock.find_tab(&Tab::Inspector).unwrap();
    app.dock.move_tab(
        inspector,
        TabDestination::Node(media, TabInsert::Split(Split::Below)),
    );
    // A non-default fraction is retained along with the custom split.
    let mut parent = node(&app, Tab::Media);
    parent.node = parent.node.parent().unwrap();
    if let egui_dock::Node::Horizontal(split) | egui_dock::Node::Vertical(split) =
        &mut app.dock[parent]
    {
        split.fraction = 0.37;
    } else {
        panic!("custom split is missing");
    }
    app.focus_panel(Tab::Transcript);
    // eframe persistence saves the entire tree, including fractions and focus.
    let storage = save(&mut app);
    let mut restored = App::restore(Some(&storage));
    let saved: DockState<Tab> = eframe::get_value(&storage, DOCK_KEY).unwrap();
    let mut actual = Memory::default();
    eframe::set_value(&mut actual, DOCK_KEY, &restored.dock);
    let mut expected = Memory::default();
    eframe::set_value(&mut expected, DOCK_KEY, &saved);
    assert_eq!(actual.0[DOCK_KEY], expected.0[DOCK_KEY]);
    let timeline = node(&restored, Tab::Timeline);
    assert_eq!(
        restored.dock[timeline].tabs().unwrap(),
        &[Tab::Transcript, Tab::RenderQueue, Tab::Timeline]
    );
    assert_eq!(
        restored.dock.find_active_focused().map(|(_, t)| *t),
        Some(Tab::Transcript)
    );
}

#[test]
fn show_and_focus_activate_inactive_tabs_without_changing_layout() {
    let mut app = App::restore(None);
    app.show_panel(Tab::Transcript);
    app.show_queue();
    group(&mut app, Tab::Transcript, Tab::Timeline, 1);
    group(&mut app, Tab::RenderQueue, Tab::Timeline, 2);
    let before = app.dock.iter_all_nodes().count();
    for tab in [
        Tab::Timeline,
        Tab::Transcript,
        Tab::RenderQueue,
        Tab::Timeline,
    ] {
        if tab == Tab::Timeline {
            app.run_action("focus_timeline", false);
        } else {
            app.show_panel(tab);
        }
        assert_eq!(app.dock.find_active_focused().map(|(_, t)| *t), Some(tab));
        assert_eq!(app.dock.iter_all_nodes().count(), before);
        assert_eq!(
            app.dock.iter_all_tabs().filter(|(_, t)| **t == tab).count(),
            1
        );
    }
    // The same guarantee applies to other core panels inside arbitrary groups.
    group(&mut app, Tab::Preview, Tab::Timeline, 0);
    app.run_action("focus_source", false);
    assert_eq!(
        app.dock.find_active_focused().map(|(_, t)| *t),
        Some(Tab::Preview)
    );
    assert_eq!(app.preview_active, 0);
}

#[test]
fn keyboard_tab_activation_and_show_actions_route_focus() {
    let mut h = ui_harness([1280.0, 720.0], 1.0);
    crate::playback::tests::add_solid(&mut h.state_mut().history, [0.2, 0.3, 0.4, 1.0], Time(0));
    h.state_mut().show_panel(Tab::Transcript);
    group(h.state_mut(), Tab::Transcript, Tab::Timeline, 1);
    h.state_mut().focus_panel(Tab::Timeline);
    h.run();
    h.get_all_by_role(Role::Tab)
        .find(|n| n.accesskit_node().label().as_deref() == Some(Tab::Transcript.title()))
        .unwrap()
        .focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    assert_eq!(
        h.state_mut().dock.find_active_focused().map(|(_, t)| *t),
        Some(Tab::Transcript)
    );
    assert!(
        !h.state().preview.program_playing(),
        "Space belongs to the focused dock tab"
    );
    // A show/focus action must release a Find field in another panel.
    h.get_all_by_role(Role::TextInput).next().unwrap().focus();
    h.run();
    assert!(h.ctx.text_edit_focused());
    h.state_mut().run_action("focus_timeline", false);
    h.run();
    assert!(!h.ctx.text_edit_focused());
    assert_eq!(
        h.state_mut().dock.find_active_focused().map(|(_, t)| *t),
        Some(Tab::Timeline)
    );
}

#[test]
fn timeline_menu_edits_apply_while_transcript_covers_timeline() {
    let mut h = ui_harness([1280.0, 720.0], 1.0);
    crate::playback::tests::add_solid(&mut h.state_mut().history, [0.2, 0.3, 0.4, 1.0], Time(0));
    h.state_mut().show_panel(Tab::Transcript);
    group(h.state_mut(), Tab::Transcript, Tab::Timeline, 1);
    h.state_mut().show_panel(Tab::Transcript);
    h.run();
    h.state_mut().run_action("select_all", true);
    h.state_mut().run_action("delete", true);
    h.run();
    assert!(
        h.state().history.project().sequence().tracks()[0]
            .clips()
            .is_empty()
    );
    assert_eq!(
        h.state_mut().dock.find_active_focused().map(|(_, t)| *t),
        Some(Tab::Transcript)
    );
    assert!(h.state_mut().history.undo());
    assert_eq!(
        h.state().history.project().sequence().tracks()[0]
            .clips()
            .len(),
        1
    );
}

#[derive(Clone, Default)]
struct DockAudio(std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<jvid_audio::Feed>>>>);

impl jvid_audio::Backend for DockAudio {
    fn devices(&self) -> Vec<String> {
        vec![]
    }
    fn open(
        &mut self,
        _: Option<&str>,
        feed: std::sync::Arc<jvid_audio::Feed>,
    ) -> Result<(String, Box<dyn std::any::Any>), String> {
        *self.0.lock().unwrap() = Some(feed);
        Ok(("dock-test".into(), Box::new(())))
    }
}

#[test]
fn playback_clock_continues_when_another_tab_covers_preview() {
    let mut h = ui_harness([1280.0, 720.0], 1.0);
    crate::playback::tests::add_solid(&mut h.state_mut().history, [0.2, 0.3, 0.4, 1.0], Time(0));
    let audio = DockAudio::default();
    h.state_mut()
        .preview
        .set_output(jvid_audio::Output::new(Box::new(audio.clone()), None));
    h.run();
    h.state_mut().show_panel(Tab::Transcript);
    group(h.state_mut(), Tab::Preview, Tab::Transcript, 0);
    h.state_mut().show_panel(Tab::Transcript);
    h.run();
    h.state_mut().preview.program_action("play_stop");
    for _ in 0..5 {
        h.run_steps(2);
        let feed = audio.0.lock().unwrap().clone().unwrap();
        feed.fill(&mut vec![0.0; 4800], 2);
    }
    h.run_steps(2);
    assert!(
        h.state().timeline.playhead() > Time(0),
        "hidden Preview must feed the audio clock"
    );
    assert!(h.state().preview.program_playing());
    h.state_mut().preview.program_action("shuttle_stop");
    h.run();
}

#[test]
fn standalone_transcript_keeps_its_title_and_close_action() {
    let mut fresh = true;
    let mut h = Harness::builder().with_size([320.0, 240.0]).build_ui_state(
        move |ui, (panel, closed): &mut (crate::transcript::TranscriptPanel, bool)| {
            if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                *closed |= panel
                    .show(ui, &Project::default(), &HashSet::new(), Time(0), true)
                    .close;
            }
        },
        (crate::transcript::TranscriptPanel::default(), false),
    );
    h.run();
    h.get_by_label("Close Transcript").click();
    h.run();
    assert!(h.state().1);
}

#[test]
fn close_reopen_reset_and_legacy_hidden_strips_are_predictable() {
    let mut app = App::restore(None);
    app.show_panel(Tab::Transcript);
    app.show_queue();
    group(&mut app, Tab::Transcript, Tab::Timeline, 1);
    group(&mut app, Tab::RenderQueue, Tab::Timeline, 2);
    app.close_panel(Tab::Transcript);
    app.show_panel(Tab::Transcript);
    assert_ne!(node(&app, Tab::Transcript), node(&app, Tab::Timeline));
    assert_eq!(node(&app, Tab::RenderQueue), node(&app, Tab::Timeline));
    app.transcript.query = "kept across reset".into();
    app.reset_layout();
    assert_eq!(app.transcript.query, "kept across reset");
    assert_eq!(app.dock.iter_all_tabs().count(), 6);
    assert_eq!(
        app.dock.find_active_focused().map(|(_, t)| *t),
        Some(Tab::Timeline)
    );
    for (_, n) in app.dock.iter_all_nodes_mut().filter(|(_, n)| n.is_leaf()) {
        n.set_tab_bar_hidden(true);
    }
    let restored = App::restore(Some(&save(&mut app)));
    assert!(restored.dock.iter_leaves().all(|(_, l)| !l.tab_bar_hidden));
    assert_eq!(restored.dock.iter_all_tabs().count(), 6);
}

#[test]
fn dock_tabs_close_optional_panels_and_toolbar_drag_does_not_move_timeline() {
    let mut h = ui_harness([960.0, 600.0], 1.5);
    h.state_mut().show_panel(Tab::Transcript);
    h.run();
    let r = tab_rect(&h, Tab::Transcript);
    // egui_dock's 24pt close target is separate from the title drag target.
    let close = r.right_center() - vec2(size::CONTROL / 2.0, 0.0);
    h.hover_at(close);
    h.drag_at(close);
    h.drop_at(close);
    h.run();
    assert!(h.state().dock.find_tab(&Tab::Transcript).is_none());
    h.state_mut().focus_panel(Tab::Timeline);
    h.run();
    let before = node(h.state(), Tab::Timeline);
    let tab = tab_rect(&h, Tab::Timeline);
    let tool = h.get_by_label("Razor Tool").rect();
    assert!(tool.top() >= tab.bottom());
    drag(&mut h, tool.center(), tool.center() + vec2(100.0, -60.0));
    assert_eq!(node(h.state(), Tab::Timeline), before);
    assert_eq!(h.state().dock.iter_all_tabs().count(), 4);
    // Reset is a real, translated menu action.
    h.get_by_label("Edit").click();
    h.run();
    h.get_by_label("Reset Panel Layout").click();
    h.run();
    h.key_press_modifiers(Modifiers::SHIFT, Key::Num3);
    h.run();
}

#[test]
fn show_scrolls_an_existing_tab_into_a_narrow_strip_at_common_scales() {
    for scale in SCALES {
        let mut h = ui_harness([960.0, 600.0], scale);
        let mut dock = DockState::new(vec![Tab::Media]);
        dock.main_surface_mut().split_right(
            NodeIndex::root(),
            0.74,
            vec![
                Tab::Preview,
                Tab::Inspector,
                Tab::Timeline,
                Tab::Transcript,
                Tab::RenderQueue,
            ],
        );
        h.state_mut().dock = dock;
        h.run();
        for tab in [
            Tab::RenderQueue,
            Tab::Preview,
            Tab::Transcript,
            Tab::Timeline,
        ] {
            h.state_mut().focus_panel(tab);
            h.run();
            let strip = h
                .state()
                .dock
                .leaf(node(h.state(), tab))
                .unwrap()
                .rect
                .shrink(space::S4);
            let r = tab_rect(&h, tab);
            assert!(
                r.left() >= strip.left() - 1.0 && r.right() <= strip.right() + 1.0,
                "{tab:?} at {scale}x: {r:?} outside {strip:?}"
            );
            assert!(r.width() >= size::TAB_MIN && r.width() <= size::TAB_MAX + 1.0);
        }
        // The 240pt Timeline still exposes every tool, including overflow at smaller sizes.
        let tool = h.get_by_label("Razor Tool").rect();
        let leaf = h.state().dock.leaf(node(h.state(), Tab::Timeline)).unwrap();
        assert!(tool.left() >= leaf.rect.left() && tool.right() <= leaf.rect.right());
    }
}

#[test]
fn grouped_panels_and_split_drop_feedback_snapshots() {
    if !gpu() {
        return;
    }
    let mut results = egui_kittest::SnapshotResults::new();
    for scale in SCALES {
        let mut h = harness([960.0, 600.0], scale);
        h.state_mut().show_panel(Tab::Transcript);
        h.state_mut().show_queue();
        group(h.state_mut(), Tab::Transcript, Tab::Timeline, 1);
        group(h.state_mut(), Tab::RenderQueue, Tab::Timeline, 2);
        h.state_mut().focus_panel(Tab::Timeline);
        h.run();
        results.add(h.try_snapshot(format!("dock_group_timeline_{scale}x")));
        h.state_mut().show_panel(Tab::Transcript);
        h.run();
        results.add(h.try_snapshot(format!("dock_group_transcript_{scale}x")));
        h.state_mut().show_queue();
        h.run();
        results.add(h.try_snapshot(format!("dock_group_queue_{scale}x")));
        let from = tab_rect(&h, Tab::Transcript).center();
        let rect = h
            .state()
            .dock
            .leaf(node(h.state(), Tab::Timeline))
            .unwrap()
            .rect;
        let to = egui::pos2(rect.right() - 20.0, rect.center().y);
        h.hover_at(from);
        h.run_steps(2);
        h.drag_at(from);
        h.run_steps(2);
        for n in 1..=12 {
            h.hover_at(from + (to - from) * (n as f32 / 12.0));
            h.run_steps(2);
        }
        results.add(h.try_snapshot(format!("dock_split_drop_{scale}x")));
        h.drop_at(to);
        h.run();
    }
}
