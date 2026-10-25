//! Export: renders a timeline range frame by frame at project size, mixes its audio, encodes and
//! muxes both into a file, then re-probes the file to check it. Runs as a [`jvid_jobs`] job.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use eframe::egui_wgpu::wgpu;
use jvid_audio::{Limiter, Meter};
use jvid_core::eval::frame_at;
use jvid_core::model::{ProbeInfo, Project};
use jvid_core::preset::{Preset, VideoCodec};
use jvid_core::time::{FLICKS_PER_SECOND, Rate, Time};
use jvid_jobs::{Ctx, Handle, Pool, Priority};
use jvid_media::audio::AudioDecoder;
use jvid_media::decode::DecoderPool;
use jvid_media::encode::{CreateError, EncodeError, Encoder};
use jvid_render::composite::Readback;

use crate::i18n::tr;
use crate::playback::{Audio, ClipAudio, Gpu, Pixels, Shown, resolve, sample};

/// What to export: `range` is a half-open timeline span, rounded out to whole output frames.
#[derive(Clone)]
pub struct Request {
    pub path: PathBuf,
    pub range: (Time, Time),
    pub preset: Preset,
    /// Integrated loudness target in LUFS; `None` exports the mix as is.
    pub loudness: Option<f64>,
    /// Copy the sources instead of rendering; the preset and loudness are then unused.
    pub lossless: Option<Lossless>,
    /// Write subtitle tracks to an `.srt` beside the file instead of burning them in.
    pub sidecar: bool,
}

/// Normalized exports' true-peak ceiling, in dBTP.
const CEILING: f32 = -1.0;

/// Built-in presets, then user presets from `presets/*.toml` in the config directory.
pub fn presets() -> Vec<Preset> {
    let dir = crate::settings_file::path().and_then(|p| Some(p.parent()?.join("presets")));
    let mut all = jvid_core::preset::builtin();
    all.extend(dir.map(|d| user_presets(&d)).unwrap_or_default());
    all
}

/// Catalog keys for the names of `jvid_core::preset::builtin()`, in its order.
const BUILT_IN_NAMES: [&str; 12] = [
    "preset.highlight_1080",
    "preset.highlight_1440",
    "preset.screen",
    "preset.youtube_1080",
    "preset.youtube_4k",
    "preset.shorts",
    "preset.prores",
    "preset.gif",
    "preset.audio_wav",
    "preset.audio_wav24",
    "preset.audio_mp3",
    "preset.audio_aac",
];

/// The name `presets()[i]` shows: a built-in's in the UI language, a user preset's as written.
pub fn preset_name(i: usize, p: &Preset) -> &str {
    BUILT_IN_NAMES
        .get(i)
        .map_or(&p.name, |k| crate::i18n::CATALOG.tr(k))
}

/// The name `p` shows, where `p` is one of `presets`, perhaps with options added since.
pub fn shown_name<'a>(presets: &'a [Preset], p: &'a Preset) -> &'a str {
    let i = presets.iter().position(|q| q.name == p.name);
    i.map_or(&p.name, |i| preset_name(i, &presets[i]))
}

/// Every `*.toml` preset in `dir`, by file name; a file that doesn't parse is logged and skipped.
fn user_presets(dir: &Path) -> Vec<Preset> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    paths.sort();
    paths
        .iter()
        .filter_map(|p| {
            let text = std::fs::read_to_string(p).map_err(|e| e.to_string());
            match text.and_then(|t| Preset::from_toml(&t).map_err(|e| e.to_string())) {
                Ok(preset) => Some(preset),
                Err(e) => {
                    tracing::warn!("preset {}: {e}", p.display());
                    None
                }
            }
        })
        .collect()
}

/// Queues an export of `project` on `pool`; the result arrives on the receiver when it's done.
pub fn submit(
    pool: &Pool,
    project: Project,
    req: Request,
    gpu: (wgpu::Device, wgpu::Queue),
) -> (Handle, Receiver<Result<ProbeInfo, Error>>) {
    let (tx, rx) = channel();
    let key = format!("export:{}", req.path.display());
    let handle = pool.submit(key, Priority::Interactive, move |ctx| {
        let _ = tx.send(run(&project, &req, &gpu.0, &gpu.1, ctx));
    });
    (handle, rx)
}

