//! The media bin: Media, Text and Effects tabs; a searchable, sortable grid or list
//! of the project's assets with rename and remove.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{
    Align2, CornerRadius, Key, Rect, Sense, StrokeKind, TextureHandle, Ui, UiBuilder, Vec2, pos2,
    vec2,
};
use jvid_core::history::Command;
use jvid_core::model::{AssetId, MediaAsset, Project, Text};
use jvid_core::time::FLICKS_PER_SECOND;
use jvid_jobs::Pool;
use jvid_media::proxy::Proxies;
use jvid_media::thumbnails::Cache;
use jvid_media::waveforms;
use suite_ui::panel::Panel;
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, radius, size, space};
use suite_ui::widgets::{Button, Dropdown, TextInput};

use crate::i18n::{tr, trn, verbatim};
use crate::preview::Preview;

/// The bin's tabs, in order.
fn tabs() -> [&'static str; 4] {
    [
        tr!("panel.media"),
        tr!("bin.effects"),
        tr!("bin.text"),
        tr!("bin.scopes"),
    ]
}

/// Sort orders by index: added, name, duration.
fn sorts() -> [&'static str; 3] {
    [
        tr!("bin.sort_added"),
        tr!("bin.sort_name"),
        tr!("bin.sort_duration"),
    ]
}
/// Grid tiles shrink to this width before a column drops.
const TILE_MIN: f32 = 120.0;
const ROW: f32 = 24.0;

/// The disk caches and job pool shared by the media bin and the timeline, built once at startup.
pub struct Caches {
    pub thumbs: Arc<Cache>,
    /// Shares the thumbnails' directory, so one eviction keeps both under the cap.
    pub peaks: Arc<waveforms::Cache>,
    pub jobs: Pool,
    pub proxies: Arc<Proxies>,
}

impl Caches {
    pub fn new(s: &jvid_core::settings::Settings) -> Arc<Self> {
        let dir = s
            .cache_dir
            .clone()
            .or_else(|| {
                directories::ProjectDirs::from("", "", "jvid").map(|d| d.cache_dir().to_owned())
            })
            .unwrap_or_else(std::env::temp_dir);
        // shortcut: proxies aren't under the cache cap; evict them too if disk use complains.
        let proxies = Arc::new(Proxies::new(dir.join("proxies")));
        let dir = dir.join("thumbnails");
        let cap = s.cache_max_mb << 20;
        Arc::new(Self {
            thumbs: Arc::new(Cache::new(dir.clone(), cap)),
            peaks: Arc::new(waveforms::Cache::new(dir, cap)),
            jobs: Pool::new(2),
            proxies,
        })
    }
}

#[derive(Default)]
pub struct MediaBin {
    pub tab: usize,
    pub list: bool,
    pub query: String,
    /// Index into [`sorts`].
    pub sort: usize,
    pub selected: Option<AssetId>,
    /// The asset being renamed and its name stem typed so far; the extension stays.
    pub renaming: Option<(AssetId, String)>,
    /// Where the rename field was drawn last frame; a press outside it commits.
    rename_field: Option<Rect>,
    /// The asset whose hover remove button holds keyboard focus, so it stays shown.
    remove_focus: Option<AssetId>,
    /// An in-use asset waiting on the remove warning.
    pub confirm_remove: Option<AssetId>,
    /// A Text preset was clicked (false) or dragged (true) when the project had this many
    /// assets; the next one added is its asset.
    new_text: Option<(usize, bool)>,
    /// Scroll the Media tab to this asset next time it draws.
    reveal: Option<AssetId>,
    /// The Effects tab's search text.
    pub fx_query: String,
    /// The effect last clicked in the Effects tab, by file stem.
    pub fx_selected: Option<String>,
    /// The user effect showing the inline delete confirm.
    pub fx_confirm_delete: Option<String>,
    /// The Effects tab shows tiles instead of rows.
    pub fx_grid: bool,
    /// Video clips are selected for Apply to add to; set by the app each frame.
    pub fx_apply: bool,
    thumbs: Thumbs,
    /// The Text tab's tiles as drawn, by preset; see [`MediaBin::text_thumbs`].
    text_tiles: [Option<egui::TextureId>; 3],
    /// The thumbnail frame generation, project size and presets they were drawn from.
    text_drawn: Option<(u64, (u32, u32), [Text; 3])>,
}

/// What the bin asked the app to do this frame.
#[derive(Default)]
pub struct BinResponse {
    pub import: bool,
    pub command: Option<Command>,
    /// The offline asset the user asked to relink.
    pub relink: Option<AssetId>,
    /// Double-clicked: open in the Source tab.
    pub open: Option<AssetId>,
    /// Add this effect (by file stem) to the selected clips.
    pub apply_effect: Option<String>,
    /// The Effects or Text tab opened, so their thumbnails want a fresh frame.
    pub thumbs: bool,
}

/// A bottom bar like the Media tab's: a wide button, then a `side_w` one at the right.
/// Returns the bar's rects and the rect left above it.
fn bottom_bar(rect: Rect, side_w: f32) -> (Rect, Rect, Rect) {
    let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - size::CONTROL), rect.max);
    let above = rect.with_max_y(bar.top() - space::S8);
    let (wide, side) = bar.split_left_right_at_x(bar.right() - side_w - space::S4);
    (wide, side.with_min_x(side.right() - side_w), above)
}

/// The assets matching `query` (case-insensitive, by name) in `sort` order.
pub fn visible<'a>(assets: &'a [MediaAsset], query: &str, sort: usize) -> Vec<&'a MediaAsset> {
    let q = query.trim().to_lowercase();
    let mut v: Vec<_> = assets
        .iter()
        .filter(|a| a.display_name().to_lowercase().contains(&q))
        .collect();
    match sort {
        1 => v.sort_by_cached_key(|a| a.display_name().to_lowercase()),
        2 => v.sort_by_key(|a| std::cmp::Reverse(a.probe.as_ref().map(|p| p.duration))),
        _ => {}
    }
    v
}

/// `m:ss`, or `h:mm:ss` from an hour, as in the prototype's badges.
pub fn duration_label(asset: &MediaAsset) -> Option<String> {
    let flicks = asset.probe.as_ref()?.duration.0;
    if flicks <= 0 {
        return None;
    }
    let s = flicks / FLICKS_PER_SECOND;
    // Sound effects are often under a second; flooring them read as 0:00.
    if s == 0 {
        let tenths = (flicks * 10 / FLICKS_PER_SECOND).max(1);
        return Some(format!("0:00.{tenths}"));
    }
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    Some(if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    })
}

/// `name` split before its extension: `("clip", ".mkv")`. Dotfiles have no extension.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// PQ or HLG video, which previews tone-mapped.
fn is_hdr(asset: &MediaAsset) -> bool {
    asset.probe.as_ref().is_some_and(|p| p.hdr().is_some())
}

fn is_audio(asset: &MediaAsset) -> bool {
    asset
        .probe
        .as_ref()
        .is_some_and(|p| p.width.is_none() && p.audio_streams > 0)
}

