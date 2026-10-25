//! The export dialog: preset, range, loudness and output path, gathered into a [`Request`].

use std::collections::HashSet;
use std::path::PathBuf;

use egui::{Align, FontId, Layout, Margin, Rect, RichText, Ui, vec2};
use jvid_core::model::{ClipId, Marker, MarkerId, Project, Sequence};
use jvid_core::preset::{AudioCodec, Preset, VideoCodec};
use jvid_core::time::{FLICKS_PER_SECOND, Rate, Time, format_timecode};
use jvid_media::remux::{Cut, SMART_ENCODER};
use rfd::FileDialog;
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, radius, size, space};
use suite_ui::widgets::{Button, Checkbox, DragField, Dropdown, Segmented, State, TextInput};

use super::lossless::{self, Lossless};
use super::{Request, preset_name};
use crate::i18n::{tr, verbatim};

/// Which part of the timeline to export.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Range {
    Entire,
    InOut,
    Selected,
    /// Each chosen marker as its own file; a point marker spans Before to After around it.
    Highlights,
}

/// The default highlight file name, before the extension.
pub const TEMPLATE: &str = "{project}-{marker}-{index}";

/// Choices that persist between exports.
pub struct ExportDialog {
    pub presets: Vec<Preset>,
    /// Index into `presets`.
    preset: usize,
    /// Loudness target in LUFS; `None` leaves the mix alone.
    loudness: Option<f64>,
    /// Copy instead of render, when the range allows it.
    lossless: Option<Lossless>,
    /// Target file size in MB (10^6 bytes); `None` encodes at the preset's quality.
    target: Option<u64>,
    /// Subtitle tracks go to an `.srt` sidecar instead of being burnt in.
    sidecar: bool,
    /// Output file; kept between opens so the next export lands in the same folder.
    path: PathBuf,
    /// `Some` while the dialog is open.
    range: Option<Range>,
    /// Project name for the highlight template.
    name: String,
    /// Markers left out of a highlights export.
    skipped: HashSet<MarkerId>,
    /// Seconds a point marker's highlight starts before and ends after it.
    before: f64,
    after: f64,
    /// Highlight file name: `{project}`, `{marker}` and `{index}` are filled in.
    template: String,
}

impl Default for ExportDialog {
    fn default() -> Self {
        Self {
            presets: super::presets(),
            preset: 0,
            loudness: Some(-14.0),
            lossless: None,
            target: None,
            sidecar: false,
            path: PathBuf::new(),
            range: None,
            name: String::new(),
            skipped: HashSet::new(),
            before: 15.0,
            after: 5.0,
            template: TEMPLATE.into(),
        }
    }
}

impl ExportDialog {
    /// Whether the export modal currently owns input, including before its first draw.
    pub fn is_open(&self) -> bool {
        self.range.is_some()
    }

