//! Export-specific status; generic job scheduling remains unaware of export phases.
use std::sync::{Arc, Mutex};
use std::time::Instant;

use jvid_jobs::{Ctx, Handle};

use crate::i18n::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    AnalyzingAudio,
    Encoding { pass: u32 },
    Retrying,
    Copying,
    Finalizing,
    Completed,
    Cancelled,
    Failed,
}

impl Phase {
    pub fn label(self) -> String {
        match self {
            Self::Preparing => tr!("export.phase_preparing").into(),
            Self::AnalyzingAudio => tr!("export.phase_analysis").into(),
            Self::Encoding { pass } => tr!("export.phase_encoding", pass = pass),
            Self::Retrying => tr!("export.phase_retry").into(),
            Self::Copying => tr!("export.phase_copying").into(),
            Self::Finalizing => tr!("export.phase_finalizing").into(),
            Self::Completed => tr!("export.done").into(),
            Self::Cancelled => tr!("export.cancelled").into(),
            Self::Failed => tr!("export.failed_short").into(),
        }
    }

    /// Stable identifiers for CLI JSON, independent of display language.
    pub fn id(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::AnalyzingAudio => "analyzing_audio",
            Self::Encoding { .. } => "encoding",
            Self::Retrying => "retrying",
            Self::Copying => "copying",
            Self::Finalizing => "finalizing",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub fn pass(self) -> Option<u32> {
        match self {
            Self::Encoding { pass } => Some(pass),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub phase: Phase,
    /// Fraction of this phase, not a promise about total work or unknown retries.
    pub progress: Option<f32>,
    pub retry: u8,
    pub phase_seconds: f64,
}

impl Snapshot {
    /// ETA only for the current measured phase. Finalization/retry duration is unknown.
    pub fn phase_left(&self) -> Option<f64> {
        self.progress
            .filter(|p| *p >= 0.05 && *p < 1.0)
            .map(|p| self.phase_seconds / p as f64 * (1.0 - p as f64))
    }
}

struct State {
    snapshot: Snapshot,
    started: Instant,
    #[cfg(test)]
    events: Vec<Snapshot>,
}

#[derive(Clone)]
pub(super) struct Progress(Arc<Mutex<State>>);

impl Default for Progress {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(State {
            snapshot: Snapshot {
                phase: Phase::Preparing,
                progress: None,
                retry: 0,
                phase_seconds: 0.0,
            },
            started: Instant::now(),
            #[cfg(test)]
            events: Vec::new(),
        })))
    }
}

impl Progress {
    pub(super) fn snapshot(&self) -> Snapshot {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        Snapshot {
            phase_seconds: state.started.elapsed().as_secs_f64(),
            ..state.snapshot.clone()
        }
    }

    pub(super) fn stage(&self, phase: Phase) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.started = Instant::now();
        state.snapshot.phase = phase;
        state.snapshot.phase_seconds = 0.0;
        state.snapshot.progress = match phase {
            Phase::AnalyzingAudio | Phase::Encoding { .. } | Phase::Copying => Some(0.0),
            Phase::Completed => Some(1.0),
            _ => None,
        };
        if phase == Phase::Retrying {
            state.snapshot.retry += 1;
        }
        #[cfg(test)]
        {
            let snapshot = state.snapshot.clone();
            state.events.push(snapshot);
        }
    }

    fn advance(&self, progress: f32) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.snapshot.progress.is_some() {
            state.snapshot.progress = Some(progress.clamp(0.0, 1.0));
            #[cfg(test)]
            {
                let snapshot = state.snapshot.clone();
                state.events.push(snapshot);
            }
        }
    }

    #[cfg(test)]
    pub(super) fn events(&self) -> Vec<Snapshot> {
        self.0.lock().unwrap().events.clone()
    }
}

/// A submitted export, with generic cancellation and an export-specific status snapshot.
#[derive(Clone)]
pub struct ExportHandle {
    job: Handle,
    pub(super) status: Progress,
}

impl ExportHandle {
    pub(super) fn new(job: Handle, status: Progress) -> Self {
        Self { job, status }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.status.snapshot()
    }
}

impl std::ops::Deref for ExportHandle {
    type Target = Handle;
    fn deref(&self) -> &Handle {
        &self.job
    }
}

// Queue tests and other callers can still supply plain jobs, with an unknown export phase.
impl From<Handle> for ExportHandle {
    fn from(job: Handle) -> Self {
        Self::new(job, Progress::default())
    }
}

pub(super) trait Context {
    fn is_cancelled(&self) -> bool;
    fn set_progress(&self, progress: f32);
    fn set_phase(&self, _phase: Phase) {}
}

impl Context for Ctx {
    fn is_cancelled(&self) -> bool {
        Ctx::is_cancelled(self)
    }
    fn set_progress(&self, progress: f32) {
        Ctx::set_progress(self, progress);
    }
}

pub(super) struct Tracked<'a> {
    pub job: &'a Ctx,
    pub status: &'a Progress,
}

impl Context for Tracked<'_> {
    fn is_cancelled(&self) -> bool {
        self.job.is_cancelled()
    }
    fn set_progress(&self, progress: f32) {
        self.job.set_progress(progress);
        self.status.advance(progress);
    }
    fn set_phase(&self, phase: Phase) {
        self.status.stage(phase);
        self.job
            .set_progress(if phase == Phase::Completed { 1.0 } else { 0.0 });
    }
}
