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
