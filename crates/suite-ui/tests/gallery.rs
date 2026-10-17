//! Renders the gallery example (the PR screenshot) and checks the radius scale.

#[allow(dead_code)]
#[path = "../examples/gallery.rs"]
mod gallery;

mod common;

use suite_ui::tokens::radius;

#[test]
fn gallery_snapshot() {
    if !common::gpu() {
        return;
    }
    let mut state = gallery::Gallery::new();
    let mut harness = egui_kittest::Harness::builder()
        .with_size([1280.0, 860.0])
        .wgpu()
        .with_context_setup(suite_ui::theme::install)
        .build_ui(|ui| gallery::gallery(ui, &mut state));
    // The loading spinners never settle, so step a fixed number of frames.
    harness.run_steps(4);
    harness.snapshot("gallery");
}

#[test]
fn corners_come_from_the_scale() {
    let sources = [
        include_str!("../src/panel.rs"),
        include_str!("../src/theme.rs"),
        include_str!("../src/widgets.rs"),
    ];
    for source in sources {
        for call in source.split("CornerRadius::same(").skip(1) {
            assert!(
                call.starts_with("radius::") || call.starts_with("corner"),
                "corner not from the scale: {}",
                &call[..30]
            );
        }
    }
    // The focus ring sits 2px outside its shape's corner.
    for r in [radius::SMALL, radius::CONTROL] {
        assert!(radius::SCALE.contains(&(r + 2)));
    }
}