    /// Sets the output file, whose folder the next `open` keeps; for tests that need a fixed path.
    #[doc(hidden)]
    pub fn set_path(&mut self, path: PathBuf) {
        self.path = path;
    }

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
        self.name = name.into();
    }

    /// What Export would queue for the entire timeline in the chosen format, without showing
    /// the dialog: Show Me's export. The file is named as by `open`.
    pub fn entire(&mut self, seq: &Sequence, name: &str) -> Request {
        self.open(seq, name);
        self.range = None;
        Request {
            path: self.path.clone(),
            range: (Time(0), seq.duration()),
            preset: self.presets[self.preset].clone(),
            loudness: self.loudness,
            lossless: None,
            sidecar: self.sidecar,
        }
    }

    /// What Add to Queue would give for Highlights in the chosen format, without showing the
    /// dialog: Show Me's highlights export. Files are named by the template beside `open`'s.
    pub fn highlights(&mut self, seq: &Sequence, name: &str) -> Vec<Request> {
        self.open(seq, name);
        self.range = None;
        let preset = &self.presets[self.preset];
        let dir = self.path.parent().unwrap_or(std::path::Path::new(""));
        let chosen = chosen(seq, &self.skipped);
        let reqs = chosen.iter().enumerate().map(|(i, m)| {
            let file = file_name(&self.template, &self.name, m, i + 1);
            Request {
                path: dir.join(format!("{file}.{}", preset.extension)),
                range: highlight(m, self.pad(), seq.duration()),
                preset: preset.clone(),
                loudness: self.loudness,
                lossless: None,
                sidecar: self.sidecar,
            }
        });
        reqs.collect()
    }

    /// Before and After, the padding around a point marker's highlight.
    pub fn pad(&self) -> (Time, Time) {
        (seconds(self.before), seconds(self.after))
    }

    /// Shows the dialog while open; returns the request when the user clicks Export or Add to
    /// Queue, with `true` for Export; Highlights gives one request per chosen marker.
    ///
    /// Format, Range and Output groups in one column with the inspector's row layout; the body
    /// scrolls when it outgrows the window and the footer, with Estimated Size, stays put.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        selected: &HashSet<ClipId>,
    ) -> Option<(Vec<Request>, bool)> {
        let range = self.range.as_mut()?;
        let path = &mut self.path;
        let seq = project.sequence();
        let s = project.settings();
        let pad = (seconds(self.before), seconds(self.after));
        let mut request = None;
        // 16px clear of the window above and below.
        let max_body = ui.ctx().content_rect().height() - space::S32 - HEAD - FOOT;
        let modal = egui::Modal::new(egui::Id::new("export_dialog"))
            .frame(
                egui::Frame::popup(ui.style())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(0),
            )
            .show(ui.ctx(), |ui| {
                ui.set_width(WIDTH);
                ui.spacing_mut().item_spacing = vec2(space::S4, space::S4);
                let head = Margin {
                    left: 16,
                    right: 16,
                    top: 16,
                    bottom: 8,
                };
                egui::Frame::NONE.inner_margin(head).show(ui, |ui| {
                    let title = RichText::new(tr!("export.export")).font(theme::headline());
                    ui.label(title.color(color::ink()));
                });
                let body = Margin {
                    left: 16,
                    right: 12,
                    top: 0,
                    bottom: 4,
                };
                egui::ScrollArea::vertical()
                    .max_height(max_body.max(120.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        // Menus stay in the body, clear of the head and the buttons.
                        let bounds = ui.clip_rect();
                        egui::Frame::NONE.inner_margin(body).show(ui, |ui| {
                            ui.spacing_mut().item_spacing = vec2(space::S4, space::S4);
                            section(ui, tr!("export.format"), true);
                            row(ui, tr!("export.preset"), |ui| {
                                let names: Vec<&str> = self
                                    .presets
                                    .iter()
                                    .enumerate()
                                    .map(|(i, p)| preset_name(i, p))
                                    .collect();
                                let before = self.preset;
                                let w = ui.available_width();
                                let d = Dropdown::new(&mut self.preset, &names).bounds(bounds);
                                ui.add(d.width(w));
                                if self.preset != before {
                                    path.set_extension(&self.presets[self.preset].extension);
                                }
                            });
                            let preset = &self.presets[self.preset];
                            let shown = if s.hdr_output {
                                &preset.clone().hdr()
                            } else {
                                preset
                            };
                            let video = video_summary(shown, s.width, s.height, s.frame_rate);
                            row(ui, tr!("export.video"), |ui| readout(ui, &video));
                            let name = preset_name(self.preset, preset);
                            if let Some(w) = aspect_warning(name, preset, (s.width, s.height)) {
                                note(ui, Note::Warn, &w);
                            }
                            let audio = preset.audio.map_or(tr!("export.none"), audio_name);
                            row(ui, tr!("inspector.audio"), |ui| readout(ui, audio));
                            row(ui, tr!("export.loudness"), |ui| {
                                ui.add_enabled_ui(preset.audio.is_some(), |ui| {
                                    loudness_picker(ui, &mut self.loudness, bounds)
                                });
                            });
                            if seq.tracks().iter().any(|t| t.subtitles) {
                                row(ui, tr!("app.subtitles"), |ui| {
                                    let mut i = self.sidecar as usize;
                                    let options = [tr!("export.burn_in"), tr!("export.sidecar")];
                                    if ui.add(Segmented::new(&mut i, &options)).changed() {
                                        self.sidecar = i == 1;
                                    }
                                });
                            }

                            section(ui, tr!("export.range"), false);
                            row(ui, tr!("export.range"), |ui| {
                                let marked = seq.in_point().is_some() || seq.out_point().is_some();
                                let none = highlights(seq).next().is_none();
                                let off = [
                                    None,
                                    (!marked).then_some(tr!("export.in_out_off")),
                                    selected.is_empty().then_some(tr!("export.selected_off")),
                                    none.then_some(tr!("export.highlights_off")),
                                ];
                                let mut i = RANGES.iter().position(|r| r == range).unwrap_or(0);
                                let options = [
                                    tr!("export.entire"),
                                    tr!("export.in_out"),
                                    tr!("export.selected"),
                                    tr!("export.highlights"),
                                ];
                                if ui.add(Segmented::new(&mut i, &options).off(&off)).changed() {
                                    *range = RANGES[i];
                                }
                            });
                            let (a, b) = span(*range, seq, selected, pad).unwrap_or_default();
                            let rate = s.frame_rate;
                            let tc =
                                |t: Time| format_timecode(rate.index_floor(t).max(0) as u64, rate);
                            row(ui, "", |ui| {
                                let pairs = [
                                    (tr!("export.in"), a),
                                    (tr!("export.out"), b),
                                    (tr!("bin.sort_duration"), b - a),
                                ];
                                for (i, (label, t)) in pairs.into_iter().enumerate() {
                                    if i > 0 {
                                        ui.add_space(space::S4);
                                    }
                                    ui.label(RichText::new(label).color(color::ink_3()));
                                    ui.label(RichText::new(tc(t)).color(color::ink_2()));
                                }
                            });
                            if *range == Range::Highlights {
                                row(ui, tr!("export.highlights"), |ui| {
                                    ui.vertical(|ui| {
                                        for m in highlights(seq) {
                                            let mut on = !self.skipped.contains(&m.id);
                                            ui.horizontal(|ui| {
                                                if ui.add(Checkbox::new(&mut on, &m.name)).changed()
                                                {
                                                    if on {
                                                        self.skipped.remove(&m.id);
                                                    } else {
                                                        self.skipped.insert(m.id);
                                                    }
                                                }
                                                let (a, b) = highlight(m, pad, seq.duration());
                                                let secs =
                                                    (b - a).0 as f64 / FLICKS_PER_SECOND as f64;
                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        let d = super::queue::clock(secs);
                                                        ui.label(
                                                            RichText::new(d).color(color::ink_3()),
                                                        );
                                                    },
                                                );
                                            });
                                        }
                                    });
                                });
                                for (label, v) in [
                                    (tr!("export.before"), &mut self.before),
                                    (tr!("export.after"), &mut self.after),
                                ] {
                                    row(ui, label, |ui| {
                                        let f = DragField::new(v)
                                            .range(0.0..=600.0)
                                            .speed(0.1)
                                            .decimals(1)
                                            .suffix(tr!("silence.seconds_suffix"));
                                        ui.add(f.width(ui.available_width()));
                                    });
                                }
                                row(ui, tr!("export.file_names"), |ui| {
                                    let w = ui.available_width();
                                    ui.add(TextInput::new(&mut self.template).width(w));
                                });
                                let help = tr!(
                                    "export.file_names_tooltip",
                                    project = verbatim("{project}"),
                                    marker = verbatim("{marker}"),
                                    index = verbatim("{index}"),
                                );
                                note(ui, Note::Help, &help);
                            }

                            section(ui, tr!("export.output"), false);
                            row(ui, tr!("export.output"), |ui| {
                                let error = path
                                    .file_name()
                                    .is_none()
                                    .then(|| tr!("export.no_file_name_row"));
                                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                                    if ui.add(Button::secondary(tr!("export.browse"))).clicked()
                                        && let Some(p) = FileDialog::new()
                                            .set_title(tr!("export.export"))
                                            .add_filter(name, &[&preset.extension])
                                            .set_directory(path.parent().unwrap_or(&PathBuf::new()))
                                            .set_file_name(
                                                path.file_name()
                                                    .unwrap_or_default()
                                                    .to_string_lossy(),
                                            )
                                            .save_file()
                                    {
                                        *path = p;
                                    }
                                    let mut text = path.display().to_string();
                                    let w = ui.available_width();
                                    let field = TextInput::new(&mut text).width(w).error(error);
                                    if ui.add(field).changed() {
                                        *path = text.into();
                                    }
                                });
                            });
                            let copyable = lossless::check(project, (a, b));
                            row(ui, tr!("export.lossless"), |ui| {
                                lossless_picker(ui, &mut self.lossless, &copyable)
                            });
                            if let Err(why) = &copyable {
                                // No "<label>: " prefix: it would force the reason's capital.
                                note(ui, Note::Help, &why[0]).on_hover_text(why.join("\n"));
                            }
                            if let (Some(_), Ok(cuts)) = (self.lossless, &copyable)
                                && let Some(ext) = cuts[0].path.extension()
                            {
                                path.set_extension(ext);
                            }
                            let sizable =
                                matches!(preset.video, Some(VideoCodec::H264 | VideoCodec::Hevc));
                            row(ui, tr!("export.target_size"), |ui| {
                                ui.add_enabled_ui(sizable, |ui| {
                                    target_picker(ui, &mut self.target, bounds)
                                });
                            });
                            let secs = (b - a).0 as f64 / FLICKS_PER_SECOND as f64;
                            let warning = match self.target.filter(|_| sizable && secs > 0.0) {
                                Some(mb) => match super::target_bitrate(preset, mb * MB, secs) {
                                    Some(bits) => {
                                        too_small(preset, (s.width, s.height), rate, bits)
                                    }
                                    None => Some(tr!("export.too_small_audio").into()),
                                },
                                None => None,
                            };
                            if !sizable {
                                note(ui, Note::Help, tr!("export.target_size_off"));
                            } else if let Some(w) = warning {
                                note(ui, Note::Warn, &w);
                            }
                        });
                    });

                rule(ui);
                let foot = Margin {
                    left: 16,
                    right: 16,
                    top: 12,
                    bottom: 16,
                };
                egui::Frame::NONE.inner_margin(foot).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = space::S8;
                        let preset = &self.presets[self.preset];
                        let sizable =
                            matches!(preset.video, Some(VideoCodec::H264 | VideoCodec::Hevc));
                        let span = span(*range, seq, selected, pad).filter(|(a, b)| b > a);
                        let size = match (span, self.target.filter(|_| sizable)) {
                            (None, _) => verbatim("–").to_owned(),
                            (Some(_), Some(mb)) => tr!("export.under_mb", mb = mb),
                            (Some((a, b)), None) => format_size(estimate(
                                preset,
                                (s.width, s.height),
                                s.frame_rate,
                                b - a,
                            )),
                        };
                        ui.label(RichText::new(tr!("export.estimated_size")).color(color::ink_3()));
                        let size = RichText::new(size).font(theme::body_strong());
                        ui.label(size.color(color::ink()));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let chosen = chosen(seq, &self.skipped);
                            let kind = *range;
                            let ok = span.is_some()
                                && path.file_name().is_some()
                                && (kind != Range::Highlights || !chosen.is_empty());
                            let state = if ok { State::Live } else { State::Disabled };
                            let export = Button::primary(tr!("export.export"));
                            let export = ui.add(export.state(state).height(FOOT_BUTTON)).clicked();
                            let queue = Button::secondary(tr!("export.add_to_queue"));
                            let queue = ui.add(queue.state(state).height(FOOT_BUTTON)).clicked();
                            if (export || queue)
                                && let Some(range) = span
                            {
                                let mut preset = preset.clone();
                                if let Some(mb) = self.target.filter(|_| sizable) {
                                    preset
                                        .options
                                        .insert("target_size".into(), (mb * MB).to_string());
                                }
                                let req = |path: PathBuf, range| Request {
                                    path,
                                    range,
                                    preset: preset.clone(),
                                    loudness: self.loudness,
                                    lossless: self
                                        .lossless
                                        .filter(|_| lossless::check(project, range).is_ok()),
                                    sidecar: self.sidecar,
                                };
                                let reqs = if kind == Range::Highlights {
                                    let ext =
                                        path.extension().unwrap_or_default().to_string_lossy();
                                    let dir = path.parent().unwrap_or(std::path::Path::new(""));
                                    chosen
                                        .iter()
                                        .enumerate()
                                        .map(|(i, m)| {
                                            let name =
                                                file_name(&self.template, &self.name, m, i + 1);
                                            req(
                                                dir.join(format!("{name}.{ext}")),
                                                highlight(m, pad, seq.duration()),
                                            )
                                        })
                                        .collect()
                                } else {
                                    vec![req(path.clone(), range)]
                                };
                                request = Some((reqs, export));
                                ui.close();
                            }
                            let cancel =
                                Button::secondary(tr!("prompt.cancel")).height(FOOT_BUTTON);
                            if ui.add(cancel).clicked() {
                                ui.close();
                            }
                        });
                    });
                });
            });
        if modal.should_close() {
            self.range = None;
        }
        request
    }
}

