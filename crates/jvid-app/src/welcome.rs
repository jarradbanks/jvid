//! The start screen: new or open a project, pick a preset, reopen a recent one, or drop media.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use egui::{
    Align2, CornerRadius, Frame, Key, KeyboardShortcut, Modifiers, Rect, Sense, Stroke, StrokeKind,
    TextureHandle, Ui, WidgetInfo, WidgetType, vec2,
};
use jvid_core::model::{ProbeInfo, Project, ProjectSettings, StreamInfo};
use jvid_core::time::{Rate, Time};
use jvid_jobs::Pool;
use jvid_media::thumbnails::Cache;
use serde::{Deserialize, Serialize};
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, radius, size, space};
use suite_ui::widgets::{Button, Segmented, State};

use crate::i18n::{Label, tr, trn, verbatim};

/// Label and size; `None` is Match source.
pub const PRESETS: [(Label, Option<(u32, u32)>); 6] = [
    (|| tr!("size.match_source"), None),
    (|| verbatim("16:9 · 1080p"), Some((1920, 1080))),
    (|| verbatim("16:9 · 1440p"), Some((2560, 1440))),
    (|| verbatim("16:9 · 4K"), Some((3840, 2160))),
    (|| tr!("size.vertical"), Some((1080, 1920))),
    (|| tr!("size.square"), Some((1080, 1080))),
];
const FPS: [u32; 2] = [30, 60];
const RECENT_MAX: usize = 8;

/// A project opened or saved before, kept in eframe storage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recent {
    pub path: PathBuf,
    /// Seconds since the Unix epoch.
    pub opened: u64,
    /// The first video asset and its duration, for the poster.
    pub poster: Option<(PathBuf, Time)>,
}

impl Recent {
    pub fn of(path: &Path, project: &Project) -> Self {
        let poster = project.assets().iter().find_map(|a| {
            let p = a.probe.as_ref()?;
            p.frame_rate?;
            Some((a.path.clone(), p.duration))
        });
        Self {
            path: path.to_owned(),
            opened: now(),
            poster,
        }
    }
}

/// Moves `entry` to the front, dropping its old place and the oldest past the limit.
pub fn remember(list: &mut Vec<Recent>, entry: Recent) {
    list.retain(|r| r.path != entry.path);
    list.insert(0, entry);
    list.truncate(RECENT_MAX);
}

pub enum Action {
    /// A new project; `None` settings is Match source.
    New(Option<ProjectSettings>),
    Open,
    OpenRecent(PathBuf),
    Import,
}

#[derive(Default)]
pub struct Welcome {
    pub preset: usize,
    pub fps: usize,
    pub recent: Vec<Recent>,
    posters: Posters,
}

impl Welcome {
    pub fn new(recent: Vec<Recent>) -> Self {
        Self {
            recent,
            ..Default::default()
        }
    }

    /// The selected preset, or `None` for Match source.
    pub fn settings(&self) -> Option<ProjectSettings> {
        let (width, height) = PRESETS[self.preset].1?;
        Some(ProjectSettings {
            width,
            height,
            frame_rate: Rate::new([30, 60][self.fps], 1),
            ..Default::default()
        })
    }

