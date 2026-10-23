//! The project settings dialog: resolution, frame rate and background, applied as one undoable edit.

use egui::Ui;
use jvid_core::history::Command;
use jvid_core::model::{Project, ProjectSettings};
use jvid_core::time::Rate;
use suite_ui::theme;
use suite_ui::tokens::{radius, space};
use suite_ui::widgets::{Button, Checkbox, DragField, Dropdown, State};

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

/// Size presets; picking one sets width and height, editing either shows Custom.
const SIZES: [(Label, Option<(u32, u32)>); 6] = [
    (|| tr!("size.custom"), None),
    (|| verbatim("16:9 · 1080p"), Some((1920, 1080))),
    (|| verbatim("16:9 · 1440p"), Some((2560, 1440))),
    (|| verbatim("16:9 · 4K"), Some((3840, 2160))),
    (|| tr!("size.vertical"), Some((1080, 1920))),
    (|| tr!("size.square"), Some((1080, 1080))),
];

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
                ui.label(egui::RichText::new(tr!("project.title")).font(theme::headline()));
                ui.add_space(space::S8);
                egui::Grid::new("project_settings_grid")
                    .spacing([space::S8, space::S8])
                    .show(ui, |ui| {
                        ui.label(tr!("project.size"));
                        let mut size = SIZES
                            .iter()
                            .position(|(_, s)| *s == Some((draft.width, draft.height)))
                            .unwrap_or(0);
                        if ui
                            .add(Dropdown::new(&mut size, &SIZES.map(|s| s.0())))
                            .changed()
                            && let Some((w, h)) = SIZES[size].1
                        {
                            (draft.width, draft.height) = (w, h);
                        }
                        ui.end_row();

                        ui.label(tr!("project.resolution"));
                        ui.horizontal(|ui| {
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
                        if ui
                            .add(Dropdown::new(
                                &mut rate,
                                &names.each_ref().map(String::as_str),
                            ))
                            .changed()
                        {
                            draft.frame_rate = RATES[rate].1;
                        }
                        ui.end_row();

                        ui.label(tr!("project.background"));
                        let [r, g, b, a] = draft.background;
                        let mut c = egui::Color32::from_rgba_unmultiplied(r, g, b, a);
                        if ui.color_edit_button_srgba(&mut c).changed() {
                            draft.background = c.to_srgba_unmultiplied();
                        }
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
