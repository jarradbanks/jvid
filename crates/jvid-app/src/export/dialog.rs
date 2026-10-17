//! The export dialog: preset, range, loudness and output path, gathered into a [`Request`].

use std::collections::HashSet;
use std::path::PathBuf;

use egui::Ui;
use jvid_core::model::{ClipId, Project, Sequence};
use jvid_core::preset::{AudioCodec, Preset, VideoCodec};
use jvid_core::time::{FLICKS_PER_SECOND, Rate, Time, format_timecode};
use rfd::FileDialog;
use suite_ui::theme;
use suite_ui::tokens::{radius, space};
use suite_ui::widgets::{Button, Dropdown, State};

use super::Request;

/// Which part of the timeline to export.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Range {
    Entire,
    InOut,
    Selected,
}

/// Choices that persist between exports.
pub struct ExportDialog {
    pub presets: Vec<Preset>,
    /// Index into `presets`.
    preset: usize,
    /// Loudness target in LUFS; `None` leaves the mix alone.
    loudness: Option<f64>,
    /// Output file; kept between opens so the next export lands in the same folder.
    path: PathBuf,
    /// `Some` while the dialog is open.
    range: Option<Range>,
}

impl Default for ExportDialog {
    fn default() -> Self {
        Self {
            presets: super::presets(),
            preset: 0,
            loudness: Some(-14.0),
            path: PathBuf::new(),
            range: None,
        }
    }
}

impl ExportDialog {
    /// Opens on In to Out when a mark is set, else the entire timeline; the file is named `name`
    /// next to the last export.
    pub fn open(&mut self, seq: &Sequence, name: &str) {
        let range = if seq.in_point().is_some() || seq.out_point().is_some() {
            Range::InOut
        } else {
            Range::Entire
        };
        let ext = &self.presets[self.preset].extension;
        let dir = match self.path.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_owned(),
            _ => std::env::current_dir().unwrap_or_default(),
        };
        self.path = dir.join(format!("{name}.{ext}"));
        self.range = Some(range);
    }

    /// Shows the dialog while open; returns the request when the user clicks Export or Add to
    /// Queue, with `true` for Export, which also shows the queue.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        selected: &HashSet<ClipId>,
    ) -> Option<(Request, bool)> {
        let range = self.range.as_mut()?;
        let path = &mut self.path;
        let seq = project.sequence();
        let s = project.settings();
        let mut request = None;
        let modal = egui::Modal::new(egui::Id::new("export_dialog"))
            .frame(
                egui::Frame::popup(ui.style())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(space::S16),
            )
            .show(ui.ctx(), |ui| {
                ui.set_width(420.0);
                ui.label(egui::RichText::new("Export").font(theme::headline()));
                ui.add_space(space::S8);
                egui::Grid::new("export_grid")
                    .spacing([space::S8, space::S8])
                    .show(ui, |ui| {
                        ui.label("Preset");
                        let names: Vec<&str> = self.presets.iter().map(|p| &*p.name).collect();
                        let before = self.preset;
                        ui.add(Dropdown::new(&mut self.preset, &names).width(260.0));
                        let preset = &self.presets[self.preset];
                        if self.preset != before {
                            path.set_extension(&preset.extension);
                        }
                        ui.end_row();

                        ui.label("Video");
                        ui.horizontal(|ui| {
                            ui.label(video_summary(preset, s.width, s.height, s.frame_rate));
                            if let Some(w) = preset.aspect_warning((s.width, s.height)) {
                                ui.label("⚠").on_hover_text(w);
                            }
                        });
                        ui.end_row();

                        ui.label("Audio");
                        ui.label(preset.audio.map_or("None", audio_name));
                        ui.end_row();

                        ui.label("Loudness");
                        let enabled = preset.audio.is_some();
                        ui.add_enabled_ui(enabled, |ui| loudness_picker(ui, &mut self.loudness));
                        ui.end_row();

                        ui.label("Range");
                        ui.horizontal(|ui| {
                            let marked = seq.in_point().is_some() || seq.out_point().is_some();
                            ui.radio_value(range, Range::Entire, "Entire timeline");
                            ui.add_enabled_ui(marked, |ui| {
                                ui.radio_value(range, Range::InOut, "In to Out")
                                    .on_disabled_hover_text("Set In or Out on the timeline first")
                            });
                            ui.add_enabled_ui(!selected.is_empty(), |ui| {
                                ui.radio_value(range, Range::Selected, "Selected clips")
                                    .on_disabled_hover_text("Select clips on the timeline first")
                            });
                        });
                        ui.end_row();

                        let span = span(*range, seq, selected);
                        let rate = s.frame_rate;
                        let tc = |t: Time| format_timecode(rate.index_floor(t).max(0) as u64, rate);
                        let (a, b) = span.unwrap_or_default();
                        for (label, t) in [("In", a), ("Out", b), ("Duration", b - a)] {
                            ui.label(label);
                            ui.monospace(tc(t));
                            ui.end_row();
                        }

                        ui.label("Output");
                        ui.horizontal(|ui| {
                            let mut text = path.display().to_string();
                            if ui
                                .add(egui::TextEdit::singleline(&mut text).desired_width(260.0))
                                .changed()
                            {
                                *path = text.into();
                            }
                            if ui.add(Button::secondary("Browse…")).clicked()
                                && let Some(p) = FileDialog::new()
                                    .set_title("Export")
                                    .add_filter(&preset.name, &[&preset.extension])
                                    .set_directory(path.parent().unwrap_or(&PathBuf::new()))
                                    .set_file_name(
                                        path.file_name().unwrap_or_default().to_string_lossy(),
                                    )
                                    .save_file()
                            {
                                *path = p;
                            }
                        });
                        ui.end_row();

                        ui.label("Estimated size");
                        ui.label(format_size(estimate(
                            preset,
                            (s.width, s.height),
                            rate,
                            b - a,
                        )));
                        ui.end_row();
                    });
                ui.add_space(space::S16);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = space::S8;
                    let span = span(*range, seq, selected).filter(|(a, b)| b > a);
                    let ok = span.is_some() && path.file_name().is_some();
                    let state = if ok { State::Live } else { State::Disabled };
                    let export = ui.add(Button::primary("Export").state(state)).clicked();
                    let queue = ui
                        .add(Button::secondary("Add to Queue").state(state))
                        .clicked();
                    if (export || queue)
                        && let Some(range) = span
                    {
                        let req = Request {
                            path: path.clone(),
                            range,
                            preset: self.presets[self.preset].clone(),
                            loudness: self.loudness,
                        };
                        request = Some((req, export));
                        ui.close();
                    }
                    if ui.add(Button::secondary("Cancel")).clicked() {
                        ui.close();
                    }
                });
            });
        if modal.should_close() {
            self.range = None;
        }
        request
    }
}

