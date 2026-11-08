//! Inspector settings clipboard and translated applicability. Transfer rules live in core.

use egui::Ui;
use j_ui::tokens::{color, space};
use j_ui::widgets::{Button, Checkbox};
use jvid_core::history::Command;
use jvid_core::model::{ClipId, Project};
use jvid_core::settings_transfer::{Animation, Category, Error, Resource, Settings};
use jvid_core::time::Time;

use crate::i18n::tr;
use crate::keymap::Applicability;

/// These registration IDs are for the shared action registry/executor.
pub const COPY_ACTION: &str = "copy_settings";
pub const PASTE_ACTION: &str = "paste_settings";

/// Session view state only; never serialized into project or export settings.
#[derive(Debug, Clone, Default)]
pub struct Transfer {
    clipboard: Option<(Settings, Settings)>,
    pub categories: Vec<Category>,
    pub animation: Animation,
}

impl Transfer {
    pub fn copy_applicability(project: &Project, selected: &[ClipId]) -> Applicability {
        let [source] = selected else {
            return Applicability::Disabled("settings_transfer.one_source");
        };
        match Settings::supported(project, *source) {
            Ok(_) => Applicability::Enabled,
            Err(error) => Applicability::Disabled(error_key(&error)),
        }
    }
    pub fn copy(&mut self, project: &Project, selected: &[ClipId], at: Time) -> Result<(), String> {
        let [source] = selected else {
            return Err(tr!("settings_transfer.one_source").to_owned());
        };
        let categories = Settings::supported(project, *source).map_err(reason)?;
        let static_settings =
            Settings::extract(project, *source, &categories, at, Animation::Static)
                .map_err(reason)?;
        let animation = Settings::extract(project, *source, &categories, at, Animation::Relative)
            .map_err(reason)?;
        self.clipboard = Some((static_settings, animation));
        self.categories = categories;
        self.animation = Animation::Static;
        Ok(())
    }

    fn settings(&self) -> Result<Settings, Error> {
        let (static_settings, animated) = self.clipboard.as_ref().ok_or(Error::NoCategories)?;
        match self.animation {
            Animation::Static => static_settings,
            Animation::Relative => animated,
        }
        .select(&self.categories)
    }

    /// Shared by Inspector/menu/palette/dispatch. Dispatch must call `command` again so a
    /// resource removed or target locked after opening a menu cannot slip past validation.
    pub fn applicability(&self, project: &Project, selected: &[ClipId]) -> Applicability {
        if self.clipboard.is_none() {
            return Applicability::Disabled("settings_transfer.empty");
        }
        match self.prepare(project, selected) {
            Ok(_) => Applicability::Enabled,
            Err(error) => Applicability::Disabled(error_key(&error)),
        }
    }

    pub fn command(&self, project: &Project, selected: &[ClipId]) -> Result<Command, String> {
        if self.clipboard.is_none() {
            return Err(tr!("settings_transfer.empty").into());
        }
        self.prepare(project, selected).map_err(reason)
    }

    fn prepare(&self, project: &Project, selected: &[ClipId]) -> Result<Command, Error> {
        let settings = self.settings()?;
        crate::effects::with(|library| {
            settings.command(project, selected, |resource| match resource {
                Resource::Effect(stem) => library.get(stem).is_some_and(Result::is_ok),
                Resource::Lut(path) => std::path::Path::new(path).is_file(),
            })
        })
    }
}

fn error_key(error: &Error) -> &'static str {
    match error {
        Error::Model(jvid_core::model::ModelError::Locked(_)) => "settings_transfer.locked",
        Error::Model(_) => "settings_transfer.invalid_target",
        Error::NoCategories => "settings_transfer.no_categories",
        Error::NoTargets => "settings_transfer.no_targets",
        Error::Incompatible { .. } => "settings_transfer.incompatible_target",
        Error::UnavailableEffect(_) | Error::UnavailableLut(_) => {
            "settings_transfer.unavailable_resources"
        }
        Error::MaskNeedsTransform => "settings_transfer.mask_needs_transform",
        Error::MaskedTarget(_) => "settings_transfer.masked_target",
        Error::InvalidEffectMask => "settings_transfer.invalid_mask",
        Error::SharedText(_) => "settings_transfer.shared_text",
        Error::FrozenAnimation(_) => "settings_transfer.frozen_animation",
    }
}

