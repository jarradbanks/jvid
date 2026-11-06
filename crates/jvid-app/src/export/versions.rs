//! Independent, immutable version exports. Editing and preview state never reach the source.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use egui::{TextureHandle, Ui, vec2};
use jvid_core::history::{Command, History};
use jvid_core::keyframe::Value;
use jvid_core::model::{ClipId, MarkerId, Project, TrackKind};
use jvid_core::preset::Preset;
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use suite_ui::tokens::color;
use suite_ui::widgets::Button;

use crate::highlight_review::{HighlightSelectionSnapshot, ReviewRevision};
use crate::i18n::tr;

use super::Context as _;
use super::{GpuContext, Request};

/// Captions are a per-version choice, including omission without changing the edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Captions {
    Burn,
    Sidecar,
    Omit,
}

/// Frozen marker identity, name and half-open range; overlaps stay separate outputs.
#[derive(Clone, Debug, PartialEq)]
pub struct VersionRange {
    pub marker: Option<MarkerId>,
    pub name: String,
    pub span: (Time, Time),
}

/// Validated H handoff plus the exact project it describes. No shared mutable configuration.
#[derive(Clone)]
pub struct VersionSource {
    project: Project,
    revision: ReviewRevision,
    ranges: Vec<VersionRange>,
}

impl VersionSource {
    /// Validate H immediately before copying the source, never re-resolve silently.
    pub fn highlights(
        session: u64,
        history: &History,
        selection: &HighlightSelectionSnapshot,
    ) -> Result<Self, String> {
        selection
            .validate(session, history)
            .map_err(|_| tr!("highlights.stale").to_owned())?;
        let ranges = selection
            .highlights()
            .iter()
            .map(|h| VersionRange {
                marker: Some(h.marker().id),
                name: h.marker().name.clone(),
                span: h.span(),
            })
            .collect::<Vec<_>>();
        if ranges.is_empty() {
            return Err(tr!("highlights.no_selected").into());
        }
        Ok(Self {
            project: history.project().clone(),
            revision: selection.revision(),
            ranges,
        })
    }

    pub fn range(session: u64, history: &History, span: (Time, Time)) -> Result<Self, String> {
        if span.0 < Time(0) || span.0 >= span.1 || span.1 > history.project().sequence().duration()
        {
            return Err(tr!("export.nothing").into());
        }
        Ok(Self {
            project: history.project().clone(),
            revision: ReviewRevision::current(session, history),
            ranges: vec![VersionRange {
                marker: None,
                name: String::new(),
                span,
            }],
        })
    }

    pub fn revision(&self) -> ReviewRevision {
        self.revision
    }

    pub fn ranges(&self) -> &[VersionRange] {
        &self.ranges
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Check before accepting the setup; an edit requires reopening and reviewing new copies.
    pub fn validate(&self, session: u64, history: &History) -> Result<(), String> {
        if self.revision != ReviewRevision::current(session, history) {
            return Err(tr!("highlights.stale").into());
        }
        Ok(())
    }
}

/// Queue handoff. Private fields and read-only access keep each job independently immutable.
/// Framing is already applied to `project`; the request still uses normal sizing/target-size.
#[derive(Clone)]
pub struct VersionJob {
    project: Project,
    request: Request,
    revision: ReviewRevision,
    marker: Option<MarkerId>,
    captions: Captions,
    label: String,
}

impl VersionJob {
    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn request(&self) -> &Request {
        &self.request
    }

    pub fn revision(&self) -> ReviewRevision {
        self.revision
    }

    pub fn marker(&self) -> Option<MarkerId> {
        self.marker
    }

    pub fn captions(&self) -> Captions {
        self.captions
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn destinations(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.request.path.clone()];
        if self.request.sidecar {
            paths.push(self.request.path.with_extension("srt"));
        }
        paths
    }