impl MediaBin {
    pub fn new(caches: Arc<Caches>) -> Self {
        let mut bin = Self::default();
        bin.thumbs.cache = Some(caches);
        bin
    }

    /// Redraws the Text tab's tiles while it's open, when the thumbnail frame, project size or
    /// presets (the UI language) changed since they were drawn.
    pub fn text_thumbs(&mut self, preview: &Preview, project: &Project) {
        if self.tab != 2 {
            return;
        }
        let Some((_, generation)) = preview.thumb_frame() else {
            return;
        };
        let s = project.settings();
        let key = (generation, (s.width, s.height), presets().map(|(.., t)| t));
        if self.text_drawn.as_ref() == Some(&key) {
            return;
        }
        for (id, text) in self.text_tiles.iter_mut().zip(&key.2) {
            *id = preview.draw_thumb(*id, |t, d, q| t.text(d, q, text, key.1));
        }
        self.text_drawn = Some(key);
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        focused: bool,
        importing: bool,
        missing: &HashSet<PathBuf>,
    ) -> BinResponse {
        let mut out = BinResponse::default();
        if let Some((n, dragged)) = self.new_text
            && let Some(a) = project.assets().get(n)
        {
            self.new_text = None;
            self.selected = Some(a.id);
            if dragged {
                // Hand the drag over to the new asset so the timeline places it on drop.
                egui::DragAndDrop::set_payload(ui.ctx(), a.id);
            } else {
                (self.tab, self.reveal, self.query) = (0, Some(a.id), String::new());
            }
        }
        // The new tab shows next frame; the click already repaints.
        let mut tab = self.tab;
        Panel::tabbed(&tabs(), &mut tab)
            .wide(true)
            .focused(focused)
            .show(ui, |ui| match self.tab {
                0 => self.media(ui, project, importing, missing, &mut out),
                1 => self.effects_tab(ui, &mut out),
                2 => {
                    if let Some(dragged) = text_presets(ui, &self.text_tiles, &mut out) {
                        self.new_text = Some((project.assets().len(), dragged));
                    }
                }
                _ => crate::scopes::show(ui),
            });
        out.thumbs = tab != self.tab && matches!(tab, 1 | 2);
        self.tab = tab;
        if let Some(id) = self.confirm_remove {
            out.command = out.command.or(self.remove_warning(ui, project, id));
        }
        out
    }