/// The dialog's width; its head and footer heights, which the body scrolls between.
const WIDTH: f32 = 480.0;
const HEAD: f32 = 46.0;
const FOOT: f32 = 57.0;
const FOOT_BUTTON: f32 = 28.0;
/// The rows' label column.
const LABEL: f32 = 96.0;
/// The Range options, in their segmented control's order.
const RANGES: [Range; 4] = [
    Range::Entire,
    Range::InOut,
    Range::Selected,
    Range::Highlights,
];

/// A group's head: Body Strong in a 28px row, with a rule above every group but the first.
fn section(ui: &mut Ui, title: &str, first: bool) {
    if !first {
        ui.add_space(space::S4);
        rule(ui);
    }
    let w = ui.available_width();
    ui.allocate_ui_with_layout(vec2(w, 28.0), Layout::left_to_right(Align::Center), |ui| {
        let t = RichText::new(title).font(theme::body_strong());
        ui.label(t.color(color::ink()));
    });
}

/// A row: the label in a 96px column, then the controls, all top-aligned on 24px.
fn row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            vec2(LABEL, size::CONTROL),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_width(LABEL);
                let l = egui::Label::new(RichText::new(label).color(color::ink_2())).truncate();
                ui.add(l);
            },
        );
        add(ui)
    })
    .inner
}

