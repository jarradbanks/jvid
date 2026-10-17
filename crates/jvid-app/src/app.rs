//! The editor window: the app bar, a dock of empty panel shells, and the file actions.

use std::path::PathBuf;

use egui::{Align, Align2, Frame, Layout, Margin, Stroke, Ui, UiBuilder, ViewportCommand, vec2};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use jvid_core::history::History;
use jvid_core::model::Project;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use serde::{Deserialize, Serialize};
use suite_ui::panel::Panel;
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{Button, State};

use crate::project_file;

#[cfg(test)]
mod tests;

const DOCK_KEY: &str = "dock";

/// One dock panel. Persisted by name in the saved layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tab {
    Media,
    Preview,
    Inspector,
    Timeline,
}

pub fn run() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("jvid")
            .with_app_id("jvid")
            .with_icon(app_icon())
            .with_inner_size([1280.0, 720.0])
            .with_min_inner_size([960.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("jvid", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

pub struct App {
    history: History,
    path: Option<PathBuf>,
    dock: DockState<Tab>,
    /// The preview's views: one shows as a title, several as tabs.
    preview_views: &'static [&'static str],
    preview_active: usize,
    window_title: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        Self::restore(cc.storage)
    }

    /// A new project in the dock layout saved last time, or the default one.
    fn restore(storage: Option<&dyn eframe::Storage>) -> Self {
        let dock = storage
            .and_then(|s| eframe::get_value::<DockState<Tab>>(s, DOCK_KEY))
            // A layout missing or duplicating a panel can't be fixed from the UI.
            .filter(|d| {
                [Tab::Media, Tab::Preview, Tab::Inspector, Tab::Timeline]
                    .iter()
                    .all(|t| d.iter_all_tabs().filter(|(_, x)| *x == t).count() == 1)
            })
            .unwrap_or_else(default_dock);
        Self {
            history: History::new(Project::default()),
            path: None,
            dock: hide_tab_bars(dock),
            preview_views: &["Source", "Program"],
            preview_active: 1,
            window_title: String::new(),
        }
    }

    fn name(&self) -> String {
        self.path
            .as_deref()
            .and_then(|p| p.file_stem())
            .map_or("Untitled".into(), |s| s.to_string_lossy().into_owned())
    }

    fn app_bar(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        ui.painter().text(
            rect.left_center() + vec2(space::GUTTER + space::INSET, 0.0),
            Align2::LEFT_CENTER,
            "jvid",
            theme::title(),
            color::INK,
        );

        let name = self.name();
        let text_w = ui
            .painter()
            .layout_no_wrap(name.clone(), theme::body_strong(), color::INK)
            .size()
            .x;
        let w = (2.0 * space::S12 + text_w).ceil();
        let menu = egui::Rect::from_center_size(rect.center(), vec2(w, size::CONTROL));
        let r = ui
            .put(menu, Button::ghost(&name))
            .on_hover_text("New, open and save");
        let mut action = None;
        egui::Popup::menu(&r).show(|ui| {
            for (a, label) in [
                (FileAction::New, "New project"),
                (FileAction::Open, "Open…"),
                (FileAction::Save, "Save"),
                (FileAction::SaveAs, "Save as…"),
            ] {
                if ui.button(label).clicked() {
                    action = Some(a);
                }
            }
        });
        if self.history.is_dirty() {
            let dot = menu.right_center() + vec2(space::S4, 0.0);
            ui.painter()
                .text(dot, Align2::LEFT_CENTER, "•", theme::body(), color::INK_2);
        }
        if let Some(a) = action {
            self.file_action(a);
        }

        let right = rect.shrink2(vec2(space::GUTTER + space::S8, 0.0));
        let rtl = Layout::right_to_left(Align::Center);
        ui.scope_builder(UiBuilder::new().max_rect(right).layout(rtl), |ui| {
            ui.spacing_mut().item_spacing.x = space::S8;
            ui.add(Button::primary("Export").state(State::Disabled))
                .on_disabled_hover_text("Nothing to export yet");
            ui.spacing_mut().item_spacing.x = space::S4;
            let live = |on: bool| if on { State::Live } else { State::Disabled };
            let redo = Button::icon(icon::REDO, "Redo").state(live(self.history.can_redo()));
            if ui.add(redo).clicked() {
                self.history.redo();
            }
            let undo = Button::icon(icon::UNDO, "Undo").state(live(self.history.can_undo()));
            if ui.add(undo).clicked() {
                self.history.undo();
            }
        });
    }

    fn file_action(&mut self, action: FileAction) {
        match action {
            FileAction::New => {
                if self.confirm_discard() {
                    self.history = History::new(Project::default());
                    self.path = None;
                }
            }
            FileAction::Open => {
                if !self.confirm_discard() {
                    return;
                }
                let Some(path) = project_dialog().pick_file() else {
                    return;
                };
                match project_file::load(&path) {
                    Ok(project) => {
                        self.history = History::new(project);
                        self.path = Some(path);
                    }
                    Err(e) => error_dialog(&format!("Couldn't open {}: {e}", path.display())),
                }
            }
            FileAction::Save => {
                self.save(false);
            }
            FileAction::SaveAs => {
                self.save(true);
            }
        }
    }

    /// Saves to the current path, asking for one first if there is none or `as_new`.
    /// Returns false if cancelled or the save failed.
    fn save(&mut self, as_new: bool) -> bool {
        let path = match (&self.path, as_new) {
            (Some(p), false) => p.clone(),
            _ => {
                let Some(mut p) = project_dialog()
                    .set_file_name(format!("{}.jvid", self.name()))
                    .save_file()
                else {
                    return false;
                };
                if p.extension().is_none_or(|e| e != "jvid") {
                    p.as_mut_os_string().push(".jvid");
                }
                p
            }
        };
        match project_file::save(self.history.project(), &path) {
            Ok(()) => {
                self.history.mark_saved();
                self.path = Some(path);
                true
            }
            Err(e) => {
                error_dialog(&format!("Couldn't save {}: {e}", path.display()));
                false
            }
        }
    }

    /// Before replacing the project: true if it's clean, saved, or the user discards it.
    fn confirm_discard(&mut self) -> bool {
        if !self.history.is_dirty() {
            return true;
        }
        let answer = MessageDialog::new()
            .set_title("jvid")
            .set_description(format!("Save changes to {}?", self.name()))
            .set_buttons(MessageButtons::YesNoCancel)
            .show();
        match answer {
            MessageDialogResult::Yes => self.save(false),
            MessageDialogResult::No => true,
            _ => false,
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) && !self.confirm_discard() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }

        egui::Panel::top("app_bar")
            .exact_size(size::APP_BAR)
            .frame(Frame::NONE.fill(color::GROUND))
            .show_separator_line(false)
            .show(ui, |ui| self.app_bar(ui));

        let gutter = space::GUTTER as i8;
        let frame = Frame::NONE.fill(color::GROUND).inner_margin(Margin {
            left: gutter,
            right: gutter,
            top: 0,
            bottom: gutter,
        });
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let focused = self.dock.find_active_focused().map(|(_, t)| *t);
            let mut viewer = Viewer {
                preview_views: self.preview_views,
                preview_active: &mut self.preview_active,
                focused,
            };
            DockArea::new(&mut self.dock)
                .style(dock_style(ui.style()))
                .hidable_tab_bars(true)
                .draggable_tabs(false)
                .tab_context_menus(false)
                .show_close_buttons(false)
                .show_add_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_leaf_collapse_buttons(false)
                .show_inside(ui, &mut viewer);
        });

        let dirty = if self.history.is_dirty() { " •" } else { "" };
        let title = format!("{}{dirty} — jvid", self.name());
        if title != self.window_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, DOCK_KEY, &self.dock);
    }
}