    fn media(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        importing: bool,
        missing: &HashSet<PathBuf>,
        out: &mut BinResponse,
    ) {
        // Import stays pinned below the list, shown even when the bin is empty.
        let solid_label = tr!("bin.add_solid");
        // 88 fits English; a longer translation widens it rather than spilling out.
        let text_w = ui
            .painter()
            .layout_no_wrap(solid_label.into(), theme::body_strong(), color::INK)
            .size()
            .x;
        let solid_w = (2.0 * space::S12 + text_w).ceil().max(88.0);
        let (bar, solid_bar, rect) = bottom_bar(ui.max_rect(), solid_w);
        let import = Button::secondary(tr!("action.import"))
            .with_icon(icon::PLUS)
            .width(bar.width());
        out.import |= ui.put(bar, import).clicked();
        let solid = Button::secondary(solid_label).width(solid_w);
        if ui.put(solid_bar, solid).clicked() {
            out.command = Some(Command::AddSolid {
                name: tr!("bin.solid_name").into(),
                color: [0.0, 0.0, 0.0, 1.0],
            });
        }
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.spacing_mut().item_spacing = vec2(space::S4, space::S8);
            let row = egui::Layout::left_to_right(egui::Align::Min);
            ui.with_layout(row, |ui| {
                let sort_w = 96.0;
                let search_w = rect.width() - sort_w - 2.0 * size::CONTROL - 3.0 * space::S4;
                ui.add(TextInput::search(&mut self.query).width(search_w.max(48.0)));
                ui.add(Dropdown::new(&mut self.sort, &sorts()).width(sort_w));
                view_toggle(ui, &mut self.list);
            });

            let assets = visible(project.assets(), &self.query, self.sort);
            if assets.is_empty() {
                let msg = match (importing, project.assets().is_empty()) {
                    (true, _) => tr!("bin.importing"),
                    (false, true) => tr!("bin.empty"),
                    (false, false) => tr!("bin.no_match"),
                };
                ui.label(egui::RichText::new(msg).color(color::INK_3));
                return;
            }
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    if self.list {
                        for a in &assets {
                            let r = ui.allocate_response(
                                vec2(ui.available_width(), ROW),
                                Sense::click_and_drag(),
                            );
                            self.row(ui, r, a, project, missing.contains(&a.path), out);
                        }
                    } else {
                        let gap = space::S8;
                        let w = ui.available_width();
                        let cols = ((w + gap) / (TILE_MIN + gap)).floor().max(1.0) as usize;
                        let tile_w = (w - gap * (cols as f32 - 1.0)) / cols as f32;
                        let tile = vec2(tile_w, tile_w * 9.0 / 16.0 + ROW);
                        for chunk in assets.chunks(cols) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = gap;
                                for a in chunk {
                                    let r = ui.allocate_response(tile, Sense::click_and_drag());
                                    self.tile(ui, r, a, project, missing.contains(&a.path), out);
                                }
                            });
                        }
                    }
                });
            if importing {
                ui.label(egui::RichText::new(tr!("bin.importing")).color(color::INK_3));
            }
        });
        self.drag_preview(ui, project, missing);
    }

    /// The dragged asset's poster and name, following the pointer above every panel.
    fn drag_preview(&mut self, ui: &Ui, project: &Project, missing: &HashSet<PathBuf>) {
        let Some(id) = egui::DragAndDrop::payload::<AssetId>(ui.ctx()) else {
            return;
        };
        let (Some(a), Some(p)) = (project.asset(*id), ui.ctx().pointer_latest_pos()) else {
            return;
        };
        egui::Area::new(egui::Id::new("bin_drag_preview"))
            .order(egui::Order::Tooltip)
            .fixed_pos(p + vec2(space::S12, space::S12))
            .interactable(false)
            .show(ui.ctx(), |ui| {
                ui.multiply_opacity(0.8);
                let w = 80.0;
                let (thumb, _) = ui.allocate_exact_size(vec2(w, w * 9.0 / 16.0), Sense::hover());
                self.thumbs
                    .paint(ui, thumb, a, missing.contains(&a.path), None);
                let name = Rect::from_min_size(thumb.left_bottom(), vec2(w, ROW));
                ui.allocate_rect(name, Sense::hover());
                self.name(ui, name, a, color::INK);
            });
    }

    fn tile(
        &mut self,
        ui: &mut Ui,
        r: egui::Response,
        a: &MediaAsset,
        project: &Project,
        offline: bool,
        out: &mut BinResponse,
    ) {
        let thumb = Rect::from_min_size(
            r.rect.min,
            vec2(r.rect.width(), r.rect.width() * 9.0 / 16.0),
        );
        let corner = CornerRadius::same(radius::CONTROL);
        let p = ui.painter();
        let skim = ui
            .ctx()
            .pointer_hover_pos()
            .filter(|p| !offline && thumb.contains(*p))
            .map(|p| (p.x - thumb.left()) / thumb.width());
        self.thumbs.paint(ui, thumb, a, offline, skim);
        // Duration bottom right, HDR bottom left, 2px inside the thumbnail.
        let badge = |text: String, left: bool| {
            let galley = p.layout_no_wrap(text, theme::small(), color::INK);
            let size = galley.size() + vec2(space::S8, space::S4);
            let x = if left {
                thumb.left() + 2.0
            } else {
                thumb.right() - 2.0 - size.x
            };
            let badge = Rect::from_min_size(pos2(x, thumb.bottom() - 2.0 - size.y), size);
            p.rect_filled(
                badge,
                CornerRadius::same(radius::SMALL),
                color::GROUND.gamma_multiply(0.8),
            );
            p.galley(
                badge.min + vec2(space::S4, space::S4 / 2.0),
                galley,
                color::INK,
            );
        };
        if let Some(d) = duration_label(a) {
            badge(d, false);
        }
        if is_hdr(a) {
            badge(verbatim("HDR").into(), true);
        }
        let show_remove = self.shows_remove(ui, &r, a);
        if show_remove {
            let btn = Rect::from_min_size(
                pos2(
                    thumb.right() - size::CONTROL - space::S4,
                    thumb.top() + space::S4,
                ),
                vec2(size::CONTROL, size::CONTROL),
            );
            p.rect_filled(
                btn,
                CornerRadius::same(radius::CONTROL),
                color::GROUND.gamma_multiply(0.8),
            );
        }
        let selected = self.selected == Some(a.id);
        if selected {
            p.rect_stroke(
                thumb,
                corner,
                egui::Stroke::new(2.0, color::LIGHT_BLUE),
                StrokeKind::Inside,
            );
        } else if r.hovered() {
            p.rect_stroke(
                thumb,
                corner,
                egui::Stroke::new(1.0, color::BORDER_STRONG),
                StrokeKind::Inside,
            );
        }
        if offline {
            p.text(
                thumb.center() + vec2(0.0, space::S12),
                Align2::CENTER_TOP,
                tr!("bin.offline"),
                theme::small(),
                color::INK_2,
            );
        }
        let name = Rect::from_x_y_ranges(r.rect.x_range(), thumb.bottom()..=r.rect.bottom());
        self.name(ui, name, a, color::INK_2);
        if show_remove {
            let btn = pos2(
                thumb.right() - size::CONTROL - space::S4,
                thumb.top() + space::S4,
            );
            self.remove_button(ui, btn, a, project, out);
        }
        self.interact(r, a, project, offline, out);
    }

    fn row(
        &mut self,
        ui: &mut Ui,
        r: egui::Response,
        a: &MediaAsset,
        project: &Project,
        offline: bool,
        out: &mut BinResponse,
    ) {
        let p = ui.painter();
        let corner = CornerRadius::same(radius::CONTROL);
        if self.selected == Some(a.id) {
            p.rect_filled(r.rect, corner, color::BLUE.gamma_multiply(0.2));
        } else if r.hovered() {
            p.rect_filled(r.rect, corner, color::RAISED);
        }
        // The type badge: one of the clip colours, the only place they show in the bin.
        let kind = if is_audio(a) {
            color::CLIP_GREEN
        } else {
            color::CLIP_VIOLET
        };
        // Inset like the search field's text, so the dot clears the panel edge.
        let badge = Rect::from_min_size(
            pos2(r.rect.left() + space::S8, r.rect.center().y - 4.0),
            vec2(8.0, 8.0),
        );
        p.rect_filled(badge, CornerRadius::same(2), kind);
        let thumb = Rect::from_min_size(
            pos2(badge.right() + space::S8, r.rect.top() + 2.0),
            vec2((ROW - 4.0) * 16.0 / 9.0, ROW - 4.0),
        );
        self.thumbs.paint(ui, thumb, a, offline, None);
        let show_remove = self.shows_remove(ui, &r, a);
        let mut right = r.rect.right() - space::S8;
        if show_remove {
            right = r.rect.right() - size::CONTROL - space::S4;
        }
        let hdr = is_hdr(a).then(|| verbatim("HDR").to_string());
        for d in duration_label(a).into_iter().chain(hdr) {
            let g = p.layout_no_wrap(d, theme::body(), color::INK_3);
            right -= g.size().x;
            p.galley(
                pos2(right, r.rect.center().y - g.size().y / 2.0),
                g,
                color::INK_3,
            );
            right -= space::S8;
        }
        let name = Rect::from_x_y_ranges(thumb.right() + space::S8..=right, r.rect.y_range());
        self.name(ui, name, a, color::INK);
        if show_remove {
            let btn = pos2(r.rect.right() - size::CONTROL, r.rect.top());
            self.remove_button(ui, btn, a, project, out);
        }
        self.interact(r, a, project, offline, out);
    }

    /// Hovering or focusing a tile or row, or its remove button, shows the button.
    fn shows_remove(&self, ui: &Ui, r: &egui::Response, a: &MediaAsset) -> bool {
        let renaming = self.renaming.as_ref().is_some_and(|(id, _)| *id == a.id);
        !renaming
            && (ui.rect_contains_pointer(r.rect)
                || r.has_focus()
                || self.remove_focus == Some(a.id))
    }

    fn remove_button(
        &mut self,
        ui: &mut Ui,
        min: egui::Pos2,
        a: &MediaAsset,
        project: &Project,
        out: &mut BinResponse,
    ) {
        let rect = Rect::from_min_size(min, vec2(size::CONTROL, size::CONTROL));
        // A stable id keeps keyboard focus on the button across frames.
        let b = ui
            .scope_builder(
                UiBuilder::new().max_rect(rect).id_salt(("remove", a.id)),
                |ui| ui.add(Button::icon(icon::X, tr!("bin.remove"))),
            )
            .inner;
        if b.has_focus() {
            self.remove_focus = Some(a.id);
        } else if self.remove_focus == Some(a.id) {
            self.remove_focus = None;
        }
        if b.clicked() {
            out.command = self.remove(project, a.id);
        }
    }

    /// The asset's name, or the rename field while it is being renamed.
    fn name(&mut self, ui: &mut Ui, rect: Rect, a: &MediaAsset, ink: egui::Color32) {
        if let Some((id, text)) = &mut self.renaming
            && *id == a.id
        {
            let name = a.display_name();
            let ext = split_ext(&name).1;
            let g = ui
                .painter()
                .layout_no_wrap(ext.to_owned(), theme::body(), color::INK_3);
            let w = (rect.width() - g.size().x - space::S4).max(0.0);
            let field = Rect::from_min_size(
                pos2(rect.left(), rect.center().y - size::CONTROL / 2.0),
                vec2(w, size::CONTROL),
            );
            self.rename_field = Some(field);
            let r = ui.put(field, TextInput::new(text).width(w));
            r.request_focus();
            let pos = pos2(
                field.right() + space::S4,
                rect.center().y - g.size().y / 2.0,
            );
            ui.painter().galley(pos, g, color::INK_3);
            return;
        }
        let mut job = egui::text::LayoutJob::single_section(
            a.display_name(),
            egui::TextFormat::simple(theme::body(), ink),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width().max(0.0));
        let galley = ui.painter().layout_job(job);
        let pos = pos2(rect.left(), rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(pos, galley, ink);
    }

    fn interact(
        &mut self,
        r: egui::Response,
        a: &MediaAsset,
        project: &Project,
        offline: bool,
        out: &mut BinResponse,
    ) {
        if offline {
            r.clone()
                .on_hover_text(tr!("bin.offline_path", path = a.path.display()));
        }
        if self.reveal == Some(a.id) {
            r.scroll_to_me(None);
            self.reveal = None;
        }
        if r.clicked() {
            self.selected = Some(a.id);
            self.renaming = self.renaming.take().filter(|(id, _)| *id == a.id);
        }
        let name = a.display_name();
        let (stem, ext) = split_ext(&name);
        if r.double_clicked() {
            out.open = Some(a.id);
        }
        // The timeline takes it on drop.
        r.dnd_set_drag_payload(a.id);
        r.context_menu(|ui| {
            self.selected = Some(a.id);
            if offline && ui.button(tr!("bin.relink")).clicked() {
                out.relink = Some(a.id);
            }
            if ui.button(tr!("bin.rename")).clicked() {
                self.renaming = Some((a.id, stem.to_owned()));
            }
            if ui.button(tr!("bin.remove")).clicked() {
                out.command = self.remove(project, a.id);
            }
        });
        if let Some((id, text)) = &self.renaming
            && *id == a.id
        {
            let (enter, esc) = r.ctx.input(|i| {
                // Clicking away commits, like Enter.
                let away = i.pointer.any_pressed()
                    && i.pointer
                        .press_origin()
                        .is_some_and(|p| !self.rename_field.is_some_and(|f| f.contains(p)));
                (
                    i.key_pressed(Key::Enter) || away,
                    i.key_pressed(Key::Escape),
                )
            });
            if enter {
                out.command = Some(Command::RenameAsset {
                    id: *id,
                    name: Some(format!("{text}{ext}")),
                });
            }
            if enter || esc {
                self.renaming = None;
            }
        }
    }

    /// Removes an unused asset at once; an in-use one waits on the warning.
    fn remove(&mut self, project: &Project, id: AssetId) -> Option<Command> {
        if project.clips_using(id) > 0 {
            self.confirm_remove = Some(id);
            return None;
        }
        self.selected = self.selected.filter(|s| *s != id);
        Some(Command::RemoveAsset(id))
    }

    fn remove_warning(&mut self, ui: &mut Ui, project: &Project, id: AssetId) -> Option<Command> {
        let Some(asset) = project.asset(id) else {
            self.confirm_remove = None;
            return None;
        };
        let n = project.clips_using(id);
        let mut command = None;
        let modal = egui::Modal::new(egui::Id::new("remove_asset"))
            .frame(
                egui::Frame::popup(ui.style())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(space::S16),
            )
            .show(ui.ctx(), |ui| {
                ui.set_width(320.0);
                ui.label(
                    egui::RichText::new(tr!("bin.remove_title", name = asset.display_name()))
                        .font(theme::headline()),
                );
                ui.add_space(space::S8);
                ui.label(trn!("bin.remove_body", n));
                ui.add_space(space::S16);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = space::S8;
                    if ui
                        .add(Button::destructive(tr!("bin.remove_button")))
                        .clicked()
                    {
                        command = Some(Command::RemoveAsset(id));
                        ui.close();
                    }
                    if ui.add(Button::secondary(tr!("prompt.cancel"))).clicked() {
                        ui.close();
                    }
                });
            });
        if modal.should_close() {
            self.confirm_remove = None;
            if command.is_some() {
                self.selected = self.selected.filter(|s| *s != id);
            }
        }
        command
    }
}