    pub fn show(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        if ui.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::N)))
        {
            action = Some(Action::New(self.settings()));
        }
        if ui.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::O)))
        {
            action = Some(Action::Open);
        }

        egui::Panel::bottom("welcome_footer")
            .exact_size(size::HEADER)
            .frame(Frame::NONE.fill(color::GROUND))
            .show_separator_line(false)
            .show(ui, |ui| {
                let r = ui
                    .max_rect()
                    .shrink2(vec2(space::GUTTER + space::INSET, 0.0));
                let p = ui.painter();
                let version = concat!("v", env!("CARGO_PKG_VERSION"));
                p.text(
                    r.left_center(),
                    Align2::LEFT_CENTER,
                    version,
                    theme::small(),
                    color::INK_3,
                );
                let (new, open) = if cfg!(target_os = "macos") {
                    (verbatim("⌘N"), verbatim("⌘O"))
                } else {
                    (verbatim("Ctrl+N"), verbatim("Ctrl+O"))
                };
                let hint = tr!("welcome.hint", new = new, open = open);
                p.text(
                    r.right_center(),
                    Align2::RIGHT_CENTER,
                    hint,
                    theme::small(),
                    color::INK_3,
                );
            });

        let gutter = space::GUTTER as i8;
        let outer = Frame::NONE.fill(color::GROUND).inner_margin(egui::Margin {
            left: gutter,
            right: gutter,
            top: gutter,
            bottom: 0,
        });
        egui::Panel::right("welcome_recent")
            .exact_size(320.0)
            .resizable(false)
            .frame(outer)
            .show_separator_line(false)
            .show(ui, |ui| {
                card_frame().show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    if let Some(p) = self.recent_list(ui) {
                        action = Some(Action::OpenRecent(p));
                    }
                });
            });
        egui::CentralPanel::default()
            .frame(outer.inner_margin(egui::Margin {
                left: gutter,
                right: 0,
                top: gutter,
                bottom: 0,
            }))
            .show(ui, |ui| {
                card_frame().show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    if let Some(a) = self.start(ui) {
                        action = Some(a);
                    }
                });
            });
        action
    }

    /// The centre column: wordmark, buttons, presets, frame rate and the drop zone.
    fn start(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        const W: f32 = 3.0 * 160.0 + 2.0 * space::S8;
        const H: f32 = 28.0
            + 28.0
            + space::S24
            + 2.0 * 96.0
            + space::S8
            + space::S16
            + 24.0
            + space::S16
            + 120.0;
        let area = Rect::from_center_size(ui.max_rect().center(), vec2(W, H));
        let mut col = ui.new_child(egui::UiBuilder::new().max_rect(area));
        let ui = &mut col;
        ui.spacing_mut().item_spacing = vec2(space::S8, space::S8);

        let (head, _) = ui.allocate_exact_size(vec2(W, 28.0), Sense::hover());
        ui.painter().text(
            head.center(),
            Align2::CENTER_CENTER,
            verbatim("jvid"),
            theme::display(),
            color::INK,
        );

        ui.add_space(space::S8);
        ui.horizontal(|ui| {
            // Each fills half the preset grid, edges aligned to it.
            let half = (W - space::S8) / 2.0;
            if ui
                .add(
                    Button::primary(tr!("menu.new_project"))
                        .width(half)
                        .height(40.0),
                )
                .clicked()
            {
                action = Some(Action::New(self.settings()));
            }
            if ui
                .add(
                    Button::secondary(tr!("welcome.open_project"))
                        .width(half)
                        .height(40.0),
                )
                .clicked()
            {
                action = Some(Action::Open);
            }
        });
        ui.add_space(space::S16);

        for row in PRESETS.chunks(3).enumerate() {
            ui.horizontal(|ui| {
                for (j, (label, dims)) in row.1.iter().enumerate() {
                    let i = row.0 * 3 + j;
                    if preset_card(ui, label(), *dims, self.preset == i).clicked() {
                        self.preset = i;
                    }
                }
            });
        }

        ui.add_space(space::S8);
        ui.horizontal(|ui| {
            ui.add_space((W - 120.0) / 2.0);
            let state = if self.preset == 0 {
                State::Disabled
            } else {
                State::Live
            };
            let fps = FPS.map(|rate| tr!("unit.fps", rate = rate));
            ui.add(Segmented::new(&mut self.fps, &fps.each_ref().map(String::as_str)).state(state))
                .on_disabled_hover_text(tr!("welcome.match_source_rate"));
        });
        ui.add_space(space::S8);

        let (zone, r) = ui.allocate_exact_size(vec2(W, 120.0), Sense::click());
        let hovered = r.hovered() || ui.input(|i| !i.raw.hovered_files.is_empty());
        dashed_rect(
            ui,
            zone,
            radius::DIALOG as f32,
            if hovered {
                color::LIGHT_BLUE
            } else {
                color::BORDER_STRONG
            },
        );
        let p = ui.painter();
        p.text(
            zone.center() - vec2(0.0, 12.0),
            Align2::CENTER_CENTER,
            icon::PLUS,
            theme::icon_font(),
            color::INK_2,
        );
        p.text(
            zone.center() + vec2(0.0, 12.0),
            Align2::CENTER_CENTER,
            tr!("welcome.drop"),
            theme::body(),
            color::INK_2,
        );
        let r = r.on_hover_text(tr!("welcome.drop_tooltip"));
        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, tr!("action.import")));
        if r.clicked() {
            action = Some(Action::Import);
        }
        action
    }

    fn recent_list(&mut self, ui: &mut Ui) -> Option<PathBuf> {
        let mut open = None;
        let r = ui.max_rect().shrink(space::INSET);
        ui.painter().text(
            r.left_top(),
            Align2::LEFT_TOP,
            tr!("welcome.recent"),
            theme::title(),
            color::INK,
        );
        let mut y = r.top() + 20.0 + space::S12;
        if self.recent.is_empty() {
            // Wraps for languages longer than the column; English fits on one line.
            let text = tr!("welcome.recent_empty").to_owned();
            let galley = ui
                .painter()
                .layout(text, theme::body(), color::INK_3, r.width());
            ui.painter()
                .galley(egui::pos2(r.left(), y), galley, color::INK_3);
            return None;
        }
        let now = now();
        for entry in &self.recent {
            let row =
                Rect::from_min_size(egui::pos2(r.left() - 4.0, y), vec2(r.width() + 8.0, 56.0));
            if row.bottom() > r.bottom() {
                break;
            }
            let name = entry
                .path
                .file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
            let resp = ui.interact(row, ui.id().with(&entry.path), Sense::click());
            resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &name));
            let p = ui.painter();
            if resp.hovered() {
                p.rect_filled(row, CornerRadius::same(radius::CONTROL), color::RAISED);
            }
            let thumb =
                Rect::from_min_size(row.min + vec2(4.0, 4.0), vec2(48.0 * 16.0 / 9.0, 48.0));
            self.posters.paint(ui, thumb, entry.poster.as_ref());
            let x = thumb.right() + space::S12;
            let p = ui.painter();
            p.text(
                egui::pos2(x, row.center().y - 2.0),
                Align2::LEFT_BOTTOM,
                &name,
                theme::body_strong(),
                color::INK,
            );
            p.text(
                egui::pos2(x, row.center().y + 2.0),
                Align2::LEFT_TOP,
                relative_date(now, entry.opened),
                theme::body(),
                color::INK_3,
            );
            let resp = resp.on_hover_text(entry.path.display().to_string());
            if resp.clicked() {
                open = Some(entry.path.clone());
            }
            y = row.bottom() + space::S4;
        }
        open
    }
}

