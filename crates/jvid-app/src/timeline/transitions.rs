//! Transitions at cuts: added from the clip menu or a shortcut at the selected clip's end (or
//! the cut nearest the playhead), drawn as a block centred on the cut whose edges drag its
//! length, with a right-click menu to change or remove it. Edits are `SetClipProps` on the
//! outgoing clip and its linked clips, as with fades, so they undo the same way.

use egui::{CornerRadius, CursorIcon, Id, Painter, Rect, Sense, Stroke, Ui};
use jvid_core::history::{Command, GestureKey};
use jvid_core::model::{Clip, ClipId, ModelError, Project, Transition, TransitionKind};
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use suite_ui::tokens::color;

use crate::i18n::tr;

use super::{Edit, Timeline, View};

/// Undo gesture for block drags, xored with the clip and drag count.
const GESTURE: GestureKey = 0x7472_616e_0000; // "tran"
/// The length a new transition asks for, held to the handles.
const DEFAULT: Time = Time(FLICKS_PER_SECOND);

/// The keymap action adding each kind.
pub const ACTIONS: [(&str, TransitionKind); 3] = [
    ("crossfade", TransitionKind::Crossfade),
    ("dip_to_black", TransitionKind::DipToBlack),
    ("dip_to_white", TransitionKind::DipToWhite),
];

/// Sets the transition at the cut after `id`, and after its linked clips that end on a cut,
/// to `x`, its length held to the handles; `None` removes them. Errors with the message to
/// show when there are no handles.
pub fn set(project: &Project, id: ClipId, x: Option<Transition>) -> Result<Command, String> {
    let seq = project.sequence();
    let x = match x {
        Some(x) => {
            let room = project.transition_room(id).map_err(|e| match e {
                ModelError::NoHandles(_) => tr!("timeline.no_handles").to_owned(),
                e => e.to_string(),
            })?;
            Some(Transition {
                duration: x.duration.clamp(Time(1), room),
                ..x
            })
        }
        None => None,
    };
    let at_cut = |t, c: &Clip| {
        let clips = seq.track(t).map_or(&[][..], |t| t.clips());
        clips.iter().any(|n| n.start == c.end())
    };
    let edits = seq
        .linked(id)
        .into_iter()
        .filter(|(t, c)| c.id == id || at_cut(*t, c));
    let edits = edits.map(|(_, c)| {
        let t = jvid_core::model::Transform {
            transition: x,
            ..c.transform.clone()
        };
        (c.id, t, c.keyframes.clone())
    });
    Ok(Command::SetClipProps(edits.collect()))
}

impl Timeline {
    /// The outgoing clip for a transition action: the first selected clip ending on a cut, else
    /// the clip ending on the cut nearest the playhead on an unlocked track.
    pub(super) fn transition_target(&self, project: &Project) -> Option<ClipId> {
        let seq = project.sequence();
        let here = self.playhead();
        let cuts = seq.tracks().iter().filter(|t| !t.locked).flat_map(|t| {
            let clips = t.clips();
            clips
                .windows(2)
                .filter(|w| w[1].start == w[0].end())
                .map(|w| &w[0])
        });
        let (mut chosen, mut near) = (None, None::<&Clip>);
        for c in cuts {
            if self.selected.contains(&c.id) && chosen.is_none() {
                chosen = Some(c.id);
            }
            if near.is_none_or(|n| (c.end() - here).0.abs() < (n.end() - here).0.abs()) {
                near = Some(c);
            }
        }
        let selected = !self.selected.is_empty();
        if selected { chosen } else { near.map(|c| c.id) }
    }

    /// The command for transition action `id`, or the message to show when it can't apply.
    pub(super) fn transition_action(&self, id: &str, project: &Project) -> Result<Command, String> {
        let (_, kind) = ACTIONS
            .iter()
            .find(|(a, _)| *a == id)
            .expect("a transition action");
        let target = self.transition_target(project);
        let target = target.ok_or(tr!("timeline.no_cut"))?;
        let x = Transition {
            kind: *kind,
            duration: DEFAULT,
        };
        set(project, target, Some(x))
    }