/// Results finished off-thread, per path, waiting for the UI thread.
type Done<T> = Arc<Mutex<Vec<(PathBuf, T)>>>;

/// Poster thumbnails, made on a job pool and kept on disk by `jvid_media::thumbnails`.
#[derive(Default)]
struct Thumbs {
    /// `None` in tests, which then load nothing.
    cache: Option<Arc<Caches>>,
    /// Posters finished off-thread, waiting to become textures.
    done: Done<egui::ColorImage>,
    /// `None` while a poster is in flight or failed; the placeholder shows.
    textures: HashMap<PathBuf, Option<TextureHandle>>,
    /// Filmstrips, loaded on first hover and skimmed from memory after.
    strip_done: Done<Vec<egui::ColorImage>>,
    strips: HashMap<PathBuf, Option<Vec<TextureHandle>>>,
}

/// The filmstrip frame under the cursor, `frac` of the way across a tile, of `n` frames.
fn skim_index(frac: f32, n: usize) -> usize {
    ((frac.clamp(0.0, 1.0) * n as f32) as usize).min(n.saturating_sub(1))
}

fn color_image(img: &image::RgbaImage) -> egui::ColorImage {
    let size = [img.width() as usize, img.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw())
}

impl Thumbs {
    /// The tile's picture: the cached poster, or a neutral placeholder until it's ready.
    /// `skim` is the hover position across the tile, 0 to 1, which swaps in a filmstrip frame.
    fn paint(&mut self, ui: &Ui, rect: Rect, asset: &MediaAsset, offline: bool, skim: Option<f32>) {
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(radius::CONTROL), color::RAISED);
        if offline {
            p.text(
                rect.center(),
                Align2::CENTER_CENTER,
                icon::CIRCLE_ALERT,
                theme::icon_font(),
                color::RED,
            );
            return;
        }
        if is_audio(asset) {
            p.text(
                rect.center(),
                Align2::CENTER_CENTER,
                icon::AUDIO,
                theme::icon_font(),
                color::INK_3,
            );
            return;
        }
        if asset.sequence.is_some() {
            // A sequence: a little timeline of three tracks, shrunk to fit a list row.
            let (w, h) = (
                (rect.width() - space::S8).min(56.0),
                (rect.height() / 6.0).min(space::S4),
            );
            let left = rect.center().x - w / 2.0;
            let rows = [
                [0.0, 0.55, 0.62, 1.0],
                [0.18, 0.8, 0.8, 0.8],
                [0.0, 0.4, 0.46, 0.9],
            ];
            for (i, [a, b, c, d]) in rows.into_iter().enumerate() {
                let y = rect.center().y + (i as f32 - 1.0) * 2.0 * h - h / 2.0;
                for (from, to) in [(a, b), (c, d)].into_iter().filter(|(f, t)| t > f) {
                    let bar = Rect::from_x_y_ranges(left + from * w..=left + to * w, y..=y + h);
                    p.rect_filled(bar, CornerRadius::same(1), color::INK_3);
                }
            }
            return;
        }
        let Some(probe) = &asset.probe else { return };
        for (path, img) in self.done.lock().unwrap().drain(..) {
            let tex = ui
                .ctx()
                .load_texture(path.display().to_string(), img, Default::default());
            self.textures.insert(path, Some(tex));
        }
        for (path, imgs) in self.strip_done.lock().unwrap().drain(..) {
            let texs = imgs
                .into_iter()
                .enumerate()
                .map(|(i, img)| {
                    let name = format!("{}#{i}", path.display());
                    ui.ctx().load_texture(name, img, Default::default())
                })
                .collect();
            self.strips.insert(path, Some(texs));
        }
        if let Some(frac) = skim {
            match self.strips.get(&asset.path) {
                Some(Some(texs)) if !texs.is_empty() => {
                    letterbox(ui, rect, &texs[skim_index(frac, texs.len())]);
                    return;
                }
                Some(_) => {}
                None => {
                    self.strips.insert(asset.path.clone(), None);
                    let (done, ctx) = (self.strip_done.clone(), ui.ctx().clone());
                    let Some(c) = &self.cache else { return };
                    let (cache, jobs) = (c.thumbs.clone(), &c.jobs);
                    let (path, duration) = (asset.path.clone(), probe.duration);
                    let key = format!("strip:{}", path.display());
                    jobs.submit(key, jvid_jobs::Priority::Preview, move |c| {
                        if c.is_cancelled() {
                            return;
                        }
                        match cache.filmstrip(&path, duration) {
                            Ok(v) => {
                                done.lock()
                                    .unwrap()
                                    .push((path, v.iter().map(color_image).collect()));
                                ctx.request_repaint();
                            }
                            Err(e) => tracing::warn!("filmstrip for {}: {e}", path.display()),
                        }
                    });
                }
            }
        }
        match self.textures.get(&asset.path) {
            Some(Some(tex)) => letterbox(ui, rect, tex),
            Some(None) => {}
            None => {
                self.textures.insert(asset.path.clone(), None);
                let (done, ctx, path) = (self.done.clone(), ui.ctx().clone(), asset.path.clone());
                let Some(c) = &self.cache else { return };
                c.thumbs
                    .submit_poster(&c.jobs, path.clone(), probe.duration, move |r| match r {
                        Ok(img) => {
                            done.lock().unwrap().push((path, color_image(&img)));
                            ctx.request_repaint();
                        }
                        Err(e) => tracing::warn!("thumbnail for {}: {e}", path.display()),
                    });
            }
        }
    }
}