fn card_frame() -> Frame {
    Frame::NONE
        .fill(color::PANEL)
        .stroke(Stroke::new(1.0, color::BORDER))
        .corner_radius(radius::PANEL)
        .inner_margin(space::S12 as i8)
}

/// A 160×96 card with the frame's shape outlined above its label.
fn preset_card(
    ui: &mut Ui,
    label: &str,
    dims: Option<(u32, u32)>,
    selected: bool,
) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(160.0, 96.0), Sense::click());
    r.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, label));
    let p = ui.painter();
    let corner = CornerRadius::same(radius::DIALOG);
    let fill = if r.hovered() {
        color::RAISED
    } else {
        color::PANEL
    };
    let (stroke, width) = if selected {
        (color::LIGHT_BLUE, 2.0)
    } else if r.hovered() {
        (color::BORDER_STRONG, 1.0)
    } else {
        (color::BORDER, 1.0)
    };
    p.rect(
        rect,
        corner,
        fill,
        Stroke::new(width, stroke),
        StrokeKind::Inside,
    );
    let centre = rect.center() - vec2(0.0, 10.0);
    match dims {
        Some((w, h)) => {
            let s = 36.0 / w.max(h) as f32;
            let frame = Rect::from_center_size(centre, vec2(w as f32 * s, h as f32 * s));
            p.rect_stroke(
                frame,
                2.0,
                Stroke::new(1.5, color::INK_2),
                StrokeKind::Inside,
            );
        }
        None => {
            // Match source: a dashed frame, its shape still to come.
            dashed_rect(
                ui,
                Rect::from_center_size(centre, vec2(36.0, 24.0)),
                2.0,
                color::INK_2,
            );
        }
    }
    ui.painter().text(
        rect.center_bottom() - vec2(0.0, space::S16),
        Align2::CENTER_BOTTOM,
        label,
        theme::body(),
        if selected { color::INK } else { color::INK_2 },
    );
    if r.has_focus() {
        ui.painter()
            .add(suite_ui::widgets::focus_ring(rect, radius::DIALOG));
    }
    r
}

