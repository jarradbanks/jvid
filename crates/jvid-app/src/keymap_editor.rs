//! The keyboard shortcut editor: search actions, record shortcuts, import and export keymaps.
//! Edits apply to the live keymap at once; the app saves them to `keymap.toml`.

use egui::{Event, Key, KeyboardShortcut, Ui};
use rfd::FileDialog;
use suite_ui::theme;
use suite_ui::tokens::{radius, size, space};
use suite_ui::widgets::{Button, TextInput};

use crate::i18n::tr;
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
        let frame = egui::Frame::popup(ui.style())
            .corner_radius(radius::DIALOG)
            .inner_margin(space::S16);
        let budget = (ui.ctx().content_rect().size()
            - egui::Vec2::splat(2.0 * space::S8)
            - frame.total_margin().sum())
        .max(egui::Vec2::ZERO);
        let width = 560.0_f32.min(budget.x);
        let modal = egui::Modal::new(egui::Id::new("keymap_editor"))
            .frame(frame)
            .show(ui.ctx(), |ui| {
                ui.set_width(width);
                let top = ui.cursor().top();
                ui.label(egui::RichText::new(tr!("keymap.title")).font(theme::headline()));
                ui.add_space(space::S8);
                ui.add(TextInput::search(&mut self.search).width(width));
                ui.add_space(space::S8);
                // Measure the translated footer and bounded error before assigning the list
                // its remaining height. The sizing child cannot receive input or paint.
                let mut tail = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt("footer_measure")
                        .sizing_pass()
                        .invisible()
                        .max_rect(egui::Rect::from_min_size(
                            ui.cursor().min,
                            egui::vec2(width, f32::INFINITY),
                        )),
                );
                footer(&mut tail, self.error.as_deref(), true);
                let list_height = (budget.y
                    - (ui.cursor().top() - top)
                    - tail.min_size().y
                    - ui.spacing().item_spacing.y)
                    .clamp(0.0, 400.0);
                egui::ScrollArea::vertical()
                    .max_height(list_height)
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for i in matches(&self.search) {
                            let a = &ACTIONS[i];
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = space::S8;
                                ui.label(a.name());
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        // Hidden, not absent, so chips stay aligned across rows.
                                        if ui
                                            .add_visible(
                                                map.bindings[i] != defaults.bindings[i],
                                                Button::ghost(tr!("keymap.reset")),
                                            )
                                            .on_hover_text(tr!(
                                                "keymap.back_to",
                                                shortcut = a.default
                                            ))
                                            .clicked()
                                        {
                                            map.bindings[i] = defaults.bindings[i];
                                        }
                                        let clash: Vec<_> = conflicts
                                            .iter()
                                            .filter_map(|&(x, y)| match (x == a.id, y == a.id) {
                                                (true, _) => Some(y),
                                                (_, true) => Some(x),
                                                _ => None,
                                            })
                                            .filter_map(|id| ACTIONS.iter().find(|b| b.id == id))
                                            .map(|b| b.name())
                                            .collect();
                                        if !clash.is_empty() {
                                            ui.colored_label(
                                                ui.visuals().warn_fg_color,
                                                tr!("keymap.also", actions = clash.join(", ")),
                                            );
                                        }
                                        let label = if self.recording == Some(i) {
                                            tr!("keymap.recording").to_owned()
                                        } else {
                                            map.bindings[i].as_ref().map_or("—".to_owned(), shown)
                                        };
                                        if ui
                                            .add(
                                                Button::secondary(&label)
                                                    .on(self.recording == Some(i)),
                                            )
                                            .on_hover_text(tr!("keymap.record_tooltip"))
                                            .clicked()
                                        {
                                            self.recording = Some(i);
                                        }
                                    },
                                );
                            });
                        }
                    });
                match footer(ui, self.error.as_deref(), false) {
                    Some(Footer::Import) => {
                        if let Some(p) = toml_dialog().pick_file() {
                            match std::fs::read_to_string(&p)
                                .map_err(|e| e.to_string())
                                .and_then(|s| Keymap::from_toml(&s).map_err(|e| e.to_string()))
                            {
                                Ok(m) => (*map, self.error) = (m, None),
                                Err(e) => self.error = Some(tr!("keymap.import_error", error = e)),
                            }
                        }
                    }
                    Some(Footer::Export) => {
                        if let Some(p) = toml_dialog().set_file_name("keymap.toml").save_file()
                            && let Err(e) = std::fs::write(&p, map.to_toml())
                        {
                            self.error = Some(tr!("keymap.export_error", error = e));
                        }
                    }
                    Some(Footer::Reset) => *map = Keymap::default(),
                    Some(Footer::Done) => ui.close(),
                    None => {}
                }
            });
        // Escape while recording cancels the recording, not the dialog.
        if modal.should_close() && !was_recording {
            self.open = false;
            self.recording = None;
        }
        *map != before
    }
}

