//! Marker-backed highlight review and immutable export handoff.
//!
//! Review selection is view state. Names and boundaries are ordinary marker commands;
//! consumers capture their own project and export settings alongside this snapshot.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use egui::{Rect, TextureHandle, Ui, pos2, vec2};
use jvid_core::history::{Command, GestureKey, History};
use jvid_core::model::{Marker, MarkerId, Project, Sequence, SequenceId, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use suite_ui::panel::Panel;
use suite_ui::theme::icon;
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{Button, Checkbox, DragField, State, TextInput};

use crate::export::dialog;
use crate::i18n::tr;
use crate::keymap::{Applicability, Keymap};
use crate::media_bin::{Caches, filmstrips::FilmstripRequests};

/// Identity of the reviewed state. The caller increments `project_session` on every new,
/// opened or restored project, even when its history revision starts at zero again.
/// History's revision also covers sequence edits, undo and redo; this deliberately rejects
/// unrelated content edits rather than accidentally exporting a different project state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReviewRevision {
    pub project_session: u64,
    pub sequence: SequenceId,
    pub revision: u64,
}

impl ReviewRevision {
    pub fn current(project_session: u64, history: &History) -> Self {
        Self {
            project_session,
            sequence: history.project().sequence().id(),
            revision: history.rev(),
        }
    }
}

/// One independently exported highlight. Overlapping markers remain separate outputs.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedHighlight {
    marker: Marker,
    span: (Time, Time),
}

impl ResolvedHighlight {
    pub fn marker(&self) -> &Marker {
        &self.marker
    }