/// A dashed outline of `r` whose corners follow `rad`.
fn dashed_rect(ui: &Ui, r: Rect, rad: f32, c: egui::Color32) {
    use std::f32::consts::FRAC_PI_2;
    let rad = rad.min(r.width() / 2.0).min(r.height() / 2.0);
    let corners = [
        (r.right_top() + vec2(-rad, rad), -FRAC_PI_2),
        (r.right_bottom() + vec2(-rad, -rad), 0.0),
        (r.left_bottom() + vec2(rad, -rad), FRAC_PI_2),
        (r.left_top() + vec2(rad, rad), 2.0 * FRAC_PI_2),
    ];
    let mut pts: Vec<_> = corners
        .iter()
        .flat_map(|&(o, a0)| {
            (0..=8).map(move |k| {
                let a = a0 + FRAC_PI_2 * k as f32 / 8.0;
                o + rad * vec2(a.cos(), a.sin())
            })
        })
        .collect();
    pts.push(pts[0]);
    ui.painter().extend(egui::Shape::dashed_line(
        &pts,
        Stroke::new(1.0, c),
        6.0,
        4.0,
    ));
}

/// Resolution and frame rate of the first video in `batch`, upright, for Match source.
pub fn match_source(batch: &[(PathBuf, ProbeInfo)]) -> Option<(u32, u32, Rate)> {
    batch.iter().find_map(|(_, p)| {
        let (w, h, rate) = (p.width?, p.height?, p.frame_rate?);
        let turned = p
            .streams
            .iter()
            .any(|s| matches!(s, StreamInfo::Video(v) if v.rotation % 180 == 90));
        Some(if turned { (h, w, rate) } else { (w, h, rate) })
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// "Just now", "5 minutes ago", "2 hours ago", "Yesterday", "Oct 12", or "Oct 12, 2025".
/// shortcut: days split at UTC midnight; use local time if a date crate joins the stack.
pub fn relative_date(now: u64, then: u64) -> String {
    let ago = now.saturating_sub(then);
    let (today, day) = (civil(now / 86_400), civil(then / 86_400));
    match ago {
        0..60 => tr!("date.just_now").into(),
        60..3600 => trn!("date.minutes_ago", ago / 60),
        _ if now / 86_400 == then / 86_400 => trn!("date.hours_ago", ago / 3600),
        _ if now / 86_400 == then / 86_400 + 1 => tr!("date.yesterday").into(),
        _ => {
            let month = [
                tr!("date.jan"),
                tr!("date.feb"),
                tr!("date.mar"),
                tr!("date.apr"),
                tr!("date.may"),
                tr!("date.jun"),
                tr!("date.jul"),
                tr!("date.aug"),
                tr!("date.sep"),
                tr!("date.oct"),
                tr!("date.nov"),
                tr!("date.dec"),
            ][day.1 as usize - 1];
            if day.0 == today.0 {
                tr!("date.this_year", month = month, day = day.2)
            } else {
                tr!("date.other_year", month = month, day = day.2, year = day.0)
            }
        }
    }
}

/// (year, month, day) of a day count since 1970-01-01; Howard Hinnant's `civil_from_days`.
fn civil(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Recent-project posters through the bin's on-disk thumbnail cache.
// shortcut: mirrors media_bin's `Thumbs`; share one loader once #20's bin work lands.
#[derive(Default)]
struct Posters {
    cache: Option<(Arc<Cache>, Pool)>,
    done: Arc<Mutex<Vec<(PathBuf, egui::ColorImage)>>>,
    textures: HashMap<PathBuf, Option<TextureHandle>>,
}

impl Posters {
    fn paint(&mut self, ui: &Ui, rect: Rect, poster: Option<&(PathBuf, Time)>) {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::CONTROL), color::RAISED);
        let Some((path, duration)) = poster else {
            return;
        };
        for (p, img) in self.done.lock().unwrap().drain(..) {
            let tex = ui
                .ctx()
                .load_texture(p.display().to_string(), img, Default::default());
            self.textures.insert(p, Some(tex));
        }
        match self.textures.get(path) {
            Some(Some(tex)) => {
                let t = tex.size_vec2();
                let r = Rect::from_center_size(rect.center(), t * (rect.size() / t).min_elem());
                egui::Image::new(tex)
                    .corner_radius(radius::CONTROL)
                    .paint_at(ui, r);
            }
            Some(None) => {}
            None => {
                self.textures.insert(path.clone(), None);
                let (cache, jobs) = self.cache.get_or_insert_with(|| {
                    let s = crate::settings_file::load();
                    let dir = s
                        .cache_dir
                        .or_else(|| {
                            directories::ProjectDirs::from("", "", "jvid")
                                .map(|d| d.cache_dir().to_owned())
                        })
                        .unwrap_or_else(std::env::temp_dir)
                        .join("thumbnails");
                    (
                        Arc::new(Cache::new(dir, s.cache_max_mb << 20)),
                        Pool::new(1),
                    )
                });
                let (done, ctx, path) = (self.done.clone(), ui.ctx().clone(), path.clone());
                cache.submit_poster(jobs, path.clone(), *duration, move |r| match r {
                    Ok(img) => {
                        let size = [img.width() as usize, img.height() as usize];
                        let img = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                        done.lock().unwrap().push((path, img));
                        ctx.request_repaint();
                    }
                    Err(e) => tracing::warn!("poster for {}: {e}", path.display()),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::model::VideoInfo;

    #[test]
    fn match_source_uses_first_video_upright() {
        let probe = |w, h, fps, rotation| ProbeInfo {
            width: Some(w),
            height: Some(h),
            frame_rate: Some(Rate::new(fps, 1)),
            streams: vec![StreamInfo::Video(VideoInfo {
                rotation,
                ..Default::default()
            })],
            ..Default::default()
        };
        let audio = ProbeInfo {
            audio_streams: 1,
            ..Default::default()
        };
        let batch = vec![
            ("a.wav".into(), audio),
            ("s.mp4".into(), probe(2560, 1440, 60, 0)),
            ("v.mp4".into(), probe(1080, 1920, 30, 0)),
        ];
        assert_eq!(match_source(&batch), Some((2560, 1440, Rate::new(60, 1))));
        // A phone clip stored landscape with a 90° turn is vertical.
        let phone = vec![("p.mp4".into(), probe(1920, 1080, 30, 270))];
        assert_eq!(match_source(&phone), Some((1080, 1920, Rate::new(30, 1))));
        assert_eq!(match_source(&batch[..1]), None);
    }

    #[test]
    fn relative_dates() {
        // 2026-10-17 15:00 UTC.
        let now = 1_792_249_200;
        assert_eq!(civil(now / 86_400), (2026, 10, 17));
        assert_eq!(relative_date(now, now - 5), "Just now");
        assert_eq!(relative_date(now, now - 300), "5 minutes ago");
        assert_eq!(relative_date(now, now - 2 * 3600), "2 hours ago");
        assert_eq!(relative_date(now, now - 20 * 3600), "Yesterday");
        assert_eq!(relative_date(now, now - 5 * 86_400), "Oct 12");
        assert_eq!(relative_date(now, now - 300 * 86_400), "Dec 21, 2025");
    }

    #[test]
    fn remember_moves_to_front_and_caps() {
        let r = |n: u32| Recent {
            path: format!("/p/{n}.jvid").into(),
            opened: n.into(),
            poster: None,
        };
        let mut list: Vec<_> = (0..RECENT_MAX as u32).map(r).collect();
        remember(&mut list, r(3));
        assert_eq!(list[0], r(3));
        assert_eq!(list.len(), RECENT_MAX);
        remember(&mut list, r(99));
        assert_eq!((list[0].opened, list.len()), (99, RECENT_MAX));
    }
}
