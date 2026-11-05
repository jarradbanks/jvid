//! Quiet Centre geometry, overflow and accessible interaction independent of GPU snapshots.
use egui::{Key, Rect, Sense, UiBuilder, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use suite_ui::empty_state::EmptyState;
use suite_ui::test_support::install_theme;
use suite_ui::theme::icon;

struct State {
    body: Rect,
    content: Rect,
    action: Rect,
    clicks: usize,
    background_clicked: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            body: Rect::NOTHING,
            content: Rect::NOTHING,
            action: Rect::NOTHING,
            clicks: 0,
            background_clicked: false,
        }
    }
}

fn harness(size: [f32; 2], action: &'static str) -> Harness<'static, State> {
    let mut fresh = true;
    Harness::builder().with_size(size).build_ui_state(
        move |ui, state: &mut State| {
            if install_theme(ui, &mut fresh) {
                return;
            }
            // A real body between a header and a pinned footer.
            let outer = ui.available_rect_before_wrap();
            let body = Rect::from_min_max(outer.min + vec2(0.0, 32.0), outer.max - vec2(0.0, 32.0));
            state.body = body;
            state.background_clicked |= ui
                .interact(
                    body,
                    ui.id().with("drop_body"),
                    Sense::click_and_drag() - Sense::FOCUSABLE,
                )
                .clicked();
            ui.scope_builder(UiBuilder::new().max_rect(body), |ui| {
                let r = EmptyState::new(
                    icon::FOLDER,
                    "No Media Yet",
                    "Import video, audio, or images to start editing.",
                )
                .primary_action(action)
                .show(ui);
                state.content = r.content_rect;
                let r = r.action.unwrap();
                state.action = r.rect;
                state.clicks += usize::from(r.clicked());
            });
        },
        State::default(),
    )
}

#[test]
fn centres_the_whole_group_below_header_and_above_footer_at_240px() {
    for size in [[240.0, 360.0], [440.0, 560.0]] {
        let mut h = harness(size, "Import Media…");
        h.run();
        let s = h.state();
        assert!(s.body.contains_rect(s.content));
        assert!(
            (s.body.center() - s.content.center()).length() < 1.0,
            "{s:?}",
            s = (s.body, s.content)
        );
        let heading = h.get_by_label("No Media Yet").rect();
        let help = h
            .get_by_label("Import video, audio, or images to start editing.")
            .rect();
        assert!(heading.bottom() < help.top());
        assert!(help.bottom() < s.action.top());
        assert!(s.body.contains_rect(help));
        assert!((heading.center().x - s.body.center().x).abs() < 1.0);
        assert!((help.center().x - s.body.center().x).abs() < 1.0);
        h.get_by_label("Import Media…").click();
        h.run();
        assert_eq!(h.state().clicks, 1);
        assert!(!h.state().background_clicked);
        // The text must not steal click/drag handling from an underlying editor/drop body.
        let at = h.get_by_label("No Media Yet").rect().center();
        h.drag_at(at);
        h.run();
        h.drop_at(at);
        h.run();
        assert!(h.state().background_clicked);
    }
}

#[test]
fn short_body_scrolls_to_a_keyboard_action() {
    let mut h = harness([240.0, 150.0], "Import Media…");
    h.run();
    assert!(h.state().content.height() > h.state().body.height());
    h.key_press(Key::Tab);
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(
        h.state().clicks,
        1,
        "action must be reachable when initially below the viewport"
    );
}

#[test]
fn long_localized_action_can_scroll_without_expanding_the_panel() {
    let action = "Ausgewählte Mediendateien in das aktuelle Projekt importieren…";
    let mut h = harness([240.0, 150.0], action);
    h.run();
    assert!(h.state().action.width() > h.state().body.width());
    assert!(h.state().body.width() <= 240.0);
    let heading = h.get_by_label("No Media Yet").rect();
    assert!((heading.center().x - h.state().body.center().x).abs() < 1.0);
    h.key_press(Key::Tab);
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().clicks, 1);
}

#[test]
fn unbounded_parent_uses_the_finite_visible_body() {
    let mut fresh = true;
    let mut h = Harness::builder()
        .with_size([240.0, 180.0])
        .build_ui(move |ui| {
            if install_theme(ui, &mut fresh) {
                return;
            }
            let clip = ui.clip_rect();
            let rect = Rect::from_min_max(clip.min, pos2(f32::INFINITY, f32::INFINITY));
            ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                let r = EmptyState::new(
                    icon::POINTER,
                    "Select a Clip",
                    "Select a clip in the timeline to edit its properties.",
                )
                .show(ui);
                assert!(r.content_rect.is_finite());
                assert!(clip.contains_rect(r.content_rect));
                assert!((r.content_rect.center() - clip.center()).length() < 1.0);
            });
        });
    h.run();
    assert!(h.query_all_by_label("Select a Clip").next().is_some());
}

#[test]
fn readable_theme_inks_and_type_sizes_keep_the_viewer_neutral() {
    use suite_ui::themes;
    for palette in [themes::DARK, themes::LIGHT, themes::HIGH_CONTRAST] {
        for viewer in [false, true] {
            let mut fresh = true;
            let mut h = Harness::builder()
                .with_size([240.0, 280.0])
                .build_ui(move |ui| {
                    if install_theme(ui, &mut fresh) {
                        return;
                    }
                    themes::apply(ui.ctx(), &palette);
                    let empty = EmptyState::new(
                        icon::PLAY,
                        "Nothing to Preview",
                        "Add a video clip to the timeline to see it here.",
                    );
                    if viewer { empty.viewer() } else { empty }.show(ui);
                });
            h.run();
            let palette = if viewer { themes::DARK } else { palette };
            for (label, size, ink) in [
                ("Nothing to Preview", 14.0, palette.ink),
                (
                    "Add a video clip to the timeline to see it here.",
                    12.0,
                    palette.ink_3,
                ),
            ] {
                let text = h
                    .output()
                    .shapes
                    .iter()
                    .find_map(|s| match &s.shape {
                        egui::Shape::Text(t) if t.galley.job.text == label => Some(t),
                        _ => None,
                    })
                    .expect("visible text shape");
                assert!(
                    text.galley
                        .job
                        .sections
                        .iter()
                        .all(|s| s.format.font_id.size == size && s.format.color == ink)
                );
                assert!(
                    h.query_by_label(label).is_some(),
                    "text is exposed to accessibility"
                );
            }
        }
    }
}
