use super::*;
use egui::{Pos2, pos2, vec2};

fn editing_harness() -> Harness<'static, App> {
    let mut h = plain_harness();
    h.state_mut().new_project(None);
    let app = h.state_mut();
    for (kind, name) in [(TrackKind::Video, "V1"), (TrackKind::Audio, "A1")] {
        app.history
            .apply(Command::AddTrack {
                kind,
                name: name.into(),
            })
            .unwrap();
    }
    app.history = History::new(app.history.project().clone());
    h.run();
    h
}

fn pointer_drag(h: &mut Harness<'_, App>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run();
    h.drag_at(from);
    h.run();
    h.hover_at(from + (to - from) * 0.5);
    h.run();
    h.hover_at(to);
    h.run();
    h.drop_at(to);
    h.run();
}

#[test]
fn two_volume_drags_are_two_undo_steps() {
    let mut h = editing_harness();
    let r = h.get_by_label("Volume").rect();
    let volume = |h: &Harness<'_, App>| h.state().history.project().sequence().tracks()[1].volume;
    pointer_drag(
        &mut h,
        r.center(),
        pos2(r.left() + r.width() * 0.3, r.center().y),
    );
    let first = volume(&h);
    assert!(first < 0.8, "first drag changed volume: {first}");
    pointer_drag(
        &mut h,
        r.center(),
        pos2(r.left() + r.width() * 0.7, r.center().y),
    );
    assert!(volume(&h) > 1.2);
    assert!(h.state_mut().history.undo());
    assert_eq!(volume(&h), first, "undo only the second drag");
    assert!(h.state_mut().history.undo());
    assert_eq!(volume(&h), 1.0);
    assert!(!h.state().history.can_undo());
}

#[test]
fn resizing_two_tracks_is_two_undo_steps() {
    let mut h = editing_harness();
    let header = h.get_all_by_label("Target").next().unwrap().rect();
    // Target is 20pt centred 16pt below the lane top; the height grip is at 40pt.
    let from = pos2(header.center().x + 30.0, header.center().y - 16.0 + 40.0);
    pointer_drag(&mut h, from, from + vec2(0.0, 20.0));
    let first = h.state().history.project().clone();
    assert_ne!(first.sequence().tracks()[0].height, 40.0);
    let a = h.get_all_by_label("Target").nth(1).unwrap().rect();
    let from = pos2(a.center().x + 30.0, a.center().y - 16.0 + 40.0);
    pointer_drag(&mut h, from, from + vec2(0.0, 20.0));
    assert_ne!(
        h.state().history.project().sequence().tracks()[1].height,
        40.0
    );
    assert!(h.state_mut().history.undo());
    assert_eq!(h.state().history.project(), &first);
    assert!(h.state_mut().history.undo());
    assert!(
        h.state()
            .history
            .project()
            .sequence()
            .tracks()
            .iter()
            .all(|t| t.height == 40.0)
    );
    assert!(!h.state().history.can_undo());
}

#[test]
fn save_between_volume_drags_undoes_to_clean_saved_value() {
    let mut h = editing_harness();
    let r = h.get_by_label("Volume").rect();
    pointer_drag(
        &mut h,
        r.center(),
        pos2(r.left() + r.width() * 0.3, r.center().y),
    );
    let saved = h.state().history.project().clone();
    let dir = std::env::temp_dir().join(format!("jvid-b04-gesture-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert!(h.state_mut().save_to(dir.join("saved.jvid")));
    pointer_drag(
        &mut h,
        r.center(),
        pos2(r.left() + r.width() * 0.7, r.center().y),
    );
    assert!(h.state().history.is_dirty());
    assert!(h.state_mut().history.undo());
    assert_eq!(h.state().history.project(), &saved);
    assert!(!h.state().history.is_dirty());
    std::fs::remove_dir_all(dir).unwrap();
}
