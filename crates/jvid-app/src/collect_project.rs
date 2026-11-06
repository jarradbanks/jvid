//! Collect a detached project snapshot, whole originals and external dependencies.
//! Copies run on the shared jobs pool; a project appears only after verified copies.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};

use jvid_core::file::{self, DependencyKind};
use jvid_core::model::Project;
use jvid_jobs::{Handle, Pool, Priority};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::i18n::tr;

pub const MANIFEST: &str = "dependencies.json";
pub const UNRESOLVED: &str = "unresolved.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Media,
    Lut,
    Effect,
    Font,
}

impl From<DependencyKind> for Kind {
    fn from(kind: DependencyKind) -> Self {
        match kind {
            DependencyKind::Media => Self::Media,
            DependencyKind::Lut => Self::Lut,
            DependencyKind::Effect => Self::Effect,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub kind: Kind,
    /// Original reference, for diagnostics only. Reopening uses `relative_path`.
    pub reference: String,
    /// Always UTF-8, `/` separated and relative to the collected project.
    pub relative_path: String,
    pub bytes: u64,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_original_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundling_permitted: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Missing,
    NotFile,
    FontPermission,
    FontCoverage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unresolved {
    pub kind: Kind,
    pub reference: String,
    pub reason: Reason,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub project: String,
    pub files: Vec<Entry>,
    pub unresolved: Vec<Unresolved>,
}

/// Permission must be supplied explicitly; installed fonts are never copied implicitly.
#[derive(Debug, Clone)]
pub struct Font {
    pub family: String,
    pub original_family: String,
    pub path: PathBuf,
    pub permitted: bool,
}

#[derive(Clone)]
pub struct Request {
    pub project: Project,
    /// Must not exist. The UI chooses a new child of the selected folder.
    pub destination: PathBuf,
    pub project_name: String,
    /// Resolves legacy relative references; `None` uses the working directory.
    pub source_project: Option<PathBuf>,
    pub effect_directory: Option<PathBuf>,
    pub fonts: Vec<Font>,
    /// Explicit opt-in to unbundled fonts, recorded in both reports. Required files still block.
    pub allow_unresolved: bool,
}

impl Request {
    pub fn new(project: Project, source_project: Option<PathBuf>, destination: PathBuf) -> Self {
        let project_name = source_project
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|p| p.to_str())
            .unwrap_or("project.jvid")
            .to_owned();
        let families = project
            .assets()
            .iter()
            .filter_map(|a| a.text.as_ref().map(|t| t.family.clone()))
            .collect::<Vec<_>>();
        let fonts = jvid_render::text::project_font_sources(&families)
            .into_iter()
            .map(|font| Font {
                family: font.family,
                original_family: font.original_family,
                path: font.path,
                permitted: true,
            })
            .collect();
        Self {
            project,
            destination,
            project_name,
            source_project,
            effect_directory: crate::effects::dir(),
            fonts,
            allow_unresolved: false,
        }
    }
}

#[derive(Debug)]
pub enum FailureKind {
    Cancelled,
    Unresolved,
    Io(io::Error),
}

#[derive(Debug)]
pub struct Failure {
    pub kind: FailureKind,
    /// Retained even when no project is published, for display or explicit report saving.
    pub manifest: Manifest,
}

#[derive(Debug)]
pub struct Collected {
    pub project: PathBuf,
    pub manifest: Manifest,
}

/// Queuing and polling never copy or hash media on the UI thread.
pub fn submit(pool: &Pool, request: Request) -> (Handle, Receiver<Result<Collected, Failure>>) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let key = format!("collect:{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let (tx, rx) = mpsc::channel();
    let handle = pool.submit(key, Priority::Interactive, move |ctx| {
        let result = collect(&request, || ctx.is_cancelled(), |p| ctx.set_progress(p));
        let _ = tx.send(result);
    });
    (handle, rx)
}

struct Destination(PathBuf);
impl Drop for Destination {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

struct Input {
    kind: Kind,
    reference: String,
    source: PathBuf,
    font_original_family: Option<String>,
}

/// Synchronous engine for CLI/tests; UI callers use [`submit`]. Existing destinations fail
/// before any mutation. Cancellation/failure removes only the directory this call created.
pub fn collect(
    request: &Request,
    cancelled: impl Fn() -> bool,
    progress: impl Fn(f32),
) -> Result<Collected, Failure> {
    let mut manifest = Manifest {
        version: 1,
        project: request.project_name.clone(),
        files: Vec::new(),
        unresolved: Vec::new(),
    };
    let result = run(request, &mut manifest, &cancelled, &progress);
    match result {
        Ok(project) => Ok(Collected { project, manifest }),
        Err(error) => Err(Failure {
            kind: if cancelled() || error.kind() == io::ErrorKind::Interrupted {
                FailureKind::Cancelled
            } else if error.kind() == io::ErrorKind::NotFound && !manifest.unresolved.is_empty() {
                FailureKind::Unresolved
            } else {
                FailureKind::Io(error)
            },
            manifest,
        }),
    }
}

fn run(
    request: &Request,
    manifest: &mut Manifest,
    cancelled: &impl Fn() -> bool,
    progress: &impl Fn(f32),
) -> io::Result<PathBuf> {
    check_cancel(cancelled)?;
    let name = Path::new(&request.project_name);
    if name.components().count() != 1
        || !matches!(name.components().next(), Some(Component::Normal(_)))
        || name
            .extension()
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("jvid"))
    {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let destination = std::path::absolute(&request.destination)?;
    // Atomic reservation, including empty directories and symlinks: never reuse a destination.
    fs::create_dir(&destination)?;
    let mut owned = Destination(destination.clone());
    let stage = destination.join(".collecting");
    fs::create_dir(&stage)?;
    fs::create_dir(stage.join("media"))?;
    fs::create_dir(stage.join("dependencies"))?;
    let base = std::path::absolute(
        request
            .source_project
            .as_deref()
            .and_then(Path::parent)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    let mut inputs = Vec::new();
    let mut seen = BTreeSet::new();
    let mut bundled_sources = Vec::new();
    for dep in file::dependencies(&request.project).map_err(io::Error::other)? {
        check_cancel(cancelled)?;
        if !seen.insert((dep.kind, dep.path.clone())) {
            continue;
        }
        let mut source = if dep.kind == DependencyKind::Effect && dep.path.components().count() == 1
        {
            request
                .effect_directory
                .as_ref()
                .map(|p| p.join(&dep.path).with_extension("wgsl"))
                .unwrap_or_else(|| base.join(&dep.path).with_extension("wgsl"))
        } else {
            file::rebase(&dep.path, &base)
        };
        // Config files take precedence, including edited examples. A shipped shader with no
        // external override is captured from this build, never guessed from another install.
        if dep.kind == DependencyKind::Effect
            && dep.path.components().count() == 1
            && !source.exists()
        {
            if let Some((_, text)) = jvid_render::effect::BUNDLED
                .iter()
                .find(|(name, _)| Path::new(name).file_stem() == Some(dep.path.as_os_str()))
            {
                source = stage.join(format!("builtin-{}.wgsl", bundled_sources.len()));
                write_new(&source, text.as_bytes())?;
                bundled_sources.push(source.clone());
            }
        }
        inputs.push(Input {
            kind: dep.kind.into(),
            reference: dep.path.to_string_lossy().into(),
            source,
            font_original_family: None,
        });
    }
    let families: BTreeSet<_> = request
        .project
        .assets()
        .iter()
        .filter_map(|a| a.text.as_ref().map(|t| t.family.clone()))
        .collect();
    for family in families {
        let supplied: Vec<_> = request
            .fonts
            .iter()
            .filter(|f| f.family == family && f.permitted)
            .collect();
        if supplied.is_empty() {
            manifest.unresolved.push(Unresolved {
                kind: Kind::Font,
                reference: family,
                reason: Reason::FontPermission,
            });
        } else {
            for font in supplied {
                inputs.push(Input {
                    kind: Kind::Font,
                    reference: family.clone(),
                    source: font.path.clone(),
                    font_original_family: Some(font.original_family.clone()),
                });
            }
        }
    }
    let mut ready = Vec::new();
    for input in inputs {
        check_cancel(cancelled)?;
        match fs::metadata(&input.source) {
            Ok(meta) if meta.is_file() => ready.push(input),
            Ok(_) => manifest.unresolved.push(Unresolved {
                kind: input.kind,
                reference: input.reference,
                reason: Reason::NotFile,
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => manifest.unresolved.push(Unresolved {
                kind: input.kind,
                reference: input.reference,
                reason: Reason::Missing,
            }),
            Err(e) => return Err(e),
        }
    }
    if manifest.unresolved.iter().any(|entry| {
        entry.kind != Kind::Font
            || entry.reason == Reason::Missing
            || entry.reason == Reason::NotFile
    }) || (!request.allow_unresolved && !manifest.unresolved.is_empty())
    {
        return Err(io::Error::from(io::ErrorKind::NotFound));
    }
    let count = ready.len().max(1) as f32;
    let mut stored: BTreeMap<(String, u64), Vec<String>> = BTreeMap::new();
    for (i, input) in ready.iter().enumerate() {
        check_cancel(cancelled)?;
        let extension = input
            .source
            .extension()
            .and_then(|s| s.to_str())
            .filter(|s| {
                s.len() <= 12 && !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric())
            })
            .unwrap_or("bin")
            .to_ascii_lowercase();
        let directory = if input.kind == Kind::Media {
            "media"
        } else {
            "dependencies"
        };
        let builtin = (input.kind == Kind::Effect)
            .then(|| Path::new(&input.reference).file_stem())
            .flatten()
            .and_then(|stem| {
                jvid_render::effect::BUNDLED.iter().find_map(|(name, _)| {
                    (Path::new(name).file_stem() == Some(stem))
                        .then(|| name.strip_suffix(".wgsl").unwrap())
                })
            });
        let mut relative = builtin.map_or_else(
            || format!("{directory}/{i:06}.{extension}"),
            |stem| format!("dependencies/{i:06}/{stem}.wgsl"),
        );
        let target = stage.join(&relative);
        if builtin.is_some() {
            fs::create_dir(target.parent().unwrap())?;
        }
        let (bytes, digest) = copy_verified(&input.source, &target, cancelled, |p| {
            progress((i as f32 + p * 0.9) / count * 0.95);
        })?;
        if builtin.is_some() {
            sync_dir(target.parent().unwrap())?;
        }
        let duplicates = stored.entry((digest.clone(), bytes)).or_default();
        let mut found = None;
        // Byte comparison also protects against digest collisions. Only same-size/hash files
        // are compared; large libraries do not require quadratic full-media comparisons.
        for previous in duplicates.iter() {
            if files_equal(&target, &stage.join(previous), cancelled)? {
                found = Some(previous.clone());
                break;
            }
        }
        if let Some(previous) = found {
            fs::remove_file(&target)?;
            relative = previous;
        } else {
            duplicates.push(relative.clone());
        }
        manifest.files.push(Entry {
            kind: input.kind,
            reference: input.reference.clone(),
            relative_path: relative,
            bytes,
            sha256: digest,
            font_original_family: input.font_original_family.clone(),
            bundling_permitted: (input.kind == Kind::Font).then_some(true),
        });
        progress((i + 1) as f32 / count * 0.95);
    }
    for entry in &manifest.files {
        check_cancel(cancelled)?;
        let path = stage.join(&entry.relative_path);
        match entry.kind {
            Kind::Effect => {
                jvid_render::effect::parse(&fs::read_to_string(path)?).map_err(io::Error::other)?;
            }
            Kind::Lut => {
                jvid_render::lut::parse(&fs::read_to_string(path)?).map_err(io::Error::other)?;
            }
            Kind::Media | Kind::Font => {}
        }
    }
    let families: BTreeSet<_> = manifest
        .files
        .iter()
        .filter(|e| e.kind == Kind::Font)
        .map(|e| e.reference.clone())
        .collect();
    for family in families {
        check_cancel(cancelled)?;
        let fonts: Vec<_> = manifest
            .files
            .iter()
            .filter(|e| e.kind == Kind::Font && e.reference == family)
            .map(|entry| jvid_render::text::FontSource {
                family: family.clone(),
                original_family: entry
                    .font_original_family
                    .clone()
                    .unwrap_or_else(|| family.clone()),
                path: stage.join(&entry.relative_path),
            })
            .collect();
        let texts: Vec<_> = request
            .project
            .assets()
            .iter()
            .filter_map(|a| a.text.as_ref())
            .filter(|t| t.family == family)
            .cloned()
            .collect();
        if !jvid_render::text::project_font_coverage(&fonts, &texts)? {
            manifest.unresolved.push(Unresolved {
                kind: Kind::Font,
                reference: family,
                reason: Reason::FontCoverage,
            });
        }
    }
    if !request.allow_unresolved && !manifest.unresolved.is_empty() {
        return Err(io::Error::from(io::ErrorKind::NotFound));
    }
    for source in bundled_sources {
        fs::remove_file(source)?;
    }
    let remapped = file::remap_paths(&request.project, |kind, path| {
        manifest
            .files
            .iter()
            .find(|e| e.kind == Kind::from(kind) && Path::new(&e.reference) == path)
            .map_or_else(
                || path.to_path_buf(),
                |e| destination.join(&e.relative_path),
            )
    })
    .map_err(io::Error::other)?;
    let mut json: serde_json::Value =
        serde_json::from_str(&file::to_json(&remapped, &destination).map_err(io::Error::other)?)
            .map_err(io::Error::other)?;
    for (asset, original) in json["project"]["assets"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .zip(request.project.assets())
    {
        if original.name.is_none() && !original.path.as_os_str().is_empty() {
            asset["name"] = original.display_name().into();
        }
    }
    let mut fonts = Vec::new();
    for entry in manifest.files.iter().filter(|e| e.kind == Kind::Font) {
        let alias = font_alias(&entry.reference, &manifest.files);
        for asset in json["project"]["assets"]
            .as_array_mut()
            .into_iter()
            .flatten()
        {
            if let Some(text) = asset.get_mut("text").filter(|t| t.is_object()) {
                if text["family"].as_str() == Some(&entry.reference) {
                    text["family"] = alias.clone().into();
                }
            }
        }
        fonts.push(crate::project_file::FontLink::new(
            jvid_render::text::FontSource {
                family: alias,
                original_family: entry
                    .font_original_family
                    .clone()
                    .unwrap_or_else(|| entry.reference.clone()),
                path: destination.join(&entry.relative_path),
            },
            &destination,
        ));
    }
    if !fonts.is_empty() {
        json["schema_version"] = jvid_core::file::SUPPORTED_SCHEMA_VERSION.into();
        json["fonts"] = serde_json::to_value(fonts).map_err(io::Error::other)?;
    }
    // Keep dependency provenance and incomplete-package status with the project.
    json["collection"] = serde_json::to_value(&manifest).map_err(io::Error::other)?;
    let project_json = serde_json::to_string_pretty(&json).map_err(io::Error::other)?;
    file::from_json(&project_json, &destination, |path| {
        path.strip_prefix(&destination)
            .is_ok_and(|relative| stage.join(relative).is_file())
    })
    .map_err(io::Error::other)?;
    write_new(&stage.join(&request.project_name), project_json.as_bytes())?;
    write_new(
        &stage.join(MANIFEST),
        &serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?,
    )?;
    write_new(
        &stage.join(UNRESOLVED),
        &serde_json::to_vec_pretty(&manifest.unresolved).map_err(io::Error::other)?,
    )?;
    check_cancel(cancelled)?;
    sync_dir(&stage.join("media"))?;
    sync_dir(&stage.join("dependencies"))?;
    sync_dir(&stage)?;
    for entry in ["media", "dependencies", MANIFEST, UNRESOLVED] {
        fs::rename(stage.join(entry), destination.join(entry))?;
    }
    sync_dir(&destination)?;
    check_cancel(cancelled)?;
    // Project is the final publication marker, after all whole files are verified and synced.
    fs::rename(
        stage.join(&request.project_name),
        destination.join(&request.project_name),
    )?;
    fs::remove_dir(&stage)?;
    sync_dir(&destination)?;
    owned.0 = PathBuf::new();
    progress(1.0);
    Ok(destination.join(&request.project_name))
}

fn font_alias(family: &str, files: &[Entry]) -> String {
    let mut hash = Sha256::new();
    hash.update(family.as_bytes());
    let digests: BTreeSet<_> = files
        .iter()
        .filter(|e| e.kind == Kind::Font && e.reference == family)
        .map(|e| &e.sha256)
        .collect();
    for digest in digests {
        hash.update(digest.as_bytes());
    }
    format!("jvid-collection-{:x}", hash.finalize())
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    let _ = path;
    Ok(())
}

fn check_cancel(cancelled: &impl Fn() -> bool) -> io::Result<()> {
    if cancelled() {
        Err(io::Error::from(io::ErrorKind::Interrupted))
    } else {
        Ok(())
    }
}

fn hash(path: &Path, cancelled: &impl Fn() -> bool) -> io::Result<(u64, String)> {
    let mut input = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut bytes = 0;
    let mut buffer = vec![0u8; 128 * 1024];
    loop {
        check_cancel(cancelled)?;
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        hasher.update(&buffer[..n]);
    }
    Ok((bytes, format!("{:x}", hasher.finalize())))
}

fn copy_verified(
    source: &Path,
    target: &Path,
    cancelled: &impl Fn() -> bool,
    progress: impl Fn(f32),
) -> io::Result<(u64, String)> {
    let mut input = File::open(source)?;
    let before = input.metadata()?;
    if !before.is_file() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    let mut buffer = vec![0u8; 128 * 1024];
    let mut hasher = Sha256::new();
    let mut bytes = 0;
    loop {
        check_cancel(cancelled)?;
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        output.write_all(&buffer[..n])?;
        hasher.update(&buffer[..n]);
        bytes += n as u64;
        if bytes > before.len() {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        progress(bytes as f32 / before.len().max(1) as f32);
    }
    output.sync_all()?;
    drop(output);
    let expected = (bytes, format!("{:x}", hasher.finalize()));
    let after = input.metadata()?;
    if bytes != before.len()
        || before.modified().ok() != after.modified().ok()
        || hash(target, cancelled)? != expected
        || hash(source, cancelled)? != expected
    {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    Ok(expected)
}

fn files_equal(left: &Path, right: &Path, cancelled: &impl Fn() -> bool) -> io::Result<bool> {
    let mut left = File::open(left)?;
    let mut right = File::open(right)?;
    if left.metadata()?.len() != right.metadata()?.len() {
        return Ok(false);
    }
    let mut a = vec![0u8; 128 * 1024];
    let mut b = vec![0u8; 128 * 1024];
    loop {
        check_cancel(cancelled)?;
        let n = left.read(&mut a)?;
        if n == 0 {
            return Ok(true);
        }
        right.read_exact(&mut b[..n])?;
        if a[..n] != b[..n] {
            return Ok(false);
        }
    }
}

/// Compact, wrapping collection dialog; owns view/job state, never the live project/history.
#[derive(Default)]
pub struct Dialog {
    open: bool,
    request: Option<Request>,
    destination: String,
    job: Option<(Handle, Receiver<Result<Collected, Failure>>)>,
    result: Option<Result<Collected, Failure>>,
    previous_focus: Option<egui::Id>,
    captured_focus: bool,
    report_error: Option<String>,
}

impl Dialog {
    pub fn open(&mut self, project: &Project, path: Option<&Path>) {
        if self.is_running() {
            self.open = true;
            return;
        }
        self.request = Some(Request::new(
            project.clone(),
            path.map(Path::to_owned),
            PathBuf::new(),
        ));
        self.destination.clear();
        self.result = None;
        self.report_error = None;
        self.captured_focus = false;
        self.open = true;
    }

    pub fn is_running(&self) -> bool {
        self.job.is_some()
    }
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn show(&mut self, ctx: &egui::Context, pool: &Pool) {
        if let Some((handle, receiver)) = &self.job {
            match receiver.try_recv() {
                Ok(result) => {
                    self.result = Some(result);
                    self.job = None;
                }
                Err(mpsc::TryRecvError::Disconnected) if handle.is_finished() => {
                    self.result = Some(Err(Failure {
                        kind: FailureKind::Cancelled,
                        manifest: Manifest {
                            version: 1,
                            project: String::new(),
                            files: Vec::new(),
                            unresolved: Vec::new(),
                        },
                    }));
                    self.job = None;
                }
                _ => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                }
            }
        }
        if !self.open {
            return;
        }
        if !self.captured_focus {
            self.previous_focus = ctx.memory(|m| m.focused());
            self.captured_focus = true;
        }
        let running = self.is_running();
        let modal = egui::Modal::new(egui::Id::new("collect_project"))
            .frame(
                egui::Frame::popup(&ctx.global_style())
                    .corner_radius(suite_ui::tokens::radius::DIALOG)
                    .inner_margin(suite_ui::tokens::space::S16),
            )
            .show(ctx, |ui| {
                ui.set_width(440.0_f32.min((ctx.content_rect().width() - 48.0).max(1.0)));
                ui.label(
                    egui::RichText::new(tr!("collect.title")).font(suite_ui::theme::headline()),
                );
                ui.set_max_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .max_height(ctx.content_rect().height() * 0.7)
                    .show(ui, |ui| {
                        ui.label(tr!("collect.explain"));
                        if let Some(request) = &mut self.request {
                            ui.add_enabled_ui(!running, |ui| {
                                ui.label(tr!("collect.destination"));
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.destination)
                                        .desired_width(f32::INFINITY),
                                );
                                if ui.button(tr!("collect.choose_folder")).clicked()
                                    && let Some(parent) = rfd::FileDialog::new().pick_folder()
                                {
                                    let stem = Path::new(&request.project_name)
                                        .file_stem()
                                        .unwrap_or_default()
                                        .to_string_lossy();
                                    self.destination = parent
                                        .join(format!("{stem}-collected"))
                                        .to_string_lossy()
                                        .into_owned();
                                }
                                let families: BTreeSet<_> = request
                                    .project
                                    .assets()
                                    .iter()
                                    .filter_map(|a| a.text.as_ref().map(|t| t.family.clone()))
                                    .collect();
                                for family in families {
                                    ui.separator();
                                    let shown = jvid_render::text::display_family(&family);
                                    ui.label(if shown.is_empty() {
                                        tr!("collect.default_font").to_owned()
                                    } else {
                                        shown
                                    });
                                    if ui.button(tr!("collect.choose_fonts")).clicked()
                                        && let Some(paths) = rfd::FileDialog::new()
                                            .add_filter(
                                                tr!("collect.font_files"),
                                                &["ttf", "otf", "ttc", "otc"],
                                            )
                                            .pick_files()
                                    {
                                        request.fonts.retain(|font| font.family != family);
                                        request.fonts.extend(paths.into_iter().map(|path| Font {
                                            family: family.clone(),
                                            original_family: jvid_render::text::display_family(
                                                &family,
                                            ),
                                            path,
                                            permitted: false,
                                        }));
                                    }
                                    for font in request
                                        .fonts
                                        .iter_mut()
                                        .filter(|font| font.family == family)
                                    {
                                        ui.label(
                                            font.path
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy(),
                                        );
                                        ui.checkbox(
                                            &mut font.permitted,
                                            tr!("collect.font_permission"),
                                        );
                                    }
                                }
                                ui.separator();
                                ui.checkbox(
                                    &mut request.allow_unresolved,
                                    tr!("collect.allow_unresolved"),
                                );
                            });
                        }
                        if let Some((handle, _)) = &self.job {
                            ui.add(egui::ProgressBar::new(handle.progress()).show_percentage());
                        }
                        if let Some(result) = &self.result {
                            match result {
                                Ok(collected) => {
                                    ui.label(if collected.manifest.unresolved.is_empty() {
                                        tr!("collect.complete")
                                    } else {
                                        tr!("collect.incomplete")
                                    });
                                    ui.label(collected.project.to_string_lossy());
                                }
                                Err(failure) => {
                                    ui.label(match &failure.kind {
                                        FailureKind::Cancelled => {
                                            tr!("collect.cancelled").to_owned()
                                        }
                                        FailureKind::Unresolved => {
                                            tr!("collect.unresolved").to_owned()
                                        }
                                        FailureKind::Io(error) => {
                                            tr!("collect.failed", error = error)
                                        }
                                    });
                                }
                            }
                            let manifest = match result {
                                Ok(c) => &c.manifest,
                                Err(f) => &f.manifest,
                            };
                            for entry in &manifest.unresolved {
                                let reason = match entry.reason {
                                    Reason::Missing => tr!("collect.missing"),
                                    Reason::NotFile => tr!("collect.not_file"),
                                    Reason::FontPermission => tr!("collect.font_unresolved"),
                                    Reason::FontCoverage => tr!("collect.font_coverage"),
                                };
                                let shown = jvid_render::text::display_family(&entry.reference);
                                let shown = if shown.is_empty() {
                                    tr!("collect.default_font").to_owned()
                                } else {
                                    shown
                                };
                                ui.label(format!("{shown}: {reason}"));
                            }
                            if !manifest.unresolved.is_empty()
                                && ui.button(tr!("collect.save_report")).clicked()
                                && let Some(path) =
                                    rfd::FileDialog::new().set_file_name(UNRESOLVED).save_file()
                            {
                                match serde_json::to_vec_pretty(&manifest.unresolved)
                                    .map_err(io::Error::other)
                                    .and_then(|bytes| fs::write(path, bytes))
                                {
                                    Ok(()) => {}
                                    Err(error) => {
                                        self.report_error = Some(error.to_string());
                                    }
                                }
                            }
                        }
                    });
                ui.separator();
                if let Some(error) = &self.report_error {
                    ui.label(tr!("collect.failed", error = error));
                }
                use suite_ui::widgets::{Button, ButtonKind, State};
                let can_start = !running && !self.destination.trim().is_empty();
                let buttons = suite_ui::dialog::action_footer(
                    ui,
                    [
                        Button::new(
                            ButtonKind::Secondary,
                            if running {
                                tr!("collect.cancel")
                            } else {
                                tr!("collect.close")
                            },
                        ),
                        Button::new(ButtonKind::Primary, tr!("collect.start")).state(
                            if can_start {
                                State::Live
                            } else {
                                State::Disabled
                            },
                        ),
                    ],
                );
                if buttons[0].clicked() {
                    if let Some((handle, _)) = &self.job {
                        handle.cancel();
                    } else {
                        self.open = false;
                    }
                }
                if buttons[1].clicked()
                    && can_start
                    && let Some(request) = &mut self.request
                {
                    request.destination = PathBuf::from(&self.destination);
                    self.job = Some(submit(pool, request.clone()));
                    self.result = None;
                }
            });
        if modal.should_close() {
            self.open = false;
            if let Some((handle, _)) = &self.job {
                handle.cancel();
            }
        }
        if !self.open
            && let Some(focus) = self.previous_focus.take()
        {
            ctx.memory_mut(|m| m.request_focus(focus));
        }
    }
}

impl Drop for Dialog {
    fn drop(&mut self) {
        if let Some((handle, _)) = &self.job {
            handle.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::history::{Command, Created, History};
    use jvid_core::model::{AssetId, NewClip, Text, TrackKind, Transform};
    use jvid_core::time::Time;
    use std::cell::Cell;

    fn project(paths: &[PathBuf]) -> Project {
        let mut history = History::new(Project::default());
        for path in paths {
            history.apply(Command::AddAsset(path.clone())).unwrap();
        }
        history.project().clone()
    }

    fn request(project: Project, root: &Path) -> Request {
        Request::new(
            project,
            Some(root.join("original/project.jvid")),
            root.join("collected"),
        )
    }

    #[test]
    fn uppercase_source_project_extension_is_preserved() {
        let root = tempfile::tempdir().unwrap();
        let mut req = request(Project::default(), root.path());
        req.source_project = Some(root.path().join("original/EDIT.JVID"));
        req.project_name = "EDIT.JVID".into();
        let collected = collect(&req, || false, |_| {}).unwrap();
        assert_eq!(collected.project.file_name().unwrap(), "EDIT.JVID");
        assert_eq!(
            crate::project_file::load(&collected.project).unwrap(),
            req.project
        );
    }

    #[test]
    fn same_names_case_collisions_and_duplicate_bytes_reopen_offline() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original");
        let mut paths = Vec::new();
        for (folder, name, bytes) in [
            ("a", "Same.MKV", b"first".as_slice()),
            ("b", "same.mkv", b"other"),
            ("c", "Same.MKV", b"first"),
        ] {
            let dir = original.join(folder);
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join(name);
            fs::write(&path, bytes).unwrap();
            paths.push(path);
        }
        let source = project(&paths);
        let collected = collect(&request(source.clone(), root.path()), || false, |_| {}).unwrap();
        assert_eq!(source, project(&paths));
        assert_eq!(collected.manifest.files.len(), 3);
        assert_eq!(
            collected.manifest.files[0].relative_path,
            collected.manifest.files[2].relative_path
        );
        assert_ne!(
            collected.manifest.files[0].relative_path.to_lowercase(),
            collected.manifest.files[1].relative_path.to_lowercase()
        );
        assert_eq!(
            fs::read_dir(root.path().join("collected/media"))
                .unwrap()
                .count(),
            2
        );
        fs::remove_dir_all(&original).unwrap();
        let moved = root.path().join("moved");
        fs::rename(root.path().join("collected"), &moved).unwrap();
        let reopened = crate::project_file::load(&moved.join("project.jvid")).unwrap();
        for (asset, expected) in
            reopened
                .assets()
                .iter()
                .zip([b"first".as_slice(), b"other", b"first"])
        {
            assert!(asset.path.starts_with(&moved));
            assert_eq!(fs::read(&asset.path).unwrap(), expected);
        }
        assert_eq!(
            reopened
                .assets()
                .iter()
                .map(|a| a.display_name())
                .collect::<Vec<_>>(),
            vec!["Same.MKV", "same.mkv", "Same.MKV"]
        );
        for entry in &collected.manifest.files {
            assert!(!Path::new(&entry.relative_path).is_absolute());
            assert_eq!(
                hash(&moved.join(&entry.relative_path), &|| false).unwrap(),
                (entry.bytes, entry.sha256.clone())
            );
        }
        assert!(!moved.join(".collecting").exists());
    }

    #[test]
    fn nested_lut_effect_voiceover_and_permitted_font_survive_move_and_save_as() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original");
        fs::create_dir(&original).unwrap();
        let voice = original.join("Voiceover 1.wav");
        let size = 4_800u32 * 2;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + size).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&96_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&size.to_le_bytes());
        wav.resize(44 + size as usize, 0);
        fs::write(&voice, &wav).unwrap();
        let lut = original.join("look.cube");
        fs::write(
            &lut,
            "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
        )
        .unwrap();
        let (effect, source) = jvid_render::effect::BUNDLED[0];
        fs::write(original.join(effect), source).unwrap();
        let font = original.join("Inter.ttf");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../suite-ui/assets/Inter-Regular.ttf"),
            &font,
        )
        .unwrap();
        let baseline_family = "jvid-collection-baseline-inter";
        jvid_render::text::load_project_fonts(&[jvid_render::text::FontSource {
            family: baseline_family.into(),
            original_family: "Inter".into(),
            path: font.clone(),
        }])
        .unwrap();
        let mut history = History::new(project(&[voice]));
        let Created::Track(track) = history
            .apply(Command::AddTrack {
                kind: TrackKind::Video,
                name: "Video".into(),
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
                    source_out: Time(10),
                    start: Time(0),
                    link: None,
                },
            })
            .unwrap()
        else {
            panic!()
        };
        let mut transform = Transform {
            lut: Some(lut.to_string_lossy().into()),
            ..Default::default()
        };
        transform.effects.push(jvid_core::model::Effect {
            shader: Path::new(effect)
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .into(),
            params: Default::default(),
            enabled: true,
            mask: None,
        });
        history
            .apply(Command::SetClipProps(vec![(
                clip,
                transform,
                Default::default(),
            )]))
            .unwrap();
        history
            .apply(Command::Nest {
                ids: vec![clip],
                name: "Nested".into(),
                outer: "Main".into(),
            })
            .unwrap();
        history
            .apply(Command::AddText {
                name: "Title".into(),
                text: Text {
                    family: baseline_family.into(),
                    weight: 400,
                    ..Default::default()
                },
            })
            .unwrap();
        history
            .apply(Command::AddSolid {
                name: "Solid".into(),
                color: [1.0; 4],
            })
            .unwrap();
        let mut req = request(history.project().clone(), root.path());
        req.effect_directory = Some(original.clone());
        let baseline_text = history
            .project()
            .assets()
            .iter()
            .find_map(|a| a.text.as_ref())
            .unwrap();
        let before_pixels = jvid_render::text::rasterize(baseline_text, None, (320, 180));
        let collected = collect(&req, || false, |_| {}).unwrap();
        assert!(collected.manifest.unresolved.is_empty());
        assert_eq!(collected.manifest.files.len(), 4);
        fs::remove_dir_all(&original).unwrap();
        let moved = root.path().join("moved");
        fs::rename(&req.destination, &moved).unwrap();
        let reopened = crate::project_file::load(&moved.join("project.jvid")).unwrap();
        assert_eq!(reopened.sequences().len(), 2);
        let reopened_text = reopened
            .assets()
            .iter()
            .find_map(|a| a.text.as_ref())
            .unwrap();
        assert_eq!(
            *before_pixels,
            *jvid_render::text::rasterize(reopened_text, None, (320, 180))
        );
        assert_eq!(
            jvid_render::text::display_family(&reopened_text.family),
            "Inter"
        );
        let voice = &reopened.assets()[0].path;
        assert_eq!(fs::read(voice).unwrap(), wav);
        jvid_media::ffmpeg::init().unwrap();
        jvid_media::audio::AudioDecoder::open(voice, 0).unwrap();
        assert!(
            file::dependencies(&reopened)
                .unwrap()
                .iter()
                .all(|d| d.path.is_file() && d.path.starts_with(&moved))
        );
        let family = &reopened
            .assets()
            .iter()
            .find_map(|a| a.text.as_ref())
            .unwrap()
            .family;
        assert!(family.starts_with("jvid-collection-"));
        let sources = jvid_render::text::project_font_sources(std::slice::from_ref(family));
        assert_eq!(sources.len(), 1);
        assert!(sources[0].path.starts_with(&moved));
        crate::effects::with(|lib| {
            for dep in file::dependencies(&reopened)
                .unwrap()
                .iter()
                .filter(|d| d.kind == DependencyKind::Effect)
            {
                assert!(lib.get(dep.path.to_str().unwrap()).unwrap().is_ok());
            }
        });
        let save_as = root.path().join("saved-as.jvid");
        crate::project_file::save(&reopened, &save_as).unwrap();
        assert_eq!(crate::project_file::load(&save_as).unwrap(), reopened);
        let mut broken: serde_json::Value =
            serde_json::from_slice(&fs::read(&save_as).unwrap()).unwrap();
        broken.as_object_mut().unwrap().remove("fonts");
        let broken_path = root.path().join("missing-font-metadata.jvid");
        fs::write(&broken_path, serde_json::to_vec(&broken).unwrap()).unwrap();
        assert!(
            crate::project_file::load(&broken_path).is_err(),
            "a warm font cache must not hide missing project font links"
        );
        let moved_again = root.path().join("moved-again");
        fs::rename(&moved, &moved_again).unwrap();
        let reopened = crate::project_file::load(&moved_again.join("project.jvid")).unwrap();
        let family = reopened
            .assets()
            .iter()
            .find_map(|a| a.text.as_ref())
            .unwrap()
            .family
            .clone();
        assert!(
            jvid_render::text::project_font_sources(&[family])
                .iter()
                .all(|f| f.path.starts_with(&moved_again))
        );
        crate::project_file::save(&reopened, &save_as).unwrap();
        assert_eq!(crate::project_file::load(&save_as).unwrap(), reopened);
        let recollect = Request::new(reopened, Some(save_as), root.path().join("again"));
        assert_eq!(recollect.fonts.len(), 1);
        assert!(
            collect(&recollect, || false, |_| {})
                .unwrap()
                .manifest
                .unresolved
                .is_empty()
        );
    }

    #[test]
    fn missing_required_files_report_even_when_unbundled_fonts_allowed() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing.wav");
        let mut req = request(project(&[missing.clone()]), root.path());
        req.allow_unresolved = true;
        let failure = collect(&req, || false, |_| {}).unwrap_err();
        assert!(matches!(failure.kind, FailureKind::Unresolved));
        assert_eq!(failure.manifest.unresolved.len(), 1);
        assert_eq!(failure.manifest.unresolved[0].reason, Reason::Missing);
        assert!(!req.destination.exists());
        assert!(!missing.exists());
    }

