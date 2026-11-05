//! Real timeline hit testing and history, without requiring a GPU renderer.

use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn harness(project: Project) -> Harness<'static, (Timeline, History)> {
    let mut fresh = true;
    let mut h = Harness::builder()
        .with_size([1000.0, 240.0])
        .build_ui_state(
            move |ui, (t, h): &mut (Timeline, History)| {
                if suite_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                if let Some((cmd, _)) = t.show(ui, h.project(), &HashSet::new(), &Keymap::default())
                {
                    h.apply(cmd).unwrap();
                }
            },
            (Timeline::default(), History::new(project)),
        );
    h.run();
    h
}

fn begin(h: &mut Harness<'_, (Timeline, History)>, at: Pos2) {
    // Begin outside the timeline so this synthetic bin drag does not select/move a clip.
    let from = pos2(20.0, 16.0);
    h.hover_at(from);
    h.run();
    h.drag_at(from);
    h.run();
    egui::DragAndDrop::set_payload(&h.ctx, crate::effects::Drag("vignette".into()));
    h.hover_at(at);
    h.run();
}

fn target_rect(h: &Harness<'_, (Timeline, History)>) -> bool {
    h.output().shapes.iter().any(|s| {
        matches!(&s.shape,
        egui::Shape::Rect(r) if r.stroke.width == 3.0 && r.stroke.color == color::light_blue())
    })
}

#[test]
fn timeline_effect_drop_highlights_exact_clip_preserves_selection_and_undoes_once() {
    let dir = tempfile::tempdir().unwrap();
    crate::effects::scan_for_test(dir.path());
    let mut h = harness(super::tests::project(4));
    let ids: Vec<_> = h.state().1.project().sequence().tracks()[0]
        .clips()
        .iter()
        .map(|c| c.id)
        .collect();
    h.state_mut().0.selected = ids.iter().copied().collect();
    let selected = h.state().0.selected.clone();
    let before = h.state().1.project().clone();
    let y = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
    let at = pos2(size::TRACK_HEADER + 36.0, y);
    begin(&mut h, at);
    assert!(
        target_rect(&h),
        "the destination must be outlined during hover"
    );
    assert_eq!(h.state().1.project(), &before);
    h.drop_at(at);
    h.run();
    let seq = h.state().1.project().sequence();
    assert_eq!(seq.clip(ids[0]).unwrap().1.transform.effects.len(), 1);
    assert!(seq.clip(ids[1]).unwrap().1.transform.effects.is_empty());
    assert_eq!(h.state().0.selected, selected);
    assert!(!target_rect(&h));
    assert!(h.state_mut().1.undo());
    assert_eq!(h.state().1.project(), &before);
    assert!(h.state_mut().1.redo());
    assert_eq!(
        h.state()
            .1
            .project()
            .sequence()
            .clip(ids[0])
            .unwrap()
            .1
            .transform
            .effects
            .len(),
        1
    );
}

#[test]
fn timeline_invalid_drop_and_cancellation_clear_highlights_and_leave_history_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    crate::effects::scan_for_test(dir.path());
    for mode in 0..5 {
        let mut h = harness(super::tests::project(4));
        let seq = h.state().1.project().sequence();
        let track = seq.tracks()[0].id;
        if mode == 3 || mode == 4 {
            let mut settings = seq.track(track).unwrap().settings();
            settings.hidden = mode == 3;
            settings.locked = mode == 4;
            h.state_mut().1.apply(Command::SetTrack(settings)).unwrap();
            h.run();
        }
        let before = h.state().1.project().clone();
        let ys: Vec<_> = h
            .get_all_by_label("Lock")
            .map(|n| n.rect().center().y)
            .collect();
        let valid = pos2(size::TRACK_HEADER + 36.0, ys[0]);
        begin(&mut h, valid);
        if mode < 3 {
            assert!(target_rect(&h));
        }
        let at = match mode {
            0 => pos2(900.0, ys[0]),   // empty lane
            1 => pos2(valid.x, ys[1]), // audio
            _ => valid,
        };
        if mode == 2 {
            h.key_press(Key::Escape);
        }
        h.hover_at(at);
        h.run();
        assert!(!target_rect(&h));
        h.drop_at(at);
        h.run();
        assert_eq!(h.state().1.project(), &before);
        assert!(!egui::DragAndDrop::has_any_payload(&h.ctx));
        assert!(!target_rect(&h));
    }
}
