//! The app preferences dialog, under Edit > Preferences: the UI language, applied at once and saved to `settings.toml`.

use egui::Ui;
use suite_ui::i18n::{self, LANGUAGES, SYSTEM};
use suite_ui::theme;
use suite_ui::tokens::{color, radius, space};
use suite_ui::widgets::{Button, Dropdown};

use crate::i18n::tr;

#[derive(Default)]
pub struct Preferences {
    open: bool,
    /// Why the last change didn't save, shown until the next one.
    error: Option<String>,
}

impl Preferences {
    pub fn open(&mut self) {
        self.open = true;
        self.error = None;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Shows the dialog while open. `language` is the setting (`system` or a code); a pick
    /// switches the UI at once and saves through `save`.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        language: &mut String,
        save: impl FnOnce(&str) -> std::io::Result<()>,
    ) {
        if !self.open {
            return;
        }
        let modal = egui::Modal::new(egui::Id::new("preferences"))
            .frame(
                egui::Frame::popup(ui.style())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(space::S16),
            )
            .show(ui.ctx(), |ui| {
                ui.set_width(360.0);
                ui.label(egui::RichText::new(tr!("settings.title")).font(theme::headline()));
                ui.add_space(space::S8);
                egui::Grid::new("preferences_grid")
                    .spacing([space::S8, space::S8])
                    .show(ui, |ui| {
                        ui.label(tr!("settings.language"));
                        let names = options();
                        let names: Vec<&str> = names.iter().map(String::as_str).collect();
                        let mut picked = choice(language);
                        if ui
                            .add(Dropdown::new(&mut picked, &names).width(200.0))
                            .changed()
                        {
                            *language = setting(picked).into();
                            i18n::set_language(i18n::resolve(language));
                            self.error = save(language).err().map(|e| e.to_string());
                        }
                        ui.end_row();
                    });
                ui.add_space(space::S8);
                ui.label(
                    egui::RichText::new(tr!("settings.language_note"))
                        .small()
                        .color(color::INK_3),
                );
                if let Some(e) = &self.error {
                    ui.add_space(space::S8);
                    let text = tr!("settings.save_failed", error = e);
                    ui.label(egui::RichText::new(text).color(color::RED));
                }
                ui.add_space(space::S16);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(Button::primary(tr!("settings.done"))).clicked() {
                        ui.close();
                    }
                });
            });
        if modal.should_close() {
            self.open = false;
        }
    }
}

/// The picker's rows: System (with the language it gives), then each language in its own
/// name so anyone can find theirs.
pub fn options() -> Vec<String> {
    let system = tr!(
        "settings.system",
        language = i18n::autonym(i18n::system_language())
    );
    let mut out = vec![system];
    out.extend(LANGUAGES.iter().map(|l| i18n::autonym(l).to_owned()));
    out
}

/// The picker row for a setting; an unshipped code shows as English, which is what it gives.
pub fn choice(setting: &str) -> usize {
    match setting.trim() {
        "" | SYSTEM => 0,
        code => {
            1 + LANGUAGES
                .iter()
                .position(|l| *l == i18n::resolve(code))
                .unwrap_or(0)
        }
    }
}

/// The setting a picker row saves.
pub fn setting(row: usize) -> &'static str {
    match row {
        0 => SYSTEM,
        n => LANGUAGES[n - 1],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_rows_map_to_settings_and_back() {
        assert_eq!(options()[1..], ["English", "Español"]);
        for row in 0..options().len() {
            assert_eq!(choice(setting(row)), row);
        }
        // An older file with no setting, or one this build doesn't ship, still has a row.
        assert_eq!(choice(""), 0);
        assert_eq!(choice("es-MX"), 2);
        assert_eq!(choice("fr"), 1);
    }
}
