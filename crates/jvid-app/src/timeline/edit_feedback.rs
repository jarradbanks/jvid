//! Transient feedback from the exact command returned on release. The core planner owns
//! validation and edit semantics; this module only compares and paints its result.

use super::*;
use jvid_core::model::MarkerId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SnapTarget {
    Playhead,
    In,
    Out,
    Marker(MarkerId, bool),
    Clip(ClipId, bool),
}

impl SnapTarget {
    pub fn label(self, project: &Project) -> String {
        match self {
            Self::Playhead => tr!("timeline_feedback.playhead").to_owned(),
            Self::In => tr!("timeline_feedback.in_point").to_owned(),
            Self::Out => tr!("timeline_feedback.out_point").to_owned(),
            Self::Marker(id, end) => {
                let name = project
                    .sequence()
                    .markers()
                    .iter()
                    .find(|m| m.id == id)
                    .map(|m| m.name.as_str())
                    .unwrap_or_default();
                if end {
                    tr!("timeline_feedback.marker_end", name = name)
                } else {
                    tr!("timeline_feedback.marker", name = name)
                }
            }
            Self::Clip(id, end) => {
                let name = project
                    .sequence()
                    .clip(id)
                    .and_then(|(_, c)| project.asset(c.asset))
                    .map(|a| a.display_name())
                    .unwrap_or_default();
                if end {
                    tr!("timeline_feedback.clip_end", name = name)
                } else {
                    tr!("timeline_feedback.clip_start", name = name)
                }
            }
        }
    }
}

