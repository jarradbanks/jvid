//! The render queue: exports run one after another in the background while editing goes on.
//! Each job owns a clone of the project taken when it was queued, so later edits don't reach it.
//! The Export button's menu sums it up; the Render Queue panel lists every job.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

use egui::{
    Align, Align2, Color32, CornerRadius, Layout, Rect, RichText, Sense, Ui, UiBuilder, WidgetInfo,
    WidgetType, pos2, vec2,
};
use jvid_core::model::ProbeInfo;
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use jvid_jobs::Handle;
use suite_ui::panel::Panel;
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, radius, size, space};
use suite_ui::widgets::{Button, State, fit_text, paint_fit};

use super::Error;
use crate::i18n::{tr, trn};

struct Job {
    path: PathBuf,
    /// The preset's shown name, or the lossless mode's.
    preset: String,
    /// The exported range's length.
    duration: Time,
    handle: Handle,
    rx: Receiver<Result<ProbeInfo, Error>>,
    /// When the job became the one running, for time left.
    started: Option<Instant>,
    /// Its place in the order jobs were pushed, counting ones since cleared.
    seq: usize,
    /// `Some` once the job is over.
    result: Option<Result<(), Error>>,
    /// The file's size in bytes, once done.
    size: Option<u64>,
}

impl Job {
    /// Not over and not cancelled; a cancelled job may still be winding down.
    fn live(&self) -> bool {
        self.result.is_none() && !self.handle.is_cancelled()
    }
}

/// One job as the menu and the panel show it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub path: PathBuf,
    pub preset: String,
    pub duration: Time,
    pub size: Option<u64>,
    pub status: Status,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Waiting,
    /// `left` is seconds to go, once there's enough progress to tell.
    Rendering {
        progress: f32,
        left: Option<f64>,
    },
    Done,
    Failed(String),
    Cancelled,
}

impl Status {
    fn live(&self) -> bool {
        matches!(self, Status::Waiting | Status::Rendering { .. })
    }
}

/// What a click in the menu or the panel asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    New,
    /// Show Render Queue.
    Show,
    /// The panel's close button.
    Close,
    Reveal(usize),
    Cancel(usize),
    CancelAll,
    ConfirmCancelAll,
    KeepExporting,
    ClearFinished,
}

/// Seconds left, from the time spent and the share done; `None` under 5%, where it would jump.
pub fn time_left(elapsed: f64, progress: f32) -> Option<f64> {
    let p = progress as f64;
    (p >= 0.05).then(|| elapsed / p * (1.0 - p))
}

#[derive(Default)]
pub struct Queue {
    jobs: Vec<Job>,
    /// Cancel All was clicked with more than one job live and waits for a confirm.
    confirm_all: bool,
    /// A job failed since the menu or the panel was last shown.
    pub failed_unseen: bool,
    /// Jobs pushed so far, cleared ones included.
    pushed: usize,
}

impl Queue {
    /// Adds a submitted export; refuses one writing to a path a live job is already writing.
    pub fn push(
        &mut self,
        path: PathBuf,
        preset: String,
        duration: Time,
        (handle, rx): (Handle, Receiver<Result<ProbeInfo, Error>>),
    ) -> bool {
        if self.writing(&path) {
            return false;
        }
        self.jobs.push(Job {
            path,
            preset,
            duration,
            handle,
            rx,
            started: None,
            seq: self.pushed,
            result: None,
            size: None,
        });
        self.pushed += 1;
        true
    }

    /// True while a job not yet over writes to `path`.
    pub fn writing(&self, path: &Path) -> bool {
        self.jobs
            .iter()
            .any(|j| j.result.is_none() && j.path == path)
    }

    /// Collects results; returns the jobs that ended since the last call.
    pub fn poll(&mut self) -> Vec<(PathBuf, Result<(), Error>)> {
        let mut ended = Vec::new();
        for j in self.jobs.iter_mut().filter(|j| j.result.is_none()) {
            let r = match j.rx.try_recv() {
                Ok(r) => r.map(|_| ()),
                Err(TryRecvError::Empty) => continue,
                // A job skipped after cancelling drops its sender; so does one that panicked.
                Err(TryRecvError::Disconnected) if j.handle.is_cancelled() => Err(Error::Cancelled),
                Err(TryRecvError::Disconnected) => Err(Error::Failed(tr!("export.crashed").into())),
            };
            if r.is_ok() {
                j.size = std::fs::metadata(&j.path).ok().map(|m| m.len());
            }
            self.failed_unseen |= matches!(r, Err(Error::Failed(_)));
            ended.push((j.path.clone(), r.clone()));
            j.result = Some(r);
        }
        if let Some(j) = self.jobs.iter_mut().find(|j| j.live()) {
            j.started.get_or_insert_with(Instant::now);
        }
        ended
    }