    /// Name allocation happens before submission; jobs cannot be retargeted by the editor.
    pub(super) fn at_path(mut self, path: &Path) -> Self {
        self.request.path = path.to_owned();
        self
    }
}

/// Use the existing exporter, including target-size passes, inside a private sibling directory.
/// Atomic hard links publish without replacement on all platforms. A racing destination
/// causes this job to fail; it never overwrites another job, a sidecar, or existing media.
pub(super) fn submit(
    pool: &jvid_jobs::Pool,
    job: VersionJob,
    gpu: Option<GpuContext>,
) -> (
    super::ExportHandle,
    Receiver<Result<jvid_core::model::ProbeInfo, super::Error>>,
) {
    let (tx, rx) = channel();
    let progress = super::Progress::default();
    let report = progress.clone();
    let handle = pool.submit(
        format!("version:{}", job.request.path.display()),
        jvid_jobs::Priority::Interactive,
        move |ctx| {
            let ctx = super::Tracked {
                job: ctx,
                status: &report,
            };
            ctx.set_phase(super::Phase::Preparing);
            let result = run_job(&job, gpu.as_ref().map(|(d, q)| (d, q)), &ctx);
            ctx.set_phase(match &result {
                Ok(_) => super::Phase::Completed,
                Err(super::Error::Cancelled) => super::Phase::Cancelled,
                Err(super::Error::Failed(_)) => super::Phase::Failed,
            });
            let _ = tx.send(result);
        },
    );
    (super::ExportHandle::new(handle, progress), rx)
}

fn run_job(
    job: &VersionJob,
    gpu: Option<(
        &eframe::egui_wgpu::wgpu::Device,
        &eframe::egui_wgpu::wgpu::Queue,
    )>,
    ctx: &dyn super::Context,
) -> Result<jvid_core::model::ProbeInfo, super::Error> {
    for path in job.destinations() {
        super::check_destination(&job.project, &path)?;
    }
    let files = super::AttemptFiles::new(&job.request.path)?;
    let request = Request {
        path: files.output.clone(),
        ..job.request.clone()
    };
    let info = super::run_export(&job.project, &request, gpu, ctx)?;
    ctx.set_phase(super::Phase::Finalizing);
    if ctx.is_cancelled() {
        return Err(super::Error::Cancelled);
    }
    let mut published = Vec::new();
    for (from, to) in job.destinations().iter().enumerate().map(|(i, p)| {
        (
            if i == 0 {
                files.output.clone()
            } else {
                files.output.with_extension("srt")
            },
            p.clone(),
        )
    }) {
        if let Err(error) = std::fs::hard_link(&from, &to) {
            for path in published {
                let _ = std::fs::remove_file(path);
            }
            return Err(super::Error::Failed(tr!(
                "export.publish_error",
                path = to.display(),
                error = error
            )));
        }
        published.push(to);
    }
    Ok(info)
}

/// Allocate readable numeric suffixes against existing media, SRT sidecars, active jobs and
/// all siblings in this batch. Final publication still checks races atomically.
pub(super) fn allocate(
    jobs: Vec<VersionJob>,
    mut busy: impl FnMut(&Path) -> bool,
) -> Vec<VersionJob> {
    let mut used = HashSet::new();
    jobs.into_iter()
        .map(|job| {
            let original = &job.request.path;
            let stem = original.file_stem().unwrap_or_default().to_string_lossy();
            let extension = original.extension().unwrap_or_default().to_string_lossy();
            for suffix in 1u64.. {
                let path = if suffix == 1 {
                    original.clone()
                } else {
                    original.with_file_name(format!("{stem}-{suffix}.{extension}"))
                };
                let candidate = job.clone().at_path(&path);
                let destinations = candidate.destinations();
                let keys = destinations
                    .iter()
                    .map(|p| output_key(p))
                    .collect::<Vec<_>>();
                if !destinations
                    .iter()
                    .any(|p| p.symlink_metadata().is_ok() || busy(p))
                    && !keys.iter().any(|p| used.contains(p))
                {
                    used.extend(keys);
                    return candidate;
                }
            }
            unreachable!("available filename")
        })
        .collect()
}

/// Reuse the preset's existing width/size semantics; reject invalid encoder dimensions early.
fn dimensions(preset: &Preset, project: &Project) -> Result<(u32, u32), String> {
    let settings = project.settings();
    let (w, h) = preset.output_size((settings.width, settings.height));
    if w < 2 || h < 2 || w > 8192 || h > 8192 || w % 2 != 0 || h % 2 != 0 {
        return Err(tr!("versions.dimensions_error").into());
    }
    Ok((w, h))
}

pub(super) fn output_key(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::canonicalize(parent)
            .unwrap_or_else(|_| parent.to_owned())
            .join(path.file_name().unwrap_or_default())
    });
    path.to_string_lossy().to_lowercase()
}

/// One editable copy. Its framing history and review acknowledgments belong only to it.
struct Version {
    name: String,
    preset: Preset,
    captions: Captions,
    history: History,
    ranges: Vec<VersionRange>,
    generation: u64,
    reviewed: HashSet<usize>,
    range: usize,
    frame: i64,
    clip: Option<ClipId>,
    preview: Option<(u64, usize, i64, TextureHandle)>,
}

impl Version {
    fn new(source: &VersionSource, name: String, preset: Preset) -> Self {
        let history = History::new(framed(source.project(), &preset));
        Self {
            name,
            preset,
            captions: Captions::Burn,
            history,
            ranges: source.ranges.clone(),
            generation: 0,
            reviewed: HashSet::new(),
            range: 0,
            frame: 0,
            clip: None,
            preview: None,
        }
    }

    fn invalidate(&mut self) {
        self.generation += 1;
        self.reviewed.clear();
        self.preview = None;
    }

    fn time(&self) -> Time {
        let span = self.ranges[self.range].span;
        let rate = self
            .preset
            .frame_rate
            .unwrap_or(self.history.project().settings().frame_rate);
        rate.time_floor(rate.index_floor(span.0) + self.frame)
            .min(span.1 - Time(1))
    }

    fn preview_current(&self) -> bool {
        self.preview
            .as_ref()
            .is_some_and(|(g, r, f, _)| (*g, *r, *f) == (self.generation, self.range, self.frame))
    }

    fn project(&self) -> Project {
        if self.captions == Captions::Burn {
            self.history.project().clone()
        } else {
            self.history.project().without_subtitles()
        }
    }

    fn pan(&mut self, id: ClipId, position: [f64; 2]) -> Result<(), String> {
        let (_, clip) = self
            .history
            .project()
            .sequence()
            .clip(id)
            .ok_or_else(|| tr!("export.nothing").to_owned())?;
        let mut transform = clip.transform.clone();
        let delta = [
            position[0] - transform.position[0],
            position[1] - transform.position[1],
        ];
        transform.position = position;
        let mut keys = clip.keyframes.clone();
        for key in keys.get_mut("position").into_iter().flatten() {
            if let Value::Vec2(v) = &mut key.value {
                v[0] += delta[0];
                v[1] += delta[1];
            }
        }
        self.history
            .apply(Command::SetClipProps(vec![(id, transform, keys)]))
            .map_err(|e| e.to_string())?;
        self.invalidate();
        Ok(())
    }

    fn file_name(&self, project_name: &str, index: usize) -> String {
        let stem = clean_name(&format!(
            "{project_name}-{}-{}-{:02}",
            self.name,
            self.ranges[index].name,
            index + 1
        ));
        format!("{stem}.{}", self.preset.extension)
    }

    fn codec_error(&self) -> Option<String> {
        let preset = if self.history.project().settings().hdr_output {
            self.preset.clone().hdr()
        } else {
            self.preset.clone()
        };
        let codec = preset.video?;
        jvid_media::encode::video_encoders(codec, &preset.options)
            .is_empty()
            .then(|| super::dialog::codec_unavailable(codec))
    }