#[derive(Clone, Copy)]
enum FileAction {
    New,
    Open,
    Save,
    SaveAs,
}

fn project_dialog() -> FileDialog {
    FileDialog::new().add_filter("jvid project", &["jvid"])
}

fn error_dialog(message: &str) {
    MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title("jvid")
        .set_description(message)
        .show();
}

/// Premiere's arrangement: media, preview and inspector over a full-width timeline.
pub fn default_dock() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Preview]);
    let tree = dock.main_surface_mut();
    let [top, _] = tree.split_below(NodeIndex::root(), 0.62, vec![Tab::Timeline]);
    let [preview, _] = tree.split_left(top, 0.26, vec![Tab::Media]);
    tree.split_right(preview, 0.69, vec![Tab::Inspector]);
    dock
}

/// Each panel draws its own suite-ui header, so egui_dock's tab bars stay hidden.
fn hide_tab_bars(mut dock: DockState<Tab>) -> DockState<Tab> {
    for (_, node) in dock.iter_all_nodes_mut() {
        if node.is_leaf() {
            node.set_tab_bar_hidden(true);
        }
    }
    dock
}

/// egui_dock draws only the 4px Ground gutters; the panels draw everything else.
fn dock_style(egui_style: &egui::Style) -> egui_dock::Style {
    let mut s = egui_dock::Style::from_egui(egui_style);
    s.dock_area_padding = None;
    s.main_surface_border_stroke = Stroke::NONE;
    s.separator.width = space::GUTTER;
    // Dragging a divider stops where the smaller side reaches the panel minimum.
    s.separator.extra = size::PANEL_MIN;
    // Transparent, not Ground: a separator snapped to pixels can overlap a panel's border.
    s.separator.color_idle = egui::Color32::TRANSPARENT;
    s.separator.color_hovered = egui::Color32::TRANSPARENT;
    s.separator.color_dragged = egui::Color32::TRANSPARENT;
    let body = &mut s.tab.tab_body;
    body.inner_margin = Margin::ZERO;
    body.stroke = Stroke::NONE;
    body.bg_fill = egui::Color32::TRANSPARENT;
    body.hidden_tab_bar_drag_height = Some(0.0);
    s.buttons.show_tab_bar_size = 0.0;
    s.buttons.show_tab_bar_hover_expand = 0.0;
    s
}

