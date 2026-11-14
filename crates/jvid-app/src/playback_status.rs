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
    last: Option<(i64, i32, bool)>,
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
        if let Some((last, prior_speed, was_ready)) = self.last {
            if index == last && speed == prior_speed {
                self.last = Some((index, speed, was_ready || ready));
                return;
            }
            if speed != prior_speed || (index - last).signum() != i64::from(speed.signum()) {
                self.reset();
            } else {
                // Commit only after the clock has left this observed frame's deadline.
                self.recent.push_back(!was_ready);
                self.misses += usize::from(!was_ready);
            }
        }
        self.last = Some((index, speed, ready));
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

use std::path::PathBuf;

use egui::Ui;
use j_ui::tokens::color;
use jvid_media::proxy::Status as ProxyStatus;

use crate::i18n::tr;

impl Observation {
    fn source_label(self) -> &'static str {
        match (self.proxies > 0, self.originals > 0) {
            (true, true) => tr!("preview.status_mixed"),
            (true, false) => tr!("preview.status_proxy"),
            (false, _) => tr!("preview.status_original"),
        }
    }

    pub fn label(self) -> &'static str {
        match self.picture {
            Picture::Idle => tr!("preview.status_idle"),
            Picture::Loading => tr!("preview.status_loading"),
            Picture::Failed => tr!("preview.status_failed"),
            Picture::Approximate => tr!("preview.status_approximate"),
            Picture::Ready => self.source_label(),
        }
    }
}