    fn freeze(
        &self,
        source: &VersionSource,
        dir: &Path,
        project_name: &str,
    ) -> Result<Vec<VersionJob>, String> {
        if let Some(error) = self.codec_error() {
            return Err(error);
        }
        if self.preset.video.is_some() {
            dimensions(&self.preset, self.history.project())?;
        }
        if self.preset.extension.is_empty()
            || !self
                .preset
                .extension
                .bytes()
                .all(|b| b.is_ascii_alphanumeric())
            || self.preset.extension.eq_ignore_ascii_case("srt")
        {
            return Err(tr!("export.no_file_name").into());
        }
        if self.preset.video.is_some() && self.reviewed.len() != self.ranges.len() {
            return Err(tr!("versions.review_required").into());
        }
        self.ranges
            .iter()
            .enumerate()
            .map(|(i, range)| {
                let request = Request {
                    path: dir.join(self.file_name(project_name, i)),
                    range: range.span,
                    preset: self.preset.clone(),
                    loudness: Some(-14.0),
                    lossless: None,
                    sidecar: self.captions == Captions::Sidecar,
                };
                Ok(VersionJob {
                    project: if self.captions == Captions::Omit {
                        self.history.project().without_subtitles()
                    } else {
                        self.history.project().clone()
                    },
                    request,
                    revision: source.revision,
                    marker: range.marker,
                    captions: self.captions,
                    label: self.name.clone(),
                })
            })
            .collect()
    }
}

/// Fill a changed aspect at the original short-side resolution. Normal preset scaling then
/// handles final dimensions exactly as ordinary export does, without stretching the picture.
fn framed(project: &Project, preset: &Preset) -> Project {
    if dimensions(preset, project).is_err() {
        return project.clone();
    }
    let s = project.settings();
    let (w, h) = preset.output_size((s.width, s.height));
    if preset.video.is_some() && w as u64 * s.height as u64 != h as u64 * s.width as u64 {
        let (fw, fh) = jvid_core::reframe::frame_size(s.width, s.height, [w, h]);
        // Keep normal short-side framing where it is exact. Extreme/unusual aspects use the
        // requested dimensions rather than allocating an enormous frame or stretching it.
        let (fw, fh) = if fw > 8192 || fh > 8192 || fw as u64 * h as u64 != fh as u64 * w as u64 {
            (w, h)
        } else {
            (fw, fh)
        };
        project.reframed(fw, fh, jvid_render::text::drawn_bounds)
    } else {
        project.clone()
    }
}

/// Portable file stem, including Windows device names and trailing dots/spaces.
fn clean_name(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    let mut name = name.trim().trim_matches('.').trim_end().to_owned();
    while name.len() > 120 {
        name.pop();
    }
    let name = name.trim_end_matches([' ', '.']).to_owned();
    let device = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if name.is_empty()
        || matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (device.len() == 4
            && (device.starts_with("COM") || device.starts_with("LPT"))
            && device.as_bytes()[3].is_ascii_digit())
    {
        format!("_{name}")
    } else {
        name
    }
}

type PreviewResult = Result<egui::ColorImage, String>;
struct PreviewWork {
    version: usize,
    generation: u64,
    range: usize,
    frame: i64,
    result: Receiver<PreviewResult>,
}

/// One worker at a time per dialog, even while rapidly changing framing. It renders through
/// export's common renderer with original media, then scales exactly like the output preset.
fn preview(
    project: &Project,
    time: Time,
    preset: &Preset,
    (device, queue): &GpuContext,
) -> PreviewResult {
    jvid_media::ffmpeg::init().map_err(|e| e.to_string())?;
    let mut renderer = super::FrameRenderer::new(device);
    let tex = renderer.render(project, time, device, queue)?;
    let rgba = jvid_render::composite::read_back(device, queue, tex);
    let image =
        image::RgbaImage::from_raw(tex.width(), tex.height(), rgba).expect("renderer dimensions");
    let (w, h) = dimensions(preset, project)?;
    let scale = (640.0 / w as f64).min(360.0 / h as f64).min(1.0);
    let (w, h) = (
        (w as f64 * scale).round().max(1.0) as u32,
        (h as f64 * scale).round().max(1.0) as u32,
    );
    let image = image::imageops::resize(&image, w, h, image::imageops::FilterType::Triangle);
    Ok(egui::ColorImage::from_rgba_premultiplied(
        [w as usize, h as usize],
        image.as_raw(),
    ))
}

#[derive(Default)]
pub struct VersionsDialog {
    source: Option<VersionSource>,
    presets: Vec<Preset>,
    versions: Vec<Version>,
    selected: usize,
    directory: PathBuf,
    project_name: String,
    work: Option<PreviewWork>,
    error: Option<String>,
}

impl VersionsDialog {
    pub fn is_open(&self) -> bool {
        self.source.is_some()
    }