    /// Progress of the job running now, if any; the export pool runs one job at a time, in order.
    pub fn running(&self) -> Option<f32> {
        let mut live = self.jobs.iter().filter(|j| j.live());
        live.next().map(|j| j.handle.progress())
    }

    /// Jobs not yet over or cancelled.
    pub fn pending(&self) -> usize {
        self.jobs.iter().filter(|j| j.live()).count()
    }

    /// Cancels job `i` if it isn't over yet.
    pub fn cancel(&self, i: usize) {
        if let Some(j) = self.jobs.get(i).filter(|j| j.result.is_none()) {
            j.handle.cancel();
        }
    }

    pub fn cancel_all(&mut self) {
        self.jobs.iter().for_each(|j| j.handle.cancel());
        self.confirm_all = false;
    }

    /// Drops jobs that are over.
    pub fn clear_finished(&mut self) {
        self.jobs.retain(|j| j.result.is_none());
        self.failed_unseen = false;
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// Jobs pushed so far, cleared ones included: a mark for [`Queue::since`].
    pub fn pushed(&self) -> usize {
        self.pushed
    }

    /// The lengths of the jobs still listed that were pushed after `mark`.
    pub fn since(&self, mark: usize) -> impl Iterator<Item = Time> + '_ {
        self.jobs
            .iter()
            .filter(move |j| j.seq >= mark)
            .map(|j| j.duration)
    }

    /// Every job, oldest first; only the first live one renders.
    pub fn rows(&self) -> Vec<Row> {
        let running = self.jobs.iter().position(|j| j.live());
        let rows = self.jobs.iter().enumerate().map(|(i, j)| {
            let status = match &j.result {
                None if Some(i) == running => {
                    let progress = j.handle.progress();
                    let elapsed = j.started.map_or(0.0, |s| s.elapsed().as_secs_f64());
                    Status::Rendering {
                        progress,
                        left: time_left(elapsed, progress),
                    }
                }
                None if j.handle.is_cancelled() => Status::Cancelled,
                None => Status::Waiting,
                Some(Ok(())) => Status::Done,
                Some(Err(Error::Cancelled)) => Status::Cancelled,
                Some(Err(Error::Failed(e))) => Status::Failed(e.clone()),
            };
            Row {
                path: j.path.clone(),
                preset: j.preset.clone(),
                duration: j.duration,
                size: j.size,
                status,
            }
        });
        rows.collect()
    }

    /// Does what a menu or panel click asks; New, Show and Close are the caller's.
    pub fn apply(&mut self, a: Action) {
        match a {
            Action::Reveal(i) => {
                if let Some(j) = self.jobs.get(i) {
                    reveal(&j.path);
                }
            }
            Action::Cancel(i) => self.cancel(i),
            Action::CancelAll if self.pending() > 1 => self.confirm_all = true,
            Action::CancelAll | Action::ConfirmCancelAll => self.cancel_all(),
            Action::KeepExporting => self.confirm_all = false,
            Action::ClearFinished => self.clear_finished(),
            Action::New | Action::Show | Action::Close => {}
        }
    }

    /// The Export button's dropdown; `shortcut` is New Export's.
    pub fn menu(&mut self, ui: &mut Ui, shortcut: &str) -> Option<Action> {
        self.failed_unseen = false;
        let a = menu(ui, &self.rows(), self.confirm_all, shortcut);
        a.inspect(|a| self.apply(*a))
    }

    /// The Render Queue dock panel.
    pub fn panel(&mut self, ui: &mut Ui, focused: bool) -> Option<Action> {
        self.failed_unseen = false;
        let a = panel(ui, &self.rows(), self.confirm_all, focused);
        a.inspect(|a| self.apply(*a))
    }
}