/// A device for exporting without a window: the default adapter, else a software one.
pub fn headless_gpu() -> Result<(wgpu::Device, wgpu::Queue), String> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = block_on(instance.request_adapter(&Default::default())).or_else(|_| {
        block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            force_fallback_adapter: true,
            ..Default::default()
        }))
    });
    let adapter = adapter.map_err(|e| format!("no GPU adapter: {e}"))?;
    block_on(adapter.request_device(&Default::default())).map_err(|e| format!("GPU device: {e}"))
}

/// wgpu's native futures are ready when polled, so a busy poll stands in for an executor.
pub(crate) fn block_on<F: std::future::Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

/// Why an export didn't produce a file.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Cancelled,
    Failed(String),
}

impl From<String> for Error {
    fn from(e: String) -> Self {
        Self::Failed(e)
    }
}

/// Internal failure provenance; only a VAAPI upload/codec failure permits replay.
#[derive(Debug)]
enum AttemptError {
    Export(Error),
    Vaapi(EncodeError),
}

impl From<Error> for AttemptError {
    fn from(e: Error) -> Self {
        Self::Export(e)
    }
}

impl From<String> for AttemptError {
    fn from(e: String) -> Self {
        Self::Export(Error::Failed(e))
    }
}

impl From<EncodeError> for AttemptError {
    fn from(e: EncodeError) -> Self {
        match e {
            EncodeError::Vaapi(_) => Self::Vaapi(e),
            EncodeError::Other(_) => Self::Export(Error::Failed(err(e))),
        }
    }
}

/// A private sibling directory created exclusively for this attempt. Every file cleaned up on
/// failure belongs to this attempt, including a target-size pass's saved best output.
struct AttemptFiles {
    dir: PathBuf,
    output: PathBuf,
}

impl AttemptFiles {
    fn new(destination: &Path) -> Result<Self, String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = destination.file_name().ok_or(tr!("export.no_file_name"))?;
        loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = parent.join(format!(".jvid-export-{}-{id}", std::process::id()));
            match std::fs::create_dir(&dir) {
                Ok(()) => {
                    return Ok(Self {
                        output: dir.join(name),
                        dir,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(tr!("export.attempt_error", error = e)),
            }
        }
    }
}

impl Drop for AttemptFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Refuse both path aliases (including symlinks) and existing-file aliases such as hard links.
/// Check every project asset, including nested sequences' sources, before creating any output.
fn check_destination(project: &Project, destination: &Path) -> Result<(), Error> {
    let identity_error = |error| {
        Error::Failed(tr!(
            "export.identity_error",
            path = destination.display(),
            error = error
        ))
    };
    let destination_identity = match same_file::Handle::from_path(destination) {
        Ok(handle) => Some(handle),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(identity_error(e)),
    };
    let destination_path = std::fs::canonicalize(destination).ok();
    for asset in project
        .assets()
        .iter()
        .filter(|a| !a.path.as_os_str().is_empty())
    {
        let same_path = destination == asset.path
            || destination_path.as_ref().is_some_and(|path| {
                std::fs::canonicalize(&asset.path).is_ok_and(|source| source == *path)
            });
        let same_file = match &destination_identity {
            Some(destination) => match same_file::Handle::from_path(&asset.path) {
                Ok(source) => source == *destination,
                // An offline asset cannot alias an existing destination.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(e) => return Err(identity_error(e)),
            },
            None => false,
        };
        if same_path || same_file {
            return Err(tr!(
                "export.source_collision",
                path = destination.display(),
                source = asset.path.display()
            )
            .into());
        }
    }
    Ok(())
}

/// Replays the entire shared job once in software, with fresh encoder/render/audio state.
/// Preset overrides live in a request clone and never affect project or saved dialog settings.
fn replay<T>(
    project: &Project,
    req: &Request,
    ctx: &Ctx,
    mut attempt: impl FnMut(&Request) -> Result<T, AttemptError>,
) -> Result<T, Error> {
    check_destination(project, &req.path)?;
    let mut request = req.clone();
    for retry in 0..=1 {
        if ctx.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let files = AttemptFiles::new(&req.path)?;
        request.path.clone_from(&files.output);
        let result = attempt(&request);
        // Cancellation wins even if the driver fails in the same frame or during final flush.
        if ctx.is_cancelled() {
            return Err(Error::Cancelled);
        }
        match result {
            Ok(info) => {
                // The destination may have become a source alias while the job ran.
                check_destination(project, &req.path)?;
                if ctx.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                std::fs::rename(&files.output, &req.path).map_err(|e| {
                    tr!("export.publish_error", path = req.path.display(), error = e)
                })?;
                return Ok(info);
            }
            Err(AttemptError::Vaapi(error))
                if retry == 0
                    && request
                        .preset
                        .options
                        .get("encoder")
                        .is_none_or(|e| e != "software") =>
            {
                tracing::warn!(%error, "VAAPI export failed; retrying once in software");
                request
                    .preset
                    .options
                    .insert("encoder".into(), "software".into());
                ctx.set_progress(0.0);
            }
            Err(AttemptError::Vaapi(error)) => return Err(Error::Failed(err(error))),
            Err(AttemptError::Export(error)) => return Err(error),
        }
    }
    unreachable!("each attempt returns or the first retries")
}

/// Removes the file it guards when dropped, including by a panic.
struct Partial<'a>(&'a Path);

