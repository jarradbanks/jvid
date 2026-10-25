//! The editor window: the app bar, a dock of empty panel shells, and the file actions.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use egui::{Align, Align2, Frame, Layout, Margin, Stroke, Ui, UiBuilder, ViewportCommand, vec2};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use jvid_core::history::{Command, History};
use jvid_core::model::{AssetId, MAX_NEST_DEPTH, ModelError, Project, ProjectSettings, TrackKind};
use jvid_core::time::Time;
use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use suite_ui::dialog::Dialog;
use suite_ui::panel::Panel;
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{Button, ButtonKind, State};

use crate::export::queue::Action as QueueAction;
use crate::export::{Error as ExportError, Request};
use crate::i18n::{tr, trn, verbatim};
use crate::import::Importer;
use crate::keymap::{self, Keymap};
use crate::media_bin::MediaBin;
use crate::preview::{PROGRAM_ACTIONS, Preview, SOURCE_ACTIONS};
use crate::subtitles::Done;
use crate::timeline::{MARK_ACTIONS, Timeline, menus};
use crate::welcome::{self, Recent, Welcome};
use crate::{autosave, project_file, relink};
use eframe::egui_wgpu::wgpu;

#[cfg(test)]
mod reel;
#[cfg(test)]
mod tests;
pub(crate) mod tutorial;

const DOCK_KEY: &str = "dock";
const RECENT_KEY: &str = "recent";

/// One dock panel. Persisted by name in the saved layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tab {
    Media,
    Preview,
    Inspector,
    Timeline,
    /// Opened from the Export menu's Show Render Queue; closable, unlike the others.
    RenderQueue,
    /// Opened from Timeline > Transcript; closable.
    Transcript,
}

impl Tab {
    /// The panel's title in the UI language.
    fn title(self) -> &'static str {
        match self {
            Tab::Media => tr!("panel.media"),
            Tab::Preview => tr!("panel.preview"),
            Tab::Inspector => tr!("panel.inspector"),
            Tab::Timeline => tr!("panel.timeline"),
            Tab::RenderQueue => tr!("export.queue"),
            Tab::Transcript => tr!("panel.transcript"),
        }
    }

    /// The panel's [`tutorial::anchor`].
    fn anchor(self) -> &'static str {
        match self {
            Tab::Media => "media",
            Tab::Preview => "preview",
            Tab::Inspector => "inspector",
            Tab::Timeline => "timeline",
            Tab::RenderQueue => "render_queue",
            Tab::Transcript => "transcript",
        }
    }
}

/// A preview view's title, by id: `source` or `program`.
fn view_title(id: &str) -> &'static str {
    match id {
        "source" => tr!("panel.source"),
        _ => tr!("panel.program"),
    }
}

pub fn run(settings: jvid_core::settings::Settings) -> eframe::Result {
    // Here rather than in App::new, so tests and embedders pick their own language.
    suite_ui::i18n::set_language(suite_ui::i18n::resolve(&settings.language));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(verbatim("jvid"))
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
    preferences: crate::preferences::Preferences,
    /// The `language` setting: `system` or a language code.
    language: String,
    /// The `input_device` setting: a device name, empty for the system default.
    input_device: String,
    /// The `reduce_motion` setting.
    reduce_motion: bool,
    /// The voiceover being recorded.
    take: Option<crate::record::Take>,
    /// The microphone backend, opened on the first take; tests swap in a fake.
    input: Option<Box<dyn jvid_audio::InputBackend>>,
    /// The `theme` setting: a built-in name or a theme file's path.
    theme_setting: String,
    theme: suite_ui::themes::LiveTheme,
    keymap_editor: crate::keymap_editor::KeymapEditor,
    exports: jvid_jobs::Pool,
    export_dialog: crate::export::dialog::ExportDialog,
    autosave: crate::autosave::Autosave,
    queue: crate::export::queue::Queue,
    subtitles: crate::subtitles::Subtitles,
    transcript: crate::transcript::TranscriptPanel,
    /// Dialogs waiting to be answered, the first one showing.
    prompts: Vec<Prompt>,
    /// The user chose to quit; the next close request goes through.
    closing: bool,
    /// The lesson running, in its own sandbox project.
    lesson: Option<tutorial::Run>,
    /// The `tutorials_done` setting: ids of the lessons finished.
    tutorials_done: Vec<String>,
    /// Where `settings.toml` is saved; `None` saves nothing.
    settings_path: Option<PathBuf>,
    /// Where the untitled project autosaves; see [`autosave::untitled`].
    untitled: Option<PathBuf>,
}

/// A question or error shown in an in-app [`Dialog`].
#[derive(Debug, PartialEq)]
enum Prompt {
    /// Unsaved changes: Save, Don't Save or Cancel, then `Then`.
    Discard(Then),
    /// Restore the autosave `auto`, newer than `path` (`None`: the untitled project).
    Restore {
        path: Option<PathBuf>,
        auto: PathBuf,
    },
    Error(String),
    /// Help > About: the version and the sample media credits.
    About,
    /// About > Third-Party Notices.
    Notices,
}

/// What runs once unsaved changes are saved or discarded.
#[derive(Debug, PartialEq)]
enum Then {
    New,
    Open,
    Recent(PathBuf),
    Close,
}

