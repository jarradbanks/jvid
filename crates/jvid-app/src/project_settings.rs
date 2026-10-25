//! The project settings dialog: resolution, frame rate and background, applied as one undoable edit.

use egui::Ui;
use jvid_core::history::Command;
use jvid_core::model::{Project, ProjectSettings};
use jvid_core::time::Rate;
use suite_ui::theme;
use suite_ui::tokens::{radius, size, space};
use suite_ui::widgets::{Button, Checkbox, ColorField, DragField, Dropdown, State};

use crate::i18n::{Label, tr, trn, verbatim};

/// Frame rates with the number shown for each.
const RATES: [(&str, Rate); 8] = [
    ("23.976", Rate::new(24000, 1001)),
    ("24", Rate::new(24, 1)),
    ("25", Rate::new(25, 1)),
    ("29.97", Rate::new(30000, 1001)),
    ("30", Rate::new(30, 1)),
    ("50", Rate::new(50, 1)),
    ("59.94", Rate::new(60000, 1001)),
    ("60", Rate::new(60, 1)),
];

/// Size presets; picking one sets width and height, and any other size shows Custom.
const SIZES: [(Label, (u32, u32)); 5] = [
    (|| verbatim("16:9 · 1080p"), (1920, 1080)),
    (|| verbatim("16:9 · 1440p"), (2560, 1440)),
    (|| verbatim("16:9 · 4K"), (3840, 2160)),
    (|| tr!("size.vertical"), (1080, 1920)),
    (|| tr!("size.square"), (1080, 1080)),
];

/// The width of the value column: the two resolution fields and the gap between.
const FIELD: f32 = 2.0 * 80.0 + space::S8;

/// Edits in progress; `None` while the dialog is closed.
#[derive(Default)]
pub struct SettingsDialog {
    pub(crate) draft: Option<ProjectSettings>,
    /// An HDR10 export would find an encoder; checked on open.
    pub(crate) hdr10: bool,
}

impl SettingsDialog {
    pub fn open(&mut self, project: &Project) {
        self.draft = Some(project.settings().clone());
        self.hdr10 = jvid_media::ffmpeg::init().is_ok() && jvid_media::encode::hdr10_available();
    }

    /// Shows the dialog while open; returns the edit when the user applies a change.
    pub fn show(&mut self, ui: &mut Ui, project: &Project) -> Option<Command> {
        let draft = self.draft.as_mut()?;
        let old = project.settings();
        let mut command = None;
        let modal = egui::Modal::new(egui::Id::new("project_settings"))
            .frame(
                egui::Frame::popup(ui.style())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(space::S16),
            )
            .show(ui.ctx(), |ui| {
                ui.set_width(360.0);
                // Menus stay inside the dialog, as it was last frame.
                let screen = ui.ctx().content_rect();
                let bounds = ui
                    .ctx()
                    .memory(|m| m.area_rect(egui::Id::new("project_settings")))
                    .unwrap_or(screen);
                ui.label(egui::RichText::new(tr!("project.title")).font(theme::headline()));
                ui.add_space(space::S8);
                egui::Grid::new("project_settings_grid")
                    .spacing([space::S8, space::S8])
                    .show(ui, |ui| {
                        ui.label(tr!("project.size"));
                        let mut size = SIZES
                            .iter()
                            .position(|(_, s)| *s == (draft.width, draft.height))
                            .unwrap_or(SIZES.len());
                        let names = SIZES.map(|s| s.0());
                        let d = Dropdown::new(&mut size, &names).placeholder(tr!("size.custom"));
                        if ui.add(d.width(FIELD).bounds(bounds)).changed() {
                            (draft.width, draft.height) = SIZES[size].1;
                        }
                        ui.end_row();

                        ui.label(tr!("project.resolution"));
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = space::S8;
                            let (w, h) =
                                (tr!("project.width_suffix"), tr!("project.height_suffix"));
                            for (v, suffix) in [(&mut draft.width, w), (&mut draft.height, h)] {
                                let mut f = *v as f64;
                                ui.add(
                                    DragField::new(&mut f)
                                        .suffix(suffix)
                                        .range(16.0..=8192.0)
                                        .decimals(0)
                                        .width(80.0),
                                );
                                // Even sizes: 4:2:0 encoders reject odd ones.
                                *v = (f as u32).max(16) & !1;
                            }
                        });
                        ui.end_row();

                        ui.label(tr!("project.aspect"));
                        ui.label(aspect(draft.width, draft.height));
                        ui.end_row();

                        ui.label(tr!("project.frame_rate"));
                        let mut rate = RATES
                            .iter()
                            .position(|(_, r)| *r == draft.frame_rate)
                            .unwrap_or(4);
                        let names = RATES.map(|r| tr!("unit.fps", rate = r.0));
                        let names = names.each_ref().map(String::as_str);
                        let d = Dropdown::new(&mut rate, &names).width(FIELD);
                        if ui.add(d.bounds(bounds)).changed() {
                            draft.frame_rate = RATES[rate].1;
                        }
                        ui.end_row();

                        ui.label(tr!("project.background"));
                        let name = tr!("project.background");
                        ui.allocate_ui(egui::vec2(FIELD, size::CONTROL), |ui| {
                            ui.add(ColorField::new(&mut draft.background, name))
                        });
                        ui.end_row();

                        ui.label(tr!("project.hdr_output"));
                        // Turning it off always works, so a project from another machine can.
                        let state = if self.hdr10 || draft.hdr_output {
                            State::Live
                        } else {
                            State::Disabled
                        };
                        ui.add(
                            Checkbox::new(&mut draft.hdr_output, tr!("project.hdr10")).state(state),
                        )
                        .on_hover_text(tr!("project.hdr_tooltip"))
                        .on_disabled_hover_text(tr!("project.hdr_no_encoder"));
                        ui.end_row();
                    });
                let clips = project
                    .sequence()
                    .tracks()
                    .iter()
                    .map(|t| t.clips().len())
                    .sum();
                if let Some(w) = refit_warning(old, draft, clips) {
                    ui.add_space(space::S8);
                    ui.label(w);
                }
                ui.add_space(space::S16);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = space::S8;
                    if ui.add(Button::primary(tr!("project.apply"))).clicked() {
                        if draft != old {
                            command = Some(Command::SetSettings(draft.clone()));
                        }
                        ui.close();
                    }
                    if ui.add(Button::secondary(tr!("prompt.cancel"))).clicked() {
                        ui.close();
                    }
                });
            });
        if modal.should_close() {
            self.draft = None;
        }
        command
    }
}