/// A read-only value, centred on the row's 24px.
fn readout(ui: &mut Ui, text: &str) {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(
        vec2(w, size::CONTROL),
        Layout::left_to_right(Align::Center),
        |ui| ui.add(egui::Label::new(RichText::new(text).color(color::ink_2())).truncate()),
    );
}

enum Note {
    Help,
    Warn,
}

/// A line written out under its row, in the controls' column: help in Ink 3, a warning in Amber.
fn note(ui: &mut Ui, kind: Note, text: &str) -> egui::Response {
    ui.horizontal_top(|ui| {
        ui.add_space(LABEL + space::S4);
        let ink = match kind {
            Note::Help => color::ink_3(),
            Note::Warn => {
                let glyph = RichText::new(icon::CIRCLE_ALERT)
                    .font(FontId::new(14.0, theme::icon_font().family));
                ui.label(glyph.color(color::amber()));
                color::amber()
            }
        };
        ui.add(egui::Label::new(RichText::new(text).color(ink)).wrap())
    })
    .inner
}

/// A 1px Border line across the available width.
fn rule(ui: &mut Ui) {
    let line = 1.0 / ui.pixels_per_point();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), line), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, color::border());
}

fn loudness_picker(ui: &mut Ui, loudness: &mut Option<f64>, bounds: Rect) {
    let label =
        |l: Option<f64>| l.map_or(tr!("export.off").into(), |l| tr!("export.lufs", value = l));
    let presets = [None, Some(-14.0), Some(-16.0), Some(-23.0)];
    let field = |ui: &mut Ui, v: &mut f64, w| {
        let f = DragField::new(v).range(-40.0..=-5.0).speed(0.1).decimals(1);
        ui.add(f.suffix(verbatim(" LUFS")).width(w))
    };
    custom_picker(
        ui,
        "export_loudness",
        loudness,
        &presets,
        label,
        -18.0,
        field,
        bounds,
    );
}

