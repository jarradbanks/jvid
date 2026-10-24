//! The app preferences dialog, under Edit > Preferences: a sidebar of sections beside the
//! chosen one's fields. General holds the UI language, the colour theme and Reduce Motion,
//! Audio the voiceover input device. Each applies at once and is saved to `settings.toml` as it changes.

use std::path::{Path, PathBuf};

use egui::{Align2, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, WidgetInfo, WidgetType};
use suite_ui::i18n::{self, LANGUAGES, SYSTEM};
use suite_ui::theme::{self, hairline, icon};
use suite_ui::themes::{self, Choice, Palette};
use suite_ui::tokens::{color, radius, size, space};
use suite_ui::widgets::{Button, Checkbox, Dropdown, fit_text};

use crate::i18n::tr;

/// The dialog's size when the window has room; it shrinks with the window and its body scrolls.
const WIDTH: f32 = 640.0;
const HEIGHT: f32 = 500.0;
const SIDEBAR: f32 = 160.0;
/// The footer: a 28px button with 12px above and 16px below.
const FOOTER: f32 = 56.0;
const CARD_COLUMNS: usize = 4;
/// How often the open dialog rereads the themes folder, in seconds.
const SCAN_SECS: f64 = 1.0;

#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub enum Section {
    #[default]
    General,
    Audio,
}

