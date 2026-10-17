//! The editor window: the app bar, a dock of empty panel shells, and the file actions.

use std::collections::HashSet;
use std::path::PathBuf;

use egui::{Align, Align2, Frame, Layout, Margin, Stroke, Ui, UiBuilder, ViewportCommand, vec2};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use jvid_core::history::{Command, History};
use jvid_core::model::{AssetId, Project, ProjectSettings};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use serde::{Deserialize, Serialize};
use suite_ui::panel::Panel;
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{Button, State};

use crate::import::Importer;
use crate::keymap::{self, Keymap};
use crate::media_bin::MediaBin;
use crate::preview::{Preview, SOURCE_ACTIONS};
use crate::timeline::Timeline;
use crate::welcome::{self, Recent, Welcome};
use crate::{project_file, relink};

#[cfg(test)]
mod tests;

const DOCK_KEY: &str = "dock";
const RECENT_KEY: &str = "recent";

/// One dock panel. Persisted by name in the saved layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tab {
    Media,
    Preview,
    Inspector,
    Timeline,
}

pub fn run(settings: jvid_core::settings::Settings) -> eframe::Result {
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
    eframe::run_native(
        "jvid",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, &settings)))),
    )
}

pub struct App {
    history: History,
    path: Option<PathBuf>,
    dock: DockState<Tab>,
    /// The preview's views: one shows as a title, several as tabs.
    preview_views: &'static [&'static str],
    preview_active: usize,
    window_title: String,
    keymap: Keymap,
    importer: Importer,
    bin: MediaBin,
    preview: Preview,
    timeline: Timeline,
    /// Asset paths found missing when the project was opened; see [`relink::missing`].
    missing: HashSet<PathBuf>,
    /// Error toasts and the time each expires.
    toasts: Vec<(String, f64)>,
    welcome: Welcome,
    /// The welcome screen shows instead of the editor.
    on_welcome: bool,
    /// Match source: the next imported video sets the project's size and frame rate.
    match_source: bool,
    settings_dialog: crate::project_settings::SettingsDialog,
    theme: suite_ui::themes::LiveTheme,
    keymap_editor: crate::keymap_editor::KeymapEditor,
}

