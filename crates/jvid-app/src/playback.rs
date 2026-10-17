//! Timeline playback: the sequence playhead on [`Transport`], video decoded ahead into a
//! memory-budgeted [`FrameCache`] and composited per frame, audio mixed per track into the output.
//!
//! Video follows the audio master clock. Each UI frame shows the frame the clock is in once every
//! layer of it is decoded, and keeps the last one shown until then. A frame that arrives after its
//! time is never shown, so slow decoding drops frames instead of drifting.
//!
//! Use: call [`Playback::tick`] every UI frame, and when it returns a [`Shown`], draw it with
//! [`Gpu::draw`]. Drive play, pause, J/K/L, seek and loop through [`Playback::transport`]; after
//! an edit that changes the picture under a parked playhead, call [`Playback::refresh`].

// The timeline panel (#33) is the first caller.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui_wgpu::wgpu;
use jvid_audio::{Mixer, Output, SAMPLE_RATE, Strip};
use jvid_core::eval::{audio_in, video_at};
use jvid_core::model::{AssetId, ClipId, Project, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND, Rate, Time};
use jvid_jobs::{Handle, Pool, Priority};
use jvid_media::decode::{DecoderPool, Frame};
use jvid_render::composite::{self, Compositor, opacity};
use jvid_render::preview::{Preview, Yuv};

use crate::transport::{AudioSource, Transport};

/// Timeline frames decoded ahead of the playhead, in the direction of play.
const AHEAD: i64 = 15;
/// Decode threads, separate from the app's job pool so thumbnails can't starve playback.
const DECODE_THREADS: usize = 4;
/// Audio clips starting this soon are opened and seeked before they're heard.
const AUDIO_AHEAD: Time = Time(FLICKS_PER_SECOND);
/// Default frame cache budget: about 170 1080p frames.
pub const DEFAULT_BUDGET: usize = 512 << 20;
const FLICKS_PER_SAMPLE: i64 = FLICKS_PER_SECOND / SAMPLE_RATE as i64;

/// Decodes the frame of `path` shown at source time `t`; runs on a decode thread.
pub type DecodeFn = Arc<dyn Fn(&Path, Time) -> Result<Frame, String> + Send + Sync>;
/// One clip's audio: fills interleaved stereo from source sample frame `start` as far as it's
/// decoded, like [`AudioSource::fill`]. An empty `out` just seeks there.
pub type ClipAudio = Box<dyn FnMut(i64, &mut [f32])>;
/// Opens a [`ClipAudio`] on a file.
pub type OpenAudio = Box<dyn FnMut(&Path) -> ClipAudio>;

/// A frame of an asset at a source time.
type Key = (AssetId, Time);
/// A finished decode and when it was queued.
type Done = (Key, Instant, Result<Frame, String>);

/// What to draw for one timeline frame.
pub struct Shown {
    /// Timeline frame index.
    pub index: i64,
    /// Top layer first, with its opacity.
    pub layers: Vec<(Arc<Frame>, f32)>,
}

/// Decoded frames up to a byte budget; the least recently used go first.
pub struct FrameCache {
    budget: usize,
    bytes: usize,
    uses: u64,
    /// `None` for a frame that failed to decode, so it isn't retried every UI frame.
    frames: HashMap<Key, (Option<Arc<Frame>>, u64)>,
}

impl FrameCache {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            bytes: 0,
            uses: 0,
            frames: HashMap::new(),
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    fn contains(&self, k: &Key) -> bool {
        self.frames.contains_key(k)
    }

    /// Marks the frame used.
    fn get(&mut self, k: &Key) -> Option<Option<Arc<Frame>>> {
        self.uses += 1;
        let (f, used) = self.frames.get_mut(k)?;
        *used = self.uses;
        Some(f.clone())
    }

    /// Adds a frame, evicting the least recently used others while over budget.
    fn insert(&mut self, k: Key, f: Option<Arc<Frame>>) {
        self.uses += 1;
        self.bytes += f.as_deref().map_or(0, size);
        if let Some((old, _)) = self.frames.insert(k, (f, self.uses)) {
            self.bytes -= old.as_deref().map_or(0, size);
        }
        while self.bytes > self.budget {
            let Some((&oldest, _)) = self
                .frames
                .iter()
                .filter(|(key, _)| **key != k)
                .min_by_key(|(_, (_, used))| *used)
            else {
                break;
            };
            let (old, _) = self.frames.remove(&oldest).unwrap();
            self.bytes -= old.as_deref().map_or(0, size);
        }
    }
}

fn size(f: &Frame) -> usize {
    f.planes.iter().map(Vec::len).sum()
}

fn sample(t: Time) -> i64 {
    t.0.div_euclid(FLICKS_PER_SAMPLE)
}

