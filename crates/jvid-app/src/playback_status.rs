//! Viewer-only observations. No project changes, decoder requests, or filesystem reads.

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Picture {
    #[default]
    Idle,
    Loading,
    Failed,
    Approximate,
    Ready,
}

/// Sources of the pixels currently retained by the viewer, including keyframe stand-ins.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    pub picture: Picture,
    pub proxies: usize,
    pub originals: usize,
}

/// A bounded sample of presentation opportunities, not an estimate of machine performance.
/// Only distinct clock frames actually observed by the viewer count; skipped UI ticks do not.
#[derive(Default)]
pub struct Health {
    last: Option<(i64, i32)>,
    recent: VecDeque<bool>,
    misses: usize,
}

impl Health {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn observe(&mut self, index: i64, speed: i32, video: bool, ready: bool) {
        if speed == 0 || !video {
            self.reset();
            return;
        }
        if let Some((last, prior_speed)) = self.last {
            if speed != prior_speed || (index - last).signum() != i64::from(speed.signum()) {
                if index == last && speed == prior_speed {
                    return;
                }
                self.reset();
            }
        }
        self.last = Some((index, speed));
        self.recent.push_back(!ready);
        self.misses += usize::from(!ready);
        if self.recent.len() > 30 {
            self.misses -= usize::from(self.recent.pop_front().unwrap());
        }
    }

    pub fn misses(&self) -> usize {
        self.misses
    }

    /// One startup miss is ordinary. Three observed missed deadlines warrant explanation.
    pub fn troubled(&self) -> bool {
        self.misses >= 3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trouble_counts_observed_frames_recovers_and_resets_for_audio_pause_seek() {
        let mut health = Health::default();
        health.observe(0, 1, true, false);
        health.observe(0, 1, true, false);
        assert_eq!(health.misses(), 1);
        assert!(!health.troubled());
        for index in [5, 9] {
            health.observe(index, 1, true, false);
        }
        assert_eq!(health.misses(), 3, "unobserved clock frames are not counted");
        assert!(health.troubled());
        for index in 10..40 {
            health.observe(index, 1, true, true);
        }
        assert_eq!(health.misses(), 0);
        health.observe(40, 1, true, false);
        health.observe(41, 1, false, true);
        assert_eq!(health.misses(), 0, "audio-only has no video deadlines");
        health.observe(42, 1, true, false);
        health.observe(42, 0, true, false);
        assert_eq!(health.misses(), 0);
        health.observe(43, 1, true, false);
        health.reset(); // Explicit seek/refresh.
        assert_eq!(health.misses(), 0);
        health.observe(5, -1, true, false);
        health.observe(4, -1, true, false);
        health.observe(3, -1, true, false);
        assert!(health.troubled());
        health.observe(4, 1, true, true);
        assert_eq!(health.misses(), 0, "direction change starts a fresh sample");
    }
}