/// The menu's fixed width, so it doesn't jump as jobs change.
const MENU_WIDTH: f32 = 300.0;
/// Finished jobs the menu lists, newest first.
const RECENT: usize = 3;

/// The summary menu: the running job's progress, the last few results, then the queue's actions.
pub fn menu(ui: &mut Ui, rows: &[Row], confirm: bool, shortcut: &str) -> Option<Action> {
    ui.set_width(MENU_WIDTH);
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut action = None;
    if rows.is_empty() {
        empty(ui);
    } else {
        summary(ui, rows);
        separator(ui);
        let finished = rows
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, r)| !r.status.live());
        for (i, r) in finished.take(RECENT) {
            if recent(ui, r) {
                action = Some(Action::Reveal(i));
            }
        }
        if rows.iter().any(|r| !r.status.live()) {
            separator(ui);
        }
    }
    let show = tr!("export.show_queue");
    if menu_row(ui, Some(icon::PANEL_BOTTOM), show, "", true, color::ink()) {
        action = Some(Action::Show);
    }
    if menu_row(
        ui,
        Some(icon::PLUS),
        tr!("export.new"),
        shortcut,
        true,
        color::ink(),
    ) {
        action = Some(Action::New);
    }
    if rows.is_empty() {
        return action;
    }
    separator(ui);
    let live = rows.iter().filter(|r| r.status.live()).count();
    if confirm {
        ui.horizontal(|ui| {
            ui.set_height(size::CONTROL);
            ui.add_space(space::S8);
            ui.label(RichText::new(trn!("export.cancel_confirm", live)).color(color::ink()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = space::S4;
                if ui
                    .add(Button::destructive(tr!("export.cancel_all")))
                    .clicked()
                {
                    action = Some(Action::ConfirmCancelAll);
                }
                if ui
                    .add(Button::ghost(tr!("export.keep_exporting")))
                    .clicked()
                {
                    action = Some(Action::KeepExporting);
                }
            });
        });
        return action;
    }
    let finished = rows.len() > live;
    let clear = tr!("export.clear_finished");
    if menu_row(ui, None, clear, "", finished, color::ink()) {
        action = Some(Action::ClearFinished);
    }
    let cancel = tr!("export.cancel_all");
    if menu_row(ui, None, cancel, "", live > 0, color::red()) {
        action = Some(Action::CancelAll);
    }
    action
}

/// "Rendering 1 of 2", the running job's bar, and its name and preset; "All Finished" after.
fn summary(ui: &mut Ui, rows: &[Row]) {
    let live = rows.iter().filter(|r| r.status.live()).count();
    let running = rows.iter().find_map(|r| match r.status {
        Status::Rendering { progress, left } => Some((r, progress, left)),
        _ => None,
    });
    let pad = space::S8;
    let h = if running.is_some() { 58.0 } else { 32.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let inner = rect.shrink(pad);
    let top = match running {
        Some(_) => tr!("export.rendering_of", total = live),
        None => tr!("export.all_finished").into(),
    };
    text(
        ui,
        inner.left_top(),
        Align2::LEFT_TOP,
        &top,
        theme::body_strong(),
        color::ink(),
    );
    let Some((r, progress, left)) = running else {
        return;
    };
    let pct = percent(progress, left);
    text(
        ui,
        inner.right_top(),
        Align2::RIGHT_TOP,
        &pct,
        theme::body(),
        color::ink_2(),
    );
    let bar_rect =
        Rect::from_min_size(inner.left_top() + vec2(0.0, 22.0), vec2(inner.width(), 6.0));
    bar(ui, bar_rect, progress);
    let bottom = format!("{} · {}", file_name(&r.path), r.preset);
    let galley = fit_text(ui, &bottom, theme::body(), color::ink_3(), inner.width());
    ui.painter().galley(
        pos2(inner.left(), inner.bottom() - galley.size().y),
        galley,
        color::ink_3(),
    );
}

/// A finished job: its status icon, name and, when done, Reveal. Returns true on Reveal.
fn recent(ui: &mut Ui, r: &Row) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), size::CONTROL), Sense::hover());
    tip(response, r);
    let (glyph, ink, _) = look(&r.status);
    text(
        ui,
        pos2(rect.left() + space::S8 + size::ICON / 2.0, rect.center().y),
        Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(),
        ink,
    );
    let left = rect.left() + space::S8 + size::ICON + space::S8;
    let reveal = Rect::from_center_size(
        pos2(
            rect.right() - space::S4 - size::SMALL_CONTROL / 2.0,
            rect.center().y,
        ),
        vec2(size::SMALL_CONTROL, size::SMALL_CONTROL),
    );
    let name = file_name(&r.path);
    let width = reveal.left() - space::S4 - left;
    let galley = fit_text(ui, &name, theme::body(), color::ink_2(), width);
    ui.painter().galley(
        pos2(left, rect.center().y - galley.size().y / 2.0),
        galley,
        color::ink_2(),
    );
    r.status == Status::Done
        && ui
            .put(reveal, Button::icon(icon::FOLDER, tr!("export.reveal")))
            .clicked()
}