/// Per-clip audio sources and the track mixer.
struct Audio {
    open: OpenAudio,
    clips: HashMap<ClipId, ClipAudio>,
    mixer: Mixer,
    /// One buffer per audio track, reused across blocks.
    inputs: Vec<Vec<f32>>,
}

impl Audio {
    /// Opens and seeks clips heard within [`AUDIO_AHEAD`] of `pos`, so they start on time, and
    /// closes the rest.
    fn prime(&mut self, project: &Project, pos: Time) {
        let segs = audio_in(project.sequence(), pos, pos + AUDIO_AHEAD);
        self.clips.retain(|c, _| segs.iter().any(|s| s.clip == *c));
        for s in segs {
            if let Some(a) = project.asset(s.asset)
                && !self.clips.contains_key(&s.clip)
            {
                let mut src = (self.open)(&a.path);
                src(sample(s.source_start), &mut []);
                self.clips.insert(s.clip, src);
            }
        }
    }

    /// Fills `out` with the mix from timeline sample frame `start`.
    fn mix(&mut self, project: &Project, start: i64, out: &mut [f32]) {
        let seq = project.sequence();
        let tracks: Vec<_> = seq
            .tracks()
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .collect();
        if self.inputs.len() != tracks.len() {
            self.mixer = Mixer::new(tracks.len());
            self.inputs = vec![Vec::new(); tracks.len()];
        }
        let strips: Vec<_> = tracks
            .iter()
            .map(|t| Strip {
                volume: t.volume,
                pan: t.pan,
                muted: t.muted,
                solo: t.solo,
            })
            .collect();
        self.mixer.set_strips(&strips);
        for b in &mut self.inputs {
            b.clear();
            b.resize(out.len(), 0.0);
        }
        let frames = out.len() as i64 / 2;
        let span = |t: Time| (sample(t) - start).clamp(0, frames) as usize * 2;
        let end = Time((start + frames) * FLICKS_PER_SAMPLE);
        for s in audio_in(seq, Time(start * FLICKS_PER_SAMPLE), end) {
            let (Some(i), Some(asset)) = (
                tracks.iter().position(|t| t.id == s.track),
                project.asset(s.asset),
            ) else {
                continue;
            };
            let src = self
                .clips
                .entry(s.clip)
                .or_insert_with(|| (self.open)(&asset.path));
            src(
                sample(s.source_start),
                &mut self.inputs[i][span(s.start)..span(s.end)],
            );
        }
        let inputs: Vec<&[f32]> = self.inputs.iter().map(Vec::as_slice).collect();
        self.mixer.mix(&inputs, out);
    }
}

/// The timeline playback engine. See the module docs.
pub struct Playback {
    pub transport: Transport,
    pub cache: FrameCache,
    jobs: Pool,
    decode: DecodeFn,
    pending: HashMap<Key, Handle>,
    /// Smoothed time from queueing a decode to having the frame.
    latency: Duration,
    done_tx: Sender<Done>,
    done: Receiver<Done>,
    /// The frame index last returned by `tick`.
    shown: Option<i64>,
    audio: Audio,
}

impl Playback {
    pub fn new(decode: DecodeFn, open_audio: OpenAudio, budget: usize) -> Self {
        let (done_tx, done) = channel();
        Self {
            transport: Transport::new(Time(0), Rate::new(30, 1)),
            cache: FrameCache::new(budget),
            jobs: Pool::new(DECODE_THREADS),
            decode,
            pending: HashMap::new(),
            latency: Duration::ZERO,
            done_tx,
            done,
            shown: None,
            audio: Audio {
                open: open_audio,
                clips: HashMap::new(),
                mixer: Mixer::new(0),
                inputs: Vec::new(),
            },
        }
    }

    /// Decodes project media from files.
    pub fn from_files(budget: usize) -> Self {
        let pool = Arc::new(DecoderPool::default());
        Self::new(
            Arc::new(move |p, t| pool.frame_at(p, t).map_err(|e| e.to_string())),
            Box::new(|p| {
                let mut src = AudioSource::open(p, true);
                Box::new(move |start, out| src.fill(start, out))
            }),
            budget,
        )
    }

    /// Makes the next `tick` return the frame even if the playhead hasn't moved.
    pub fn refresh(&mut self) {
        self.shown = None;
    }

    /// Call every UI frame, after `out.poll()`: queues audio, decodes ahead, and returns the frame
    /// to draw when it changed.
    pub fn tick(&mut self, project: &Project, out: &mut Output) -> Option<Shown> {
        self.sync(project);
        let audio = &mut self.audio;
        self.transport
            .tick(out, &mut |start, buf| audio.mix(project, start, buf));
        let clock = out.clock();
        self.audio.prime(project, self.transport.position(clock));
        self.decode_ahead(project, clock);
        self.show(project, clock)
    }