    pub fn open(&mut self, source: VersionSource, name: &str) {
        let presets = super::presets();
        let landscape = presets[0].clone();
        let vertical = presets
            .iter()
            .find(|p| p.size.is_some_and(|[w, h]| w < h))
            .unwrap_or(&landscape)
            .clone();
        let mut limited = landscape.clone();
        limited
            .options
            .insert("target_size".into(), "10000000".into());
        self.versions = vec![
            Version::new(&source, tr!("versions.landscape").into(), landscape),
            Version::new(&source, tr!("versions.vertical").into(), vertical),
            Version::new(&source, tr!("versions.limited").into(), limited),
        ];
        self.presets = presets;
        self.source = Some(source);
        self.selected = 0;
        self.project_name = name.into();
        if self.directory.as_os_str().is_empty() {
            self.directory = std::env::current_dir().unwrap_or_default();
        }
        // An in-flight obsolete preview must finish before another starts; its result is ignored.
        if let Some(work) = &mut self.work {
            work.generation = u64::MAX;
        }
        self.error = None;
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        session: u64,
        history: &History,
        gpu: Option<GpuContext>,
    ) -> Option<Vec<VersionJob>> {
        let source = self.source.as_ref()?;
        if let Err(error) = source.validate(session, history) {
            self.source = None;
            self.error = Some(error);
            return None;
        }
        self.poll(ui.ctx());
        let mut jobs = None;
        let mut close = false;
        let width = (ui.ctx().content_rect().width() - 32.0).clamp(200.0, 680.0);
        let modal = egui::Modal::new(egui::Id::new("versions_dialog")).show(ui.ctx(), |ui| {
            ui.set_width(width);
            ui.label(
                egui::RichText::new(tr!("action.export_versions"))
                    .font(suite_ui::theme::headline()),
            );
            egui::ScrollArea::vertical()
                .max_height((ui.ctx().content_rect().height() - 150.0).max(100.0))
                .show(ui, |ui| {
                    ui.label(tr!("versions.help"));
                    ui.horizontal_wrapped(|ui| {
                        for (i, v) in self.versions.iter().enumerate() {
                            if ui.selectable_label(self.selected == i, &v.name).clicked() {
                                self.selected = i;
                                self.error = None;
                            }
                        }
                    });
                    self.review(ui);
                    self.controls(ui);
                    ui.separator();
                    ui.label(tr!("export.output"));
                    ui.label(self.directory.display().to_string());
                    if ui.add(Button::secondary(tr!("export.browse"))).clicked()
                        && let Some(dir) = rfd::FileDialog::new()
                            .set_directory(&self.directory)
                            .pick_folder()
                    {
                        self.directory = dir;
                    }
                    if let Some(error) = &self.error {
                        ui.colored_label(color::red(), error);
                    }
                });
            let ready = self.versions.iter().all(|v| {
                v.codec_error().is_none()
                    && (v.preset.video.is_none() || v.reviewed.len() == v.ranges.len())
            });
            let buttons = suite_ui::dialog::action_footer(
                ui,
                [
                    Button::secondary(tr!("prompt.cancel")),
                    Button::primary(tr!("export.add_to_queue")).state(if ready {
                        suite_ui::widgets::State::Live
                    } else {
                        suite_ui::widgets::State::Disabled
                    }),
                ],
            );
            close = buttons[0].clicked();
            if buttons[1].clicked() {
                let source = self.source.as_ref().unwrap();
                match self
                    .versions
                    .iter()
                    .map(|v| v.freeze(source, &self.directory, &self.project_name))
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(all) => {
                        jobs = Some(all.into_iter().flatten().collect());
                        close = true;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        });
        if close || modal.should_close() {
            self.source = None;
        }
        if self.source.is_some() {
            self.start_preview(ui.ctx(), gpu);
        }
        jobs
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(work) = &self.work else {
            return;
        };
        let result = match work.result.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(_) => Err(tr!("export.crashed").into()),
        };
        let work = self.work.take().unwrap();
        let Some(v) = self.versions.get_mut(work.version) else {
            return;
        };
        if (work.generation, work.range, work.frame) != (v.generation, v.range, v.frame) {
            return;
        }
        match result {
            Ok(image) => {
                let texture = ctx.load_texture("version_preview", image, Default::default());
                v.preview = Some((v.generation, v.range, v.frame, texture));
                if work.version == self.selected {
                    self.error = None;
                }
            }
            Err(error) if work.version == self.selected => self.error = Some(error),
            Err(_) => {}
        }
    }

    fn start_preview(&mut self, ctx: &egui::Context, gpu: Option<GpuContext>) {
        let v = &self.versions[self.selected];
        if self.work.is_some()
            || v.preview_current()
            || self.error.is_some()
            || v.preset.video.is_none()
        {
            return;
        }
        let Some(gpu) = gpu else {
            return;
        };
        let (project, time, preset) = (v.project(), v.time(), v.preset.clone());
        let (tx, result) = channel();
        let ctx = ctx.clone();
        self.work = Some(PreviewWork {
            version: self.selected,
            generation: v.generation,
            range: v.range,
            frame: v.frame,
            result,
        });
        std::thread::spawn(move || {
            let _ = tx.send(preview(&project, time, &preset, &gpu));
            ctx.request_repaint();
        });
    }

    fn review(&mut self, ui: &mut Ui) {
        let v = &mut self.versions[self.selected];
        if v.preset.video.is_none() {
            return;
        }
        ui.label(format!("{} · {}", v.range + 1, v.ranges[v.range].name));
        let span = v.ranges[v.range].span;
        let rate = v
            .preset
            .frame_rate
            .unwrap_or(v.history.project().settings().frame_rate);
        let max = rate.index_floor(span.1 - span.0).saturating_sub(1).max(0);
        if ui
            .add(egui::Slider::new(&mut v.frame, 0..=max).text(tr!("versions.preview")))
            .changed()
        {
            self.error = None;
        }
        if v.preview_current() {
            let texture = &v.preview.as_ref().unwrap().3;
            egui::Frame::NONE.fill(color::VIEWER).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.add(
                        egui::Image::new(texture)
                            .max_size(vec2(ui.available_width(), 240.0))
                            .maintain_aspect_ratio(true),
                    );
                });
            });
            if ui.add(Button::secondary(tr!("versions.accept"))).clicked() {
                v.reviewed.insert(v.range);
            }
        } else {
            ui.spinner();
            ui.label(tr!("versions.review_required"));
        }
        ui.label(tr!(
            "versions.review_count",
            done = v.reviewed.len(),
            total = v.ranges.len()
        ));
    }

    fn controls(&mut self, ui: &mut Ui) {
        let presets = &self.presets;
        let v = &mut self.versions[self.selected];
        ui.label(tr!("versions.name"));
        ui.text_edit_singleline(&mut v.name);
        ui.label(v.file_name(&self.project_name, v.range));
        ui.label(tr!("export.preset"));
        let mut picked = None;
        egui::ComboBox::from_id_salt("version_preset")
            .selected_text(super::shown_name(presets, &v.preset))
            .show_ui(ui, |ui| {
                for (i, p) in presets.iter().enumerate() {
                    if ui
                        .selectable_label(p.name == v.preset.name, super::preset_name(i, p))
                        .clicked()
                    {
                        picked = Some(p.clone());
                    }
                }
            });
        let mut resized = false;
        if let Some(p) = picked {
            v.preset = p;
            resized = true;
        }
        if let Some(error) = v.codec_error() {
            ui.colored_label(color::red(), error);
        }
        let (mut w, mut h) = v.preset.output_size((
            v.history.project().settings().width,
            v.history.project().settings().height,
        ));
        ui.label(tr!("versions.dimensions"));
        ui.horizontal_wrapped(|ui| {
            resized |= ui
                .add(egui::DragValue::new(&mut w).range(2..=8192).speed(2))
                .changed();
            ui.label(crate::i18n::verbatim("×"));
            resized |= ui
                .add(egui::DragValue::new(&mut h).range(2..=8192).speed(2))
                .changed();
        });
        if resized {
            v.preset.size = Some([w, h]);
            v.history = History::new(framed(self.source.as_ref().unwrap().project(), &v.preset));
            v.invalidate();
            self.error = None;
        }
        ui.label(tr!("export.target_size"));
        let mut mb = v
            .preset
            .options
            .get("target_size")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
            / 1_000_000;
        let sizable = matches!(
            v.preset.video,
            Some(jvid_core::preset::VideoCodec::H264 | jvid_core::preset::VideoCodec::Hevc)
        );
        if ui
            .add_enabled(
                sizable,
                egui::DragValue::new(&mut mb)
                    .range(0..=100_000)
                    .suffix(crate::i18n::verbatim(" MB")),
            )
            .changed()
        {
            if mb == 0 {
                v.preset.options.remove("target_size");
            } else {
                v.preset
                    .options
                    .insert("target_size".into(), (mb * 1_000_000).to_string());
            }
        }
        if !sizable {
            ui.label(tr!("export.target_size_off"));
        }
        ui.label(tr!("versions.captions"));
        ui.horizontal_wrapped(|ui| {
            for (policy, label) in [
                (Captions::Burn, tr!("export.burn_in")),
                (Captions::Sidecar, tr!("export.sidecar")),
                (Captions::Omit, tr!("export.none")),
            ] {
                if ui
                    .selectable_value(&mut v.captions, policy, label)
                    .changed()
                {
                    v.invalidate();
                    self.error = None;
                }
            }
        });
        ui.label(tr!("export.range"));
        egui::ComboBox::from_id_salt("version_range")
            .selected_text(format!("{} · {}", v.range + 1, v.ranges[v.range].name))
            .show_ui(ui, |ui| {
                for (i, range) in v.ranges.iter().enumerate() {
                    if ui
                        .selectable_value(&mut v.range, i, format!("{} · {}", i + 1, range.name))
                        .changed()
                    {
                        v.frame = 0;
                        self.error = None;
                    }
                }
            });
        let range = &mut v.ranges[v.range];
        let (mut start, mut end) = (
            range.span.0.0 as f64 / FLICKS_PER_SECOND as f64,
            range.span.1.0 as f64 / FLICKS_PER_SECOND as f64,
        );
        let duration =
            v.history.project().sequence().duration().0 as f64 / FLICKS_PER_SECOND as f64;
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(tr!("export.in"));
            changed |= ui
                .add(
                    egui::DragValue::new(&mut start)
                        .range(0.0..=duration)
                        .speed(0.1),
                )
                .changed();
            ui.label(tr!("export.out"));
            changed |= ui
                .add(
                    egui::DragValue::new(&mut end)
                        .range(0.0..=duration)
                        .speed(0.1),
                )
                .changed();
        });
        if changed && start < end {
            let rate = v.history.project().settings().frame_rate;
            let snap = |s: f64| {
                rate.time_nearest(
                    rate.index_nearest(Time((s * FLICKS_PER_SECOND as f64).round() as i64)),
                )
            };
            let span = (
                snap(start),
                snap(end).min(v.history.project().sequence().duration()),
            );
            if span.0 < span.1 {
                range.span = span;
                v.frame = 0;
                v.invalidate();
                self.error = None;
            }
        }
        if v.preset.video.is_some() {
            ui.separator();
            ui.label(tr!("versions.framing"));
            let clips = v
                .history
                .project()
                .sequence()
                .tracks()
                .iter()
                .filter(|t| t.kind == TrackKind::Video)
                .flat_map(|t| t.clips())
                .filter_map(|c| {
                    let asset = v.history.project().asset(c.asset)?;
                    (asset.text.is_none()).then(|| {
                        (
                            c.id,
                            asset.name.clone().unwrap_or_else(|| {
                                asset
                                    .path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned()
                            }),
                        )
                    })
                })
                .collect::<Vec<_>>();
            if !clips.iter().any(|(id, _)| Some(*id) == v.clip) {
                v.clip = clips.first().map(|(id, _)| *id);
            }
            egui::ComboBox::from_id_salt("version_clip")
                .selected_text(
                    clips
                        .iter()
                        .find(|(id, _)| Some(*id) == v.clip)
                        .map(|(_, n)| n.as_str())
                        .unwrap_or(tr!("export.none")),
                )
                .show_ui(ui, |ui| {
                    for (id, name) in &clips {
                        ui.selectable_value(&mut v.clip, Some(*id), name);
                    }
                });
            if let Some(id) = v.clip
                && let Some((_, c)) = v.history.project().sequence().clip(id)
            {
                let mut position = c.transform.position;
                let mut pan = false;
                ui.horizontal_wrapped(|ui| {
                    ui.label(tr!("inspector.position"));
                    for p in &mut position {
                        pan |= ui
                            .add(egui::DragValue::new(p).range(-16384.0..=16384.0).speed(1.0))
                            .changed();
                    }
                });
                if pan {
                    self.error = v.pan(id, position).err();
                }
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(v.history.can_undo(), Button::secondary(tr!("action.undo")))
                    .clicked()
                {
                    v.history.undo();
                    v.invalidate();
                    self.error = None;
                }
                if ui
                    .add_enabled(v.history.can_redo(), Button::secondary(tr!("action.redo")))
                    .clicked()
                {
                    v.history.redo();
                    v.invalidate();
                    self.error = None;
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::history::Created;
    use jvid_core::model::{NewClip, ProjectSettings};
    use jvid_core::time::Rate;
    use jvid_jobs::{Pool, Priority};
    use std::time::{Duration, Instant};

    fn secs(s: i64) -> Time {
        Time(s * FLICKS_PER_SECOND)
    }

    fn fixture() -> Option<History> {
        let dir = jvid_media::test_media::corpus()?;
        let path = dir.join("frame-index-1080p30.mp4");
        let probe = jvid_media::probe::probe(&path).unwrap();
        let mut h = History::new(Project::default());
        h.apply(Command::Import(vec![(path, probe)])).unwrap();
        let asset = h.project().assets()[0].id;
        let Created::Track(track) = h
            .apply(Command::AddTrack {
                kind: TrackKind::Video,
                name: "Video".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        h.apply(Command::AddClip {
            track,
            clip: NewClip {
                asset,
                source_in: Time(0),
                source_out: secs(3),
                start: Time(0),
                link: None,
            },
        })
        .unwrap();
        h.apply(Command::SetSettings(ProjectSettings {
            frame_rate: Rate::new(30, 1),
            width: 1920,
            height: 1080,
            ..h.project().settings().clone()
        }))
        .unwrap();
        h.apply(Command::ImportSubtitles {
            name: "Captions".into(),
            cues: vec![jvid_core::srt::Cue {
                start: secs(1),
                end: secs(2),
                text: "Visible caption".into(),
            }],
            style: Default::default(),
        })
        .unwrap();
        for _ in 0..2 {
            h.apply(Command::AddMarker {
                time: secs(1),
                duration: Time(FLICKS_PER_SECOND / 2),
                name: "Duplicate/name".into(),
            })
            .unwrap();
        }
        Some(h)
    }

    fn reviewed_source(h: &History) -> VersionSource {
        let selected = h
            .project()
            .sequence()
            .markers()
            .iter()
            .map(|m| m.id)
            .collect();
        let snapshot =
            HighlightSelectionSnapshot::capture(7, h, &selected, (secs(1), secs(1))).unwrap();
        VersionSource::highlights(7, h, &snapshot).unwrap()
    }

    fn preset(size: [u32; 2]) -> Preset {
        let mut p = super::super::presets()
            .into_iter()
            .find(|p| p.video == Some(jvid_core::preset::VideoCodec::ProRes))
            .unwrap();
        p.size = Some(size);
        p.audio = None;
        p.frame_rate = Some(Rate::new(30, 1));
        p
    }

    fn ready(source: &VersionSource, size: [u32; 2]) -> Version {
        let mut v = Version::new(source, "Version".into(), preset(size));
        v.reviewed.extend(0..v.ranges.len());
        v
    }

    #[test]
    fn version_sources_reject_stale_h_selections_and_project_sessions() {
        let Some(mut h) = fixture() else {
            return;
        };
        let selected = h
            .project()
            .sequence()
            .markers()
            .iter()
            .map(|m| m.id)
            .collect();
        let selection =
            HighlightSelectionSnapshot::capture(7, &h, &selected, (secs(1), secs(1))).unwrap();
        let source = VersionSource::highlights(7, &h, &selection).unwrap();
        assert!(source.validate(8, &h).is_err());
        h.apply(Command::SetInOut {
            in_point: Some(secs(1)),
            out_point: Some(secs(2)),
        })
        .unwrap();
        assert!(VersionSource::highlights(7, &h, &selection).is_err());
        assert!(source.validate(7, &h).is_err());
        assert_eq!(
            source.ranges()[0].span,
            (secs(1), Time(FLICKS_PER_SECOND * 3 / 2))
        );
    }

    #[test]
    fn variant_framing_undo_and_frozen_jobs_are_independent() {
        let Some(h) = fixture() else {
            return;
        };
        let original = h.project().clone();
        let source = reviewed_source(&h);
        let dir = tempfile::tempdir().unwrap();
        let mut v = ready(&source, [180, 320]);
        let other = ready(&source, [320, 180]);
        let other_project = other.history.project().clone();
        let jobs = v.freeze(&source, dir.path(), "Edit").unwrap();
        let frozen = jobs[0].project().clone();
        let id = v
            .history
            .project()
            .sequence()
            .tracks()
            .iter()
            .flat_map(|t| t.clips())
            .next()
            .unwrap()
            .id;
        let before = v.history.project().clone();
        v.pan(id, [100.0, 20.0]).unwrap();
        assert!(v.freeze(&source, dir.path(), "Edit").is_err());
        assert_ne!(v.history.project(), &before);
        v.history.undo();
        assert_eq!(v.history.project(), &before);
        v.history.redo();
        assert_ne!(v.history.project(), &before);
        let json = jvid_core::file::to_json(v.history.project(), dir.path()).unwrap();
        let restored = jvid_core::file::from_json(&json, dir.path(), |_| false).unwrap();
        assert_eq!(&restored, v.history.project());
        assert_eq!(
            jvid_core::eval::frame_at(&restored, secs(1)),
            jvid_core::eval::frame_at(v.history.project(), secs(1))
        );
        assert_eq!(jobs[0].project(), &frozen);
        assert_eq!(other.history.project(), &other_project);
        assert_eq!(h.project(), &original);
        assert_eq!(jobs[0].revision(), source.revision());
        assert_eq!(jobs[0].marker(), source.ranges()[0].marker);
        let mut audio = ready(&source, [320, 180]);
        audio.captions = Captions::Omit;
        assert!(
            audio.freeze(&source, dir.path(), "Edit").unwrap()[0]
                .project()
                .subtitle_cues()
                .is_empty()
        );
        audio.captions = Captions::Sidecar;
        assert!(
            audio.freeze(&source, dir.path(), "Edit").unwrap()[0]
                .request()
                .sidecar
        );
    }

    #[test]
    fn names_avoid_existing_sidecars_batch_duplicates_and_live_outputs() {
        let Some(h) = fixture() else {
            return;
        };
        let source = reviewed_source(&h);
        let dir = tempfile::tempdir().unwrap();
        let mut v = ready(&source, [320, 180]);
        v.captions = Captions::Sidecar;
        let mut jobs = v.freeze(&source, dir.path(), "Edit").unwrap();
        jobs[1].request.path = jobs[0].request.path.clone();
        let original = jobs[0].request.path.clone();
        std::fs::write(original.with_extension("srt"), b"keep").unwrap();
        let allocated = allocate(jobs, |path| {
            path.file_stem().unwrap().to_string_lossy().ends_with("-2")
        });
        assert_ne!(allocated[0].request.path, original);
        assert_ne!(allocated[0].request.path, allocated[1].request.path);
        assert!(
            allocated[0]
                .request
                .path
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .ends_with("-3")
        );
        assert_eq!(
            std::fs::read(original.with_extension("srt")).unwrap(),
            b"keep"
        );
        for name in ["CON", "NUL.txt", "COM1", "../", "a:b/c\\d"] {
            let clean = clean_name(name);
            assert!(!clean.contains(['/', '\\', ':']));
            assert_ne!(clean, name);
        }
        let unicode = clean_name(&"映像".repeat(120));
        assert!(unicode.len() <= 120);
        assert!(!unicode.is_empty());
        let mut bad = preset([321, 180]);
        assert!(dimensions(&bad, h.project()).is_err());
        bad.size = Some([8194, 2]);
        assert!(dimensions(&bad, h.project()).is_err());
    }

    fn wait(queue: &mut super::super::queue::Queue) {
        let until = Instant::now() + Duration::from_secs(90);
        while queue.unsettled() {
            queue.poll();
            assert!(Instant::now() < until, "exports did not settle");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn concurrent_versions_encode_exact_dimensions_ranges_and_caption_policy() {
        let Some(gpu) = crate::playback::tests::device() else {
            return;
        };
        let Some(h) = fixture() else {
            return;
        };
        let original = h.project().clone();
        let source =
            VersionSource::range(7, &h, (secs(1), Time(FLICKS_PER_SECOND * 3 / 2))).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let wide = ready(&source, [1920, 1080]);
        let mut tall = ready(&source, [180, 320]);
        tall.captions = Captions::Sidecar;
        let mut jobs = wide.freeze(&source, dir.path(), "Edit").unwrap();
        let mut second = tall.freeze(&source, dir.path(), "Edit").unwrap();
        second[0].request.path = second[0].request.path.with_file_name("tall.mov");
        jobs.extend(second);
        let pool = Pool::new(2);
        let mut queue = super::super::queue::Queue::default();
        queue.versions(&pool, jobs, Some(gpu));
        assert_eq!(queue.pending(), 2);
        wait(&mut queue);
        let rows = queue.rows();
        assert_eq!(rows.len(), 2);
        for (row, size) in rows.iter().zip([[1920, 1080], [180, 320]]) {
            assert_eq!(
                row.status,
                super::super::queue::Status::Done,
                "{:?}",
                row.status
            );
            let probe = jvid_media::probe::probe(&row.path).unwrap();
            assert_eq!((probe.width, probe.height), (Some(size[0]), Some(size[1])));
            assert!(
                (probe.duration - Time(FLICKS_PER_SECOND / 2)).0.abs()
                    <= Rate::new(30, 1).time_floor(1).0
            );
        }
        let mut decoder = jvid_media::decode::VideoDecoder::open(&rows[0].path).unwrap();
        for (time, index) in [(Time(0), 30), (secs(10), 44)] {
            let frame = decoder.frame_at(time).unwrap();
            assert_eq!(
                jvid_media::test_media::frame_index(&frame.planes[0], frame.width as usize),
                index
            );
        }
        assert!(!rows[0].path.with_extension("srt").exists());
        let srt = std::fs::read_to_string(rows[1].path.with_extension("srt")).unwrap();
        assert!(srt.contains("00:00:00,000 --> 00:00:00,500"));
        assert!(srt.contains("Visible caption"));
        assert_eq!(h.project(), &original);
    }

    #[test]
    fn cancelling_and_failing_one_version_leave_siblings_and_sidecar_reservations_independent() {
        let Some(gpu) = crate::playback::tests::device() else {
            return;
        };
        let Some(h) = fixture() else {
            return;
        };
        let source =
            VersionSource::range(7, &h, (secs(1), Time(FLICKS_PER_SECOND * 3 / 2))).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut v = ready(&source, [160, 90]);
        v.captions = Captions::Sidecar;
        let template = v.freeze(&source, dir.path(), "Edit").unwrap()[0].clone();
        let jobs = (0..3)
            .map(|i| {
                let mut job = template
                    .clone()
                    .at_path(&dir.path().join(format!("job{i}.mov")));
                if i == 1 {
                    job.request.range = (Time(0), Time(0));
                }
                job
            })
            .collect();
        let pool = Pool::new(1);
        let (start_tx, start_rx) = channel();
        let (release_tx, release_rx) = channel();
        let _block = pool.submit("blocked", Priority::Interactive, move |_| {
            start_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        start_rx.recv().unwrap();
        let mut queue = super::super::queue::Queue::default();
        queue.versions(&pool, jobs, Some(gpu));
        queue.cancel(0);
        assert!(queue.writing(&dir.path().join("job0.srt")));
        release_tx.send(()).unwrap();
        wait(&mut queue);
        let rows = queue.rows();
        assert_eq!(rows[0].status, super::super::queue::Status::Cancelled);
        assert!(matches!(
            rows[1].status,
            super::super::queue::Status::Failed(_)
        ));
        assert_eq!(rows[2].status, super::super::queue::Status::Done);
        assert!(!dir.path().join("job0.mov").exists());
        assert!(!dir.path().join("job1.mov").exists());
        assert!(dir.path().join("job2.srt").exists());
        assert!(!queue.writing(&dir.path().join("job0.srt")));
    }

    #[test]
    fn racing_sidecar_never_overwrites_and_rolls_back_only_this_jobs_media() {
        let Some(gpu) = crate::playback::tests::device() else {
            return;
        };
        let Some(h) = fixture() else {
            return;
        };
        let source =
            VersionSource::range(7, &h, (secs(1), Time(FLICKS_PER_SECOND * 3 / 2))).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut v = ready(&source, [160, 90]);
        v.captions = Captions::Sidecar;
        let job = v.freeze(&source, dir.path(), "Edit").unwrap().remove(0);
        let media = job.request.path.clone();
        let sidecar = media.with_extension("srt");
        std::fs::write(&sidecar, b"existing subtitles").unwrap();
        let pool = Pool::new(1);
        let (_, rx) = submit(&pool, job, Some(gpu));
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(90)).unwrap(),
            Err(super::super::Error::Failed(_))
        ));
        assert!(!media.exists());
        assert_eq!(std::fs::read(sidecar).unwrap(), b"existing subtitles");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn size_limited_version_uses_existing_encoder_passes_and_meets_limit() {
        let Some(gpu) = crate::playback::tests::device() else {
            return;
        };
        let Some(h) = fixture() else {
            return;
        };
        let source =
            VersionSource::range(7, &h, (secs(1), Time(FLICKS_PER_SECOND * 3 / 2))).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut v = ready(&source, [320, 180]);
        let mut p = super::super::presets().remove(0);
        p.size = Some([320, 180]);
        p.frame_rate = Some(Rate::new(30, 1));
        p.audio = None;
        p.options.insert("encoder".into(), "software".into());
        p.options.insert("target_size".into(), "200000".into());
        // Also exercise a codec disappearing after capture. Ordinary setup refuses it early.
        let mut job = v.freeze(&source, dir.path(), "Limited").unwrap().remove(0);
        v.preset = p.clone();
        if v.codec_error().is_some() {
            assert!(v.freeze(&source, dir.path(), "Limited").is_err());
        }
        job.request.preset = p;
        job.request.path.set_extension("mp4");
        let request = job.request.clone();
        let path = request.path.clone();
        let pool = Pool::new(1);
        super::super::audit_b10a::check_codec_result(
            &request,
            || {
                let (_, rx) = submit(&pool, job, Some(gpu));
                rx.recv_timeout(Duration::from_secs(90)).unwrap()
            },
            |info| {
                assert_eq!((info.width, info.height), (Some(320), Some(180)));
                assert!(std::fs::metadata(path).unwrap().len() <= 200000);
            },
        );
    }

    #[test]
    fn cancelling_active_version_does_not_cancel_simultaneous_sibling() {
        let Some(gpu) = crate::playback::tests::device() else {
            return;
        };
        let Some(h) = fixture() else {
            return;
        };
        let source = VersionSource::range(7, &h, (Time(0), secs(3))).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let first = ready(&source, [640, 360])
            .freeze(&source, dir.path(), "First")
            .unwrap()
            .remove(0);
        let second = ready(&source, [160, 90])
            .freeze(&source, dir.path(), "Second")
            .unwrap()
            .remove(0);
        let path = first.request.path.clone();
        let sibling = second.request.path.clone();
        let pool = Pool::new(2);
        let (active, a) = submit(&pool, first, Some(gpu.clone()));
        let (other, b) = submit(&pool, second, Some(gpu));
        let until = Instant::now() + Duration::from_secs(30);
        while active.progress() == 0.0 {
            assert!(Instant::now() < until, "active job never made progress");
            std::thread::sleep(Duration::from_millis(1));
        }
        active.cancel();
        assert!(!other.is_cancelled());
        assert_eq!(
            a.recv_timeout(Duration::from_secs(90)).unwrap(),
            Err(super::super::Error::Cancelled)
        );
        assert!(b.recv_timeout(Duration::from_secs(90)).unwrap().is_ok());
        assert!(!path.exists());
        assert!(sibling.exists());
    }

    #[test]
    fn rendered_preview_required_and_each_range_review_invalidates_after_edit() {
        use egui_kittest::kittest::Queryable;
        let Some(gpu) = crate::playback::tests::device() else {
            return;
        };
        let Some(h) = fixture() else {
            return;
        };
        let source = reviewed_source(&h);
        let original = h.project().clone();
        let mut dialog = VersionsDialog::default();
        dialog.open(source, "Review");
        dialog.directory = "/exports".into();
        dialog.selected = 1;
        let mut fresh = true;
        let mut harness = egui_kittest::Harness::builder()
            .with_size([960.0, 600.0])
            .build_ui_state(
                move |ui, d: &mut VersionsDialog| {
                    if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                        d.show(ui, 7, &h, Some(gpu.clone()));
                    }
                },
                dialog,
            );
        harness.run_steps(3);
        assert!(harness.state().versions[1].reviewed.is_empty());
        let until = Instant::now() + Duration::from_secs(30);
        while !harness.state().versions[1].preview_current() {
            assert!(Instant::now() < until, "preview did not render");
            std::thread::sleep(Duration::from_millis(10));
            harness.run_steps(1);
        }
        harness.get_by_label(tr!("versions.accept")).click();
        harness.run_steps(1);
        assert_eq!(harness.state().versions[1].reviewed.len(), 1);
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/multiversion-export");
        std::fs::create_dir_all(&dir).unwrap();
        harness
            .render()
            .unwrap()
            .save(dir.join("versions_vertical_960x600.png"))
            .unwrap();
        let d = harness.state_mut();
        d.versions[1].range = 1;
        assert!(!d.versions[1].preview_current());
        assert!(
            d.versions[1]
                .freeze(d.source.as_ref().unwrap(), &d.directory, &d.project_name)
                .is_err()
        );
        assert_eq!(d.source.as_ref().unwrap().project(), &original);
        d.versions[1].invalidate();
        assert!(d.versions[1].reviewed.is_empty());
    }

    #[test]
    fn version_dialog_fits_narrow_window_and_rejects_stale_setup() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let Some(h) = fixture() else {
            return;
        };
        let source = reviewed_source(&h);
        let mut dialog = VersionsDialog::default();
        dialog.open(source, "Review");
        dialog.directory = "/exports".into();
        let mut fresh = true;
        let mut harness = egui_kittest::Harness::builder()
            .with_size([240.0, 640.0])
            .build_ui_state(
                move |ui, d: &mut VersionsDialog| {
                    if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                        d.show(ui, 7, &h, None);
                    }
                },
                dialog,
            );
        harness.run_steps(3);
        let cancel = harness.get_by_label(tr!("prompt.cancel")).rect();
        assert!(
            harness.ctx.content_rect().contains_rect(cancel),
            "{cancel:?}"
        );
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/multiversion-export");
        std::fs::create_dir_all(&dir).unwrap();
        harness
            .render()
            .unwrap()
            .save(dir.join("versions_narrow_240x640.png"))
            .unwrap();
        // Validation also covers edits occurring between modal opening and its first paint.
        let Some(mut h) = fixture() else {
            return;
        };
        let source = reviewed_source(&h);
        h.apply(Command::SetInOut {
            in_point: Some(secs(1)),
            out_point: None,
        })
        .unwrap();
        assert!(source.validate(7, &h).is_err());
    }
}
