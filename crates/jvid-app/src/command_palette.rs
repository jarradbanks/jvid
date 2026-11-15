//! Search registered actions, then return an ID to the app's existing action executor.
//! This owns only temporary view state; opening it never changes the focused dock panel.

use egui::{Id, Key, Modifiers, Rect, Response, Sense, Ui, UiBuilder, vec2};
use j_ui::theme::{self, icon};
use j_ui::tokens::{color, radius, space};
use j_ui::widgets::{Button, fit_text, focus_ring, hover_full, paint_icon};

use crate::i18n::{CATALOG, tr};
use crate::keymap::{ACTIONS, Applicability, Keymap};
use crate::timeline::menus;

/// The approved outer size; smaller viewports shrink it, never the 40pt rows.
const MAX_SIZE: egui::Vec2 = vec2(720.0, 560.0);
const HEADER: f32 = 56.0;
const FOOTER: f32 = 40.0;
/// Result rows and the search field share one height.
const ROW: f32 = 40.0;
const KEYCAP: f32 = 20.0;

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
            ctx.request_repaint();
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
            .inner_margin(0);
        let size = (ui.ctx().content_rect().size() - egui::Vec2::splat(2.0 * space::S24))
            .min(MAX_SIZE)
            .max(egui::Vec2::ZERO);
        let mut picked = None;
        let mut dismiss = false;
        let modal = egui::Modal::new(Id::new("command_palette"))
            .frame(frame)
            .show(ui.ctx(), |ui| {
                let (outer, _) = ui.allocate_exact_size(size, Sense::hover());
                let header = Rect::from_min_size(outer.min, vec2(size.x, HEADER));
                let field = Rect::from_min_size(
                    outer.min + vec2(space::S16, HEADER),
                    vec2(size.x - 2.0 * space::S16, ROW),
                )
                .intersect(outer);
                let footer = Rect::from_min_max(
                    egui::pos2(outer.left(), (outer.bottom() - FOOTER).max(outer.top())),
                    outer.max,
                );
                let list = Rect::from_x_y_ranges(
                    outer.left() + space::S8..=outer.right() - space::S8,
                    (field.bottom() + space::S16).min(footer.top())..=footer.top(),
                );
                dismiss = paint_header(ui, header);
                let search = self.search(ui, field, map);
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
                let moved = up || down || search.changed();
                let mut list_ui = ui.new_child(UiBuilder::new().max_rect(list));
                egui::ScrollArea::vertical()
                    .max_height(list.height())
                    .auto_shrink(false)
                    .show(&mut list_ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if found.is_empty() {
                            ui.add_space(space::S16);
                            ui.vertical_centered(|ui| {
                                ui.add(egui::Label::new(tr!("palette.no_results")).wrap())
                            });
                        }
                        for (row, &index) in found.iter().enumerate() {
                            let selected = row == self.selected;
                            let available = applicability(ACTIONS[index].id);
                            let response = result_row(ui, map, index, available, selected);
                            if selected && moved {
                                response.scroll_to_me(None);
                            }
                            if available.is_enabled() && (response.clicked() || (enter && selected))
                            {
                                picked = Some(ACTIONS[index].id);
                            }
                        }
                    });
                let selected = found
                    .get(self.selected)
                    .map_or("", |&i| category(ACTIONS[i].id));
                paint_footer(ui, footer, selected);
            });
        if dismiss {
            self.close(ui.ctx());
            return None;
        }
        if picked.is_some() || modal.should_close() {
            self.close(ui.ctx());
            self.pending_action = picked;
        }
        None
    }
}

impl CommandPalette {
    /// A 40pt search field: j-ui's TextInput is fixed at control height, so its frame is
    /// repainted here around a frameless TextEdit with the same colours.
    fn search(&mut self, ui: &mut Ui, rect: Rect, map: &Keymap) -> Response {
        let painter = ui.painter().clone();
        let frame = painter.add(egui::Shape::Noop);
        paint_icon(
            &painter,
            egui::pos2(rect.left() + space::S12 + space::S8, rect.center().y),
            icon::SEARCH,
            color::ink_3(),
        );
        let keys = keycaps(
            ui,
            rect.right() - space::S12,
            rect.center().y,
            &map.shortcut("command_palette"),
        );
        let text = Rect::from_x_y_ranges(
            rect.left() + space::S12 + space::S16 + space::S8..=keys - space::S8,
            rect.y_range(),
        );
        let response = ui.put(
            text,
            egui::TextEdit::singleline(&mut self.query)
                .frame(egui::Frame::NONE)
                .margin(0)
                .vertical_align(egui::Align::Center)
                .font(theme::body())
                .text_color(color::ink())
                .hint_text(tr!("palette.search")),
        );
        if self.focus_search {
            response.request_focus();
            self.focus_search = false;
        }
        let focused = response.has_focus();
        let border = if focused {
            color::blue()
        } else {
            color::border_strong()
        };
        let mut shapes = vec![
            egui::Shape::rect_filled(rect, radius::CONTROL, color::field()),
            egui::Shape::rect_stroke(
                rect,
                radius::CONTROL,
                theme::hairline(ui.ctx(), border),
                egui::StrokeKind::Inside,
            ),
        ];
        if focused {
            shapes.push(focus_ring(rect, radius::CONTROL));
        }
        painter.set(frame, egui::Shape::Vec(shapes));
        response
    }
}

