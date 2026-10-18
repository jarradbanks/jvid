//! The render queue: exports run one after another in the background while editing goes on.
//! Each job owns a clone of the project taken when it was queued, so later edits don't reach it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Align, Layout, Ui};
use jvid_core::model::ProbeInfo;
use jvid_jobs::Handle;
use suite_ui::tokens::space;
use suite_ui::widgets::{Button, State};

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
    /// Cancel All was clicked with more than one job live and waits for a confirm.
    confirm_all: bool,
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

    /// Cancels job `i` if it isn't over yet.
    pub fn cancel(&self, i: usize) {
        if let Some(j) = self.jobs.get(i).filter(|j| j.result.is_none()) {
            j.handle.cancel();
        }
    }

    pub fn cancel_all(&mut self) {
        self.jobs.iter().for_each(|j| j.handle.cancel());
        self.confirm_all = false;
    }

    /// Drops jobs that are over.
    pub fn clear_finished(&mut self) {
        self.jobs.retain(|j| j.result.is_none());
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// The Export button's dropdown: one row per job, then the footer. Returns true when
    /// New Export… is clicked.
    pub fn menu(&mut self, ui: &mut Ui) -> bool {
        // A fixed width so the menu doesn't jump as rows change.
        ui.set_width(380.0);
        for (i, j) in self.jobs.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::S8;
                let name = j.path.file_name().unwrap_or_default().to_string_lossy();
                ui.add(egui::Label::new(name).truncate())
                    .on_hover_text(j.path.display().to_string());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| match &j.result {
                    None => {
                        if ui.add(Button::destructive("Cancel")).clicked() {
                            self.cancel(i);
                        }
                        let p = j.handle.progress();
                        let text = if p > 0.0 {
                            format!("{:.0}%", p * 100.0)
                        } else {
                            "Waiting".into()
                        };
                        ui.add(egui::ProgressBar::new(p).desired_width(120.0).text(text));
                    }
                    Some(r) => {
                        if r.is_ok() && ui.add(Button::secondary("Reveal")).clicked() {
                            reveal(&j.path);
                        }
                        match r {
                            Ok(()) => ui.label("Done"),
                            Err(Error::Cancelled) => ui.label("Cancelled"),
                            Err(Error::Failed(e)) => ui.label("Failed").on_hover_text(e),
                        };
                    }
                });
            });
        }
        if !self.jobs.is_empty() {
            ui.separator();
        }
        let live = |on: bool| if on { State::Live } else { State::Disabled };
        let mut new = false;
        ui.horizontal(|ui| {
            if self.confirm_all {
                ui.label(format!("Cancel {} Exports?", self.pending()));
                if ui.add(Button::destructive("Cancel All")).clicked() {
                    self.cancel_all();
                }
                if ui.add(Button::ghost("Keep Exporting")).clicked() {
                    self.confirm_all = false;
                }
                return;
            }
            new = ui.add(Button::secondary("New Export…")).clicked();
            let cancel_all = Button::destructive("Cancel All").state(live(self.pending() > 0));
            if ui.add(cancel_all).clicked() {
                if self.pending() > 1 {
                    self.confirm_all = true;
                } else {
                    self.cancel_all();
                }
            }
            let clear =
                Button::ghost("Clear Finished").state(live(self.jobs.len() > self.pending()));
            if ui.add(clear).clicked() {
                self.clear_finished();
            }
        });
        new
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
