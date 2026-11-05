//! #418: a short Dialog hugs its content in a tall window, even after a taller one
//! (all Dialogs share one Area, which remembers its size).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use suite_ui::dialog::Dialog;
use suite_ui::test_support as common;
use suite_ui::widgets::ButtonKind;

#[test]
fn short_dialog_hugs_content() {
    let mut fresh = true;
    let mut frame = 0;
    let mut harness = Harness::builder()
        .with_size([1920.0, 1080.0])
        .build_ui(move |ui| {
            if common::install_theme(ui, &mut fresh) {
                return;
            }
            frame += 1;
            // A few frames of the long Notices dialog, then a short one.
            let long = frame < 5;
            Dialog {
                title: "Unsaved Changes",
                body: if long {
                    "x\n".repeat(200).leak()
                } else {
                    "Save changes before closing?"
                },
                buttons: &[
                    ("Save", ButtonKind::Primary),
                    ("Cancel", ButtonKind::Secondary),
                ],
                cancel: 1,
                long,
            }
            .show(ui.ctx());
        });
    for _ in 0..10 {
        harness.step();
    }
    let rect = harness
        .ctx
        .memory(|m| m.area_rect(egui::Id::new("suite_ui_dialog")))
        .expect("dialog area");
    assert!(rect.height() < 200.0, "dialog is {} px high", rect.height());
    let body = harness.get_by_label("Save changes before closing?").rect();
    let save = harness.get_by_label("Save").rect();
    assert!(
        save.top() - body.bottom() < 40.0,
        "body {body:?}, button {save:?}"
    );
}

#[test]
fn switching_to_a_long_dialog_settles_the_footer_and_preserves_dismissal() {
    for key in [
        None,
        Some(egui::Key::Enter),
        Some(egui::Key::Space),
        Some(egui::Key::Escape),
    ] {
        let mut fresh = true;
        let notices = "Third-party licence text.\n".repeat(200);
        let mut h = Harness::builder()
            .with_size([1280.0, 720.0])
            .build_ui_state(
                move |ui, stage: &mut usize| {
                    if common::install_theme(ui, &mut fresh) || *stage == 2 {
                        return;
                    }
                    let long = *stage == 1;
                    let picked = Dialog {
                        title: if long { "Notices" } else { "About" },
                        body: if long { &notices } else { "Version 1" },
                        buttons: if long {
                            &[("OK", ButtonKind::Primary)]
                        } else {
                            &[
                                ("OK", ButtonKind::Primary),
                                ("Third-Party Notices", ButtonKind::Secondary),
                            ]
                        },
                        cancel: 0,
                        long,
                    }
                    .show(ui.ctx());
                    if picked.is_some() {
                        *stage += 1;
                    }
                },
                0,
            );
        h.run();
        h.get_by_label("Third-Party Notices").click();
        h.run();
        assert_eq!(*h.state(), 1);
        let settled = h.get_by_label("OK").rect();
        h.get_by_label("OK").hover();
        h.run();
        assert_eq!(h.get_by_label("OK").rect(), settled);
        if let Some(key) = key {
            h.key_press(key);
        } else {
            h.get_by_label("OK").click();
        }
        h.run();
        assert_eq!(*h.state(), 2, "dismissal with {key:?}");
    }
}

#[test]
fn footer_fits_content_aligns_right_and_wraps_in_reading_order() {
    use suite_ui::dialog::action_footer;
    use suite_ui::widgets::Button;

    for scale in [1.0, 1.5, 2.0] {
        for width in [160.0, 240.0, 460.0] {
            let mut fresh = true;
            let mut h = Harness::builder()
                .with_size([640.0, 900.0])
                .with_pixels_per_point(scale)
                .build_ui_state(
                    move |ui, rects: &mut Vec<egui::Rect>| {
                        if common::install_theme(ui, &mut fresh) {
                            return;
                        }
                        egui::Modal::new(egui::Id::new("footer_test")).show(ui.ctx(), |ui| {
                            ui.set_width(width);
                            ui.label("Body");
                            *rects = action_footer(
                                ui,
                                [
                                    Button::secondary("Cancel"),
                                    Button::secondary("Save a Copy with a Longer Translated Label"),
                                    Button::primary("Done"),
                                ],
                            )
                            .into_iter()
                            .map(|r| r.rect)
                            .collect();
                        });
                    },
                    Vec::new(),
                );
            h.run();
            let rects = h.state();
            let body = h.get_by_label("Body").rect();
            assert!(
                rects[0].top() - body.bottom() < 12.0,
                "footer claimed spare height: {rects:?}"
            );
            for rect in rects {
                assert!(rect.left() >= body.left() - 1.0, "left overflow: {rect:?}");
                assert!(
                    rect.right() <= body.left() + width + 1.0,
                    "right overflow: {rect:?}"
                );
                assert!(rect.height() >= 28.0 - 1.0 / scale);
            }
            let done = rects[2];
            assert!(
                (done.right() - body.left() - width).abs() <= 1.0,
                "primary not right aligned: {done:?}"
            );
            for pair in rects.windows(2) {
                assert!(pair[1].top() >= pair[0].top() - 1.0);
                if (pair[1].top() - pair[0].top()).abs() <= 1.0 {
                    assert!(pair[0].right() <= pair[1].left());
                } else {
                    assert!(pair[0].bottom() <= pair[1].top());
                }
            }
            if width == 160.0 {
                assert!(rects[1].height() > 28.0);
            }
        }
    }
}