/// What the menu shows with nothing queued: it keeps its width and still offers New Export.
fn empty(ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(space::S16);
        ui.label(
            RichText::new(tr!("export.empty_title"))
                .font(theme::title())
                .color(color::ink()),
        );
        ui.add_space(space::S4);
        ui.label(RichText::new(tr!("export.empty_help")).color(color::ink_3()));
        ui.add_space(space::S16);
    });
    separator(ui);
}

/// A 24px menu row: an optional icon, the label and a shortcut hint. Returns true when clicked.
fn menu_row(
    ui: &mut Ui,
    glyph: Option<&'static str>,
    label: &str,
    shortcut: &str,
    enabled: bool,
    ink: Color32,
) -> bool {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), size::CONTROL), sense);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    if enabled && (response.hovered() || response.has_focus()) {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::CONTROL), color::border());
    }
    let ink = if enabled { ink } else { color::ink_4() };
    let mut x = rect.left() + space::S8;
    if let Some(g) = glyph {
        let c = if enabled { color::ink_2() } else { ink };
        let at = pos2(x + size::ICON / 2.0, rect.center().y);
        text(ui, at, Align2::CENTER_CENTER, g, theme::icon_font(), c);
        x += size::ICON + space::S8;
    }
    text(
        ui,
        pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::body(),
        ink,
    );
    let at = pos2(rect.right() - space::S8, rect.center().y);
    text(
        ui,
        at,
        Align2::RIGHT_CENTER,
        shortcut,
        theme::small(),
        color::ink_3(),
    );
    response.clicked()
}

fn separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    let line = 1.0 / ui.pixels_per_point();
    ui.painter().rect_filled(
        Rect::from_x_y_ranges(rect.x_range(), rect.center().y..=rect.center().y + line),
        0.0,
        color::border(),
    );
}

/// Columns of the panel's table, after File, which takes what's left.
const PRESET_W: f32 = 150.0;
const DURATION_W: f32 = 64.0;
const SIZE_W: f32 = 72.0;
const PROGRESS_W: f32 = 200.0;
const CHIP_W: f32 = 96.0;
const ROW_H: f32 = 32.0;
/// Narrower than this, the panel drops the Preset, Duration and Size columns.
const NARROW: f32 = 640.0;

