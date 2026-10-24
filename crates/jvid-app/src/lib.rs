//! The jvid desktop video editor's code, shared by its binary and the headless `jvid` CLI.

pub mod app;
mod autosave;
mod easing;
mod effects;
pub mod export;
mod handles;
pub mod i18n;
mod import;
mod inspector;
mod keymap;
mod keymap_editor;
mod masks;
mod media_bin;
mod playback;
pub mod preferences;
mod preview;
pub mod project_file;
mod project_settings;
mod record;
mod relink;
mod scopes;
pub mod settings_file;
mod timeline;
mod transport;
mod welcome;
mod zoom;

/// Test helpers the modules share.
#[cfg(test)]
mod test_util {
    use egui_kittest::kittest::Queryable;

    /// Opens each dropdown `labels` names in turn and closes it again: its label and the
    /// rect of the menu it opened.
    pub fn menu_rects<S>(
        h: &mut egui_kittest::Harness<'_, S>,
        labels: &[&str],
    ) -> Vec<(String, egui::Rect)> {
        let mut out = Vec::new();
        for l in labels {
            let before = suite_ui::test_support::area_rects(&h.ctx);
            h.get_by_role_and_label(egui::accesskit::Role::ComboBox, l)
                .click();
            h.run();
            let after = suite_ui::test_support::area_rects(&h.ctx);
            let menu = after.into_iter().find(|a| !before.contains(a));
            let (_, r) = menu.unwrap_or_else(|| panic!("{l} opens a menu"));
            out.push((l.to_string(), r));
            h.key_press(egui::Key::Escape);
            h.run();
        }
        out
    }
}
