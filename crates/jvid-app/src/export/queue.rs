//! The render queue: exports run one after another in the background while editing goes on.
//! Each job owns a clone of the project taken when it was queued, so later edits don't reach it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::Ui;
use jvid_core::model::ProbeInfo;
use jvid_jobs::Handle;
use suite_ui::tokens::space;
use suite_ui::widgets::Button;

use super::Error;

struct Job {
    path: PathBuf,
    handle: Handle,
    rx: Receiver<Result<ProbeInfo, Error>>,
    /// `Some` once the job is over.
    result: Option<Result<(), Error>>,
}

#[derive(Default)]
pub struct Queue {
    jobs: Vec<Job>,
    /// The queue window is shown.
    pub open: bool,
}

impl Queue {
    /// Adds a submitted export; refuses one writing to a path a live job is already writing.
    pub fn push(
        &mut self,
        path: PathBuf,
        (handle, rx): (Handle, Receiver<Result<ProbeInfo, Error>>),
    ) -> bool {
        if self.writing(&path) {
            return false;
        }
        self.jobs.push(Job {
            path,
            handle,
            rx,
            result: None,
        });
        true
    }

    /// True while a job not yet over writes to `path`.
    pub fn writing(&self, path: &Path) -> bool {
        self.jobs
            .iter()
            .any(|j| j.result.is_none() && j.path == path)
    }

    /// Collects results; returns the jobs that ended since the last call.
    pub fn poll(&mut self) -> Vec<(PathBuf, Result<(), Error>)> {
        let mut ended = Vec::new();
        for j in self.jobs.iter_mut().filter(|j| j.result.is_none()) {
            let r = match j.rx.try_recv() {
                Ok(r) => r.map(|_| ()),
                Err(TryRecvError::Empty) => continue,
                // A job skipped after cancelling drops its sender; so does one that panicked.
                Err(TryRecvError::Disconnected) if j.handle.is_cancelled() => Err(Error::Cancelled),
                Err(TryRecvError::Disconnected) => Err(Error::Failed("the export crashed".into())),
            };
            ended.push((j.path.clone(), r.clone()));
            j.result = Some(r);
        }
        ended
    }

    /// Progress of the job running now, if any; the export pool runs one job at a time, in order.
    pub fn running(&self) -> Option<f32> {
        let mut live = self.jobs.iter().filter(|j| j.result.is_none());
        live.next().map(|j| j.handle.progress())
    }

    /// Jobs not yet over.
    pub fn pending(&self) -> usize {
        self.jobs.iter().filter(|j| j.result.is_none()).count()
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        let mut open = self.open;
        egui::Window::new("Render Queue")
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| self.list(ui));
        self.open = open;
        if self.pending() > 0 {
            ctx.request_repaint();
        }
    }

    fn list(&mut self, ui: &mut Ui) {
        if self.jobs.is_empty() {
            ui.label("Nothing queued. Use Add to Queue in the export dialog.");
            return;
        }
        let mut remove = None;
        for (i, j) in self.jobs.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::S8;
                let name = j.path.file_name().unwrap_or_default().to_string_lossy();
                ui.label(name).on_hover_text(j.path.display().to_string());
                match &j.result {
                    None => {
                        let p = j.handle.progress();
                        let text = if p > 0.0 {
                            format!("{:.0}%", p * 100.0)
                        } else {
                            "Waiting".into()
                        };
                        ui.add(egui::ProgressBar::new(p).desired_width(140.0).text(text));
                        if ui.add(Button::secondary("Cancel")).clicked() {
                            j.handle.cancel();
                        }
                    }
                    Some(r) => {
                        match r {
                            Ok(()) => ui.label("Done"),
                            Err(Error::Cancelled) => ui.label("Cancelled"),
                            Err(Error::Failed(e)) => ui.label("Failed").on_hover_text(e),
                        };
                        if r.is_ok() && ui.add(Button::secondary("Show in Folder")).clicked() {
                            reveal(&j.path);
                        }
                        if ui.add(Button::ghost("Remove")).clicked() {
                            remove = Some(i);
                        }
                    }
                }
            });
        }
        if let Some(i) = remove {
            self.jobs.remove(i);
        }
    }
}

/// Opens the file manager on `path`'s folder, selecting it where the platform can.
pub fn reveal(path: &Path) {
    let dir = path.parent().unwrap_or(path);
    let r = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
    } else if cfg!(windows) {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
    } else {
        std::process::Command::new("xdg-open").arg(dir).spawn()
    };
    if let Err(e) = r {
        tracing::warn!("can't open {}: {e}", dir.display());
    }
}

/// Posts a desktop notification.
/// shortcut: shells out to notify-send / osascript and does nothing on Windows; upgrade to a
/// notification crate if Windows users ask.
pub fn notify(title: &str, body: &str) {
    let r = if cfg!(target_os = "macos") {
        let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        let script = format!(
            "display notification {} with title {}",
            quote(body),
            quote(title)
        );
        std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn()
    } else if cfg!(windows) {
        return;
    } else {
        std::process::Command::new("notify-send")
            .args(["--app-name=jvid", title, body])
            .spawn()
    };
    if let Err(e) = r {
        tracing::debug!("notification: {e}");
    }
}
