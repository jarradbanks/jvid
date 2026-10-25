//! Small explicit state machine: a late progress/result cannot revive a cancelled download.
use crate::network::{Available, Download};

#[derive(Debug, Default)]
pub enum State {
    #[default]
    Idle,
    Checking,
    Current,
    Available(Available),
    Downloading {
        update: Available,
        received: u64,
    },
    Ready(Download),
    Failed(String),
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Work {
    pub unsaved: bool,
    pub recording: bool,
    /// Includes queued exports and cancelled jobs still winding down.
    pub exporting: bool,
    pub other_job: bool,
}

impl Work {
    pub fn can_install(self) -> bool {
        !self.unsaved && !self.recording && !self.exporting && !self.other_job
    }
}

impl State {
    pub fn busy(&self) -> bool {
        matches!(self, Self::Checking | Self::Downloading { .. })
    }

    pub fn progress(&mut self, received: u64) {
        if let Self::Downloading {
            received: current,
            update,
        } = self
        {
            *current = received.min(update.asset.size).max(*current);
        }
    }

    pub fn cancel(&mut self) {
        if let Self::Downloading { update, .. } = self {
            *self = Self::Available(update.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_kind_of_active_work_defers_installation() {
        assert!(Work::default().can_install());
        for work in [
            Work {
                unsaved: true,
                ..Work::default()
            },
            Work {
                recording: true,
                ..Work::default()
            },
            Work {
                exporting: true,
                ..Work::default()
            },
            Work {
                other_job: true,
                ..Work::default()
            },
        ] {
            assert!(!work.can_install(), "{work:?}");
        }
    }
}
