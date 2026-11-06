//! Search registered actions, then return an ID to the app's existing action executor.
//! This owns only temporary view state; opening it never changes the focused dock panel.

use egui::{Id, Key, Modifiers, Ui};
use suite_ui::theme;
use suite_ui::tokens::{radius, size, space};
use suite_ui::widgets::TextInput;

use crate::i18n::{CATALOG, tr};
use crate::keymap::{ACTIONS, Applicability, Keymap};

#[derive(Default)]
pub struct CommandPalette {
    open: bool,
    query: String,
    selected: usize,
    focus_search: bool,
    previous_focus: Option<Id>,
    menu_focus: Option<Id>,
    pending_action: Option<&'static str>,
}

impl CommandPalette {
    pub fn open(&mut self, ctx: &egui::Context) {
        if !self.open {
            self.previous_focus = ctx.memory(|m| m.focused());
            self.query.clear();
            self.selected = 0;
            self.focus_search = true;
            self.open = true;
            self.pending_action = None;
            egui::Popup::close_all(ctx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn needs_frame(&self) -> bool {
        self.open || self.previous_focus.is_some() || self.pending_action.is_some()
    }

    /// Save before a menu button takes focus; its item disappears when the palette opens.
    pub fn remember_menu_focus(&mut self, focus: Option<Id>) {
        self.menu_focus = focus;
    }

    pub fn open_from_menu(&mut self, ctx: &egui::Context) {
        self.open(ctx);
        self.previous_focus = self.menu_focus.take();
    }

    fn close(&mut self, ctx: &egui::Context) {
        self.open = false;
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
            if let Some(id) = self.previous_focus {
                m.request_focus(id);
            }
        });
        ctx.request_repaint();
    }

    /// `applicability` must use the same target and checks as dispatch. The returned ID
    /// is returned the frame after dismissal, once egui releases the modal layer. Restore
    /// widget focus before dispatch, so an intentional focus action can then replace it.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        map: &Keymap,
        mut applicability: impl FnMut(&str) -> Applicability,
    ) -> Option<&'static str> {
        if !self.open {
            if let Some(id) = self.previous_focus.take() {
                ui.ctx().memory_mut(|m| m.request_focus(id));
            }
            return self.pending_action.take();
        }
        // Consume before the search field sees these; modifier shortcuts still edit text.
        let (up, down, enter, escape) = ui.ctx().input_mut(|i| {
            if !i.modifiers.is_none() {
                return (false, false, false, false);
            }
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            self.close(ui.ctx());
            return None;
        }
        let frame = egui::Frame::popup(ui.style())
            .corner_radius(radius::DIALOG)
            .inner_margin(space::S16);
        let budget = (ui.ctx().content_rect().size()
            - egui::Vec2::splat(2.0 * space::S8)
            - frame.total_margin().sum())
        .max(egui::Vec2::ZERO);
        let width = budget.x.min(560.0);
        let mut picked = None;
        let modal = egui::Modal::new(Id::new("command_palette"))
            .frame(frame)
            .show(ui.ctx(), |ui| {
                ui.set_width(width);
                let top = ui.cursor().top();
                ui.label(egui::RichText::new(tr!("palette.title")).font(theme::headline()));
                ui.add_space(space::S8);
                let search = ui.add(TextInput::search(&mut self.query).width(width));
                if self.focus_search {
                    search.request_focus();
                    self.focus_search = false;
                }
                if search.changed() {
                    self.selected = 0;
                }
                let found = matches(&self.query, map);
                self.selected = self.selected.min(found.len().saturating_sub(1));
                if !found.is_empty() {
                    if down {
                        self.selected = (self.selected + 1) % found.len();
                    }
                    if up {
                        self.selected = (self.selected + found.len() - 1) % found.len();
                    }
                }
                ui.add_space(space::S8);
                let height = (budget.y - (ui.cursor().top() - top)).clamp(0.0, 400.0);
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if found.is_empty() {
                            ui.label(tr!("palette.no_results"));
                        }
                        for (row, &index) in found.iter().enumerate() {
                            let action = &ACTIONS[index];
                            let available = applicability(action.id);
                            let selected = row == self.selected;
                            let shortcut = map.bindings.get(index).copied().flatten();
                            let shortcut =
                                shortcut.map_or_else(String::new, |s| ui.ctx().format_shortcut(&s));
                            let mut button = egui::Button::selectable(selected, action.name())
                                .wrap()
                                .min_size(egui::vec2(ui.available_width(), size::CONTROL));
                            if !shortcut.is_empty() {
                                button = button.shortcut_text(shortcut);
                            }
                            let response = ui.add_enabled(available.is_enabled(), button);
                            if selected && (up || down || search.changed()) {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                            if available.is_enabled() && (response.clicked() || (enter && selected))
                            {
                                picked = Some(action.id);
                            }
                            if let Some(reason) = available.reason() {
                                ui.add(egui::Label::new(reason).wrap());
                            }
                        }
                    });
            });
        if picked.is_some() || modal.should_close() {
            self.close(ui.ctx());
            self.pending_action = picked;
        }
        None
    }
}