    /// Half-open timeline span, using export dialog's point-marker padding semantics.
    pub fn span(&self) -> (Time, Time) {
        self.span
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionError {
    Stale,
    DifferentProjectOrSequence,
    MissingMarker(MarkerId),
    InvalidSpan(MarkerId),
    InvalidPadding,
}

/// Frozen subset, names and resolved spans, sorted by timeline position then marker ID.
/// Private fields prevent consumers from mutating the selection shared between jobs.
#[derive(Clone, Debug, PartialEq)]
pub struct HighlightSelectionSnapshot {
    revision: ReviewRevision,
    point_padding: (Time, Time),
    highlights: Vec<ResolvedHighlight>,
}

impl HighlightSelectionSnapshot {
    pub fn capture(
        project_session: u64,
        history: &History,
        selected: &HashSet<MarkerId>,
        point_padding: (Time, Time),
    ) -> Result<Self, SelectionError> {
        if point_padding.0 < Time(0) || point_padding.1 < Time(0) {
            return Err(SelectionError::InvalidPadding);
        }
        let sequence = history.project().sequence();
        let mut highlights = Vec::with_capacity(selected.len());
        for marker in sequence
            .markers()
            .iter()
            .filter(|m| selected.contains(&m.id))
        {
            let end = sequence.duration();
            if marker.time < Time(0)
                || marker.time > end
                || marker.duration < Time(0)
                || marker.duration > end - marker.time
                || marker.time.0.checked_add(point_padding.1.0).is_none()
            {
                return Err(SelectionError::InvalidSpan(marker.id));
            }
            let span = dialog::highlight(marker, point_padding, end);
            if span.0 >= span.1 {
                return Err(SelectionError::InvalidSpan(marker.id));
            }
            highlights.push(ResolvedHighlight {
                marker: marker.clone(),
                span,
            });
        }
        if highlights.len() != selected.len() {
            let present: HashSet<_> = highlights.iter().map(|h| h.marker.id).collect();
            return Err(SelectionError::MissingMarker(
                *selected.difference(&present).min_by_key(|id| id.0).unwrap(),
            ));
        }
        highlights.sort_by_key(|h| (h.marker.time, h.marker.id.0));
        Ok(Self {
            revision: ReviewRevision::current(project_session, history),
            point_padding,
            highlights,
        })
    }

    pub fn revision(&self) -> ReviewRevision {
        self.revision
    }

    pub fn highlights(&self) -> &[ResolvedHighlight] {
        &self.highlights
    }

    pub fn point_padding(&self) -> (Time, Time) {
        self.point_padding
    }

    /// Validate immediately before capturing/enqueueing a job's project snapshot.
    pub fn validate(&self, project_session: u64, history: &History) -> Result<(), SelectionError> {
        let current = ReviewRevision::current(project_session, history);
        if self.revision.project_session != current.project_session
            || self.revision.sequence != current.sequence
        {
            return Err(SelectionError::DifferentProjectOrSequence);
        }
        if self.revision != current {
            return Err(SelectionError::Stale);
        }
        Ok(())
    }

    /// Explicitly accept edits in the same project/sequence, dropping removed IDs and
    /// resolving the remaining markers again. Never transfers IDs into another project.
    pub fn reconcile(
        &self,
        project_session: u64,
        history: &History,
    ) -> Result<Self, SelectionError> {
        if self.revision.project_session != project_session
            || self.revision.sequence != history.project().sequence().id()
        {
            return Err(SelectionError::DifferentProjectOrSequence);
        }
        let present: HashSet<_> = history
            .project()
            .sequence()
            .markers()
            .iter()
            .map(|m| m.id)
            .collect();
        let selected = self
            .highlights
            .iter()
            .map(|h| h.marker.id)
            .filter(|id| present.contains(id))
            .collect();
        Self::capture(project_session, history, &selected, self.point_padding)
    }
}

/// Content edits pass through the shell's existing history path. Export is a request to
/// run the registered `export_highlights` action, never a second action executor.
#[derive(Default)]
pub(crate) struct Output {
    pub command: Option<(Command, Option<GestureKey>)>,
    pub audition: Option<(Time, Time)>,
    pub export: bool,
}

/// Panel-local view state and thumbnail subscriptions. Nothing here is serialized into
/// project content or a workspace snapshot.
pub struct HighlightReview {
    selected: HashSet<MarkerId>,
    rows: Vec<Marker>,
    shown: Option<ReviewRevision>,
    reviewed: Option<ReviewRevision>,
    before: f64,
    after: f64,
    caches: Option<Arc<Caches>>,
    requests: FilmstripRequests,
    strips: HashMap<PathBuf, Option<Vec<TextureHandle>>>,
    audition: Option<(ReviewRevision, (Time, Time), Time)>,
}

impl Default for HighlightReview {
    fn default() -> Self {
        Self {
            selected: HashSet::new(),
            rows: Vec::new(),
            shown: None,
            reviewed: None,
            before: 15.0,
            after: 5.0,
            caches: None,
            requests: FilmstripRequests::default(),
            strips: HashMap::new(),
            audition: None,
        }
    }
}

impl HighlightReview {
    pub(crate) fn new(caches: Arc<Caches>) -> Self {
        Self {
            caches: Some(caches),
            ..Self::default()
        }
    }

    pub(crate) fn reset_session(&mut self) {
        *self = Self {
            caches: self.caches.clone(),
            ..Self::default()
        };
    }

    /// Refresh row data after content changes, but require explicit review before export.
    /// New markers are selected by default, matching the established Highlights dialog.
    pub(crate) fn sync(&mut self, revision: ReviewRevision, sequence: &Sequence) {
        if self.shown == Some(revision) {
            return;
        }
        let same_sequence = self.shown.is_some_and(|old| {
            old.project_session == revision.project_session && old.sequence == revision.sequence
        });
        if !same_sequence {
            self.selected.clear();
            self.rows.clear();
            self.reviewed = Some(revision);
        }
        let known: HashSet<_> = self.rows.iter().map(|m| m.id).collect();
        self.rows = sequence.markers().to_vec();
        self.rows.sort_by_key(|m| (m.time, m.id.0));
        let present: HashSet<_> = self.rows.iter().map(|m| m.id).collect();
        self.selected.retain(|id| present.contains(id));
        self.selected.extend(present.difference(&known));
        self.shown = Some(revision);
    }

    pub fn selected(&self) -> &HashSet<MarkerId> {
        &self.selected
    }

    pub fn is_stale(&self, revision: ReviewRevision) -> bool {
        self.reviewed != Some(revision)
    }

    pub fn snapshot(
        &self,
        project_session: u64,
        history: &History,
    ) -> Result<HighlightSelectionSnapshot, SelectionError> {
        if self.is_stale(ReviewRevision::current(project_session, history)) {
            return Err(SelectionError::Stale);
        }
        HighlightSelectionSnapshot::capture(
            project_session,
            history,
            &self.selected,
            self.padding(),
        )
    }

    fn padding(&self) -> (Time, Time) {
        let time = |s: f64| Time((s * FLICKS_PER_SECOND as f64).round() as i64);
        (time(self.before), time(self.after))
    }

    pub(crate) fn begin_audition(&mut self, revision: ReviewRevision, span: (Time, Time)) {
        self.audition = Some((revision, span, span.0));
    }

    /// Runs even when this panel is hidden. An edit, navigation out of the span or its end
    /// stops audition; never writes in/out points, clip links or other project content.
    pub(crate) fn audition_stop(
        &mut self,
        revision: ReviewRevision,
        playhead: Time,
        playing: bool,
    ) -> Option<Time> {
        let (started, (start, end), last) = self.audition?;
        if started != revision || playhead < start || playhead >= end || playhead < last || !playing
        {
            self.audition = None;
            return Some(if started == revision && playhead >= end {
                end
            } else {
                playhead
            });
        }
        self.audition = Some((started, (start, end), playhead));
        None
    }

    pub(crate) fn show_docked(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        revision: ReviewRevision,
        focused: bool,
        export: Applicability,
        keymap: &Keymap,
    ) -> Output {
        self.sync(revision, project.sequence());
        self.requests.on_focus(ui.ctx(), self.caches.as_deref());
        self.requests.poll(ui.ctx(), &mut self.strips);
        let mut output = Output::default();
        Panel::body()
            .focused(focused)
            .embedded(true)
            .show(ui, |ui| {
                if self.rows.is_empty() {
                    suite_ui::empty_state::EmptyState::new(
                        icon::PLAY,
                        tr!("highlights.review"),
                        tr!("highlights.review_empty"),
                    )
                    .show(ui);
                    return;
                }
                if self.is_stale(revision) {
                    ui.colored_label(color::amber(), tr!("highlights.stale"));
                    if ui
                        .push_id("review_refresh", |ui| {
                            suite_ui::dialog::action_footer(
                                ui,
                                [Button::secondary(tr!("highlights.refresh"))],
                            )[0]
                            .clicked()
                        })
                        .inner
                    {
                        self.reviewed = Some(revision);
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add(Button::secondary(tr!("action.select_all")))
                        .clicked()
                    {
                        self.selected = self.rows.iter().map(|m| m.id).collect();
                    }
                    if ui
                        .add(Button::secondary(tr!("highlights.select_none")))
                        .clicked()
                    {
                        self.selected.clear();
                    }
                });
                for (label, value) in [
                    (tr!("export.before"), &mut self.before),
                    (tr!("export.after"), &mut self.after),
                ] {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(label);
                        ui.add(
                            DragField::new(value)
                                .range(0.0..=600.0)
                                .decimals(1)
                                .speed(0.1)
                                .width(96.0)
                                .suffix(tr!("silence.seconds_suffix")),
                        );
                    });
                }
                let state = if export.is_enabled() {
                    State::Live
                } else {
                    State::Disabled
                };
                let name = tr!("highlights.export_selected");
                let binding = crate::keymap::ACTIONS
                    .iter()
                    .position(|a| a.id == "export_highlights")
                    .and_then(|index| keymap.bindings[index].as_ref());
                let label = binding.map_or_else(
                    || name.to_owned(),
                    |binding| {
                        tr!(
                            "preview.with_key",
                            action = name,
                            key = ui.ctx().format_shortcut(binding)
                        )
                    },
                );
                let response = ui
                    .push_id("review_export", |ui| {
                        suite_ui::dialog::action_footer(ui, [Button::primary(&label).state(state)])
                            [0]
                        .clone()
                    })
                    .inner;
                output.export = response.clicked();
                if let Some(reason) = export.reason() {
                    response.on_hover_text(reason);
                }
                ui.separator();
                let height = size::CONTROL * 5.0 + space::S16;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show_rows(ui, height, self.rows.len(), |ui, range| {
                        for index in range {
                            let marker = self.rows[index].clone();
                            ui.push_id(marker.id.0, |ui| {
                                ui.set_min_height(height - space::S4);
                                self.row(ui, project, revision, &marker, &mut output);
                            });
                        }
                    });
            });
        output
    }