impl Drop for Partial<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

/// Exports and validates in a private attempt directory, then publishes the finished file.
/// Failure, cancellation and panic discard only attempt files and preserve any existing output.
pub fn run(
    project: &Project,
    req: &Request,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    ctx: &Ctx,
) -> Result<ProbeInfo, Error> {
    if !req.sidecar {
        return encode(project, req, device, queue, ctx);
    }
    check_destination(project, &req.path.with_extension("srt"))?;
    let info = encode(&project.without_subtitles(), req, device, queue, ctx)?;
    let srt = jvid_core::srt::write(&project.subtitle_cues(), req.range);
    let path = req.path.with_extension("srt");
    std::fs::write(&path, srt)
        .map_err(|e| tr!("export.write_error", path = path.display(), error = e))?;
    Ok(info)
}

fn encode(
    project: &Project,
    req: &Request,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    ctx: &Ctx,
) -> Result<ProbeInfo, Error> {
    if let Some(mode) = req.lossless {
        return lossless::run(project, req, mode, ctx);
    }
    // HDR output is a project setting, so it applies to whichever preset was picked.
    let mut req = req.clone();
    if project.settings().hdr_output {
        req.preset = req.preset.hdr();
    }
    replay(project, &req, ctx, |req| {
        encode_attempt(project, req, device, queue, ctx)
    })
}