#[derive(Default)]
pub struct Preferences {
    open: bool,
    section: Section,
    /// The setting whose last change didn't save, and why, shown under it until the next one.
    error: Option<(&'static str, String)>,
    /// Input device names, listed when the dialog opened.
    inputs: Vec<String>,
    themes_dir: Option<PathBuf>,
    themes: Vec<Choice>,
    next_scan: f64,
}

impl Preferences {
    pub fn open(&mut self, inputs: Vec<String>, themes_dir: Option<PathBuf>) {
        self.open = true;
        self.error = None;
        self.inputs = inputs;
        self.themes_dir = themes_dir;
        self.next_scan = 0.0;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn section(&mut self, section: Section) {
        self.section = section;
    }

    /// Shows the dialog while open. `language` is the setting (`system` or a code), `theme`
    /// a built-in name or a theme file's path, `motion` the `reduce_motion` setting and `input`
    /// the input device name (empty for the system default). A pick applies at once and saves
    /// through `save(key, value)`. True when `theme` changed.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        language: &mut String,
        theme: &mut String,
        motion: &mut bool,
        input: &mut String,
        mut save: impl FnMut(&str, toml::Value) -> std::io::Result<()>,
    ) -> bool {
        if !self.open {
            return false;
        }
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        if now >= self.next_scan {
            self.themes = themes::choices(self.themes_dir.as_deref(), theme);
            self.next_scan = now + SCAN_SECS;
        }
        // A theme dropped into the folder shows up without moving the mouse.
        ctx.request_repaint_after_secs(SCAN_SECS as f32);
        let screen = ctx.content_rect();
        let size = egui::vec2(
            WIDTH.min(screen.width() - 2.0 * space::S16),
            HEIGHT.min(screen.height() - 2.0 * space::S16),
        );
        let mut save = |error: &mut Option<_>, key: &'static str, value: toml::Value| {
            *error = save(key, value).err().map(|e| (key, e.to_string()));
        };
        let mut theme_changed = false;
        let modal = egui::Modal::new(egui::Id::new("preferences"))
            .frame(
                egui::Frame::popup(ui.style())
                    .fill(color::panel())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(0),
            )
            .show(&ctx, |ui| {
                ui.set_min_size(size);
                ui.set_max_size(size);
                let (main, footer) = ui
                    .max_rect()
                    .split_top_bottom_at_y(ui.max_rect().bottom() - FOOTER);
                let (side, body) = main.split_left_right_at_x(main.left() + SIDEBAR);
                let line = hairline(ui.ctx(), color::border());
                ui.painter().vline(side.right(), side.y_range(), line);
                ui.painter().hline(footer.x_range(), footer.top(), line);

                ui.scope_builder(egui::UiBuilder::new().max_rect(side), |ui| {
                    self.sidebar(ui);
                });
                ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            egui::Frame::new().inner_margin(space::S16).show(ui, |ui| {
                                match self.section {
                                    Section::General => {
                                        if self.general(ui, language, theme, motion, &mut save) {
                                            theme_changed = true;
                                        }
                                    }
                                    Section::Audio => self.audio(ui, body, input, &mut save),
                                }
                            });
                        });
                });
                let done = footer.shrink2(egui::vec2(space::S16, 0.0));
                let done = Rect::from_min_max(
                    egui::pos2(done.left(), done.top() + space::S12),
                    egui::pos2(done.right(), done.bottom() - space::S16),
                );
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(done)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    |ui| {
                        if ui
                            .add(Button::primary(tr!("settings.done")).height(28.0))
                            .clicked()
                        {
                            ui.close();
                        }
                    },
                );
            });
        if modal.should_close() {
            self.open = false;
        }
        theme_changed
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(ui.max_rect().shrink2(egui::vec2(space::S8, space::S16))),
        );
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.horizontal(|ui| {
            ui.add_space(space::S8);
            ui.label(egui::RichText::new(tr!("settings.title")).font(theme::headline()));
        });
        ui.add_space(space::S12 - 2.0);
        let sections = [
            (Section::General, icon::SLIDERS, tr!("settings.general")),
            (Section::Audio, icon::MIC, tr!("settings.audio")),
        ];
        for (section, glyph, label) in sections {
            if nav_row(&mut ui, glyph, label, self.section == section).clicked() {
                self.section = section;
            }
        }
    }

    fn general(
        &mut self,
        ui: &mut Ui,
        language: &mut String,
        theme: &mut String,
        motion: &mut bool,
        save: &mut impl FnMut(&mut Option<(&'static str, String)>, &'static str, toml::Value),
    ) -> bool {
        section_head(ui, tr!("settings.general"));
        field_label(ui, tr!("settings.language"));
        let names = options();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut picked = choice(language);
        if ui
            .add(Dropdown::new(&mut picked, &names).width(240.0))
            .changed()
        {
            *language = setting(picked).into();
            i18n::set_language(i18n::resolve(language));
            save(&mut self.error, "language", language.as_str().into());
            // The built-in theme names are in the language they were listed in.
            self.next_scan = 0.0;
        }
        self.save_error(ui, "language");
        ui.add_space(20.0);

        field_label(ui, tr!("settings.theme"));
        let mut changed = false;
        let gap = space::S4;
        let w = (ui.available_width() - gap * (CARD_COLUMNS - 1) as f32) / CARD_COLUMNS as f32;
        egui::Grid::new("theme_cards")
            .spacing([gap, gap])
            .min_col_width(w)
            .max_col_width(w)
            .show(ui, |ui| {
                for (i, t) in self.themes.iter().enumerate() {
                    let on = t.setting == *theme;
                    if theme_card(ui, t, on, w).clicked() && !on {
                        theme.clone_from(&t.setting);
                        changed = true;
                    }
                    if (i + 1) % CARD_COLUMNS == 0 {
                        ui.end_row();
                    }
                }
            });
        if changed {
            save(&mut self.error, "theme", theme.as_str().into());
        }
        if let Some(Err(e)) = self
            .themes
            .iter()
            .find(|t| t.setting == *theme)
            .map(|t| &t.palette)
        {
            ui.add_space(space::S8);
            ui.label(egui::RichText::new(e).color(color::red()));
        }
        self.save_error(ui, "theme");
        ui.add_space(space::S8);
        let open = Button::secondary(tr!("settings.open_themes")).with_icon(icon::FOLDER);
        if ui.add(open).clicked()
            && let Some(dir) = &self.themes_dir
        {
            open_folder(dir);
        }
        ui.add_space(20.0);
        if ui
            .add(Checkbox::new(motion, tr!("settings.reduce_motion")))
            .changed()
        {
            reduce_motion(ui.ctx(), *motion);
            save(&mut self.error, "reduce_motion", (*motion).into());
        }
        self.save_error(ui, "reduce_motion");
        changed
    }

    fn audio(
        &mut self,
        ui: &mut Ui,
        body: Rect,
        input: &mut String,
        save: &mut impl FnMut(&mut Option<(&'static str, String)>, &'static str, toml::Value),
    ) {
        section_head(ui, tr!("settings.audio"));
        field_label(ui, tr!("settings.input_device"));
        let rows = input_rows(&self.inputs, input);
        let names: Vec<&str> = rows.iter().map(String::as_str).collect();
        let mut picked = rows[1..]
            .iter()
            .position(|r| r == input)
            .map_or(0, |i| i + 1);
        let dropdown = Dropdown::new(&mut picked, &names).width(280.0).bounds(body);
        if ui.add(dropdown).changed() {
            *input = if picked == 0 {
                String::new()
            } else {
                rows[picked].clone()
            };
            save(&mut self.error, "input_device", input.as_str().into());
        }
        self.save_error(ui, "input_device");
    }

    fn save_error(&self, ui: &mut Ui, key: &str) {
        if let Some((_, e)) = self.error.as_ref().filter(|(k, _)| *k == key) {
            ui.add_space(space::S4);
            let text = tr!("settings.save_failed", error = e);
            ui.label(egui::RichText::new(text).color(color::red()));
        }
    }
}

/// Turns egui's animations off, or back to their default length.
pub fn reduce_motion(ctx: &egui::Context, on: bool) {
    let time = if on {
        0.0
    } else {
        egui::Style::default().animation_time
    };
    ctx.global_style_mut(|s| s.animation_time = time);
}

fn section_head(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).font(theme::title()));
    ui.add_space(space::S12 - ui.spacing().item_spacing.y);
}

