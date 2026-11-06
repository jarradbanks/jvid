//! Library view state and organization controls. Only LibraryEdit changes the project.

use super::*;
use crate::keymap::{Applicability, Keymap};
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
    Unfiled,
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
    /// `None` ID creates a bin; otherwise renames that stable bin.
    draft: Option<(Option<BinId>, String)>,
    focus_name: bool,
    pub(super) keymap: Keymap,
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
                    Location::Unfiled => entry.bin.is_none(),
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
                        a.probe.as_ref().is_some_and(|p| p.width.is_some())
                            || a.solid.is_some()
                            || a.text.is_some()
                            || a.sequence.is_some()
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
}

impl MediaBin {
    pub(crate) fn set_library_keymap(&mut self, keymap: &Keymap) {
        self.library.keymap = keymap.clone();
    }

    pub(crate) fn library_modal_open(&self) -> bool {
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
            "media_rename_bin" | "media_delete_bin" => Applicability::when(
                matches!(self.library.location, Location::Bin(id) if project.media_library().bins().contains_key(&id)),
                "bin.choose_bin",
            ),
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
                self.library.draft = Some((None, String::new()));
                self.library.focus_name = true;
                return None;
            }
            "media_rename_bin" => {
                if let Location::Bin(id) = self.library.location {
                    self.library.draft = Some((Some(id), library.bins()[&id].clone()));
                    self.library.focus_name = true;
                }
                return None;
            }
            "media_delete_bin" => {
                let Location::Bin(id) = self.library.location else {
                    return None;
                };
                LibraryEdit::DeleteBin(id)
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
        let response = ui.add_enabled(
            available.is_enabled(),
            egui::Button::new(name)
                .shortcut_text(binding)
                .selected(favorite),
        );
        let response = if let Some(reason) = available.reason() {
            response.on_disabled_hover_text(reason)
        } else {
            response
        };
        response.clicked()
    }

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
            self.library.location = Location::All;
        }
        ui.horizontal_wrapped(|ui| {
            // Fixed field width truncates long bin names instead of widening narrow panels.
            let mut locations = vec![Location::All, Location::Unfiled];
            let mut labels = vec![tr!("bin.all_media"), tr!("bin.unfiled")];
            for (id, name) in library.bins() {
                locations.push(Location::Bin(*id));
                labels.push(name.as_str());
            }
            let mut selected = locations
                .iter()
                .position(|l| *l == self.library.location)
                .unwrap_or(0);
            let width = (ui.available_width() - 2.0 * size::CONTROL - space::S8).clamp(64.0, 144.0);
            ui.add(Dropdown::new(&mut selected, &labels).width(width));
            self.library.location = locations[selected];
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
            let manage = ui.add(Button::icon(icon::ELLIPSIS, tr!("bin.manage_bins")));
            egui::Popup::menu(&manage).show(|ui| {
                for id in &ACTIONS[..3] {
                    if self.library_menu_action(ui, project, id) {
                        out.command = self.library_action(id, project);
                        ui.close();
                    }
                }
                ui.label(tr!("bin.delete_bin_help"));
            });
        });
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

    pub(super) fn library_dialog(&mut self, ui: &mut Ui, project: &Project, out: &mut BinResponse) {
        let Some((id, mut name)) = self.library.draft.clone() else {
            return;
        };
        if id.is_some_and(|id| !project.media_library().bins().contains_key(&id)) {
            self.library.draft = None;
            return;
        }
        let mut commit = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("media_bin_name")).show(ui.ctx(), |ui| {
            ui.set_width(240.0);
            ui.label(
                egui::RichText::new(if id.is_some() {
                    tr!("action.media_rename_bin")
                } else {
                    tr!("action.media_new_bin")
                })
                .font(theme::headline()),
            );
            let label = ui.label(tr!("bin.enter_name"));
            let response = ui
                .add(TextInput::new(&mut name).width(ui.available_width()))
                .labelled_by(label.id);
            if std::mem::take(&mut self.library.focus_name) {
                response.request_focus();
            }
            let valid = project.media_library().check_name(&name, id);
            if let Err(LibraryError::DuplicateName) = valid {
                ui.label(egui::RichText::new(tr!("bin.duplicate_name")).color(color::ink_3()));
            }
            let actions = suite_ui::dialog::action_footer(
                ui,
                [
                    Button::secondary(tr!("prompt.cancel")),
                    Button::primary(tr!("bin.save_bin")).state(if valid.is_ok() {
                        suite_ui::widgets::State::Live
                    } else {
                        suite_ui::widgets::State::Disabled
                    }),
                ],
            );
            cancel = actions[0].clicked();
            commit =
                valid.is_ok() && (actions[1].clicked() || ui.input(|i| i.key_pressed(Key::Enter)));
        });
        if commit {
            out.command = Some(Command::EditMediaLibrary(match id {
                Some(id) => LibraryEdit::RenameBin { id, name },
                None => LibraryEdit::CreateBin { name },
            }));
        } else if !cancel && !modal.should_close() {
            self.library.draft = Some((id, name));
            return;
        }
        self.library.draft = None;
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
        view.location = Location::Unfiled;
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
                if suite_ui::test_support::install_theme(ui, &mut fresh) {
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

    #[test]
    fn bin_dialogs_favorites_membership_removal_and_delete_use_undoable_commands() {
        let mut h = harness(true, 360.0, 1.0, 3);
        h.get_by_label("Keep 00000.mov").click();
        h.run();
        let command = h
            .state_mut()
            .bin
            .library_action("media_new_bin", &Project::default());
        assert!(command.is_none());
        h.run();
        h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Enter a bin name.")
            .type_text("Selects");
        h.run();
        h.get_by_label("Save Bin").click();
        h.run();
        assert_eq!(
            h.state().history.project().media_library().bins()[&BinId(1)],
            "Selects"
        );
        edit(
            &mut h.state_mut().history,
            LibraryEdit::MoveAssets {
                ids: vec![AssetId(1)],
                bin: Some(BinId(1)),
            },
        );
        h.state_mut().bin.library.location = Location::Bin(BinId(1));
        h.run();
        h.get_by_label("Keep 00000.mov").click_secondary();
        h.run();
        h.get_by_label("Toggle Favorite").click();
        h.run();
        assert!(
            h.state()
                .history
                .project()
                .media_library()
                .entry(AssetId(1))
                .favorite
        );
        h.get_by_label("Keep 00000.mov").click_secondary();
        h.run();
        h.get_by_label("Remove from Bin").click();
        h.run();
        assert!(h.state().history.project().asset(AssetId(1)).is_some());
        assert!(
            h.state()
                .history
                .project()
                .media_library()
                .entry(AssetId(1))
                .bin
                .is_none()
        );
        assert!(h.state_mut().history.undo());
        h.run();
        assert_eq!(
            h.state()
                .history
                .project()
                .media_library()
                .entry(AssetId(1))
                .bin,
            Some(BinId(1))
        );
        let state = h.state_mut();
        let command = state
            .bin
            .library_action("media_delete_bin", state.history.project())
            .unwrap();
        state.history.apply(command).unwrap();
        h.run();
        assert_eq!(h.state().history.project().assets().len(), 3);
        assert!(
            h.state()
                .history
                .project()
                .media_library()
                .bins()
                .is_empty()
        );
        assert!(h.state_mut().history.undo());
        assert_eq!(
            h.state()
                .history
                .project()
                .media_library()
                .entry(AssetId(1))
                .bin,
            Some(BinId(1))
        );
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
                let control = h
                    .get_by_role_and_label(egui::accesskit::Role::ComboBox, "All Media")
                    .rect();
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
                if std::env::var_os("JVID_LIBRARY_CAPTURE").is_some()
                    && suite_ui::test_support::gpu()
                {
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

