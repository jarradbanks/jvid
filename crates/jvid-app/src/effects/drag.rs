//! Effect drag feedback and destination resolution. Targets are recomputed every frame;
//! no remembered hover can survive leaving a panel, cancelling, or changing the playhead.

use std::collections::HashSet;

use egui::{Align, DragAndDrop, Layout, Rect, Stroke, Ui, UiBuilder, vec2};
use jvid_core::history::Command;
use jvid_core::model::{ClipId, Project, TrackId, TrackKind};
use jvid_core::time::Time;
use suite_ui::theme;
use suite_ui::tokens::{color, radius, size, space};

use super::Drag;

/// Hidden, locked, disabled and non-video clips cannot receive a drag.
fn eligible(project: &Project, id: ClipId) -> bool {
    let seq = project.sequence();
    let Some((track, clip)) = seq.clip(id) else {
        return false;
    };
    seq.track(track)
        .is_some_and(|t| t.kind == TrackKind::Video && !t.hidden && !t.locked)
        && clip.enabled
        && project
            .asset(clip.asset)
            .is_some_and(|a| a.generated() || a.probe.as_ref().is_some_and(|p| p.width.is_some()))
}

pub fn usable(stem: &str) -> bool {
    super::with(|l| !l.disabled.contains(stem) && l.get(stem).is_some_and(Result::is_ok))
}

/// The clip actually beneath the timeline pointer, never a neighbouring clip or track.
pub fn timeline_target(project: &Project, track: TrackId, time: Time) -> Option<ClipId> {
    project
        .sequence()
        .track(track)?
        .clips()
        .iter()
        .find_map(|c| {
            (c.start <= time && time < c.end() && eligible(project, c.id)).then_some(c.id)
        })
}

/// Prefer a selected visible layer at the playhead, resolving ties in compositing order.
/// Otherwise use the topmost eligible layer. Selection itself is left untouched.
pub fn program_target(project: &Project, selected: &HashSet<ClipId>, time: Time) -> Option<ClipId> {
    let layers = jvid_core::eval::video_at(project.sequence(), time);
    let candidates: Vec<_> = layers
        .iter()
        .filter_map(|l| {
            let (_, c) = project.sequence().clip(l.clip)?;
            (c.start <= time
                && time < c.end()
                && l.transform.opacity > 0.0
                && eligible(project, c.id))
            .then_some(c.id)
        })
        .collect();
    candidates
        .iter()
        .find(|id| selected.contains(*id))
        .copied()
        .or_else(|| candidates.first().copied())
}

pub fn command(project: &Project, target: ClipId, stem: &str) -> Option<Command> {
    (eligible(project, target) && usable(stem))
        .then(|| crate::inspector::add_effect(project, &[target].into(), stem))
        .flatten()
}

/// A floating label with the translated effect name, above every panel like media drags.
pub fn ghost(ui: &Ui) {
    let Some(fx) = DragAndDrop::payload::<Drag>(ui.ctx()) else {
        return;
    };
    let Some(pos) = ui.ctx().pointer_latest_pos() else {
        return;
    };
    let name = super::with(|l| {
        l.rows("")
            .into_iter()
            .find(|r| r.stem == fx.0)
            .map(|r| r.name)
    });
    egui::Area::new(egui::Id::new("effect drag preview"))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos + vec2(space::S12, space::S12))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(240.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(name.as_deref().unwrap_or(&fx.0))
                            .font(theme::body_strong()),
                    )
                    .truncate(),
                );
            });
        });
}