    /// Draws and edits the transition at `clip`'s end inside its lane row `r`, when the next
    /// clip on the track starts there.
    pub(super) fn transition_block(
        &mut self,
        ui: &Ui,
        painter: &Painter,
        project: &Project,
        view: View,
        clip: &Clip,
        r: Rect,
    ) -> Option<Edit> {
        let x = clip.transform.transition?;
        let id = Id::new("transition block");
        let seq = project.sequence();
        let (t, _) = seq.clip(clip.id)?;
        seq.track(t)?
            .clips()
            .iter()
            .find(|n| n.start == clip.end())?;
        let cut = clip.end();
        let start = cut - Time(x.duration.0 / 2);
        let (l, rt) = (view.x(start), view.x(start + x.duration));
        let block = Rect::from_x_y_ranges(l..=rt.max(l + 6.0), r.top()..=r.center().y);
        let fill = color::amber().gamma_multiply(0.6);
        painter.rect_filled(block, CornerRadius::same(2), fill);
        painter.line_segment(
            [block.left_bottom(), block.right_top()],
            Stroke::new(1.0, color::border()),
        );
        let mut edit = None;
        let resp = ui.interact(
            block,
            id.with(("transition", clip.id)),
            Sense::click_and_drag(),
        );
        let drags = id.with(("transition drags", clip.id));
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        if resp.drag_started() {
            ui.data_mut(|d| *d.get_temp_mut_or(drags, 0u64) += 1);
        }
        // Either edge: the length is twice the distance from the cut, in whole frames.
        if resp.dragged()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let rate = self.transport.rate;
            let half = rate
                .index_nearest(Time((view.time(p.x) - cut).0.abs()))
                .max(1);
            let x = Transition {
                duration: rate.time_floor(2 * half),
                ..x
            };
            let n = ui.data(|d| d.get_temp(drags).unwrap_or(0u64));
            if let Ok(c) = set(project, clip.id, Some(x)) {
                edit = Some((c, Some(GESTURE ^ clip.id.0 ^ (n << 16))));
            }
        }
        resp.on_hover_text(name(x.kind)).context_menu(|ui| {
            for (_, kind) in ACTIONS {
                if ui.radio(x.kind == kind, name(kind)).clicked() {
                    edit = set(project, clip.id, Some(Transition { kind, ..x }))
                        .ok()
                        .map(|c| (c, None));
                    ui.close();
                }
            }
            ui.separator();
            if ui.button(tr!("timeline.remove_transition")).clicked() {
                edit = set(project, clip.id, None).ok().map(|c| (c, None));
                ui.close();
            }
        });
        edit
    }
}

fn name(kind: TransitionKind) -> &'static str {
    match kind {
        TransitionKind::Crossfade => tr!("timeline.crossfade"),
        TransitionKind::DipToBlack => tr!("timeline.dip_to_black"),
        TransitionKind::DipToWhite => tr!("timeline.dip_to_white"),
    }
}

#[cfg(test)]
mod tests {
    use jvid_core::history::History;

    use super::super::tests::project;
    use super::*;

    fn clips(h: &History) -> Vec<Clip> {
        h.project().sequence().tracks()[0].clips().to_vec()
    }

    #[test]
    fn needs_handles_then_applies_holds_and_undoes() {
        use jvid_core::model::Edge;
        let mut h = History::new(project(4));
        let t = Timeline::default();
        let [a, b] = [0, 1].map(|i| clips(&h)[i].id);
        // b starts at source 0: nothing before its first frame to fade in from.
        let msg = t.transition_action("crossfade", h.project()).unwrap_err();
        assert!(msg.contains("no media handles"), "{msg}");
        let s = FLICKS_PER_SECOND;
        let trims = vec![(b, Edge::Head, Time(s / 2))];
        h.apply(Command::Trim {
            trims,
            ripple: true,
        })
        .unwrap();
        h.apply(t.transition_action("dip_to_white", h.project()).unwrap())
            .unwrap();
        let x = clips(&h)[0].transform.transition.unwrap();
        assert_eq!((x.kind, x.duration), (TransitionKind::DipToWhite, DEFAULT));
        let long = Transition {
            duration: Time(10 * s),
            ..x
        };
        h.apply(set(h.project(), a, Some(long)).unwrap()).unwrap();
        let got = clips(&h)[0].transform.transition.unwrap().duration;
        assert_eq!(got, Time(s), "held to half a second each side");
        h.apply(set(h.project(), a, None).unwrap()).unwrap();
        assert_eq!(clips(&h)[0].transform.transition, None);
        assert!(h.undo());
        assert_eq!(clips(&h)[0].transform.transition, Some(x));
    }
}
