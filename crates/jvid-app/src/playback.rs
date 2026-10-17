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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui_wgpu::wgpu;
use jvid_audio::{Mixer, Output, SAMPLE_RATE, Strip};
use jvid_core::eval::{audio_in, video_at};
use jvid_core::model::{AssetId, ClipId, Project, ProjectSettings, TrackKind, Transform};
use jvid_core::time::{FLICKS_PER_SECOND, Rate, Time};
use jvid_jobs::{Handle, Pool, Priority};
use jvid_media::decode::{DecoderPool, Frame};
use jvid_render::composite::{self, Compositor};
use jvid_render::preview::{Preview, Yuv};
use jvid_render::solid;

use crate::transport::{AudioSource, Transport};

/// Timeline frames decoded ahead of the playhead, in the direction of play.
const AHEAD: i64 = 15;
/// Decode threads, separate from the app's job pool so thumbnails can't starve playback.
const DECODE_THREADS: usize = 4;
/// Audio clips starting this soon are opened and seeked before they're heard.
const AUDIO_AHEAD: Time = Time(FLICKS_PER_SECOND);
/// A failed decode is retried after this long, e.g. once a file being written or relinked is ready.
const RETRY: Duration = Duration::from_secs(1);
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
/// A finished decode and how long decoding took.
type Done = (Key, Duration, Result<Frame, String>);

/// What to draw for one timeline frame.
pub struct Shown {
    /// Timeline frame index.
    #[cfg_attr(not(test), allow(dead_code))]
    pub index: i64,
    /// Top layer first, with its evaluated transform.
    pub layers: Vec<(Pixels, Transform)>,
    /// False while a parked seek shows keyframe stand-ins; the exact frame follows.
    pub exact: bool,
}

/// What one layer shows.
#[derive(Clone)]
pub enum Pixels {
    Frame(Arc<Frame>),
    /// A solid color layer, straight RGBA.
    Solid([u8; 4]),
}

/// Decoded frames up to a byte budget; the least recently used go first.
pub struct FrameCache {
    budget: usize,
    bytes: usize,
    uses: u64,
    frames: HashMap<Key, (Arc<Frame>, u64)>,
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

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    fn contains(&self, k: &Key) -> bool {
        self.frames.contains_key(k)
    }

    /// Marks the frame used.
    fn get(&mut self, k: &Key) -> Option<Arc<Frame>> {
        self.uses += 1;
        let (f, used) = self.frames.get_mut(k)?;
        *used = self.uses;
        Some(f.clone())
    }

    /// Adds a frame, evicting the least recently used others while over budget.
    fn insert(&mut self, k: Key, f: Arc<Frame>) {
        self.uses += 1;
        self.bytes += size(&f);
        if let Some((old, _)) = self.frames.insert(k, (f, self.uses)) {
            self.bytes -= size(&old);
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
            self.bytes -= size(&old);
        }
    }

    /// Drops every frame of `asset`.
    fn forget(&mut self, asset: AssetId) {
        self.frames.retain(|k, _| k.0 != asset);
        self.bytes = self.frames.values().map(|(f, _)| size(f)).sum();
    }
}

fn size(f: &Frame) -> usize {
    f.planes.iter().map(Vec::len).sum()
}

pub(crate) fn sample(t: Time) -> i64 {
    t.0.div_euclid(FLICKS_PER_SAMPLE)
}

/// Per-clip audio sources and the track mixer.
pub(crate) struct Audio {
    open: OpenAudio,
    clips: HashMap<ClipId, ClipAudio>,
    mixer: Mixer,
    /// One buffer per audio track, reused across blocks.
    inputs: Vec<Vec<f32>>,
}

impl Audio {
    pub(crate) fn new(open: OpenAudio) -> Self {
        Self {
            open,
            clips: HashMap::new(),
            mixer: Mixer::new(0),
            inputs: Vec::new(),
        }
    }