/// Title and close button; returns true when the close button was clicked.
fn paint_header(ui: &mut Ui, rect: Rect) -> bool {
    let title = fit_text(
        ui,
        tr!("palette.title"),
        theme::headline(),
        color::ink(),
        rect.width() - 3.0 * space::S16 - space::S24,
    );
    let pos = egui::pos2(
        rect.left() + space::S16,
        rect.center().y - title.size().y / 2.0,
    );
    j_ui::widgets::paint_fit(ui, pos, title, color::ink());
    let close = Rect::from_center_size(
        egui::pos2(rect.right() - space::S16 - space::S12, rect.center().y),
        egui::Vec2::splat(space::S24),
    );
    ui.put(close, Button::icon(icon::X, tr!("palette.close")))
        .clicked()
}

/// One 40pt result: name over category or disabled reason, shortcut keycaps on the right.
/// Hover and selection only tint the background; no accent strip.
fn result_row(
    ui: &mut Ui,
    map: &Keymap,
    index: usize,
    available: Applicability,
    selected: bool,
) -> Response {
    let action = &ACTIONS[index];
    let sense = if available.is_enabled() {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), sense);
    let shortcut = map
        .bindings
        .get(index)
        .copied()
        .flatten()
        .map_or_else(String::new, |s| ui.ctx().format_shortcut(&s));
    let detail = available.reason().unwrap_or_else(|| category(action.id));
    let label = [action.name(), detail, &shortcut].join(" ");
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            available.is_enabled(),
            selected,
            label.trim(),
        )
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    if selected || response.hovered() {
        ui.painter()
            .rect_filled(rect, radius::CONTROL, color::border());
    }
    if response.has_focus() {
        ui.painter().add(focus_ring(rect, radius::CONTROL));
    }
    let keys = keycaps(ui, rect.right() - space::S12, rect.center().y, &shortcut);
    let width = keys - space::S16 - (rect.left() + space::S12);
    let ink = if available.is_enabled() {
        color::ink()
    } else {
        color::ink_3()
    };
    let name = fit_text(ui, action.name(), theme::body(), ink, width);
    let detail = fit_text(ui, detail, theme::small(), color::ink_3(), width);
    let top = rect.center().y - (name.size().y + detail.size().y) / 2.0;
    let left = rect.left() + space::S12;
    let elided = name.elided || detail.elided;
    let full = format!("{}\n{}", name.text(), detail.text());
    ui.painter()
        .galley(egui::pos2(left, top), name.clone(), ink);
    ui.painter().galley(
        egui::pos2(left, top + name.size().y),
        detail,
        color::ink_3(),
    );
    if elided {
        return response.on_hover_text(full);
    }
    hover_full(response, &name)
}

/// Paints `shortcut` as keycaps ending at `right`; returns their left edge.
/// shortcut: splits egui's formatted text on '+', so a bare Plus key shows as one empty cap pair; upgrade if Plus is ever bound by default.
fn keycaps(ui: &Ui, right: f32, center_y: f32, shortcut: &str) -> f32 {
    let mut x = right;
    for key in shortcut
        .split('+')
        .rev()
        .filter(|k| !shortcut.is_empty() && !k.is_empty())
    {
        let galley = ui
            .painter()
            .layout_no_wrap(key.into(), theme::small(), color::ink_2());
        let width = (galley.size().x + 2.0 * space::S4 + 2.0).max(KEYCAP);
        let rect = Rect::from_min_max(
            egui::pos2(x - width, center_y - KEYCAP / 2.0),
            egui::pos2(x, center_y + KEYCAP / 2.0),
        );
        let painter = ui.painter();
        painter.rect_stroke(
            rect,
            radius::SMALL,
            theme::hairline(ui.ctx(), color::border_strong()),
            egui::StrokeKind::Inside,
        );
        painter.galley(rect.center() - galley.size() / 2.0, galley, color::ink_2());
        x = rect.left() - space::S4;
    }
    x
}

