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
        // Reopening/checking must keep the verified payload available for this session.
        if self.state.busy() || matches!(self.state, State::Ready(_)) {
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
                            ui.label(if *received == update.asset.size {
                                tr!("updates.verifying")
                            } else {
                                tr!("updates.downloading")
                            });
                            ui.add(
                                egui::ProgressBar::new(*received as f32 / update.asset.size as f32)
                                    .show_percentage(),
                            );
                            ui.label(tr!(
                                "updates.progress",
                                received = received,
                                total = update.asset.size
                            ));
                            ui.label(tr!("updates.download_later"));
                        } else if !jvid_updater::metadata::self_update(TARGET, &self.distribution) {
                            ui.hyperlink_to(tr!("updates.release_page"), update.release_url());
                        }
                    }
                    State::Ready(d) => {
                        release(ui, &d.update);
                        ui.label(tr!("updates.verified"));
                        ui.label(tr!("updates.ready_later"));
                        let mut allowed = work;
                        allowed.unsaved = false;
                        if !allowed.can_install() {
                            ui.label(tr!("updates.busy"));
                        }
                    }
                }
                if let Some(previous) = &self.previous {
                    ui.collapsing(tr!("updates.previous"), |ui| {
                        ui.label(previous);
                    });
                }
                ui.separator();
                let check_state = if self.state.busy() || matches!(self.state, State::Ready(_)) {
                    suite_ui::widgets::State::Disabled
                } else {
                    suite_ui::widgets::State::Live
                };
                let mut buttons = vec![
                    Button::secondary(tr!("updates.later")),
                    Button::secondary(tr!("updates.check")).state(check_state),
                ];
                let mut allowed = work;
                allowed.unsaved = false;
                let primary = match &self.state {
                    State::Available(_)
                        if jvid_updater::metadata::self_update(TARGET, &self.distribution) =>
                    {
                        Some(Button::primary(tr!("updates.download")))
                    }
                    State::Downloading { .. } => Some(Button::secondary(tr!("updates.cancel"))),
                    State::Ready(_) => Some(
                        Button::primary(if work.unsaved {
                            tr!("updates.save_restart")
                        } else {
                            tr!("updates.restart")
                        })
                        .state(if allowed.can_install() {
                            suite_ui::widgets::State::Live
                        } else {
                            suite_ui::widgets::State::Disabled
                        }),
                    ),
                    _ => None,
                };
                if let Some(primary) = primary {
                    buttons.push(primary);
                }
                let actions = suite_ui::dialog::action_footer(ui, buttons);
                if actions[0].clicked() {
                    ui.close();
                }
                check = actions[1].clicked();
                if actions.get(2).is_some_and(|r| r.clicked()) {
                    match &self.state {
                        State::Available(update) => download = Some(update.clone()),
                        State::Downloading { .. } => cancel = true,
                        State::Ready(_) => install = true,
                        _ => {}
                    }
                }
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
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

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

    struct Window {
        updates: Updates,
        work: Work,
        install: bool,
    }

    fn window(state: State, work: Work) -> Harness<'static, Window> {
        let mut fresh = true;
        let mut updates = Updates::default();
        updates.open = true;
        updates.state = state;
        Harness::builder().with_size([800.0, 640.0]).build_ui_state(
            move |ui, state: &mut Window| {
                if suite_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                state.updates.poll(ui.ctx());
                state.install |= state.updates.show(ui, state.work, None);
            },
            Window {
                updates,
                work,
                install: false,
            },
        )
    }

    fn capture(h: &mut Harness<'_, Window>, name: &str) {
        if let Some(dir) = std::env::var_os("JVID_UPDATE_SCREENSHOTS") {
            assert!(suite_ui::test_support::gpu());
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            h.render()
                .unwrap()
                .save(dir.join(format!("{name}.png")))
                .unwrap();
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
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().to_owned();
        let mut h = window(
            State::Ready(Download {
                update: available(),
                dir,
            }),
            Work::default(),
        );
        for work in [
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
            Work {
                unsaved: true,
                other_job: true,
                ..Work::default()
            },
        ] {
            h.state_mut().work = work;
            h.run();
            h.get_by_label(tr!("updates.busy"));
            let label = if work.unsaved {
                tr!("updates.save_restart")
            } else {
                tr!("updates.restart")
            };
            assert!(h.get_by_label(label).accesskit_node().is_disabled());
            assert!(!h.state().install);
        }
        h.get_by_label(tr!("updates.later")).click();
        h.run();
        assert!(!h.state().updates.open && stage.exists());

        // Both reopening and a direct check keep the same verified download.
        h.state_mut().updates.check(true);
        h.state_mut().work = Work {
            unsaved: true,
            ..Work::default()
        };
        h.run();
        assert!(
            h.get_by_label(tr!("updates.check"))
                .accesskit_node()
                .is_disabled()
        );
        assert!(matches!(h.state().updates.state, State::Ready(_)) && stage.exists());
        let install = h.get_by_label(tr!("updates.save_restart"));
        assert!(!install.accesskit_node().is_disabled());
        let install_rect = install.rect();
        let later = h.get_by_label(tr!("updates.later")).rect();
        let check = h.get_by_label(tr!("updates.check")).rect();
        assert!(later.right() <= check.left() && check.right() <= install_rect.left());
        let modal = h
            .ctx
            .memory(|m| m.area_rect(egui::Id::new("updates")))
            .unwrap();
        assert!((modal.right() - space::S16 - install_rect.right()).abs() <= 1.0);
        capture(&mut h, "updates-save-install-restart");
        h.get_by_label(tr!("updates.save_restart")).click();
        h.run();
        assert!(h.state().install && stage.exists());
        drop(h);
        assert!(!stage.exists());
    }

    #[test]
    fn download_can_continue_when_deferred_and_only_verified_completion_offers_install() {
        let mut h = window(
            State::Downloading {
                update: available(),
                received: 30,
            },
            Work::default(),
        );
        let (tx, rx) = mpsc::channel();
        h.state_mut().updates.receiver = Some(rx);
        h.run_steps(4);
        h.get_by_label(tr!("updates.downloading"));
        h.get_by_label(&tr!("updates.progress", received = 30, total = 100));
        h.get_by_label(tr!("updates.download_later"));
        assert!(h.query_by_label(tr!("updates.restart")).is_none());
        assert!(
            h.get_by_label(tr!("updates.check"))
                .accesskit_node()
                .is_disabled()
        );
        capture(&mut h, "updates-downloading");
        tx.send(Event::Progress(100)).unwrap();
        h.run_steps(4);
        h.get_by_label(tr!("updates.verifying"));
        assert!(h.query_by_label(tr!("updates.restart")).is_none());
        capture(&mut h, "updates-verifying");

        h.get_by_label(tr!("updates.later")).click();
        h.run_steps(4);
        assert!(!h.state().updates.open && h.state().updates.receiver.is_some());
        assert!(!h.state().updates.cancel.load(Ordering::Relaxed));
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().to_owned();
        tx.send(Event::Downloaded(Ok(Download {
            update: available(),
            dir,
        })))
        .unwrap();
        h.run();
        assert!(!h.state().updates.open && matches!(h.state().updates.state, State::Ready(_)));
        assert!(h.state().updates.receiver.is_none() && stage.exists());
        h.state_mut().updates.check(true);
        h.run();
        h.get_by_label(tr!("updates.verified"));
        h.get_by_label(tr!("updates.ready_later"));
        capture(&mut h, "updates-install-restart");
        h.get_by_label(tr!("updates.restart")).click();
        h.run();
        assert!(h.state().install);
    }

    #[test]
    fn cancelling_download_rejects_late_results_and_failure_leaves_retry_available() {
        let mut h = window(
            State::Downloading {
                update: available(),
                received: 30,
            },
            Work::default(),
        );
        let (tx, rx) = mpsc::channel();
        h.state_mut().updates.receiver = Some(rx);
        h.run_steps(4);
        h.get_by_label(tr!("updates.cancel")).click();
        h.run();
        assert!(matches!(h.state().updates.state, State::Available(_)));
        assert!(h.state().updates.cancel.load(Ordering::Relaxed));
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().to_owned();
        let late = tx.send(Event::Downloaded(Ok(Download {
            update: available(),
            dir,
        })));
        assert!(late.is_err());
        drop(late);
        assert!(!stage.exists());
        assert!(!h.state().install && h.query_by_label(tr!("updates.restart")).is_none());

        h.state_mut().updates.state = State::Failed("disk full".into());
        h.run();
        h.get_by_label(&tr!("updates.failed", error = "disk full"));
        assert!(
            !h.get_by_label(tr!("updates.check"))
                .accesskit_node()
                .is_disabled()
        );
        assert!(h.query_by_label(tr!("updates.restart")).is_none());
    }
}