    /// Follows edits to the sequence length and the project frame rate.
    fn sync(&mut self, project: &Project) {
        self.transport.duration = project.sequence().duration();
        self.transport.rate = project.settings().frame_rate;
    }

    /// Takes finished decodes, then queues every layer of the next [`AHEAD`] frames that isn't
    /// cached or queued, nearest first, and cancels queued frames no longer ahead.
    fn decode_ahead(&mut self, project: &Project, clock: u64) {
        while let Ok((k, queued, r)) = self.done.try_recv() {
            self.pending.remove(&k);
            self.latency = (self.latency * 3 + queued.elapsed()) / 4;
            let r = r.map_err(|e| tracing::warn!("playback decode: {e}"));
            self.cache.insert(k, r.ok().map(Arc::new));
        }
        let t = &self.transport;
        // Parked: just ahead, so pressing play starts on decoded frames.
        let step = match t.speed() {
            0 => 1,
            s => s as i64,
        };
        // Playing, start a decode's latency ahead: a frame the clock passes before it's ready is
        // wasted work, and with a slow decoder would starve every frame after it.
        let lead = match t.speed() {
            0 => 0,
            _ => t.rate.index_floor(Time(
                (self.latency.as_secs_f64() * FLICKS_PER_SECOND as f64) as i64,
            )),
        };
        let first = t.frame(clock) + lead * step;
        let mut want = HashSet::new();
        for k in (0..AHEAD).map(|i| first + i * step) {
            let time = t.rate.time_floor(k);
            if k < 0 || time >= t.duration {
                break;
            }
            for layer in video_at(project.sequence(), time) {
                let key = (layer.asset, layer.source_time);
                want.insert(key);
                let Some(asset) = project.asset(layer.asset) else {
                    continue;
                };
                if self.cache.contains(&key) || self.pending.contains_key(&key) {
                    continue;
                }
                let (decode, tx, path) = (
                    self.decode.clone(),
                    self.done_tx.clone(),
                    asset.path.clone(),
                );
                let job = format!("playback:{}:{}", key.0.0, key.1.0);
                let queued = Instant::now();
                let handle = self.jobs.submit(job, Priority::Interactive, move |ctx| {
                    if !ctx.is_cancelled() {
                        let _ = tx.send((key, queued, decode(&path, key.1)));
                    }
                });
                self.pending.insert(key, handle);
            }
        }
        self.pending.retain(|k, h| {
            let keep = want.contains(k);
            if !keep {
                h.cancel();
            }
            keep
        });
    }

    /// The frame at the clock, once all its layers are decoded and if it isn't the one shown.
    fn show(&mut self, project: &Project, clock: u64) -> Option<Shown> {
        let index = self.transport.frame(clock);
        if self.shown == Some(index) {
            return None;
        }
        let time = self.transport.rate.time_floor(index);
        let mut layers = Vec::new();
        for layer in video_at(project.sequence(), time) {
            if project.asset(layer.asset).is_none() {
                continue;
            }
            // Not decoded yet: keep what's on screen. A failed decode leaves the layer out.
            if let Some(f) = self.cache.get(&(layer.asset, layer.source_time))? {
                layers.push((f, opacity(&layer)));
            }
        }
        self.shown = Some(index);
        Some(Shown { index, layers })
    }
}

/// The GPU side: converts a [`Shown`] frame's layers to RGB and composites them.
pub struct Gpu {
    compositor: Compositor,
    layers: Vec<Preview>,
}

impl Gpu {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            compositor: Compositor::new(device),
            layers: Vec::new(),
        }
    }

    /// The composited frame at 1/`scale` of the project size (the preview scale; 1 for full).
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        project: &Project,
        scale: u32,
        shown: &Shown,
    ) -> &wgpu::Texture {
        while self.layers.len() < shown.layers.len() {
            self.layers.push(Preview::new(device, composite::FORMAT));
        }
        for ((f, _), preview) in shown.layers.iter().zip(&mut self.layers) {
            let yuv = Yuv {
                width: f.width,
                height: f.height,
                planes: [&f.planes[0], &f.planes[1], &f.planes[2]],
                bt709: f.bt709,
                full_range: f.full_range,
            };
            preview.upload(device, queue, &yuv, scale);
        }
        let layers: Vec<_> = shown
            .layers
            .iter()
            .zip(&self.layers)
            .map(|((_, o), p)| (p.image().expect("uploaded above"), *o))
            .collect();
        let s = project.settings();
        self.compositor.render(
            device,
            queue,
            (s.width, s.height),
            scale,
            s.background,
            &layers,
        )
    }
}

#[cfg(test)]
mod tests;
