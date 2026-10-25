//! The update window owns the asynchronous check/download, never an editor job or project.
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};

use jvid_updater::{
    TARGET, VERSION, install,
    network::{Available, Client, Download},
    state::{State, Work},
};
use suite_ui::tokens::{color, radius, space};
use suite_ui::widgets::{Button, Checkbox};

use crate::i18n::{tr, verbatim};

enum Event {
    Checked(Result<Option<Available>, String>),
    Progress(u64),
    Downloaded(Result<Download, String>),
}

pub struct Updates {
    pub open: bool,
    pub automatic: bool,
    pub state: State,
    receiver: Option<Receiver<Event>>,
    cancel: Arc<AtomicBool>,
    distribution: String,
    report: Option<PathBuf>,
    previous: Option<String>,
    preference_error: Option<String>,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            open: false,
            automatic: true,
            state: State::Idle,
            receiver: None,
            cancel: Arc::new(AtomicBool::new(false)),
            distribution: std::env::current_exe()
                .ok()
                .map(|e| jvid_updater::metadata::distribution(&e))
                .unwrap_or_else(|| "external".into()),
            report: None,
            previous: None,
            preference_error: None,
        }
    }
}

impl Updates {
    pub fn init(&mut self, automatic: bool, settings: Option<PathBuf>) {
        self.automatic = automatic;
        self.report = settings.map(|p| p.with_file_name("update-result.txt"));
        self.previous = self
            .report
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok());
        if automatic {
            self.check(false);
        }
    }

    pub fn check(&mut self, manual: bool) {
        self.open |= manual;
        if self.state.busy() {
            return;
        }
        let key = match jvid_updater::public_key() {
            Ok(k) => k,
            Err(e) => {
                self.state = State::Failed(e);
                return;
            }
        };
        self.state = State::Checking;
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        let distribution = self.distribution.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::Checked(Client::default().check(
                VERSION,
                TARGET,
                &distribution,
                &key,
            )));
        });
    }

    fn download(&mut self, update: Available) {
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        self.state = State::Downloading {
            update: update.clone(),
            received: 0,
        };
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        std::thread::spawn(move || {
            let result = Client::default().download(update, &cancel, |n| {
                let _ = tx.send(Event::Progress(n));
            });
            let _ = tx.send(Event::Downloaded(result));
        });
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut finished = false;
        if let Some(rx) = &self.receiver {
            loop {
                let event = match rx.try_recv() {
                    Ok(event) => event,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if !finished {
                            self.state = State::Failed(tr!("updates.worker_failed").to_string());
                            finished = true;
                        }
                        break;
                    }
                };
                match event {
                    Event::Checked(Ok(Some(update))) => {
                        self.state = State::Available(update);
                        self.open = true;
                        finished = true;
                    }
                    Event::Checked(Ok(None)) => {
                        self.state = State::Current;
                        finished = true;
                    }
                    Event::Checked(Err(e)) | Event::Downloaded(Err(e)) => {
                        self.state = State::Failed(e);
                        finished = true;
                    }
                    Event::Progress(n) => self.state.progress(n),
                    Event::Downloaded(Ok(d)) => {
                        self.state = State::Ready(d);
                        finished = true;
                    }
                }
            }
        }
        if finished {
            self.receiver = None;
        }
        if self.state.busy() {
            ctx.request_repaint_after_secs(0.1);
        }
    }

    /// Rechecks all work immediately before preparation; the caller must also gate Save.
    pub fn prepare(&mut self, work: Work) -> Option<install::Prepared> {
        if !work.can_install() || !matches!(self.state, State::Ready(_)) {
            return None;
        }
        let State::Ready(download) = std::mem::take(&mut self.state) else {
            return None;
        };
        let result = (|| {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let report = self
                .report
                .as_deref()
                .ok_or_else(|| tr!("settings.no_config_dir").to_string())?;
            install::prepare(download, &exe, report)
        })();
        match result {
            Ok(prepared) => Some(prepared),
            Err(e) => {
                self.state = State::Failed(e);
                None
            }
        }
    }

    /// True asks the editor to save if necessary, then recheck the install guard.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        work: Work,
        settings: Option<&std::path::Path>,
    ) -> bool {
        if !self.open {
            return false;
        }
        let mut install = false;
        let mut download = None;
        let mut check = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("updates"))
            .frame(
                egui::Frame::popup(ui.style())
                    .fill(color::panel())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(space::S16),
            )
            .show(ui.ctx(), |ui| {
                ui.set_width(540.0_f32.min(ui.ctx().content_rect().width() - 48.0));
                ui.heading(tr!("updates.title"));
                ui.label(tr!("updates.installed", version = VERSION));
                if !jvid_updater::metadata::self_update(TARGET, &self.distribution) {
                    ui.label(if self.distribution == verbatim("external") {
                        tr!("updates.external")
                    } else {
                        tr!("updates.manual")
                    });
                    ui.hyperlink_to(
                        tr!("updates.release_page"),
                        verbatim("https://github.com/jarradbanks/jvid/releases"),
                    );
                }
                if ui
                    .add(Checkbox::new(&mut self.automatic, tr!("updates.automatic")))
                    .changed()
                {
                    self.preference_error = settings
                        .ok_or_else(|| std::io::Error::other(tr!("settings.no_config_dir")))
                        .and_then(|p| {
                            crate::settings_file::save_value_to(
                                p,
                                "automatic_update_checks",
                                self.automatic.into(),
                            )
                        })
                        .err()
                        .map(|e| e.to_string());
                }
                if let Some(e) = &self.preference_error {
                    ui.label(tr!("settings.save_failed", error = e));
                }
                ui.separator();
                match &self.state {
                    State::Idle => {
                        ui.label(tr!("updates.idle"));
                    }
                    State::Checking => {
                        ui.spinner();
                        ui.label(tr!("updates.checking"));
                    }
                    State::Current => {
                        ui.label(tr!("updates.current"));
                    }
                    State::Failed(e) => {
                        ui.label(tr!("updates.failed", error = e));
                    }
                    State::Available(update) | State::Downloading { update, .. } => {
                        release(ui, update);
                        if let State::Downloading { received, .. } = &self.state {
                            ui.add(
                                egui::ProgressBar::new(*received as f32 / update.asset.size as f32)
                                    .show_percentage(),
                            );
                            ui.label(tr!(
                                "updates.progress",
                                received = received,
                                total = update.asset.size
                            ));
                            cancel = ui.add(Button::secondary(tr!("updates.cancel"))).clicked();
                        } else if jvid_updater::metadata::self_update(TARGET, &self.distribution) {
                            if ui.add(Button::primary(tr!("updates.download"))).clicked() {
                                download = Some(update.clone());
                            }
                        } else {
                            ui.hyperlink_to(tr!("updates.release_page"), update.release_url());
                        }
                    }
                    State::Ready(d) => {
                        release(ui, &d.update);
                        ui.label(tr!("updates.verified"));
                        let mut allowed = work;
                        allowed.unsaved = false;
                        if !allowed.can_install() {
                            ui.label(tr!("updates.busy"));
                        }
                        let label = if work.unsaved {
                            tr!("updates.save_restart")
                        } else {
                            tr!("updates.restart")
                        };
                        install = ui
                            .add_enabled(allowed.can_install(), Button::primary(label))
                            .clicked();
                    }
                }
                if let Some(previous) = &self.previous {
                    ui.collapsing(tr!("updates.previous"), |ui| {
                        ui.label(previous);
                    });
                }
                ui.separator();
                ui.horizontal(|ui| {
                    check = ui
                        .add_enabled(!self.state.busy(), Button::secondary(tr!("updates.check")))
                        .clicked();
                    if ui.add(Button::secondary(tr!("updates.later"))).clicked() {
                        ui.close();
                    }
                });
            });
        if modal.should_close() {
            self.open = false;
        }
        if cancel {
            self.cancel_download();
        }
        if let Some(update) = download {
            self.download(update);
        }
        if check {
            self.check(true);
        }
        install
    }

    fn cancel_download(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.receiver = None;
        self.state.cancel();
    }
}