const TOAST_SECS: f64 = 6.0;

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, settings: &jvid_core::settings::Settings) -> Self {
        theme::install(&cc.egui_ctx);
        let mut app = Self::restore(cc.storage);
        let caches = crate::media_bin::Caches::new(settings);
        app.bin = MediaBin::new(caches.clone());
        app.timeline = Timeline::new(caches);
        app.preview = Preview::new(cc.wgpu_render_state.as_ref());
        app.theme = suite_ui::themes::LiveTheme::new(&settings.theme);
        app
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
            keymap: keymap::load(),
            importer: Importer::media(),
            bin: MediaBin::default(),
            preview: Preview::new(None),
            timeline: Timeline::default(),
            missing: HashSet::new(),
            toasts: Vec::new(),
            welcome: Welcome::new(
                storage
                    .and_then(|s| eframe::get_value(s, RECENT_KEY))
                    .unwrap_or_default(),
            ),
            on_welcome: true,
            match_source: false,
            settings_dialog: Default::default(),
            theme: suite_ui::themes::LiveTheme::new("dark"),
            keymap_editor: Default::default(),
        }
    }

    /// Replaces the project with an empty one; `None` settings is Match source.
    fn new_project(&mut self, settings: Option<ProjectSettings>) {
        self.history = History::new(Project::default());
        if let Some(s) = &settings {
            // Setting up isn't an edit: no undo step, nothing to save yet.
            let _ = self.history.apply(Command::SetSettings(s.clone()));
            self.history = History::new(self.history.project().clone());
        }
        self.match_source = settings.is_none();
        self.missing.clear();
        self.path = None;
        self.on_welcome = false;
    }

    fn open(&mut self, path: PathBuf) {
        match project_file::load(&path) {
            Ok(project) => {
                welcome::remember(&mut self.welcome.recent, Recent::of(&path, &project));
                self.missing = relink::missing(&project);
                self.history = History::new(project);
                self.path = Some(path);
                self.match_source = false;
                self.on_welcome = false;
            }
            Err(e) => error_dialog(&format!("Couldn't open {}: {e}", path.display())),
        }
    }

    fn welcome_action(&mut self, action: welcome::Action) {
        match action {
            welcome::Action::New(settings) => self.new_project(settings),
            welcome::Action::Open => self.file_action(FileAction::Open),
            welcome::Action::OpenRecent(path) => self.open(path),
            welcome::Action::Import => {
                if let Some(paths) = FileDialog::new().set_title("Import media").pick_files() {
                    self.new_project(None);
                    self.import(paths);
                }
            }
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
                (FileAction::Settings, "Project settings…"),
                (FileAction::Shortcuts, "Keyboard shortcuts…"),
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
                    self.new_project(Some(ProjectSettings::default()));
                    self.on_welcome = true;
                }
            }
            FileAction::Open => {
                if !self.confirm_discard() {
                    return;
                }
                let Some(path) = project_dialog().pick_file() else {
                    return;
                };
                self.open(path);
            }
            FileAction::Save => {
                self.save(false);
            }
            FileAction::SaveAs => {
                self.save(true);
            }
            FileAction::Settings => self.settings_dialog.open(self.history.project()),
            FileAction::Shortcuts => self.keymap_editor.open(),
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
                welcome::remember(
                    &mut self.welcome.recent,
                    Recent::of(&path, self.history.project()),
                );
                self.path = Some(path);
                true
            }
            Err(e) => {
                error_dialog(&format!("Couldn't save {}: {e}", path.display()));
                false
            }
        }
    }

    /// Saves the shown frame as a PNG at full project resolution.
    fn export_frame(&self) {
        let Some(path) = FileDialog::new()
            .set_title("Export frame")
            .add_filter("PNG image", &["png"])
            .set_file_name("frame.png")
            .save_file()
        else {
            return;
        };
        if let Err(e) = self
            .preview
            .export_frame(self.history.project().settings(), &path)
        {
            error_dialog(&format!("Couldn't export {}: {e}", path.display()));
        }
    }

    /// `,` / `.`: the Source viewer's in/out at the playhead on the targeted tracks, then the
    /// playhead moves to the end of the new clip, as in Premiere.
    fn place_from_source(&mut self, insert: bool) {
        let Some(asset) = self.preview.source_asset() else {
            return;
        };
        let at = self.timeline.playhead();
        let Some(p) = self.history.project().placement(asset, at, insert, None) else {
            return;
        };
        let end = p.start + (p.source_out - p.source_in);
        match self.history.apply(Command::Place(p)) {
            Ok(_) => self.timeline.seek(end),
            Err(e) => tracing::warn!("place failed: {e}"),
        }
    }

    /// Runs a keymap action. shortcut: only panel focus is real; the rest are stubs until
    /// their features land.
    fn run_action(&mut self, id: &str) {
        let preview_focused = self
            .dock
            .find_active_focused()
            .is_some_and(|(_, t)| *t == Tab::Preview);
        if preview_focused
            && self.preview_active == 0
            && self.preview.source_asset().is_some()
            && SOURCE_ACTIONS.contains(&id)
        {
            let project = self.history.project();
            if let Some(c) = self.preview.source_action(id, project)
                && let Err(e) = self.history.apply(c)
            {
                tracing::warn!("source mark failed: {e}");
            }
            return;
        }
        let timeline_focused = self
            .dock
            .find_active_focused()
            .is_some_and(|(_, t)| *t == Tab::Timeline);
        if timeline_focused && self.timeline.action(id, self.history.project()) {
            return;
        }
        let tab = match id {
            "focus_media" => Tab::Media,
            "focus_timeline" => Tab::Timeline,
            "focus_inspector" => Tab::Inspector,
            "focus_source" => {
                self.preview_active = 0;
                Tab::Preview
            }
            "focus_program" => {
                self.preview_active = 1;
                Tab::Preview
            }
            "export_frame" => return self.export_frame(),
            "insert" | "overwrite" => return self.place_from_source(id == "insert"),
            _ => {
                tracing::debug!("action {id} has no implementation yet");
                return;
            }
        };
        if let Some(path) = self.dock.find_tab(&tab) {
            self.dock.set_focused_node_and_surface(path.node_path());
        }
    }

    /// Asks for media files and queues them for import.
    fn import_dialog(&mut self) {
        if let Some(paths) = FileDialog::new().set_title("Import media").pick_files() {
            self.import(paths);
        }
    }

    fn import(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        let existing: Vec<_> = self
            .history
            .project()
            .assets()
            .iter()
            .map(|a| a.path.clone())
            .collect();
        self.importer.submit(paths, &existing);
    }

    /// Applies a finished batch and queues toasts for files that failed.
    fn poll_import(&mut self, now: f64) {
        let polled = self.importer.poll();
        if let Some(batch) = polled.batch {
            // Import never fails; duplicates are skipped inside the command.
            let fit = self
                .match_source
                .then(|| welcome::match_source(&batch))
                .flatten();
            let _ = self.history.apply(Command::Import(batch));
            if let Some((width, height, frame_rate)) = fit {
                let settings = ProjectSettings {
                    width,
                    height,
                    frame_rate,
                    ..self.history.project().settings().clone()
                };
                let _ = self.history.apply(Command::SetSettings(settings));
                self.match_source = false;
            }
        }
        self.toasts
            .extend(polled.errors.into_iter().map(|e| (e, now + TOAST_SECS)));
        self.toasts.retain(|(_, until)| *until > now);
    }

    /// Asks where an offline asset went, then relinks it and the others found beside it.
    fn relink_dialog(&mut self, id: AssetId) {
        let Some(asset) = self.history.project().asset(id) else {
            return;
        };
        let dialog = FileDialog::new().set_title(format!("Relink {}", asset.display_name()));
        let dialog = match asset.path.parent().filter(|d| d.is_dir()) {
            Some(d) => dialog.set_directory(d),
            None => dialog,
        };
        if let Some(chosen) = dialog.pick_file() {
            let cmd = relink::plan(self.history.project(), &self.missing, id, chosen);
            if let Err(e) = self.history.apply(cmd) {
                tracing::warn!("relink failed: {e}");
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
        if let Some(e) = self.theme.poll(&ctx) {
            let until = ctx.input(|i| i.time) + TOAST_SECS;
            self.toasts.push((e, until));
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.confirm_discard() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }

        let timeline = self
            .dock
            .find_active_focused()
            .is_some_and(|(_, t)| *t == Tab::Timeline);
        let pressed = if self.keymap_editor.is_open() {
            Vec::new()
        } else {
            self.keymap.pressed(&ctx, timeline)
        };
        for id in pressed {
            self.run_action(id);
        }

        let dropped: Vec<_> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() {
            if self.on_welcome {
                self.new_project(None);
            }
            self.import(dropped);
        }
        let now = ctx.input(|i| i.time);
        self.poll_import(now);
        if self.importer.is_busy() || !self.toasts.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        if self.on_welcome {
            if let Some(a) = self.welcome.show(ui) {
                self.welcome_action(a);
            }
            self.show_toasts(&ctx);
            return;
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
                project: self.history.project(),
                importing: self.importer.is_busy(),
                bin: &mut self.bin,
                missing: &self.missing,
                relink: None,
                import_clicked: false,
                command: None,
                gesture: None,
                open: None,
                preview: &mut self.preview,
                timeline: &mut self.timeline,
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
            let (import, command, gesture, relink, open) = (
                viewer.import_clicked,
                viewer.command,
                viewer.gesture,
                viewer.relink,
                viewer.open,
            );
            if let Some(id) = open {
                self.preview.open(self.history.project(), id);
                self.preview_active = 0;
            }
            let applied = match (command, gesture) {
                (Some(c), Some(k)) => Some(self.history.apply_coalesced(k, c)),
                (Some(c), None) => Some(self.history.apply(c)),
                (None, _) => None,
            };
            if let Some(Err(e)) = applied {
                tracing::warn!("edit failed: {e}");
            }
            if import {
                self.import_dialog();
            }
            if let Some(id) = relink {
                self.relink_dialog(id);
            }
            if self.keymap_editor.show(ui, &mut self.keymap)
                && let Err(e) = keymap::save(&self.keymap)
            {
                tracing::warn!("keymap: can't save: {e}");
            }
            if let Some(c) = self.settings_dialog.show(ui, self.history.project())
                && let Err(e) = self.history.apply(c)
            {
                tracing::warn!("edit failed: {e}");
            }
        });

        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            drop_overlay(&ctx);
        }
        self.show_toasts(&ctx);

        let dirty = if self.history.is_dirty() { " •" } else { "" };
        let title = format!("{}{dirty} — jvid", self.name());
        if title != self.window_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, DOCK_KEY, &self.dock);
        eframe::set_value(storage, RECENT_KEY, &self.welcome.recent);
    }
}