struct Viewer<'a> {
    preview_views: &'static [&'static str],
    preview_active: &'a mut usize,
    focused: Option<Tab>,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(*tab)
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        format!("{tab:?}").into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        let title = format!("{tab:?}");
        let panel = match (*tab, self.preview_views) {
            (Tab::Preview, [one]) => Panel::titled(one),
            (Tab::Preview, views) => Panel::tabbed(views, self.preview_active),
            _ => Panel::titled(&title),
        };
        // shortcut: panels are empty shells; their content lands in later issues.
        panel.focused(self.focused == Some(*tab)).show(ui, |_| {});
    }

    fn clear_background(&self, _tab: &Tab) -> bool {
        false
    }

    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        [false, false]
    }
}

/// shortcut: a placeholder drawn in code (play mark on Blue); replace with a real icon.
fn app_icon() -> egui::IconData {
    const N: u32 = 64;
    const R: f32 = 14.0;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let (cx, cy) = (fx.clamp(R, N as f32 - R), fy.clamp(R, N as f32 - R));
            let in_square = (fx - cx).hypot(fy - cy) <= R;
            let in_play = fx >= 24.0 && (fy - 32.0).abs() <= 12.0 * (44.0 - fx) / 20.0;
            let c = match (in_square, in_play) {
                (false, _) => egui::Color32::TRANSPARENT,
                (true, true) => color::WHITE,
                (true, false) => color::BLUE,
            };
            rgba.extend_from_slice(&c.to_array());
        }
    }
    egui::IconData {
        rgba,
        width: N,
        height: N,
    }
}
