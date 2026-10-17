//! The media bin: Media, Text and Effects tabs; a searchable, sortable grid or list
//! of the project's assets with rename and remove.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{
    Align2, CornerRadius, Key, Rect, Sense, StrokeKind, TextureHandle, Ui, UiBuilder, pos2, vec2,
};
use jvid_core::history::Command;
use jvid_core::model::{AssetId, MediaAsset, Project};
use jvid_core::time::FLICKS_PER_SECOND;
use jvid_jobs::Pool;
use jvid_media::thumbnails::Cache;
use jvid_media::waveforms;
use suite_ui::panel::Panel;
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, radius, size, space};
use suite_ui::widgets::{Button, Dropdown, TextInput};

pub const TABS: [&str; 3] = ["Media", "Effects", "Text"];
pub const SORTS: [&str; 3] = ["Added", "Name", "Duration"];
/// Grid tiles shrink to this width before a column drops.
const TILE_MIN: f32 = 120.0;
const ROW: f32 = 24.0;

/// The disk caches and job pool shared by the media bin and the timeline, built once at startup.
pub struct Caches {
    pub thumbs: Arc<Cache>,
    /// Shares the thumbnails' directory, so one eviction keeps both under the cap.
    pub peaks: Arc<waveforms::Cache>,
    pub jobs: Pool,
}

impl Caches {
    pub fn new(s: &jvid_core::settings::Settings) -> Arc<Self> {
        let dir = s
            .cache_dir
            .clone()
            .or_else(|| {
                directories::ProjectDirs::from("", "", "jvid").map(|d| d.cache_dir().to_owned())
            })
            .unwrap_or_else(std::env::temp_dir)
            .join("thumbnails");
        let cap = s.cache_max_mb << 20;
        Arc::new(Self {
            thumbs: Arc::new(Cache::new(dir.clone(), cap)),
            peaks: Arc::new(waveforms::Cache::new(dir, cap)),
            jobs: Pool::new(2),
        })
    }
}

#[derive(Default)]
pub struct MediaBin {
    pub tab: usize,
    pub list: bool,
    pub query: String,
    /// Index into [`SORTS`].
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
    thumbs: Thumbs,
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
}

/// The assets matching `query` (case-insensitive, by name) in `sort` order.
pub fn visible<'a>(assets: &'a [MediaAsset], query: &str, sort: usize) -> Vec<&'a MediaAsset> {
    let q = query.trim().to_lowercase();
    let mut v: Vec<_> = assets
        .iter()
        .filter(|a| a.display_name().to_lowercase().contains(&q))
        .collect();
    match SORTS[sort] {
        "Name" => v.sort_by_cached_key(|a| a.display_name().to_lowercase()),
        "Duration" => v.sort_by_key(|a| std::cmp::Reverse(a.probe.as_ref().map(|p| p.duration))),
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

    pub fn show(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        focused: bool,
        importing: bool,
        missing: &HashSet<PathBuf>,
    ) -> BinResponse {
        let mut out = BinResponse::default();
        // The new tab shows next frame; the click already repaints.
        let mut tab = self.tab;
        Panel::tabbed(&TABS, &mut tab)
            .focused(focused)
            .show(ui, |ui| match self.tab {
                0 => self.media(ui, project, importing, missing, &mut out),
                1 => placeholder(ui, "Effects arrive in a later version"),
                _ => placeholder(ui, "Text and titles arrive in a later version"),
            });
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
        let mut rect = ui.max_rect();
        // Import stays pinned below the list, shown even when the bin is empty.
        let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - size::CONTROL), rect.max);
        rect.max.y = bar.top() - space::S8;
        let solid_w = 88.0;
        let (bar, solid_bar) = bar.split_left_right_at_x(bar.right() - solid_w - space::S4);
        let import = Button::secondary("Import media")
            .with_icon(icon::PLUS)
            .width(bar.width());
        out.import |= ui.put(bar, import).clicked();
        let solid = Button::secondary("New solid").width(solid_w);
        if ui
            .put(solid_bar.with_min_x(solid_bar.right() - solid_w), solid)
            .clicked()
        {
            out.command = Some(Command::AddSolid {
                name: "Solid".into(),
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
                ui.add(Dropdown::new(&mut self.sort, &SORTS).width(sort_w));
                if ui
                    .add(Button::icon(icon::GRID, "Grid view").on(!self.list))
                    .clicked()
                {
                    self.list = false;
                }
                if ui
                    .add(Button::icon(icon::LIST, "List view").on(self.list))
                    .clicked()
                {
                    self.list = true;
                }
            });

            let assets = visible(project.assets(), &self.query, self.sort);
            if assets.is_empty() {
                let msg = match (importing, project.assets().is_empty()) {
                    (true, _) => "Importing…",
                    (false, true) => "Drop media here or click Import media",
                    (false, false) => "No media matches the search",
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
                ui.label(egui::RichText::new("Importing…").color(color::INK_3));
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
        if let Some(d) = duration_label(a) {
            let galley = p.layout_no_wrap(d, theme::small(), color::INK);
            let badge = Rect::from_min_size(
                thumb.right_bottom() - galley.size() - vec2(space::S8 + 2.0, space::S4 + 2.0),
                galley.size() + vec2(space::S8, space::S4),
            );
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
                "Media offline",
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
        if let Some(d) = duration_label(a) {
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
                |ui| ui.add(Button::icon(icon::X, "Remove from project")),
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
                .on_hover_text(format!("Media offline: {}", a.path.display()));
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
            if offline && ui.button("Relink…").clicked() {
                out.relink = Some(a.id);
            }
            if ui.button("Rename").clicked() {
                self.renaming = Some((a.id, stem.to_owned()));
            }
            if ui.button("Remove from project").clicked() {
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
            ui.label(egui::RichText::new(format!("Remove “{}”?", asset.display_name())).font(theme::headline()));
            ui.add_space(space::S8);
            let clips = if n == 1 { "1 clip" } else { &format!("{n} clips") };
            ui.label(format!("It's used by {clips} in the timeline. Removing it removes them too. You can undo this."));
            ui.add_space(space::S16);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = space::S8;
                if ui.add(Button::destructive("Remove")).clicked() {
                    command = Some(Command::RemoveAsset(id));
                    ui.close();
                }
                if ui.add(Button::secondary("Cancel")).clicked() {
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

fn placeholder(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(color::INK_3));
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
