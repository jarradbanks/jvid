//! Subtitle-only export. Reads editable timeline captions; never starts a media job.

use std::io::Write;
use std::path::{Path, PathBuf};

use egui::{Align, Layout, RichText, Ui};
use jvid_core::model::Project;
use jvid_core::srt;
use rfd::FileDialog;
use suite_ui::theme;
use suite_ui::tokens::{color, radius, space};
use suite_ui::widgets::Button;

use super::dialog::{Range, span};
use crate::i18n::{tr, verbatim};

#[derive(Default)]
pub(crate) struct SubtitleExport {
    open: bool,
    in_out: bool,
    name: String,
    error: Option<String>,
    #[cfg(test)]
    pub(crate) test_destination: Option<Option<PathBuf>>,
}

impl SubtitleExport {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, name: &str) {
        self.open = true;
        self.in_out = false;
        self.name = format!("{name}.srt");
        self.error = None;
    }

    /// A successful save returns its destination for the app's confirmation toast.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        project_path: Option<&Path>,
    ) -> Option<PathBuf> {
        if !self.open {
            return None;
        }
        let (mut save, mut close) = (false, false);
        let modal = egui::Modal::new(egui::Id::new("export_subtitles"))
            .frame(
                egui::Frame::popup(ui.style())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(space::S16),
            )
            .show(ui.ctx(), |ui| {
                ui.set_width(400.0);
                ui.label(RichText::new(tr!("action.export_subtitles")).font(theme::headline()));
                ui.add_space(space::S8);
                ui.label(tr!("subtitle_export.scope"));
                ui.add_space(space::S8);
                ui.radio_value(&mut self.in_out, false, tr!("export.entire"));
                let seq = project.sequence();
                let marked = seq.in_point().is_some() || seq.out_point().is_some();
                ui.add_enabled_ui(marked, |ui| {
                    ui.radio_value(&mut self.in_out, true, tr!("export.in_out"));
                })
                .response
                .on_disabled_hover_text(tr!("export.in_out_off"));
                let content = contents(project, self.in_out);
                if let Err(error) = &content {
                    ui.label(RichText::new(error).color(color::amber()));
                }
                if let Some(error) = &self.error {
                    ui.label(RichText::new(error).color(color::red()));
                }
                ui.add_space(space::S16);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    save = ui
                        .add_enabled(
                            content.is_ok(),
                            Button::primary(tr!("subtitle_export.save")),
                        )
                        .clicked();
                    close = ui.add(Button::secondary(tr!("prompt.cancel"))).clicked();
                });
            });
        if close || modal.should_close() {
            self.open = false;
            return None;
        }
        if save {
            #[cfg(test)]
            let choice = self.test_destination.take();
            return self.save_with(project, project_path, |name| {
                #[cfg(test)]
                if let Some(path) = choice {
                    return path;
                }
                FileDialog::new()
                    .set_title(tr!("action.export_subtitles"))
                    .add_filter(verbatim("SubRip"), &["srt"])
                    .set_file_name(name)
                    .save_file()
            });
        }
        None
    }

    fn save_with(
        &mut self,
        project: &Project,
        project_path: Option<&Path>,
        choose: impl FnOnce(&str) -> Option<PathBuf>,
    ) -> Option<PathBuf> {
        self.error = None;
        // Validate before opening the native dialog, including an empty marked range.
        let result = contents(project, self.in_out).and_then(|text| {
            let Some(path) = choose(&self.name) else {
                return Ok(None);
            };
            write(project, project_path, &path, &text)?;
            Ok(Some(path))
        });
        match result {
            Ok(Some(path)) => {
                self.open = false;
                Some(path)
            }
            Ok(None) => None, // Native cancellation keeps the scope choices available.
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }
}

/// Uses precisely the video sidecar's cue traversal, half-open clipping and rebasing.
fn contents(project: &Project, in_out: bool) -> Result<String, String> {
    let seq = project.sequence();
    if in_out && seq.in_point().is_none() && seq.out_point().is_none() {
        return Err(tr!("export.in_out_off").into());
    }
    let range = if in_out { Range::InOut } else { Range::Entire };
    let (from, to) = span(range, seq, &Default::default(), Default::default()).unwrap_or_default();
    if to <= from {
        return Err(tr!("subtitle_export.empty").into());
    }
    let text = srt::write(&project.subtitle_cues(), (from, to));
    if text.is_empty() {
        return Err(tr!("subtitle_export.empty").into());
    }
    Ok(text)
}

/// Stage and sync before replacement, preserving an existing file on write failure.
/// The native save dialog asks about replacement; the writer still protects project/media aliases.
fn write(
    project: &Project,
    project_path: Option<&Path>,
    path: &Path,
    text: &str,
) -> Result<(), String> {
    if !path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("srt"))
    {
        return Err(tr!("subtitle_export.extension").into());
    }
    let check = || {
        super::check_destination(project, path).map_err(|e| match e {
            super::Error::Failed(e) => e,
            super::Error::Cancelled => unreachable!(),
        })?;
        if let Some(saved) = project_path
            && (path == saved || same_file::is_same_file(path, saved).unwrap_or(false))
        {
            return Err(tr!("subtitle_export.project_collision").into());
        }
        match std::fs::symlink_metadata(path) {
            Ok(meta) if !meta.is_file() => Err(tr!("export.sidecar_file_target").into()),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        }
    };
    check()?;
    let files = super::AttemptFiles::new(path)?;
    let result = (|| {
        let mut file = std::fs::File::create(&files.output)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        check().map_err(std::io::Error::other)?;
        std::fs::rename(&files.output, path)
    })();
    result.map_err(|e| tr!("export.publish_error", path = path.display(), error = e))
}

#[cfg(test)]
mod tests;
