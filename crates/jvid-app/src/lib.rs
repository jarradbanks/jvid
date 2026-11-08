//! The jvid desktop video editor's code, shared by its binary and the headless `jvid` CLI.

// Preserve the updater module verbatim during the UI kit extraction.
extern crate j_ui as suite_ui;

mod annotations;
pub mod app;
mod autosave;
mod caption_workspace;
pub mod collect_project;
mod command_palette;
mod components;
mod easing;
mod effects;
#[cfg(test)]
mod empty_state_tests;
pub mod export;
mod framing;
mod handles;
mod hide_region;
pub mod highlight_review;
pub mod i18n;
mod import;
mod inspector;
mod keymap;
mod keymap_editor;
mod masks;
mod media_bin;
mod notices;
pub mod package_smoke;
mod personal_presets;
mod playback;
mod playback_status;
pub mod preferences;
mod preview;
pub mod project_file;
mod project_settings;
mod record;
mod relink;
pub mod samples;
mod scopes;
pub mod settings_file;
pub mod settings_transfer;
mod speech_review;
mod subtitles;
mod timeline;
mod transcript;
mod transport;
mod updates;
mod viewer_geometry;
mod welcome;
pub mod workspaces;
mod zoom;

/// Test helpers the modules share.
#[cfg(test)]
mod test_util {
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
