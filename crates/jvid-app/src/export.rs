//! Export: renders a timeline range frame by frame at project size, mixes its audio, encodes and
//! muxes both into a file, then re-probes the file to check it. Runs as a [`jvid_jobs`] job.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use eframe::egui_wgpu::wgpu;
use jvid_core::eval::video_at;
use jvid_core::model::{ProbeInfo, Project};
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use jvid_jobs::{Ctx, Handle, Pool, Priority};
use jvid_media::audio::AudioDecoder;
use jvid_media::decode::DecoderPool;
use jvid_media::encode::Encoder;
use jvid_render::composite::Readback;
use jvid_render::solid;

use crate::playback::{Audio, ClipAudio, Gpu, Pixels, Shown, sample};

/// What to export: `range` is a half-open timeline span, rounded out to whole frames.
#[derive(Clone)]
pub struct Request {
    pub path: PathBuf,
    pub range: (Time, Time),
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

/// Why an export didn't produce a file.
#[derive(Debug, PartialEq)]
pub enum Error {
    Cancelled,
    Failed(String),
}

impl From<String> for Error {
    fn from(e: String) -> Self {
        Self::Failed(e)
    }
}

/// Removes the file it guards when dropped, including by a panic.
struct Partial<'a>(&'a Path);

impl Drop for Partial<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

/// Exports and validates; on failure, cancellation or panic after the file was created, the
/// partial file is removed. A failure before that leaves whatever was at `req.path` alone.
pub fn run(
    project: &Project,
    req: &Request,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    ctx: &Ctx,
) -> Result<ProbeInfo, Error> {
    let s = project.settings();
    let (first, end) = frames(project, req.range);
    if end <= first {
        return Err("the export range is empty".to_string().into());
    }
    jvid_media::ffmpeg::init().map_err(|e| e.to_string())?;
    let enc = Encoder::create(&req.path, (s.width, s.height), s.frame_rate, true)
        .map_err(|e| format!("couldn't create {}: {e}", req.path.display()))?;
    let partial = Partial(&req.path);
    render(project, req, enc, device, queue, ctx)?;
    let info = validate(project, req)?;
    std::mem::forget(partial);
    Ok(info)
}

/// Timeline frames `[first, end)` covering `range`.
fn frames(project: &Project, (start, end): (Time, Time)) -> (i64, i64) {
    let rate = project.settings().frame_rate;
    (rate.index_floor(start), rate.index_floor(end - Time(1)) + 1)
}

fn render(
    project: &Project,
    req: &Request,
    mut enc: Encoder,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    ctx: &Ctx,
) -> Result<(), Error> {
    let rate = project.settings().frame_rate;
    let (first, end) = frames(project, req.range);
    let decoders = DecoderPool::default();
    let mut audio = Audio::new(Box::new(|p| blocking(p)));
    let mut gpu = Gpu::new(device);
    let mut readback = Readback::default();
    let mut mix = Vec::new();
    for k in first..end {
        if ctx.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let time = rate.time_floor(k);
        let mut layers = Vec::new();
        for layer in video_at(project.sequence(), time) {
            let Some(asset) = project.asset(layer.asset) else {
                continue;
            };
            if let Some(c) = layer.color.or(asset.solid) {
                layers.push((Pixels::Solid(solid::rgba8(c)), layer.transform));
                continue;
            }
            let f = decoders
                .frame_at(&asset.path, layer.source_time)
                .map_err(|e| format!("decoding {}: {e}", asset.path.display()))?;
            layers.push((Pixels::Frame(Arc::new(f)), layer.transform));
        }
        let tex = gpu.draw(device, queue, project, 1, &Shown { index: k, layers });
        if let Some(rgba) = readback.push(device, queue, tex) {
            enc.video(&rgba).map_err(err)?;
        }
        // Each frame's own samples, so audio and video advance together and end together.
        let (a, b) = (sample(time), sample(rate.time_floor(k + 1)));
        mix.clear();
        mix.resize((b - a) as usize * 2, 0.0);
        // Closes clips no longer heard, as playback does, so a long timeline doesn't pile up decoders.
        audio.prime(project, time);
        audio.mix(project, a, &mut mix);
        enc.audio(&mix).map_err(err)?;
        ctx.set_progress((k + 1 - first) as f32 / (end - first) as f32);
    }
    if let Some(rgba) = readback.take(device) {
        enc.video(&rgba).map_err(err)?;
    }
    if ctx.is_cancelled() {
        return Err(Error::Cancelled);
    }
    enc.finish().map_err(err)?;
    if ctx.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(())
}

fn err(e: impl std::fmt::Display) -> String {
    format!("encode: {e}")
}

/// Re-probes the written file: it must open, have video and audio, and last the range to within
/// one frame.
fn validate(project: &Project, req: &Request) -> Result<ProbeInfo, Error> {
    let info = jvid_media::probe::probe(&req.path).map_err(|e| format!("output: {e}"))?;
    let rate = project.settings().frame_rate;
    let (first, end) = frames(project, req.range);
    let want = rate.time_floor(end) - rate.time_floor(first);
    let frame = rate.time_floor(1);
    if info.width.is_none() || info.audio_streams == 0 {
        return Err("output is missing its video or audio stream"
            .to_string()
            .into());
    }
    if (info.duration - want).0.abs() > frame.0 {
        let secs = |t: Time| t.0 as f64 / FLICKS_PER_SECOND as f64;
        return Err(format!(
            "output lasts {:.3}s, expected {:.3}s",
            secs(info.duration),
            secs(want)
        )
        .into());
    }
    Ok(info)
}

/// Reads one clip's audio synchronously, for export: unlike playback's source it always fills
/// all of `out` that the file has.
fn blocking(path: &Path) -> ClipAudio {
    let mut dec = AudioDecoder::open(path, 0)
        .map_err(|e| tracing::debug!("export audio of {}: {e}", path.display()))
        .ok();
    let (mut buf, mut at) = (Vec::<f32>::new(), i64::MAX);
    Box::new(move |start, out| {
        let Some(d) = &mut dec else { return };
        if start < at || start > at + buf.len() as i64 / 2 {
            if let Err(e) = d.seek(start) {
                return tracing::warn!("export audio seek: {e}");
            }
            buf.clear();
            at = start;
        }
        buf.drain(..(start - at) as usize * 2);
        at = start;
        while buf.len() < out.len() {
            match d.next_chunk() {
                Ok(Some(c)) => buf.extend(c.samples),
                Ok(None) => break,
                Err(e) => return tracing::warn!("export audio decode: {e}"),
            }
        }
        let n = out.len().min(buf.len());
        out[..n].copy_from_slice(&buf[..n]);
    })
}

#[cfg(test)]
mod tests;