impl Drop for Updates {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn release(ui: &mut egui::Ui, update: &Available) {
    ui.label(tr!("updates.available", version = &update.version));
    egui::ScrollArea::vertical()
        .max_height(240.0)
        .show(ui, |ui| {
            ui.label(&update.notes);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available() -> Available {
        Available {
            version: "1.2.0".into(),
            notes: "Notes".into(),
            asset: jvid_updater::metadata::Asset {
                target: TARGET.into(),
                distribution: "windows-installer".into(),
                name: "jvid-1.2.0-windows-x86_64-setup.exe".into(),
                size: 100,
                sha256: "00".repeat(32),
            },
            tag: "v1.2.0".into(),
            manifest: vec![],
            signature: vec![],
        }
    }

    #[test]
    fn disabling_automatic_checks_is_persisted_and_manual_action_remains_available() {
        let dir =
            std::env::temp_dir().join(format!("jvid-477-update-setting-{}", std::process::id()));
        let path = dir.join("settings.toml");
        crate::settings_file::save_value_to(&path, "automatic_update_checks", false.into())
            .unwrap();
        let settings = crate::settings_file::load_from(&path);
        assert!(!settings.automatic_update_checks);
        let mut updates = Updates::default();
        updates.init(settings.automatic_update_checks, Some(path));
        assert!(matches!(updates.state, State::Idle));
        assert!(updates.receiver.is_none());
        updates.check(true);
        assert!(updates.open);
        // An unconfigured development build fails closed; a configured build starts checking.
        assert!(!matches!(updates.state, State::Idle));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn progress_ready_error_retry_and_cancelled_worker_results_have_explicit_states() {
        let mut updates = Updates::default();
        let (tx, rx) = mpsc::channel();
        updates.receiver = Some(rx);
        updates.state = State::Checking;
        tx.send(Event::Checked(Ok(Some(available())))).unwrap();
        updates.poll(&egui::Context::default());
        assert!(updates.open && matches!(updates.state, State::Available(_)));
        updates.state = State::Downloading {
            update: available(),
            received: 0,
        };
        let (tx, rx) = mpsc::channel();
        updates.receiver = Some(rx);
        tx.send(Event::Progress(30)).unwrap();
        tx.send(Event::Progress(20)).unwrap();
        updates.poll(&egui::Context::default());
        assert!(matches!(
            updates.state,
            State::Downloading { received: 30, .. }
        ));
        updates.cancel_download();
        assert!(updates.cancel.load(Ordering::Relaxed));
        assert!(
            tx.send(Event::Downloaded(Err("late failure".into())))
                .is_err()
        );
        updates.poll(&egui::Context::default());
        assert!(matches!(updates.state, State::Available(_)));
        let (tx, rx) = mpsc::channel();
        updates.receiver = Some(rx);
        tx.send(Event::Downloaded(Err("disk full".into()))).unwrap();
        updates.poll(&egui::Context::default());
        assert!(matches!(updates.state, State::Failed(_)) && updates.receiver.is_none());
        // Installer preparation outside Ready must preserve the diagnostic.
        assert!(updates.prepare(Work::default()).is_none());
        assert!(matches!(updates.state, State::Failed(_)));
        let (tx, rx) = mpsc::channel();
        updates.receiver = Some(rx);
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().to_owned();
        tx.send(Event::Downloaded(Ok(Download {
            update: available(),
            dir,
        })))
        .unwrap();
        updates.poll(&egui::Context::default());
        assert!(matches!(updates.state, State::Ready(_)));
        assert!(
            updates
                .prepare(Work {
                    unsaved: true,
                    ..Work::default()
                })
                .is_none()
        );
        assert!(matches!(updates.state, State::Ready(_)) && stage.exists());
        drop(updates);
        assert!(!stage.exists());
    }

    #[test]
    fn a_disconnected_worker_becomes_a_retryable_error() {
        let mut updates = Updates::default();
        let (tx, rx) = mpsc::channel();
        updates.receiver = Some(rx);
        updates.state = State::Checking;
        drop(tx);
        updates.poll(&egui::Context::default());
        assert!(matches!(updates.state, State::Failed(_)) && updates.receiver.is_none());
    }

    #[test]
    fn update_window_blocks_restart_for_work_and_retains_a_download_when_deferred() {
        use egui_kittest::kittest::{NodeT, Queryable};
        struct Test {
            updates: Updates,
            work: Work,
            install: bool,
        }
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().to_owned();
        let mut updates = Updates::default();
        updates.open = true;
        updates.state = State::Ready(Download {
            update: available(),
            dir,
        });
        let mut fresh = true;
        let mut h = egui_kittest::Harness::new_ui_state(
            move |ui, state: &mut Test| {
                if suite_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                state.install |= state.updates.show(ui, state.work, None);
            },
            Test {
                updates,
                work: Work {
                    recording: true,
                    ..Work::default()
                },
                install: false,
            },
        );
        h.run();
        assert!(
            h.get_by_label(tr!("updates.restart"))
                .accesskit_node()
                .is_disabled()
        );
        assert!(!h.state().install);
        h.get_by_label(tr!("updates.later")).click();
        h.run();
        assert!(!h.state().updates.open && stage.exists());
        drop(h);
        assert!(!stage.exists());
    }
}
