//! Library view state and organization controls. Only LibraryEdit changes the project.

use super::*;
use crate::keymap::{Applicability, Keymap};
use j_ui::widgets::ButtonKind;
use jvid_core::media_library::{BinId, LibraryEdit, LibraryError};

pub(crate) const ACTIONS: &[&str] = &[
    "media_new_bin",
    "media_rename_bin",
    "media_delete_bin",
    "media_favorite",
];

pub(super) fn usage_counts(project: &Project) -> HashMap<AssetId, usize> {
    let mut counts = HashMap::new();
    for sequence in project.sequences() {
        for track in sequence.tracks() {
            for clip in track.clips() {
                *counts.entry(clip.asset).or_default() += 1;
            }
        }
    }
    counts
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Location {
    #[default]
    All,
    /// The Bins view: a folder tile per bin, then the media in no bin.
    Bins,
    Bin(BinId),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Usage {
    #[default]
    Any,
    Used,
    Unused,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Kind {
    #[default]
    Any,
    Video,
    Audio,
}

#[derive(Default)]
pub(super) struct LibraryView {
    location: Location,
    usage: Usage,
    kind: Kind,
    favorites: bool,
    missing: bool,
    /// The inline name field on a tile: `None` ID creates a bin; otherwise renames it.
    draft: Option<(Option<BinId>, String)>,
    /// Where the name field was drawn last frame; a press outside it commits.
    draft_field: Option<Rect>,
    /// The bin tile last clicked in the Bins view.
    selected_bin: Option<BinId>,
    /// The bin waiting on the Delete Bin confirmation.
    confirm_delete: Option<BinId>,
    pub(super) keymap: Keymap,
}

/// A tile in the Bins view ahead of the media: each bin, then New Bin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum BinTile {
    Bin(BinId),
    New,
}

impl LibraryView {
    pub(super) fn active(&self) -> bool {
        self.location != Location::All || self.filtered()
    }

    fn filtered(&self) -> bool {
        self.usage != Usage::Any || self.kind != Kind::Any || self.favorites || self.missing
    }

    pub(super) fn visible<'a>(
        &self,
        project: &'a Project,
        query: &str,
        sort: usize,
        missing: &HashSet<PathBuf>,
    ) -> Vec<&'a MediaAsset> {
        // One pass over every stored sequence includes nested usage without counting shared
        // nests repeatedly. Missing comes from the app's existing off-thread path checks.
        let used = if self.usage != Usage::Any {
            usage_counts(project)
        } else {
            HashMap::new()
        };
        let library = project.media_library();
        let query = query.trim().to_lowercase();
        let mut assets: Vec<_> = project
            .assets()
            .iter()
            .filter(|a| {
                let entry = library.entry(a.id);
                let location = match self.location {
                    Location::All => true,
                    Location::Bins => entry.bin.is_none(),
                    Location::Bin(id) => entry.bin == Some(id),
                };
                let usage = match self.usage {
                    Usage::Any => true,
                    Usage::Used => used.contains_key(&a.id),
                    Usage::Unused => !used.contains_key(&a.id),
                };
                let kind = match self.kind {
                    Kind::Any => true,
                    Kind::Audio => is_audio(a),
                    Kind::Video => {
                        a.probe.as_ref().is_some_and(|p| p.width.is_some()) || a.generated()
                    }
                };
                !a.generated_caption
                    && location
                    && usage
                    && kind
                    && (!self.favorites || entry.favorite)
                    && (!self.missing
                        || (!a.path.as_os_str().is_empty() && missing.contains(&a.path)))
                    && (query.is_empty() || a.display_name().to_lowercase().contains(&query))
            })
            .collect();
        sort_assets(&mut assets, sort);
        assets
    }

    /// The folder tiles ahead of the media, in the Bins view only. A search hides bins
    /// whose names don't match it.
    pub(super) fn tiles(&self, project: &Project, query: &str) -> Vec<BinTile> {
        if self.location != Location::Bins {
            return Vec::new();
        }
        let query = query.trim().to_lowercase();
        let mut tiles: Vec<_> = project
            .media_library()
            .bins()
            .iter()
            .filter(|(_, name)| name.to_lowercase().contains(&query))
            .map(|(id, _)| BinTile::Bin(*id))
            .collect();
        if query.is_empty() {
            tiles.push(BinTile::New);
        }
        tiles
    }

    /// The bin Rename Bin and Delete Bin act on: the open bin, or the tile last clicked.
    fn target(&self, project: &Project) -> Option<BinId> {
        match self.location {
            Location::Bin(id) => Some(id),
            Location::Bins => self.selected_bin,
            Location::All => None,
        }
        .filter(|id| project.media_library().bins().contains_key(id))
    }
}

