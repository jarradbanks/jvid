//! The keyboard shortcut editor: search actions, record shortcuts, import and export keymaps.
//! Edits apply to the live keymap at once; the app saves them to `keymap.toml`.

use egui::{Event, Key, KeyboardShortcut, Ui};
use rfd::FileDialog;
use suite_ui::theme;
use suite_ui::tokens::{radius, space};
use suite_ui::widgets::{Button, TextInput};

use crate::keymap::{self, ACTIONS, Keymap};

#[derive(Default)]
pub struct KeymapEditor {
    open: bool,
    search: String,
    /// Action index waiting for a key press.
    recording: Option<usize>,
    error: Option<String>,
}

impl KeymapEditor {
    pub fn open(&mut self) {
        self.open = true;
    }

    /// Shortcuts must not fire while the editor is up, or recording a key would also run it.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Shows the editor while open, editing `map` in place; true when it changed.
    pub fn show(&mut self, ui: &mut Ui, map: &mut Keymap) -> bool {
        if !self.open {
            return false;
        }
        let before = map.clone();
        let was_recording = self.recording.is_some();
        if let Some(i) = self.recording {
            match recorded(ui.ctx()) {
                Some(Recorded::Cancel) => self.recording = None,
                Some(Recorded::Unbind) => (map.bindings[i], self.recording) = (None, None),
                Some(Recorded::Key(sc)) => (map.bindings[i], self.recording) = (Some(sc), None),
                None => {}
            }
        }
        let defaults = Keymap::default();
        let conflicts = map.conflicts();
        let modal = egui::Modal::new(egui::Id::new("keymap_editor"))
            .frame(
                egui::Frame::popup(ui.style())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(space::S16),
            )
            .show(ui.ctx(), |ui| {
                ui.set_width(560.0);
                ui.label(egui::RichText::new("Keyboard shortcuts").font(theme::headline()));
                ui.add_space(space::S8);
                ui.add(TextInput::search(&mut self.search).width(560.0));
                ui.add_space(space::S8);
                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .show(ui, |ui| {
                        egui::Grid::new("keymap_grid")
                            .spacing([space::S8, space::S4])
                            .show(ui, |ui| {
                                for i in matches(&self.search) {
                                    let a = &ACTIONS[i];
                                    ui.label(a.name);
                                    let label = if self.recording == Some(i) {
                                        "Press keys… (Esc cancels, Backspace unbinds)".to_owned()
                                    } else {
                                        map.bindings[i]
                                            .as_ref()
                                            .map_or("—".to_owned(), keymap::format)
                                    };
                                    if ui
                                        .add(
                                            Button::secondary(&label).on(self.recording == Some(i)),
                                        )
                                        .on_hover_text("Click, then press the new shortcut")
                                        .clicked()
                                    {
                                        self.recording = Some(i);
                                    }
                                    let clash: Vec<_> = conflicts
                                        .iter()
                                        .filter_map(|&(x, y)| match (x == a.id, y == a.id) {
                                            (true, _) => Some(y),
                                            (_, true) => Some(x),
                                            _ => None,
                                        })
                                        .filter_map(|id| ACTIONS.iter().find(|b| b.id == id))
                                        .map(|b| b.name)
                                        .collect();
                                    if clash.is_empty() {
                                        ui.label("");
                                    } else {
                                        ui.colored_label(
                                            ui.visuals().warn_fg_color,
                                            format!("Also {}", clash.join(", ")),
                                        );
                                    }
                                    if map.bindings[i] != defaults.bindings[i]
                                        && ui
                                            .add(Button::ghost("Reset"))
                                            .on_hover_text(format!("Back to {}", a.default))
                                            .clicked()
                                    {
                                        map.bindings[i] = defaults.bindings[i];
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                if let Some(e) = &self.error {
                    ui.add_space(space::S8);
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.add_space(space::S16);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = space::S8;
                    if ui.add(Button::secondary("Import…")).clicked()
                        && let Some(p) = toml_dialog().pick_file()
                    {
                        match std::fs::read_to_string(&p)
                            .map_err(|e| e.to_string())
                            .and_then(|s| Keymap::from_toml(&s).map_err(|e| e.to_string()))
                        {
                            Ok(m) => (*map, self.error) = (m, None),
                            Err(e) => self.error = Some(format!("Couldn't import: {e}")),
                        }
                    }
                    if ui.add(Button::secondary("Export…")).clicked()
                        && let Some(p) = toml_dialog().set_file_name("keymap.toml").save_file()
                        && let Err(e) = std::fs::write(&p, map.to_toml())
                    {
                        self.error = Some(format!("Couldn't export: {e}"));
                    }
                    if ui.add(Button::secondary("Reset all")).clicked() {
                        *map = Keymap::default();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(Button::primary("Done")).clicked() {
                            ui.close();
                        }
                    });
                });
            });
        // Escape while recording cancels the recording, not the dialog.
        if modal.should_close() && !was_recording {
            self.open = false;
            self.recording = None;
        }
        *map != before
    }
}

enum Recorded {
    Cancel,
    Unbind,
    Key(KeyboardShortcut),
}

/// The first key press this frame, consumed so nothing else sees it.
fn recorded(ctx: &egui::Context) -> Option<Recorded> {
    ctx.input_mut(|i| {
        let pos = i
            .events
            .iter()
            .position(|e| matches!(e, Event::Key { pressed: true, .. }))?;
        let Event::Key {
            key,
            physical_key,
            modifiers,
            ..
        } = i.events.remove(pos)
        else {
            unreachable!()
        };
        Some(match key {
            Key::Escape => Recorded::Cancel,
            Key::Backspace | Key::Delete if modifiers.is_none() => Recorded::Unbind,
            // Physical, so Shift+1 records as Shift+1 rather than `!`.
            _ => Recorded::Key(KeyboardShortcut::new(
                modifiers,
                physical_key.unwrap_or(key),
            )),
        })
    })
}

/// Indices of actions whose name, id or shortcut default contains `query`, ignoring case.
fn matches(query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    (0..ACTIONS.len())
        .filter(|&i| {
            let a = &ACTIONS[i];
            [a.name, a.id, a.default]
                .iter()
                .any(|s| s.to_lowercase().contains(&q))
        })
        .collect()
}

fn toml_dialog() -> FileDialog {
    FileDialog::new().add_filter("Keymap", &["toml"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn search_matches_names_case_insensitively() {
        let found: Vec<_> = matches("MARK").iter().map(|&i| ACTIONS[i].id).collect();
        assert!(found.contains(&"mark_in") && found.contains(&"mark_clip"));
        assert!(!found.contains(&"lift"));
        assert_eq!(matches("").len(), ACTIONS.len());
    }

    #[test]
    fn recorded_shortcut_applies_immediately() {
        let mut state = (KeymapEditor::default(), Keymap::default());
        state.0.open();
        state.0.search = "Mark In".into();
        let mut fresh = true;
        let mut h = Harness::new_ui_state(
            move |ui, (ed, map)| {
                if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                    ed.show(ui, map);
                }
            },
            state,
        );
        h.run_steps(2);
        h.get_by_label("I").click();
        h.run();
        h.key_press_modifiers(Modifiers::COMMAND, Key::M);
        h.run();
        let i = ACTIONS.iter().position(|a| a.id == "mark_in").unwrap();
        let sc = KeyboardShortcut::new(Modifiers::COMMAND, Key::M);
        assert_eq!(h.state().1.bindings[i], Some(sc));
        assert!(h.state().0.is_open());
    }
}