/// A passive caption beside the Proxies checkbox: which media the viewer shows, shown
/// while proxies are on or something needs attention, with the details in its tooltip.
/// A failed proxy adds Retry Proxies; returns the sources it asked to retry.
pub fn label(
    ui: &mut Ui,
    observation: Observation,
    health: &Health,
    quality: &str,
    jobs: &[(PathBuf, ProxyStatus)],
    proxies: bool,
) -> Vec<PathBuf> {
    let failed = jobs
        .iter()
        .any(|(_, s)| matches!(s, ProxyStatus::Failed(_) | ProxyStatus::Cancelled));
    let generating: Vec<_> = jobs
        .iter()
        .filter_map(|(_, state)| match state {
            ProxyStatus::Generating(progress) => Some(*progress),
            _ => None,
        })
        .collect();
    if !generating.is_empty() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
    let warning = health.troubled() || failed || observation.picture == Picture::Failed;
    if !proxies && !warning {
        return Vec::new();
    }
    let ink = if warning {
        color::amber()
    } else {
        color::ink_3()
    };
    let caption = if generating.is_empty() {
        observation.label().into()
    } else {
        format!("{} …", observation.label())
    };
    let galley = j_ui::widgets::fit_text(ui, &caption, j_ui::theme::body(), ink, 88.0);
    let (rect, response) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    ui.painter().galley(rect.min, galley, ink);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, observation.label())
    });
    response.on_hover_ui(|ui| {
        ui.set_max_width(320.0);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        ui.label(tr!(
            "preview.status_quality",
            quality = quality,
            source = observation.source_label()
        ));
        ui.label(tr!("preview.status_export"));
        if health.troubled() {
            ui.colored_label(
                color::amber(),
                tr!("preview.status_trouble", count = health.misses()),
            );
        }
        if observation.picture == Picture::Failed {
            ui.label(tr!("preview.status_decode_failed"));
        }
        if !generating.is_empty() {
            let progress =
                (generating.iter().sum::<f32>() / generating.len() as f32 * 100.0).round() as u32;
            ui.label(tr!(
                "preview.status_generating",
                count = generating.len(),
                progress = progress
            ));
        }
        let ready = jobs
            .iter()
            .filter(|(_, s)| *s == ProxyStatus::Ready)
            .count();
        if ready > 0 {
            ui.label(tr!("preview.status_ready", count = ready));
        }
        for (path, state) in jobs {
            match state {
                ProxyStatus::Failed(error) => {
                    ui.label(tr!(
                        "preview.status_proxy_failed",
                        source = path.display(),
                        error = error
                    ));
                }
                ProxyStatus::Cancelled => {
                    ui.label(tr!("preview.status_cancelled"));
                }
                _ => {}
            }
        }
    });
    let mut retry = Vec::new();
    if failed
        && ui
            .add(j_ui::widgets::Button::ghost(tr!("preview.status_retry")))
            .clicked()
    {
        retry.extend(
            jobs.iter()
                .filter(|(_, state)| {
                    matches!(state, ProxyStatus::Failed(_) | ProxyStatus::Cancelled)
                })
                .map(|(path, _)| path.clone()),
        );
    }
    retry
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn label_is_passive_shows_with_proxies_or_trouble_and_retries_failures() {
        let mut fresh = true;
        let jobs = [(
            PathBuf::from("a long source name.mov"),
            ProxyStatus::Failed("The decoder could not read the source".into()),
        )];
        let ready = Observation {
            picture: Picture::Ready,
            originals: 1,
            proxies: 0,
        };
        let mut h = Harness::builder().with_size([240.0, 120.0]).build_ui_state(
            move |ui, (proxies, failed, retry): &mut (bool, bool, Vec<PathBuf>)| {
                if j_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                let jobs = if *failed { &jobs[..] } else { &[] };
                ui.horizontal(|ui| {
                    retry.extend(label(ui, ready, &Health::default(), "Full", jobs, *proxies));
                });
            },
            (false, false, Vec::new()),
        );
        h.run();
        // Proxies off and nothing wrong: nothing to say.
        assert!(h.query_by_label(tr!("preview.status_original")).is_none());
        h.state_mut().0 = true;
        h.run();
        h.get_by_role_and_label(egui::accesskit::Role::Label, tr!("preview.status_original"));
        assert!(
            h.query_by_label(tr!("preview.status")).is_none(),
            "no status button"
        );
        h.state_mut().1 = true;
        h.run();
        h.get_by_label(tr!("preview.status_retry")).click();
        h.run();
        assert_eq!(h.state().2.len(), 1);
    }

    #[test]
    fn playback_status_ui_evidence() {
        let Some(dir) = std::env::var_os("JVID_PLAYBACK_EVIDENCE_DIR").map(PathBuf::from) else {
            return;
        };
        if !j_ui::test_support::gpu() {
            return;
        }
        std::fs::create_dir_all(&dir).unwrap();
        for (name, observation, jobs) in [
            (
                "original",
                Observation {
                    picture: Picture::Ready,
                    originals: 1,
                    proxies: 0,
                },
                vec![],
            ),
            (
                "proxy",
                Observation {
                    picture: Picture::Ready,
                    originals: 0,
                    proxies: 1,
                },
                vec![(PathBuf::from("recording.mov"), ProxyStatus::Ready)],
            ),
            (
                "generating",
                Observation {
                    picture: Picture::Ready,
                    originals: 1,
                    proxies: 0,
                },
                vec![(
                    PathBuf::from("recording.mov"),
                    ProxyStatus::Generating(0.42),
                )],
            ),
            (
                "failed",
                Observation {
                    picture: Picture::Ready,
                    originals: 1,
                    proxies: 0,
                },
                vec![(
                    PathBuf::from("a long source filename.mov"),
                    ProxyStatus::Failed("The proxy decoder could not read this file".into()),
                )],
            ),
        ] {
            let mut fresh = true;
            let mut h = Harness::builder()
                .with_size([360.0, 540.0])
                .wgpu()
                .build_ui(move |ui| {
                    if j_ui::test_support::install_theme(ui, &mut fresh) {
                        return;
                    }
                    ui.horizontal(|ui| {
                        label(
                            ui,
                            observation,
                            &Health::default(),
                            tr!("preview.full"),
                            &jobs,
                            true,
                        );
                    });
                });
            h.run_steps(4);
            h.get_by_label(observation.label()).hover();
            h.run_steps(4);
            h.render()
                .unwrap()
                .save(dir.join(format!("{name}.png")))
                .unwrap();
        }
    }

    #[test]
    fn trouble_counts_observed_frames_recovers_and_resets_for_audio_pause_seek() {
        let mut health = Health::default();
        health.observe(0, 1, true, false);
        health.observe(0, 1, true, false);
        assert_eq!(health.misses(), 0, "current deadline has not passed");
        assert!(!health.troubled());
        for index in [5, 9] {
            health.observe(index, 1, true, false);
        }
        health.observe(10, 1, true, true);
        assert_eq!(
            health.misses(),
            3,
            "unobserved clock frames are not counted"
        );
        assert!(health.troubled());
        for index in 11..41 {
            health.observe(index, 1, true, true);
        }
        assert_eq!(health.misses(), 0);
        health.observe(41, 1, true, false);
        health.observe(42, 1, false, true);
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
        health.observe(2, -1, true, false);
        assert!(health.troubled());
        health.observe(4, 1, true, true);
        assert_eq!(health.misses(), 0, "direction change starts a fresh sample");
        health.observe(5, 1, true, false);
        health.observe(5, 1, true, true);
        health.observe(6, 1, true, true);
        assert_eq!(
            health.misses(),
            0,
            "a decode recovered within its frame is on time"
        );
    }
}