/// Off, Copy or Smart Render; Copy and Smart Render are off when the range doesn't copy, with
/// the reason written under the row by the caller.
fn lossless_picker(
    ui: &mut Ui,
    mode: &mut Option<Lossless>,
    copyable: &Result<Vec<Cut>, Vec<String>>,
) {
    const MODES: [Option<Lossless>; 3] = [None, Some(Lossless::Copy), Some(Lossless::Smart)];
    let why = copyable.as_ref().err().map(|w| w[0].as_str());
    let off = [
        None,
        why,
        why.or(SMART_ENCODER.is_none().then(|| tr!("export.smart_off"))),
    ];
    let tips = [
        None,
        Some(tr!("export.copy_tooltip")),
        Some(tr!("export.smart_tooltip")),
    ];
    // Shown as Off while the range can't copy; the saved choice comes back when it can.
    let mut i = if why.is_some() {
        0
    } else {
        MODES.iter().position(|m| m == mode).unwrap_or(0)
    };
    let options = [tr!("export.off"), tr!("export.copy"), tr!("export.smart")];
    if ui
        .add(Segmented::new(&mut i, &options).off(&off).tips(&tips))
        .changed()
    {
        *mode = MODES[i];
    }
}

const MB: u64 = 1_000_000;

fn target_picker(ui: &mut Ui, target: &mut Option<u64>, bounds: Rect) {
    let label =
        |t: Option<u64>| t.map_or(tr!("export.off").into(), |t| tr!("export.mb", value = t));
    let presets = [None, Some(10), Some(25), Some(50), Some(100), Some(500)];
    let field = |ui: &mut Ui, v: &mut u64, w| {
        let mut mb = *v as f64;
        let f = DragField::new(&mut mb).range(1.0..=100_000.0).decimals(0);
        // The dropdown's "{value} MB" without the value, so both show the same unit.
        let unit = tr!("export.mb", value = "");
        let r = ui.add(f.suffix(&unit).width(w));
        *v = mb as u64;
        r
    };
    custom_picker(
        ui,
        "export_target_size",
        target,
        &presets,
        label,
        200,
        field,
        bounds,
    );
}

/// A dropdown of `presets`, then Custom, kept inside `bounds`. Custom, or a value no preset
/// holds, adds `field` beside it for any value, starting at `start`.
#[allow(clippy::too_many_arguments)]
fn custom_picker<T: Copy + PartialEq + Send + Sync + 'static>(
    ui: &mut Ui,
    salt: &str,
    value: &mut Option<T>,
    presets: &[Option<T>],
    label: impl Fn(Option<T>) -> String,
    start: T,
    field: impl FnOnce(&mut Ui, &mut T, f32) -> egui::Response,
    bounds: Rect,
) {
    // Custom stays picked while its field passes through a preset's value.
    let id = ui.id().with(salt);
    let picked = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let n = presets.len();
    let mut i = match presets.iter().position(|p| p == value) {
        Some(i) if !picked => i,
        _ => n,
    };
    let mut names: Vec<String> = presets.iter().map(|p| label(*p)).collect();
    names.push(tr!("export.custom").into());
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let w = ui.available_width();
    let custom_w = 96.0;
    let shown = if i == n { w - custom_w - space::S4 } else { w };
    let d = Dropdown::new(&mut i, &names).width(shown).bounds(bounds);
    if ui.add(d).changed() {
        if i < n {
            *value = presets[i];
        } else if value.is_none() {
            *value = Some(start);
        }
        ui.data_mut(|d| d.insert_temp(id, i == n));
    }
    if i == n
        && let Some(v) = value
    {
        field(ui, v, custom_w);
    }
}

/// Bits per pixel per frame below which H.264/HEVC turns to mush.
const MIN_BPP: f64 = 0.03;