/// The Render Queue panel: Cancel All and Clear Finished in its header, then one row per job.
pub fn panel(ui: &mut Ui, rows: &[Row], confirm: bool, focused: bool) -> Option<Action> {
    let mut action = None;
    let close = [(icon::X, tr!("export.close_queue"))];
    let r = Panel::titled(tr!("export.queue"))
        .actions(&close)
        .focused(focused)
        .show(ui, |ui| {
            if rows.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(space::S32);
                    let t = RichText::new(tr!("export.empty_title")).font(theme::title());
                    ui.label(t.color(color::ink()));
                    ui.add_space(space::S4);
                    ui.label(RichText::new(tr!("export.empty_help")).color(color::ink_3()));
                });
                return;
            }
            let wide = ui.available_width() >= NARROW;
            header_row(ui, wide);
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (i, r) in rows.iter().enumerate() {
                        if let Some(a) = row(ui, i, r, wide) {
                            action = Some(a);
                        }
                    }
                });
        });
    if r.action == Some(0) {
        action = Some(Action::Close);
    }
    // The header's actions, right to left from the close button.
    let header = r
        .response
        .rect
        .with_max_y(r.response.rect.top() + size::HEADER);
    let right = header.right() - space::S8 - size::CONTROL - space::S8;
    let painter = ui.painter().clone();
    let text_w = move |s: &str, font| {
        let g = painter.layout_no_wrap(s.into(), font, color::ink());
        g.size().x.ceil()
    };
    let left = header.left() + space::INSET + text_w(tr!("export.queue"), theme::body_strong());
    let left = left + space::S12;
    if right <= left {
        return action;
    }
    let area = Rect::from_x_y_ranges(left..=right, header.y_range());
    let live = rows.iter().filter(|r| r.status.live()).count();
    let on = |b: bool| if b { State::Live } else { State::Disabled };
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(area)
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            ui.set_clip_rect(area.intersect(ui.clip_rect()));
            ui.spacing_mut().item_spacing.x = space::S4;
            // Labels while both fit, else icons named on hover; the question goes first.
            let button_w = |s: &str| 2.0 * space::S12 + text_w(s, theme::body_strong());
            let room = area.width();
            let pair = |a: &str, b: &str| button_w(a) + space::S4 + button_w(b);
            if confirm {
                let (yes, no) = (tr!("export.cancel_all"), tr!("export.keep_exporting"));
                let question = trn!("export.cancel_confirm", live);
                let labels = pair(yes, no) <= room;
                let asks = pair(yes, no) + space::S4 + text_w(&question, theme::body()) <= room;
                // Without room for the question, each button's tooltip asks it.
                let tip = |r: egui::Response, name: &str| {
                    r.on_hover_ui(|ui| {
                        if !asks {
                            ui.label(&question);
                        }
                        ui.label(name);
                    })
                };
                let b = head_button(yes, icon::BAN, true, labels);
                if tip(ui.add(b), yes).clicked() {
                    action = Some(Action::ConfirmCancelAll);
                }
                let b = head_button(no, icon::PLAY, false, labels);
                if tip(ui.add(b), no).clicked() {
                    action = Some(Action::KeepExporting);
                }
                if asks {
                    ui.label(RichText::new(question).color(color::ink()));
                }
                return;
            }
            let (cancel, clear) = (tr!("export.cancel_all"), tr!("export.clear_finished"));
            let labels = pair(cancel, clear) <= room;
            let b = head_button(cancel, icon::BAN, true, labels).state(on(live > 0));
            if ui.add(b).on_hover_text(cancel).clicked() {
                action = Some(Action::CancelAll);
            }
            let b = head_button(clear, icon::CIRCLE_CHECK, false, labels);
            let b = b.state(on(rows.len() > live));
            if ui.add(b).on_hover_text(clear).clicked() {
                action = Some(Action::ClearFinished);
            }
        },
    );
    action
}

/// A header button: `label` when `labels`, else `glyph` alone; Red when `destructive`.
fn head_button<'a>(
    label: &'a str,
    glyph: &'static str,
    destructive: bool,
    labels: bool,
) -> Button<'a> {
    match (labels, destructive) {
        (true, true) => Button::destructive(label),
        (true, false) => Button::ghost(label),
        (false, true) => Button::icon(glyph, label).ink(color::red()),
        (false, false) => Button::icon(glyph, label),
    }
}

/// The cells of a row of `width`, left to right: file, preset, duration, size, progress, chip,
/// Reveal, Cancel. Narrow rows give preset, duration and size no width and split what's left
/// between file and progress.
fn columns(rect: Rect, wide: bool) -> [Rect; 8] {
    let gap = space::S12;
    let mut fixed = [
        if wide { PRESET_W } else { 0.0 },
        if wide { DURATION_W } else { 0.0 },
        if wide { SIZE_W } else { 0.0 },
        PROGRESS_W,
        CHIP_W,
        size::CONTROL,
        size::CONTROL,
    ];
    if !wide {
        fixed[3] = 0.0;
    }
    let used: f32 = fixed
        .iter()
        .map(|w| w + if *w > 0.0 { gap } else { 0.0 })
        .sum();
    let mut file = (rect.width() - used).max(0.0);
    if !wide {
        let rest = (file - gap).max(0.0);
        (file, fixed[3]) = ((rest * 0.45).floor(), (rest * 0.55).ceil());
    }
    let mut x = rect.left();
    let mut out = [Rect::NOTHING; 8];
    for (i, w) in std::iter::once(file).chain(fixed).enumerate() {
        out[i] = Rect::from_x_y_ranges(x..=x + w, rect.y_range());
        if w > 0.0 {
            x += w + gap;
        }
    }
    // Reveal and Cancel sit 4px apart at the right edge.
    out[6] = out[6].translate(vec2(gap - space::S4, 0.0));
    out
}