fn category_name(category: Category) -> &'static str {
    match category {
        Category::Transform => tr!("inspector.transform"),
        Category::Grade => tr!("settings_transfer.grade"),
        Category::Effects => tr!("inspector.effects"),
        Category::Audio => tr!("inspector.audio"),
        Category::Text => tr!("inspector.text"),
    }
}

pub fn reason(error: Error) -> String {
    match error {
        Error::Model(jvid_core::model::ModelError::Locked(_)) => {
            tr!("settings_transfer.locked").into()
        }
        Error::Model(_) => tr!("settings_transfer.invalid_target").into(),
        Error::NoCategories => tr!("settings_transfer.no_categories").into(),
        Error::NoTargets => tr!("settings_transfer.no_targets").into(),
        Error::Incompatible { category, .. } => tr!(
            "settings_transfer.incompatible",
            category = category_name(category)
        ),
        Error::UnavailableEffect(name) => tr!("settings_transfer.missing_effect", name = name),
        Error::UnavailableLut(name) => tr!("settings_transfer.missing_lut", name = name),
        error => crate::i18n::CATALOG
            .get(error_key(&error))
            .unwrap_or(error_key(&error))
            .to_owned(),
    }
}

fn clipboard_id() -> egui::Id {
    egui::Id::new("settings transfer clipboard")
}

/// Access the same clipboard from shared action dispatch without a second executor.
pub fn with<R>(ctx: &egui::Context, f: impl FnOnce(&mut Transfer) -> R) -> R {
    let mut transfer = ctx
        .data_mut(|data| data.remove_temp::<Transfer>(clipboard_id()))
        .unwrap_or_default();
    let result = f(&mut transfer);
    ctx.data_mut(|data| data.insert_temp(clipboard_id(), transfer));
    result
}