#[test]
fn footer_keyboard_order_and_confirm_default_are_preserved() {
    use suite_ui::dialog::action_footer;
    use suite_ui::widgets::Button;
    let mut fresh = true;
    let mut h = Harness::builder().build_ui_state(
        move |ui, ids: &mut Vec<egui::Id>| {
            if common::install_theme(ui, &mut fresh) {
                return;
            }
            *ids = action_footer(
                ui,
                [
                    Button::secondary("Cancel"),
                    Button::secondary("Back"),
                    Button::primary("Done"),
                ],
            )
            .into_iter()
            .map(|r| r.id)
            .collect();
        },
        Vec::new(),
    );
    h.run();
    let ids = h.state().clone();
    h.ctx.memory_mut(|m| m.request_focus(ids[0]));
    h.key_press(egui::Key::Tab);
    h.run();
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(ids[1]));
    h.key_press(egui::Key::Tab);
    h.run();
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(ids[2]));

    let mut fresh = true;
    let mut h = Harness::builder().build_ui_state(
        move |ui, picked: &mut Option<usize>| {
            if common::install_theme(ui, &mut fresh) {
                return;
            }
            if let Some(i) = (Dialog {
                title: "Confirm",
                body: "Save changes?",
                buttons: &[
                    ("Save", ButtonKind::Primary),
                    ("Discard", ButtonKind::Destructive),
                    ("Cancel", ButtonKind::Secondary),
                ],
                cancel: 2,
                long: false,
            })
            .show(ui.ctx())
            {
                *picked = Some(i);
            }
        },
        None,
    );
    h.run();
    h.key_press(egui::Key::Enter);
    h.run();
    assert_eq!(*h.state(), Some(0));
    h.key_press(egui::Key::Escape);
    h.run();
    assert_eq!(*h.state(), Some(2));
}

#[test]
fn wrapping_a_footer_preserves_keyboard_focus() {
    use suite_ui::dialog::action_footer;
    use suite_ui::widgets::Button;
    let mut fresh = true;
    let mut h = Harness::builder().build_ui_state(
        move |ui, (width, ids): &mut (f32, Vec<egui::Id>)| {
            if common::install_theme(ui, &mut fresh) {
                return;
            }
            ui.set_width(*width);
            *ids = action_footer(
                ui,
                [
                    Button::secondary("Cancel"),
                    Button::secondary("Save a Copy"),
                    Button::primary("Done"),
                ],
            )
            .into_iter()
            .map(|r| r.id)
            .collect();
        },
        (460.0, Vec::new()),
    );
    h.run();
    let before = h.state().1.clone();
    h.ctx.memory_mut(|m| m.request_focus(before[2]));
    h.state_mut().0 = 160.0;
    h.run();
    assert_eq!(h.state().1, before);
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(before[2]));
}

#[test]
fn measuring_a_footer_does_not_duplicate_accessible_actions() {
    use suite_ui::dialog::action_footer;
    use suite_ui::widgets::Button;
    let mut fresh = true;
    let mut h = Harness::builder().build_ui(move |ui| {
        if common::install_theme(ui, &mut fresh) {
            return;
        }
        let mut measure = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("measure_footer")
                .invisible()
                .sizing_pass(),
        );
        action_footer(
            &mut measure,
            [Button::secondary("Cancel"), Button::primary("Done")],
        );
        action_footer(ui, [Button::secondary("Cancel"), Button::primary("Done")]);
    });
    h.run();
    h.get_by_label("Cancel");
    h.get_by_label("Done");
}