fn encode_attempt(
    project: &Project,
    req: &Request,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    ctx: &Ctx,
) -> Result<ProbeInfo, AttemptError> {
    let s = project.settings();
    let (first, end) = frames(project, req);
    if end <= first {
        return Err(tr!("export.empty_range").to_string().into());
    }
    jvid_media::ffmpeg::init().map_err(|e| e.to_string())?;
    // Otherwise it fails as a bare "Encoder not found".
    if req.preset.options.contains_key("hdr") && !jvid_media::encode::hdr10_available() {
        return Err(tr!("export.hdr_no_encoder").to_string().into());
    }
    let enc = Encoder::create(&req.path, (s.width, s.height), s.frame_rate, &req.preset)
        .map_err(|e| create_error(&req.path, e))?;
    let partial = Partial(&req.path);
    let gain = match req.loudness.filter(|_| req.preset.audio.is_some()) {
        Some(target) => measure(project, req, ctx)?.map(|lufs| 10f64.powf((target - lufs) / 20.0)),
        None => None,
    };
    let limiter = || gain.map(|g| Limiter::new(g as f32, CEILING));
    match sized(project, req)? {
        None => render(project, req, enc, limiter(), device, queue, ctx)?,
        Some((target, audio, mut bits)) => {
            drop(enc);
            // Encoders only roughly hit an average bit rate, so each pass rescales the video rate
            // by how far the last file missed, until one lands in the window.
            // shortcut: progress restarts each pass; weight it by pass if users notice.
            let want = target as f64 * (1.0 - OVERHEAD) - audio;
            // The fullest pass under the target so far, moved aside so later passes can't lose it.
            let best = PathBuf::from(format!("{}.best", req.path.display()));
            let _best = Partial(&best);
            let (mut under, mut over) = (None::<(u64, f64)>, None::<(u64, f64)>);
            for pass in 1..=PASSES {
                let mut preset = req.preset.clone();
                preset.options.remove("target_size");
                preset.options.remove("crf");
                preset.options.insert("b".into(), bits.to_string());
                let req = Request {
                    preset,
                    ..req.clone()
                };
                let enc =
                    Encoder::create(&req.path, (s.width, s.height), s.frame_rate, &req.preset)
                        .map_err(|e| create_error(&req.path, e))?;
                render(project, &req, enc, limiter(), device, queue, ctx)?;
                let size = std::fs::metadata(&req.path)
                    .map_err(|e| e.to_string())?
                    .len() as f64;
                tracing::info!(pass, bits, size, target, "target-size pass");
                if size > target as f64 {
                    over = Some((bits, size - audio));
                } else {
                    if size >= MIN_FILL * target as f64 {
                        break;
                    }
                    // No bigger for more bits: the encoder is at its best quality, as simple
                    // content can't use the whole rate.
                    if under.is_some_and(|(b, v)| b < bits && size - audio < v * 1.01) {
                        break;
                    }
                    under = Some((bits, size - audio));
                    std::fs::rename(&req.path, &best).map_err(|e| e.to_string())?;
                }
                if pass == PASSES {
                    if under.is_none() {
                        return Err(tr!("export.over_target", size = size).into());
                    }
                    std::fs::rename(&best, &req.path).map_err(|e| e.to_string())?;
                    break;
                }
                bits = next_bits(under, over, want);
            }
        }
    }
    let info = validate(project, req)?;
    std::mem::forget(partial);
    Ok(info)
}

/// Share of a target size kept back for container headers and indexes, which run well under 1%
/// for MP4, MOV and MKV, plus encoder overshoot.
const OVERHEAD: f64 = 0.015;

/// The video bit rate that fits `secs` of `p` into `target` bytes, or `None` when the audio
/// alone would fill it.
pub fn target_bitrate(p: &Preset, target: u64, secs: f64) -> Option<u64> {
    let audio = p.audio.map_or(0.0, dialog::audio_bitrate);
    let video = target as f64 * 8.0 * (1.0 - OVERHEAD) / secs - audio;
    (video >= 1000.0).then_some(video as u64)
}

/// The next pass's video bit rate from the closest passes (bit rate, video bytes) that came in
/// `under` and `over` the target, aiming at `want` video bytes: between the two along their line
/// once both exist, else in proportion from the one there is, stepping at most 2x.
fn next_bits(under: Option<(u64, f64)>, over: Option<(u64, f64)>, want: f64) -> u64 {
    let next = match (under, over) {
        (Some((lb, lv)), Some((hb, hv))) => {
            let (lb, hb) = (lb as f64, hb as f64);
            let t = ((want - lv) / (hv - lv).max(1.0)).clamp(0.1, 0.9);
            lb + t * (hb - lb)
        }
        (Some((b, v)), None) | (None, Some((b, v))) => {
            let b = b as f64;
            (b * want / v.max(1.0)).clamp(b / 2.0, b * 2.0)
        }
        (None, None) => unreachable!("called after a pass"),
    };
    next.max(1000.0) as u64
}

/// Encodes a target-size export gets before giving up.
const PASSES: u32 = 5;
/// A target-size export should fill at least this share of the target.
const MIN_FILL: f64 = 0.97;