fn loudness_picker(ui: &mut Ui, loudness: &mut Option<f64>) {
    let label = |l: Option<f64>| l.map_or("Off".into(), |l| format!("{l} LUFS"));
    egui::ComboBox::from_id_salt("export loudness")
        .selected_text(label(*loudness))
        .show_ui(ui, |ui| {
            for l in [None, Some(-14.0), Some(-16.0), Some(-23.0)] {
                ui.selectable_value(loudness, l, label(l));
            }
            let mut custom = loudness.unwrap_or(-14.0);
            let drag = egui::DragValue::new(&mut custom)
                .range(-40.0..=-5.0)
                .speed(0.1)
                .prefix("Custom ")
                .suffix(" LUFS");
            if ui.add(drag).changed() {
                *loudness = Some(custom);
            }
        });
}

/// The half-open timeline span `range` covers, or `None` when it covers nothing.
pub fn span(range: Range, seq: &Sequence, selected: &HashSet<ClipId>) -> Option<(Time, Time)> {
    match range {
        Range::Entire => Some((Time(0), seq.duration())),
        Range::InOut => Some((
            seq.in_point().unwrap_or(Time(0)),
            seq.out_point().unwrap_or(seq.duration()),
        )),
        Range::Selected => {
            let clips = seq.tracks().iter().flat_map(|t| t.clips());
            let clips: Vec<_> = clips.filter(|c| selected.contains(&c.id)).collect();
            Some((
                clips.iter().map(|c| c.start).min()?,
                clips.iter().map(|c| c.end()).max()?,
            ))
        }
    }
}

fn video_summary(p: &Preset, w: u32, h: u32, rate: Rate) -> String {
    let Some(v) = p.video else {
        return "None".into();
    };
    let (w, h) = p.output_size((w, h));
    let rate = p.frame_rate.unwrap_or(rate);
    let fps = rate.num as f64 / rate.den as f64;
    let codec = match v {
        VideoCodec::H264 => "H.264",
        VideoCodec::Hevc => "HEVC",
        VideoCodec::ProRes => "ProRes",
        VideoCodec::Gif => "GIF",
    };
    format!("{codec} · {w}×{h} · {fps:.3} fps").replace(".000 fps", " fps")
}