impl MediaBin {
    pub(crate) fn set_library_keymap(&mut self, keymap: &Keymap) {
        self.library.keymap = keymap.clone();
    }

    pub(crate) fn library_modal_open(&self) -> bool {
        self.library.confirm_delete.is_some()
    }

    /// A bin tile's inline name field is open.
    pub(crate) fn library_naming(&self) -> bool {
        self.library.draft.is_some()
    }

    /// Shared by panel menus, remapped dispatch and the coordinator's palette wiring.
    pub(crate) fn library_applicability(
        &self,
        id: &str,
        project: &Project,
        focused: bool,
    ) -> Applicability {
        if !focused || self.tab != 0 {
            return Applicability::Disabled("bin.focus_required");
        }
        if self.library_modal_open() || self.confirm_remove.is_some() {
            return Applicability::Disabled("bin.finish_dialog");
        }
        match id {
            "media_new_bin" => Applicability::Enabled,
            "media_rename_bin" | "media_delete_bin" => {
                Applicability::when(self.library.target(project).is_some(), "bin.choose_bin")
            }
            "media_favorite" => Applicability::when(
                !self.library_selection(project).is_empty(),
                "bin.choose_assets",
            ),
            _ => Applicability::Disabled("bin.focus_required"),
        }
    }

    fn library_selection(&self, project: &Project) -> Vec<AssetId> {
        project
            .assets()
            .iter()
            .filter(|a| self.selected_ids.contains(&a.id) || self.selected == Some(a.id))
            .map(|a| a.id)
            .collect()
    }

    pub(crate) fn library_action(&mut self, id: &str, project: &Project) -> Option<Command> {
        if !self.library_applicability(id, project, true).is_enabled() {
            return None;
        }
        let library = project.media_library();
        let edit = match id {
            "media_new_bin" => {
                // The new tile's name field opens in the Bins view, where the bin will show.
                self.library.location = Location::Bins;
                self.library.draft = Some((None, String::new()));
                self.library.draft_field = None;
                return None;
            }
            "media_rename_bin" => {
                let id = self.library.target(project)?;
                if self.library.location == Location::Bin(id) {
                    // Rename on the bin's own tile.
                    self.library.location = Location::Bins;
                }
                self.library.selected_bin = Some(id);
                self.library.draft = Some((Some(id), library.bins()[&id].clone()));
                self.library.draft_field = None;
                return None;
            }
            "media_delete_bin" => {
                self.library.confirm_delete = self.library.target(project);
                return None;
            }
            "media_favorite" => {
                let ids = self.library_selection(project);
                let favorite = !ids.iter().all(|id| library.entry(*id).favorite);
                LibraryEdit::SetFavorites { ids, favorite }
            }
            _ => return None,
        };
        Some(Command::EditMediaLibrary(edit))
    }

    fn library_menu_action(&mut self, ui: &mut Ui, project: &Project, id: &str) -> bool {
        let available = self.library_applicability(id, project, true);
        let (name, binding) = self.library.keymap.label(id);
        let favorite = id == "media_favorite"
            && self
                .library_selection(project)
                .iter()
                .all(|id| project.media_library().entry(*id).favorite);
        let mut button = egui::Button::new(name).selected(favorite);
        if !binding.is_empty() {
            button = button.shortcut_text(binding);
        }
        let response = ui.add_enabled(available.is_enabled(), button);
        let response = if let Some(reason) = available.reason() {
            response.on_disabled_hover_text(reason)
        } else {
            response
        };
        response.clicked()
    }