#[derive(Clone, Copy)]
enum Footer {
    Import,
    Export,
    Reset,
    Done,
}

/// The same chrome is measured and drawn, so translation and wrapping affect the list budget.
fn footer(ui: &mut Ui, error: Option<&str>, measure: bool) -> Option<Footer> {
    if let Some(error) = error {
        ui.add_space(space::S8);
        if measure {
            ui.allocate_space(egui::vec2(
                ui.available_width(),
                ui.text_style_height(&egui::TextStyle::Body),
            ));
        } else {
            ui.add(
                egui::Label::new(egui::RichText::new(error).color(ui.visuals().error_fg_color))
                    .truncate(),
            )
            .on_hover_text(error);
        }
    }
    ui.add_space(space::S16);
    let labels = [
        tr!("keymap.import"),
        tr!("keymap.export"),
        tr!("keymap.reset_all"),
        tr!("settings.done"),
    ];
    let widths = labels.map(|label| {
        let galley = ui.painter().layout_no_wrap(
            label.to_owned(),
            theme::body_strong(),
            ui.visuals().text_color(),
        );
        (galley.size().x + 2.0 * space::S12).ceil()
    });
    let total = widths.iter().sum::<f32>() + 3.0 * space::S8;
    let one_row = total <= ui.available_width();
    let button = |ui: &mut Ui, i: usize, primary: bool| {
        if measure {
            ui.allocate_space(egui::vec2(widths[i], size::CONTROL));
            false
        } else {
            ui.add(if primary {
                Button::primary(labels[i])
            } else {
                Button::secondary(labels[i])
            })
            .clicked()
        }
    };
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = space::S8;
        for (i, value) in [Footer::Import, Footer::Export, Footer::Reset]
            .into_iter()
            .enumerate()
        {
            if button(ui, i, false) {
                action = Some(value);
            }
        }
        let mut done = |ui: &mut Ui| {
            if button(ui, 3, true) {
                action = Some(Footer::Done);
            }
        };
        if one_row {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), done);
        } else {
            done(ui);
        }
    });
    action
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
            [a.name(), a.id, a.default]
                .iter()
                .any(|s| s.to_lowercase().contains(&q))
        })
        .collect()
}

/// [`keymap::format`] with punctuation keys as their symbols: "Ctrl+/" not "Ctrl+Slash".
fn shown(sc: &KeyboardShortcut) -> String {
    let full = keymap::format(sc);
    let name = sc.logical_key.name();
    let key = match sc.logical_key {
        // egui's arrow glyphs and Unicode minus read worse than the names.
        Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight => name,
        Key::Minus => "-",
        k => k.symbol_or_name(),
    };
    format!("{}{key}", &full[..full.len() - name.len()])
}

