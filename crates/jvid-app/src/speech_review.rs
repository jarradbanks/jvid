//! Speech-cut proposals are view state until a validated, atomic Apply.

use std::path::PathBuf;
use std::time::SystemTime;

use jvid_core::history::{Command, History};
use jvid_core::model::{ClipId, Project};
use jvid_core::time::Time;

pub(crate) type Span = (ClipId, Time, Time);

/// Includes the project session: opening another project can reuse history revisions/IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Revision {
    pub session: u64,
    pub history: u64,
    pub transcript: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::history::History;
    use jvid_core::time::FLICKS_PER_SECOND as S;

    fn rev() -> Revision {
        Revision {
            session: 1,
            history: 0,
            transcript: 0,
        }
    }

    #[test]
    fn proposals_cancel_none_all_edges_and_undo() {
        let (p, mic, game) = crate::subtitles::test_support::two_streams(false);
        let mut plan = Plan::begin(&p, rev());
        plan.complete(vec![
            (mic, Time(S), Time(3 * S)),
            (game, Time(2 * S), Time(3 * S)),
            (mic, Time(5 * S), Time(7 * S)),
        ])
        .unwrap();
        assert_eq!(plan.saved(), Time(2 * S));
        assert_eq!(plan.audition(&p, rev(), false).unwrap(), p);
        let after = plan.audition(&p, rev(), true).unwrap();
        assert_eq!(after.sequence().duration(), Time(4 * S));
        let mut h = History::new(p.clone());
        assert_eq!(
            h.project(),
            &p,
            "analysis and audition never mutate history"
        );
        h.apply(plan.command(&p, rev()).unwrap().unwrap()).unwrap();
        assert_eq!(h.project(), &after);
        assert!(h.undo());
        assert_eq!(h.project(), &p);
        assert!(!h.can_undo());
        for proposal in &mut plan.proposals {
            proposal.accepted = false;
        }
        assert_eq!(plan.saved(), Time(0));
        assert_eq!(plan.command(&p, rev()).unwrap(), None);
        drop(plan);
        assert_eq!(h.project(), &p, "cancellation discards view state only");
    }

    #[test]
    fn edits_session_transcript_and_source_replacement_invalidate() {
        let (p, mic, _) = crate::subtitles::test_support::two_streams(false);
        let mut plan = Plan::begin(&p, rev());
        let mut h = History::new(p.clone());
        h.apply(Command::SetInOut {
            in_point: Some(Time(S)),
            out_point: None,
        })
        .unwrap();
        plan.complete(vec![(mic, Time(3 * S), Time(4 * S))])
            .unwrap();
        assert_eq!(plan.command(h.project(), rev()), Err(Refusal::Stale));
        for revision in [
            Revision {
                session: 2,
                ..rev()
            },
            Revision {
                history: 1,
                ..rev()
            },
            Revision {
                transcript: 1,
                ..rev()
            },
        ] {
            assert_eq!(plan.command(&p, revision), Err(Refusal::Stale));
        }
        std::fs::write(&p.assets()[0].path, b"replacement").unwrap();
        assert_eq!(plan.command(&p, rev()), Err(Refusal::Stale));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceStamp(PathBuf, Option<(u64, SystemTime)>);

fn sources(project: &Project) -> Vec<SourceStamp> {
    project
        .assets()
        .iter()
        .filter(|a| a.solid.is_none() && a.text.is_none() && a.sequence.is_none())
        .map(|a| {
            let stamp = std::fs::metadata(&a.path)
                .ok()
                .and_then(|m| Some((m.len(), m.modified().ok()?)));
            SourceStamp(a.path.clone(), stamp)
        })
        .collect()
}

#[derive(Clone, Debug)]
pub(crate) struct Proposal {
    pub span: Span,
    pub accepted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    Stale,
    Invalid,
    Locked,
}

/// Source/project snapshot is captured before analysis starts, never when it completes.
pub(crate) struct Plan {
    project: Project,
    revision: Revision,
    sources: Vec<SourceStamp>,
    pub proposals: Vec<Proposal>,
}

impl Plan {
    pub fn begin(project: &Project, revision: Revision) -> Self {
        Self {
            project: project.clone(),
            revision,
            sources: sources(project),
            proposals: Vec::new(),
        }
    }

    pub fn current(&self, project: &Project, revision: Revision) -> bool {
        self.revision == revision && self.project == *project && self.sources == sources(project)
    }

    /// Reject malformed detector results and clamp edge proposals to actual clip bounds.
    pub fn complete(&mut self, spans: Vec<Span>) -> Result<(), Refusal> {
        let mut proposals = Vec::new();
        for (id, a, b) in spans {
            let (_, clip) = self.project.sequence().clip(id).ok_or(Refusal::Invalid)?;
            if a >= b {
                return Err(Refusal::Invalid);
            }
            let span = (id, a.max(clip.start), b.min(clip.end()));
            if span.1 < span.2 {
                proposals.push(Proposal {
                    span,
                    accepted: true,
                });
            }
        }
        proposals.sort_by_key(|p| (p.span.1, p.span.0));
        proposals.dedup_by_key(|p| p.span);
        self.proposals = proposals;
        Ok(())
    }

    /// Overlapping suggestions from multiple streams remove time only once.
    pub fn ranges(&self) -> Vec<(Time, Time)> {
        let mut spans: Vec<_> = self
            .proposals
            .iter()
            .filter(|p| p.accepted)
            .map(|p| (p.span.1, p.span.2))
            .collect();
        spans.sort_unstable();
        let mut ranges: Vec<(Time, Time)> = Vec::new();
        for (a, b) in spans {
            if let Some(last) = ranges.last_mut()
                && a <= last.1
            {
                last.1 = last.1.max(b);
            } else {
                ranges.push((a, b));
            }
        }
        ranges
    }

    pub fn saved(&self) -> Time {
        Time(self.ranges().iter().map(|(a, b)| (*b - *a).0).sum())
    }

    /// Same validated command drives audition and Apply. Existing sequence extraction cuts
    /// all tracks (captions included), preserving sync even for unlinked background media.
    pub fn command(
        &self,
        project: &Project,
        revision: Revision,
    ) -> Result<Option<Command>, Refusal> {
        if !self.current(project, revision) {
            return Err(Refusal::Stale);
        }
        let ranges = self.ranges();
        let Some((first, _)) = ranges.first() else {
            return Ok(None);
        };
        if project
            .sequence()
            .tracks()
            .iter()
            .any(|t| t.locked && t.clips().iter().any(|c| c.end() > *first))
        {
            return Err(Refusal::Locked);
        }
        let mut commands = Vec::new();
        for &(a, b) in ranges.iter().rev() {
            commands.push(Command::SetInOut {
                in_point: Some(a),
                out_point: Some(b),
            });
            commands.push(Command::RemoveRange { ripple: true });
        }
        // Analysis and temporary extraction marks never replace the user's marks.
        commands.push(Command::SetInOut {
            in_point: project.sequence().in_point(),
            out_point: project.sequence().out_point(),
        });
        let command = Command::Batch(commands);
        let mut candidate = History::new(project.clone());
        candidate
            .apply(command.clone())
            .map_err(|_| Refusal::Invalid)?;
        Ok(Some(command))
    }

    pub fn audition(
        &self,
        project: &Project,
        revision: Revision,
        after: bool,
    ) -> Result<Project, Refusal> {
        let command = self.command(project, revision)?;
        let mut candidate = History::new(project.clone());
        if after && let Some(command) = command {
            candidate.apply(command).map_err(|_| Refusal::Invalid)?;
        }
        Ok(candidate.project().clone())
    }
}