    /// The All Media / Bins switch and Filters, then, inside a bin, its breadcrumb.
    pub(super) fn library_controls(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        out: &mut BinResponse,
    ) {
        let library = project.media_library();
        if let Location::Bin(id) = self.library.location
            && !library.bins().contains_key(&id)
        {
            self.library.location = Location::Bins;
        }
        let row = egui::Layout::left_to_right(egui::Align::Center);
        ui.allocate_ui_with_layout(vec2(ui.available_width(), size::CONTROL), row, |ui| {
            let mut view = usize::from(self.library.location != Location::All);
            let views = [tr!("bin.all_media"), tr!("bin.bins")];
            if ui
                .add(j_ui::widgets::Segmented::new(&mut view, &views))
                .changed()
            {
                self.library.location = if view == 0 {
                    Location::All
                } else {
                    Location::Bins
                };
                self.library.draft = None;
            }
            ui.menu_button(tr!("bin.filters"), |ui| {
                ui.checkbox(&mut self.library.favorites, tr!("bin.favorites"));
                ui.checkbox(&mut self.library.missing, tr!("bin.missing"));
                ui.separator();
                for (value, label) in [
                    (Usage::Any, tr!("bin.any_usage")),
                    (Usage::Used, tr!("bin.used")),
                    (Usage::Unused, tr!("bin.unused")),
                ] {
                    ui.radio_value(&mut self.library.usage, value, label);
                }
                ui.separator();
                for (value, label) in [
                    (Kind::Any, tr!("bin.any_type")),
                    (Kind::Video, tr!("bin.video")),
                    (Kind::Audio, tr!("bin.audio")),
                ] {
                    ui.radio_value(&mut self.library.kind, value, label);
                }
                if self.library.filtered() && ui.button(tr!("bin.clear_filters")).clicked() {
                    self.library.usage = Usage::Any;
                    self.library.kind = Kind::Any;
                    self.library.favorites = false;
                    self.library.missing = false;
                    ui.close();
                }
            });
        });
        let Location::Bin(id) = self.library.location else {
            return;
        };
        // Breadcrumb: Bins > Name. Bins goes back; media dropped on it leaves the bin.
        ui.allocate_ui_with_layout(vec2(ui.available_width(), size::CONTROL), row, |ui| {
            ui.spacing_mut().item_spacing.x = space::S4;
            let back = ui.add(Button::ghost(tr!("bin.bins")));
            if back.clicked() {
                self.library.location = Location::Bins;
                self.library.selected_bin = Some(id);
            }
            if let Some(ids) = self.dropped(&back, project) {
                out.command = Some(Command::EditMediaLibrary(LibraryEdit::MoveAssets {
                    ids,
                    bin: None,
                }));
            }
            ui.label(
                egui::RichText::new(icon::CHEVRON_RIGHT)
                    .font(theme::icon_font())
                    .color(color::ink_3()),
            );
            let name = &library.bins()[&id];
            let galley = fit_text(
                ui,
                name,
                theme::body_strong(),
                color::ink(),
                ui.available_width(),
            );
            let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
            ui.painter().galley(rect.min, galley, color::ink());
        });
    }

    /// Media dropped on `r` this frame: the dragged asset, with the rest of the selection
    /// when it is part of it. Highlights `r` while media hovers it.
    fn dropped(&self, r: &egui::Response, project: &Project) -> Option<Vec<AssetId>> {
        if r.dnd_hover_payload::<AssetId>().is_some() {
            r.ctx.set_cursor_icon(egui::CursorIcon::Copy);
            r.ctx.layer_painter(r.layer_id).rect_stroke(
                r.rect,
                CornerRadius::same(radius::CONTROL),
                egui::Stroke::new(2.0, color::light_blue()),
                StrokeKind::Inside,
            );
        }
        let id = *r.dnd_release_payload::<AssetId>()?;
        Some(if self.selected_ids.contains(&id) {
            self.library_selection(project)
        } else {
            vec![id]
        })
    }

