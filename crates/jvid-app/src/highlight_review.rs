//! Marker-backed highlight review and immutable export handoff.
//!
//! Review selection is view state. Names and boundaries are ordinary marker commands;
//! consumers capture their own project and export settings alongside this snapshot.

use std::collections::HashSet;

use jvid_core::history::History;
use jvid_core::model::{Marker, MarkerId, SequenceId};
use jvid_core::time::Time;

use crate::export::dialog;

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
}
