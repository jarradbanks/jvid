//! Whole-source framing presets, using the Inspector's transform/keyframe command path.

use egui::{Rect, Sense, Ui};
use jvid_core::framing::{Framing, image_size};
use jvid_core::history::Command;
use jvid_core::model::{ClipId, Project, TrackKind};
use jvid_core::time::Time;

use crate::i18n::tr;
use crate::timeline::Timeline;

pub const ACTIONS: [&str; 3] = ["fit_to_frame", "fill_frame", "stretch_to_fit"];
const PROPS: &[&str] = &[
    "position",
    "anchor",
    "scale",
    "rotation",
    "crop_left",
    "crop_top",
    "crop_right",
    "crop_bottom",
];

pub fn mode(id: &str) -> Option<Framing> {
    match id {
        "fit_to_frame" => Some(Framing::Fit),
        "fill_frame" => Some(Framing::Fill),
        "stretch_to_fit" => Some(Framing::Stretch),
        _ => None,
    }
}

/// One undo step for all eligible clips. Spatial properties reset at the playhead using
/// the Inspector's grouped-key rules; grade, masks, opacity, audio and effects stay intact.
pub fn command(project: &Project, ids: &[ClipId], mode: Framing, t: Time) -> Option<Command> {
    let s = project.settings();
    let frame = [s.width as f64, s.height as f64];
    let mut edits = Vec::new();
    for id in ids {
        let Some((track, c)) = project.sequence().clip(*id) else {
            continue;
        };
        let Some(track) = project.sequence().track(track) else {
            continue;
        };
        if track.kind != TrackKind::Video || track.locked {
            continue;
        }
        let Some(x) = image_size(project, c).and_then(|size| mode.transform(size, frame)) else {
            continue;
        };
        if let Command::SetClipProps(props) = crate::inspector::set_props(&[c], PROPS, &x, t) {
            edits.extend(props);
        }
    }
    (!edits.is_empty()).then_some(Command::SetClipProps(edits))
}

/// The same rows in both context menus. `id` always names the clicked clip, even when
/// other clips (including linked clips) are selected.
pub fn menu(ui: &mut Ui, project: &Project, id: Option<ClipId>, t: Time) -> Option<Command> {
    if let Some((track, c)) = id.and_then(|id| project.sequence().clip(id))
        && let Some(track) = project.sequence().track(track)
        && let Some(asset) = project.asset(c.asset)
    {
        ui.label(tr!(
            "framing.target",
            track = track.name,
            clip = asset.display_name()
        ));
        ui.separator();
    }
    let mut edit = None;
    for (action, label) in ACTIONS.into_iter().zip([
        tr!("action.fit_to_frame"),
        tr!("action.fill_frame"),
        tr!("action.stretch_to_fit"),
    ]) {
        let mode = mode(action).expect("framing action");
        let cmd = id.and_then(|id| command(project, &[id], mode, t));
        if ui
            .add_enabled(cmd.is_some(), egui::Button::new(label))
            .clicked()
        {
            edit = cmd;
            ui.close();
        }
    }
    edit
}

