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

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui_wgpu::wgpu;
use jvid_audio::{Fx, Mixer, Output, SAMPLE_RATE, Strip, automate};
use jvid_core::eval::{AudioSegment, Layer, audio_in, frame_at, gain_at};
use jvid_core::model::{
    AssetId, ClipId, MediaAsset, Project, ProjectSettings, Sequence, SequenceId, TrackKind,
    Transform,
};
use jvid_core::time::{FLICKS_PER_SECOND, Rate, Time};
use jvid_jobs::{Handle, Pool, Priority};
use jvid_media::decode::{DecoderPool, Frame};
use jvid_media::proxy::Proxies;
use jvid_render::composite::{self, Compositor};
use jvid_render::preview::{Preview, Yuv};
use jvid_render::{solid, text};

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
/// Sample frames heard per scrub step: a bit over a UI frame, so a steady drag sounds continuous.
const SCRUB_SAMPLES: usize = SAMPLE_RATE as usize / 15;
const FLICKS_PER_SAMPLE: i64 = FLICKS_PER_SECOND / SAMPLE_RATE as i64;

/// Decodes the frame of `path` shown at source time `t`; runs on a decode thread.
pub type DecodeFn = Arc<dyn Fn(&Path, Time) -> Result<Frame, String> + Send + Sync>;
/// One clip's audio: fills interleaved stereo from source sample frame `start` as far as it's
/// decoded, like [`AudioSource::fill`]. An empty `out` just seeks there.
pub type ClipAudio = Box<dyn FnMut(i64, &mut [f32])>;
/// Opens a [`ClipAudio`] on the nth audio stream of a file.
pub type OpenAudio = Box<dyn FnMut(&Path, usize) -> ClipAudio>;

/// A frame of an asset at a source time.
type Key = (AssetId, Time);
/// A finished decode and how long decoding took.
type Done = (Key, Duration, Result<Frame, String>);

/// What to draw for one timeline frame.
pub struct Shown {
    /// Timeline frame index.
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
    /// A text layer rasterized at project size, straight RGBA.
    Text(Arc<Vec<u8>>),
    /// A sequence clip: its sequence's layers, top first, composited over transparent.
    Nested(Vec<(Pixels, Transform)>),
}

/// The pixels of a solid or text asset's `layer` on a `size` frame; `None` for a file.
pub fn generated(asset: &MediaAsset, layer: &Layer, size: (u32, u32)) -> Option<Pixels> {
    if let Some(t) = &asset.text {
        let t = t.with(&layer.text);
        return Some(Pixels::Text(text::rasterize(&t, layer.color, size)));
    }
    let c = layer.color.or(asset.solid)?;
    Some(Pixels::Solid(solid::rgba8(c)))
}

/// What each of `layers` (from [`frame_at`]) shows, top first: generated pixels, a nested
/// sequence's layers, or `frame`'s pixels for a file. `Ok(None)` from `frame` leaves the layer
/// out; an error gives up on the whole frame.
pub fn resolve<E, F>(
    project: &Project,
    layers: Vec<Layer>,
    frame: &mut F,
) -> Result<Vec<(Pixels, Transform)>, E>
where
    F: FnMut(&MediaAsset, &Layer) -> Result<Option<Pixels>, E>,
{
    let s = project.settings();
    let mut out = Vec::new();
    for mut layer in layers {
        let Some(asset) = project.asset(layer.asset) else {
            continue;
        };
        let px = if asset.sequence.is_some() {
            let nested = std::mem::take(&mut layer.nested);
            Some(Pixels::Nested(resolve(project, nested, frame)?))
        } else if let Some(px) = generated(asset, &layer, (s.width, s.height)) {
            Some(px)
        } else {
            frame(asset, &layer)?
        };
        out.extend(px.map(|px| (px, layer.transform)));
    }
    Ok(out)
}