/// Paints `tex` letterboxed inside the 16:9 tile.
fn letterbox(ui: &Ui, rect: Rect, tex: &TextureHandle) {
    let t = tex.size_vec2();
    let r = Rect::from_center_size(rect.center(), t * (rect.size() / t).min_elem());
    egui::Image::new(tex)
        .corner_radius(radius::CONTROL)
        .paint_at(ui, r);
}

/// The Text tab's presets in `Text::presets` order: name, one line on what it makes, and
/// its style with the text a new one starts with.
fn presets() -> [(&'static str, &'static str, Text); 3] {
    let shown = [
        (
            tr!("bin.title"),
            tr!("bin.title_about"),
            tr!("bin.title_text"),
        ),
        (
            tr!("bin.lower_third"),
            tr!("bin.lower_third_about"),
            tr!("bin.lower_third_text"),
        ),
        (
            tr!("bin.caption"),
            tr!("bin.caption_about"),
            tr!("bin.caption_text"),
        ),
    ];
    let mut out = Text::presets().map(|(_, t)| ("", "", t));
    for ((name, about, text), (n, a, content)) in out.iter_mut().zip(shown) {
        (*name, *about, text.content) = (n, a, content.into());
    }
    out
}

/// The Text tab: two columns of tiles, each its preset drawn on the thumbnail frame (`tiles`,
/// black until drawn) over its name and what it makes, adding a text asset to the bin when
/// clicked or dragged out. Returns whether the press was a drag, when one was.
fn text_presets(
    ui: &mut Ui,
    tiles: &[Option<egui::TextureId>; 3],
    out: &mut BinResponse,
) -> Option<bool> {
    let mut added = None;
    let w = ((ui.available_width() - space::S8) / 2.0).floor();
    let inner = w - 2.0 * space::S4;
    let thumb_h = inner * 9.0 / 16.0;
    let p = ui.painter().clone();
    let laid: Vec<_> = presets()
        .into_iter()
        .zip(tiles)
        .map(|((name, about, text), tex)| {
            let n = p.layout(name.into(), theme::body_strong(), color::INK, inner);
            let a = p.layout(about.into(), theme::body(), color::INK_3, inner);
            (name, text, tex, n, a)
        })
        .collect();
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(space::S8, space::S8);
            for row in laid.chunks(2) {
                // Both tiles in a row as tall as the taller one's two text lines.
                let text_h = row
                    .iter()
                    .map(|(.., n, a)| n.size().y + a.size().y)
                    .fold(0.0, f32::max);
                let h = space::S4 + thumb_h + space::S4 + text_h + space::S8;
                ui.horizontal(|ui| {
                    for (name, text, tex, n, a) in row {
                        let (rect, r) = ui.allocate_exact_size(vec2(w, h), Sense::click_and_drag());
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, *name)
                        });
                        let p = ui.painter();
                        if r.hovered() {
                            p.rect_filled(rect, CornerRadius::same(radius::CONTROL), color::RAISED);
                        }
                        let thumb = Rect::from_min_size(
                            rect.min + vec2(space::S4, space::S4),
                            vec2(inner, thumb_h),
                        );
                        match tex {
                            Some(id) => {
                                egui::Image::new((*id, thumb.size()))
                                    .corner_radius(radius::SMALL)
                                    .paint_at(ui, thumb);
                            }
                            None => {
                                let corner = CornerRadius::same(radius::SMALL);
                                p.rect_filled(thumb, corner, egui::Color32::BLACK);
                            }
                        }
                        let at = thumb.left_bottom() + vec2(0.0, space::S4);
                        p.galley(at, n.clone(), color::INK);
                        p.galley(at + vec2(0.0, n.size().y), a.clone(), color::INK_3);
                        if r.has_focus() {
                            p.add(suite_ui::widgets::focus_ring(rect, radius::CONTROL));
                        }
                        let dragged = r.drag_started();
                        if dragged || r.clicked() {
                            out.command = Some(Command::AddText {
                                name: (*name).into(),
                                text: text.clone(),
                            });
                            added = Some(dragged);
                        }
                    }
                });
            }
        });
    added
}

/// An Effects tab list row: padding, the 80x45 thumbnail, and its name over two lines.
const FX_ROW: f32 = 2.0 * space::S4 + 50.0;
const FX_THUMB: egui::Vec2 = vec2(80.0, 45.0);

/// The Media tab's Grid View and List View toggle, shared by the Effects tab.
fn view_toggle(ui: &mut Ui, list: &mut bool) {
    if ui
        .add(Button::icon(icon::GRID, tr!("bin.grid_view")).on(!*list))
        .clicked()
    {
        *list = false;
    }
    if ui
        .add(Button::icon(icon::LIST, tr!("bin.list_view")).on(*list))
        .clicked()
    {
        *list = true;
    }
}