fn header_row(ui: &mut Ui, wide: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let cols = columns(rect, wide);
    let names = [
        tr!("export.file"),
        tr!("export.preset"),
        tr!("bin.sort_duration"),
        tr!("export.size"),
        tr!("export.progress_column"),
        tr!("export.status"),
    ];
    for (c, name) in cols.iter().zip(names) {
        if c.width() > 0.0 {
            let galley = fit_text(ui, name, theme::body(), color::ink_3(), c.width());
            let at = pos2(c.left(), c.center().y - galley.size().y / 2.0);
            paint_fit(ui, at, galley, color::ink_3());
        }
    }
    rule(ui, rect);
}

/// One job: every state fills the same columns, so nothing moves as it changes.
fn row(ui: &mut Ui, i: usize, r: &Row, wide: bool) -> Option<Action> {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let cols = columns(rect, wide);
    let mut action = None;
    let name = file_name(&r.path);
    cell(ui, cols[0], &name, color::ink());
    let size = r.size.map_or("–".into(), size_text);
    cell(ui, cols[1], &r.preset, color::ink_3());
    cell(ui, cols[2], &clock(seconds(r.duration)), color::ink_3());
    cell(ui, cols[3], &size, color::ink_3());
    match &r.status {
        Status::Rendering { progress, left } => {
            let pct = percent(*progress, *left);
            let font = theme::body();
            let w = ui
                .painter()
                .layout_no_wrap(pct.clone(), font.clone(), color::ink_2())
                .size()
                .x;
            let c = cols[4];
            let at = pos2(c.right(), c.center().y);
            text(ui, at, Align2::RIGHT_CENTER, &pct, font, color::ink_2());
            let bar_w = (c.width() - w - space::S8).max(0.0);
            let b = Rect::from_min_size(pos2(c.left(), c.center().y - 2.0), vec2(bar_w, 4.0));
            bar(ui, b, *progress);
        }
        Status::Failed(e) => cell(ui, cols[4], e, color::red()),
        _ => {}
    }
    chip(ui, cols[5], &r.status);
    if r.status == Status::Done
        && ui
            .put(cols[6], Button::icon(icon::FOLDER, tr!("export.reveal")))
            .clicked()
    {
        action = Some(Action::Reveal(i));
    }
    if r.status.live()
        && ui
            .put(cols[7], Button::icon(icon::X, tr!("prompt.cancel")))
            .clicked()
    {
        action = Some(Action::Cancel(i));
    }
    tip(response, r);
    rule(ui, rect);
    action
}

/// The full path on hover; a failure's error first.
fn tip(response: egui::Response, r: &Row) {
    let path = r.path.display();
    match &r.status {
        Status::Failed(e) => {
            response.on_hover_text(format!("{}\n{path}", tr!("export.failed", error = e)))
        }
        _ => response.on_hover_text(path.to_string()),
    };
}

/// A 1px Border line along `rect`'s bottom.
fn rule(ui: &Ui, rect: Rect) {
    let line = 1.0 / ui.pixels_per_point();
    ui.painter().rect_filled(
        Rect::from_x_y_ranges(rect.x_range(), rect.bottom() - line..=rect.bottom()),
        0.0,
        color::border(),
    );
}

/// A status chip: icon and label on a faint tint of the status colour.
fn chip(ui: &mut Ui, cell: Rect, s: &Status) {
    let (glyph, ink, label) = look(s);
    let font = theme::body();
    let w = ui
        .painter()
        .layout_no_wrap(label.into(), font.clone(), ink)
        .size()
        .x;
    let rect = Rect::from_min_size(
        pos2(cell.left(), cell.center().y - 10.0),
        vec2(space::S4 + 12.0 + space::S4 + w + 6.0, 20.0),
    );
    let tint = match s {
        Status::Done | Status::Failed(_) => 0.14,
        _ => 0.10,
    };
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(radius::SMALL),
        ink.gamma_multiply(tint),
    );
    let icon_font = egui::FontId::new(12.0, theme::icon_font().family);
    let at = pos2(rect.left() + space::S4 + 6.0, rect.center().y);
    text(ui, at, Align2::CENTER_CENTER, glyph, icon_font, ink);
    let at = pos2(rect.left() + space::S4 + 12.0 + space::S4, rect.center().y);
    text(ui, at, Align2::LEFT_CENTER, label, font, ink);
    ui.interact(
        rect,
        ui.id().with(("chip", cell.min.y as i32)),
        Sense::hover(),
    )
    .widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, label));
}