/// The layers read from files: `layers` with each nested sequence's in its place, however deep.
fn leaves(layers: Vec<Layer>) -> Vec<Layer> {
    let mut out = Vec::new();
    for mut l in layers {
        match std::mem::take(&mut l.nested) {
            n if n.is_empty() => out.push(l),
            n => out.extend(leaves(n)),
        }
    }
    out
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
    /// Shared with the nested mixers.
    open: Rc<RefCell<OpenAudio>>,
    /// One source per stream the clip plays; several are summed.
    clips: HashMap<ClipId, Vec<ClipAudio>>,
    /// Effect state of the clips in `clips`.
    fx: HashMap<ClipId, Fx>,
    /// What mixes each sequence clip's sequence.
    nested: HashMap<ClipId, Audio>,
    scratch: Vec<f32>,
    /// One segment's samples, summed into its track since a transition overlaps two.
    seg: Vec<f32>,
    /// Source samples for a clip not at 100% speed, before resampling.
    stretch: Vec<f32>,
    mixer: Mixer,
    /// One buffer per audio track, reused across blocks.
    inputs: Vec<Vec<f32>>,
}

impl Audio {
    pub(crate) fn new(open: OpenAudio) -> Self {
        Self::sharing(Rc::new(RefCell::new(open)))
    }

    fn sharing(open: Rc<RefCell<OpenAudio>>) -> Self {
        Self {
            open,
            clips: HashMap::new(),
            fx: HashMap::new(),
            nested: HashMap::new(),
            scratch: Vec::new(),
            seg: Vec::new(),
            stretch: Vec::new(),
            mixer: Mixer::new(0),
            inputs: Vec::new(),
        }
    }

    /// Closes every clip's audio, to reopen as each is next heard.
    fn reset(&mut self) {
        self.clips.clear();
        self.fx.clear();
        self.nested.clear();
    }

    /// Opens and seeks clips heard within [`AUDIO_AHEAD`] of `pos`, so they start on time, and
    /// closes the rest.
    pub(crate) fn prime(&mut self, project: &Project, pos: Time) {
        self.prime_in(project, project.sequence(), pos);
    }

    fn prime_in(&mut self, project: &Project, seq: &Sequence, pos: Time) {
        let segs = audio_in(seq, pos, pos + AUDIO_AHEAD);
        self.clips.retain(|c, _| segs.iter().any(|s| s.clip == *c));
        self.fx.retain(|c, _| segs.iter().any(|s| s.clip == *c));
        self.nested.retain(|c, _| segs.iter().any(|s| s.clip == *c));
        for s in segs {
            if let Some(n) = project.nested(s.asset) {
                let open = &self.open;
                let child = self.nested.entry(s.clip);
                let child = child.or_insert_with(|| Audio::sharing(open.clone()));
                child.prime_in(project, n, s.source_start);
            } else if let Some(a) = project.asset(s.asset)
                && !self.clips.contains_key(&s.clip)
            {
                let mut srcs = open_streams(&mut self.open.borrow_mut(), a, s.stream);
                srcs.iter_mut()
                    .for_each(|src| src(sample(s.source_start), &mut []));
                self.clips.insert(s.clip, srcs);
            }
        }
    }

    /// Fills `out` with the mix from timeline sample frame `start`.
    pub(crate) fn mix(&mut self, project: &Project, start: i64, out: &mut [f32]) {
        self.mix_in(project, project.sequence(), false, start, out);
    }

