//! App-owned preferences, notifications, paths and save/shutdown integration.
//! j-update owns workers and installation; j-ui owns update presentation.
use std::path::{Path, PathBuf};

use j_ui::update::{Action, Prompt};
use j_update::{
    Client, Config, InstallPaths, install, metadata,
    state::{State, Updater, Work},
};
use jvid_update_config::VERSION;

use crate::i18n::{tr, verbatim};

pub struct Updates {
    pub open: bool,
    pub automatic: bool,
    worker: Result<Updater, String>,
    unconfigured: State,
    config: Option<Config>,
    report: Option<PathBuf>,
    previous: Option<String>,
    preference_error: Option<String>,
}

impl Default for Updates {
    fn default() -> Self {
        Self::new(jvid_update_config::current_install())
    }
}

impl Updates {
    fn new(config: j_update::Result<Config>) -> Self {
        Self {
            open: false,
            automatic: true,
            config: config.as_ref().ok().cloned(),
            // Anonymous only: jvid releases have always been public.
            worker: config
                .and_then(Client::new)
                .map(|c| Updater::new(c, VERSION)),
            unconfigured: State::Idle,
            report: None,
            previous: None,
            preference_error: None,
        }
    }

    pub fn state(&self) -> &State {
        match &self.worker {
            Ok(worker) => &worker.state,
            Err(_) => &self.unconfigured,
        }
    }

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
        match &mut self.worker {
            Ok(worker) => worker.check(),
            Err(e) => self.unconfigured = State::Failed(e.clone()),
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Ok(worker) = &mut self.worker {
            if worker.poll() && matches!(worker.state, State::Available(_)) {
                self.open = true;
            }
            if worker.state.busy() {
                ctx.request_repaint_after_secs(0.1);
            }
        }
    }

    /// Called only after the editor finishes its final save and rechecks all active work.
    pub fn prepare(&mut self, work: Work) -> Option<install::Prepared> {
        if !work.can_install() || !matches!(self.state(), State::Ready(_)) {
            return None;
        }
        let worker = self.worker.as_mut().ok()?;
        let paths: j_update::Result<InstallPaths> = (|| {
            let executable = std::env::current_exe().map_err(|e| e.to_string())?;
            let helper = executable.with_file_name("jvid-update-helper.exe");
            let report = self
                .report
                .clone()
                .ok_or_else(|| tr!("settings.no_config_dir").to_string())?;
            Ok(InstallPaths {
                executable,
                helper,
                report,
            })
        })();
        match paths {
            Ok(paths) => worker.prepare(work, &paths)?.ok(),
            Err(e) => {
                worker.state = State::Failed(e);
                None
            }
        }
    }

    /// Requests the editor's save-before-restart flow; the core still guards preparation.
    pub fn show(&mut self, ui: &mut egui::Ui, work: Work, settings: Option<&Path>) -> bool {
        if !self.open {
            return false;
        }
        let automatic = self.automatic;
        let installed = self
            .config
            .as_ref()
            .is_some_and(|c| metadata::self_update(&c.target, &c.distribution));
        let state = match &self.worker {
            Ok(worker) => &worker.state,
            Err(_) => &self.unconfigured,
        };
        // The inline shared prompt lets jvid retain its manual-package instructions and
        // preference-save diagnostics in the same modal.
        let id = egui::Id::new("updates");
        let previous_rect = ui.ctx().memory(|m| m.area_rect(id));
        let modal = j_ui::dialog::modal(
            ui.ctx(),
            id,
            j_ui::tr!(j_ui::i18n::CATALOG, "updates.title"),
            540.0,
            |ui| {
                if !installed {
                    if self
                        .config
                        .as_ref()
                        .is_some_and(|c| c.distribution != "external")
                    {
                        ui.label(tr!("updates.manual"));
                    }
                    if !matches!(state, State::Available(_)) {
                        ui.hyperlink_to(
                            tr!("updates.release_page"),
                            verbatim("https://github.com/jarradbanks/jvid/releases"),
                        );
                    }
                }
                if let Some(e) = &self.preference_error {
                    ui.label(tr!("settings.save_failed", error = e));
                }
                Prompt {
                    state,
                    current_version: VERSION,
                    installed,
                    work,
                    save_before_restart: true,
                    previous: self.previous.as_deref(),
                }
                .contents(ui, &mut self.automatic)
            },
        );
        if previous_rect.is_some_and(|r| r.size() != modal.response.rect.size()) {
            ui.ctx().request_repaint();
        }
        let (action, dismiss) = modal.inner;
        if dismiss || modal.should_close() {
            self.open = false;
            if let Ok(worker) = &mut self.worker {
                worker.cancel();
            }
        }
        if automatic != self.automatic {
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
        match action {
            Some(Action::Check) => self.check(true),
            Some(Action::Download) => {
                if let Ok(worker) = &mut self.worker {
                    worker.download();
                }
            }
            Some(Action::Cancel) => {
                if let Ok(worker) = &mut self.worker {
                    worker.cancel();
                }
            }
            Some(Action::OpenRelease) => {
                if let (Some(config), State::Available(update)) = (&self.config, self.state())
                    && let Ok(url) = update.release_url(config)
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            }
            Some(Action::Install) => return true,
            None => {}
        }
        false
    }

    #[cfg(test)]
    pub(crate) fn for_test(state: State, distribution: &str) -> Self {
        let mut config = jvid_update_config::with_public_key([0; 32], distribution);
        config.target = "x86_64-pc-windows-msvc".into();
        let mut updates = Self::new(Ok(config));
        *updates.state_mut() = state;
        updates
    }

    #[cfg(test)]
    pub(crate) fn state_mut(&mut self) -> &mut State {
        match &mut self.worker {
            Ok(worker) => &mut worker.state,
            Err(_) => &mut self.unconfigured,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };
    use j_update::network::{Available, Download};

    fn available() -> Available {
        Available {
            version: "1.2.0".into(),
            notes: "Update fixture".into(),
            asset: j_update::metadata::Asset {
                target: "x86_64-pc-windows-msvc".into(),
                distribution: "windows-installer".into(),
                name: "jvid-1.2.0-windows-x86_64-setup.exe".into(),
                size: 100,
                sha256: "00".repeat(32),
            },
            asset_id: 1,
            tag: "v1.2.0".into(),
            manifest: vec![],
            signature: vec![],
        }
    }

    struct Window {
        updates: Updates,
        work: Work,
        settings: Option<PathBuf>,
        install: bool,
    }

    fn window(state: State, distribution: &str) -> Harness<'static, Window> {
        let mut fresh = true;
        let mut updates = Updates::for_test(state, distribution);
        updates.open = true;
        Harness::builder().with_size([820.0, 720.0]).build_ui_state(
            move |ui, state: &mut Window| {
                if j_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                state.install |= state
                    .updates
                    .show(ui, state.work, state.settings.as_deref());
            },
            Window {
                updates,
                work: Work::default(),
                settings: None,
                install: false,
            },
        )
    }

    fn label(key: &str) -> String {
        j_ui::i18n::CATALOG.get_in("en", key).unwrap().to_owned()
    }

    #[test]
    fn automatic_preference_and_previous_report_keep_their_existing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.toml");
        std::fs::write(&settings, "automatic_update_checks = true\n").unwrap();
        std::fs::write(
            dir.path().join("update-result.txt"),
            "Previous installer result",
        )
        .unwrap();
        let mut h = window(State::Current, "windows-installer");
        h.state_mut().settings = Some(settings.clone());
        h.state_mut().updates.init(false, Some(settings.clone()));
        assert_eq!(
            h.state().updates.previous.as_deref(),
            Some("Previous installer result")
        );
        // Start from the saved preference, without starting a network request.
        h.state_mut().updates.automatic = true;
        h.run();
        h.get_by_label(&label("updates.automatic")).click();
        h.run();
        assert!(!crate::settings_file::load_from(&settings).automatic_update_checks);
        assert!(h.state().updates.preference_error.is_none());
        assert!(
            !h.get_by_label(&label("updates.check"))
                .accesskit_node()
                .is_disabled()
        );
        let mut reopened = Updates::new(Err("Unconfigured fixture".into()));
        reopened.init(
            crate::settings_file::load_from(&settings).automatic_update_checks,
            Some(settings),
        );
        assert!(matches!(reopened.state(), State::Idle));
        assert!(!reopened.open);
        reopened.check(true);
        assert!(reopened.open && matches!(reopened.state(), State::Failed(_)));
    }

