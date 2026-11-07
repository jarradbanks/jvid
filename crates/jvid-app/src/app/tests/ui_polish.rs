//! Review captures for the UI polish pass: the Bins view, the passive playback status,
//! View Selection in the preview toolbar and a title typed at a playhead past the end.
//! `JVID_UI_POLISH_CAPTURES=docs/screenshots/ui-polish cargo test -p jvid-app ui_polish -- --ignored`

use egui::{Key, Pos2};
use jvid_core::media_library::LibraryEdit;
use jvid_core::model::AssetId;
use jvid_core::time::FLICKS_PER_SECOND as S;

use super::*;

fn save(h: &mut Harness<'_, App>, dir: &std::path::Path, name: &str) {
    h.run_steps(4);
    h.render().unwrap().save(dir.join(name)).unwrap();
}

fn click(h: &mut Harness<'_, App>, at: Pos2) {
    h.hover_at(at);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
    }
    h.run();
}

#[test]
#[ignore = "writes review captures; run with JVID_UI_POLISH_CAPTURES set"]
fn ui_polish_captures() {
    let Some(dir) = std::env::var_os("JVID_UI_POLISH_CAPTURES").map(PathBuf::from) else {
        return;
    };
    if !gpu() {
        return;
    }
    std::fs::create_dir_all(&dir).unwrap();

    // Item 2: bins.
    let mut h = bin_harness();
    for name in ["Selects", "B-Roll"] {
        let edit = LibraryEdit::CreateBin { name: name.into() };
        h.state_mut()
            .history
            .apply(Command::EditMediaLibrary(edit))
            .unwrap();
    }
    let file = LibraryEdit::MoveAssets {
        ids: vec![AssetId(1), AssetId(3)],
        bin: Some(jvid_core::media_library::BinId(1)),
    };
    h.state_mut()
        .history
        .apply(Command::EditMediaLibrary(file))
        .unwrap();
    save(&mut h, &dir, "1-bins-all-media.png");
    h.get_by_label("Bins").click();
    save(&mut h, &dir, "2-bins-view-folder-tiles.png");
    let media = h.get_by_label("clutch_round_3.mkv").rect().center();
    let tile = h.get_by_label("B-Roll").rect().center();
    h.drag_at(media);
    h.run();
    for step in 1..=4 {
        h.hover_at(media.lerp(tile, step as f32 / 4.0));
        h.run();
    }
    save(&mut h, &dir, "3-bins-drag-media-onto-tile.png");
    h.drop_at(tile);
    h.run();
    h.get_by_label("New Bin").click();
    h.run();
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "New Bin")
        .type_text("Highlights");
    save(&mut h, &dir, "4-bins-new-bin-inline-name.png");
    h.key_press(Key::Enter);
    h.run();
    h.get_by_label("Selects").click_secondary();
    save(&mut h, &dir, "5-bins-right-click-rename-delete.png");
    h.get_by_label("Delete Bin").click();
    save(&mut h, &dir, "6-bins-delete-confirmation.png");
    h.get_by_label("Cancel").click();
    h.run();
    let at = h.get_by_label("Selects").rect().center();
    h.input_mut().time = Some(h.ctx.input(|i| i.time) + 1.0);
    h.hover_at(at);
    h.step();
    for pressed in [true, false, true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        h.step();
    }
    save(&mut h, &dir, "7-bins-inside-bin-breadcrumb.png");

    // Items 3 to 5 on a wide editor, so the preview row shows its view controls.
    let mut h = harness([1920.0, 1080.0], 1.0);
    crate::playback::tests::add_solid(&mut h.state_mut().history, [0.2, 0.3, 0.4, 1.0], Time(0));
    let rate = h.state().history.project().settings().frame_rate;
    h.state_mut()
        .timeline
        .seek(rate.time_floor(rate.index_floor(Time(3 * S))));
    h.run_steps(4);
    save(&mut h, &dir, "8-preview-proxies-off-no-status.png");
    h.get_by_label(tr!("preview.proxies")).click();
    h.run();
    h.get_by_label(tr!("preview.status_original")).hover();
    save(&mut h, &dir, "9-preview-proxies-on-passive-status.png");
    h.get_by_label(tr!("preview.proxies")).click();
    h.run();
    h.state_mut().run_action("focus_program", false);
    h.key_press(Key::T);
    h.run();
    let canvas = h
        .get_all_by_label("Text Tool")
        .find(|n| n.rect().width() > 32.0)
        .unwrap()
        .rect();
    click(&mut h, canvas.lerp_inside(egui::vec2(0.7, 0.75)));
    h.key_press(Key::End);
    h.event(egui::Event::Text(" at 3s".into()));
    save(
        &mut h,
        &dir,
        "10-text-tool-title-shows-at-playhead-past-end.png",
    );
    // Esc ends typing, then leaves the Text tool so the transform handles show.
    h.key_press(Key::Escape);
    h.run();
    h.key_press(Key::Escape);
    h.run();
    let button =
        h.get_by_role_and_label(egui::accesskit::Role::Button, tr!("preview.view_selection"));
    button.hover();
    save(&mut h, &dir, "11-view-selection-in-toolbar.png");
    h.get_by_role_and_label(egui::accesskit::Role::Button, tr!("preview.view_selection"))
        .click();
    save(&mut h, &dir, "12-view-selection-zoomed-to-title.png");
}