    /// Opens and seeks clips heard within [`AUDIO_AHEAD`] of `pos`, so they start on time, and
    /// closes the rest.
    pub(crate) fn prime(&mut self, project: &Project, pos: Time) {
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
    pub(crate) fn mix(&mut self, project: &Project, start: i64, out: &mut [f32]) {
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
    /// Decodes the keyframe before a source time, for stand-ins; `None` shows none.
    pub quick: Option<DecodeFn>,
    /// One thread, so a stand-in never waits behind the long exact decodes.
    quick_jobs: Pool,
    /// Keyframe stand-ins for the parked frame's layers, `None` while decoding.
    stand_ins: HashMap<Key, Option<Arc<Frame>>>,
    quick_tx: Sender<(Key, Result<Frame, String>)>,
    quick_done: Receiver<(Key, Result<Frame, String>)>,
    /// Smoothed time a decode takes.
    latency: Duration,
    /// Frames that failed to decode and when, so they're left out until [`RETRY`].
    failed: HashMap<Key, Instant>,
    /// Asset paths cached frames were decoded from, to notice a relink.
    paths: HashMap<AssetId, PathBuf>,
    /// Forward play loops the sequence's in/out range (all of it where a mark is unset).
    pub looping: bool,
    done_tx: Sender<Done>,
    done: Receiver<Done>,
    /// The frame index last returned by `tick`, and whether it was exact.
    shown: Option<(i64, bool)>,
    audio: Audio,
}

impl Playback {
    pub fn new(decode: DecodeFn, open_audio: OpenAudio, budget: usize) -> Self {
        let (done_tx, done) = channel();
        let (quick_tx, quick_done) = channel();
        Self {
            transport: Transport::new(Time(0), Rate::new(30, 1)),
            cache: FrameCache::new(budget),
            jobs: Pool::new(DECODE_THREADS),
            decode,
            pending: HashMap::new(),
            quick: None,
            quick_jobs: Pool::new(1),
            stand_ins: HashMap::new(),
            quick_tx,
            quick_done,
            latency: Duration::ZERO,
            failed: HashMap::new(),
            paths: HashMap::new(),
            looping: false,
            done_tx,
            done,
            shown: None,
            audio: Audio::new(open_audio),
        }
    }

    /// Decodes project media from files.
    pub fn from_files(budget: usize) -> Self {
        let pool = Arc::new(DecoderPool::default());
        // Its own pool, so the stand-in thread always finds its file open: opening a long MP4
        // reads an index of every frame, which alone can blow the seek budget.
        let quick = DecoderPool::default();
        let mut pb = Self::new(
            Arc::new(move |p, t| pool.frame_at(p, t).map_err(|e| e.to_string())),
            Box::new(|p| {
                let mut src = AudioSource::open(p, true);
                Box::new(move |start, out| src.fill(start, out))
            }),
            budget,
        );
        pb.quick = Some(Arc::new(move |p, t| {
            quick.keyframe_at(p, t).map_err(|e| e.to_string())
        }));
        pb
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

    /// [`Playback::tick`] while stopped and before an audio output is open: decodes and returns
    /// the frame at the parked playhead.
    pub fn park(&mut self, project: &Project) -> Option<Shown> {
        self.sync(project);
        self.decode_ahead(project, 0);
        self.show(project, 0)
    }

    /// Follows edits to the sequence length, in/out marks and frame rate, and drops what was
    /// decoded or opened from an asset's old path after a relink.
    fn sync(&mut self, project: &Project) {
        let seq = project.sequence();
        let t = &mut self.transport;
        t.duration = seq.duration();
        t.rate = project.settings().frame_rate;
        t.loop_range = self.looping.then(|| {
            let out = seq.out_point().unwrap_or(t.duration);
            (seq.in_point().unwrap_or(Time(0)), out)
        });
        for a in project.assets() {
            if self.paths.get(&a.id).is_some_and(|p| *p != a.path) {
                self.cache.forget(a.id);
                self.failed.retain(|k, _| k.0 != a.id);
                // Clips are reopened on the new path as they're next heard.
                self.audio.clips.clear();
            }
            self.paths.insert(a.id, a.path.clone());
        }
    }

    /// Takes finished decodes, then queues every layer of the next [`AHEAD`] frames that isn't
    /// cached or queued, nearest first, and cancels queued frames no longer ahead.
    fn decode_ahead(&mut self, project: &Project, clock: u64) {
        while let Ok((k, took, r)) = self.done.try_recv() {
            self.pending.remove(&k);
            self.latency = (self.latency * 3 + took) / 4;
            match r {
                Ok(f) => self.cache.insert(k, Arc::new(f)),
                Err(e) => {
                    tracing::warn!("playback decode: {e}");
                    self.failed.insert(k, Instant::now());
                }
            }
        }
        self.failed.retain(|_, at| at.elapsed() < RETRY);
        self.stand_in(project, clock);
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
        // Forward play past the loop's end continues from its start.
        let wrap = t.loop_range.filter(|_| step > 0).map(|(start, end)| {
            let (a, b) = (
                t.rate.index_floor(start),
                t.rate.index_floor(end - Time(1)) + 1,
            );
            move |k: i64| {
                if k >= b && b > a {
                    a + (k - b) % (b - a)
                } else {
                    k
                }
            }
        });
        let first = t.frame(clock) + lead * step;
        let mut want = HashSet::new();
        for k in (0..AHEAD).map(|i| first + i * step) {
            let k = wrap.map_or(k, |w| w(k));
            let time = t.rate.time_floor(k);
            if k < 0 || time >= t.duration {
                break;
            }
            for layer in video_at(project.sequence(), time) {
                let key = (layer.asset, layer.source_time);
                want.insert(key);
                let Some(asset) = project.asset(layer.asset).filter(|a| a.solid.is_none()) else {
                    continue;
                };
                if self.cache.contains(&key)
                    || self.pending.contains_key(&key)
                    || self.failed.contains_key(&key)
                {
                    continue;
                }
                let (decode, tx, path) = (
                    self.decode.clone(),
                    self.done_tx.clone(),
                    asset.path.clone(),
                );
                let job = format!("playback:{}:{}", key.0.0, key.1.0);
                let handle = self.jobs.submit(job, Priority::Interactive, move |ctx| {
                    if !ctx.is_cancelled() {
                        let start = Instant::now();
                        let r = decode(&path, key.1);
                        let _ = tx.send((key, start.elapsed(), r));
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

    /// Parked on a frame whose layers aren't decoded, queues each one's keyframe as a stand-in,
    /// since an exact frame deep in a long GOP can take a second. Playing, there are none.
    fn stand_in(&mut self, project: &Project, clock: u64) {
        while let Ok((k, r)) = self.quick_done.try_recv() {
            match r {
                Ok(f) => self.stand_ins.insert(k, Some(Arc::new(f))),
                Err(_) => self.stand_ins.remove(&k),
            };
        }
        let t = &self.transport;
        let Some(quick) = self.quick.as_ref().filter(|_| t.speed() == 0) else {
            self.stand_ins.clear();
            return;
        };
        let time = t.rate.time_floor(t.frame(clock));
        let mut want = HashSet::new();
        for layer in video_at(project.sequence(), time) {
            let key = (layer.asset, layer.source_time);
            let Some(asset) = project.asset(layer.asset).filter(|a| a.solid.is_none()) else {
                continue;
            };
            if self.cache.contains(&key) || self.failed.contains_key(&key) {
                continue;
            }
            want.insert(key);
            if self.stand_ins.contains_key(&key) {
                continue;
            }
            self.stand_ins.insert(key, None);
            let (quick, tx, path) = (quick.clone(), self.quick_tx.clone(), asset.path.clone());
            let job = format!("keyframe:{}:{}", key.0.0, key.1.0);
            self.quick_jobs
                .submit(job, Priority::Interactive, move |_| {
                    let _ = tx.send((key, quick(&path, key.1)));
                });
        }
        self.stand_ins.retain(|k, _| want.contains(k));
    }

    /// The frame at the clock, once all its layers are decoded (or stood in for) and if it isn't
    /// the one shown.
    fn show(&mut self, project: &Project, clock: u64) -> Option<Shown> {
        let index = self.transport.frame(clock);
        if self.shown == Some((index, true)) {
            return None;
        }
        let mut exact = true;
        let time = self.transport.rate.time_floor(index);
        let mut layers = Vec::new();
        for layer in video_at(project.sequence(), time) {
            let Some(asset) = project.asset(layer.asset) else {
                continue;
            };
            if let Some(c) = layer.color.or(asset.solid) {
                layers.push((Pixels::Solid(solid::rgba8(c)), layer.transform));
                continue;
            }
            // Not decoded yet: keep what's on screen. A failed decode leaves the layer out.
            let key = (layer.asset, layer.source_time);
            if !self.failed.contains_key(&key) {
                let f = match self.cache.get(&key) {
                    Some(f) => f,
                    None => {
                        exact = false;
                        self.stand_ins.get(&key)?.clone()?
                    }
                };
                layers.push((Pixels::Frame(f), layer.transform));
            }
        }
        if self.shown == Some((index, exact)) {
            return None;
        }
        self.shown = Some((index, exact));
        Some(Shown {
            index,
            layers,
            exact,
        })
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
        s: &ProjectSettings,
        scale: u32,
        shown: &Shown,
    ) -> &wgpu::Texture {
        while self.layers.len() < shown.layers.len() {
            self.layers.push(Preview::new(device, composite::FORMAT));
        }
        // shortcut: a solid texture per layer per frame; cache by color if playback profiles hot.
        let mut solids = Vec::new();
        for ((px, _), preview) in shown.layers.iter().zip(&mut self.layers) {
            let f = match px {
                Pixels::Frame(f) => f,
                Pixels::Solid(c) => {
                    solids.push(solid::solid(device, queue, (s.width, s.height), *c));
                    continue;
                }
            };
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
            .scan(solids.iter(), |solids, ((px, t), p)| {
                let tex = match px {
                    Pixels::Frame(_) => p.image().expect("uploaded above"),
                    Pixels::Solid(_) => solids.next().expect("made above"),
                };
                Some((tex, *t))
            })
            .collect();
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
pub(crate) mod tests;