/// A status's icon, colour and label.
fn look(s: &Status) -> (&'static str, Color32, &'static str) {
    match s {
        Status::Waiting => (icon::CLOCK, color::ink_2(), tr!("export.waiting")),
        Status::Rendering { .. } => (icon::LOADER, color::ink(), tr!("export.rendering")),
        Status::Done => (icon::CIRCLE_CHECK, color::green(), tr!("export.done")),
        Status::Failed(_) => (icon::CIRCLE_ALERT, color::red(), tr!("export.failed_short")),
        Status::Cancelled => (icon::BAN, color::ink_3(), tr!("export.cancelled")),
    }
}

/// A neutral progress bar: Ink on Border Strong, never Blue.
pub(crate) fn bar(ui: &Ui, rect: Rect, progress: f32) {
    let corner = CornerRadius::same((rect.height() / 2.0) as u8);
    ui.painter()
        .rect_filled(rect, corner, color::border_strong());
    let done = rect.with_max_x(rect.left() + rect.width() * progress.clamp(0.0, 1.0));
    ui.painter().rect_filled(done, corner, color::ink());
}

/// A truncated label in `rect`, left-aligned and vertically centred; screen readers get it whole.
fn cell(ui: &mut Ui, rect: Rect, s: &str, ink: Color32) {
    if rect.width() <= 0.0 {
        return;
    }
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| ui.add(egui::Label::new(RichText::new(s).color(ink)).truncate()),
    );
}

fn text(ui: &Ui, at: egui::Pos2, align: Align2, s: &str, font: egui::FontId, ink: Color32) {
    ui.painter().text(at, align, s, font, ink);
}

/// "42% · 1:12 Left", or "Estimating…" before there's enough to go on.
fn percent(progress: f32, left: Option<f64>) -> String {
    let pct = format!("{:.0}%", progress * 100.0);
    match left {
        Some(s) => format!("{pct} · {}", tr!("export.left", time = clock(s))),
        None => format!("{pct} · {}", tr!("export.estimating")),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into()
}

fn seconds(t: Time) -> f64 {
    t.0 as f64 / FLICKS_PER_SECOND as f64
}

/// `m:ss`, or `h:mm:ss` from an hour.
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A finished file's size: "186.4 MB".
pub fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        tr!("export.gb", value = format!("{:.1}", b / 1e9))
    } else if b >= 1e6 {
        tr!("export.mb", value = format!("{:.1}", b / 1e6))
    } else {
        tr!("export.kb", value = format!("{:.0}", b / 1e3))
    }
}

/// Opens the file manager on `path`'s folder, selecting it where the platform can.
pub fn reveal(path: &Path) {
    let dir = path.parent().unwrap_or(path);
    let r = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
    } else if cfg!(windows) {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
    } else {
        std::process::Command::new("xdg-open").arg(dir).spawn()
    };
    if let Err(e) = r {
        tracing::warn!("can't open {}: {e}", dir.display());
    }
}