/// Resolve names, aliases and bindings each frame; never cache a locale or default key.
fn matches(query: &str, map: &Keymap) -> Vec<usize> {
    let query = query.trim().to_lowercase().replace('_', " ");
    ACTIONS
        .iter()
        .enumerate()
        .filter_map(|(index, action)| {
            let english = CATALOG
                .get_in("en", &format!("action.{}", action.id))
                .unwrap_or(action.id);
            let aliases = CATALOG
                .get(&format!("action_aliases.{}", action.id))
                .unwrap_or("");
            let text = [
                action.name().to_owned(),
                english.to_owned(),
                action.id.replace('_', " "),
                action.aliases.join(" "),
                aliases.to_owned(),
                map.shortcut(action.id),
            ]
            .join(" ")
            .to_lowercase();
            query
                .split_whitespace()
                .all(|word| text.contains(word))
                .then_some(index)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn search_uses_aliases_ids_and_current_custom_or_unbound_shortcuts() {
        let map = Keymap::from_toml("export = \"Ctrl+Shift+F9\"\nmark_in = \"\"").unwrap();
        let ids = |query| {
            matches(query, &map)
                .into_iter()
                .map(|i| ACTIONS[i].id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("  RENDER  "), ["export"]);
        assert_eq!(ids("export ctrl+shift+f9"), ["export"]);
        assert!(!ids("ctrl+m").contains(&"export"));
        assert!(ids("mark_in").contains(&"mark_in"));
        assert_eq!(map.shortcut("mark_in"), "");
        assert_eq!(ids("voiceover"), ["record"]);
        assert_eq!(ids("NoSuchCommand"), Vec::<&str>::new());
    }

    fn harness(query: &str) -> Harness<'static, (CommandPalette, Keymap, Vec<&'static str>)> {
        let mut palette = CommandPalette::default();
        palette.open(&egui::Context::default());
        palette.query = query.into();
        let mut fresh = true;
        Harness::new_ui_state(
            move |ui, (palette, map, fired)| {
                if !suite_ui::test_support::install_theme(ui, &mut fresh)
                    && let Some(id) = palette.show(ui, map, |id| {
                        Applicability::when(id != "export", "export.nothing")
                    })
                {
                    fired.push(id);
                }
            },
            (palette, Keymap::default(), Vec::new()),
        )
    }

    #[test]
    fn disabled_enter_stays_open_navigation_wraps_and_escape_cancels() {
        let mut h = harness("export");
        h.run_steps(3);
        h.get_by_label(tr!("export.nothing"));
        h.key_press(Key::ArrowDown);
        h.run();
        assert_eq!(
            ACTIONS[matches("export", &h.state().1)[h.state().0.selected]].id,
            "export"
        );
        h.key_press(Key::Enter);
        h.run();
        assert!(h.state().0.is_open());
        assert!(h.state().2.is_empty());
        h.key_press(Key::ArrowUp);
        h.run();
        h.key_press(Key::Enter);
        h.run();
        assert_eq!(h.state().2, ["export_subtitles"]);
        assert!(!h.state().0.is_open());

        let ctx = h.ctx.clone();
        h.state_mut().0.open(&ctx);
        h.run_steps(2);
        h.key_press(Key::ArrowUp);
        h.run();
        assert_eq!(h.state().0.selected, ACTIONS.len() - 1);
        h.key_press(Key::ArrowDown);
        h.run();
        assert_eq!(h.state().0.selected, 0);
        h.key_press(Key::Escape);
        h.run();
        assert!(!h.state().0.is_open());
        assert_eq!(h.state().2.len(), 1);
    }

    #[test]
    fn open_palette_displays_live_custom_and_unbound_shortcuts() {
        let mut h = harness("mark_in");
        h.state_mut().1 = Keymap::from_toml("mark_in = \"Ctrl+Shift+F9\"").unwrap();
        h.run_steps(3);
        let name = CATALOG.get("action.mark_in").unwrap();
        let shortcut = h
            .ctx
            .format_shortcut(&crate::keymap::parse("Ctrl+Shift+F9").unwrap());
        h.get_by_label(&format!("{name} {shortcut}"));
        h.state_mut().1 = Keymap::from_toml("mark_in = \"\"").unwrap();
        h.run();
        h.get_by_label(name);
        assert!(h.state().0.is_open());
    }

    #[test]
    fn escape_restores_previous_widget_focus_and_typing_shortcuts_stay_in_search() {
        let state = (CommandPalette::default(), String::new());
        let mut fresh = true;
        let mut h = Harness::new_ui_state(
            move |ui, (palette, text)| {
                if suite_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                ui.add(egui::TextEdit::singleline(text).id(Id::new("prior_input")));
                palette.show(ui, &Keymap::default(), |_| Applicability::Enabled);
            },
            state,
        );
        h.run_steps(2);
        h.get_by_role(egui::accesskit::Role::TextInput).focus();
        h.run();
        let previous = h.ctx.memory(|m| m.focused());
        let ctx = h.ctx.clone();
        h.state_mut().0.open(&ctx);
        h.run_steps(2);
        assert_ne!(h.ctx.memory(|m| m.focused()), previous);
        h.key_press_modifiers(Modifiers::COMMAND, Key::A);
        h.run();
        assert!(h.state().0.is_open());
        assert!(Keymap::default().pressed(&h.ctx, true).is_empty());
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert_eq!(h.ctx.memory(|m| m.focused()), previous);
        h.state_mut().0.open(&ctx);
        h.state_mut().0.query = "snapping".into();
        h.run_steps(2);
        h.key_press(Key::Enter);
        h.run_steps(2);
        assert!(!h.state().0.is_open());
        assert_eq!(h.ctx.memory(|m| m.focused()), previous);
        h.state_mut().0.remember_menu_focus(previous);
        ctx.memory_mut(|m| m.request_focus(Id::new("menu_button")));
        h.state_mut().0.open_from_menu(&ctx);
        h.run_steps(2);
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert_eq!(h.ctx.memory(|m| m.focused()), previous);
    }

    #[test]
    fn modal_fits_narrow_short_viewports() {
        for size in [
            [240.0, 240.0],
            [320.0, 240.0],
            [640.0, 400.0],
            [768.0, 480.0],
        ] {
            let mut h = harness("");
            h.set_size(size.into());
            h.run_steps(4);
            let area = h
                .ctx
                .memory(|m| m.area_rect(Id::new("command_palette")))
                .unwrap();
            assert!(
                h.ctx.content_rect().contains_rect(area),
                "{size:?}: {area:?}"
            );
        }
    }

    #[test]
    #[ignore = "renders review artifacts; run explicitly with JVID_PALETTE_ARTIFACT_DIR"]
    fn render_review_artifacts() {
        assert!(suite_ui::test_support::gpu());
        let directory = std::path::PathBuf::from(
            std::env::var_os("JVID_PALETTE_ARTIFACT_DIR").expect("artifact directory"),
        );
        std::fs::create_dir_all(&directory).unwrap();
        for (size, query, name) in [
            ([640.0, 400.0], "export", "command-palette-disabled.png"),
            ([320.0, 240.0], "", "command-palette-narrow.png"),
        ] {
            let mut h = harness(query);
            h.set_size(size.into());
            h.run_steps(4);
            h.render().unwrap().save(directory.join(name)).unwrap();
        }
    }

    // Locale is process-global, so validate switching in its own test process.
    #[test]
    #[ignore = "changes global locale; run this test in isolation"]
    fn search_and_open_palette_follow_language_switches() {
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                suite_ui::i18n::set_language("en");
            }
        }
        let _restore = Restore;
        let mut h = harness("");
        h.run_steps(2);
        for language in ["es", "de", "fr", "ja", "en"] {
            suite_ui::i18n::set_language(language);
            modal_fits_narrow_short_viewports();
            let name = CATALOG.get("action.export").unwrap();
            assert!(
                matches(name, &h.state().1)
                    .iter()
                    .any(|&i| ACTIONS[i].id == "export")
            );
            h.state_mut().0.query = "render".into();
            h.run();
            h.get_by_label(tr!("export.nothing"));
            assert_eq!(
                Applicability::Disabled("export.nothing").reason(),
                CATALOG.get("export.nothing")
            );
        }
    }
}