    /// A bin's folder tile, or the New Bin tile, sized like a media tile or row.
    pub(super) fn bin_tile(
        &mut self,
        ui: &mut Ui,
        r: egui::Response,
        tile: BinTile,
        project: &Project,
        out: &mut BinResponse,
    ) {
        let library = project.media_library();
        let id = match tile {
            BinTile::Bin(id) => Some(id),
            BinTile::New => None,
        };
        let naming = self.library.draft.as_ref().is_some_and(|(d, _)| *d == id);
        let corner = CornerRadius::same(radius::CONTROL);
        let selected = id.is_some() && self.library.selected_bin == id;
        let (art, name) = if self.list {
            let art = Rect::from_min_size(
                r.rect.min + vec2(space::S8, 0.0),
                vec2(size::ICON, r.rect.height()),
            );
            let name = Rect::from_x_y_ranges(
                art.right() + space::S8..=r.rect.right() - space::S8,
                r.rect.y_range(),
            );
            (art, name)
        } else {
            let art = Rect::from_min_size(
                r.rect.min,
                vec2(r.rect.width(), r.rect.width() * 9.0 / 16.0),
            );
            (
                art,
                Rect::from_x_y_ranges(r.rect.x_range(), art.bottom()..=r.rect.bottom()),
            )
        };
        let p = ui.painter().clone();
        if self.list {
            if selected {
                p.rect_filled(r.rect, corner, color::blue().gamma_multiply(0.2));
            } else if r.hovered() {
                p.rect_filled(r.rect, corner, color::raised());
            }
        } else {
            p.rect_filled(art, corner, color::field());
            let stroke = if selected {
                egui::Stroke::new(2.0, color::light_blue())
            } else if r.hovered() {
                egui::Stroke::new(1.0, color::border_strong())
            } else {
                egui::Stroke::new(1.0, color::border())
            };
            p.rect_stroke(art, corner, stroke, StrokeKind::Inside);
        }
        let glyph = if id.is_some() {
            icon::FOLDER
        } else {
            icon::PLUS
        };
        let glyph_ink = if r.hovered() {
            color::ink()
        } else {
            color::ink_2()
        };
        let font = if self.list {
            theme::icon_font()
        } else {
            egui::FontId::new(28.0, theme::icon_font().family)
        };
        let galley = p.layout_no_wrap(glyph.into(), font, glyph_ink);
        p.galley(
            art.center() - galley.mesh_bounds.center().to_vec2(),
            galley,
            glyph_ink,
        );
        let label = match id {
            Some(id) => library.bins()[&id].as_str(),
            None => tr!("action.media_new_bin"),
        };
        if naming {
            self.name_field(ui, name, project, r.id);
        } else {
            let mut right = name.right();
            if let Some(id) = id {
                // How many media the bin holds, like a media tile's duration.
                let n = project
                    .assets()
                    .iter()
                    .filter(|a| library.entry(a.id).bin == Some(id))
                    .count();
                let g = p.layout_no_wrap(n.to_string(), theme::body(), color::ink_3());
                right -= g.size().x;
                p.galley(
                    pos2(right, name.center().y - g.size().y / 2.0),
                    g,
                    color::ink_3(),
                );
                right -= space::S8;
            }
            let ink = if self.list {
                color::ink()
            } else {
                color::ink_2()
            };
            let galley = fit_text(
                ui,
                label,
                theme::body(),
                ink,
                (right - name.left()).max(0.0),
            );
            p.galley(
                pos2(name.left(), name.center().y - galley.size().y / 2.0),
                galley,
                ink,
            );
        }
        r.widget_info(|| match id {
            Some(_) => {
                egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, label)
            }
            None => egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label),
        });
        let Some(id) = id else {
            if r.clicked() && !naming {
                self.library.draft = Some((None, String::new()));
                self.library.draft_field = None;
            }
            return;
        };
        if r.clicked() {
            self.library.selected_bin = Some(id);
            r.request_focus();
        }
        if r.double_clicked() {
            self.library.location = Location::Bin(id);
            self.library.selected_bin = None;
        }
        if let Some(ids) = self.dropped(&r, project) {
            out.command = Some(Command::EditMediaLibrary(LibraryEdit::MoveAssets {
                ids,
                bin: Some(id),
            }));
        }
        r.context_menu(|ui| {
            self.library.selected_bin = Some(id);
            for action in ["media_rename_bin", "media_delete_bin"] {
                if self.library_menu_action(ui, project, action) {
                    out.command = self.library_action(action, project);
                    ui.close();
                }
            }
        });
    }

    /// The inline bin name field in `rect`, named by its tile and focused while it is open.
    fn name_field(&mut self, ui: &mut Ui, rect: Rect, project: &Project, tile: egui::Id) {
        let Some((id, name)) = &mut self.library.draft else {
            return;
        };
        let duplicate = matches!(
            project.media_library().check_name(name, *id),
            Err(LibraryError::DuplicateName)
        );
        let field = Rect::from_min_size(
            pos2(rect.left(), rect.center().y - size::CONTROL / 2.0),
            vec2(rect.width(), size::CONTROL),
        );
        // Focus once, when the field opens; asking every frame can drop fast typing.
        let opened = self.library.draft_field.replace(field).is_none();
        let input = TextInput::new(name)
            .hint(tr!("bin.enter_name"))
            .width(field.width())
            // An empty message only reddens the border; the message is the tooltip.
            .error(duplicate.then_some(""));
        let r = put_over(ui, field, input).labelled_by(tile);
        let r = if duplicate {
            r.on_hover_text(tr!("bin.duplicate_name"))
        } else {
            r
        };
        if opened {
            r.request_focus();
        }
    }

    /// Commits or cancels the inline name field: Enter or a press elsewhere saves a valid
    /// name, Esc or an invalid one cancels. Runs even when the tile scrolled away.
    pub(super) fn finish_naming(&mut self, ui: &Ui, project: &Project, out: &mut BinResponse) {
        let Some((id, name)) = &self.library.draft else {
            return;
        };
        if id.is_some_and(|id| !project.media_library().bins().contains_key(&id)) {
            self.library.draft = None;
            return;
        }
        let field = self.library.draft_field;
        let (enter, esc, away) = ui.ctx().input(|i| {
            let away = i.pointer.any_pressed()
                && i.pointer
                    .press_origin()
                    .is_some_and(|p| !field.is_some_and(|f| f.contains(p)));
            (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape), away)
        });
        let valid = project.media_library().check_name(name, *id).is_ok();
        let unchanged = id.is_some_and(|id| project.media_library().bins()[&id] == name.trim());
        if (enter || away) && valid && !esc && !unchanged {
            out.command = Some(Command::EditMediaLibrary(match *id {
                Some(id) => LibraryEdit::RenameBin {
                    id,
                    name: name.clone(),
                },
                None => LibraryEdit::CreateBin { name: name.clone() },
            }));
        }
        // Enter on a duplicate keeps the field open so the name can be fixed.
        if esc || away || (enter && (valid || unchanged)) {
            self.library.draft = None;
            self.library.draft_field = None;
        }
    }

    /// The Delete Bin confirmation, the one place that says what deleting keeps.
    pub(super) fn delete_dialog(&mut self, ui: &Ui, project: &Project, out: &mut BinResponse) {
        let Some(id) = self.library.confirm_delete else {
            return;
        };
        if !project.media_library().bins().contains_key(&id) {
            self.library.confirm_delete = None;
            return;
        }
        let picked = j_ui::dialog::Dialog {
            title: tr!("action.media_delete_bin"),
            body: tr!("bin.delete_bin_help"),
            buttons: &[
                (tr!("action.media_delete_bin"), ButtonKind::Destructive),
                (tr!("prompt.cancel"), ButtonKind::Secondary),
            ],
            cancel: 1,
            long: false,
        }
        .show(ui.ctx());
        match picked {
            Some(0) => {
                out.command = Some(Command::EditMediaLibrary(LibraryEdit::DeleteBin(id)));
                if self.library.location == Location::Bin(id) {
                    self.library.location = Location::Bins;
                }
                self.library.selected_bin = None;
                self.library.confirm_delete = None;
            }
            Some(_) => self.library.confirm_delete = None,
            None => {}
        }
    }

    pub(super) fn library_asset_menu(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        out: &mut BinResponse,
    ) {
        let ids = self.library_selection(project);
        if self.library_menu_action(ui, project, "media_favorite") {
            out.command = self.library_action("media_favorite", project);
            ui.close();
        }
        ui.menu_button(tr!("bin.move_to_bin"), |ui| {
            if ui.button(tr!("bin.unfiled")).clicked() {
                out.command = Some(Command::EditMediaLibrary(LibraryEdit::MoveAssets {
                    ids: ids.clone(),
                    bin: None,
                }));
                ui.close();
            }
            for (id, name) in project.media_library().bins() {
                if ui.button(name).clicked() {
                    out.command = Some(Command::EditMediaLibrary(LibraryEdit::MoveAssets {
                        ids: ids.clone(),
                        bin: Some(*id),
                    }));
                    ui.close();
                }
            }
        });
        if let Location::Bin(_) = self.library.location
            && ui.button(tr!("bin.remove_from_bin")).clicked()
        {
            out.command = Some(Command::EditMediaLibrary(LibraryEdit::MoveAssets {
                ids,
                bin: None,
            }));
            ui.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    use jvid_core::{
        file,
        history::{Created, History},
        model::{NewClip, ProbeInfo, TrackKind},
        time::Time,
    };
    use std::path::Path;

    fn history(count: usize) -> History {
        if count > 100 {
            // Build a reopened large project without timing the pre-existing quadratic import
            // duplicate check. The assertion below measures library drawing/selection only.
            let mut value = toml::Value::try_from(history(1).project()).unwrap();
            let template = value["assets"][0].clone();
            value["assets"] = toml::Value::Array(
                (0..count)
                    .map(|n| {
                        let mut asset = template.clone();
                        asset["id"] = toml::Value::Integer(n as i64 + 1);
                        asset["path"] = toml::Value::String(format!("/media/Keep {n:05}.mov"));
                        asset
                    })
                    .collect(),
            );
            value["next_asset"] = toml::Value::Integer(count as i64);
            return History::new(value.try_into().unwrap());
        }
        let mut history = History::new(Project::default());
        history
            .apply(Command::Import(
                (0..count)
                    .map(|n| {
                        (
                            PathBuf::from(format!("/media/Keep {n:05}.mov")),
                            ProbeInfo {
                                width: Some(1920),
                                height: Some(1080),
                                duration: Time(n as i64 + 1),
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
            ))
            .unwrap();
        history
    }

    fn edit(history: &mut History, edit: LibraryEdit) {
        history.apply(Command::EditMediaLibrary(edit)).unwrap();
    }

    #[test]
    fn combined_filters_search_sort_and_reopen_include_nested_sequence_usage() {
        let mut history = history(3);
        let Created::Track(track) = history
            .apply(Command::AddTrack {
                kind: TrackKind::Video,
                name: "V1".into(),
            })
            .unwrap()
        else {
            panic!()
        };
        let Created::Clip(clip) = history
            .apply(Command::AddClip {
                track,
                clip: NewClip {
                    asset: AssetId(1),
                    source_in: Time(0),
                    source_out: Time(1),
                    start: Time(0),
                    link: None,
                },
            })
            .unwrap()
        else {
            panic!()
        };
        history
            .apply(Command::Nest {
                ids: vec![clip],
                name: "Nested".into(),
                outer: "Program".into(),
            })
            .unwrap();
        assert!(
            !history
                .project()
                .sequence()
                .tracks()
                .iter()
                .flat_map(|t| t.clips())
                .any(|c| c.asset == AssetId(1))
        );
        edit(
            &mut history,
            LibraryEdit::CreateBin {
                name: "Selects".into(),
            },
        );
        edit(
            &mut history,
            LibraryEdit::MoveAssets {
                ids: vec![AssetId(1), AssetId(2)],
                bin: Some(BinId(1)),
            },
        );
        edit(
            &mut history,
            LibraryEdit::SetFavorites {
                ids: vec![AssetId(1), AssetId(2)],
                favorite: true,
            },
        );
        history.mark_saved();
        let missing = HashSet::from([
            PathBuf::from("/media/Keep 00000.mov"),
            PathBuf::from("/media/Keep 00001.mov"),
        ]);
        let mut view = LibraryView {
            location: Location::Bin(BinId(1)),
            favorites: true,
            missing: true,
            kind: Kind::Video,
            usage: Usage::Used,
            ..Default::default()
        };
        let ids = |view: &LibraryView, p: &Project, sort| {
            view.visible(p, " KEEP ", sort, &missing)
                .iter()
                .map(|a| a.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&view, history.project(), 1), [AssetId(1)]);
        view.usage = Usage::Unused;
        assert_eq!(ids(&view, history.project(), 1), [AssetId(2)]);
        view.usage = Usage::Any;
        assert_eq!(ids(&view, history.project(), 2), [AssetId(2), AssetId(1)]);
        let json = file::to_json(history.project(), Path::new("/project")).unwrap();
        let loaded = file::from_json(&json, Path::new("/project"), |_| false).unwrap();
        assert_eq!(ids(&view, &loaded, 1), [AssetId(1), AssetId(2)]);
        view.location = Location::Bins;
        assert!(ids(&view, &loaded, 1).is_empty());
        assert!(!history.is_dirty());
    }

    #[test]
    fn audio_missing_generated_media_and_name_filters_compose() {
        let mut history = history(1);
        history
            .apply(Command::Import(vec![(
                "/media/Song.wav".into(),
                ProbeInfo {
                    audio_streams: 1,
                    ..Default::default()
                },
            )]))
            .unwrap();
        history
            .apply(Command::AddSolid {
                name: "Keep Solid".into(),
                color: [0.0; 4],
            })
            .unwrap();
        history
            .apply(Command::RenameAsset {
                id: AssetId(2),
                name: Some("Keep Audio".into()),
            })
            .unwrap();
        let missing = HashSet::from([PathBuf::from("/media/Song.wav"), PathBuf::new()]);
        let mut view = LibraryView {
            kind: Kind::Audio,
            missing: true,
            ..Default::default()
        };
        assert_eq!(
            view.visible(history.project(), "Keep", 1, &missing)[0].id,
            AssetId(2)
        );
        assert!(
            view.visible(history.project(), "Song", 1, &missing)
                .is_empty()
        );
        view.kind = Kind::Video;
        assert!(view.visible(history.project(), "", 1, &missing).is_empty());
        view.missing = false;
        assert_eq!(view.visible(history.project(), "", 1, &missing).len(), 2);
    }

    struct State {
        bin: MediaBin,
        history: History,
    }

    fn harness(list: bool, width: f32, scale: f32, count: usize) -> Harness<'static, State> {
        let mut fresh = true;
        let builder = Harness::builder()
            .with_size([width, 620.0])
            .with_pixels_per_point(scale);
        let builder = if std::env::var_os("JVID_LIBRARY_CAPTURE").is_some() {
            builder.wgpu()
        } else {
            builder
        };
        let mut harness = builder.build_ui_state(
            move |ui, state: &mut State| {
                if j_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                let response =
                    state
                        .bin
                        .show(ui, state.history.project(), true, false, &HashSet::new());
                if let Some(command) = response.command {
                    state.history.apply(command).unwrap();
                }
            },
            State {
                bin: MediaBin {
                    list,
                    ..Default::default()
                },
                history: history(count),
            },
        );
        harness.run();
        harness
    }

    fn bin(h: &Harness<'_, State>) -> Option<BinId> {
        let p = h.state().history.project();
        p.media_library().entry(AssetId(1)).bin
    }

    #[test]
    fn bins_view_tiles_create_drop_enter_rename_and_delete_use_undoable_commands() {
        let mut h = harness(false, 360.0, 1.0, 3);
        // No Manage Bins menu: bins live in their own view.
        assert!(h.query_by_label("Manage Bins").is_none());
        h.get_by_label("Bins").click();
        h.run();
        h.get_by_label("New Bin").click();
        h.run();
        h.get_by_role_and_label(egui::accesskit::Role::TextInput, "New Bin")
            .type_text("Selects");
        h.run();
        h.key_press(Key::Enter);
        h.run();
        let library = |h: &Harness<'_, State>| h.state().history.project().media_library().clone();
        assert_eq!(library(&h).bins()[&BinId(1)], "Selects");
        // A duplicate name keeps the inline field open and saves nothing.
        h.get_by_label("New Bin").click();
        h.run();
        h.get_by_role_and_label(egui::accesskit::Role::TextInput, "New Bin")
            .type_text("selects");
        h.run();
        h.key_press(Key::Enter);
        h.run();
        assert_eq!(library(&h).bins().len(), 1);
        assert!(h.state().bin.library_naming());
        h.key_press(Key::Escape);
        h.run();
        assert!(!h.state().bin.library_naming());

        // Unfiled media sit beside the bin tiles; drag one onto the Selects tile.
        let tile = h.get_by_label("Selects").rect().center();
        let media = h.get_by_label("Keep 00000.mov").rect().center();
        h.drag_at(media);
        h.run();
        h.hover_at(tile);
        h.run();
        h.drop_at(tile);
        h.run();
        assert_eq!(bin(&h), Some(BinId(1)));
        assert!(
            h.query_by_label("Keep 00000.mov").is_none(),
            "filed media leave Bins"
        );
        assert!(h.state_mut().history.undo());
        assert_eq!(bin(&h), None);
        assert!(h.state_mut().history.redo());
        h.run();

        // Double-click enters the bin; its breadcrumb goes back.
        let at = h.get_by_label("Selects").rect().center();
        // Far from the drop's click, so the two presses count as one double-click.
        h.input_mut().time = Some(h.ctx.input(|i| i.time) + 1.0);
        h.hover_at(at);
        h.step();
        for pressed in [true, false, true, false] {
            h.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            h.step();
        }
        h.run();
        assert_eq!(h.state().bin.library.location, Location::Bin(BinId(1)));
        h.get_by_label("Keep 00000.mov").click_secondary();
        h.run_steps(4);
        h.get_by_label("Toggle Favorite").click();
        h.run_steps(4);
        assert!(library(&h).entry(AssetId(1)).favorite);
        h.get_by_label("Keep 00000.mov").click_secondary();
        h.run_steps(4);
        h.get_by_label("Remove from Bin").click();
        h.run_steps(4);
        assert!(h.state().history.project().asset(AssetId(1)).is_some());
        assert_eq!(bin(&h), None);
        assert!(h.state_mut().history.undo());
        h.run_steps(4);
        assert_eq!(bin(&h), Some(BinId(1)));
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Bins")
            .click();
        h.run_steps(4);
        assert_eq!(h.state().bin.library.location, Location::Bins);

        // Right-click a bin tile to rename it inline.
        h.get_by_label("Selects").click_secondary();
        h.run_steps(4);
        h.get_by_label("Rename Bin").click();
        h.run_steps(4);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
        h.run_steps(4);
        h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Selects")
            .type_text("Keepers");
        h.run_steps(4);
        h.key_press(Key::Enter);
        h.run_steps(4);
        assert_eq!(library(&h).bins()[&BinId(1)], "Keepers");

        // Delete asks first; the dialog alone says what deleting keeps.
        assert!(h.query_by_label(tr!("bin.delete_bin_help")).is_none());
        h.get_by_label("Keepers").click_secondary();
        h.run_steps(4);
        h.get_by_label("Delete Bin").click();
        h.run_steps(4);
        assert!(h.state().bin.library_modal_open());
        h.get_by_label(tr!("bin.delete_bin_help"));
        h.get_by_label("Cancel").click();
        h.run_steps(4);
        assert_eq!(library(&h).bins().len(), 1);
        h.get_by_label("Keepers").click_secondary();
        h.run_steps(4);
        h.get_by_label("Delete Bin").click();
        h.run_steps(4);
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Delete Bin")
            .click();
        h.run_steps(4);
        assert_eq!(h.state().history.project().assets().len(), 3);
        assert!(library(&h).bins().is_empty());
        assert!(h.state_mut().history.undo());
        assert_eq!(bin(&h), Some(BinId(1)));
    }

    #[test]
    fn large_library_draws_only_visible_items_and_selection_stays_linear() {
        for list in [true, false] {
            let mut h = harness(list, 360.0, 1.0, 20_000);
            h.get_by_label("Keep 00000.mov");
            assert!(h.query_by_label("Keep 19999.mov").is_none());
            h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
            h.run();
            assert_eq!(h.state().bin.selected_ids.len(), 20_000);
            let start = std::time::Instant::now();
            h.step();
            let elapsed = start.elapsed();
            eprintln!("20k media, list={list}: selected frame {elapsed:?}");
            assert!(
                elapsed.as_secs_f32() < 2.0,
                "20k selected frame took {elapsed:?}"
            );
            h.state_mut().bin.library.missing = true;
            h.run();
            assert!(h.state().bin.selected_ids.is_empty());
        }
    }

    #[test]
    fn filtering_away_a_renamed_asset_commits_and_does_not_strand_editing() {
        let mut h = harness(true, 360.0, 1.0, 3);
        h.state_mut().bin.renaming = Some((AssetId(1), "Renamed".into()));
        h.run();
        h.state_mut().bin.query = "00001".into();
        h.get_by_label("Filters").click();
        h.run();
        assert!(h.state().bin.renaming.is_none());
        assert_eq!(
            h.state()
                .history
                .project()
                .asset(AssetId(1))
                .unwrap()
                .display_name(),
            "Renamed.mov"
        );
        assert!(h.state_mut().history.undo());
        assert_eq!(
            h.state()
                .history
                .project()
                .asset(AssetId(1))
                .unwrap()
                .display_name(),
            "Keep 00000.mov"
        );
    }

    #[test]
    fn media_selection_shortcuts_follow_current_bindings() {
        let mut h = harness(true, 360.0, 1.0, 3);
        let keymap =
            Keymap::from_toml("select_all = \"Ctrl+Alt+A\"\ndelete = \"Backspace\"").unwrap();
        h.state_mut().bin.set_library_keymap(&keymap);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
        h.run();
        assert!(h.state().bin.selected_ids.is_empty());
        h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::ALT, Key::A);
        h.run();
        assert_eq!(h.state().bin.selected_ids.len(), 3);
        h.key_press(Key::Delete);
        h.run();
        assert_eq!(h.state().history.project().assets().len(), 3);
        h.key_press(Key::Backspace);
        h.run();
        assert!(h.state().history.project().assets().is_empty());
        assert!(h.state_mut().history.undo());
        assert_eq!(h.state().history.project().assets().len(), 3);
    }

    #[test]
    fn narrow_high_dpi_library_controls_and_actions_stay_inside_panel() {
        for list in [true, false] {
            for scale in [1.0, 1.5] {
                let mut h = harness(list, 240.0, scale, 5);
                let control = h.get_by_label("All Media").rect();
                let filters = h.get_by_label("Filters").rect();
                assert!(control.left() >= 0.0 && filters.right() <= 240.0);
                h.get_by_label("Filters").click();
                h.run();
                h.get_by_label("Favorites");
                h.get_by_label("Missing");
                h.key_press(Key::Escape);
                h.run();
                let p = h.state().history.project();
                assert!(
                    h.state()
                        .bin
                        .library_applicability("media_new_bin", p, true)
                        .is_enabled()
                );
                assert!(
                    !h.state()
                        .bin
                        .library_applicability("media_favorite", p, true)
                        .is_enabled()
                );
                assert!(
                    !h.state()
                        .bin
                        .library_applicability("media_new_bin", p, false)
                        .is_enabled()
                );
                assert!(
                    !h.state()
                        .bin
                        .library_applicability("media_delete_bin", p, true)
                        .is_enabled()
                );
                if std::env::var_os("JVID_LIBRARY_CAPTURE").is_some() && j_ui::test_support::gpu() {
                    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../../target/media-library-ui");
                    std::fs::create_dir_all(&dir).unwrap();
                    h.render()
                        .unwrap()
                        .save(dir.join(format!("library-240-list-{list}-scale-{scale}.png")))
                        .unwrap();
                }
            }
        }
    }
}