/// With a `target_size` option: the target in bytes, the audio's bytes and the first pass's
/// video bit rate. `None` without the option.
fn sized(project: &Project, req: &Request) -> Result<Option<(u64, f64, u64)>, Error> {
    let p = &req.preset;
    let Some(target) = p.options.get("target_size") else {
        return Ok(None);
    };
    let target = target
        .parse::<u64>()
        .map_err(|_| tr!("export.bad_target", target = target))?;
    let rate = rate(project, req);
    let (first, end) = frames(project, req);
    let secs = (rate.time_floor(end) - rate.time_floor(first)).0 as f64 / FLICKS_PER_SECOND as f64;
    let bits = target_bitrate(p, target, secs)
        .ok_or_else(|| tr!("export.target_too_small").to_string())?;
    let audio = p.audio.map_or(0.0, dialog::audio_bitrate) * secs / 8.0;
    Ok(Some((target, audio, bits)))
}

/// The output frame rate.
fn rate(project: &Project, req: &Request) -> Rate {
    req.preset
        .frame_rate
        .unwrap_or(project.settings().frame_rate)
}

/// Output frames `[first, end)` covering the range.
fn frames(project: &Project, req: &Request) -> (i64, i64) {
    let rate = rate(project, req);
    let (start, end) = req.range;
    (rate.index_floor(start), rate.index_floor(end - Time(1)) + 1)
}

/// Each output frame's timeline time and its samples `[a, b)`, so audio and video advance together
/// and end together.
fn spans(project: &Project, req: &Request) -> impl Iterator<Item = (i64, Time, i64, i64)> {
    let rate = rate(project, req);
    let (first, end) = frames(project, req);
    (first..end).map(move |k| {
        let time = rate.time_floor(k);
        (k, time, sample(time), sample(rate.time_floor(k + 1)))
    })
}

/// First pass for normalization: the range's integrated loudness, or `None` if it's silent.
fn measure(project: &Project, req: &Request, ctx: &Ctx) -> Result<Option<f64>, Error> {
    let mut audio = Audio::new(Box::new(blocking));
    let mut meter = Meter::default();
    let mut mix = Vec::new();
    for (_, time, a, b) in spans(project, req) {
        if ctx.is_cancelled() {
            return Err(Error::Cancelled);
        }
        mix.clear();
        mix.resize((b - a) as usize * 2, 0.0);
        audio.prime(project, time);
        audio.mix(project, a, &mut mix);
        meter.push(&mix);
    }
    Ok(meter.integrated())
}

fn render(
    project: &Project,
    req: &Request,
    mut enc: Encoder,
    mut limiter: Option<Limiter>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    ctx: &Ctx,
) -> Result<(), AttemptError> {
    let (first, end) = frames(project, req);
    let decoders = DecoderPool::hdr();
    let mut audio = Audio::new(Box::new(blocking));
    // Audio-only presets never touch the GPU.
    let mut gpu = enc.has_video().then(|| {
        let mut g = Gpu::new(device);
        g.pin_effects = true;
        g
    });
    let mut readback = Readback::default();
    let (mut mix, mut limited) = (Vec::new(), Vec::new());
    for (k, time, a, b) in spans(project, req) {
        if ctx.is_cancelled() {
            return Err(Error::Cancelled.into());
        }
        if enc.has_video() {
            let layers =
                resolve::<String, _>(project, frame_at(project, time), &mut |asset, layer| {
                    let f = decoders
                        .frame_at(&asset.path, layer.source_time)
                        .map_err(|e| {
                            tr!(
                                "export.decode_error",
                                path = asset.path.display(),
                                error = e
                            )
                        })?;
                    Ok(Some(Pixels::Frame(Arc::new(f))))
                })?;
            let tex = gpu.get_or_insert_with(|| Gpu::new(device)).draw(
                device,
                queue,
                project.settings(),
                1,
                &Shown {
                    index: k,
                    layers,
                    // Decoded synchronously with frame_at, never a keyframe stand-in.
                    exact: true,
                },
            );
            if let Some(rgba) = readback.push(device, queue, tex) {
                enc.video(&rgba)?;
            }
        }
        mix.clear();
        mix.resize((b - a) as usize * 2, 0.0);
        // Closes clips no longer heard, as playback does, so a long timeline doesn't pile up decoders.
        audio.prime(project, time);
        audio.mix(project, a, &mut mix);
        match &mut limiter {
            Some(l) => {
                limited.clear();
                l.process(&mix, &mut limited);
                enc.audio(&limited).map_err(err)?;
            }
            None => enc.audio(&mix).map_err(err)?,
        }
        ctx.set_progress((k + 1 - first) as f32 / (end - first) as f32);
    }
    if let Some(rgba) = readback.take(device) {
        enc.video(&rgba)?;
    }
    if let Some(l) = &mut limiter {
        limited.clear();
        l.flush(&mut limited);
        enc.audio(&limited).map_err(err)?;
    }
    if ctx.is_cancelled() {
        return Err(Error::Cancelled.into());
    }
    enc.finish()?;
    if ctx.is_cancelled() {
        return Err(Error::Cancelled.into());
    }
    Ok(())
}