/// Compact Inspector controls: paste options wrap naturally in a 240-point panel.
pub fn show(ui: &mut Ui, project: &Project, selected: &[ClipId], at: Time) -> Option<Command> {
    let ctx = ui.ctx().clone();
    with(&ctx, |transfer| {
        let mut command = None;
        ui.horizontal_wrapped(|ui| {
            let applicable = Transfer::copy_applicability(project, selected);
            let copy = ui.add_enabled(
                applicable.is_enabled(),
                Button::secondary(tr!("action.copy_settings")),
            );
            if let Some(reason) = applicable.reason() {
                copy.clone().on_disabled_hover_text(reason);
            }
            if copy.clicked() {
                let _ = transfer.copy(project, selected, at);
            }
            if transfer.clipboard.is_some() {
                ui.menu_button(tr!("settings_transfer.options"), |ui| {
                    let categories: Vec<_> = transfer
                        .clipboard
                        .as_ref()
                        .unwrap()
                        .0
                        .categories()
                        .collect();
                    for category in categories {
                        let mut selected = transfer.categories.contains(&category);
                        if ui
                            .add(Checkbox::new(&mut selected, category_name(category)))
                            .changed()
                        {
                            transfer.categories.retain(|c| *c != category);
                            if selected {
                                transfer.categories.push(category);
                            }
                        }
                    }
                    ui.separator();
                    let mut animated = transfer.animation == Animation::Relative;
                    if ui
                        .add(Checkbox::new(
                            &mut animated,
                            tr!("settings_transfer.animation"),
                        ))
                        .changed()
                    {
                        transfer.animation = if animated {
                            Animation::Relative
                        } else {
                            Animation::Static
                        };
                    }
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_max_width(216.0);
                        ui.add(egui::Label::new(tr!("settings_transfer.replace_hint")).wrap());
                        if animated {
                            ui.add(
                                egui::Label::new(tr!("settings_transfer.animation_hint")).wrap(),
                            );
                        }
                    });
                });
            }
            let applicable = transfer.applicability(project, selected);
            let paste = ui.add_enabled(
                applicable.is_enabled(),
                Button::secondary(tr!("action.paste_settings")),
            );
            if let Some(reason) = applicable.reason() {
                paste.clone().on_disabled_hover_text(reason);
            }
            if paste.clicked() {
                command = transfer.command(project, selected).ok();
            }
        });
        if transfer.clipboard.is_some() {
            let mode = match transfer.animation {
                Animation::Static => tr!("settings_transfer.static"),
                Animation::Relative => tr!("settings_transfer.with_animation"),
            };
            ui.label(egui::RichText::new(mode).color(color::ink_3()));
        }
        ui.add_space(space::S4);
        command
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::history::{Created, History};
    use jvid_core::model::{Effect, NewClip, TrackId, TrackKind, Transform};
    use std::collections::BTreeMap;

    fn history() -> (History, TrackId, [ClipId; 2]) {
        let mut history = History::new(Project::default());
        let Created::Track(track) = history
            .apply(Command::AddTrack {
                kind: TrackKind::Video,
                name: "V1".into(),
            })
            .unwrap()
        else {
            panic!()
        };
        let ids = [0, 1].map(|index| {
            let Created::Asset(asset) = history
                .apply(Command::AddSolid {
                    name: format!("Solid {index}"),
                    color: [1.0; 4],
                })
                .unwrap()
            else {
                panic!()
            };
            let Created::Clip(id) = history
                .apply(Command::AddClip {
                    track,
                    clip: NewClip {
                        asset,
                        start: Time(index * 100),
                        source_in: Time(0),
                        source_out: Time(100),
                        link: None,
                    },
                })
                .unwrap()
            else {
                panic!()
            };
            id
        });
        history
            .apply(Command::SetClipProps(vec![(
                ids[0],
                Transform {
                    position: [20.0, 40.0],
                    ..Transform::default()
                },
                BTreeMap::new(),
            )]))
            .unwrap();
        history.mark_saved();
        (history, track, ids)
    }

    #[test]
    fn clipboard_options_do_not_dirty_project_and_paste_undoes_once() {
        let (mut history, _, ids) = history();
        let before = history.project().clone();
        let revision = history.rev();
        let mut transfer = Transfer::default();
        transfer
            .copy(history.project(), &ids[..1], Time(0))
            .unwrap();
        assert_eq!(transfer.animation, Animation::Static);
        transfer.categories = vec![Category::Transform];
        transfer.animation = Animation::Relative;
        assert_eq!(history.project(), &before);
        assert_eq!(history.rev(), revision);
        assert!(!history.is_dirty());
        history
            .apply(transfer.command(history.project(), &ids[1..]).unwrap())
            .unwrap();
        assert_eq!(
            history
                .project()
                .sequence()
                .clip(ids[1])
                .unwrap()
                .1
                .transform
                .position,
            [20.0, 40.0]
        );
        assert!(history.undo());
        assert_eq!(history.project(), &before);
    }

    #[test]
    fn applicability_and_dispatch_share_resource_and_lock_validation() {
        let (mut history, track, ids) = history();
        let source = history.project().sequence().clip(ids[0]).unwrap().1;
        let mut transform = source.transform.clone();
        transform.effects.push(Effect {
            shader: "p1-missing-effect-95378".into(),
            params: BTreeMap::new(),
            enabled: true,
            mask: None,
        });
        history
            .apply(Command::SetClipProps(vec![(
                ids[0],
                transform,
                BTreeMap::new(),
            )]))
            .unwrap();
        let mut transfer = Transfer::default();
        transfer
            .copy(history.project(), &ids[..1], Time(0))
            .unwrap();
        transfer.categories = vec![Category::Effects];
        assert!(transfer.command(history.project(), &ids[1..]).is_err());
        assert_eq!(
            transfer.applicability(history.project(), &ids[1..]),
            Applicability::Disabled("settings_transfer.unavailable_resources")
        );
        assert!(
            transfer
                .command(history.project(), &ids[1..])
                .unwrap_err()
                .contains("p1-missing-effect-95378")
        );
        transfer.categories = vec![Category::Transform];
        assert!(
            transfer
                .applicability(history.project(), &ids[1..])
                .is_enabled()
        );
        let mut settings = history
            .project()
            .sequence()
            .track(track)
            .unwrap()
            .settings();
        settings.locked = true;
        history.apply(Command::SetTrack(settings)).unwrap();
        assert_eq!(
            transfer.applicability(history.project(), &ids[1..]),
            Applicability::Disabled("settings_transfer.locked")
        );
        assert_eq!(
            transfer.command(history.project(), &ids[1..]).unwrap_err(),
            tr!("settings_transfer.locked")
        );
        assert!(Transfer::copy_applicability(history.project(), &ids[..1]).is_enabled());
        assert!(!Transfer::copy_applicability(history.project(), &ids).is_enabled());
    }

    #[test]
    fn inspector_controls_work_in_narrow_panel() {
        if crate::test_util::isolate_locale() {
            return;
        }
        controls(1.0);
    }

    #[test]
    fn inspector_controls_work_at_high_dpi() {
        if crate::test_util::isolate_locale() {
            return;
        }
        controls(1.5);
    }

    fn controls(scale: f32) {
        use egui_kittest::kittest::Queryable;
        let previous_language = j_ui::i18n::language();
        let language = std::env::var("JVID_P1_UI_LANGUAGE").ok();
        if let Some(language) = &language {
            j_ui::i18n::set_language(language);
        }
        let (history, _, ids) = history();
        let mut fresh = true;
        let artifact_dir = std::env::var_os("JVID_P1_UI_ARTIFACTS").map(std::path::PathBuf::from);
        if artifact_dir.is_some() {
            assert!(
                j_ui::test_support::gpu(),
                "UI artifacts require a rendering backend"
            );
        }
        let builder = egui_kittest::Harness::builder()
            .with_size([240.0, 260.0])
            .with_pixels_per_point(scale);
        let builder = if artifact_dir.is_some() {
            builder.wgpu()
        } else {
            builder
        };
        let mut harness = builder.build_ui_state(
            |ui, history: &mut History| {
                if j_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                ui.painter().rect_filled(ui.max_rect(), 0.0, color::panel());
                if let Some(command) = show(ui, history.project(), &ids[..1], Time(0)) {
                    history.apply(command).unwrap();
                }
            },
            history,
        );
        harness.run();
        harness.get_by_label(tr!("action.copy_settings")).click();
        harness.run();
        for label in [
            tr!("action.copy_settings"),
            tr!("action.paste_settings"),
            tr!("settings_transfer.options"),
        ] {
            assert!(harness.get_by_label(label).rect().right() <= 240.0);
        }
        if let Some(dir) = &artifact_dir {
            std::fs::create_dir_all(dir).unwrap();
            harness
                .render()
                .unwrap()
                .save(dir.join(format!("settings-transfer-240-{scale}.png")))
                .unwrap();
        }
        harness
            .get_by_label(tr!("settings_transfer.options"))
            .click();
        harness.run();
        harness
            .get_by_label(tr!("settings_transfer.animation"))
            .click();
        harness.run();
        assert_eq!(
            with(&harness.ctx, |transfer| transfer.animation),
            Animation::Relative
        );
        assert!(!harness.state().is_dirty());
        if let Some(dir) = &artifact_dir {
            harness
                .get_by_label(tr!("settings_transfer.options"))
                .click();
            harness.run();
            harness
                .render()
                .unwrap()
                .save(dir.join(format!("settings-transfer-options-240-{scale}.png")))
                .unwrap();
        }
        if language.is_some() {
            j_ui::i18n::set_language(previous_language);
        }
    }
}