    #[test]
    fn fonts_require_permission_and_incomplete_packages_remain_explicit() {
        let root = tempfile::tempdir().unwrap();
        let mut history = History::new(Project::default());
        history
            .apply(Command::AddText {
                name: "Title".into(),
                text: Text {
                    family: "Restricted Font".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let mut req = request(history.project().clone(), root.path());
        req.fonts.push(Font {
            family: "Restricted Font".into(),
            original_family: "Restricted Font".into(),
            path: root.path().join("not-read.ttf"),
            permitted: false,
        });
        let failure = collect(&req, || false, |_| {}).unwrap_err();
        assert!(matches!(failure.kind, FailureKind::Unresolved));
        assert_eq!(
            failure.manifest.unresolved[0].reason,
            Reason::FontPermission
        );
        req.allow_unresolved = true;
        let collected = collect(&req, || false, |_| {}).unwrap();
        assert_eq!(collected.manifest.unresolved.len(), 1);
        assert!(collected.manifest.files.is_empty());
        let unresolved: Vec<Unresolved> =
            serde_json::from_slice(&fs::read(req.destination.join(UNRESOLVED)).unwrap()).unwrap();
        assert_eq!(unresolved[0].reference, "Restricted Font");
    }

    #[test]
    fn cancellation_mid_copy_and_source_mutation_do_not_publish() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.wav");
        fs::write(&source, vec![1u8; 512 * 1024]).unwrap();
        let req = request(project(&[source.clone()]), root.path());
        let stop = Cell::new(false);
        let failure = collect(
            &req,
            || stop.get(),
            |p| {
                if p > 0.0 {
                    stop.set(true);
                }
            },
        )
        .unwrap_err();
        assert!(matches!(failure.kind, FailureKind::Cancelled));
        assert!(!req.destination.exists());
        assert_eq!(fs::metadata(&source).unwrap().len(), 512 * 1024);
        let changed = Cell::new(false);
        let failure = collect(
            &req,
            || false,
            |p| {
                assert!(!req.destination.join("project.jvid").exists());
                if p > 0.0 && !changed.replace(true) {
                    fs::write(&source, b"source changed during copy").unwrap();
                }
            },
        )
        .unwrap_err();
        assert!(matches!(failure.kind, FailureKind::Io(_)));
        assert!(!req.destination.exists());
        assert_eq!(fs::read(&source).unwrap(), b"source changed during copy");
    }

    #[test]
    fn existing_destination_and_existing_empty_directory_are_untouched() {
        let root = tempfile::tempdir().unwrap();
        let req = request(Project::default(), root.path());
        fs::create_dir(&req.destination).unwrap();
        assert!(collect(&req, || false, |_| {}).is_err());
        assert!(req.destination.is_dir());
        assert_eq!(fs::read_dir(&req.destination).unwrap().count(), 0);
        fs::write(req.destination.join("project.jvid"), b"existing").unwrap();
        assert!(collect(&req, || false, |_| {}).is_err());
        assert_eq!(
            fs::read(req.destination.join("project.jvid")).unwrap(),
            b"existing"
        );
    }

    #[test]
    fn invalid_font_and_destination_name_fail_before_project_publication() {
        let root = tempfile::tempdir().unwrap();
        let mut req = request(Project::default(), root.path());
        req.project_name = "../escape.jvid".into();
        assert!(collect(&req, || false, |_| {}).is_err());
        assert!(!req.destination.exists());
        let font = root.path().join("bad.ttf");
        fs::write(&font, b"not a font").unwrap();
        let mut history = History::new(Project::default());
        history
            .apply(Command::AddText {
                name: "Title".into(),
                text: Text {
                    family: "Example".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        req.project = history.project().clone();
        req.project_name = "project.jvid".into();
        req.fonts.push(Font {
            family: "Example".into(),
            original_family: "Example".into(),
            path: font,
            permitted: true,
        });
        assert!(matches!(
            collect(&req, || false, |_| {}).unwrap_err().kind,
            FailureKind::Io(_)
        ));
        assert!(!req.destination.exists());
    }

    #[test]
    fn background_job_cancel_cleans_partial_copy() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("large.wav");
        let file = File::create(&source).unwrap();
        file.set_len(64 * 1024 * 1024).unwrap();
        let req = request(project(&[source]), root.path());
        let pool = Pool::new(1);
        let (handle, receiver) = submit(&pool, req.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while handle.progress() == 0.0 && !handle.is_finished() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        handle.cancel();
        let failure = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .unwrap_err();
        assert!(matches!(failure.kind, FailureKind::Cancelled));
        assert!(!req.destination.exists());
    }
    #[test]
    fn failed_report_write_never_publishes_project_or_changes_original() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.wav");
        fs::write(&source, b"original").unwrap();
        let req = request(project(&[source.clone()]), root.path());
        let injected = Cell::new(false);
        let failure = collect(
            &req,
            || false,
            |progress| {
                if progress >= 0.95 && !injected.replace(true) {
                    fs::create_dir(req.destination.join(".collecting").join(UNRESOLVED)).unwrap();
                }
            },
        )
        .unwrap_err();
        assert!(matches!(failure.kind, FailureKind::Io(_)));
        assert!(!req.destination.exists());
        assert_eq!(fs::read(&source).unwrap(), b"original");
    }

    #[test]
    fn missing_nested_lut_and_shader_are_both_reported() {
        let root = tempfile::tempdir().unwrap();
        let media = root.path().join("media.wav");
        fs::write(&media, b"media").unwrap();
        let mut history = History::new(project(&[media]));
        let Created::Track(track) = history
            .apply(Command::AddTrack {
                kind: TrackKind::Video,
                name: "Video".into(),
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
                    source_out: Time(10),
                    start: Time(0),
                    link: None,
                },
            })
            .unwrap()
        else {
            panic!()
        };
        let mut transform = Transform {
            lut: Some(root.path().join("missing.cube").to_string_lossy().into()),
            ..Default::default()
        };
        transform.effects.push(jvid_core::model::Effect {
            shader: "missing_shader".into(),
            params: Default::default(),
            enabled: true,
            mask: None,
        });
        history
            .apply(Command::SetClipProps(vec![(
                clip,
                transform,
                Default::default(),
            )]))
            .unwrap();
        history
            .apply(Command::Nest {
                ids: vec![clip],
                name: "Nested".into(),
                outer: "Main".into(),
            })
            .unwrap();
        let mut req = request(history.project().clone(), root.path());
        req.effect_directory = Some(root.path().to_owned());
        req.allow_unresolved = true;
        let failure = collect(&req, || false, |_| {}).unwrap_err();
        assert!(matches!(failure.kind, FailureKind::Unresolved));
        assert_eq!(failure.manifest.unresolved.len(), 2);
        assert!(
            failure
                .manifest
                .unresolved
                .iter()
                .any(|u| u.kind == Kind::Lut)
        );
        assert!(
            failure
                .manifest
                .unresolved
                .iter()
                .any(|u| u.kind == Kind::Effect)
        );
        assert!(!req.destination.exists());
    }

    #[test]
    fn narrow_dialog_keeps_actions_inside_view_and_snapshot_unchanged() {
        use egui_kittest::kittest::Queryable;
        let artifact = std::env::var("JVID_COLLECT_ARTIFACT").ok();
        let render_artifact = artifact.is_some() && suite_ui::test_support::gpu();
        let language = suite_ui::i18n::language();
        if let Ok(value) = std::env::var("JVID_COLLECT_LANGUAGE") {
            suite_ui::i18n::set_language(&value);
        }
        let mut dialog = Dialog::default();
        let snapshot = Project::default();
        dialog.open(&snapshot, None);
        let pool = Pool::new(1);
        let mut fresh = true;
        let mut harness = egui_kittest::Harness::builder()
            .with_size([300.0, 720.0])
            .build_ui_state(
                move |ui, dialog: &mut Dialog| {
                    if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                        dialog.show(ui.ctx(), &pool);
                    }
                },
                dialog,
            );
        harness.run_steps(3);
        let area = harness
            .ctx
            .memory(|m| m.area_rect(egui::Id::new("collect_project")))
            .unwrap();
        assert!(area.left() >= 0.0 && area.right() <= 300.0, "{area:?}");
        let start =
            harness.get_by_role_and_label(egui::accesskit::Role::Button, tr!("collect.start"));
        assert!(area.contains_rect(start.rect()));
        assert_eq!(harness.state().request.as_ref().unwrap().project, snapshot);
        if render_artifact {
            harness.render().unwrap().save(artifact.unwrap()).unwrap();
        }
        harness
            .get_by_role_and_label(egui::accesskit::Role::Button, tr!("collect.close"))
            .click();
        harness.run();
        assert!(!harness.state().is_open());
        suite_ui::i18n::set_language(language);
    }
    #[test]
    fn unbundled_fallback_fonts_are_reported() {
        let root = tempfile::tempdir().unwrap();
        let mut history = History::new(Project::default());
        history
            .apply(Command::AddText {
                name: "Title".into(),
                text: Text {
                    family: "Inter".into(),
                    content: "\u{10FFFF}".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let mut req = request(history.project().clone(), root.path());
        req.fonts.push(Font {
            family: "Inter".into(),
            original_family: "Inter".into(),
            path: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../suite-ui/assets/Inter-Regular.ttf"),
            permitted: true,
        });
        let failure = collect(&req, || false, |_| {}).unwrap_err();
        assert!(matches!(failure.kind, FailureKind::Unresolved));
        assert!(
            failure
                .manifest
                .unresolved
                .iter()
                .any(|u| u.reason == Reason::FontCoverage)
        );
        assert!(!req.destination.exists());
    }
    #[test]
    fn shipped_shader_without_external_override_is_captured() {
        let root = tempfile::tempdir().unwrap();
        let mut history = History::new(Project::default());
        history
            .apply(Command::AddSolid {
                name: "Solid".into(),
                color: [1.0; 4],
            })
            .unwrap();
        let Created::Track(track) = history
            .apply(Command::AddTrack {
                kind: TrackKind::Video,
                name: "Video".into(),
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
                    source_out: Time(10),
                    start: Time(0),
                    link: None,
                },
            })
            .unwrap()
        else {
            panic!()
        };
        let (name, source) = jvid_render::effect::BUNDLED[0];
        let mut transform = Transform::default();
        transform.effects.push(jvid_core::model::Effect {
            shader: Path::new(name)
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .into(),
            params: Default::default(),
            enabled: true,
            mask: None,
        });
        history
            .apply(Command::SetClipProps(vec![(
                clip,
                transform,
                Default::default(),
            )]))
            .unwrap();
        let mut req = request(history.project().clone(), root.path());
        req.effect_directory = None;
        let collected = collect(&req, || false, |_| {}).unwrap();
        assert_eq!(collected.manifest.files.len(), 1);
        assert_eq!(collected.manifest.files[0].kind, Kind::Effect);
        assert_eq!(
            fs::read_to_string(
                req.destination
                    .join(&collected.manifest.files[0].relative_path)
            )
            .unwrap(),
            source
        );
        let reopened = crate::project_file::load(&collected.project).unwrap();
        crate::effects::with(|library| {
            let shader = file::dependencies(&reopened)
                .unwrap()
                .into_iter()
                .find(|d| d.kind == DependencyKind::Effect)
                .unwrap();
            let key = shader.path.to_str().unwrap();
            assert!(library.shipped(key));
            let shader = library.get(key).unwrap().as_ref().unwrap();
            let stem = Path::new(name).file_stem().unwrap().to_str().unwrap();
            assert_eq!(
                crate::effects::name(key, true, shader),
                crate::i18n::CATALOG.tr(&format!("bundled.{stem}"))
            );
        });
    }
}