fn field_label(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(color::ink_2()));
    ui.add_space(6.0 - ui.spacing().item_spacing.y);
}

/// A 24px sidebar row: icon and Body Strong label, Raised behind the chosen one.
fn nav_row(ui: &mut Ui, glyph: &str, label: &str, on: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), size::CONTROL),
        Sense::click(),
    );
    response.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, on, label));
    if on {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::CONTROL), color::raised());
    }
    let (ink, glyph_ink) = match (on, response.hovered()) {
        (true, _) => (color::ink(), color::ink_2()),
        (false, true) => (color::ink(), color::ink_3()),
        (false, false) => (color::ink_2(), color::ink_3()),
    };
    let x = rect.left() + space::S8;
    ui.painter().text(
        egui::pos2(x + size::ICON / 2.0, rect.center().y),
        Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(),
        glyph_ink,
    );
    ui.painter().text(
        egui::pos2(x + size::ICON + space::S8, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::body_strong(),
        ink,
    );
    response
}

/// A theme's card: the editor drawn small in its colours, then its name. A file that doesn't
/// load shows a Red alert instead, with its error on hover.
fn theme_card(ui: &mut Ui, t: &Choice, on: bool, width: f32) -> egui::Response {
    let pad = space::S4;
    let mini_w = width - 2.0 * pad;
    let mini_h = (mini_w * 9.0 / 16.0).round();
    let height = pad + mini_h + 6.0 + 16.0 + pad;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), Sense::click());
    response.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, on, &t.name));
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::CONTROL), color::raised());
    }
    let mini = Rect::from_min_size(rect.min + egui::vec2(pad, pad), egui::vec2(mini_w, mini_h));
    match &t.palette {
        Ok(p) => paint_mini(ui, mini, p),
        Err(_) => {
            ui.painter().rect(
                mini,
                CornerRadius::same(radius::SMALL),
                color::field(),
                hairline(ui.ctx(), color::red()),
                StrokeKind::Inside,
            );
            ui.painter().text(
                mini.center(),
                Align2::CENTER_CENTER,
                icon::CIRCLE_ALERT,
                theme::icon_font(),
                color::red(),
            );
        }
    }
    if on {
        ui.painter().rect_stroke(
            mini.expand(1.0),
            CornerRadius::same(radius::SMALL + 1),
            Stroke::new(2.0, color::light_blue()),
            StrokeKind::Outside,
        );
    }
    let name = fit_text(
        ui,
        &t.name,
        theme::body_strong(),
        color::ink(),
        mini_w - 4.0,
    );
    let elided = name.elided;
    let at = egui::pos2(mini.left() + 2.0, mini.bottom() + 6.0);
    ui.painter().galley(at, name, color::ink());
    match &t.palette {
        Err(e) => response.on_hover_text(e),
        Ok(_) if elided => response.on_hover_text(&t.name),
        Ok(_) => response,
    }
}