/// `w` at `rect` over what's already laid out, leaving the layout cursor where it was.
fn put_over(ui: &mut Ui, rect: Rect, w: impl egui::Widget) -> egui::Response {
    ui.new_child(UiBuilder::new().max_rect(rect)).add(w)
}

/// An effect's name in Body Strong, then Error or Disabled when it is, on one line `width` wide.
fn fx_name(ui: &Ui, row: &crate::effects::Row, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    let ink = if row.disabled {
        color::INK_3
    } else {
        color::INK
    };
    job.append(
        &row.name,
        0.0,
        egui::TextFormat::simple(theme::body_strong(), ink),
    );
    let tag = match (&row.error, row.disabled) {
        (Some(_), _) => Some((tr!("bin.fx_error"), color::RED)),
        (None, true) => Some((tr!("bin.fx_disabled"), color::INK_3)),
        _ => None,
    };
    if let Some((tag, ink)) = tag {
        job.append(tag, 6.0, egui::TextFormat::simple(theme::body(), ink));
    }
    job.wrap.max_width = width;
    job.wrap.max_rows = 1;
    ui.fonts_mut(|f| f.layout_job(job))
}

impl MediaBin {
    /// The Effects tab: the effects folder's files, Bundled then My Effects, each its effect on
    /// the thumbnail frame with what it does or why it doesn't load, applied by dragging onto a
    /// clip or to the selected clips by double-click or Apply.
    fn effects_tab(&mut self, ui: &mut Ui, out: &mut BinResponse) {
        let dir = crate::effects::dir();
        let reveal_w = 136.0;
        let (bar, reveal_bar, rect) = bottom_bar(ui.max_rect(), reveal_w);
        let new = Button::secondary(tr!("bin.new_effect"))
            .with_icon(icon::PLUS)
            .width(bar.width());
        if ui.put(bar, new).clicked()
            && let Some(dir) = &dir
        {
            match crate::effects::create(dir) {
                Ok(path) => crate::export::queue::reveal(&path),
                Err(e) => tracing::warn!("effects: can't create a new effect: {e}"),
            }
        }
        let open = Button::secondary(tr!("bin.open_effects")).width(reveal_w);
        if ui.put(reveal_bar, open).clicked()
            && let Some(dir) = &dir
        {
            // `reveal` opens a file's folder; any child of the folder works.
            crate::export::queue::reveal(&dir.join("effect.wgsl"));
        }
        let rows = crate::effects::with(|lib| lib.rows(&self.fx_query));
        let empty_folder =
            rows.is_empty() && crate::effects::with(|l| l.entries().next().is_none());
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.spacing_mut().item_spacing = vec2(space::S4, space::S8);
            ui.horizontal(|ui| {
                let search_w = rect.width() - 2.0 * size::CONTROL - 2.0 * space::S4;
                ui.add(TextInput::search(&mut self.fx_query).width(search_w.max(48.0)));
                let mut list = !self.fx_grid;
                view_toggle(ui, &mut list);
                self.fx_grid = !list;
            });
            if rows.is_empty() {
                let hint = if empty_folder {
                    tr!("bin.fx_empty")
                } else {
                    tr!("bin.fx_no_match")
                };
                ui.label(egui::RichText::new(hint).color(color::INK_3));
                return;
            }
            let (bundled, mine): (Vec<_>, Vec<_>) = rows.iter().partition(|r| r.bundled);
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let groups = [(tr!("bin.fx_bundled"), bundled), (tr!("bin.fx_mine"), mine)];
                    for (i, (title, rows)) in groups.into_iter().enumerate() {
                        if rows.is_empty() {
                            continue;
                        }
                        if i > 0 {
                            ui.add_space(space::S8);
                        }
                        let (head, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), size::CONTROL),
                            Sense::hover(),
                        );
                        ui.painter().text(
                            head.left_center(),
                            Align2::LEFT_CENTER,
                            title,
                            theme::body_strong(),
                            color::INK_3,
                        );
                        if self.fx_grid {
                            self.effect_tiles(ui, &rows, dir.as_deref(), out);
                        } else {
                            for row in rows {
                                self.effect_row(ui, row, dir.as_deref(), out);
                            }
                        }
                    }
                    ui.add_space(space::S12);
                    ui.label(
                        egui::RichText::new(tr!("bin.fx_hint"))
                            .small()
                            .color(color::INK_3),
                    );
                });
        });
    }

    /// A list row in fixed columns: thumbnail, name over two lines of description, then Apply
    /// and More. Hovering changes only the fill.
    fn effect_row(
        &mut self,
        ui: &mut Ui,
        row: &crate::effects::Row,
        dir: Option<&std::path::Path>,
        out: &mut BinResponse,
    ) {
        let w = ui.available_width();
        let (rect, r) = ui.allocate_exact_size(vec2(w, FX_ROW), Sense::click_and_drag());
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &row.name));
        self.fx_fill(ui, rect, row);
        let inset = rect.shrink(space::S4);
        let thumb = Rect::from_min_size(
            pos2(inset.left(), inset.center().y - FX_THUMB.y / 2.0),
            FX_THUMB,
        );
        crate::effects::paint_thumb(ui, thumb, row);
        let confirming = self.fx_confirm_delete.as_deref() == Some(row.stem.as_str());
        let button_w = if confirming { 64.0 } else { size::CONTROL };
        let buttons = 2.0 * button_w + space::S4;
        let text = Rect::from_x_y_ranges(
            thumb.right() + space::S8..=inset.right() - buttons - space::S8,
            inset.y_range(),
        );
        let name = fx_name(ui, row, text.width());
        let about = if confirming {
            let msg = tr!(
                "bin.fx_delete_confirm",
                file = format_args!("{}.wgsl", row.stem)
            );
            let mut job =
                egui::text::LayoutJob::simple(msg, theme::body(), color::RED, text.width());
            job.wrap.max_rows = 2;
            job
        } else {
            let ink = if row.error.is_some() {
                color::RED
            } else {
                color::INK_3
            };
            crate::effects::about(row, text.width(), 2, ink)
        };
        let about = ui.fonts_mut(|f| f.layout_job(about));
        let top = text.center().y - (name.size().y + 2.0 + 2.0 * 16.0) / 2.0;
        let p = ui.painter();
        p.galley(pos2(text.left(), top), name.clone(), color::INK);
        let below = pos2(text.left(), top + name.size().y + 2.0);
        p.galley(below, about.clone(), color::INK_3);
        // A compiler message can run long; descriptions are clamped to two lines, as drawn.
        let r = match &row.error {
            Some(e) if about.elided && !confirming => r.on_hover_text(e),
            _ => r,
        };
        self.fx_interact(ui, &r, row, dir, out);
        let mut right = inset.right();
        let mut column = |ui: &mut Ui, b: Button| {
            let at = Rect::from_min_size(
                pos2(right - button_w, rect.center().y - size::CONTROL / 2.0),
                vec2(button_w, size::CONTROL),
            );
            right -= button_w + space::S4;
            put_over(ui, at, b.width(button_w))
        };
        if confirming {
            if column(ui, Button::destructive(tr!("bin.fx_delete"))).clicked()
                && let Some(dir) = dir
                && let Err(e) = crate::effects::delete(dir, &row.stem)
            {
                tracing::warn!("effects: can't delete {}: {e}", row.stem);
            }
            if column(ui, Button::ghost(tr!("prompt.cancel"))).clicked()
                || ui.input(|i| i.key_pressed(Key::Escape))
            {
                self.fx_confirm_delete = None;
            }
            return;
        }
        let more = column(ui, Button::icon(icon::ELLIPSIS, tr!("bin.fx_more")));
        egui::Popup::menu(&more).show(|ui| self.fx_menu(ui, row, dir, out));
        let apply =
            Button::icon(icon::PLUS, tr!("bin.fx_apply_selected")).state(self.apply_state(row));
        if column(ui, apply).clicked() {
            out.apply_effect = Some(row.stem.clone());
        }
    }

    /// Grid tiles: the thumbnail across the tile over the name, with Apply in its corner on hover.
    fn effect_tiles(
        &mut self,
        ui: &mut Ui,
        rows: &[&crate::effects::Row],
        dir: Option<&std::path::Path>,
        out: &mut BinResponse,
    ) {
        let gap = space::S8;
        let w = ui.available_width();
        let cols = ((w + gap) / (TILE_MIN + gap)).floor().max(1.0) as usize;
        let tile_w = (w - gap * (cols as f32 - 1.0)) / cols as f32;
        let thumb_w = tile_w - 2.0 * space::S4;
        let tile = vec2(tile_w, space::S4 + thumb_w * 9.0 / 16.0 + 6.0 + ROW);
        for chunk in rows.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for row in chunk {
                    let (rect, r) = ui.allocate_exact_size(tile, Sense::click_and_drag());
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &row.name)
                    });
                    self.fx_fill(ui, rect, row);
                    let thumb = Rect::from_min_size(
                        rect.min + vec2(space::S4, space::S4),
                        vec2(thumb_w, thumb_w * 9.0 / 16.0),
                    );
                    crate::effects::paint_thumb(ui, thumb, row);
                    let name = fx_name(ui, row, thumb_w);
                    let at = pos2(thumb.left(), thumb.bottom() + 6.0);
                    ui.painter().galley(at, name, color::INK);
                    // Tiles show no description, so it's the tooltip.
                    let ink = if row.error.is_some() {
                        color::RED
                    } else {
                        color::INK
                    };
                    let r = r.on_hover_ui(|ui| {
                        ui.label(crate::effects::about(row, 320.0, 8, ink));
                    });
                    self.fx_interact(ui, &r, row, dir, out);
                    if ui.rect_contains_pointer(rect) || r.has_focus() {
                        let at = Rect::from_min_size(
                            thumb.right_top() + vec2(-space::S4 - size::CONTROL, space::S4),
                            Vec2::splat(size::CONTROL),
                        );
                        ui.painter().rect_filled(at, radius::CONTROL, color::PANEL);
                        let apply = Button::icon(icon::PLUS, tr!("bin.fx_apply_selected"))
                            .state(self.apply_state(row));
                        if put_over(ui, at, apply).clicked() {
                            out.apply_effect = Some(row.stem.clone());
                        }
                    }
                }
            });
            ui.add_space(space::S4);
        }
    }

    /// Selected or hovered (by the pointer over `rect`, so its buttons don't clear it), the fill.
    fn fx_fill(&self, ui: &Ui, rect: Rect, row: &crate::effects::Row) {
        let selected = self.fx_selected.as_deref() == Some(row.stem.as_str());
        let fill = if selected {
            color::BLUE.gamma_multiply(0.2)
        } else if ui.rect_contains_pointer(rect) {
            color::RAISED
        } else {
            return;
        };
        ui.painter().rect_filled(rect, radius::CONTROL, fill);
    }

    /// Apply works on a usable effect with video clips selected.
    fn apply_state(&self, row: &crate::effects::Row) -> suite_ui::widgets::State {
        if self.fx_apply && row.error.is_none() && !row.disabled {
            suite_ui::widgets::State::Live
        } else {
            suite_ui::widgets::State::Disabled
        }
    }

    /// A row's or tile's click selects, double-click applies, drag goes onto a clip, and
    /// right-click opens its menu.
    fn fx_interact(
        &mut self,
        ui: &Ui,
        r: &egui::Response,
        row: &crate::effects::Row,
        dir: Option<&std::path::Path>,
        out: &mut BinResponse,
    ) {
        if r.has_focus() {
            ui.painter()
                .add(suite_ui::widgets::focus_ring(r.rect, radius::CONTROL));
        }
        if r.clicked() {
            self.fx_selected = Some(row.stem.clone());
        }
        if row.error.is_none() {
            if r.double_clicked() {
                out.apply_effect = Some(row.stem.clone());
            }
            // The timeline adds it to the clip it's dropped on.
            r.dnd_set_drag_payload(crate::effects::Drag(row.stem.clone()));
        }
        r.context_menu(|ui| self.fx_menu(ui, row, dir, out));
    }

    /// A row's menu, from right-click or More.
    fn fx_menu(
        &mut self,
        ui: &mut Ui,
        row: &crate::effects::Row,
        dir: Option<&std::path::Path>,
        out: &mut BinResponse,
    ) {
        self.fx_selected = Some(row.stem.clone());
        if row.error.is_none() && ui.button(tr!("bin.fx_apply_selected")).clicked() {
            out.apply_effect = Some(row.stem.clone());
        }
        let Some(dir) = dir else { return };
        if ui.button(tr!("bin.fx_reveal")).clicked() {
            crate::export::queue::reveal(&dir.join(format!("{}.wgsl", row.stem)));
        }
        if ui.button(tr!("bin.fx_duplicate")).clicked()
            && let Err(e) = crate::effects::duplicate(dir, &row.stem)
        {
            tracing::warn!("effects: can't duplicate {}: {e}", row.stem);
        }
        let toggle = if row.disabled {
            tr!("bin.fx_enable")
        } else {
            tr!("bin.fx_disable")
        };
        if ui.button(toggle).clicked()
            && let Err(e) = crate::effects::set_disabled(dir, &row.stem, !row.disabled)
        {
            tracing::warn!("effects: can't change {}: {e}", row.stem);
        }
        if !row.bundled && ui.button(tr!("bin.fx_delete")).clicked() {
            self.fx_confirm_delete = Some(row.stem.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use jvid_core::model::ProbeInfo;
    use jvid_core::time::Time;

    use super::*;

    fn asset(id: u64, path: &str, secs: i64) -> MediaAsset {
        MediaAsset {
            id: AssetId(id),
            path: PathBuf::from(path),
            probe: Some(ProbeInfo {
                duration: Time(secs * FLICKS_PER_SECOND),
                ..Default::default()
            }),
            name: None,
            in_point: None,
            out_point: None,
            solid: None,
            text: None,
            sequence: None,
        }
    }

    fn names(v: Vec<&MediaAsset>) -> Vec<String> {
        v.iter().map(|a| a.display_name()).collect()
    }

    #[test]
    fn filter_and_sort() {
        let mut renamed = asset(3, "c.mkv", 30);
        renamed.name = Some("Alpha".into());
        let all = [
            asset(1, "/x/clutch.mkv", 47),
            asset(2, "Bravo.mp4", 7500),
            renamed,
        ];
        assert_eq!(
            names(visible(&all, "", 0)),
            ["clutch.mkv", "Bravo.mp4", "Alpha"]
        );
        assert_eq!(
            names(visible(&all, "", 1)),
            ["Alpha", "Bravo.mp4", "clutch.mkv"]
        );
        assert_eq!(
            names(visible(&all, "", 2)),
            ["Bravo.mp4", "clutch.mkv", "Alpha"]
        );
        assert_eq!(names(visible(&all, " ALP ", 0)), ["Alpha"]);
        // Searches the shown name, not the path.
        assert!(visible(&all, "x/", 0).is_empty());
    }

    #[test]
    fn remove_warns_only_when_in_use() {
        use jvid_core::history::History;
        use jvid_core::model::{NewClip, TrackKind};
        let mut h = History::new(Project::default());
        h.apply(Command::AddAsset("a.mkv".into())).unwrap();
        h.apply(Command::AddAsset("b.mkv".into())).unwrap();
        let track = match h.apply(Command::AddTrack {
            kind: TrackKind::Video,
            name: "V1".into(),
        }) {
            Ok(jvid_core::history::Created::Track(t)) => t,
            r => panic!("{r:?}"),
        };
        let clip = NewClip {
            asset: AssetId(1),
            source_in: Time(0),
            source_out: Time(10),
            start: Time(0),
            link: None,
        };
        h.apply(Command::AddClip { track, clip }).unwrap();

        let mut bin = MediaBin {
            selected: Some(AssetId(2)),
            ..Default::default()
        };
        assert!(bin.remove(h.project(), AssetId(1)).is_none());
        assert_eq!(bin.confirm_remove, Some(AssetId(1)));
        assert!(matches!(
            bin.remove(h.project(), AssetId(2)),
            Some(Command::RemoveAsset(AssetId(2)))
        ));
        assert_eq!(bin.selected, None);
    }

    #[test]
    fn duration_labels() {
        assert_eq!(duration_label(&asset(1, "a", 47)).unwrap(), "0:47");
        assert_eq!(duration_label(&asset(1, "a", 7471)).unwrap(), "2:04:31");
        assert_eq!(duration_label(&asset(1, "a", 0)), None);
        let mut short = asset(1, "a", 0);
        short.probe.as_mut().unwrap().duration = Time(FLICKS_PER_SECOND * 4 / 10);
        assert_eq!(duration_label(&short).unwrap(), "0:00.4");
    }

    #[test]
    fn effects_bar_matches_the_media_bar() {
        let panel = Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 400.0));
        let (import, solid, above) = bottom_bar(panel, 88.0);
        let (new, reveal, fx_above) = bottom_bar(panel, 136.0);
        assert_eq!(above, fx_above);
        assert_eq!((import.min, solid.max), (new.min, reveal.max));
        assert_eq!(import.height(), size::CONTROL);
        assert_eq!(new.height(), reveal.height());
        assert_eq!(reveal.left() - new.right(), solid.left() - import.right());
    }

    #[test]
    fn split_ext_keeps_the_extension() {
        assert_eq!(split_ext("clip.v2.mkv"), ("clip.v2", ".mkv"));
        assert_eq!(split_ext("noext"), ("noext", ""));
        assert_eq!(split_ext(".hidden"), (".hidden", ""));
    }

    #[test]
    fn skim_maps_cursor_x_to_frame() {
        assert_eq!(skim_index(0.0, 10), 0);
        assert_eq!(skim_index(0.55, 10), 5);
        assert_eq!(skim_index(1.0, 10), 9);
        assert_eq!(skim_index(-0.2, 10), 0);
        assert_eq!(skim_index(3.0, 10), 9);
        assert_eq!(skim_index(0.5, 0), 0);
        // Frame i of a 10 s clip's strip sits at i seconds.
        let clip = asset(1, "a", 10);
        assert_eq!(
            skim_index(0.35, 10) as i64 * FLICKS_PER_SECOND,
            3 * FLICKS_PER_SECOND
        );
        assert!(!is_audio(&clip));
    }

    #[test]
    fn skimming_a_cached_strip_decodes_nothing() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let d = std::env::temp_dir().join(format!("jvid-skim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let clip = d.join("clip.mp4");
        std::fs::write(&clip, [0]).unwrap();
        let grabs = Arc::new(AtomicUsize::new(0));
        let g = grabs.clone();
        let cache = Arc::new(Cache::with_grab(
            d.join("cache"),
            u64::MAX,
            Box::new(move |_, _, _| {
                g.fetch_add(1, Ordering::SeqCst);
                Ok(image::RgbaImage::new(jvid_media::thumbnails::WIDTH, 90))
            }),
        ));
        let a = asset(1, clip.to_str().unwrap(), 10);
        let dur = a.probe.as_ref().unwrap().duration;
        // Warm the disk cache, as an earlier session would have.
        cache.poster(&clip, dur).unwrap();
        cache.filmstrip(&clip, dur).unwrap();
        grabs.store(0, Ordering::SeqCst);

        let mut thumbs = Thumbs {
            cache: Some(Arc::new(Caches {
                thumbs: cache,
                peaks: Arc::new(waveforms::Cache::new(d.join("cache"), u64::MAX)),
                jobs: Pool::new(1),
                proxies: Arc::new(Proxies::new(d.join("proxies"))),
            })),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(160.0, 90.0));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        for i in 0.. {
            let mut out = ctx.run_ui(Default::default(), |ui| {
                thumbs.paint(ui, rect, &a, false, Some((i % 20) as f32 / 20.0));
            });
            out.textures_delta.clear();
            let strip = matches!(thumbs.strips.get(&clip), Some(Some(v)) if v.len() == 10);
            if (strip && i > 50) || std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(matches!(thumbs.strips.get(&clip), Some(Some(v)) if v.len() == 10));
        assert_eq!(grabs.load(Ordering::SeqCst), 0);
    }

    /// The reported 0:00 WAV: probe truncated durations and the label floored them.
    #[test]
    fn corpus_wav_has_a_duration_label() {
        let Some(dir) = jvid_media::test_media::corpus() else {
            return;
        };
        jvid_media::ffmpeg::init().unwrap();
        let probe = jvid_media::probe::probe(&dir.join("tone-1khz.wav")).unwrap();
        let mut a = asset(1, "tone-1khz.wav", 0);
        a.probe = Some(probe);
        assert!(is_audio(&a));
        assert_eq!(duration_label(&a).unwrap(), "0:05");
    }
}