/// Keyboard hints on the left, the selected result's category on the right.
fn paint_footer(ui: &Ui, rect: Rect, category: &str) {
    let painter = ui.painter();
    let line = theme::hairline(ui.ctx(), color::border());
    painter.hline(rect.x_range(), rect.top(), line);
    let side = rect.shrink2(vec2(space::S16, 0.0));
    let category = fit_text(
        ui,
        category,
        theme::small(),
        color::ink_3(),
        side.width() / 3.0,
    );
    let hints = fit_text(
        ui,
        tr!("palette.hints"),
        theme::small(),
        color::ink_3(),
        side.width() - category.size().x - space::S16,
    );
    let y = rect.center().y;
    let hint_pos = egui::pos2(side.left(), y - hints.size().y / 2.0);
    let category_pos = egui::pos2(
        side.right() - category.size().x,
        y - category.size().y / 2.0,
    );
    j_ui::widgets::paint_fit(ui, hint_pos, hints, color::ink_3());
    j_ui::widgets::paint_fit(ui, category_pos, category, color::ink_3());
}

/// The group a result belongs to, as the menus that hold it name it.
fn category(id: &str) -> &'static str {
    if id.starts_with("workspace_") || ["maximize_panel", "reset_layout"].contains(&id) {
        return tr!("workspace.menu");
    }
    if id.starts_with("focus_") || id == "command_palette" {
        return tr!("palette.navigation");
    }
    if id.starts_with("media_") {
        return tr!("panel.media");
    }
    if menus::FILE.contains(&id) {
        return tr!("palette.project");
    }
    if let Some((label, _)) = menus::EDIT_GROUPS.iter().find(|(_, ids)| ids.contains(&id)) {
        return CATALOG.tr(label);
    }
    if menus::EDIT.contains(&id) {
        return tr!("menu.edit");
    }
    tr!("menu.timeline")
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
    use egui_kittest::kittest::{NodeT, Queryable};

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
        sized_harness(query, [800.0, 600.0], 1.0)
    }

    fn sized_harness(
        query: &str,
        size: [f32; 2],
        scale: f32,
    ) -> Harness<'static, (CommandPalette, Keymap, Vec<&'static str>)> {
        let mut palette = CommandPalette::default();
        palette.open(&egui::Context::default());
        palette.query = query.into();
        let mut fresh = true;
        Harness::builder()
            .with_size(size)
            .with_pixels_per_point(scale)
            .build_ui_state(
                move |ui, (palette, map, fired)| {
                    if !j_ui::test_support::install_theme(ui, &mut fresh)
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
        h.get_by_label_contains(tr!("export.nothing"));
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
        let row = h.get_by_label_contains(name);
        assert!(row.accesskit_node().label().unwrap().ends_with(&shortcut));
        h.state_mut().1 = Keymap::from_toml("mark_in = \"\"").unwrap();
        h.run();
        let row = h.get_by_label_contains(name);
        assert!(!row.accesskit_node().label().unwrap().contains(&shortcut));
        assert!(h.state().0.is_open());
    }

    #[test]
    fn escape_restores_previous_widget_focus_and_typing_shortcuts_stay_in_search() {
        let state = (CommandPalette::default(), String::new());
        let mut fresh = true;
        let mut h = Harness::new_ui_state(
            move |ui, (palette, text)| {
                if j_ui::test_support::install_theme(ui, &mut fresh) {
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

    /// The rendered result rows, top to bottom, as (label, rect).
    fn rows(h: &Harness<'_, (CommandPalette, Keymap, Vec<&'static str>)>) -> Vec<(String, Rect)> {
        let mut rows: Vec<_> = h
            .get_all_by_role(egui::accesskit::Role::Button)
            .filter(|n| {
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l != tr!("palette.close"))
            })
            .map(|n| (n.accesskit_node().label().unwrap(), n.rect()))
            .collect();
        rows.sort_by(|a, b| a.1.top().total_cmp(&b.1.top()));
        rows
    }

    #[test]
    fn layout_keeps_40pt_rows_inside_a_fixed_frame_and_reaches_the_last_result() {
        for size in [[1280.0, 720.0], [1920.0, 1080.0], [640.0, 400.0]] {
            for scale in [1.0, 1.5] {
                let mut h = sized_harness("", size, scale);
                h.run_steps(4);
                let screen = h.ctx.content_rect();
                let area = h
                    .ctx
                    .memory(|m| m.area_rect(Id::new("command_palette")))
                    .unwrap();
                let at = format!("{size:?}@{scale}");
                assert!(screen.contains_rect(area), "{at}: {area:?}");
                let wanted = (screen.size() - egui::Vec2::splat(2.0 * space::S24)).min(MAX_SIZE);
                // The popup frame adds up to a 1pt stroke on each side.
                let extra = area.size() - wanted;
                assert!((0.0..=2.5).contains(&extra.min_elem()), "{at}: {area:?}");
                assert!(extra.max_elem() <= 2.5, "{at}: {area:?}");
                let first = rows(&h);
                assert!(
                    first.iter().all(|(_, r)| (r.height() - ROW).abs() < 0.5),
                    "{at}"
                );
                let list_bottom = area.bottom() - FOOTER;
                let visible = first
                    .iter()
                    .filter(|(_, r)| r.bottom() <= list_bottom + 0.5);
                assert!(visible.count() >= 2, "{at}");

                h.key_press(Key::ArrowUp);
                h.run_steps(3);
                assert_eq!(h.state().0.selected, ACTIONS.len() - 1);
                let last = ACTIONS[ACTIONS.len() - 1].name();
                let rect = h.get_by_label_contains(last).rect();
                assert!(rect.top() >= area.top() + HEADER + ROW, "{at}: {rect:?}");
                assert!(rect.bottom() <= list_bottom + 0.5, "{at}: {rect:?}");
            }
        }
    }

    #[test]
    fn empty_results_are_explained_and_enter_dispatches_nothing() {
        let mut h = harness("NoSuchCommand");
        h.run_steps(3);
        h.get_by_label(tr!("palette.no_results"));
        h.key_press(Key::Enter);
        h.run_steps(2);
        assert!(h.state().0.is_open());
        assert!(h.state().2.is_empty());
    }

    #[test]
    fn close_button_dismisses_without_dispatch() {
        let mut h = harness("");
        h.run_steps(3);
        h.get_by_label(tr!("palette.close")).click();
        h.run_steps(2);
        assert!(!h.state().0.is_open());
        assert!(h.state().2.is_empty());
    }

    /// Selected and hovered rows tint only their background: the 3pt strip at a row's left
    /// edge matches the row's middle, so no accent is drawn.
    #[test]
    fn selected_and_hovered_rows_have_no_left_accent() {
        if !j_ui::test_support::gpu() {
            return;
        }
        for scale in [1.0, 1.5] {
            let mut h = sized_harness("mark", [1280.0, 720.0], scale);
            h.run_steps(4);
            let found = rows(&h);
            let hovered = found[1].1;
            h.hover_at(hovered.center());
            h.run_steps(2);
            let image = h.render().unwrap();
            for (_, rect) in &found[..2] {
                let pixel = |x: f32| {
                    let p = (egui::pos2(x, rect.top() + 4.0) * scale).round();
                    *image.get_pixel(p.x as u32, p.y as u32)
                };
                let middle = pixel(rect.left() + space::S8 + 1.0);
                for x in [rect.left() + 1.0, rect.left() + 2.0] {
                    assert_eq!(pixel(x), middle, "{scale}: {rect:?}");
                }
                assert_ne!(middle, pixel(rect.left() - 4.0), "row is tinted");
            }
        }
    }

    #[test]
    #[ignore = "renders review artifacts; run explicitly with JVID_PALETTE_ARTIFACT_DIR"]
    fn render_review_artifacts() {
        assert!(j_ui::test_support::gpu());
        let directory = std::path::PathBuf::from(
            std::env::var_os("JVID_PALETTE_ARTIFACT_DIR").expect("artifact directory"),
        );
        std::fs::create_dir_all(&directory).unwrap();
        for (size, scale, query, name) in [
            ([1280.0, 720.0], 1.0, "", "command-palette-1280.png"),
            ([1920.0, 1080.0], 1.5, "", "command-palette-1920@1.5.png"),
            (
                [640.0, 400.0],
                1.0,
                "export",
                "command-palette-disabled.png",
            ),
            ([320.0, 240.0], 1.0, "", "command-palette-narrow.png"),
            (
                [1280.0, 720.0],
                1.0,
                "NoSuchCommand",
                "command-palette-empty.png",
            ),
        ] {
            let mut h = sized_harness(query, size, scale);
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
                j_ui::i18n::set_language("en");
            }
        }
        let _restore = Restore;
        let mut h = harness("");
        h.run_steps(2);
        for language in ["es", "de", "fr", "ja", "en"] {
            j_ui::i18n::set_language(language);
            modal_fits_narrow_short_viewports();
            let name = CATALOG.get("action.export").unwrap();
            assert!(
                matches(name, &h.state().1)
                    .iter()
                    .any(|&i| ACTIONS[i].id == "export")
            );
            h.state_mut().0.query = "render".into();
            h.run();
            h.get_by_label_contains(tr!("export.nothing"));
            assert_eq!(
                Applicability::Disabled("export.nothing").reason(),
                CATALOG.get("export.nothing")
            );
        }
    }
}