/// Times and target identities share one ordered list, so ties paint the target that
/// actually won snapping. Labels resolve through the active catalog when drawn.
pub(super) fn snap_targets(
    seq: &Sequence,
    playhead: Time,
    moving: &[ClipId],
    flag: Option<bool>,
    marker: Option<MarkerId>,
) -> Vec<(Time, SnapTarget)> {
    let mut targets = vec![(playhead, SnapTarget::Playhead)];
    if flag != Some(true)
        && let Some(t) = seq.in_point()
    {
        targets.push((t, SnapTarget::In));
    }
    if flag != Some(false)
        && let Some(t) = seq.out_point()
    {
        targets.push((t, SnapTarget::Out));
    }
    for m in seq.markers().iter().filter(|m| Some(m.id) != marker) {
        targets.push((m.time, SnapTarget::Marker(m.id, false)));
        if m.duration > Time(0) {
            targets.push((m.time + m.duration, SnapTarget::Marker(m.id, true)));
        }
    }
    for c in seq
        .tracks()
        .iter()
        .flat_map(|t| t.clips())
        .filter(|c| !moving.contains(&c.id))
    {
        targets.extend([
            (c.start, SnapTarget::Clip(c.id, false)),
            (c.end(), SnapTarget::Clip(c.id, true)),
        ]);
    }
    targets
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Span {
    pub track: Option<TrackId>,
    pub start: Time,
    pub end: Time,
    pub primary: bool,
}

/// Kept only while a pointer edit is active. No history, media, or project copy.
#[derive(Debug)]
pub(super) struct Plan {
    pub command: Command,
    pub ghosts: Vec<Span>,
    pub erased: Vec<Span>,
    pub overwritten: Vec<Span>,
    /// Alt held on a move that overwrites: the drop adds a crossfade at the cut.
    pub crossfade: bool,
}

impl Plan {
    pub fn new(project: &Project, command: Command) -> Result<Self, ModelError> {
        let result = command
            .preview_timeline(project)?
            .expect("timeline command");
        let before = project.sequence();
        let old: HashMap<_, _> = before
            .tracks()
            .iter()
            .flat_map(|t| t.clips().iter().map(move |c| (c.id, (t.id, c))))
            .collect();
        let after: HashMap<_, _> = result
            .tracks()
            .iter()
            .flat_map(|t| t.clips().iter().map(move |c| (c.id, (t.id, c))))
            .collect();
        let moving: HashSet<_> = match &command {
            Command::MoveSelection { ids, anchor, .. } => ids
                .iter()
                .chain([anchor])
                .flat_map(|id| before.linked(*id).into_iter().map(|(_, c)| c.id))
                .collect(),
            Command::Trim { trims, .. } => trims
                .iter()
                .flat_map(|(id, ..)| before.linked(*id).into_iter().map(|(_, c)| c.id))
                .collect(),
            _ => HashSet::new(),
        };
        let primary = |c: &Clip| match &command {
            Command::Place(p) => {
                !old.contains_key(&c.id)
                    && c.asset == p.asset
                    && c.start == p.start
                    && c.source_in == p.source_in
                    && c.source_out == p.source_out
            }
            _ => moving.contains(&c.id),
        };
        let mut ghosts = Vec::new();
        for track in result.tracks() {
            for c in track.clips() {
                if old
                    .get(&c.id)
                    .is_some_and(|&(t, old)| t == track.id && old == c)
                {
                    continue;
                }
                ghosts.push(Span {
                    track: before.track(track.id).map(|t| t.id),
                    start: c.start,
                    end: c.end(),
                    primary: primary(c),
                });
            }
        }
        let erased = old
            .iter()
            .filter(|(id, value)| after.get(id) != Some(value))
            .map(|(_, &(track, c))| Span {
                track: Some(track),
                start: c.start,
                end: c.end(),
                primary: false,
            })
            .collect();
        let overwrite = matches!(&command, Command::MoveSelection { .. })
            || matches!(&command, Command::Place(p) if !p.insert);
        let mut overwritten = Vec::new();
        if overwrite {
            for ghost in ghosts.iter().filter(|g| g.primary) {
                for c in ghost
                    .track
                    .and_then(|t| before.track(t))
                    .into_iter()
                    .flat_map(|t| t.clips())
                {
                    if !moving.contains(&c.id) && c.start < ghost.end && c.end() > ghost.start {
                        overwritten.push(Span {
                            track: ghost.track,
                            start: c.start.max(ghost.start),
                            end: c.end().min(ghost.end),
                            primary: false,
                        });
                    }
                }
            }
        }
        Ok(Self {
            command,
            ghosts,
            erased,
            overwritten,
            crossfade: false,
        })
    }

    pub fn label(&self, rate: Rate) -> String {
        match &self.command {
            Command::Place(p) if p.insert => tr!("timeline_feedback.insert").to_owned(),
            _ if self.crossfade => tr!("timeline_feedback.crossfade").to_owned(),
            Command::Trim { trims, ripple } => {
                let by = trims[0].2;
                let sign = if by < Time(0) { "−" } else { "+" };
                let delta = format!(
                    "{sign}{}",
                    format_timecode(rate.index_nearest(Time(by.0.abs())) as u64, rate)
                );
                if *ripple {
                    tr!("timeline_feedback.ripple_trim", delta = delta)
                } else {
                    tr!("timeline_feedback.trim", delta = delta)
                }
            }
            _ => tr!("timeline_feedback.overwrite").to_owned(),
        }
    }
}

pub(super) fn refusal(error: &ModelError) -> String {
    match error {
        ModelError::Locked(_) => tr!("timeline.track_locked").to_owned(),
        _ => tr!("timeline_feedback.unavailable").to_owned(),
    }
}

/// A passive overlay, never a hit target. Wrap within even a 240pt dock panel, flip above
/// the pointer near the bottom, align to physical pixels at fractional display scales.
pub(super) fn paint_label(ui: &Ui, bounds: Rect, pointer: Pos2, label: String) -> Rect {
    let ppp = ui.pixels_per_point();
    let bounds = bounds.shrink(4.0);
    let bounds = Rect::from_min_max(
        pos2(
            (bounds.left() * ppp).ceil() / ppp,
            (bounds.top() * ppp).ceil() / ppp,
        ),
        pos2(
            (bounds.right() * ppp).floor() / ppp,
            (bounds.bottom() * ppp).floor() / ppp,
        ),
    );
    if bounds.width() < 16.0 || bounds.height() < 16.0 {
        return Rect::NOTHING;
    }
    let painter = ui
        .ctx()
        .layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            ui.id().with("timeline_edit_feedback"),
        ))
        .with_clip_rect(bounds);
    let galley = painter.layout(
        label,
        theme::body(),
        color::ink(),
        (bounds.width() - 12.0).max(1.0),
    );
    let size = galley.size() + vec2(12.0, 8.0);
    let size = vec2((size.x * ppp).ceil() / ppp, (size.y * ppp).ceil() / ppp).min(bounds.size());
    let mut origin = pointer + vec2(12.0, 16.0);
    // The bin draws its dragged thumbnail below the pointer on the foreground layer.
    // Keep source feedback above it, even when the panel has room on both sides.
    if egui::DragAndDrop::has_payload_of_type::<AssetId>(ui.ctx())
        || origin.y + size.y > bounds.bottom()
    {
        origin.y = pointer.y - size.y - 12.0;
    }
    origin = pos2(
        ((origin.x * ppp).round() / ppp).clamp(bounds.left(), bounds.right() - size.x),
        ((origin.y * ppp).round() / ppp).clamp(bounds.top(), bounds.bottom() - size.y),
    );
    let rect = Rect::from_min_size(origin, size).intersect(bounds);
    painter.rect_filled(rect, radius::SMALL, color::raised());
    painter.rect_stroke(
        rect,
        radius::SMALL,
        Stroke::new(1.0 / ppp, color::border_strong()),
        egui::StrokeKind::Inside,
    );
    painter.with_clip_rect(rect.shrink2(vec2(6.0, 4.0))).galley(
        rect.min + vec2(6.0, 4.0),
        galley,
        color::ink(),
    );
    rect
}