fn create_error(path: &Path, error: CreateError) -> String {
    let error = match error {
        CreateError::NoVideoEncoder(codec) => dialog::codec_unavailable(codec),
        other => other.to_string(),
    };
    tr!("export.create_error", path = path.display(), error = error)
}

fn validate_video_family(expected: VideoCodec, actual: &str) -> Result<(), Error> {
    if actual != jvid_media::encode::video_codec_id(expected).name() {
        return Err(tr!(
            "export.codec_wrong",
            expected = dialog::codec_name(expected),
            actual = actual
        )
        .into());
    }
    Ok(())
}

fn err(e: impl std::fmt::Display) -> String {
    tr!("export.encode_error", error = e)
}

/// Re-probes the written file: it must open, have the preset's streams, and last the range to
/// within one frame.
fn validate(project: &Project, req: &Request) -> Result<ProbeInfo, Error> {
    let info = jvid_media::probe::probe(&req.path)
        .map_err(|e| tr!("export.output_error", error = crate::import::describe(&e)))?;
    let rate = rate(project, req);
    let (first, end) = frames(project, req);
    let want = rate.time_floor(end) - rate.time_floor(first);
    let frame = rate.time_floor(1);
    let p = &req.preset;
    if info.width.is_some() != p.video.is_some() || (info.audio_streams > 0) != p.audio.is_some() {
        return Err(tr!("export.streams_wrong").to_string().into());
    }
    if let Some(expected) = p.video {
        for stream in &info.streams {
            if let jvid_core::model::StreamInfo::Video(video) = stream {
                validate_video_family(expected, &video.codec)?;
            }
        }
    }
    if (info.duration - want).0.abs() > frame.0 {
        let secs = |t: Time| t.0 as f64 / FLICKS_PER_SECOND as f64;
        let (got, want) = (
            format!("{:.3}", secs(info.duration)),
            format!("{:.3}", secs(want)),
        );
        return Err(tr!("export.duration_wrong", got = got, want = want).into());
    }
    Ok(info)
}

/// Reads one clip's audio synchronously, for export: unlike playback's source it always fills
/// all of `out` that the file has.
fn blocking(path: &Path, nth: usize) -> ClipAudio {
    let mut dec = AudioDecoder::open(path, nth)
        .map_err(|e| tracing::debug!("export audio of {}: {e}", path.display()))
        .ok();
    let mut buf = crate::transport::AudioBuffer::default();
    Box::new(move |start, out| {
        out.fill(0.0);
        let Some(d) = &mut dec else { return };
        if buf.needs_seek(start, 0) {
            if let Err(e) = d.seek(start) {
                return tracing::warn!("export audio seek: {e}");
            }
            buf.reset(start);
        }
        buf.discard_before(start);
        while buf.end() < start.saturating_add(out.len() as i64 / 2) {
            match d.next_chunk() {
                Ok(Some(c)) => buf.push(c),
                Ok(None) => break,
                Err(e) => return tracing::warn!("export audio decode: {e}"),
            }
        }
        buf.copy(start, out);
    })
}

pub mod dialog;
pub mod lossless;
pub mod queue;

pub use lossless::Lossless;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod audit_b10a;