const TOAST_SECS: f64 = 6.0;

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, settings: &jvid_core::settings::Settings) -> Self {
        theme::install(&cc.egui_ctx);
        jvid_media::hwdec::set_enabled(settings.hardware_decode);
        let mut app = Self::restore(cc.storage);
        let caches = crate::media_bin::Caches::new(settings);
        app.bin = MediaBin::new(caches.clone());
        app.preview = Preview::new(cc.wgpu_render_state.as_ref());
        app.preview.set_proxies(caches.proxies.clone());
        app.transcript.cache = crate::transcript::Cache::new(caches.transcripts.clone());
        app.timeline = Timeline::new(caches);
        app.theme = suite_ui::themes::LiveTheme::new(&settings.theme);
        app.theme_setting = settings.theme.clone();
        app.language = settings.language.clone();
        app.input_device = settings.input_device.clone();
        app.reduce_motion = settings.reduce_motion;
        app.tutorials_done = settings.tutorials_done.clone();
        app.welcome.tour = !settings.tour_offered;
        // Tests never write the real settings file; those that save set their own.
        app.settings_path = crate::settings_file::path().filter(|_| !cfg!(test));
        // Nor the real untitled autosave: one there opens every harness on a Restore prompt.
        app.untitled = autosave::untitled().filter(|_| !cfg!(test));
        crate::preferences::reduce_motion(&cc.egui_ctx, settings.reduce_motion);
        if let Some(auto) = app.untitled.as_deref().and_then(autosave::newer) {
            app.prompts.push(Prompt::Restore { path: None, auto });
        }
        app
    }

    /// A new project in the dock layout saved last time, or the default one.
    fn restore(storage: Option<&dyn eframe::Storage>) -> Self {
        let dock = storage
            .and_then(|s| eframe::get_value::<DockState<Tab>>(s, DOCK_KEY))
            // A layout missing or duplicating a panel can't be fixed from the UI.
            .filter(|d| {
                let count = |t: Tab| d.iter_all_tabs().filter(|(_, x)| **x == t).count();
                [Tab::Media, Tab::Preview, Tab::Inspector, Tab::Timeline]
                    .into_iter()
                    .all(|t| count(t) == 1)
                    && count(Tab::RenderQueue) <= 1
                    && count(Tab::Transcript) <= 1
            })
            .unwrap_or_else(default_dock);
        let exports = jvid_jobs::Pool::new(1);
        // Titles share the UI's CJK faces, so the binary links one copy of them.
        jvid_render::text::set_faces(suite_ui::theme::CJK_FACES.map(|(_, b)| b).to_vec());
        // The system font scan can take seconds; done here, the first text frame doesn't stall.
        exports.submit("fonts", jvid_jobs::Priority::Preview, |_| {
            jvid_render::text::warm()
        });
        Self {
            history: History::new(Project::default()),
            path: None,
            dock: {
                let mut dock = dock;
                hide_tab_bars(&mut dock);
                dock
            },
            preview_views: &["source", "program"],
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
            preferences: Default::default(),
            language: jvid_core::settings::Settings::default().language,
            input_device: String::new(),
            reduce_motion: false,
            take: None,
            input: None,
            theme_setting: "dark".into(),
            theme: suite_ui::themes::LiveTheme::new("dark"),
            keymap_editor: Default::default(),
            exports,
            export_dialog: Default::default(),
            autosave: Default::default(),
            queue: Default::default(),
            subtitles: Default::default(),
            transcript: Default::default(),
            prompts: Vec::new(),
            closing: false,
            lesson: None,
            tutorials_done: Vec::new(),
            settings_path: None,
            untitled: None,
        }
    }

    /// Replaces the project with an empty one; `None` settings is Match source.
    fn new_project(&mut self, settings: Option<ProjectSettings>) {
        self.leave_project();
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
                self.leave_project();
                self.history = History::new(project);
                if let Some(auto) = autosave::newer(&path) {
                    let path = Some(path.clone());
                    self.prompts.push(Prompt::Restore { path, auto });
                }
                self.path = Some(path);
                self.match_source = false;
                self.on_welcome = false;
            }
            Err(e) => self.error(tr!("app.open_failed", path = path.display(), error = e)),
        }
    }

    fn welcome_action(&mut self, action: welcome::Action) {
        match action {
            welcome::Action::New(settings) => self.new_project(settings),
            welcome::Action::Open => self.file_action(FileAction::Open),
            welcome::Action::OpenRecent(path) => self.open(path),
            welcome::Action::Import => {
                if let Some(paths) = FileDialog::new()
                    .set_title(tr!("app.import_title"))
                    .pick_files()
                {
                    self.new_project(None);
                    self.import(paths);
                }
            }
            welcome::Action::Tour => self.start_lesson(&tutorial::LESSONS[0]),
            welcome::Action::DeclineTour => self.tour_offered(),
        }
    }

    fn name(&self) -> String {
        self.path
            .as_deref()
            .and_then(|p| p.file_stem())
            .map_or(tr!("app.untitled").into(), |s| {
                s.to_string_lossy().into_owned()
            })
    }

    fn app_bar(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let logo = ui.painter().text(
            rect.left_center() + vec2(space::GUTTER + space::INSET, 0.0),
            Align2::LEFT_CENTER,
            verbatim("jvid"),
            theme::title(),
            color::ink(),
        );
        let file = self.file_menu(ui, logo.right() + space::S12);
        let edit = self.edit_menu(ui, file + space::S4);
        let timeline = self.timeline_menu(ui, edit + space::S4);
        self.help_menu(ui, timeline + space::S4);

        let name = self.name();
        let text_w = ui
            .painter()
            .layout_no_wrap(name.clone(), theme::body_strong(), color::ink())
            .size()
            .x;
        let w = (2.0 * space::S12 + text_w).ceil();
        let menu = egui::Rect::from_center_size(rect.center(), vec2(w, size::CONTROL));
        let r = ui
            .put(menu, Button::ghost(&name))
            .on_hover_text(tr!("menu.project_tooltip"));
        let mut action = None;
        egui::Popup::menu(&r).show(|ui| {
            for (a, label) in [
                (FileAction::New, tr!("menu.new_project")),
                (FileAction::Open, tr!("menu.open")),
                (FileAction::Save, tr!("menu.save")),
                (FileAction::SaveAs, tr!("menu.save_as")),
                (FileAction::Settings, tr!("menu.project_settings")),
                (FileAction::Shortcuts, tr!("menu.shortcuts")),
            ] {
                if ui.button(label).clicked() {
                    action = Some(a);
                }
            }
        });
        if self.history.is_dirty() {
            let dot = menu.right_center() + vec2(space::S4, 0.0);
            ui.painter()
                .text(dot, Align2::LEFT_CENTER, "•", theme::body(), color::ink_2());
        }
        if let Some(a) = action {
            self.file_action(a);
        }

        let right = rect.shrink2(vec2(space::GUTTER + space::S8, 0.0));
        let rtl = Layout::right_to_left(Align::Center);
        ui.scope_builder(UiBuilder::new().max_rect(right).layout(rtl), |ui| {
            ui.spacing_mut().item_spacing.x = space::S8;
            self.export_button(ui);
            ui.spacing_mut().item_spacing.x = space::S4;
            let live = |on: bool| if on { State::Live } else { State::Disabled };
            let redo =
                Button::icon(icon::REDO, tr!("action.redo")).state(live(self.history.can_redo()));
            if ui.add(redo).clicked() {
                self.run_action("redo", true);
            }
            let undo =
                Button::icon(icon::UNDO, tr!("action.undo")).state(live(self.history.can_undo()));
            if ui.add(undo).clicked() {
                self.run_action("undo", true);
            }
        });
    }

    /// The File menu, its left edge at `left`: each row runs its keymap action.
    /// Returns its right edge.
    fn file_menu(&mut self, ui: &mut Ui, left: f32) -> f32 {
        let button = menu_button(ui, tr!("menu.file"), left);
        let mut picked = None;
        let mut recent = None;
        let mut reframe = None;
        egui::Popup::menu(&button).show(|ui| {
            for &id in menus::FILE {
                match id {
                    "" => {
                        ui.separator();
                    }
                    "recent" => {
                        ui.add_enabled_ui(!self.welcome.recent.is_empty(), |ui| {
                            ui.menu_button(tr!("menu.open_recent"), |ui| {
                                for r in &self.welcome.recent {
                                    let name = r.path.file_stem().unwrap_or_default();
                                    let b = ui.button(name.to_string_lossy());
                                    if b.on_hover_text(r.path.display().to_string()).clicked() {
                                        recent = Some(r.path.clone());
                                    }
                                }
                            });
                        });
                    }
                    "reframe" => {
                        ui.menu_button(tr!("menu.reframe"), |ui| {
                            for (label, aspect) in REFRAME {
                                if ui.button(label).clicked() {
                                    reframe = Some(aspect);
                                }
                            }
                        });
                    }
                    _ => {
                        let can = id != "export" || self.can_export();
                        if menus::item(ui, &self.keymap, id, None, can) {
                            picked = Some(id);
                        }
                    }
                }
            }
        });
        if let Some(path) = recent {
            self.discard_then(Then::Recent(path));
        }
        if let Some(aspect) = reframe {
            self.reframe(aspect);
        }
        match picked {
            Some("quit") => ui.ctx().send_viewport_cmd(ViewportCommand::Close),
            Some(id) => self.run_action(id, true),
            None => {}
        }
        button.rect.right()
    }

    /// The Edit menu, its left edge at `left`: each row runs its keymap action, then Preferences.
    /// Returns its right edge.
    fn edit_menu(&mut self, ui: &mut Ui, left: f32) -> f32 {
        let button = menu_button(ui, tr!("menu.edit"), left);
        let mut picked = None;
        egui::Popup::menu(&button).show(|ui| {
            for &id in menus::EDIT {
                if id.is_empty() {
                    ui.separator();
                    continue;
                }
                let can = match id {
                    "undo" => self.history.can_undo(),
                    "redo" => self.history.can_redo(),
                    _ => self.timeline.can(id, self.history.project().sequence()),
                };
                if menus::item(ui, &self.keymap, id, None, can) {
                    picked = Some(id);
                }
            }
            ui.separator();
            if ui.button(tr!("menu.preferences")).clicked() {
                use jvid_audio::InputBackend;
                self.preferences.open(
                    jvid_audio::CpalInput::default().devices(),
                    crate::settings_file::themes_dir(),
                );
            }
        });
        if let Some(id) = picked {
            self.run_action(id, true);
        }
        button.rect.right()
    }

    /// The Timeline menu, its left edge at `left`. Returns its right edge.
    fn timeline_menu(&mut self, ui: &mut Ui, left: f32) -> f32 {
        let button = menu_button(ui, tr!("menu.timeline"), left);
        let (mut picked, mut transcript) = (false, false);
        egui::Popup::menu(&button).show(|ui| {
            let can = self.can_generate_subtitles();
            picked = menus::item(ui, &self.keymap, "generate_subtitles", None, can);
            transcript = ui.button(tr!("menu.transcript")).clicked();
        });
        if picked {
            self.run_action("generate_subtitles", true);
        }
        if transcript {
            self.show_panel(Tab::Transcript);
        }
        button.rect.right()
    }

    fn can_generate_subtitles(&self) -> bool {
        let selected = self.timeline.selected();
        crate::subtitles::Subtitles::can_open(self.history.project(), selected)
    }

    /// The Help menu, its left edge at `left`.
    fn help_menu(&mut self, ui: &mut Ui, left: f32) {
        let button = menu_button(ui, tr!("menu.help"), left);
        egui::Popup::menu(&button).show(|ui| {
            self.tutorials_menu(ui);
            if ui.button(tr!("menu.about")).clicked() {
                self.prompts.push(Prompt::About);
            }
        });
    }

    fn file_action(&mut self, action: FileAction) {
        match action {
            FileAction::New => self.discard_then(Then::New),
            // In a lesson the dialog comes first, so cancelling it leaves the lesson running.
            FileAction::Open if self.lesson.is_some() => {
                if let Some(path) = project_dialog().pick_file() {
                    self.discard_then(Then::Recent(path));
                }
            }
            FileAction::Open => self.discard_then(Then::Open),
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
        self.save_to(path)
    }

    /// Saves a copy reframed to `aspect` (width:height) under a new name and opens it, leaving
    /// this project as it is.
    fn reframe(&mut self, aspect: [u32; 2]) {
        let [aw, ah] = aspect;
        let Some(mut path) = project_dialog()
            .set_file_name(format!("{}-{aw}x{ah}.jvid", self.name()))
            .save_file()
        else {
            return;
        };
        if path.extension().is_none_or(|e| e != "jvid") {
            path.as_mut_os_string().push(".jvid");
        }
        let project = self.history.project();
        let st = project.settings();
        let (w, h) = jvid_core::reframe::frame_size(st.width, st.height, aspect);
        let copy = project.reframed(w, h, jvid_render::text::drawn_bounds);
        match project_file::save(&copy, &path) {
            Ok(()) => self.discard_then(Then::Recent(path)),
            Err(e) => self.error(tr!("app.save_failed", path = path.display(), error = e)),
        }
    }

    /// Saves the project to `path`, which becomes its path. Returns false if the save failed.
    fn save_to(&mut self, path: PathBuf) -> bool {
        match project_file::save(self.history.project(), &path) {
            Ok(()) => {
                self.history.mark_saved();
                // A lesson's sandbox is untitled too, but the untitled autosave is the user's.
                if self.path.is_none()
                    && self.lesson.is_none()
                    && let Some(p) = &self.untitled
                {
                    autosave::clear(p);
                }
                welcome::remember(
                    &mut self.welcome.recent,
                    Recent::of(&path, self.history.project()),
                );
                self.path = Some(path);
                true
            }
            Err(e) => {
                self.error(tr!("app.save_failed", path = path.display(), error = e));
                false
            }
        }
    }

    /// Opens the export dialog; while jobs exist it shows progress and a chevron opens the queue
    /// menu. After a failure the chevron is a Red alert until the menu opens, since the toast may
    /// have gone unseen during a long render.
    fn export_button(&mut self, ui: &mut Ui) {
        if !self.queue.is_empty() {
            let button = if self.queue.failed_unseen {
                Button::icon(icon::CIRCLE_ALERT, tr!("export.queue")).ink(color::red())
            } else {
                Button::icon(icon::CHEVRON_DOWN, tr!("export.queue"))
            };
            let r = ui
                .add(button)
                .on_hover_text(trn!("export.queue_pending", self.queue.pending()));
            let shortcut = self.keymap.shortcut("export");
            let mut action = None;
            egui::Popup::menu(&r)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| action = self.queue.menu(ui, &shortcut));
            self.queue_action(ui.ctx(), action);
        }
        if self.queue.pending() > 0 {
            ui.ctx().request_repaint();
        }
        let label = match self.queue.running() {
            Some(p) => tr!("export.progress", percent = format!("{:.0}", p * 100.0)),
            None => tr!("export.export").into(),
        };
        let state = if self.can_export() {
            State::Live
        } else {
            State::Disabled
        };
        // While a queue exists, "Exporting 9%" to "Export" keeps the chevron still.
        let min = if self.queue.is_empty() { 0.0 } else { 112.0 };
        let export = ui
            .add(Button::primary(&label).state(state).width(min))
            .on_disabled_hover_text(tr!("export.nothing"));
        tutorial::anchor(ui.ctx(), "export", export.rect);
        if export.clicked() {
            self.run_action("export", true);
        }
    }

    /// What the Export menu or the Render Queue panel asked the app for.
    fn queue_action(&mut self, ctx: &egui::Context, action: Option<QueueAction>) {
        match action {
            Some(QueueAction::New) if self.can_export() => {
                egui::Popup::close_all(ctx);
                self.run_action("export", true);
            }
            Some(QueueAction::Show) => {
                egui::Popup::close_all(ctx);
                self.show_queue();
            }
            Some(QueueAction::Close) => self.close_panel(Tab::RenderQueue),
            _ => {}
        }
    }

    /// Focuses the Render Queue panel, first splitting it off the right of Timeline if closed.
    pub fn show_queue(&mut self) {
        self.show_panel(Tab::RenderQueue);
    }

    /// Focuses a closable panel, first splitting it off the right of Timeline if closed.
    fn show_panel(&mut self, tab: Tab) {
        if self.dock.find_tab(&tab).is_none() {
            match self.dock.find_main_surface_tab(&Tab::Timeline) {
                Some((node, _)) => {
                    let tree = self.dock.main_surface_mut();
                    tree.split_right(node, 0.5, vec![tab]);
                }
                None => self.dock.push_to_first_leaf(tab),
            }
            hide_tab_bars(&mut self.dock);
        }
        if let Some(path) = self.dock.find_tab(&tab) {
            self.dock.set_focused_node_and_surface(path.node_path());
        }
    }

    fn close_panel(&mut self, tab: Tab) {
        if let Some(path) = self.dock.find_tab(&tab) {
            self.dock.remove_tab(path);
        }
    }

    fn can_export(&self) -> bool {
        self.history.project().sequence().duration() > Time(0) && self.preview.gpu().is_some()
    }

    /// Queues `req` on a snapshot of the project, so later edits don't change the file.
    fn queue_export(&mut self, req: Request, gpu: (wgpu::Device, wgpu::Queue), now: f64) {
        if self.queue.writing(&req.path) {
            let msg = tr!("export.busy", path = req.path.display());
            self.toasts.push((msg, now + TOAST_SECS));
            return;
        }
        let project = self.history.project().clone();
        let path = req.path.clone();
        let preset = match req.lossless {
            Some(crate::export::Lossless::Copy) => tr!("export.copy").to_owned(),
            Some(crate::export::Lossless::Smart) => tr!("export.smart").to_owned(),
            None => crate::export::shown_name(&self.export_dialog.presets, &req.preset).into(),
        };
        let duration = req.range.1 - req.range.0;
        let job = crate::export::submit(&self.exports, project, req, gpu);
        self.queue.push(path, preset, duration, job);
    }

    /// Takes finished exports' results: each posts a notification, failures also a toast.
    fn poll_export(&mut self, now: f64) {
        for (path, r) in self.queue.poll() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            match r {
                Ok(()) => crate::export::queue::notify(tr!("export.finished"), &name),
                Err(ExportError::Failed(e)) => {
                    crate::export::queue::notify(
                        tr!("export.failed_title"),
                        &tr!("export.failed_file", name = name, error = e),
                    );
                    self.toasts
                        .push((tr!("export.failed", error = e), now + TOAST_SECS));
                }
                Err(ExportError::Cancelled) => {}
            }
        }
    }

    /// Saves the shown frame as a PNG at full project resolution.
    fn export_frame(&mut self) {
        let Some(path) = FileDialog::new()
            .set_title(tr!("action.export_frame"))
            .add_filter(tr!("app.png_filter"), &["png"])
            .set_file_name(tr!("app.frame_file"))
            .save_file()
        else {
            return;
        };
        if let Err(e) = self
            .preview
            .export_frame(self.history.project().settings(), &path)
        {
            self.error(tr!("app.export_failed", path = path.display(), error = e));
        }
    }

    /// `,` / `.`: the Source viewer's in/out at the playhead on the targeted tracks, then the
    /// playhead moves to the end of the new clip, as in Premiere.
    fn place_from_source(&mut self, insert: bool) {
        let Some(asset) = self.preview.source_asset() else {
            return;
        };
        let at = self.timeline.playhead();
        let Some(p) = self
            .history
            .project()
            .placement(asset, at, insert, None, false)
        else {
            return;
        };
        let end = p.start + (p.source_out - p.source_in);
        match self.history.apply(Command::Place(p)) {
            Ok(_) => self.timeline.seek(end),
            Err(e) => tracing::warn!("place failed: {e}"),
        }
    }

    /// Runs a keymap action; `menu` (Edit menu or app bar) targets the timeline whatever has
    /// focus. shortcut: unimplemented actions are stubs until their features land.
    fn run_action(&mut self, id: &str, menu: bool) {
        // Space and K stop the take along with playback, wherever focus is.
        if self.take.is_some() && matches!(id, "play_stop" | "shuttle_stop" | "record") {
            return self.end_take();
        }
        match id {
            "record" => return self.start_take(),
            "undo" => {
                self.history.undo();
                return;
            }
            "redo" | "redo_alt" => {
                self.history.redo();
                return;
            }
            "new_project" => return self.file_action(FileAction::New),
            "open_project" => return self.file_action(FileAction::Open),
            "save" => return self.file_action(FileAction::Save),
            "save_as" => return self.file_action(FileAction::SaveAs),
            "import" => return self.import_dialog(),
            "import_subtitles" => return self.import_subtitles(),
            "generate_subtitles" => {
                if self.can_generate_subtitles() {
                    self.subtitles.open();
                }
                return;
            }
            "export" => {
                if self.can_export() {
                    let seq = self.history.project().sequence();
                    self.export_dialog.open(seq, &self.name());
                }
                return;
            }
            _ => {}
        }
        let preview_focused = !menu
            && self
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
        let program_focused = preview_focused && self.preview_active == 1;
        if (timeline_focused || program_focused) && PROGRAM_ACTIONS.contains(&id) {
            self.preview_active = 1;
            self.preview.program_action(id);
            return;
        }
        let marking = program_focused && MARK_ACTIONS.contains(&id);
        if (timeline_focused || menu || marking) && self.timeline.action(id, self.history.project())
        {
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

    /// Starts a voiceover take on the armed track at the playhead, and plays the timeline
    /// under it. With no audio track armed, records into a new one, armed. Errors leave the
    /// project unchanged.
    fn start_take(&mut self) {
        let Some(dir) = crate::record::media_dir(self.path.as_deref()) else {
            return;
        };
        let seq = self.history.project().sequence();
        let armed = self.timeline.armed.and_then(|id| seq.track(id));
        let armed = armed.filter(|t| t.kind == TrackKind::Audio).map(|t| t.id);
        // One undo step with the track, rolled back if the device won't open.
        let txn = self.history.begin();
        let track = match armed {
            Some(id) => Ok(id),
            None => self
                .history
                .apply(Command::AddTrack {
                    kind: TrackKind::Audio,
                    name: String::new(),
                })
                .map_err(|e| e.to_string())
                .and_then(|c| match c {
                    jvid_core::history::Created::Track(id) => Ok(id),
                    _ => Err(tr!("timeline.record_no_track").into()),
                }),
        };
        let device = Some(self.input_device.as_str()).filter(|d| !d.is_empty());
        let input = self
            .input
            .get_or_insert_with(|| Box::new(jvid_audio::CpalInput::default()));
        let start = self.timeline.playhead();
        let take = track.and_then(|track| {
            crate::record::Take::start(input.as_mut(), device, &dir, track, start)
        });
        match take {
            Ok(take) => {
                if txn {
                    self.history.commit();
                }
                self.timeline.armed = Some(take.track);
                self.take = Some(take);
                self.preview_active = 1;
                if !self.preview.playing() {
                    self.preview.program_action("shuttle_right");
                }
            }
            Err(e) => {
                if txn {
                    self.history.rollback();
                }
                self.timeline.notice = Some(tr!("timeline.record_failed", error = e));
            }
        }
    }

    /// Ends the take, stops playback and places the recording.
    fn end_take(&mut self) {
        let Some(take) = self.take.take() else {
            return;
        };
        self.timeline.level = None;
        self.timeline.live = None;
        self.preview.program_action("shuttle_stop");
        let path = take.path.clone();
        match take.finish(&mut self.history) {
            Ok(Some(peaks)) => self.timeline.recorded(path, peaks),
            Ok(None) => {}
            Err(e) => self.timeline.notice = Some(tr!("timeline.record_save_failed", error = e)),
        }
    }

    /// Runs the take a frame: feeds the header meter and the live clip, and ends it when the
    /// track is disarmed or the device fails.
    fn poll_take(&mut self, ctx: &egui::Context) {
        let Some(take) = &mut self.take else {
            return;
        };
        if self.timeline.armed != Some(take.track) {
            return self.end_take();
        }
        match take.poll() {
            Ok(level) => {
                self.timeline.level = Some(level);
                take.show(&mut self.timeline.live);
                ctx.request_repaint();
            }
            Err(e) => {
                self.end_take();
                self.timeline.notice = Some(e);
            }
        }
    }

    /// Before the project is replaced: a take in progress lands in the old one.
    fn leave_project(&mut self) {
        self.end_take();
        self.timeline.armed = None;
    }

    /// Asks for media files and queues them for import.
    fn import_dialog(&mut self) {
        let mut dialog = FileDialog::new().set_title(tr!("app.import_title"));
        // A lesson's sample clips sit in the app data folder, where no one would look.
        if self.lesson.is_some() {
            match crate::samples::dir() {
                Ok(Some(dir)) => dialog = dialog.set_directory(dir),
                Ok(None) => {}
                Err(e) => tracing::warn!("couldn't write the sample clips: {e}"),
            }
        }
        if let Some(paths) = dialog.pick_files() {
            self.import(paths);
        }
    }

    /// Asks for an `.srt` and adds it as a subtitle track in the Caption style.
    fn import_subtitles(&mut self) {
        let Some(path) = FileDialog::new()
            .set_title(tr!("action.import_subtitles"))
            .add_filter(verbatim("SubRip"), &["srt"])
            .pick_file()
        else {
            return;
        };
        let cues = match std::fs::read(&path) {
            Ok(b) => jvid_core::srt::parse(&String::from_utf8_lossy(&b)),
            Err(e) => return tracing::warn!("couldn't read {}: {e}", path.display()),
        };
        let [.., (_, style)] = jvid_core::model::Text::presets();
        let name = tr!("app.subtitles").to_string();
        if let Err(e) = self
            .history
            .apply(Command::ImportSubtitles { name, cues, style })
        {
            tracing::warn!("subtitle import failed: {e}");
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
        let dialog =
            FileDialog::new().set_title(tr!("app.relink_title", name = asset.display_name()));
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

    /// `None` in a lesson, so the sandbox never overwrites the untitled project's autosave.
    fn autosave_target(&self) -> Option<PathBuf> {
        if self.lesson.is_some() {
            return None;
        }
        self.path.clone().or_else(|| self.untitled.clone())
    }

    /// Runs `then` now if the project is clean, else once the user saves or discards it.
    fn discard_then(&mut self, then: Then) {
        // The question is about the user's project, not the lesson's.
        self.end_lesson();
        if self.history.is_dirty() {
            self.prompts.push(Prompt::Discard(then));
        } else {
            self.proceed(then);
        }
    }

    fn proceed(&mut self, then: Then) {
        match then {
            Then::New => {
                self.new_project(Some(ProjectSettings::default()));
                self.on_welcome = true;
            }
            Then::Open => {
                if let Some(path) = project_dialog().pick_file() {
                    self.open(path);
                }
            }
            Then::Recent(path) => self.open(path),
            Then::Close => self.closing = true,
        }
    }

    fn error(&mut self, message: String) {
        self.prompts.push(Prompt::Error(message));
    }

    /// Shows the first waiting prompt and acts on its answer.
    fn show_prompt(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.prompts.first() else {
            return;
        };
        let name = self.name();
        let (title, body, buttons, cancel): (_, Cow<str>, &[_], _) = match prompt {
            Prompt::Discard(_) => (
                tr!("prompt.unsaved"),
                tr!("prompt.unsaved_body", name = name).into(),
                &[
                    (tr!("prompt.save"), ButtonKind::Primary),
                    (tr!("prompt.cancel"), ButtonKind::Secondary),
                    (tr!("prompt.dont_save"), ButtonKind::Destructive),
                ],
                1,
            ),
            Prompt::Restore { path, .. } => (
                tr!("prompt.restore"),
                tr!(
                    "prompt.restore_body",
                    name = path
                        .as_deref()
                        .and_then(Path::file_stem)
                        .map_or(tr!("app.untitled").into(), |s| s.to_string_lossy())
                )
                .into(),
                &[
                    (tr!("prompt.restore_button"), ButtonKind::Primary),
                    (tr!("prompt.discard"), ButtonKind::Destructive),
                ],
                1,
            ),
            Prompt::Error(message) => (
                tr!("prompt.error"),
                message.clone().into(),
                &[(tr!("prompt.ok"), ButtonKind::Primary)],
                0,
            ),
            Prompt::About => (
                verbatim("jvid"),
                format!(
                    "{}\n\n{}\n{}",
                    tr!("about.version", version = env!("CARGO_PKG_VERSION")),
                    tr!("about.samples"),
                    crate::samples::credits().join("\n")
                )
                .into(),
                &[
                    (tr!("prompt.ok"), ButtonKind::Primary),
                    (tr!("about.notices"), ButtonKind::Secondary),
                ],
                0,
            ),
            Prompt::Notices => (
                tr!("about.notices"),
                crate::notices::TEXT.into(),
                &[(tr!("prompt.ok"), ButtonKind::Primary)],
                0,
            ),
        };
        let dialog = Dialog {
            title,
            body: &body,
            buttons,
            cancel,
            long: *prompt == Prompt::Notices,
        };
        if let Some(answer) = dialog.show(ctx) {
            let prompt = self.prompts.remove(0);
            self.answer(prompt, answer);
        }
    }

    /// Acts on button `answer` of `prompt`, numbered as in [`Self::show_prompt`].
    fn answer(&mut self, prompt: Prompt, answer: usize) {
        match (prompt, answer) {
            (Prompt::Discard(then), 0) => {
                if self.save(false) {
                    self.proceed(then);
                }
            }
            (Prompt::Discard(then), 2) => {
                if let Some(p) = self.autosave_target() {
                    autosave::clear(&p);
                }
                self.proceed(then);
            }
            (Prompt::Restore { path, auto }, 0) => match project_file::load(&auto) {
                Ok(project) => {
                    if path.is_none() {
                        self.new_project(Some(project.settings().clone()));
                    }
                    self.leave_project();
                    self.history = History::new(project);
                    self.history.mark_unsaved();
                }
                Err(e) => self.error(tr!("app.restore_failed", path = auto.display(), error = e)),
            },
            (Prompt::About, 1) => self.prompts.push(Prompt::Notices),
            (Prompt::Restore { path: None, .. }, _) => {
                if let Some(p) = &self.untitled {
                    autosave::clear(p);
                }
            }
            _ => {}
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
        if ctx.input(|i| i.viewport().close_requested()) {
            self.end_lesson();
        }
        self.lesson_keys(&ctx);
        if ctx.input(|i| i.viewport().close_requested()) && !self.closing && self.history.is_dirty()
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            if !self.prompts.contains(&Prompt::Discard(Then::Close)) {
                self.prompts.push(Prompt::Discard(Then::Close));
            }
        }
        let was_closing = self.closing;
        self.show_prompt(&ctx);
        if self.closing && !was_closing {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }

        let timeline = self
            .dock
            .find_active_focused()
            .is_some_and(|(_, t)| *t == Tab::Timeline);
        let pressed =
            if self.keymap_editor.is_open() || self.subtitles.is_open() || !self.prompts.is_empty()
            {
                Vec::new()
            } else {
                self.keymap.pressed(&ctx, timeline)
            };
        for id in pressed {
            if id == "quit" {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            } else {
                self.run_action(id, false);
            }
        }
        if !self.keymap.held(&ctx, "add_marker") {
            self.timeline.release_marker();
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
        self.poll_take(&ctx);
        // The Program transport row's Record button shows the take Shift+R drives.
        self.preview.recording = self.take.as_ref().map(|t| {
            let level = self.timeline.level.unwrap_or(0.0);
            (t.began.elapsed(), level)
        });
        self.poll_import(now);
        self.poll_export(now);
        let target = self.autosave_target().filter(|_| !self.on_welcome);
        if let Some(due) = self.autosave.tick(&self.history, target.as_deref(), now) {
            ctx.request_repaint_after(due);
        }
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
        // Offered on the first Welcome screen only, whatever left it.
        if self.welcome.tour {
            self.tour_offered();
        }

        egui::Panel::top("app_bar")
            .exact_size(size::APP_BAR)
            .frame(Frame::NONE.fill(color::ground()))
            .show_separator_line(false)
            .show(ui, |ui| self.app_bar(ui));

        let gutter = space::GUTTER as i8;
        let frame = Frame::NONE.fill(color::ground()).inner_margin(Margin {
            left: gutter,
            right: gutter,
            top: 0,
            bottom: gutter,
        });
        // Before the dock, so the Effects tab and Add Effect menu draw this frame's thumbnails.
        crate::effects::draw_thumbs(&ctx, &self.preview, self.bin.tab == 1);
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
                keymap: &self.keymap,
                queue: &mut self.queue,
                queue_action: None,
                transcript: &mut self.transcript,
                transcript_out: Default::default(),
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
            let (import, command, gesture, relink, open, queue, transcript) = (
                viewer.import_clicked,
                viewer.command,
                viewer.gesture,
                viewer.relink,
                viewer.open,
                viewer.queue_action,
                viewer.transcript_out,
            );
            if transcript.close {
                self.close_panel(Tab::Transcript);
            }
            if let Some(sources) = transcript.transcribe {
                self.subtitles.open_transcribe(sources);
            }
            if std::mem::take(&mut self.preview.record_clicked) {
                self.run_action("record", true);
            }
            self.queue_action(ui.ctx(), queue);
            if let Some(msg) = self.timeline.notice.take() {
                self.toasts.push((msg, now + TOAST_SECS));
            }
            if let Some(id) = self.timeline.reveal.take() {
                self.bin.selected = Some(id);
                self.run_action("focus_media", false);
            }
            // A sequence opens in the timeline rather than the source viewer.
            let nested = open.and_then(|id| self.history.project().asset(id)?.sequence);
            if let Some(id) = self.timeline.open_sequence.take().or(nested)
                && let Err(e) = self.history.open_sequence(id)
            {
                tracing::warn!("open sequence: {e}");
            }
            if let Some(id) = open.filter(|_| nested.is_none()) {
                self.preview.open(self.history.project(), id);
                self.preview_active = 0;
            }
            let applied = match (command, gesture) {
                (Some(c), Some(k)) => Some(self.history.apply_coalesced(k, c)),
                (Some(c), None) => Some(self.history.apply(c)),
                (None, _) => None,
            };
            if let Some(r) = &applied {
                self.timeline.applied(r.is_ok());
            }
            let hide = self.timeline.hide_region.take();
            if let Some(id) = hide.or(crate::hide_region::requested(ui.ctx()))
                && let Some(blur) = crate::hide_region::blur()
                && let Some(copy) = crate::hide_region::apply(&mut self.history, id, &blur)
                && let Some((_, c)) = self.history.project().sequence().clip(copy)
            {
                // Selected, with the playhead in it, so the mask's handles show to place it.
                let (start, end) = (c.start, c.end());
                self.timeline
                    .select_clip(self.history.project().sequence(), copy);
                let t = self.timeline.playhead();
                if !(start <= t && t < end) {
                    self.timeline.seek(start);
                }
            }
            if let Some(Err(e)) = applied {
                tracing::warn!("edit failed: {e}");
                // Refusals a bin drop or delete can meet, which nothing else explains.
                let msg = match e {
                    ModelError::Cycle => Some(tr!("timeline.cycle").to_owned()),
                    ModelError::TooDeep => Some(tr!("timeline.too_deep", max = MAX_NEST_DEPTH)),
                    ModelError::SequenceOpen => Some(tr!("bin.sequence_open").to_owned()),
                    _ => None,
                };
                if let Some(msg) = msg {
                    self.toasts.push((msg, now + TOAST_SECS));
                }
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
            let selected = self.timeline.selected();
            if let Some((reqs, _)) = self
                .export_dialog
                .show(ui, self.history.project(), selected)
                && let Some(gpu) = self.preview.gpu()
            {
                for req in reqs {
                    self.queue_export(req, gpu.clone(), now);
                }
            }
            if let Some(c) = self.settings_dialog.show(ui, self.history.project())
                && let Err(e) = self.history.apply(c)
            {
                tracing::warn!("edit failed: {e}");
            }
            let selected = self.timeline.selected();
            match self.subtitles.show(ui, self.history.project(), selected) {
                Some(Done::Edit(c)) => {
                    if let Err(e) = self.history.apply(*c) {
                        tracing::warn!("subtitles failed: {e}");
                    }
                }
                Some(Done::Transcribed(done)) => {
                    for (source, t) in done {
                        self.transcript.cache.insert(&source, t);
                    }
                }
                None => {}
            }
            let (language, input) = (&mut self.language, &mut self.input_device);
            let (theme, motion) = (&mut self.theme_setting, &mut self.reduce_motion);
            let changed =
                self.preferences
                    .show(ui, language, theme, motion, input, |key, value| {
                        let path = crate::settings_file::path()
                            .ok_or_else(|| std::io::Error::other(tr!("settings.no_config_dir")))?;
                        crate::settings_file::save_value_to(&path, key, value)
                    });
            if changed {
                self.theme = suite_ui::themes::LiveTheme::new(&self.theme_setting);
            }
        });

        self.lesson_frame(&ctx);
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            drop_overlay(&ctx);
        }
        self.show_toasts(&ctx);

        let dirty = if self.history.is_dirty() { " •" } else { "" };
        let title = tr!("app.window_title", name = self.name(), dirty = dirty);
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

/// A ghost app-bar menu button labelled `name`, its left edge at `left`.
fn menu_button(ui: &mut Ui, name: &str, left: f32) -> egui::Response {
    let r = ui.max_rect();
    let text_w = ui
        .painter()
        .layout_no_wrap(name.into(), theme::body_strong(), color::ink())
        .size()
        .x;
    let w = (2.0 * space::S12 + text_w).ceil();
    let at = egui::Rect::from_min_size(
        egui::pos2(left, r.center().y - size::CONTROL / 2.0),
        vec2(w, size::CONTROL),
    );
    ui.put(at, Button::ghost(name))
}

/// File > Reframe's frame shapes, width:height.
const REFRAME: [(&str, [u32; 2]); 3] = [("9:16", [9, 16]), ("1:1", [1, 1]), ("4:5", [4, 5])];

fn project_dialog() -> FileDialog {
    FileDialog::new().add_filter(tr!("app.project_filter"), &["jvid"])
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
fn hide_tab_bars(dock: &mut DockState<Tab>) {
    for (_, node) in dock.iter_all_nodes_mut() {
        if node.is_leaf() {
            node.set_tab_bar_hidden(true);
        }
    }
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
    keymap: &'a Keymap,
    queue: &'a mut crate::export::queue::Queue,
    queue_action: Option<QueueAction>,
    transcript: &'a mut crate::transcript::TranscriptPanel,
    transcript_out: crate::transcript::Output,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(*tab)
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        tutorial::anchor(ui.ctx(), tab.anchor(), ui.max_rect());
        let focused = self.focused == Some(*tab);
        if *tab == Tab::Media {
            self.bin.text_thumbs(self.preview, self.project);
            // The Scopes tab, fourth in the bin.
            self.preview.scopes_open(self.bin.tab == 3);
            let selected = self.timeline.selected();
            self.bin.fx_apply = !crate::inspector::video_clips(self.project, selected).is_empty();
            let r = self
                .bin
                .show(ui, self.project, focused, self.importing, self.missing);
            self.import_clicked |= r.import;
            // The Source tab may have set one this frame too.
            self.command = r.command.or(self.command.take());
            if let Some(stem) = r.apply_effect {
                let selected = self.timeline.selected();
                let add = crate::inspector::add_effect(self.project, selected, &stem);
                self.command = add.or(self.command.take());
            }
            self.relink = r.relink;
            self.open = r.open;
            if r.thumbs {
                self.preview.refresh_thumb();
            }
            return;
        }
        if *tab == Tab::RenderQueue {
            self.queue_action = self.queue.panel(ui, focused);
            return;
        }
        if *tab == Tab::Transcript {
            let (selected, t) = (self.timeline.selected(), self.timeline.playhead());
            let mut out = self.transcript.show(ui, self.project, selected, t, focused);
            if let Some(t) = out.seek.take() {
                self.timeline.seek(t);
            }
            self.command = out.command.take().or(self.command.take());
            self.transcript_out = out;
            return;
        }
        let source_tab = self.preview_views.get(*self.preview_active) == Some(&"source");
        let views: Vec<&str> = self.preview_views.iter().map(|v| view_title(v)).collect();
        let panel = match (*tab, views.as_slice()) {
            (Tab::Preview, [one]) => Panel::titled(one),
            (Tab::Preview, views) => Panel::tabbed(views, self.preview_active),
            _ => Panel::titled(tab.title()),
        };
        let panel = panel.focused(focused);
        let (actions, on) = self.preview.compare_actions();
        let panel = if *tab == Tab::Preview && !source_tab {
            panel.actions(&actions).on(&on)
        } else {
            panel
        };
        if *tab == Tab::Preview && source_tab {
            panel.show(ui, |ui| {
                if let Some(c) = self.preview.show_source(ui, self.project, self.missing) {
                    self.command = Some(c);
                }
            });
            return;
        }
        if *tab == Tab::Preview {
            // While a clip edge is dragged, the frame at the trim point.
            let trim = self.timeline.trim_frame(self.project);
            let trim = trim.and_then(|(a, t)| {
                let a = self.project.asset(a)?;
                let video = a.probe.as_ref().is_none_or(|p| p.width.is_some());
                Some((a.path.as_path(), t, video))
            });
            let offline = trim.is_some_and(|(p, ..)| self.missing.contains(p));
            let r = panel.show(ui, |ui| {
                let frame = self.preview.project_rect(ui, self.project.settings());
                let head = self.timeline.playhead();
                let shown = self
                    .preview
                    .show_program(ui, self.project, head, trim, offline);
                if let Some(t) = shown {
                    self.timeline.seek(t);
                }
                let (selected, t) = (self.timeline.selected(), self.timeline.playhead());
                if crate::handles::editing(ui.ctx()) {
                    if let Some((c, g)) = crate::handles::edit_text(ui, self.project, t, frame) {
                        (self.command, self.gesture) = (Some(c), g);
                    }
                    return;
                }
                if let Some((c, g)) = crate::handles::show(ui, self.project, selected, t, frame) {
                    (self.command, self.gesture) = (Some(c), g);
                }
                // After the handles, so an Alt-drag draws a zoom instead of moving the clip.
                if let Some(c) = crate::zoom::show(ui, self.project, selected, t, frame) {
                    (self.command, self.gesture) = (Some(c), None);
                }
                // After the zoom, so Alt on a mask handle breaks its tangents instead.
                if let Some((c, g)) = crate::masks::show(ui, self.project, selected, t, frame) {
                    (self.command, self.gesture) = (Some(c), g);
                }
                tutorial::ghost(ui, frame, self.project.settings().width);
                if let Some((id, at)) =
                    crate::handles::double_clicked_text(ui, self.project, t, frame)
                {
                    self.timeline.select_clip(self.project.sequence(), id);
                    crate::handles::start_editing(ui.ctx(), self.project, id, at);
                }
            });
            if let Some(i) = r.action {
                self.preview.toggle_compare(i);
                ui.ctx().request_repaint();
            }
            return;
        }
        if *tab == Tab::Timeline {
            let r = panel.show(ui, |ui| {
                let edit = self
                    .timeline
                    .show(ui, self.project, self.missing, self.keymap);
                if let Some((c, g)) = edit {
                    (self.command, self.gesture) = (Some(c), g);
                }
            });
            let header = r
                .response
                .rect
                .with_max_y(r.response.rect.top() + size::HEADER);
            let title_w = ui
                .painter()
                .layout_no_wrap(tab.title().to_owned(), theme::body_strong(), color::ink())
                .size()
                .x;
            let left = header.left() + space::INSET + title_w + space::S12;
            let (project, keymap) = (self.project, self.keymap);
            self.timeline.toolbar(ui, header, left, project, keymap);
            return;
        }
        if *tab == Tab::Inspector {
            let solid = self.bin.selected.and_then(|id| self.project.asset(id));
            let sel =
                crate::inspector::Selection::new(self.project, self.timeline.selected(), solid);
            let panes = sel.panes();
            let labels: Vec<String> = panes.iter().map(|p| sel.label(*p)).collect();
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            let shown = crate::inspector::active(ui.ctx(), &panes);
            let mut active = shown;
            let panel = if panes.is_empty() {
                Panel::titled(tab.title())
            } else {
                Panel::tabbed(&labels, &mut active).wide(true)
            };
            // The new tab shows next frame; the click already repaints.
            panel.focused(focused).show(ui, |ui| {
                let t = self.timeline.playhead();
                let pane = panes.get(shown).copied();
                let (edit, seek) = egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| crate::inspector::show(ui, &sel, pane, t))
                    .inner;
                if let Some((c, g)) = edit {
                    (self.command, self.gesture) = (Some(c), g);
                }
                if let Some(t) = seek {
                    self.timeline.seek(t);
                }
            });
            if active != shown {
                crate::inspector::choose(ui.ctx(), panes[active]);
            }
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
    painter.rect_filled(rect, corner, color::blue().gamma_multiply(0.08));
    painter.rect_stroke(
        rect,
        corner,
        Stroke::new(2.0, color::light_blue()),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        tr!("app.drop_to_import"),
        theme::title(),
        color::ink(),
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
                        .fill(color::raised())
                        .inner_margin(Margin::same(space::S12 as i8))
                        .show(ui, |ui| {
                            ui.set_max_width(360.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(icon::CIRCLE_ALERT)
                                        .font(theme::icon_font())
                                        .color(color::red()),
                                );
                                ui.add(egui::Label::new(msg.as_str()).wrap());
                                if ui.add(Button::icon(icon::X, tr!("app.dismiss"))).clicked() {
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
                (true, false) => color::blue(),
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
