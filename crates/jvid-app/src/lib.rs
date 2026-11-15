//! The jvid desktop video editor's code, shared by its binary and the headless `jvid` CLI.

// The hot-reload binary reexports internal modules to retain their crate:: paths.
pub(crate) mod annotations;
pub mod app;
pub(crate) mod autosave;
pub(crate) mod caption_workspace;
pub mod collect_project;
pub(crate) mod command_palette;
pub(crate) mod components;
pub(crate) mod devices;
pub(crate) mod easing;
pub(crate) mod effects;
#[cfg(test)]
pub(crate) mod empty_state_tests;
pub mod export;
pub(crate) mod framing;
pub(crate) mod handles;
pub(crate) mod hide_region;
pub mod highlight_review;
pub mod i18n;
pub(crate) mod import;
pub(crate) mod inspector;
pub(crate) mod keymap;
pub(crate) mod keymap_editor;
pub(crate) mod masks;
pub(crate) mod media_bin;
pub(crate) mod notices;
pub mod package_smoke;
pub(crate) mod personal_presets;
pub(crate) mod playback;
pub(crate) mod playback_status;
pub mod preferences;
pub(crate) mod preview;
pub mod project_file;
pub(crate) mod project_settings;
pub(crate) mod record;
pub(crate) mod relink;
pub mod samples;
pub(crate) mod scopes;
pub mod settings_file;
pub mod settings_transfer;
pub(crate) mod speech_review;
pub(crate) mod subtitles;
pub(crate) mod timeline;
pub(crate) mod transcript;
pub(crate) mod transport;
pub(crate) mod updates;
pub(crate) mod viewer_geometry;
pub(crate) mod welcome;
pub mod workspaces;
pub(crate) mod zoom;

/// Test helpers the modules share.
#[cfg(test)]
pub(crate) mod test_util {
    use egui_kittest::kittest::Queryable;

    /// Materialize the dependency's embedded fonts for tests that need real file paths.
    pub fn font_file(name: &str) -> std::path::PathBuf {
        static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        let dir = DIR.get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            for (name, bytes) in [
                ("Inter-Regular.ttf", j_ui::theme::INTER_REGULAR),
                ("Inter-Medium.ttf", j_ui::theme::INTER_MEDIUM),
                ("Inter-SemiBold.ttf", j_ui::theme::INTER_SEMIBOLD),
            ] {
                std::fs::write(dir.path().join(name), bytes).unwrap();
            }
            dir
        });
        dir.path().join(name)
    }

    /// Locale is process-global; readers outside a locale test do not take its lock.
    /// Returns true in the parent after the exact test passed in a fresh process.
    pub fn isolate_locale() -> bool {
        const CHILD: &str = "JVID_LOCALE_TEST_CHILD";
        let thread = std::thread::current();
        let name = thread.name().expect("named test thread");
        if std::env::var(CHILD).is_ok_and(|child| child == name) {
            return false;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([name, "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD, name)
            .output()
            .expect("start isolated locale test");
        assert!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
            "{name} failed in isolated process: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        true
    }

    /// Opens each dropdown `labels` names in turn and closes it again: its label and the
    /// rect of the menu it opened.
    pub fn menu_rects<S>(
        h: &mut egui_kittest::Harness<'_, S>,
        labels: &[&str],
    ) -> Vec<(String, egui::Rect)> {
        let mut out = Vec::new();
        for l in labels {
            let before = j_ui::test_support::area_rects(&h.ctx);
            h.get_by_role_and_label(egui::accesskit::Role::ComboBox, l)
                .click();
            h.run();
            let after = j_ui::test_support::area_rects(&h.ctx);
            let menu = after.into_iter().find(|a| !before.contains(a));
            let (_, r) = menu.unwrap_or_else(|| panic!("{l} opens a menu"));
            out.push((l.to_string(), r));
            h.key_press(egui::Key::Escape);
            h.run();
        }
        out
    }
}