impl Timeline {
    /// Edge grips inside a clip in the selection tool; dragging one trims, Ctrl ripples.
    /// Returns the edit on release.
    pub(super) fn trim_handles(
        &mut self,
        ui: &Ui,
        project: &Project,
        view: View,
        clip: &Clip,
        r: Rect,
    ) -> Option<Edit> {
        if self.tool != Tool::Selection {
            return None;
        }
        if locked(project.sequence(), clip.id) {
            if self.trim.is_some_and(|trim| trim.clip == clip.id) {
                self.trim = None;
                self.clear_snap();
                if let Some((track, _)) = project
                    .sequence()
                    .linked(clip.id)
                    .into_iter()
                    .find(|(track, _)| project.sequence().track(*track).is_some_and(|t| t.locked))
                {
                    self.feedback_error = Some(ModelError::Locked(track));
                }
                ui.ctx().set_cursor_icon(CursorIcon::NotAllowed);
            }
            return None;
        }
        let w = 6.0f32.min(r.width() / 3.0);
        let rate = self.transport.rate;
        for (edge, zone, actual, painted) in [
            (
                Edge::Head,
                r.left()..=r.left() + w,
                view.x(clip.start) + 1.0,
                r.left(),
            ),
            (
                Edge::Tail,
                r.right() - w..=r.right(),
                view.x(clip.end()) - 1.0,
                r.right(),
            ),
        ] {
            let key = ("trim", clip.id, edge == Edge::Head);
            let active = ui.ctx().dragged_id() == Some(ui.id().with(key))
                || self
                    .trim
                    .is_some_and(|trim| trim.clip == clip.id && trim.edge == edge);
            // A viewport-clamped clip end is not a new trim handle. Keep an already
            // captured handle alive when autoscroll carries its original edge offscreen.
            if (actual - painted).abs() > 0.1 && !active {
                continue;
            }
            let zone = Rect::from_x_y_ranges(zone, r.y_range()).intersect(ui.clip_rect());
            let h = ui.interact(zone, ui.id().with(key), Sense::drag());
            if h.hovered() || h.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            if h.is_pointer_button_down_on() {
                self.holding = Some(clip.id);
            }
            if !self.cancelled
                && (h.dragged() || h.drag_stopped())
                && let Some(p) = h.interact_pointer_pos()
            {
                let by = view.time(p.x) - self.press_time(ui, view, p);
                let by = rate.time_floor(rate.index_nearest(by));
                let at = match edge {
                    Edge::Head => clip.start + by,
                    Edge::Tail => clip.end() + by,
                };
                let seq = project.sequence();
                let group: Vec<_> = seq.linked(clip.id).iter().map(|(_, c)| c.id).collect();
                let mods = ui.input(|i| i.modifiers);
                let by = by
                    + if mods.shift {
                        self.clear_snap();
                        Time(0)
                    } else {
                        self.snap(seq, &group, &[at])
                    };
                let ripple = mods.command;
                let limited = project.trim_limit(clip.id, edge, by, ripple);
                if limited != by {
                    self.clear_snap();
                }
                let command = Command::Trim {
                    trims: vec![(clip.id, edge, limited)],
                    ripple,
                };
                match edit_feedback::Plan::new(project, command) {
                    Ok(plan) => {
                        self.trim = Some(Trim {
                            clip: clip.id,
                            edge,
                            by: limited,
                        });
                        self.feedback = Some(plan);
                    }
                    Err(error) => {
                        self.trim = None;
                        self.feedback_error = Some(error);
                        self.clear_snap();
                        ui.ctx().set_cursor_icon(CursorIcon::NotAllowed);
                    }
                }
            }
            if h.drag_stopped() {
                self.clear_snap();
                if let Some(t) = self.trim.take()
                    && t.by != Time(0)
                    && !self.cancelled
                {
                    return self.feedback.take().map(|p| (p.command, None));
                }
            }
        }
        None
    }