/// The editor in miniature, in `p`'s colours: app bar, three panels, the timeline with two
/// clips and the playhead. Positions are fractions of the card, as in the mockup.
fn paint_mini(ui: &Ui, r: Rect, p: &Palette) {
    let painter = ui.painter();
    let at = |x: f32, y: f32, w: f32, h: f32| {
        Rect::from_min_size(
            egui::pos2(r.left() + x * r.width(), r.top() + y * r.height()),
            egui::vec2(w * r.width(), h * r.height()),
        )
    };
    let line = hairline(ui.ctx(), p.border);
    painter.rect(
        r,
        CornerRadius::same(radius::SMALL),
        p.ground,
        line,
        StrokeKind::Inside,
    );
    let fill = |rect: Rect, c| painter.rect_filled(rect, CornerRadius::same(1), c);
    let panel = |rect: Rect| {
        painter.rect(
            rect,
            CornerRadius::same(2),
            p.panel,
            line,
            StrokeKind::Inside,
        );
    };
    fill(at(0.02, 0.035, 0.08, 0.03), p.ink);
    fill(at(0.84, 0.025, 0.13, 0.055), p.blue);
    panel(at(0.02, 0.11, 0.23, 0.51));
    panel(at(0.265, 0.11, 0.47, 0.51));
    fill(at(0.29, 0.22, 0.42, 0.33), color::VIEWER);
    panel(at(0.75, 0.11, 0.23, 0.51));
    for (y, w) in [(0.28, 0.12), (0.36, 0.16), (0.44, 0.10)] {
        let row = at(0.78, y, w, 0.0);
        painter.hline(row.x_range(), row.top(), hairline(ui.ctx(), p.ink_3));
    }
    panel(at(0.02, 0.64, 0.96, 0.33));
    fill(at(0.16, 0.71, 0.30, 0.08), color::CLIP_VIOLET);
    fill(at(0.47, 0.71, 0.26, 0.08), color::CLIP_VIOLET);
    fill(at(0.22, 0.82, 0.36, 0.08), color::CLIP_GREEN);
    let x = r.left() + 0.44 * r.width();
    painter.vline(
        x,
        r.top() + 0.66 * r.height()..=r.top() + 0.96 * r.height(),
        Stroke::new(1.0, p.light_blue),
    );
}

/// Opens `dir` in the system file manager.
fn open_folder(dir: &Path) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(opener).arg(dir).spawn() {
        tracing::warn!("can't open {}: {e}", dir.display());
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

/// The input picker's rows: System Default, then each device. A saved device that isn't
/// plugged in stays listed, so opening the dialog doesn't lose it.
fn input_rows(devices: &[String], input: &str) -> Vec<String> {
    let mut rows = vec![tr!("settings.input_default").to_owned()];
    rows.extend(devices.iter().cloned());
    if !input.is_empty() && !devices.iter().any(|d| d == input) {
        rows.push(input.into());
    }
    rows
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
    fn reduce_motion_turns_animation_off_and_saves() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut fresh = true;
        let mut prefs = Preferences::default();
        prefs.open(Vec::new(), None);
        let saved = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = saved.clone();
        let mut h = egui_kittest::Harness::builder()
            .with_size([800.0, 600.0])
            .build_ui_state(
                move |ui, motion: &mut bool| {
                    if suite_ui::test_support::install_theme(ui, &mut fresh) {
                        return;
                    }
                    let (mut language, mut theme, mut input) =
                        ("system".into(), "dark".into(), String::new());
                    prefs.show(ui, &mut language, &mut theme, motion, &mut input, |k, v| {
                        log.borrow_mut().push((k.to_owned(), v));
                        Ok(())
                    });
                },
                false,
            );
        h.run();
        h.get_by_label("Reduce Motion").click();
        h.run();
        assert!(*h.state());
        assert_eq!(h.ctx.global_style().animation_time, 0.0);
        let want = ("reduce_motion".to_owned(), toml::Value::Boolean(true));
        assert_eq!(saved.borrow().last(), Some(&want));
        h.get_by_label("Reduce Motion").click();
        h.run();
        assert!(h.ctx.global_style().animation_time > 0.0);
    }

    #[test]
    fn picker_rows_map_to_settings_and_back() {
        assert_eq!(options()[1..3], ["English", "Español"]);
        assert_eq!(options().len(), 1 + LANGUAGES.len());
        for row in 0..options().len() {
            assert_eq!(choice(setting(row)), row);
        }
        // An older file with no setting, or one this build doesn't ship, still has a row.
        assert_eq!(choice(""), 0);
        assert_eq!(choice("es-MX"), 2);
        assert_eq!(choice("eo"), 1);
    }

    #[test]
    fn input_rows_keep_an_unplugged_choice() {
        let devices = ["Mic".to_owned(), "Line In".to_owned()];
        assert_eq!(
            input_rows(&devices, ""),
            ["System Default", "Mic", "Line In"]
        );
        assert_eq!(input_rows(&devices, "Mic").len(), 3);
        assert_eq!(input_rows(&devices, "USB")[3], "USB");
    }
}
