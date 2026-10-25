//! Optional speech models, offered once and available later from Help. Editing never
//! depends on this dialog or a download. Uses the same verified downloader as subtitles.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use egui::RichText;
use jvid_jobs::{Handle, Pool};
use jvid_transcribe::Error;
use jvid_transcribe::models::{self, Model};
use suite_ui::tokens::{color, space};
use suite_ui::widgets::Button;

use crate::i18n::{tr, verbatim};

#[derive(Default)]
pub struct Components {
    pub open: bool,
    jobs: Option<Pool>,
    download: Option<(Handle, Receiver<Result<PathBuf, Error>>)>,
    error: Option<String>,
}

impl Components {
    pub fn offer(&mut self) {
        self.open =
            jvid_paths::config_dir().is_some_and(|d| !d.join("components-offered").is_file());
    }

    /// Includes cancelled jobs until their bounded network wait has unwound. The
    /// updater must wait for this before exiting to run an installer.
    pub fn is_busy(&self) -> bool {
        self.jobs.as_ref().is_some_and(|jobs| !jobs.is_idle())
    }

    fn cancel(&mut self) {
        if let Some((job, _)) = self.download.take() {
            job.cancel();
        }
    }

    fn close(&mut self) {
        self.cancel();
        self.open = false;
        if let Some(dir) = jvid_paths::config_dir().filter(|_| !cfg!(test))
            && let Err(e) = std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::write(dir.join("components-offered"), b"1\n"))
        {
            tracing::warn!("can't remember optional component offer: {e}");
        }
    }

    fn start(&mut self, model: Model, dir: &std::path::Path, ctx: &egui::Context) {
        self.cancel();
        self.error = None;
        let jobs = self.jobs.get_or_insert_with(|| Pool::new(1));
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let job = model.download(jobs, dir, move |result| {
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.download = Some((job, rx));
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if self.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if !self.open {
            return;
        }
        if let Some((_, rx)) = &self.download
            && let Ok(result) = rx.try_recv()
        {
            self.error = result.err().map(|e| e.to_string());
            self.download = None;
        }
        let mut start = None;
        let mut cancel = false;
        let mut close = false;
        let dir = models::dir();
        let modal = egui::Modal::new(egui::Id::new("optional_components")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.heading(tr!("components.title"));
            ui.label(tr!("components.purpose"));
            ui.add_space(space::S8);
            if let Some(dir) = &dir {
                ui.label(tr!("components.location", path = dir.display()));
                ui.add_space(space::S8);
                for (model, name) in [
                    (Model::Base, verbatim("Whisper Base")),
                    (Model::Small, verbatim("Whisper Small")),
                    (Model::LargeV3Turbo, verbatim("Whisper Large v3 Turbo (Q5)")),
                ] {
                    ui.horizontal(|ui| {
                        let size = (model.size() as f64 / 1e6).round().to_string();
                        ui.label(tr!("subtitles.with_size", name = name, size = size));
                        if model.is_downloaded(dir) {
                            ui.label(tr!("components.installed"));
                        } else if self.download.is_none()
                            && ui
                                .add(Button::secondary(tr!("components.download")))
                                .clicked()
                        {
                            start = Some(model);
                        }
                    });
                }
            } else {
                ui.label(tr!("subtitles.no_folder"));
            }
            if let Some((job, _)) = &self.download {
                let progress = job.progress();
                ui.add(egui::ProgressBar::new(progress).text(tr!("subtitles.downloading")));
                cancel = ui.add(Button::secondary(tr!("prompt.cancel"))).clicked();
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            if let Some(error) = &self.error {
                ui.label(RichText::new(error).color(color::red()));
            }
            ui.add_space(space::S8);
            close = ui.add(Button::primary(tr!("settings.done"))).clicked();
        });
        if close || modal.should_close() {
            self.close();
        } else if cancel {
            self.cancel();
        } else if let (Some(model), Some(dir)) = (start, dir) {
            self.start(model, &dir, ctx);
        }
    }
}

impl Drop for Components {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn cancelled_transfer_remains_busy_until_its_worker_exits() {
        let (started_tx, started_rx) = channel();
        let (finish_tx, finish_rx) = channel();
        let jobs = Pool::new(1);
        let job = jobs.submit(
            "blocked-model",
            jvid_jobs::Priority::Interactive,
            move |_| {
                started_tx.send(()).unwrap();
                finish_rx.recv().unwrap();
            },
        );
        let (_, receiver) = channel();
        let mut components = Components {
            jobs: Some(jobs),
            download: Some((job.clone(), receiver)),
            ..Default::default()
        };
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        components.cancel();
        assert!(job.is_cancelled());
        assert!(components.is_busy());
        finish_tx.send(()).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(2);
        while components.is_busy() && std::time::Instant::now() < until {
            std::thread::yield_now();
        }
        assert!(!components.is_busy());
    }

    #[test]
    fn optional_models_offer_sizes_and_can_be_dismissed_without_download() {
        let mut h = egui_kittest::Harness::new_ui_state(
            |ui, c: &mut Components| c.show(ui.ctx()),
            Components {
                open: true,
                ..Default::default()
            },
        );
        h.run();
        h.get_by_label(&tr!(
            "subtitles.with_size",
            name = "Whisper Base",
            size = "148"
        ));
        assert!(h.state().download.is_none());
        h.key_press(egui::Key::Escape);
        h.run();
        assert!(!h.state().open);
        assert!(h.state().download.is_none());
        h.state_mut().open = true;
        h.run();
        h.get_by_label(tr!("components.purpose"));
    }
}