    pub(super) fn clear_snap(&mut self) {
        self.snapped = None;
        self.snap_target = None;
    }
}

impl Timeline {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn feedback_drop(
        &mut self,
        ui: &Ui,
        project: &Project,
        view: View,
        area: Rect,
        lanes: &[(TrackId, TrackKind, Rect)],
        held: Option<(&Clip, &Track, egui::Response)>,
        edit: &mut Option<Edit>,
        top: &mut bool,
        slot: bool,
        p: Pos2,
    ) {
        let rate = self.transport.rate;
        let snap = |t: Time| rate.time_floor(rate.index_nearest(t)).max(Time(0));
        let lane_at = |kind: Option<TrackKind>| {
            lanes
                .iter()
                .find(|l| l.2.with_min_x(area.left()).contains(p))
                .filter(|l| kind.is_none_or(|k| k == l.1))
                .map(|l| l.0)
        };
        if let Some(asset) = egui::DragAndDrop::payload::<AssetId>(ui.ctx()) {
            if !area.contains(p) {
                self.clear_snap();
                return;
            }
            let mods = ui.input(|i| i.modifiers);
            let on = lane_at(None).filter(|_| !slot);
            if let Some(mut placement) =
                project.placement(*asset, snap(view.time(p.x)), mods.shift, on, mods.alt)
            {
                let len = placement.source_out - placement.source_in;
                placement.start = placement.start
                    + self.snap(
                        project.sequence(),
                        &[],
                        &[placement.start, placement.start + len],
                    );
                placement.start = placement.start.max(Time(0));
                *top = on.is_none() && onto_new_track(&mut placement);
                match edit_feedback::Plan::new(project, Command::Place(placement)) {
                    Ok(plan) => {
                        if ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary)) {
                            *edit = Some((plan.command, None));
                            egui::DragAndDrop::clear_payload(ui.ctx());
                        } else {
                            self.feedback = Some(plan);
                        }
                    }
                    Err(error) => {
                        self.feedback_error = Some(error);
                        self.clear_snap();
                        ui.ctx().set_cursor_icon(CursorIcon::NotAllowed);
                    }
                }
            }
            return;
        }
        let Some((clip, from, r)) = held else {
            return;
        };
        let start = snap(clip.start + (view.time(p.x) - self.press_time(ui, view, p)));
        let seq = project.sequence();
        let ids = self.drag_selection.clone();
        let edges: Vec<_> = ids
            .iter()
            .filter_map(|id| seq.clip(*id))
            .flat_map(|(_, c)| [c.start + start - clip.start, c.end() + start - clip.start])
            .collect();
        let shift = if ui.input(|i| i.modifiers.shift) {
            self.clear_snap();
            Time(0)
        } else {
            self.snap(seq, &ids, &edges)
        };
        let start = start + shift;
        let to = lane_at(Some(from.kind)).unwrap_or(from.id);
        let command = Command::MoveSelection {
            ids,
            anchor: clip.id,
            track: to,
            start,
        };
        match edit_feedback::Plan::new(project, command) {
            Ok(mut plan) => {
                plan.crossfade = ui.input(|i| i.modifiers.alt) && !plan.overwritten.is_empty();
                // A linked clip earlier than the anchor can clamp the common shift.
                // Clear the indicator unless a resulting primary edge really hit it.
                if self.snapped.is_some_and(|at| {
                    !plan
                        .ghosts
                        .iter()
                        .any(|s| s.primary && (s.start == at || s.end == at))
                }) {
                    self.clear_snap();
                }
                self.drag_to = Some((to, start));
                if r.drag_stopped() {
                    if start != clip.start || to != from.id {
                        let fade = plan.crossfade.then(|| {
                            transitions::on_drop(project, &plan.command, clip.id, &plan.overwritten)
                        });
                        *edit = Some((fade.flatten().unwrap_or(plan.command), None));
                    }
                    self.clear_snap();
                } else {
                    self.feedback = Some(plan);
                }
            }
            Err(error) => {
                self.drag_to = None;
                self.feedback_error = Some(error);
                self.clear_snap();
                ui.ctx().set_cursor_icon(CursorIcon::NotAllowed);
            }
        }
    }
}