    #[test]
    fn preference_save_failure_stays_visible_and_does_not_start_a_check() {
        let mut h = window(State::Idle, "external");
        h.run();
        h.get_by_label(&label("updates.automatic")).click();
        h.run();
        assert!(h.state().updates.preference_error.is_some());
        assert!(matches!(h.state().updates.state(), State::Idle));
        h.get_by_label(&tr!(
            "settings.save_failed",
            error = h.state().updates.preference_error.as_ref().unwrap()
        ));
    }

    #[test]
    fn shared_prompt_requests_jvid_save_and_keeps_ready_when_deferred() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().to_owned();
        let mut h = window(
            State::Ready(Download {
                update: available(),
                dir,
            }),
            "windows-installer",
        );
        h.state_mut().work.unsaved = true;
        h.run();
        h.get_by_label(&label("updates.later")).click();
        h.run();
        assert!(!h.state().updates.open && stage.exists());
        h.state_mut().updates.check(true);
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
        ] {
            h.state_mut().work = Work {
                unsaved: true,
                ..work
            };
            h.run();
            assert!(
                h.get_by_label(&label("updates.save_restart"))
                    .accesskit_node()
                    .is_disabled()
            );
            assert!(!h.state().install);
        }
        h.state_mut().work = Work {
            unsaved: true,
            ..Work::default()
        };
        h.run();
        assert!(
            h.get_by_label(&label("updates.check"))
                .accesskit_node()
                .is_disabled()
        );
        h.get_by_label(&label("updates.save_restart")).click();
        h.run();
        assert!(h.state().install && stage.exists());
        drop(h);
        assert!(!stage.exists());
    }

    #[test]
    fn dismissing_a_download_uses_the_shared_cancellation_policy() {
        let mut h = window(
            State::Downloading {
                update: available(),
                received: 30,
            },
            "windows-installer",
        );
        h.run_steps(4);
        assert!(h.query_by_label(&label("updates.restart")).is_none());
        h.get_by_label(&label("updates.later")).click();
        h.run_steps(4);
        assert!(!h.state().updates.open);
        assert!(matches!(h.state().updates.state(), State::Available(_)));
        assert!(!h.state().install);
    }

    #[test]
    fn manual_packages_keep_release_links_without_self_install_actions() {
        for distribution in [
            "external",
            "windows-portable",
            "macos-dmg",
            "macos-portable",
            "linux-portable",
        ] {
            let mut h = window(State::Available(available()), distribution);
            h.run();
            h.get_by_label(&label("updates.release_page"));
            assert!(h.query_by_label(&label("updates.download")).is_none());
            assert!(h.query_by_label(&label("updates.restart")).is_none());
            assert!(!h.state().install);
        }
    }
}