    /// [`Audio::mix`] of `seq`, which is `nested` in another.
    fn mix_in(
        &mut self,
        project: &Project,
        seq: &Sequence,
        nested: bool,
        start: i64,
        out: &mut [f32],
    ) {
        let tracks: Vec<_> = seq
            .tracks()
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .collect();
        // A nest's tracks pan relative to the outer track's centre, so the pan law applies once
        // and a nest at default settings sounds as its clips did before nesting.
        let unity = if nested {
            std::f32::consts::SQRT_2
        } else {
            1.0
        };
        let strips: Vec<_> = tracks
            .iter()
            .map(|t| Strip {
                volume: t.volume * unity,
                pan: t.pan,
                muted: t.muted,
                solo: t.solo,
                fx: t.fx,
            })
            .collect();
        if self.inputs.len() != tracks.len() {
            self.mixer = Mixer::with_strips(&strips);
            self.inputs = vec![Vec::new(); tracks.len()];
        }
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
            let range = span(s.start)..span(s.end);
            let out = &mut self.seg;
            out.clear();
            out.resize(range.len(), 0.0);
            if let Some(n) = project.nested(asset.id) {
                let open = &self.open;
                let child = self.nested.entry(s.clip);
                let child = child.or_insert_with(|| Audio::sharing(open.clone()));
                let mut src = |from, buf: &mut [f32]| child.mix_in(project, n, true, from, buf);
                read(&mut src, &s, out, &mut self.stretch);
            } else if let [src] = self
                .clips
                .entry(s.clip)
                .or_insert_with(|| open_streams(&mut self.open.borrow_mut(), asset, s.stream))
                .as_mut_slice()
            {
                read(src, &s, out, &mut self.stretch);
            } else {
                let srcs = self.clips.get_mut(&s.clip).expect("opened above");
                for src in srcs {
                    self.scratch.clear();
                    self.scratch.resize(out.len(), 0.0);
                    read(src, &s, &mut self.scratch, &mut self.stretch);
                    out.iter_mut().zip(&self.scratch).for_each(|(o, x)| *o += x);
                }
            }
            if let Some((t, c)) = seq.clip(s.clip)
                && let Some(track) = seq.track(t)
            {
                // Effects before gain, so fades and automation don't change how it compresses.
                if !c.transform.audio_fx.is_default() {
                    let fx = self.fx.entry(s.clip).or_default();
                    fx.process(&c.transform.audio_fx, out);
                }
                let first = sample(s.start);
                let at = |i: usize| {
                    let t = Time((first + i as i64) * FLICKS_PER_SAMPLE);
                    let (v, p) = c.gain_at(t);
                    ((v * gain_at(track, c, t)) as f32, p as f32)
                };
                if c.gain_animated() || s.crossfade {
                    automate(out, at);
                } else if c.transform.volume != 1.0 || c.transform.pan != 0.0 {
                    let g = at(0);
                    automate(out, |_| g);
                }
            }
            let mixed = &mut self.inputs[i][range];
            mixed.iter_mut().zip(&*out).for_each(|(m, x)| *m += x);
        }
        self.mixer.set_strips(&strips);
        let inputs: Vec<&[f32]> = self.inputs.iter().map(Vec::as_slice).collect();
        self.mixer.mix(&inputs, out);
    }
}

/// Fills stereo `out` with segment `s` from `src`, resampled linearly when not at 100%
/// speed, so pitch follows speed, and backwards when reversed.
fn read(
    src: &mut dyn FnMut(i64, &mut [f32]),
    s: &AudioSegment,
    out: &mut [f32],
    tmp: &mut Vec<f32>,
) {
    let from = sample(s.source_start);
    if s.speed == 1.0 && !s.reverse {
        return src(from, out);
    }
    let frames = out.len() / 2;
    let n = (frames as f64 * s.speed).ceil() as usize;
    // One frame spare to interpolate towards.
    tmp.clear();
    tmp.resize((n + 1) * 2, 0.0);
    src(if s.reverse { from - n as i64 } else { from }, tmp);
    for i in 0..frames {
        let x = i as f64 * s.speed;
        let x = if s.reverse { n as f64 - x } else { x };
        let (j, f) = (x.floor() as usize, (x.fract()) as f32);
        for ch in 0..2 {
            let (a, b) = (tmp[j * 2 + ch], tmp[(j + 1).min(n) * 2 + ch]);
            out[i * 2 + ch] = a + (b - a) * f;
        }
    }
}