#[derive(Clone, Copy)]
enum FileAction {
    New,
    Open,
    Save,
    SaveAs,
    Settings,
    Shortcuts,
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
    project: &'a Project,
    importing: bool,
    bin: &'a mut MediaBin,
    missing: &'a HashSet<PathBuf>,
    relink: Option<AssetId>,
    import_clicked: bool,
    command: Option<Command>,
    /// Set when `command` continues a drag, so the drag is one undo step.
    gesture: Option<jvid_core::history::GestureKey>,
    open: Option<jvid_core::model::AssetId>,
    preview: &'a mut Preview,
    timeline: &'a mut Timeline,
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
        let focused = self.focused == Some(*tab);
        if *tab == Tab::Media {
            let r = self
                .bin
                .show(ui, self.project, focused, self.importing, self.missing);
            self.import_clicked |= r.import;
            // The Source tab may have set one this frame too.
            self.command = r.command.or(self.command.take());
            self.relink = r.relink;
            self.open = r.open;
            return;
        }
        let source_tab = self.preview_views.get(*self.preview_active) == Some(&"Source");
        let title = format!("{tab:?}");
        let panel = match (*tab, self.preview_views) {
            (Tab::Preview, [one]) => Panel::titled(one),
            (Tab::Preview, views) => Panel::tabbed(views, self.preview_active),
            _ => Panel::titled(&title),
        };
        let panel = panel.focused(focused);
        if *tab == Tab::Preview && source_tab {
            panel.show(ui, |ui| {
                if let Some(c) = self.preview.show_source(ui, self.project, self.missing) {
                    self.command = Some(c);
                }
            });
            return;
        }
        if *tab == Tab::Preview {
            // The newest video (or still) asset, until the timeline and source viewer exist.
            let source = self.project.assets().iter().rev().find_map(|a| {
                a.probe.as_ref()?.width?;
                Some(a.path.as_path())
            });
            let offline = source.is_some_and(|p| self.missing.contains(p));
            panel.show(ui, |ui| self.preview.show_frame(ui, source, offline));
            return;
        }
        if *tab == Tab::Timeline {
            panel.show(ui, |ui| {
                if let Some((c, g)) = self.timeline.show(ui, self.project, self.missing) {
                    (self.command, self.gesture) = (Some(c), g);
                }
            });
            return;
        }
        if *tab == Tab::Inspector {
            let solid = self.bin.selected.and_then(|id| self.project.asset(id));
            panel.show(ui, |ui| {
                let (selected, t) = (self.timeline.selected(), self.timeline.playhead());
                let (edit, seek) = crate::inspector::show(ui, self.project, selected, solid, t);
                if let Some((c, g)) = edit {
                    (self.command, self.gesture) = (Some(c), g);
                }
                if let Some(t) = seek {
                    self.timeline.seek(t);
                }
            });
            return;
        }
        // shortcut: panels are empty shells; their content lands in later issues.
        panel.show(ui, |_| {});
    }

    fn clear_background(&self, _tab: &Tab) -> bool {
        false
    }

    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        [false, false]
    }
}