    fn row(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        revision: ReviewRevision,
        marker: &Marker,
        output: &mut Output,
    ) {
        let sequence = project.sequence();
        let span = dialog::highlight(marker, self.padding(), sequence.duration());
        let valid = Time(0) <= span.0 && span.0 < span.1 && span.1 <= sequence.duration();
        let mut edited = marker.clone();
        let mut changed = false;
        let mut gesture = ui.id().value();
        ui.horizontal(|ui| {
            let mut selected = self.selected.contains(&marker.id);
            let response = ui.add(Checkbox::new(&mut selected, ""));
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Checkbox,
                    true,
                    selected,
                    tr!("highlights.choose"),
                )
            });
            if response.on_hover_text(tr!("highlights.choose")).changed() {
                if selected {
                    self.selected.insert(marker.id);
                } else {
                    self.selected.remove(&marker.id);
                }
            }
            self.thumbnail(ui, project, span.0.max(Time(0)));
            ui.vertical(|ui| {
                let name = ui.add(TextInput::new(&mut edited.name).width(ui.available_width()));
                name.clone().widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::TextEdit,
                        true,
                        tr!("timeline.marker_name"),
                    )
                });
                changed |= name.changed();
                if name.changed() {
                    gesture = name.id.value();
                }
                let duration = (span.1 - span.0).0.max(0) as f64 / FLICKS_PER_SECOND as f64;
                ui.label(tr!(
                    "highlights.duration",
                    seconds = format!("{duration:.2}")
                ));
            });
        });
        let seconds = |t: Time| t.0 as f64 / FLICKS_PER_SECOND as f64;
        let (mut start, mut end) = (seconds(marker.time), seconds(marker.time + marker.duration));
        let mut start_changed = false;
        let mut end_changed = false;
        for (label, value, changed) in [
            (tr!("highlights.start"), &mut start, &mut start_changed),
            (tr!("highlights.end"), &mut end, &mut end_changed),
        ] {
            ui.horizontal(|ui| {
                ui.label(label);
                let response = ui.add(
                    DragField::new(value)
                        .range(0.0..=seconds(sequence.duration()))
                        .decimals(3)
                        .speed(0.01)
                        .width(72.0),
                );
                response.clone().widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, label)
                });
                *changed = response.changed();
                if *changed {
                    gesture = response.id.value();
                }
            });
        }
        if start_changed || end_changed {
            if marker.duration == Time(0) && start_changed && !end_changed {
                end = start;
            }
            if start > end {
                if start_changed {
                    end = start;
                } else {
                    start = end;
                }
            }
            let rate = project.settings().frame_rate;
            let snap = |s: f64| {
                rate.time_floor(
                    rate.index_nearest(Time((s * FLICKS_PER_SECOND as f64).round() as i64)),
                )
                .clamp(Time(0), sequence.duration())
            };
            edited.time = snap(start);
            edited.duration = snap(end) - edited.time;
            changed = true;
        }
        if changed && edited != *marker {
            output.command = Some((Command::SetMarker(edited), Some(gesture)));
        }
        let state = if valid && !self.is_stale(revision) {
            State::Live
        } else {
            State::Disabled
        };
        if ui
            .add(Button::secondary(tr!("highlights.audition")).state(state))
            .clicked()
        {
            output.audition = Some(span);
        }
    }

    fn thumbnail(&mut self, ui: &mut Ui, project: &Project, time: Time) {
        let (rect, _) = ui.allocate_exact_size(vec2(64.0, 36.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 4.0, color::field());
        let sequence = project.sequence();
        let Some((_, clip)) = sequence.clips_at(time).find(|(id, clip)| {
            clip.enabled
                && sequence
                    .track(*id)
                    .is_some_and(|t| t.kind == TrackKind::Video && !t.hidden)
        }) else {
            return;
        };
        let Some(asset) = project.asset(clip.asset) else {
            return;
        };
        let Some(probe) = asset.probe.as_ref() else {
            return;
        };
        self.requests.refresh(
            ui.ctx(),
            self.caches.as_deref(),
            &asset.path,
            probe.duration,
            &mut self.strips,
            true,
        );
        let Some(Some(textures)) = self.strips.get(&asset.path) else {
            return;
        };
        let step = FLICKS_PER_SECOND.max(probe.duration.0 / jvid_media::thumbnails::STRIP_MAX);
        let index = (clip.source_at(time).0 / step).max(0) as usize;
        let Some(texture) = textures.get(index) else {
            return;
        };
        ui.painter().image(
            texture.id(),
            rect,
            Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::history::{Command, Created};
    use jvid_core::model::{NewClip, Project, TrackKind};
    use jvid_core::time::FLICKS_PER_SECOND as S;

    fn secs(s: i64) -> Time {
        Time(s * S)
    }

    fn fixture() -> History {
        let mut h = History::new(Project::default());
        let Created::Asset(asset) = h.apply(Command::AddAsset("review.mp4".into())).unwrap() else {
            panic!("asset")
        };
        let Created::Track(track) = h
            .apply(Command::AddTrack {
                kind: TrackKind::Video,
                name: "V1".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        h.apply(Command::AddClip {
            track,
            clip: NewClip {
                asset,
                source_in: secs(0),
                source_out: secs(60),
                start: secs(0),
                link: None,
            },
        })
        .unwrap();
        for (start, duration, name) in [(1, 0, "Point"), (3, 8, "Manual"), (5, 8, "Detected")] {
            h.apply(Command::AddMarker {
                time: secs(start),
                duration: secs(duration),
                name: name.into(),
            })
            .unwrap();
        }
        h
    }

    #[test]
    fn subset_freezes_names_spans_and_overlaps_with_export_padding() {
        let h = fixture();
        let m = h.project().sequence().markers();
        let selected = HashSet::from([m[0].id, m[2].id]);
        let snapshot =
            HighlightSelectionSnapshot::capture(7, &h, &selected, (secs(15), secs(5))).unwrap();
        let got: Vec<_> = snapshot
            .highlights()
            .iter()
            .map(|h| (h.marker().name.as_str(), h.span()))
            .collect();
        assert_eq!(
            got,
            [
                ("Point", (secs(0), secs(6))),
                ("Detected", (secs(5), secs(13)))
            ]
        );
        assert_eq!(snapshot.validate(7, &h), Ok(()));
        assert_eq!(
            snapshot.validate(8, &h),
            Err(SelectionError::DifferentProjectOrSequence)
        );
    }

    #[test]
    fn edits_reject_stale_snapshots_reconciliation_drops_deleted_ids() {
        let mut h = fixture();
        let m = h.project().sequence().markers();
        let selected = HashSet::from([m[1].id, m[2].id]);
        let snapshot =
            HighlightSelectionSnapshot::capture(1, &h, &selected, (secs(15), secs(5))).unwrap();
        let mut marker = m[1].clone();
        marker.time = secs(20);
        marker.name = "Edited".into();
        h.apply(Command::SetMarker(marker)).unwrap();
        assert_eq!(snapshot.validate(1, &h), Err(SelectionError::Stale));
        let remove = snapshot.highlights()[1].marker().id;
        h.apply(Command::RemoveMarker(remove)).unwrap();
        let reconciled = snapshot.reconcile(1, &h).unwrap();
        assert_eq!(reconciled.highlights().len(), 1);
        assert_eq!(reconciled.highlights()[0].span(), (secs(20), secs(28)));
        assert_eq!(snapshot.highlights()[0].marker().name, "Manual");
        assert_eq!(
            snapshot.reconcile(2, &h),
            Err(SelectionError::DifferentProjectOrSequence)
        );
        assert!(h.undo());
        assert!(h.undo());
        assert_eq!(snapshot.validate(1, &h), Ok(()));
    }

    #[test]
    fn invalid_selection_never_silently_exports_another_range() {
        let h = fixture();
        let missing = HashSet::from([MarkerId(u64::MAX)]);
        assert_eq!(
            HighlightSelectionSnapshot::capture(1, &h, &missing, (secs(1), secs(1))),
            Err(SelectionError::MissingMarker(MarkerId(u64::MAX)))
        );
        assert_eq!(
            HighlightSelectionSnapshot::capture(1, &h, &HashSet::new(), (secs(-1), secs(1))),
            Err(SelectionError::InvalidPadding)
        );
        let point = HashSet::from([h.project().sequence().markers()[0].id]);
        assert!(matches!(
            HighlightSelectionSnapshot::capture(1, &h, &point, (secs(0), secs(0))),
            Err(SelectionError::InvalidSpan(_))
        ));
    }

    #[test]
    fn selected_dialog_requests_match_snapshot_and_reject_stale_handoff() {
        let mut h = fixture();
        let markers = h.project().sequence().markers();
        let selected = HashSet::from([markers[0].id, markers[2].id]);
        let snapshot =
            HighlightSelectionSnapshot::capture(3, &h, &selected, (secs(2), secs(3))).unwrap();
        let mut dialog = dialog::ExportDialog::default();
        dialog.set_path("/exports/review.mp4".into());
        dialog
            .open_highlight_selection(&snapshot, 3, &h, "Review")
            .unwrap();
        let requests = dialog.highlights(h.project().sequence(), "Review");
        assert_eq!(requests.len(), 2);
        for (request, highlight) in requests.iter().zip(snapshot.highlights()) {
            assert_eq!(request.range, highlight.span());
            assert!(
                request
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(&highlight.marker().name)
            );
        }
        dialog
            .open_highlight_selection(&snapshot, 3, &h, "Review")
            .unwrap();
        assert!(dialog.review_is_current(3, &h));
        h.apply(Command::SetInOut {
            in_point: Some(secs(40)),
            out_point: Some(secs(50)),
        })
        .unwrap();
        assert!(!dialog.review_is_current(3, &h));
        assert!(!dialog.is_open());
        assert_eq!(
            dialog.open_highlight_selection(&snapshot, 3, &h, "Review"),
            Err(SelectionError::Stale)
        );
    }

    fn harness(
        history: History,
        review: HighlightReview,
        width: f32,
    ) -> egui_kittest::Harness<'static, (History, HighlightReview)> {
        let mut fresh = true;
        let mut h = egui_kittest::Harness::builder()
            .with_size([width, 640.0])
            .build_ui_state(
                move |ui, state: &mut (History, HighlightReview)| {
                    if suite_ui::test_support::install_theme(ui, &mut fresh) {
                        return;
                    }
                    if ui.input(|i| i.pointer.any_pressed()) {
                        state.0.end_gesture();
                    }
                    let revision = ReviewRevision::current(1, &state.0);
                    let output = state.1.show_docked(
                        ui,
                        state.0.project(),
                        revision,
                        true,
                        Applicability::Enabled,
                        &Keymap::default(),
                    );
                    if let Some((command, gesture)) = output.command {
                        if let Some(key) = gesture {
                            state.0.apply_coalesced(key, command).unwrap();
                        } else {
                            state.0.apply(command).unwrap();
                        }
                    }
                },
                (history, review),
            );
        h.run_steps(2);
        h
    }

    #[test]
    fn boundary_editor_undo_reopen_and_view_state_preserve_project_evaluation() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;
        let mut history = fixture();
        let remove: Vec<_> = history
            .project()
            .sequence()
            .markers()
            .iter()
            .filter(|m| m.name != "Manual")
            .map(|m| m.id)
            .collect();
        for id in remove {
            history.apply(Command::RemoveMarker(id)).unwrap();
        }
        let original = history.project().clone();
        let mut h = harness(history, HighlightReview::default(), 240.0);
        h.get_by_role_and_label(Role::SpinButton, "Start").focus();
        h.key_press(egui::Key::Enter);
        h.run();
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.run();
        h.event(egui::Event::Text("4.5".into()));
        h.run();
        h.key_press(egui::Key::Enter);
        h.run();
        let (history, review) = h.state_mut();
        let marker = &history.project().sequence().markers()[0];
        assert_eq!(
            (marker.time, marker.duration),
            (Time(9 * S / 2), Time(13 * S / 2))
        );
        assert!(
            review.snapshot(1, history).is_err(),
            "edits require re-review"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.jvid");
        crate::project_file::save(history.project(), &path).unwrap();
        let reopened = crate::project_file::load(&path).unwrap();
        assert_eq!(
            reopened.sequence().markers(),
            history.project().sequence().markers()
        );
        let ids = HashSet::from([marker.id]);
        let snapshot =
            HighlightSelectionSnapshot::capture(1, history, &ids, (secs(15), secs(5))).unwrap();
        assert_eq!(snapshot.highlights()[0].span(), (Time(9 * S / 2), secs(11)));
        assert_eq!(
            snapshot.validate(2, &History::new(reopened.clone())),
            Err(SelectionError::DifferentProjectOrSequence)
        );
        assert_eq!(
            jvid_core::eval::frame_at(&original, secs(6)),
            jvid_core::eval::frame_at(&reopened, secs(6))
        );
        assert!(history.undo());
        assert_eq!(history.project(), &original);
        assert!(history.redo());
        review.sync(
            ReviewRevision::current(1, history),
            history.project().sequence(),
        );
        let before = history.project().clone();
        review.selected.clear();
        review.before = 1.0;
        review.after = 2.0;
        assert_eq!(history.project(), &before);
        assert_eq!(
            jvid_core::eval::frame_at(history.project(), secs(6)),
            jvid_core::eval::frame_at(&before, secs(6))
        );
    }

    #[test]
    fn audition_stops_at_boundary_after_edit_and_when_playback_rewinds() {
        let h = fixture();
        let revision = ReviewRevision::current(1, &h);
        let mut review = HighlightReview::default();
        review.begin_audition(revision, (secs(3), secs(11)));
        assert_eq!(review.audition_stop(revision, secs(8), true), None);
        assert_eq!(
            review.audition_stop(revision, secs(12), true),
            Some(secs(11))
        );
        review.begin_audition(revision, (secs(3), secs(11)));
        assert_eq!(
            review.audition_stop(
                ReviewRevision {
                    revision: revision.revision + 1,
                    ..revision
                },
                secs(6),
                true
            ),
            Some(secs(6))
        );
        review.begin_audition(revision, (secs(3), secs(11)));
        assert_eq!(review.audition_stop(revision, secs(8), true), None);
        assert_eq!(review.audition_stop(revision, secs(4), true), Some(secs(4)));
        assert_eq!(review.audition_stop(revision, secs(5), true), None);
    }

    #[test]
    fn narrow_review_panel_snapshot() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        harness(fixture(), HighlightReview::default(), 240.0).snapshot("highlight_review_narrow");
    }

    #[test]
    fn panel_registration_preserves_closed_panels_and_legacy_tab_names() {
        use crate::app::Tab;
        use crate::workspaces::{HIGHLIGHTS_PANEL_ID, Panels, Snapshot};
        let panels = Panels::default();
        assert_eq!(panels.tab(HIGHLIGHTS_PANEL_ID), Some(Tab::Highlights));
        let mut dock = egui_dock::DockState::new(vec![
            Tab::Media,
            Tab::Preview,
            Tab::Inspector,
            Tab::Timeline,
        ]);
        let closed = Snapshot::capture(&dock, panels).restore(panels);
        assert!(closed.find_tab(&Tab::Highlights).is_none());
        dock.push_to_first_leaf(Tab::Highlights);
        let opened = Snapshot::capture(&dock, panels).restore(panels);
        assert!(opened.find_tab(&Tab::Highlights).is_some());
        let legacy = toml::to_string(&HashMap::from([("tab", Tab::Transcript)])).unwrap();
        assert_eq!(legacy, "tab = \"Transcript\"\n");
        let keymap =
            Keymap::from_toml("review_highlights = 'Ctrl+Alt+9'\nexport_highlights = 'Ctrl+Alt+8'")
                .unwrap();
        assert_eq!(keymap.shortcut("review_highlights"), "Ctrl+Alt+9");
        assert_eq!(keymap.shortcut("export_highlights"), "Ctrl+Alt+8");
    }

    #[test]
    fn long_source_thumbnail_decode_never_blocks_or_restarts_during_paint() {
        use std::sync::{
            Condvar, Mutex,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        };
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("long.mp4");
        std::fs::write(&source, b"long-source-fixture").unwrap();
        let mut history = History::new(Project::default());
        let Created::Asset(asset) = history.apply(Command::AddAsset(source.clone())).unwrap()
        else {
            panic!("asset")
        };
        history
            .apply(Command::SetAssetProbe {
                id: asset,
                probe: Some(jvid_core::model::ProbeInfo {
                    duration: secs(10 * 60 * 60),
                    width: Some(160),
                    height: Some(90),
                    ..Default::default()
                }),
            })
            .unwrap();
        let Created::Track(track) = history
            .apply(Command::AddTrack {
                kind: TrackKind::Video,
                name: "V1".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        history
            .apply(Command::AddClip {
                track,
                clip: NewClip {
                    asset,
                    source_in: secs(0),
                    source_out: secs(10 * 60 * 60),
                    start: secs(0),
                    link: None,
                },
            })
            .unwrap();
        for n in 0..500 {
            history
                .apply(Command::AddMarker {
                    time: secs(n * 30),
                    duration: secs(10),
                    name: format!("Highlight {n}"),
                })
                .unwrap();
        }
        let mut caches = Caches::new(&jvid_core::settings::Settings {
            cache_dir: Some(dir.path().join("cache")),
            ..Default::default()
        });
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let (started, ready) = mpsc::channel();
        let (wait, count) = (gate.clone(), calls.clone());
        Arc::get_mut(&mut caches).unwrap().thumbs =
            Arc::new(jvid_media::thumbnails::Cache::with_grab(
                dir.path().join("thumbs"),
                u64::MAX,
                Box::new(move |_, _, exact| {
                    assert!(!exact, "long source uses keyframe sampling");
                    if count.fetch_add(1, Ordering::SeqCst) == 0 {
                        started.send(()).unwrap();
                        let (lock, condition) = &*wait;
                        let mut released = lock.lock().unwrap();
                        while !*released {
                            released = condition.wait(released).unwrap();
                        }
                    }
                    Ok(image::RgbaImage::new(160, 90))
                }),
            ));
        let mut h = harness(history, HighlightReview::new(caches.clone()), 240.0);
        let started = ready.recv_timeout(Duration::from_secs(5));
        let reads = crate::media_bin::filmstrips::metadata_reads();
        let requests = caches.filmstrips.requests_started();
        h.run_steps(10);
        let reads_after = crate::media_bin::filmstrips::metadata_reads();
        let requests_after = caches.filmstrips.requests_started();
        let calls_while_painting = calls.load(Ordering::SeqCst);
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        assert!(started.is_ok(), "thumbnail decode did not start");
        assert_eq!(
            reads_after, reads,
            "UI paint performed filesystem metadata reads"
        );
        assert_eq!(requests, 1);
        assert_eq!(requests_after, requests);
        assert_eq!(
            calls_while_painting, 1,
            "decoder stayed blocked while UI painted"
        );
        assert_eq!(h.state().1.rows.len(), 500);
        drop(h);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !caches.jobs.is_idle() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(caches.jobs.is_idle());
    }
}