/// A warning, with a smaller width that would hold up, when `bits` per second is too few for
/// `p`'s output size; `None` when it's enough.
pub fn too_small(p: &Preset, src: (u32, u32), rate: Rate, bits: u64) -> Option<String> {
    let (w, h) = p.output_size(src);
    let r = p.frame_rate.unwrap_or(rate);
    let per_frame = bits as f64 * r.den as f64 / r.num as f64;
    if per_frame / (w as f64 * h as f64) >= MIN_BPP {
        return None;
    }
    // Same aspect ratio, so pixels scale with width squared.
    let fits = [1920, 1280, 854, 640, 426].into_iter().find(|&nw| {
        nw < w && per_frame / (w as f64 * h as f64) * (w as f64 / nw as f64).powi(2) >= MIN_BPP
    });
    Some(match fits {
        Some(nw) => tr!(
            "export.too_small",
            size = format_args!("{w}×{h}"),
            width = nw
        ),
        None => tr!("export.too_small_any", size = format_args!("{w}×{h}")),
    })
}

/// The bit rate the encoder gives audio `a`, in bits per second.
pub fn audio_bitrate(a: AudioCodec) -> f64 {
    match a {
        AudioCodec::Aac | AudioCodec::Mp3 => 192_000.0,
        AudioCodec::Pcm => 1_536_000.0,
        AudioCodec::Pcm24 => 2_304_000.0,
    }
}

/// Markers, ranged and point, in timeline order.
pub fn highlights(seq: &Sequence) -> impl Iterator<Item = &Marker> {
    let mut all: Vec<_> = seq.markers().iter().collect();
    all.sort_by_key(|m| m.time);
    all.into_iter()
}

/// The span `m` exports: its own range, or `before` to `after` around a point marker, kept
/// within 0 to `end`.
pub fn highlight(m: &Marker, (before, after): (Time, Time), end: Time) -> (Time, Time) {
    if m.duration > Time(0) {
        return (m.time, m.time + m.duration);
    }
    ((m.time - before).max(Time(0)), (m.time + after).min(end))
}

fn seconds(s: f64) -> Time {
    Time((s * FLICKS_PER_SECOND as f64).round() as i64)
}

/// The highlights to export: every marker not in `skipped`.
pub fn chosen<'a>(seq: &'a Sequence, skipped: &HashSet<MarkerId>) -> Vec<&'a Marker> {
    highlights(seq)
        .filter(|m| !skipped.contains(&m.id))
        .collect()
}

/// Fills `template` for the `index`th (1-based) highlight; path separators and characters
/// Windows forbids become `-`.
pub fn file_name(template: &str, project: &str, marker: &Marker, index: usize) -> String {
    let name = template
        .replace("{project}", project)
        .replace("{marker}", &marker.name)
        .replace("{index}", &format!("{index:02}"));
    let clean = |c: char| match c {
        '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
        c if c.is_control() => '-',
        c => c,
    };
    name.chars()
        .map(clean)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The half-open timeline span `range` covers, or `None` when it covers nothing; `pad` is
/// Before and After for point-marker highlights.
pub fn span(
    range: Range,
    seq: &Sequence,
    selected: &HashSet<ClipId>,
    pad: (Time, Time),
) -> Option<(Time, Time)> {
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
        Range::Highlights => {
            let all: Vec<_> = highlights(seq)
                .map(|m| highlight(m, pad, seq.duration()))
                .collect();
            Some((
                all.iter().map(|r| r.0).min()?,
                all.iter().map(|r| r.1).max()?,
            ))
        }
    }
}

/// Why `p` would stretch the picture, when its size has another aspect than the project's.
fn aspect_warning(name: &str, p: &Preset, (w, h): (u32, u32)) -> Option<String> {
    p.aspect_warning((w, h))?;
    let [pw, ph] = p.size?;
    Some(tr!(
        "export.aspect",
        name = name,
        size = format_args!("{pw}×{ph}"),
        project = format_args!("{w}×{h}"),
    ))
}

fn video_summary(p: &Preset, w: u32, h: u32, rate: Rate) -> String {
    let Some(v) = p.video else {
        return tr!("export.none").into();
    };
    let (w, h) = p.output_size((w, h));
    let rate = p.frame_rate.unwrap_or(rate);
    let fps = rate.num as f64 / rate.den as f64;
    let codec = match v {
        VideoCodec::H264 => verbatim("H.264"),
        VideoCodec::Hevc if p.options.contains_key("hdr") => verbatim("HEVC HDR10"),
        VideoCodec::Hevc => verbatim("HEVC"),
        VideoCodec::ProRes => verbatim("ProRes"),
        VideoCodec::Gif => verbatim("GIF"),
    };
    let rate = format!("{fps:.3}");
    let rate = rate.strip_suffix(".000").unwrap_or(&rate);
    let fps = tr!("unit.fps", rate = rate);
    format!("{codec} · {w}×{h} · {fps}")
}

