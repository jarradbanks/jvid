//! The media bin: Media, Text and Effects tabs; a searchable, sortable grid or list
//! of the project's assets with rename and remove.

use std::collections::HashMap;
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
use suite_ui::panel::{Panel, PanelResponse};
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, radius, size, space};
use suite_ui::widgets::{Button, Dropdown, TextInput};

pub const TABS: [&str; 3] = ["Media", "Effects", "Text"];
pub const SORTS: [&str; 3] = ["Added", "Name", "Duration"];
/// Grid tiles shrink to this width before a column drops.
const TILE_MIN: f32 = 120.0;
const ROW: f32 = 24.0;

#[derive(Default)]
pub struct MediaBin {
    pub tab: usize,
    pub list: bool,
    pub query: String,
    /// Index into [`SORTS`].
    pub sort: usize,
    pub selected: Option<AssetId>,
    /// The asset being renamed and the text typed so far.
    pub renaming: Option<(AssetId, String)>,
    /// An in-use asset waiting on the remove warning.
    pub confirm_remove: Option<AssetId>,
    thumbs: Thumbs,
}

/// What the bin asked the app to do this frame.
#[derive(Default)]
pub struct BinResponse {
    pub import: bool,
    pub command: Option<Command>,
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
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    Some(if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    })
}

fn is_audio(asset: &MediaAsset) -> bool {
    asset
        .probe
        .as_ref()
        .is_some_and(|p| p.width.is_none() && p.audio_streams > 0)
}

impl MediaBin {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        focused: bool,
        importing: bool,
    ) -> BinResponse {
        let mut out = BinResponse::default();
        // The new tab shows next frame; the click already repaints.
        let mut tab = self.tab;
        let PanelResponse { action, .. } = Panel::tabbed(&TABS, &mut tab)
            .actions(&[(icon::PLUS, "Import media")])
            .focused(focused)
            .show(ui, |ui| match self.tab {
                0 => self.media(ui, project, importing, &mut out),
                1 => placeholder(ui, "Effects arrive in a later version"),
                _ => placeholder(ui, "Text and titles arrive in a later version"),
            });
        self.tab = tab;
        out.import |= action == Some(0);
        if let Some(id) = self.confirm_remove {
            out.command = out.command.or(self.remove_warning(ui, project, id));
        }
        out
    }

    fn media(&mut self, ui: &mut Ui, project: &Project, importing: bool, out: &mut BinResponse) {
        let rect = ui.max_rect();
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
                    (false, true) => "Drop media here or click + to import",
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
                            let r = ui
                                .allocate_response(vec2(ui.available_width(), ROW), Sense::click());
                            self.row(ui, r, a, project, out);
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
                                    let r = ui.allocate_response(tile, Sense::click());
                                    self.tile(ui, r, a, project, out);
                                }
                            });
                        }
                    }
                });
            if importing {
                ui.label(egui::RichText::new("Importing…").color(color::INK_3));
            }
        });
    }

    fn tile(
        &mut self,
        ui: &mut Ui,
        r: egui::Response,
        a: &MediaAsset,
        project: &Project,
        out: &mut BinResponse,
    ) {
        let thumb = Rect::from_min_size(
            r.rect.min,
            vec2(r.rect.width(), r.rect.width() * 9.0 / 16.0),
        );
        let corner = CornerRadius::same(radius::CONTROL);
        let p = ui.painter();
        self.thumbs.paint(ui, thumb, a);
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
        let name = Rect::from_x_y_ranges(r.rect.x_range(), thumb.bottom()..=r.rect.bottom());
        self.name(ui, name, a, color::INK_2);
        self.interact(r, a, project, out);
    }

    fn row(
        &mut self,
        ui: &mut Ui,
        r: egui::Response,
        a: &MediaAsset,
        project: &Project,
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
        let badge =
            Rect::from_center_size(pos2(r.rect.left() + 4.0, r.rect.center().y), vec2(8.0, 8.0));
        p.rect_filled(badge, CornerRadius::same(2), kind);
        let mut right = r.rect.right() - space::S4;
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
        let name = Rect::from_x_y_ranges(r.rect.left() + space::S16..=right, r.rect.y_range());
        self.name(ui, name, a, color::INK);
        self.interact(r, a, project, out);
    }

    /// The asset's name, or the rename field while it is being renamed.
    fn name(&mut self, ui: &mut Ui, rect: Rect, a: &MediaAsset, ink: egui::Color32) {
        if let Some((id, text)) = &mut self.renaming
            && *id == a.id
        {
            let field = Rect::from_center_size(rect.center(), vec2(rect.width(), size::CONTROL));
            let r = ui.put(field, TextInput::new(text).width(rect.width()));
            r.request_focus();
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
        out: &mut BinResponse,
    ) {
        if r.clicked() {
            self.selected = Some(a.id);
            self.renaming = self.renaming.take().filter(|(id, _)| *id == a.id);
        }
        if r.double_clicked() {
            self.renaming = Some((a.id, a.display_name()));
        }
        r.context_menu(|ui| {
            self.selected = Some(a.id);
            if ui.button("Rename").clicked() {
                self.renaming = Some((a.id, a.display_name()));
            }
            if ui.button("Remove from project").clicked() {
                out.command = self.remove(project, a.id);
            }
        });
        if let Some((id, text)) = &self.renaming
            && *id == a.id
        {
            let (enter, esc) = r
                .ctx
                .input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if enter {
                out.command = Some(Command::RenameAsset {
                    id: *id,
                    name: Some(text.clone()),
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

/// Poster thumbnails, made on a job pool and kept on disk by `jvid_media::thumbnails`.
#[derive(Default)]
struct Thumbs {
    /// Started on first paint, so tests and empty bins spawn nothing.
    cache: Option<(Arc<Cache>, Pool)>,
    /// Posters finished off-thread, waiting to become textures.
    done: Arc<Mutex<Vec<(PathBuf, egui::ColorImage)>>>,
    /// `None` while a poster is in flight or failed; the placeholder shows.
    textures: HashMap<PathBuf, Option<TextureHandle>>,
}

impl Thumbs {
    /// The tile's picture: the cached poster, or a neutral placeholder until it's ready.
    fn paint(&mut self, ui: &Ui, rect: Rect, asset: &MediaAsset) {
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(radius::CONTROL), color::RAISED);
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
        match self.textures.get(&asset.path) {
            Some(Some(tex)) => {
                // Letterbox inside the 16:9 tile.
                let t = tex.size_vec2();
                let r = Rect::from_center_size(rect.center(), t * (rect.size() / t).min_elem());
                egui::Image::new(tex)
                    .corner_radius(radius::CONTROL)
                    .paint_at(ui, r);
            }
            Some(None) => {}
            None => {
                self.textures.insert(asset.path.clone(), None);
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
                        Pool::new(2),
                    )
                });
                let (done, ctx, path) = (self.done.clone(), ui.ctx().clone(), asset.path.clone());
                cache.submit_poster(jobs, path.clone(), probe.duration, move |r| match r {
                    Ok(img) => {
                        let size = [img.width() as usize, img.height() as usize];
                        let img = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                        done.lock().unwrap().push((path, img));
                        ctx.request_repaint();
                    }
                    Err(e) => tracing::warn!("thumbnail for {}: {e}", path.display()),
                });
            }
        }
    }
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
    }
}