fn toml_dialog() -> FileDialog {
    FileDialog::new().add_filter(tr!("keymap.file_type"), &["toml"])
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
        assert_eq!(matches("edit point").len(), 4);
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

    #[test]
    fn punctuation_keys_show_as_symbols() {
        let sc = |m, k| shown(&KeyboardShortcut::new(m, k));
        assert_eq!(sc(Modifiers::COMMAND, Key::Slash), "Ctrl+/");
        assert_eq!(sc(Modifiers::NONE, Key::Semicolon), ";");
        assert_eq!(sc(Modifiers::SHIFT, Key::Comma), "Shift+,");
        assert_eq!(sc(Modifiers::NONE, Key::Minus), "-");
        assert_eq!(sc(Modifiers::NONE, Key::ArrowLeft), "Left");
    }

    #[test]
    fn chrome_fits_short_logical_viewports_even_with_an_import_error() {
        for size in [[960.0, 600.0], [768.0, 480.0], [640.0, 400.0]] {
            for scale in [1.0, 1.5] {
                let mut editor = KeymapEditor::default();
                editor.open();
                editor.error = Some(format!(
                    "Could not import keyboard shortcuts from {}: invalid shortcut definition",
                    "a very long folder name/".repeat(12)
                ));
                let mut fresh = true;
                let mut h = Harness::builder()
                    .with_size(size)
                    .with_pixels_per_point(scale)
                    .build_ui_state(
                        move |ui, (ed, map)| {
                            if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                                ed.show(ui, map);
                            }
                        },
                        (editor, Keymap::default()),
                    );
                h.run_steps(4);
                let viewport = h.ctx.content_rect();
                let area = h
                    .ctx
                    .memory(|m| m.area_rect(egui::Id::new("keymap_editor")))
                    .unwrap();
                assert!(
                    viewport.contains_rect(area),
                    "{size:?} at {scale}: {area:?}"
                );
                for label in [
                    tr!("keymap.title"),
                    tr!("keymap.import"),
                    tr!("keymap.export"),
                    tr!("keymap.reset_all"),
                    tr!("settings.done"),
                ] {
                    let rect = h.get_by_label(label).rect();
                    assert!(viewport.contains_rect(rect), "{label}: {rect:?}");
                }
                let search = h.get_by_role(egui::accesskit::Role::TextInput).rect();
                assert!(viewport.contains_rect(search), "search: {search:?}");
                let error = h.get_by_label(h.state().0.error.as_deref().unwrap()).rect();
                assert!(viewport.contains_rect(error), "error: {error:?}");
                let footer = h.get_by_label(tr!("settings.done")).rect();
                let first = h.get_by_label(ACTIONS[0].name()).rect();
                h.hover_at(first.center());
                h.event(Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -240.0),
                    modifiers: Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                });
                h.run();
                assert!(h.get_by_label(ACTIONS[0].name()).rect().top() < first.top());
                assert_eq!(h.get_by_label(tr!("settings.done")).rect(), footer);
                h.get_by_label(tr!("settings.done")).click();
                h.run();
                assert!(!h.state().0.is_open(), "Done remains directly reachable");
            }
        }
    }

    // Locale changes are process-global. Keep this out of the English unit-test process;
    // run it explicitly with `--ignored --exact` when validating translated layouts.
    #[test]
    #[ignore = "changes global locale; run this test in isolation"]
    fn translated_chrome_fits_short_logical_viewports() {
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                suite_ui::i18n::set_pseudo(false);
                suite_ui::i18n::set_language("en");
            }
        }
        let _restore = Restore;
        for language in ["de", "fr", "pt-BR", "en"] {
            suite_ui::i18n::set_language(language);
            suite_ui::i18n::set_pseudo(language == "en");
            chrome_fits_short_logical_viewports_even_with_an_import_error();
        }
    }

    #[test]
    fn list_fills_dialog_width() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut state = (KeymapEditor::default(), Keymap::default());
        state.0.open();
        let mut fresh = true;
        let mut h = Harness::builder().with_size([800.0, 600.0]).build_ui_state(
            move |ui, (ed, map)| {
                if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                    ed.show(ui, map);
                }
            },
            state,
        );
        h.run_steps(2);
        h.snapshot("keymap_editor");
    }
}