/// A Light Blue outline over the window while files are dragged over it.
fn drop_overlay(ctx: &egui::Context) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("drop"),
    ));
    let rect = ctx.content_rect().shrink(space::GUTTER);
    let corner = egui::CornerRadius::same(suite_ui::tokens::radius::PANEL);
    painter.rect_filled(rect, corner, color::BLUE.gamma_multiply(0.08));
    painter.rect_stroke(
        rect,
        corner,
        Stroke::new(2.0, color::LIGHT_BLUE),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        "Drop to import",
        theme::title(),
        color::INK,
    );
}

impl App {
    /// Error toasts stacked bottom-right, newest at the bottom.
    fn show_toasts(&mut self, ctx: &egui::Context) {
        let mut dismiss = None;
        let mut y = -space::S16;
        for (i, (msg, _)) in self.toasts.iter().enumerate().rev() {
            let r = egui::Area::new(egui::Id::new(("toast", i)))
                .anchor(Align2::RIGHT_BOTTOM, vec2(-space::S16, y))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .fill(color::RAISED)
                        .inner_margin(Margin::same(space::S12 as i8))
                        .show(ui, |ui| {
                            ui.set_max_width(360.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(icon::CIRCLE_ALERT)
                                        .font(theme::icon_font())
                                        .color(color::RED),
                                );
                                ui.add(egui::Label::new(msg.as_str()).wrap());
                                if ui.add(Button::icon(icon::X, "Dismiss")).clicked() {
                                    dismiss = Some(i);
                                }
                            });
                        });
                });
            y -= r.response.rect.height() + space::S8;
        }
        if let Some(i) = dismiss {
            self.toasts.remove(i);
        }
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
