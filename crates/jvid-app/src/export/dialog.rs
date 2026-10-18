//! The export dialog: preset, range, loudness and output path, gathered into a [`Request`].

use std::collections::HashSet;
use std::path::PathBuf;

use egui::Ui;
use jvid_core::model::{ClipId, Marker, MarkerId, Project, Sequence};
use jvid_core::preset::{AudioCodec, Preset, VideoCodec};
use jvid_core::time::{FLICKS_PER_SECOND, Rate, Time, format_timecode};
use jvid_media::remux::{Cut, SMART_ENCODER};
use rfd::FileDialog;
use suite_ui::theme;
use suite_ui::tokens::{radius, space};
use suite_ui::widgets::{Button, Dropdown, State};

use super::Request;
use super::lossless::{self, Lossless};

/// Which part of the timeline to export.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Range {
    Entire,
    InOut,
    Selected,
    /// Each chosen ranged marker as its own file.
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
    /// Ranged markers left out of a highlights export.
    skipped: HashSet<MarkerId>,
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
            template: TEMPLATE.into(),
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
        self.name = name.into();
    }

    /// Shows the dialog while open; returns the request when the user clicks Export or Add to
    /// Queue, with `true` for Export; Highlights gives one request per chosen marker.
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

                        if seq.tracks().iter().any(|t| t.subtitles) {
                            ui.label("Subtitles");
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut self.sidecar, false, "Burn In");
                                ui.radio_value(&mut self.sidecar, true, "Sidecar .srt");
                            });
                            ui.end_row();
                        }

                        ui.label("Range");
                        ui.horizontal(|ui| {
                            let marked = seq.in_point().is_some() || seq.out_point().is_some();
                            ui.radio_value(range, Range::Entire, "Entire Timeline");
                            ui.add_enabled_ui(marked, |ui| {
                                ui.radio_value(range, Range::InOut, "In to Out")
                                    .on_disabled_hover_text("Set In or Out on the timeline first")
                            });
                            ui.add_enabled_ui(!selected.is_empty(), |ui| {
                                ui.radio_value(range, Range::Selected, "Selected Clips")
                                    .on_disabled_hover_text("Select clips on the timeline first")
                            });
                            ui.add_enabled_ui(highlights(seq).next().is_some(), |ui| {
                                ui.radio_value(range, Range::Highlights, "Highlights")
                                    .on_disabled_hover_text("Add ranged markers first")
                            });
                        });
                        ui.end_row();

                        if *range == Range::Highlights {
                            ui.label("Highlights");
                            ui.vertical(|ui| {
                                for m in highlights(seq) {
                                    let mut on = !self.skipped.contains(&m.id);
                                    if ui.checkbox(&mut on, &m.name).changed() {
                                        if on {
                                            self.skipped.remove(&m.id);
                                        } else {
                                            self.skipped.insert(m.id);
                                        }
                                    }
                                }
                            });
                            ui.end_row();
                            ui.label("File Names");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.template).desired_width(260.0),
                            )
                            .on_hover_text("{project}, {marker} and {index} are filled in");
                            ui.end_row();
                        }

                        let span = span(*range, seq, selected);
                        let rate = s.frame_rate;
                        let tc = |t: Time| format_timecode(rate.index_floor(t).max(0) as u64, rate);
                        let (a, b) = span.unwrap_or_default();
                        for (label, t) in [("In", a), ("Out", b), ("Duration", b - a)] {
                            ui.label(label);
                            ui.monospace(tc(t));
                            ui.end_row();
                        }

                        ui.label("Lossless");
                        let copyable = lossless::check(project, (a, b));
                        lossless_picker(ui, &mut self.lossless, &copyable);
                        ui.end_row();
                        if let (Some(_), Ok(cuts)) = (self.lossless, &copyable)
                            && let Some(ext) = cuts[0].path.extension()
                        {
                            path.set_extension(ext);
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

                        ui.label("Target Size");
                        let sizable =
                            matches!(preset.video, Some(VideoCodec::H264 | VideoCodec::Hevc));
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(sizable, |ui| target_picker(ui, &mut self.target))
                                .response
                                .on_disabled_hover_text("Only H.264 and HEVC presets");
                            let secs = (b - a).0 as f64 / FLICKS_PER_SECOND as f64;
                            let warning = match self.target.filter(|_| sizable && secs > 0.0) {
                                Some(mb) => match super::target_bitrate(preset, mb * MB, secs) {
                                    Some(bits) => {
                                        too_small(preset, (s.width, s.height), rate, bits)
                                    }
                                    None => Some("Too small even for the audio".into()),
                                },
                                None => None,
                            };
                            if let Some(w) = warning {
                                ui.label("⚠").on_hover_text(w);
                            }
                        });
                        ui.end_row();

                        ui.label("Estimated Size");
                        match self.target.filter(|_| sizable) {
                            Some(mb) => ui.label(format!("under {mb} MB")),
                            None => ui.label(format_size(estimate(
                                preset,
                                (s.width, s.height),
                                rate,
                                b - a,
                            ))),
                        };
                        ui.end_row();
                    });
                ui.add_space(space::S16);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = space::S8;
                    let span = span(*range, seq, selected).filter(|(a, b)| b > a);
                    let chosen = chosen(seq, &self.skipped);
                    let kind = *range;
                    let ok = span.is_some()
                        && path.file_name().is_some()
                        && (kind != Range::Highlights || !chosen.is_empty());
                    let state = if ok { State::Live } else { State::Disabled };
                    let export = ui.add(Button::primary("Export").state(state)).clicked();
                    let queue = ui
                        .add(Button::secondary("Add to Queue").state(state))
                        .clicked();
                    if (export || queue)
                        && let Some(range) = span
                    {
                        let mut preset = self.presets[self.preset].clone();
                        if let Some(mb) = self.target
                            && matches!(preset.video, Some(VideoCodec::H264 | VideoCodec::Hevc))
                        {
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
                            let ext = path.extension().unwrap_or_default().to_string_lossy();
                            let dir = path.parent().unwrap_or(std::path::Path::new(""));
                            chosen
                                .iter()
                                .enumerate()
                                .map(|(i, m)| {
                                    let name = file_name(&self.template, &self.name, m, i + 1);
                                    req(
                                        dir.join(format!("{name}.{ext}")),
                                        (m.time, m.time + m.duration),
                                    )
                                })
                                .collect()
                        } else {
                            vec![req(path.clone(), range)]
                        };
                        request = Some((reqs, export));
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

/// Off, copy or smart render when the range copies; else why it can't, the first reason shown
/// and all of them on hover.
fn lossless_picker(
    ui: &mut Ui,
    mode: &mut Option<Lossless>,
    copyable: &Result<Vec<Cut>, Vec<String>>,
) {
    match copyable {
        Ok(_) => {
            ui.horizontal(|ui| {
                ui.radio_value(mode, None, "Off");
                ui.radio_value(mode, Some(Lossless::Copy), "Copy")
                    .on_hover_text("No re-encode; cuts snap to the nearest keyframe");
                ui.add_enabled_ui(SMART_ENCODER.is_some(), |ui| {
                    ui.radio_value(mode, Some(Lossless::Smart), "Smart Render")
                        .on_hover_text(
                            "Re-encodes only the frames around each cut, so cuts are exact",
                        )
                        .on_disabled_hover_text("Needs a GPL FFmpeg build with x264")
                });
            });
        }
        Err(why) => {
            ui.label(format!("Unavailable: {}", why[0]))
                .on_hover_text(why.join("\n"));
        }
    }
}

const MB: u64 = 1_000_000;

fn target_picker(ui: &mut Ui, target: &mut Option<u64>) {
    let label = |t: Option<u64>| t.map_or("Off".into(), |t| format!("{t} MB"));
    egui::ComboBox::from_id_salt("export target size")
        .selected_text(label(*target))
        .show_ui(ui, |ui| {
            for t in [None, Some(10), Some(25), Some(50), Some(100), Some(500)] {
                ui.selectable_value(target, t, label(t));
            }
            let mut custom = target.unwrap_or(25);
            let drag = egui::DragValue::new(&mut custom)
                .range(1..=100_000)
                .prefix("Custom ")
                .suffix(" MB");
            if ui.add(drag).changed() {
                *target = Some(custom);
            }
        });
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
        Some(nw) => format!("Too small for {w}×{h}; try a {nw}-wide preset"),
        None => format!("Too small for {w}×{h} at any common size; shorten the range"),
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

/// Ranged markers in timeline order.
pub fn highlights(seq: &Sequence) -> impl Iterator<Item = &Marker> {
    let mut all: Vec<_> = seq
        .markers()
        .iter()
        .filter(|m| m.duration > Time(0))
        .collect();
    all.sort_by_key(|m| m.time);
    all.into_iter()
}

/// The highlights to export: every ranged marker not in `skipped`.
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
        Range::Highlights => {
            let all: Vec<_> = highlights(seq).collect();
            Some((
                all.first()?.time,
                all.iter().map(|m| m.time + m.duration).max()?,
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
        AudioCodec::Aac => "AAC Stereo",
        AudioCodec::Mp3 => "MP3 Stereo",
        AudioCodec::Pcm => "PCM 16-bit Stereo",
        AudioCodec::Pcm24 => "PCM 24-bit Stereo",
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
    fn highlights_skip_points_and_unchosen_and_fill_names() {
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
        assert_eq!(names, ["a/b", "b"]);
        assert_eq!(
            span(Range::Highlights, seq, &HashSet::new()),
            Some((secs(2), secs(6)))
        );
        let skip = HashSet::from([highlights(seq).next().unwrap().id]);
        let left = chosen(seq, &skip);
        assert_eq!(left.len(), 1);
        assert_eq!(file_name(TEMPLATE, "Trip", left[0], 3), "Trip-b-03");
        let first = highlights(seq).next().unwrap();
        assert_eq!(file_name("{marker}: {index}", "p", first, 1), "a-b- 01");
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
}