/// `w:h` in lowest terms, e.g. `16:9`.
fn aspect(w: u32, h: u32) -> String {
    let gcd = |mut a: u32, mut b: u32| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a.max(1)
    };
    let g = gcd(w, h);
    format!("{}:{}", w / g, h / g)
}

/// What changing `old` to `new` does to the timeline's `clips`, if anything.
fn refit_warning(old: &ProjectSettings, new: &ProjectSettings, clips: usize) -> Option<String> {
    if clips == 0 {
        return None;
    }
    let mut lines = Vec::new();
    if (old.width, old.height) != (new.width, new.height) {
        lines.push(
            if aspect(old.width, old.height) == aspect(new.width, new.height) {
                trn!("project.rescale", clips)
            } else {
                trn!("project.refit", clips)
            },
        );
    }
    if old.frame_rate != new.frame_rate {
        lines.push(trn!("project.retime", clips));
    }
    (!lines.is_empty()).then(|| lines.join("\n") + "\n" + tr!("project.undo_note"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::history::History;

    #[test]
    fn every_dropdown_menu_fits_inside_the_dialog_and_custom_isnt_a_choice() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let project = Project::default();
        let mut dialog = SettingsDialog::default();
        dialog.open(&project);
        // This snapshot covers menu geometry; encoder availability has separate tests.
        dialog.hdr10 = false;
        let mut fresh = true;
        let mut h = egui_kittest::Harness::builder()
            .with_size([1280.0, 720.0])
            .build_ui_state(
                move |ui, d: &mut SettingsDialog| {
                    if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                        d.show(ui, &project);
                    }
                },
                dialog,
            );
        h.run_steps(2);
        let area = |h: &egui_kittest::Harness<'_, SettingsDialog>| {
            h.ctx
                .memory(|m| m.area_rect(egui::Id::new("project_settings")))
                .unwrap()
        };
        let rate = tr!("unit.fps", rate = "30");
        for (l, r) in crate::test_util::menu_rects(&mut h, &["16:9 · 1080p", &rate]) {
            assert!(area(&h).contains_rect(r), "{l}: {r:?} in {:?}", area(&h));
        }
        // An edited size reads Custom, which the menu doesn't offer.
        h.state_mut().draft.as_mut().unwrap().width = 1000;
        h.run();
        h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Custom")
            .click();
        h.run();
        assert_eq!(h.query_all_by_label("Custom").count(), 1, "the field alone");
        h.snapshot("project_settings_size_menu");
    }

    #[test]
    fn aspect_reduces() {
        assert_eq!(aspect(1920, 1080), "16:9");
        assert_eq!(aspect(1080, 1920), "9:16");
        assert_eq!(aspect(1080, 1080), "1:1");
    }

    #[test]
    fn warns_only_when_clips_are_affected() {
        let old = ProjectSettings::default();
        let bg = ProjectSettings {
            background: [9, 9, 9, 255],
            ..old.clone()
        };
        assert_eq!(refit_warning(&old, &bg, 3), None);
        let square = ProjectSettings {
            width: 1080,
            height: 1080,
            ..old.clone()
        };
        assert_eq!(refit_warning(&old, &square, 0), None);
        assert!(
            refit_warning(&old, &square, 1)
                .unwrap()
                .contains("letterbox")
        );
        let fourk = ProjectSettings {
            width: 3840,
            height: 2160,
            frame_rate: Rate::new(60, 1),
            ..old.clone()
        };
        let w = refit_warning(&old, &fourk, 2).unwrap();
        assert!(w.contains("rescales") && w.contains("between frames"));
    }

    #[test]
    fn applied_settings_undo() {
        let mut h = History::new(Project::default());
        let new = ProjectSettings {
            width: 1080,
            height: 1920,
            frame_rate: Rate::new(24, 1),
            sample_rate: 48_000,
            background: [255, 0, 0, 255],
            hdr_output: true,
        };
        h.apply(Command::SetSettings(new.clone())).unwrap();
        assert_eq!(h.project().settings(), &new);
        h.undo();
        assert_eq!(h.project().settings(), &ProjectSettings::default());
        h.redo();
        assert_eq!(h.project().settings(), &new);
    }
}