fn audio_name(a: AudioCodec) -> &'static str {
    match a {
        AudioCodec::Aac => tr!("export.aac"),
        AudioCodec::Mp3 => tr!("export.mp3"),
        AudioCodec::Pcm => tr!("export.pcm16"),
        AudioCodec::Pcm24 => tr!("export.pcm24"),
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
    let audio = p.audio.map_or(0.0, audio_bitrate);
    ((video + audio) * secs / 8.0) as u64
}

fn format_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => {
            let value = format!("{:.1}", b as f64 / (1u64 << 30) as f64);
            tr!("export.about_gb", value = value)
        }
        b if b >= 1 << 20 => {
            let value = format!("{:.0}", b as f64 / (1u64 << 20) as f64);
            tr!("export.about_mb", value = value)
        }
        b => tr!("export.about_kb", value = b.div_ceil(1024)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::history::{Command, Created, History};
    use jvid_core::model::{NewClip, ProjectSettings, TrackKind};

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
        let pad = (secs(15), secs(5));
        assert_eq!(
            span(Range::Entire, seq, &none, pad),
            Some((secs(0), secs(10)))
        );
        // A missing Out runs to the end of the timeline.
        assert_eq!(
            span(Range::InOut, seq, &none, pad),
            Some((secs(2), secs(10)))
        );
        assert_eq!(span(Range::Selected, seq, &none, pad), None);
        assert_eq!(span(Range::Highlights, seq, &none, pad), None);
        let sel = HashSet::from([second]);
        assert_eq!(
            span(Range::Selected, seq, &sel, pad),
            Some((secs(6), secs(10)))
        );
    }

    #[test]
    fn highlights_pad_points_skip_unchosen_and_fill_names() {
        let (mut h, _) = marked();
        for (t, d, name) in [(5, 1, "b"), (1, 0, "point"), (2, 2, "a/b")] {
            let (time, duration) = (secs(t), secs(d));
            let name = name.into();
            h.apply(Command::AddMarker {
                time,
                duration,
                name,
            })
            .unwrap();
        }
        let seq = h.project().sequence();
        let names: Vec<_> = highlights(seq).map(|m| &*m.name).collect();
        assert_eq!(names, ["point", "a/b", "b"]);
        // The point at 1 s starts at 0, not -1 s; ranged markers keep their own range.
        let pad = (secs(2), secs(3));
        let all: Vec<_> = highlights(seq)
            .map(|m| highlight(m, pad, seq.duration()))
            .collect();
        assert_eq!(
            all,
            [(secs(0), secs(4)), (secs(2), secs(4)), (secs(5), secs(6))]
        );
        assert_eq!(
            span(Range::Highlights, seq, &HashSet::new(), pad),
            Some((secs(0), secs(6)))
        );
        let skip = HashSet::from([highlights(seq).next().unwrap().id]);
        let left = chosen(seq, &skip);
        assert_eq!(left.len(), 2);
        assert_eq!(file_name(TEMPLATE, "Trip", left[1], 3), "Trip-b-03");
        assert_eq!(file_name("{marker}: {index}", "p", left[0], 1), "a-b- 01");
        // With the default 15 s Before and 5 s After, a point 3 s in exports 0 to 8 s.
        let d = ExportDialog::default();
        let pad = (seconds(d.before), seconds(d.after));
        let point = highlights(seq).next().unwrap();
        let three = Marker {
            time: secs(3),
            ..point.clone()
        };
        assert_eq!(highlight(&three, pad, seq.duration()), (secs(0), secs(8)));
        // And one near the end stops at the sequence's end.
        let nine = Marker {
            time: secs(9),
            ..point.clone()
        };
        assert_eq!(highlight(&nine, pad, seq.duration()), (secs(0), secs(10)));
        // Show Me's export: one request per chosen marker, as Add to Queue gives.
        let mut d = ExportDialog {
            presets: jvid_core::preset::builtin(),
            skipped: skip,
            ..Default::default()
        };
        let reqs = d.highlights(seq, "Trip");
        let got: Vec<_> = reqs.iter().map(|r| (r.path.clone(), r.range)).collect();
        let want: Vec<_> = (left.iter().enumerate())
            .map(|(i, m)| {
                let file = format!("{}.mp4", file_name(TEMPLATE, "Trip", m, i + 1));
                (
                    d.path.with_file_name(file),
                    highlight(m, pad, seq.duration()),
                )
            })
            .collect();
        assert_eq!(got, want);
        assert_eq!(d.range, None, "the dialog stays shut");
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
        assert_eq!(format_size(0), "~0 KB");
        assert_eq!(format_size(3 << 20), "~3 MB");
    }

    #[test]
    fn small_targets_suggest_a_narrower_size() {
        let p = &jvid_core::preset::builtin()[0];
        let r = Rate::new(30, 1);
        // The preset is 1080p60.
        assert_eq!(too_small(p, (1920, 1080), r, 8_000_000), None);
        // 0.02 bpp at 1080p60; 1280×720 has 2.25x fewer pixels, so 0.045 bpp.
        let w = too_small(p, (1920, 1080), r, 2_488_320).unwrap();
        assert!(w.contains("1280"), "{w}");
        assert!(
            too_small(p, (1920, 1080), r, 10_000)
                .unwrap()
                .contains("any")
        );
    }

    /// The mockup's scenario: a 2560×1440 60 fps project with In set, a subtitle track and three
    /// ranged markers, on the 9:16 Shorts preset so the aspect warning shows.
    fn scenario() -> (Project, ExportDialog) {
        let (mut h, _) = marked();
        let settings = ProjectSettings {
            width: 2560,
            height: 1440,
            frame_rate: Rate::new(60, 1),
            ..h.project().settings().clone()
        };
        h.apply(Command::SetSettings(settings)).unwrap();
        let cue = jvid_core::srt::Cue {
            start: secs(1),
            end: secs(3),
            text: "Hello".into(),
        };
        let (name, cues, style) = ("Subtitles".into(), vec![cue], Default::default());
        h.apply(Command::ImportSubtitles { name, cues, style })
            .unwrap();
        for (t, name) in [(1, "Opening"), (4, "Ridge Line"), (7, "Summit")] {
            let (time, duration, name) = (secs(t), secs(2), name.into());
            h.apply(Command::AddMarker {
                time,
                duration,
                name,
            })
            .unwrap();
        }
        let project = h.project().clone();
        let mut dialog = ExportDialog::default();
        dialog.preset = dialog
            .presets
            .iter()
            .position(|p| p.name.starts_with("Shorts"))
            .unwrap();
        dialog.open(project.sequence(), "Project");
        dialog.path = "/exports/Project.mp4".into();
        (project, dialog)
    }

    fn harness(
        project: Project,
        dialog: ExportDialog,
    ) -> egui_kittest::Harness<'static, ExportDialog> {
        let mut fresh = true;
        let mut harness = egui_kittest::Harness::builder()
            .with_size([960.0, 600.0])
            .build_ui_state(
                move |ui, d: &mut ExportDialog| {
                    if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                        d.show(ui, &project, &HashSet::new());
                    }
                },
                dialog,
            );
        harness.run_steps(2);
        harness
    }

    fn snapshot(name: &str, project: Project, dialog: ExportDialog) {
        harness(project, dialog).snapshot(name);
    }

    #[test]
    fn custom_loudness_adds_a_field_and_a_preset_takes_it_away() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let (project, dialog) = scenario();
        let mut h = harness(project, dialog);
        h.get_by_role_and_label(Role::ComboBox, "-14 LUFS").click();
        h.run();
        h.get_by_label("Custom").click();
        h.run();
        // Custom keeps the value it had, in a field beside the dropdown.
        assert_eq!(h.state().loudness, Some(-14.0));
        h.get_by_role_and_label(Role::ComboBox, "Custom");
        assert!(h.query_by_role(Role::SpinButton).is_some());
        h.get_by_role_and_label(Role::ComboBox, "Custom").click();
        h.run();
        h.get_by_label("-23 LUFS").click();
        h.run();
        assert_eq!(h.state().loudness, Some(-23.0));
        assert!(h.query_by_role(Role::SpinButton).is_none());
    }

    /// Option A at the smallest supported window: Format, Range and Output in one column, the
    /// aspect warning under Video, the Subtitles control, Lossless off with its reason, Estimated
    /// Size by Export.
    #[test]
    fn export_dialog_960x600() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let (project, dialog) = scenario();
        snapshot("export_dialog", project, dialog);
    }

    /// Every dropdown's menu opens inside the dialog's body, clear of its head and buttons.
    #[test]
    fn every_dropdown_menu_fits_inside_the_dialog() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        // Highlights' marker list pushes Loudness and Target Size down the body.
        let (project, mut dialog) = scenario();
        dialog.range = Some(Range::Highlights);
        let mut h = harness(project, dialog);
        let top = h
            .get_by_role_and_label(Role::Label, "Export")
            .rect()
            .bottom();
        let bottom = h.get_by_role_and_label(Role::Button, "Cancel").rect().top();
        let preset = h.state().presets[h.state().preset].name.clone();
        let mut menus = crate::test_util::menu_rects(&mut h, &[&preset, "-14 LUFS"]);
        // Down to Output, for Target Size.
        h.hover_at(egui::pos2(480.0, 300.0));
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -1000.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        h.run_steps(2);
        menus.extend(crate::test_util::menu_rects(&mut h, &["Off"]));
        for (l, r) in menus {
            assert!(r.top() >= top && r.bottom() <= bottom, "{l}: {r:?}");
        }
    }

    /// Highlights lists the markers; the body scrolls and the footer stays pinned.
    #[test]
    fn export_dialog_highlights_960x600() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let (project, mut dialog) = scenario();
        dialog.range = Some(Range::Highlights);
        snapshot("export_dialog_highlights", project, dialog);
    }
}