/// Sources for `stream` of `asset`, or for every audio stream it has when `None`.
fn open_streams(open: &mut OpenAudio, asset: &MediaAsset, stream: Option<u32>) -> Vec<ClipAudio> {
    let all = asset.probe.as_ref().map_or(1, |p| p.audio_streams);
    let streams = stream.map_or(0..all, |n| n..n + 1);
    streams.map(|n| open(&asset.path, n as usize)).collect()
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
    /// Parked seeks while dragging play a short snippet (Shift+S toggles).
    pub scrub_audio: bool,
    done_tx: Sender<Done>,
    done: Receiver<Done>,
    /// The frame index last returned by `tick`, and whether it was exact.
    shown: Option<(i64, bool)>,
    audio: Audio,
    /// The sequence `audio` was opened for; clip ids are per sequence.
    sequence: SequenceId,
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
            scrub_audio: true,
            done_tx,
            done,
            shown: None,
            audio: Audio::new(open_audio),
            sequence: SequenceId::default(),
        }
    }

    /// Decodes project media from files.
    pub fn from_files(budget: usize) -> Self {
        Self::from_proxies(budget, Arc::new(Proxies::new(PathBuf::new())))
    }

    /// Decodes project media from files, or from their proxies while `proxies` is on.
    pub fn from_proxies(budget: usize, proxies: Arc<Proxies>) -> Self {
        let pool = Arc::new(DecoderPool::default());
        let quick_proxies = proxies.clone();
        // Its own pool, so the stand-in thread always finds its file open: opening a long MP4
        // reads an index of every frame, which alone can blow the seek budget.
        let quick = DecoderPool::default();
        let mut pb = Self::new(
            Arc::new(move |p, t| {
                pool.frame_at(&proxies.resolve(p), t)
                    .map_err(|e| e.to_string())
            }),
            Box::new(|p, nth| {
                let mut src = AudioSource::stream(p, nth);
                Box::new(move |start, out| src.fill(start, out))
            }),
            budget,
        );
        pb.quick = Some(Arc::new(move |p, t| {
            quick
                .keyframe_at(&quick_proxies.resolve(p), t)
                .map_err(|e| e.to_string())
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

    /// Queues a short snippet of the mix from the parked playhead, for audio scrubbing. The
    /// first snippet after a jump may be silent while the clips' audio seeks.
    pub fn scrub(&mut self, project: &Project, out: &mut Output) {
        let pos = self.transport.position(0);
        self.audio.prime(project, pos);
        let mut buf = vec![0.0; SCRUB_SAMPLES * 2];
        self.audio.mix(project, sample(pos), &mut buf);
        out.push(&buf);
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
        if self.sequence != seq.id() {
            self.sequence = seq.id();
            self.audio.reset();
        }
        for a in project.assets() {
            if self.paths.get(&a.id).is_some_and(|p| *p != a.path) {
                self.cache.forget(a.id);
                self.failed.retain(|k, _| k.0 != a.id);
                // Clips are reopened on the new path as they're next heard.
                self.audio.reset();
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
            for layer in leaves(frame_at(project, time)) {
                let key = (layer.asset, layer.source_time);
                want.insert(key);
                let Some(asset) = project.asset(layer.asset).filter(|a| !a.generated()) else {
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
        for layer in leaves(frame_at(project, time)) {
            let key = (layer.asset, layer.source_time);
            let Some(asset) = project.asset(layer.asset).filter(|a| !a.generated()) else {
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
        let (cache, failed, stand_ins) = (&mut self.cache, &self.failed, &self.stand_ins);
        let layers = resolve::<(), _>(project, frame_at(project, time), &mut |_, layer| {
            // Not decoded yet: keep what's on screen. A failed decode leaves the layer out.
            let key = (layer.asset, layer.source_time);
            if failed.contains_key(&key) {
                return Ok(None);
            }
            let f = match cache.get(&key) {
                Some(f) => f,
                None => {
                    exact = false;
                    stand_ins.get(&key).cloned().flatten().ok_or(())?
                }
            };
            Ok(Some(Pixels::Frame(f)))
        })
        .ok()?;
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
    /// Text uploads from the last draw, by raster; holding the `Arc` keeps its address unique.
    texts: Vec<(Arc<Vec<u8>>, wgpu::Texture)>,
    /// `.cube` paths already read into the compositor, or that failed to.
    /// shortcut: read once per session; a file edited on disk shows after a restart.
    luts: HashSet<String>,
    /// The [`crate::effects::Library::generation`] last loaded into the compositor (`None`
    /// before the first load), and the effect stems set there.
    effects: (Option<u64>, HashSet<String>),
    /// Keep the effects first loaded, ignoring later edits; export sets it so a file changed or
    /// deleted mid-export doesn't change the output partway.
    pub pin_effects: bool,
    /// Effects named by a layer but not loaded, already warned about.
    missing: HashSet<String>,
    /// By layer index, what composites a nested sequence's layers.
    nested: HashMap<usize, Gpu>,
}

impl Gpu {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            compositor: Compositor::new(device),
            layers: Vec::new(),
            texts: Vec::new(),
            luts: HashSet::new(),
            effects: Default::default(),
            pin_effects: false,
            missing: HashSet::new(),
            nested: HashMap::new(),
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
        let index = shown.index.max(0) as u32;
        self.composite(device, queue, s, scale, s.background, index, &shown.layers)
    }

    /// `layers` composited over `background`, for timeline frame `index`.
    #[allow(clippy::too_many_arguments)]
    fn composite(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        s: &ProjectSettings,
        scale: u32,
        background: [u8; 4],
        index: u32,
        layers: &[(Pixels, Transform)],
    ) -> &wgpu::Texture {
        while self.layers.len() < layers.len() {
            self.layers.push(Preview::new(device, composite::FORMAT));
        }
        // shortcut: a solid texture per layer per frame; cache by color if playback profiles hot.
        let mut solids = Vec::new();
        let mut texts = Vec::new();
        for (i, ((px, _), preview)) in layers.iter().zip(&mut self.layers).enumerate() {
            let f = match px {
                Pixels::Frame(f) => f,
                Pixels::Nested(inner) => {
                    let pin = self.pin_effects;
                    let gpu = self.nested.entry(i).or_insert_with(|| Gpu {
                        pin_effects: pin,
                        ..Gpu::new(device)
                    });
                    let tex = gpu.composite(device, queue, s, scale, [0; 4], index, inner);
                    solids.push(tex.clone());
                    continue;
                }
                Pixels::Solid(c) => {
                    solids.push(solid::solid(device, queue, (s.width, s.height), *c));
                    continue;
                }
                Pixels::Text(rgba) => {
                    // A cached raster is the same Arc, so its texture is reused, not re-uploaded.
                    let old = self.texts.iter().find(|(r, _)| Arc::ptr_eq(r, rgba));
                    let tex = old.map_or_else(
                        || text::texture(device, queue, (s.width, s.height), rgba),
                        |(_, t)| t.clone(),
                    );
                    texts.push((rgba.clone(), tex.clone()));
                    solids.push(tex);
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
        self.texts = texts;
        for path in layers.iter().filter_map(|(_, t)| t.lut.as_ref()) {
            if self.luts.insert(path.clone()) {
                let lut = std::fs::read_to_string(path)
                    .map_err(|e| e.to_string())
                    .and_then(|text| jvid_render::lut::parse(&text));
                match lut {
                    Ok(lut) => self.compositor.add_lut(device, queue, path, &lut),
                    Err(e) => tracing::warn!("LUT {path}: {e}"),
                }
            }
        }
        let mut failed = Vec::new();
        crate::effects::with(|lib| {
            if self.effects.0 == Some(lib.generation)
                || (self.pin_effects && self.effects.0.is_some())
            {
                return;
            }
            let old = std::mem::take(&mut self.effects.1);
            for stem in &old {
                let _ = self.compositor.set_effect(device, stem, None);
            }
            for (stem, entry) in lib.entries() {
                let Ok(shader) = entry else { continue };
                match self.compositor.set_effect(device, stem, Some(shader)) {
                    Ok(()) => {
                        self.effects.1.insert(stem.to_owned());
                    }
                    Err(e) => failed.push((stem.to_owned(), e)),
                }
            }
            self.effects.0 = Some(lib.generation);
        });
        for (stem, e) in failed {
            tracing::warn!("effect {stem}: {e}");
            crate::effects::fail(&stem, e);
        }
        for (_, t) in layers {
            for e in &t.effects {
                if !self.effects.1.contains(&e.shader) && self.missing.insert(e.shader.clone()) {
                    tracing::warn!("effect {}: not loaded, drawing without it", e.shader);
                }
            }
        }
        self.compositor.frame = index;
        self.compositor.premultiplied = layers
            .iter()
            .map(|(px, _)| matches!(px, Pixels::Nested(_)))
            .collect();
        let layers: Vec<_> = layers
            .iter()
            .zip(&self.layers)
            .scan(solids.iter(), |solids, ((px, t), p)| {
                let tex = match px {
                    Pixels::Frame(_) => p.image().expect("uploaded above"),
                    Pixels::Solid(_) | Pixels::Text(_) | Pixels::Nested(_) => {
                        solids.next().expect("made above")
                    }
                };
                Some((tex, t.clone()))
            })
            .collect();
        self.compositor.render(
            device,
            queue,
            (s.width, s.height),
            scale,
            background,
            &layers,
        )
    }
}

#[cfg(test)]
pub(crate) mod tests;