/// Program right-clicks hit the top visible layer's transformed quad, including crop and
/// rotation. Store the target when opening: moving the pointer into the menu cannot retarget it.
pub fn program_menu(
    ui: &mut Ui,
    project: &Project,
    timeline: &mut Timeline,
    frame: Rect,
) -> Option<Command> {
    let t = timeline.playhead();
    let id = ui.id().with("program_framing");
    let response = ui.interact(crate::preview::viewer_rect(ui), id, Sense::click());
    if response.secondary_clicked() {
        let clip = response
            .interact_pointer_pos()
            .filter(|p| frame.contains(*p))
            .and_then(|p| crate::handles::clip_at(project, t, frame, p));
        ui.data_mut(|d| d.insert_temp(id, clip));
        if let Some(clip) = clip {
            timeline.select_program_clip(clip);
        }
    }
    let target: Option<ClipId> = ui.data(|d| d.get_temp(id)).flatten();
    let mut edit = None;
    response.context_menu(|ui| edit = menu(ui, project, target, t));
    edit
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashSet;
    use std::path::Path;

    use egui::{Pos2, pos2, vec2};
    use egui_kittest::kittest::{NodeT, Queryable};
    use jvid_core::history::{Created, History};
    use jvid_core::keyframe::{Easing, Keyframe, Value};
    use jvid_core::model::{NewClip, ProbeInfo, ProjectSettings, Transform};
    use jvid_core::time::FLICKS_PER_SECOND as S;

    use super::*;

    pub(crate) fn fixture(layers: usize) -> (History, Vec<ClipId>) {
        let mut h = History::new(Project::default());
        h.apply(Command::Import(vec![(
            "/nonexistent/four-three.mp4".into(),
            ProbeInfo {
                width: Some(1440),
                height: Some(1080),
                duration: Time(20 * S),
                ..Default::default()
            },
        )]))
        .unwrap();
        let asset = h.project().assets()[0].id;
        let mut ids = Vec::new();
        for n in 0..layers {
            let Created::Track(track) = h
                .apply(Command::AddTrack {
                    kind: TrackKind::Video,
                    name: format!("V{n}"),
                })
                .unwrap()
            else {
                panic!()
            };
            let Created::Clip(id) = h
                .apply(Command::AddClip {
                    track,
                    clip: NewClip {
                        asset,
                        start: Time(0),
                        source_in: Time(2 * S),
                        source_out: Time(12 * S),
                        link: None,
                    },
                })
                .unwrap()
            else {
                panic!()
            };
            ids.insert(0, id);
        }
        (h, ids)
    }

    #[test]
    fn presets_reset_geometry_preserve_other_props_undo_redo_and_reopen() {
        let (mut h, ids) = fixture(1);
        let id = ids[0];
        let x = Transform {
            position: [123.0, 456.0],
            anchor: [50.0, 20.0],
            scale: [0.4, 0.8],
            rotation: 32.0,
            crop: [80.0; 4],
            opacity: 0.7,
            exposure: 0.5,
            ..Default::default()
        };
        h.apply(Command::SetClipProps(vec![(id, x, Default::default())]))
            .unwrap();
        let before = h.project().clone();
        for (mode, expected) in [
            (Framing::Fit, [240.0, 0.0, 1440.0, 1080.0]),
            (Framing::Fill, [0.0, -180.0, 1920.0, 1440.0]),
            (Framing::Stretch, [0.0, 0.0, 1920.0, 1080.0]),
        ] {
            h.apply(command(h.project(), &ids, mode, Time(0)).unwrap())
                .unwrap();
            let c = h.project().sequence().clip(id).unwrap().1;
            let x = c.transform_at(Time(0));
            assert_eq!(
                (x.position, x.anchor, x.crop, x.rotation),
                ([0.0; 2], [0.0; 2], [0.0; 4], 0.0)
            );
            assert_eq!((x.opacity, x.exposure), (0.7, 0.5));
            // The same quad positions the compositor and the transform handles. Check several
            // viewer sizes/zooms: after mapping back they are always these project pixels.
            for zoom in [0.25, 0.5, 1.0, 2.0] {
                let frame = Rect::from_min_size(pos2(13.0, 29.0), vec2(1920.0, 1080.0) * zoom);
                let q = crate::handles::corners(h.project(), c, &x, frame).unwrap();
                let tl = (q[0] - frame.min) / zoom;
                let size = (q[3] - q[0]) / zoom;
                for (got, want) in [tl.x, tl.y, size.x, size.y].into_iter().zip(expected) {
                    assert!((got - want).abs() < 0.001, "{mode:?} {got} != {want}");
                }
            }
            let json = jvid_core::file::to_json(h.project(), Path::new("/nonexistent")).unwrap();
            let reopened =
                jvid_core::file::from_json(&json, Path::new("/nonexistent"), |_| true).unwrap();
            assert_eq!(&reopened, h.project());
            let after = h.project().clone();
            assert!(h.undo());
            assert_eq!(h.project(), &before);
            assert!(h.redo());
            assert_eq!(h.project(), &after);
            assert!(h.undo());
        }
        h.apply(Command::SetSettings(ProjectSettings {
            width: 1440,
            height: 1080,
            ..Default::default()
        }))
        .unwrap();
        for mode in [Framing::Fit, Framing::Fill, Framing::Stretch] {
            h.apply(command(h.project(), &ids, mode, Time(0)).unwrap())
                .unwrap();
            assert_eq!(
                h.project().sequence().clip(id).unwrap().1.transform.scale,
                [1.0; 2]
            );
        }
    }

    #[test]
    fn presets_key_at_source_time_and_promote_legacy_scale_keys() {
        let (mut h, ids) = fixture(1);
        let id = ids[0];
        let key = |time, value| Keyframe {
            time: Time(time * S),
            value,
            easing: Easing::EaseInOut,
        };
        let keys = [
            (
                "scale".into(),
                vec![key(2, Value::Scalar(1.0)), key(10, Value::Scalar(3.0))],
            ),
            ("position".into(), vec![key(2, Value::Vec2([40.0, 60.0]))]),
            ("opacity".into(), vec![key(2, Value::Scalar(0.5))]),
        ]
        .into();
        h.apply(Command::SetClipProps(vec![(
            id,
            Transform::default(),
            keys,
        )]))
        .unwrap();
        let before = h.project().clone();
        h.apply(command(h.project(), &ids, Framing::Stretch, Time(2 * S)).unwrap())
            .unwrap();
        let c = h.project().sequence().clip(id).unwrap().1;
        assert_eq!(c.transform_at(Time(2 * S)).scale, [4.0 / 3.0, 1.0]);
        assert_eq!(c.transform_at(Time(2 * S)).position, [0.0; 2]);
        assert_eq!(
            c.keyframes["scale"]
                .iter()
                .map(|k| k.time)
                .collect::<Vec<_>>(),
            [Time(2 * S), Time(4 * S), Time(10 * S)]
        );
        assert!(
            c.keyframes["scale"]
                .iter()
                .all(|k| matches!(k.value, Value::Vec2(_)))
        );
        assert_eq!(c.keyframes["scale"][0].easing, Easing::EaseInOut);
        // Both axes interpolate to the new key, rather than holding a mismatched scalar.
        let mid = c.transform_at(Time(S)).scale;
        assert!(mid[0] > 1.0 && mid[0] < 4.0 / 3.0);
        assert_eq!(mid[1], 1.0);
        assert_eq!(
            c.keyframes["opacity"],
            before.sequence().clip(id).unwrap().1.keyframes["opacity"]
        );
        assert_eq!(
            c.keyframes["anchor"].len(),
            3,
            "grouped spatial keys seed existing times plus the playhead"
        );
        let json = jvid_core::file::to_json(h.project(), Path::new("/nonexistent")).unwrap();
        assert_eq!(
            jvid_core::file::from_json(&json, Path::new("/nonexistent"), |_| true).unwrap(),
            *h.project()
        );
        assert!(h.undo());
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn layered_hit_targets_follow_visible_transformed_quads() {
        let (mut h, ids) = fixture(2);
        // Tracks are top first; shrink the top clip so clicks beside it reach the lower one.
        let x = Transform {
            scale: [0.5; 2],
            rotation: 30.0,
            ..Default::default()
        };
        h.apply(Command::SetClipProps(vec![(ids[0], x, Default::default())]))
            .unwrap();
        let frame = Rect::from_min_size(Pos2::ZERO, vec2(960.0, 540.0));
        assert_eq!(
            crate::handles::clip_at(h.project(), Time(0), frame, pos2(480.0, 270.0)),
            Some(ids[0])
        );
        assert_eq!(
            crate::handles::clip_at(h.project(), Time(0), frame, pos2(150.0, 270.0)),
            Some(ids[1])
        );
        assert_eq!(
            crate::handles::clip_at(h.project(), Time(0), frame, pos2(10.0, 270.0)),
            None
        );
        let mut settings = h.project().sequence().tracks()[0].settings();
        settings.hidden = true;
        h.apply(Command::SetTrack(settings)).unwrap();
        assert_eq!(
            crate::handles::clip_at(h.project(), Time(0), frame, pos2(480.0, 270.0)),
            Some(ids[1])
        );
    }

    #[test]
    fn program_context_menu_targets_clicked_layer_and_disables_empty_space() {
        let (h, ids) = fixture(2);
        let mut timeline = Timeline::default();
        timeline.select_clip(h.project().sequence(), ids[1]);
        let mut h = egui_kittest::Harness::builder()
            .with_size([960.0, 600.0])
            .build_ui_state(
                move |ui, (h, timeline): &mut (History, Timeline)| {
                    let frame = Rect::from_min_size(Pos2::ZERO, vec2(960.0, 540.0));
                    // Run with handles too: the context click must coexist with their drag targets.
                    if let Some((cmd, gesture)) = crate::handles::show(
                        ui,
                        h.project(),
                        timeline.selected(),
                        Time(0),
                        frame,
                        &|_| None,
                    ) {
                        if let Some(g) = gesture {
                            h.apply_coalesced(g, cmd).unwrap();
                        } else {
                            h.apply(cmd).unwrap();
                        }
                    }
                    if let Some(cmd) = program_menu(ui, h.project(), timeline, frame) {
                        h.apply(cmd).unwrap();
                    }
                },
                (h, timeline),
            );
        h.run();
        let click = |h: &mut egui_kittest::Harness<'_, (History, Timeline)>, pos| {
            h.hover_at(pos);
            h.run();
            for pressed in [true, false] {
                h.event(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Secondary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
                h.run();
            }
        };
        click(&mut h, pos2(480.0, 270.0));
        assert!(h.state().1.selected().contains(&ids[0]));
        for label in ["Fit to Frame", "Fill Frame", "Stretch to Fit"] {
            assert!(!h.get_by_label(label).accesskit_node().is_disabled());
        }
        h.get_by_label("Stretch to Fit").click();
        h.run();
        assert_eq!(
            h.state()
                .0
                .project()
                .sequence()
                .clip(ids[0])
                .unwrap()
                .1
                .transform
                .scale,
            [4.0 / 3.0, 1.0]
        );
        assert_eq!(
            h.state()
                .0
                .project()
                .sequence()
                .clip(ids[1])
                .unwrap()
                .1
                .transform
                .scale,
            [1.0; 2]
        );
        // A stretched image exactly matching the frame uses normal clip movement, rather
        // than the inverse wide-shot reframe drag used when Fill has overflow.
        let at = pos2(400.0, 220.0);
        h.hover_at(at);
        h.run();
        h.drag_at(at);
        h.run();
        h.hover_at(at + vec2(30.0, 0.0));
        h.run();
        h.drop_at(at + vec2(30.0, 0.0));
        h.run();
        assert!(
            h.state()
                .0
                .project()
                .sequence()
                .clip(ids[0])
                .unwrap()
                .1
                .transform_at(Time(0))
                .position[0]
                > 0.0
        );
        assert!(h.state_mut().0.undo());
        assert!(h.state_mut().0.undo());
        h.run();
        click(&mut h, pos2(10.0, 270.0));
        for label in ["Fit to Frame", "Fill Frame", "Stretch to Fit"] {
            assert!(h.get_by_label(label).accesskit_node().is_disabled());
        }
        h.key_press(egui::Key::Escape);
        h.run();
        // A primary drag still moves the selected clip after a context menu closes.
        let at = pos2(400.0, 220.0);
        h.hover_at(at);
        h.run();
        h.drag_at(at);
        h.run();
        h.hover_at(at + vec2(30.0, 0.0));
        h.run();
        h.drop_at(at + vec2(30.0, 0.0));
        h.run();
        assert!(
            h.state()
                .0
                .project()
                .sequence()
                .clip(ids[0])
                .unwrap()
                .1
                .transform_at(Time(0))
                .position[0]
                > 0.0
        );
        assert_eq!(h.state().1.selected(), &HashSet::from([ids[0]]));
    }
}