/// Outline the actual layer and identify it, even when a selected lower layer receives it.
/// Returns the same validated target used to make the release command.
pub fn program_drop(
    ui: &mut Ui,
    project: &Project,
    selected: &HashSet<ClipId>,
    time: Time,
    frame: Rect,
) -> Option<Command> {
    let fx = DragAndDrop::payload::<Drag>(ui.ctx())?;
    let viewer = crate::preview::viewer_rect(ui).intersect(ui.clip_rect());
    if !ui.rect_contains_pointer(viewer) {
        return None;
    }
    let target = usable(&fx.0)
        .then(|| program_target(project, selected, time))
        .flatten();
    ui.ctx().set_cursor_icon(if target.is_some() {
        egui::CursorIcon::Copy
    } else {
        egui::CursorIcon::NotAllowed
    });
    let target = target?;
    let (track, clip) = project.sequence().clip(target)?;
    let track = project.sequence().track(track)?;
    let painter = ui.painter().with_clip_rect(viewer);
    let stroke = Stroke::new(2.0, color::light_blue());
    painter.rect_stroke(
        viewer.shrink(2.0),
        radius::SMALL,
        stroke,
        egui::StrokeKind::Inside,
    );
    let transform = clip.transform_at(time);
    if let Some([tl, tr, bl, br]) = crate::handles::corners(project, clip, &transform, frame) {
        for edge in [[tl, tr], [tr, br], [br, bl], [bl, tl]] {
            painter.line_segment(edge, stroke);
        }
    }
    let name = format!(
        "{} · {}",
        track.name,
        project.asset(clip.asset)?.display_name()
    );
    let label = Rect::from_min_size(
        viewer.min + vec2(space::S8, space::S8),
        vec2((viewer.width() - 2.0 * space::S8).max(0.0), size::CONTROL),
    );
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(label)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(name)
                        .font(theme::body_strong())
                        .background_color(color::panel()),
                )
                .truncate(),
            );
        },
    );
    if ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary)) {
        let cmd = command(project, target, &fx.0);
        DragAndDrop::clear_payload(ui.ctx());
        return cmd;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, pos2};
    use egui_kittest::{Harness, kittest::Queryable};
    use jvid_core::history::History;
    use jvid_core::model::{NewClip, ProbeInfo};
    use jvid_core::time::FLICKS_PER_SECOND as S;

    fn stacked() -> (History, [ClipId; 3], [TrackId; 3]) {
        let mut h = History::new(Project::default());
        h.apply(Command::Import(vec![(
            "/nonexistent/drop.mp4".into(),
            ProbeInfo {
                duration: Time(10 * S),
                width: Some(1920),
                height: Some(1080),
                audio_streams: 1,
                ..Default::default()
            },
        )]))
        .unwrap();
        for (kind, name) in [
            (TrackKind::Video, "Top"),
            (TrackKind::Video, "Lower"),
            (TrackKind::Audio, "Audio"),
        ] {
            h.apply(Command::AddTrack {
                kind,
                name: name.into(),
            })
            .unwrap();
        }
        let asset = h.project().assets()[0].id;
        let tracks: Vec<_> = h
            .project()
            .sequence()
            .tracks()
            .iter()
            .map(|t| t.id)
            .collect();
        for &track in &tracks {
            h.apply(Command::AddClip {
                track,
                clip: NewClip {
                    asset,
                    source_in: Time(0),
                    source_out: Time(2 * S),
                    start: Time(0),
                    link: None,
                },
            })
            .unwrap();
        }
        let clips: Vec<_> = h
            .project()
            .sequence()
            .tracks()
            .iter()
            .map(|t| t.clips()[0].id)
            .collect();
        (h, clips.try_into().unwrap(), tracks.try_into().unwrap())
    }

    #[test]
    fn program_resolves_selected_layers_then_compositing_order_deterministically() {
        let (mut h, [top, lower, audio], tracks) = stacked();
        for selected in [HashSet::new(), [top, lower, audio].into(), [audio].into()] {
            assert_eq!(program_target(h.project(), &selected, Time(S)), Some(top));
        }
        assert_eq!(
            program_target(h.project(), &[lower, audio].into(), Time(S)),
            Some(lower)
        );
        let mut settings = h.project().sequence().track(tracks[0]).unwrap().settings();
        settings.hidden = true;
        h.apply(Command::SetTrack(settings)).unwrap();
        assert_eq!(
            program_target(h.project(), &[top].into(), Time(S)),
            Some(lower)
        );
        let mut settings = h.project().sequence().track(tracks[1]).unwrap().settings();
        settings.locked = true;
        h.apply(Command::SetTrack(settings)).unwrap();
        assert_eq!(program_target(h.project(), &[lower].into(), Time(S)), None);
        assert_eq!(
            program_target(&Project::default(), &HashSet::new(), Time(0)),
            None
        );
    }

    #[test]
    fn both_destinations_reject_gaps_audio_disabled_hidden_and_locked_clips() {
        let (mut h, [top, lower, audio], tracks) = stacked();
        assert_eq!(timeline_target(h.project(), tracks[0], Time(S)), Some(top));
        assert_eq!(timeline_target(h.project(), tracks[2], Time(S)), None);
        assert_eq!(timeline_target(h.project(), tracks[0], Time(2 * S)), None);
        assert_eq!(
            program_target(h.project(), &[top].into(), Time(2 * S)),
            None
        );
        for (hidden, locked) in [(true, false), (false, true)] {
            let mut s = h.project().sequence().track(tracks[0]).unwrap().settings();
            (s.hidden, s.locked) = (hidden, locked);
            h.apply(Command::SetTrack(s)).unwrap();
            assert_eq!(timeline_target(h.project(), tracks[0], Time(S)), None);
            assert_eq!(
                program_target(h.project(), &[top, audio].into(), Time(S)),
                Some(lower)
            );
        }
        h.undo();
        h.undo();
        h.apply(Command::SetClipsEnabled {
            ids: vec![top],
            enabled: false,
        })
        .unwrap();
        assert_eq!(timeline_target(h.project(), tracks[0], Time(S)), None);
        assert_eq!(
            program_target(h.project(), &[top].into(), Time(S)),
            Some(lower)
        );
        h.undo();
        let c = h.project().sequence().clip(top).unwrap().1;
        let mut x = c.transform.clone();
        x.opacity = 0.0;
        h.apply(Command::SetClipProps(vec![(top, x, c.keyframes.clone())]))
            .unwrap();
        assert_eq!(
            program_target(h.project(), &[top].into(), Time(S)),
            Some(lower)
        );
        let mut s = h.project().sequence().track(tracks[1]).unwrap().settings();
        s.solo = true;
        h.apply(Command::SetTrack(s)).unwrap();
        assert_eq!(
            program_target(h.project(), &[top].into(), Time(S)),
            Some(lower)
        );
        assert!(command(h.project(), audio, "vignette").is_none());
        assert!(command(h.project(), lower, "missing_drop_effect").is_none());
    }

    struct State {
        h: History,
        selected: HashSet<ClipId>,
        time: Time,
    }
    fn harness() -> (Harness<'static, State>, [ClipId; 3]) {
        let (h, clips, _) = stacked();
        let mut fresh = true;
        let harness = Harness::builder().with_size([640.0, 360.0]).build_ui_state(
            move |ui, s: &mut State| {
                if suite_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                let frame = crate::preview::viewer_rect(ui);
                if let Some(cmd) = program_drop(ui, s.h.project(), &s.selected, s.time, frame) {
                    s.h.apply(cmd).unwrap();
                }
                ghost(ui);
            },
            State {
                h,
                selected: [clips[1], clips[2]].into(),
                time: Time(S),
            },
        );
        (harness, clips)
    }
    fn begin(h: &mut Harness<'_, State>, at: Pos2) {
        h.run();
        h.hover_at(at);
        h.run();
        DragAndDrop::set_payload(&h.ctx, Drag("vignette".into()));
        h.drag_at(at);
        h.run();
    }
    fn library() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        super::super::scan_for_test(dir.path());
        dir
    }

    #[test]
    fn program_drop_identifies_actual_recipient_applies_once_and_undoes_without_changing_selection()
    {
        let _dir = library();
        let (mut h, [top, lower, audio]) = harness();
        let before = h.state().h.project().clone();
        let selected = h.state().selected.clone();
        let at = pos2(300.0, 140.0);
        begin(&mut h, at);
        h.get_by_label("Lower · drop.mp4");
        h.get_by_label(crate::i18n::tr!("bundled.vignette"));
        h.drop_at(at);
        h.run();
        let seq = h.state().h.project().sequence();
        assert!(seq.clip(top).unwrap().1.transform.effects.is_empty());
        assert!(seq.clip(audio).unwrap().1.transform.effects.is_empty());
        let fx = &seq.clip(lower).unwrap().1.transform.effects;
        assert_eq!(fx.len(), 1);
        assert_eq!(fx[0].shader, "vignette");
        assert!(fx[0].params.contains_key("strength"));
        assert_eq!(h.state().selected, selected);
        assert!(!DragAndDrop::has_any_payload(&h.ctx));
        assert!(h.query_by_label("Lower · drop.mp4").is_none());
        assert!(h.state_mut().h.undo());
        assert_eq!(h.state().h.project(), &before);
        assert!(h.state_mut().h.redo());
        assert_eq!(
            h.state()
                .h
                .project()
                .sequence()
                .clip(lower)
                .unwrap()
                .1
                .transform
                .effects
                .len(),
            1
        );
    }

    #[test]
    fn program_invalid_releases_and_cancellation_clear_feedback() {
        let _dir = library();
        for mode in 0..5 {
            let (mut h, _) = harness();
            let at = pos2(300.0, 140.0);
            begin(&mut h, at);
            h.get_by_label("Lower · drop.mp4");
            let drop = if mode == 0 { pos2(300.0, 345.0) } else { at };
            if mode == 1 {
                h.state_mut().time = Time(2 * S);
            }
            if mode == 2 {
                h.key_press(egui::Key::Escape);
            }
            if mode == 3 {
                DragAndDrop::set_payload(&h.ctx, Drag("missing_drop_effect".into()));
            }
            if mode == 4 {
                h.state_mut().h = History::new(Project::default());
            }
            let before = h.state().h.project().clone();
            h.hover_at(drop);
            h.run();
            assert!(h.query_by_label("Lower · drop.mp4").is_none());
            h.drop_at(drop);
            h.run();
            assert_eq!(h.state().h.project(), &before);
            assert!(!DragAndDrop::has_any_payload(&h.ctx));
            assert!(
                h.query_by_label(crate::i18n::tr!("bundled.vignette"))
                    .is_none()
            );
        }
    }
}