fn audio_name(a: AudioCodec) -> &'static str {
    match a {
        AudioCodec::Aac => "AAC stereo",
        AudioCodec::Mp3 => "MP3 stereo",
        AudioCodec::Pcm => "PCM 16-bit stereo",
        AudioCodec::Pcm24 => "PCM 24-bit stereo",
    }
}

/// Rough output size in bytes for `len` of the timeline.
/// shortcut: fixed bits-per-pixel per codec, ignores crf and content; upgrade by sampling a few
/// encoded frames if users rely on it.
pub fn estimate(p: &Preset, (w, h): (u32, u32), rate: Rate, len: Time) -> u64 {
    let secs = len.0.max(0) as f64 / FLICKS_PER_SECOND as f64;
    let video = p.video.map_or(0.0, |v| {
        let (w, h) = p.output_size((w, h));
        let r = p.frame_rate.unwrap_or(rate);
        let bpp = match v {
            VideoCodec::H264 => 0.1,
            VideoCodec::Hevc => 0.06,
            VideoCodec::ProRes => 2.0,
            VideoCodec::Gif => 0.5,
        };
        w as f64 * h as f64 * r.num as f64 / r.den as f64 * bpp
    });
    let audio = p.audio.map_or(0.0, |a| match a {
        AudioCodec::Aac | AudioCodec::Mp3 => 192_000.0,
        AudioCodec::Pcm => 1_536_000.0,
        AudioCodec::Pcm24 => 2_304_000.0,
    });
    ((video + audio) * secs / 8.0) as u64
}

fn format_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("≈ {:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("≈ {:.0} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("≈ {} KB", b.div_ceil(1024)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::history::{Command, Created, History};
    use jvid_core::model::{NewClip, TrackKind};

    fn secs(s: i64) -> Time {
        Time(s * FLICKS_PER_SECOND)
    }

    /// Clips at 0-4 s and 6-10 s, In at 2 s; returns the second clip's id.
    fn marked() -> (History, ClipId) {
        let mut h = History::new(Project::default());
        let Ok(Created::Asset(asset)) = h.apply(Command::AddAsset("a.mp4".into())) else {
            panic!("asset")
        };
        let (kind, name) = (TrackKind::Video, "v".into());
        let Ok(Created::Track(track)) = h.apply(Command::AddTrack { kind, name }) else {
            panic!("track")
        };
        let mut last = None;
        for start in [secs(0), secs(6)] {
            let clip = NewClip {
                asset,
                source_in: Time(0),
                source_out: secs(4),
                start,
                link: None,
            };
            if let Ok(Created::Clip(id)) = h.apply(Command::AddClip { track, clip }) {
                last = Some(id);
            }
        }
        let (in_point, out_point) = (Some(secs(2)), None);
        h.apply(Command::SetInOut {
            in_point,
            out_point,
        })
        .unwrap();
        (h, last.expect("clip"))
    }

    #[test]
    fn ranges_cover_marks_selection_and_timeline() {
        let (h, second) = marked();
        let seq = h.project().sequence();
        let none = HashSet::new();
        assert_eq!(span(Range::Entire, seq, &none), Some((secs(0), secs(10))));
        // A missing Out runs to the end of the timeline.
        assert_eq!(span(Range::InOut, seq, &none), Some((secs(2), secs(10))));
        assert_eq!(span(Range::Selected, seq, &none), None);
        let sel = HashSet::from([second]);
        assert_eq!(span(Range::Selected, seq, &sel), Some((secs(6), secs(10))));
    }

    #[test]
    fn opens_on_in_to_out_only_when_marked() {
        let (h, _) = marked();
        let mut d = ExportDialog {
            presets: jvid_core::preset::builtin(),
            ..Default::default()
        };
        d.open(h.project().sequence(), "cut");
        assert_eq!(d.range, Some(Range::InOut));
        assert_eq!(d.path.file_name().unwrap(), "cut.mp4");
        d.open(Project::default().sequence(), "cut");
        assert_eq!(d.range, Some(Range::Entire));
    }

    #[test]
    fn size_scales_with_length_and_formats() {
        let p = &jvid_core::preset::builtin()[0];
        let r = Rate::new(60, 1);
        let one = estimate(p, (1920, 1080), r, Time(FLICKS_PER_SECOND));
        assert_eq!(
            estimate(p, (1920, 1080), r, Time(10 * FLICKS_PER_SECOND)),
            one * 10
        );
        assert_eq!(format_size(0), "≈ 0 KB");
        assert_eq!(format_size(3 << 20), "≈ 3 MB");
    }
}