/// Posts a desktop notification.
/// shortcut: shells out to notify-send / osascript and does nothing on Windows; upgrade to a
/// notification crate if Windows users ask.
pub fn notify(title: &str, body: &str) {
    let r = if cfg!(target_os = "macos") {
        let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        let script = format!(
            "display notification {} with title {}",
            quote(body),
            quote(title)
        );
        std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn()
    } else if cfg!(windows) {
        return;
    } else {
        std::process::Command::new("notify-send")
            .args(["--app-name=jvid", title, body])
            .spawn()
    };
    if let Err(e) = r {
        tracing::debug!("notification: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    #[test]
    fn time_left_waits_for_five_percent_then_scales_elapsed() {
        assert_eq!(time_left(10.0, 0.0), None);
        assert_eq!(time_left(10.0, 0.049), None);
        assert_eq!(time_left(30.0, 0.5), Some(30.0));
        assert_eq!(time_left(10.0, 0.25), Some(30.0));
        assert_eq!(time_left(99.0, 1.0), Some(0.0));
    }

    #[test]
    fn clock_and_size_read_like_the_mockup() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(83.4), "1:23");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(size_text(186_400_000), "186.4 MB");
        assert_eq!(size_text(2_300_000_000), "2.3 GB");
        assert_eq!(size_text(52_000), "52 KB");
    }

    fn row(name: &str, status: Status, size: Option<u64>) -> Row {
        Row {
            path: PathBuf::from(format!("/exports/{name}")),
            preset: "YouTube 1080p".into(),
            duration: Time(154 * FLICKS_PER_SECOND),
            size,
            status,
        }
    }

    /// One of every state, oldest first as the queue lists them.
    fn rows() -> Vec<Row> {
        vec![
            row("Intro Cut.mp4", Status::Done, Some(186_400_000)),
            row("Broken Take.mp4", Status::Failed("Disk full".into()), None),
            row("Old Draft.mp4", Status::Cancelled, None),
            row(
                "Main Edit.mp4",
                Status::Rendering {
                    progress: 0.42,
                    left: Some(95.0),
                },
                None,
            ),
            row(
                "Second Edit.mp4",
                Status::Rendering {
                    progress: 0.02,
                    left: None,
                },
                None,
            ),
            row("Outro.mp4", Status::Waiting, None),
        ]
    }

    fn shot(name: &str, size: [f32; 2], mut draw: impl FnMut(&mut Ui) + 'static) {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut fresh = true;
        let mut h = Harness::builder().with_size(size).build_ui(move |ui| {
            if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                draw(ui);
            }
        });
        h.run_steps(2);
        h.snapshot(name);
    }

    #[test]
    fn render_queue_rows() {
        shot("render_queue_rows", [960.0, 280.0], |ui| {
            panel(ui, &rows(), false, true);
        });
    }

    #[test]
    fn render_queue_narrow() {
        shot("render_queue_narrow", [480.0, 280.0], |ui| {
            panel(ui, &rows(), true, true);
        });
    }

    /// Too narrow for their labels, the header's buttons drop to icons rather than clip.
    #[test]
    fn render_queue_header_drops_to_icons() {
        shot("render_queue_icons", [300.0, 200.0], |ui| {
            panel(ui, &rows(), false, true);
        });
    }

    #[test]
    fn render_queue_confirm_drops_its_question_then_its_labels() {
        shot("render_queue_icons_confirm", [300.0, 200.0], |ui| {
            panel(ui, &rows(), true, true);
        });
    }

    /// The question the narrow confirm row drops is in its buttons' tooltips.
    #[test]
    fn narrow_confirm_asks_its_question_on_hover() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut fresh = true;
        let mut h = Harness::builder()
            .with_size([300.0, 200.0])
            .build_ui(move |ui| {
                if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                    panel(ui, &rows(), true, true);
                }
            });
        h.run_steps(2);
        let question = trn!("export.cancel_confirm", 3);
        assert!(h.query_by_label(&question).is_none(), "no room for it");
        h.get_by_label(tr!("export.keep_exporting")).hover();
        for _ in 0..40 {
            h.step();
        }
        assert!(h.query_by_label(&question).is_some(), "not in the tooltip");
    }

    #[test]
    fn render_queue_empty() {
        shot("render_queue_empty", [960.0, 280.0], |ui| {
            panel(ui, &[], false, false);
        });
    }

    #[test]
    fn export_menu_summary() {
        shot("export_menu", [320.0, 360.0], |ui| {
            menu(ui, &rows(), false, "Ctrl+E");
        });
    }

    #[test]
    fn export_menu_confirming_cancel_all() {
        shot("export_menu_confirm", [320.0, 360.0], |ui| {
            menu(ui, &rows(), true, "Ctrl+E");
        });
    }

    #[test]
    fn export_menu_empty() {
        shot("export_menu_empty", [320.0, 200.0], |ui| {
            menu(ui, &[], false, "Ctrl+E");
        });
    }
}