impl Timeline {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_edit_feedback(
        &self,
        clip_painter: &egui::Painter,
        view: View,
        lanes: &[(TrackId, TrackKind, Rect)],
        y: f32,
        slot: Rect,
        top: bool,
        open: bool,
    ) {
        let ghosts = self
            .feedback
            .as_ref()
            .map(|p| p.ghosts.as_slice())
            .unwrap_or_default();
        if let Some(plan) = &self.feedback {
            for span in &plan.erased {
                if let Some((_, _, lane)) = lanes.iter().find(|l| Some(l.0) == span.track) {
                    let rect = Rect::from_x_y_ranges(
                        view.x(span.start)..=view.x(span.end),
                        lane.y_range(),
                    )
                    .shrink2(vec2(1.0, 2.0));
                    clip_painter.rect_filled(rect, radius::SMALL, color::field());
                }
            }
        }
        // Ghosts for tracks a drop will add go in rows below the last lane, a new top video
        // track's (it comes first) in the open slot.
        let mut below = y;
        let mut slot_rows = (top && open).then(|| slot.y_range());
        for ghost in ghosts {
            let (t, from, to) = (ghost.track, ghost.start, ghost.end);
            let rows = match t {
                Some(t) => lanes.iter().find(|l| l.0 == t).map(|l| l.2.y_range()),
                None if slot_rows.is_some() => slot_rows.take(),
                None => {
                    below += Track::default_height();
                    Some((below - Track::default_height()..=below).into())
                }
            };
            if let Some(rows) = rows {
                let x = view.x(from)..=view.x(to);
                let r = Rect::from_x_y_ranges(x, rows).shrink2(vec2(1.0, 2.0));
                let corner = CornerRadius::same(radius::SMALL);
                let fill = if ghost.primary {
                    color::blue().gamma_multiply(0.3)
                } else {
                    color::ink().gamma_multiply(0.12)
                };
                clip_painter.rect_filled(r, corner, fill);
                clip_painter.rect_stroke(
                    r,
                    corner,
                    Stroke::new(
                        1.0 / clip_painter.ctx().pixels_per_point(),
                        color::light_blue(),
                    ),
                    egui::StrokeKind::Inside,
                );
            }
        }
        if let Some(plan) = &self.feedback {
            for span in &plan.overwritten {
                if let Some((_, _, lane)) = lanes.iter().find(|l| Some(l.0) == span.track) {
                    let rect = Rect::from_x_y_ranges(
                        view.x(span.start)..=view.x(span.end),
                        lane.y_range(),
                    )
                    .shrink2(vec2(1.0, 2.0));
                    clip_painter.rect_filled(rect, 0.0, color::red().gamma_multiply(0.45));
                }
            }
        }
    }
}

impl Timeline {
    pub(super) fn paint_edit_label(&self, ui: &Ui, area: Rect, project: &Project) {
        if let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) {
            let mut label = self
                .feedback
                .as_ref()
                .map(|plan| plan.label(self.transport.rate))
                .or_else(|| self.feedback_error.as_ref().map(edit_feedback::refusal));
            if let Some(target) = self.snap_target
                && self.snapped.is_some()
                && self.feedback.is_some()
            {
                label = label.map(|label| format!("{label}\n{}", target.label(project)));
            }
            if let Some(label) = label {
                edit_feedback::paint_label(ui, area, pointer, label);
            }
        }
    }
}
