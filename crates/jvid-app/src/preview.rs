//! The Preview panel's viewer: the current source's frame, decoded off-thread and drawn by
//! jvid-render through an egui paint callback, fitted or zoomed on Viewer black. The Source
//! tab plays the clip opened from the bin, with J/K/L and in/out marks.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, wgpu};
use egui::{Align, Align2, Layout, Pos2, Rect, Stroke, Ui, UiBuilder, pos2, vec2};
use jvid_audio::{CpalBackend, Output};
use jvid_core::history::Command;
use jvid_core::model::{AssetId, MediaAsset, Project, ProjectSettings, Sequence};
use jvid_core::settings::PreviewQuality;
use jvid_core::time::{Time, format_timecode};
use jvid_jobs::Priority;
use jvid_media::decode::{DecoderPool, Frame};
use jvid_media::proxy::{self, Proxies};
use jvid_render::preview::{self as render, Yuv};
use jvid_render::scope::{Analysis, Scopes};
use jvid_render::thumb::Thumbs;
use jvid_render::{solid, text};
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{Button, Checkbox, Dropdown};

#[cfg(test)]
use crate::i18n::verbatim;
use crate::i18n::{Label, tr, trn};
use crate::keymap::{ACTIONS, Keymap};
use crate::playback::{self, Pixels, Playback, Shown};
use crate::scopes;
use crate::transport::{AudioSource, Transport};

const ZOOMS: [(Label, Option<f32>); 4] = [
    (|| tr!("preview.fit"), None),
    (|| "50%", Some(0.5)),
    (|| "100%", Some(1.0)),
    (|| "200%", Some(2.0)),
];
const QUALITIES: [(Label, PreviewQuality); 3] = [
    (|| tr!("preview.full"), PreviewQuality::Full),
    (|| "½", PreviewQuality::Half),
    (|| "¼", PreviewQuality::Quarter),
];
/// Action-safe and title-safe as fractions of the frame (SMPTE ST 2046-1).
const SAFE: [f32; 2] = [0.93, 0.9];
/// Aspect guides, width over height.
const ASPECTS: [(&str, f32); 2] = [("9:16", 9.0 / 16.0), ("1:1", 1.0)];

/// Framing guides drawn over the viewer by egui only, so they never reach a render or export.
#[derive(Default)]
struct Overlays {
    safe: bool,
    thirds: bool,
    center: bool,
    aspects: [bool; 2],
}

impl Overlays {
    /// Guide outlines (as rects) and lines over `image`, the frame's on-screen rect.
    fn shapes(&self, image: Rect) -> (Vec<Rect>, Vec<[Pos2; 2]>) {
        let (mut rects, mut lines) = (Vec::new(), Vec::new());
        if self.safe {
            rects.extend(SAFE.map(|f| Rect::from_center_size(image.center(), image.size() * f)));
        }
        for (on, (_, a)) in self.aspects.iter().zip(ASPECTS) {
            if *on {
                let w = image.width().min(image.height() * a);
                let h = image.height().min(image.width() / a);
                rects.push(Rect::from_center_size(image.center(), vec2(w, h)));
            }
        }
        if self.thirds {
            for t in [1.0 / 3.0, 2.0 / 3.0] {
                let x = image.left() + image.width() * t;
                let y = image.top() + image.height() * t;
                lines.push([pos2(x, image.top()), pos2(x, image.bottom())]);
                lines.push([pos2(image.left(), y), pos2(image.right(), y)]);
            }
        }
        if self.center {
            let (c, r) = (image.center(), image.width().min(image.height()) * 0.03);
            lines.push([c - vec2(r, 0.0), c + vec2(r, 0.0)]);
            lines.push([c - vec2(0.0, r), c + vec2(0.0, r)]);
        }
        (rects, lines)
    }
}

/// The Program monitor's grade compare. It reaches only the Program view, never an export.
struct Compare {
    bypass: bool,
    split: bool,
    /// Where the split sits across the frame, 0..=1; left of it is before the grade.
    at: f32,
}

impl Default for Compare {
    fn default() -> Self {
        Self {
            bypass: false,
            split: false,
            at: 0.5,
        }
    }
}

impl Compare {
    /// How much of the frame, from the left, draws ungraded (see `Compositor::before`).
    fn before(&self) -> f32 {
        match (self.bypass, self.split) {
            (true, _) => 1.0,
            (false, true) => self.at,
            (false, false) => 0.0,
        }
    }
}

type Decoded = (PathBuf, Time, Result<Pixels, String>);

/// Keymap actions the Source tab handles while it's showing a clip.
pub const SOURCE_ACTIONS: &[&str] = &[
    "play_stop",
    "shuttle_left",
    "shuttle_stop",
    "shuttle_right",
    "step_back",
    "step_forward",
    "go_to_start",
    "go_to_end",
    "mark_in",
    "mark_out",
    "clear_in",
    "clear_out",
    "clear_in_out",
    "go_to_in",
    "go_to_out",
];

/// Keymap actions the Program tab and timeline hand to timeline playback.
pub const PROGRAM_ACTIONS: &[&str] = &[
    "play_stop",
    "shuttle_left",
    "shuttle_stop",
    "shuttle_right",
    "loop_playback",
    "audio_scrub",
];

/// The 160×90 frame effect and text thumbnails draw on: see [`Preview::thumb_frame`].
#[derive(Default)]
struct Thumb {
    /// Its egui texture; `None` without wgpu.
    id: Option<egui::TextureId>,
    /// The program generation it shows; `None` for the bundled fallback.
    from: Option<u64>,
    /// The Effects or Text tab opened: refresh once the playhead settles, even if unchanged.
    wanted: bool,
    /// Copy the Program frame into it as it's next drawn.
    copy: bool,
}

/// The clip open in the Source tab.
struct Player {
    asset: AssetId,
    path: PathBuf,
    /// Has a video stream to decode; an unprobed asset is assumed to.
    video: bool,
    transport: Transport,
    audio: AudioSource,
}

pub struct Preview {
    /// Set by the app while a voiceover records: its elapsed time and input level, 0..=1.
    pub recording: Option<(std::time::Duration, f32)>,
    /// The Program row's Record button was clicked; the app runs the `record` action.
    pub record_clicked: bool,
    pool: Arc<DecoderPool>,
    jobs: jvid_jobs::Pool,
    tx: Sender<Decoded>,
    rx: Receiver<Decoded>,
    /// The source last asked for, and what came back for it.
    source: Option<PathBuf>,
    /// The time last asked for in `source`, and whether a decode is out (one at a time, so a
    /// slow decode drops frames instead of queueing them).
    requested: Option<Time>,
    pending: bool,
    source_job: Option<jvid_jobs::Handle>,
    frame: Option<Arc<Frame>>,
    still: Option<Arc<image::RgbaImage>>,
    error: Option<String>,
    /// Bumped when `frame` or the preview scale changes; the GPU side re-uploads when it sees a new one.
    generation: u64,
    zoom: usize,
    quality: usize,
    overlays: Overlays,
    compare: Compare,
    player: Option<Player>,
    /// Opened on first play, so tests and idle sessions never touch the audio device.
    output: Option<Output>,
    /// The timeline's playback, shown in the Program tab.
    program: Playback,
    /// The composited frame last returned, and a count bumped with it.
    program_shown: Option<Arc<Shown>>,
    /// A solid or title open in the Source tab, drawn like a one-layer timeline frame.
    source_shown: Option<(MediaAsset, Arc<Shown>)>,
    program_gen: u64,
    /// The timeline playhead last frame; a change is a seek.
    program_head: Time,
    /// The sequence and assets last frame; a change (a clip or a title's text) redraws a
    /// parked playhead.
    program_seq: Option<(Sequence, Vec<MediaAsset>)>,
    /// For full-size renders outside the paint callback, such as frame export.
    render_state: Option<egui_wgpu::RenderState>,
    /// Preview decodes these instead of the originals while on; export never does.
    proxies: Arc<Proxies>,
    proxy_jobs: jvid_jobs::Pool,
    /// Sources a proxy was queued for this session.
    proxied: HashSet<PathBuf>,
    /// Project sources [`proxy::suggested`] says want a proxy.
    suggested: usize,
    thumb: Thumb,
    scopes: scopes::State,
    /// What the Scopes tab shows, for a lesson step to read.
    pub analysis: Option<Analysis>,
}

impl Preview {
    /// Puts the GPU half in egui-wgpu's callback resources; without wgpu the viewer stays empty.
    pub fn new(render_state: Option<&egui_wgpu::RenderState>) -> Self {
        let mut thumb = Thumb::default();
        if let Some(rs) = render_state {
            let preview = render::Preview::new(&rs.device, rs.target_format);
            let gpu = Gpu {
                preview,
                shown: None,
                program: playback::Gpu::new(&rs.device),
                program_view: render::Preview::new(&rs.device, rs.target_format),
                program_drawn: None,
                thumbs: Thumbs::new(&rs.device, &rs.queue),
                scopes: Scopes::new(&rs.device),
            };
            let mut r = rs.renderer.write();
            let view = gpu.thumbs.frame().create_view(&Default::default());
            thumb.id = Some(r.register_native_texture(&rs.device, &view, wgpu::FilterMode::Linear));
            r.callback_resources.insert(gpu);
        }
        let (tx, rx) = channel();
        Self {
            pool: Arc::new(DecoderPool::hdr()),
            jobs: jvid_jobs::Pool::new(1),
            tx,
            rx,
            source: None,
            requested: None,
            pending: false,
            source_job: None,
            frame: None,
            still: None,
            error: None,
            generation: 0,
            zoom: 0,
            quality: 0,
            overlays: Overlays::default(),
            compare: Compare::default(),
            player: None,
            output: None,
            program: Playback::from_files(playback::DEFAULT_BUDGET),
            program_shown: None,
            source_shown: None,
            program_gen: 0,
            program_head: Time(0),
            program_seq: None,
            render_state: render_state.cloned(),
            proxies: Arc::new(Proxies::new(PathBuf::new())),
            proxy_jobs: jvid_jobs::Pool::new(1),
            proxied: HashSet::new(),
            suggested: 0,
            thumb,
            scopes: scopes::State::default(),
            analysis: None,
            recording: None,
            record_clicked: false,
        }
    }

    /// Clears both viewers and detaches old decode replies without joining worker pools.
    pub fn reset_session(&mut self) {
        if let Some(out) = &mut self.output {
            out.flush();
        }
        self.program.reset_session();
        self.player = None;
        self.source = None;
        self.requested = None;
        self.pending = false;
        if let Some(handle) = self.source_job.take() {
            handle.cancel();
        }
        self.frame = None;
        self.still = None;
        self.error = None;
        (self.tx, self.rx) = channel();
        self.generation += 1;
        self.program_shown = None;
        self.source_shown = None;
        self.program_gen += 1;
        self.program_head = Time(0);
        self.program_seq = None;
        self.compare = Compare::default();
        self.thumb.from = None;
        self.thumb.wanted = true;
        self.thumb.copy = false;
        self.scopes = scopes::State::default();
        self.analysis = None;
        self.recording = None;
        self.record_clicked = false;
    }

    /// Stores proxies in `proxies`' directory and switches timeline playback to decode through it.
    pub fn set_proxies(&mut self, proxies: Arc<Proxies>) {
        self.program = Playback::from_proxies(playback::DEFAULT_BUDGET, proxies.clone());
        self.proxies = proxies;
    }

    /// Counts the sources that want a proxy and, while proxies are on, queues the missing ones.
    fn proxy_sources(&mut self, project: &Project) {
        self.suggested = 0;
        for a in project.assets() {
            let Some(info) = a.probe.as_ref().filter(|i| proxy::suggested(i)) else {
                continue;
            };
            self.suggested += 1;
            if self.proxies.enabled() && self.proxied.insert(a.path.clone()) {
                self.proxies.submit(&self.proxy_jobs, a.path.clone(), info);
            }
        }
    }

    /// The device and queue for offscreen rendering, such as export; `None` without wgpu.
    pub fn gpu(&self) -> Option<(egui_wgpu::wgpu::Device, egui_wgpu::wgpu::Queue)> {
        self.render_state
            .as_ref()
            .map(|rs| (rs.device.clone(), rs.queue.clone()))
    }

    /// Saves the immutable timeline snapshot at `time` using the full-resolution Program path.
    pub fn export_frame(&self, project: &Project, time: Time, path: &Path) -> Result<(), String> {
        let Some(rs) = &self.render_state else {
            return Err(tr!("preview.no_frame").into());
        };
        crate::export::frame_png(project, time, &rs.device, &rs.queue, path)
    }

    /// Opens an asset in the Source tab, parked at its start.
    pub fn open(&mut self, project: &Project, id: AssetId) {
        let Some(a) = project.asset(id) else { return };
        let probe = a.probe.clone().unwrap_or_default();
        let rate = probe.frame_rate.unwrap_or(project.settings().frame_rate);
        // The old clip's queued audio mustn't play over the new one.
        if let Some(out) = &mut self.output {
            out.flush();
        }
        // Opening/reopening a Source is an explicit media refresh boundary. Metadata is
        // checked by its decode worker, never on each UI frame.
        if let Some(job) = self.source_job.take() {
            job.cancel();
        }
        (self.tx, self.rx) = channel();
        self.source = None;
        self.requested = None;
        self.pending = false;
        self.frame = None;
        self.still = None;
        self.error = None;
        self.generation += 1;
        self.program.refresh_media();
        self.player = Some(Player {
            asset: id,
            path: a.path.clone(),
            video: a.probe.as_ref().is_none_or(|p| p.width.is_some()),
            transport: Transport::new(probe.duration, rate),
            audio: AudioSource::open(&a.path, probe.audio_streams > 0),
        });
    }

    pub fn playing(&self) -> bool {
        self.program.transport.speed() != 0
    }

    pub fn source_asset(&self) -> Option<AssetId> {
        self.player.as_ref().map(|p| p.asset)
    }

    fn clock(&self) -> u64 {
        self.output.as_ref().map_or(0, Output::clock)
    }

    /// Runs a `SOURCE_ACTIONS` action on the open clip; marks come back as a command to apply.
    pub fn source_action(&mut self, id: &str, project: &Project) -> Option<Command> {
        let starts = matches!(id, "play_stop" | "shuttle_left" | "shuttle_right");
        if starts && self.player.is_some() && self.output.is_none() {
            self.output = Some(Output::new(Box::new(CpalBackend::default()), None));
        }
        let marking = id.starts_with("mark_") || id.starts_with("clear_");
        if !marking
            && self.player.is_some()
            && let Some(out) = &mut self.output
        {
            out.flush();
        }
        let clock = self.clock();
        let p = self.player.as_mut()?;
        let a = project.asset(p.asset)?;
        let t = &mut p.transport;
        let here = t.rate.time_floor(t.frame(clock));
        // Out is exclusive, like a clip's source_out: the end of the marked frame.
        let after = t.rate.time_floor(t.frame(clock) + 1);
        let marks = |in_point, out_point| {
            Some(Command::SetAssetInOut {
                id: a.id,
                in_point,
                out_point,
            })
        };
        match id {
            "play_stop" => t.toggle(clock),
            "shuttle_left" => t.shuttle(-1, clock),
            "shuttle_stop" => t.stop(clock),
            "shuttle_right" => t.shuttle(1, clock),
            "step_back" => t.step(-1, clock),
            "step_forward" => t.step(1, clock),
            "go_to_start" => t.seek(Time(0), clock),
            "go_to_end" => t.seek(t.duration, clock),
            "go_to_in" => t.seek(a.in_point.unwrap_or(Time(0)), clock),
            "go_to_out" => {
                let end = a.out_point.map_or(t.duration, |o| {
                    t.rate.time_floor(t.rate.index_floor(Time(o.0 - 1)))
                });
                t.seek(end, clock);
            }
            // A mark past the other one clears it.
            "mark_in" => return marks(Some(here), a.out_point.filter(|o| *o > here)),
            "mark_out" => return marks(a.in_point.filter(|i| *i < after), Some(after)),
            "clear_in" => return marks(None, a.out_point),
            "clear_out" => return marks(a.in_point, None),
            "clear_in_out" => return marks(None, None),
            _ => {}
        }
        None
    }

    /// The Source tab: the open clip at its playhead, with the transport row. Returns a mark
    /// button's command to apply.
    pub fn show_source(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        missing: &HashSet<PathBuf>,
        keymap: &Keymap,
    ) -> Option<Command> {
        if let Some(p) = &self.player
            && project.asset(p.asset).is_none()
        {
            self.player = None;
        }
        if self.program.transport.speed() != 0 {
            self.program_action("shuttle_stop");
        }
        let Some(p) = &mut self.player else {
            self.show(ui, None, false, None, None, keymap);
            ui.painter().text(
                ui.max_rect().center(),
                Align2::CENTER_CENTER,
                tr!("preview.source_empty"),
                theme::body(),
                color::ink_3(),
            );
            return None;
        };
        if let Some(out) = &mut self.output {
            out.poll();
            let audio = &mut p.audio;
            p.transport.tick(out, &mut |s, b| audio.fill(s, b));
        }
        let clock = self.output.as_ref().map_or(0, Output::clock);
        let t = &p.transport;
        if t.speed() != 0 {
            ui.ctx().request_repaint();
        }
        let frame = t.frame(clock);
        let asset = project.asset(p.asset);
        let row = SourceRow {
            frame,
            end: t.rate.index_floor(t.duration),
            rate: t.rate,
            playing: t.speed() != 0,
            in_point: asset.and_then(|a| a.in_point),
            out_point: asset.and_then(|a| a.out_point),
            duration: t.duration,
            position: t.rate.time_floor(frame),
        };
        let input = match asset.filter(|a| a.generated()) {
            Some(a) => {
                if self.source_shown.as_ref().is_none_or(|(was, _)| was != a) {
                    let s = project.settings();
                    let px = match &a.text {
                        Some(t) => Pixels::Text(text::rasterize(t, None, (s.width, s.height))),
                        None => Pixels::Solid(solid::rgba8(a.solid.unwrap_or_default())),
                    };
                    let shown = Shown {
                        index: 0,
                        layers: vec![(px, Default::default())],
                        exact: true,
                    };
                    self.source_shown = Some((a.clone(), Arc::new(shown)));
                    self.generation += 1;
                }
                let program = Some((project.settings(), true));
                self.show(ui, None, false, Some(&row), program, keymap)
            }
            None => {
                let (path, video) = (p.path.clone(), p.video);
                let offline = missing.contains(&path);
                let source = Some((path.as_path(), row.position, video));
                self.show(ui, source, offline, Some(&row), None, keymap)
            }
        };
        if let Some(time) = input.seek {
            self.seek_source(time);
            ui.ctx().request_repaint();
        }
        input
            .action
            .and_then(|action| self.source_action(action, project))
    }

    fn seek_source(&mut self, time: Time) {
        let clock = self.clock();
        let Some(player) = &mut self.player else {
            return;
        };
        let t = &mut player.transport;
        let last = t
            .rate
            .index_floor(Time(t.duration.0.saturating_sub(1)))
            .max(0);
        let frame = t.rate.index_floor(time).clamp(0, last);
        if frame == t.frame(clock) {
            return;
        }
        if let Some(output) = &mut self.output {
            output.flush();
        }
        t.seek(t.rate.time_floor(frame), clock);
        // Keep one decode in flight; discard its old result and request the latest
        // position when it completes instead of queueing every drag event.
        self.requested = None;
        self.frame = None;
        self.error = None;
        self.generation += 1;
    }

    /// Runs a `PROGRAM_ACTIONS` action on timeline playback.
    pub fn program_action(&mut self, id: &str) {
        if id == "loop_playback" {
            self.program.looping = !self.program.looping;
            return;
        }
        if id == "audio_scrub" {
            self.program.scrub_audio = !self.program.scrub_audio;
            return;
        }
        let out = self
            .output
            .get_or_insert_with(|| Output::new(Box::new(CpalBackend::default()), None));
        out.flush();
        let clock = out.clock();
        let t = &mut self.program.transport;
        match id {
            "play_stop" => t.toggle(clock),
            "shuttle_left" => t.shuttle(-1, clock),
            "shuttle_right" => t.shuttle(1, clock),
            _ => t.stop(clock),
        }
    }

    /// The Program tab: the timeline composited at `playhead`, or `trim` (the source frame at a
    /// dragged clip edge, and whether it has video to decode) instead, with the transport row.
    /// Returns where the playhead moved to while playing or from a transport button.
    pub fn show_program(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        playhead: Time,
        trim: Option<(&Path, Time, bool)>,
        offline: bool,
        keymap: &Keymap,
    ) -> Option<Time> {
        if let Some(p) = &mut self.player
            && p.transport.speed() != 0
        {
            p.transport
                .stop(self.output.as_ref().map_or(0, Output::clock));
        }
        // An empty sequence leaves the viewer empty, like a Source tab with nothing open.
        let empty = project.sequence().duration() == Time(0);
        if trim.is_some() || empty {
            // A trim is a drag. A new frame lands as this one draws: one more pass shows it in
            // the thumbnails drawn on it.
            if self.update_thumb(empty, trim.is_some()) {
                ui.ctx().request_repaint();
            }
            if empty {
                scopes::clear(ui.ctx());
                self.analysis = None;
            }
            self.show(ui, trim, offline, None, None, keymap);
            return None;
        }
        self.proxy_sources(project);
        let dragging = ui.input(|i| i.pointer.primary_down());
        let ctx = ui.ctx().clone();
        self.program
            .set_decode_notify(Arc::new(move || ctx.request_repaint()));
        let follow = self.sync_program(project, playhead, dragging);
        if self.update_thumb(false, dragging) {
            ui.ctx().request_repaint();
        }
        self.update_scopes(ui.ctx(), dragging);
        if self.program.transport.speed() != 0 {
            ui.ctx().request_repaint();
        }
        let t = &self.program.transport;
        let frame = t.frame(self.clock());
        let row = SourceRow {
            frame,
            end: t.rate.index_floor(t.duration),
            rate: t.rate,
            playing: t.speed() != 0,
            in_point: None,
            out_point: None,
            duration: t.duration,
            position: t.rate.time_floor(frame),
        };
        let end = t
            .rate
            .time_floor(t.rate.index_floor(Time(t.duration.0 - 1)).max(0));
        let input = self.show(
            ui,
            None,
            false,
            Some(&row),
            Some((project.settings(), false)),
            keymap,
        );
        if let Some(time) = input.seek {
            return Some(time);
        }
        match input.action {
            Some("go_to_start") => Some(Time(0)),
            Some("go_to_end") => Some(end),
            Some(id) => {
                self.program_action(id);
                ui.ctx().request_repaint();
                follow
            }
            None => follow,
        }
    }

    /// Seeks timeline playback to a moved `playhead` and runs it a frame; see `show_program`.
    /// A parked seek while `dragging` scrubs audio.
    fn sync_program(&mut self, project: &Project, playhead: Time, dragging: bool) -> Option<Time> {
        let pb = &mut self.program;
        let seq = project.sequence();
        let now = (seq, project.assets());
        if self
            .program_seq
            .as_ref()
            .is_none_or(|(_, assets)| assets.as_slice() != project.assets())
        {
            pb.refresh_media();
        }
        if self.program_seq.as_ref().map(|(s, a)| (s, a.as_slice())) != Some(now) {
            self.program_seq = Some((seq.clone(), project.assets().to_vec()));
            pb.refresh();
        }
        if playhead != self.program_head {
            let clock = self.output.as_mut().map_or(0, |o| {
                o.flush();
                o.clock()
            });
            // Before the first tick has set it, or the seek would clamp to 0.
            pb.transport.duration = seq.duration();
            pb.transport.seek(playhead, clock);
            pb.refresh();
            if dragging && pb.scrub_audio && pb.transport.speed() == 0 {
                let out = self
                    .output
                    .get_or_insert_with(|| Output::new(Box::new(CpalBackend::default()), None));
                pb.scrub(project, out);
            }
        }
        let playing = pb.transport.speed() != 0;
        let shown = match &mut self.output {
            Some(out) => {
                out.poll();
                pb.tick(project, out)
            }
            None => pb.park(project),
        };
        if let Some(s) = shown {
            self.program_shown = Some(Arc::new(s));
            self.program_gen += 1;
        }
        let clock = self.output.as_ref().map_or(0, Output::clock);
        let t = &pb.transport;
        // Playing (or just stopped at the end), the timeline follows; parked, it leads.
        self.program_head = if playing {
            t.rate.time_floor(t.frame(clock))
        } else {
            playhead
        };
        playing.then_some(self.program_head)
    }

    /// Refreshes the thumbnail frame once the playhead settles on a frame it doesn't show yet,
    /// or after [`Preview::refresh_thumb`]; never while playing or scrubbing. Nothing at the
    /// playhead shows the fallback; otherwise the Program frame is copied as it's drawn.
    /// Returns whether it refreshed.
    fn update_thumb(&mut self, empty: bool, dragging: bool) -> bool {
        let shown = self.program_shown.as_ref().filter(|_| !empty);
        let playing = self.program.transport.speed() != 0;
        if playing || dragging || shown.is_some_and(|s| !s.exact) {
            return false;
        }
        let from = shown
            .filter(|s| !s.layers.is_empty())
            .map(|_| self.program_gen);
        let wanted = std::mem::take(&mut self.thumb.wanted);
        if from == self.thumb.from && !wanted {
            return false;
        }
        (self.thumb.from, self.thumb.copy) = (from, from.is_some());
        if from.is_none() {
            self.with_thumbs(|t, d, q| t.set_fallback(d, q));
        }
        true
    }

    /// Whether the Scopes tab is showing; the Program frame is copied for it only while it is.
    pub fn scopes_open(&mut self, open: bool) {
        self.scopes.open = open;
    }

    /// Shows scopes that have landed, then picks whether the Program frame drawn next is
    /// copied for them; see [`scopes::State::due`]. Never waits on the GPU.
    fn update_scopes(&mut self, ctx: &egui::Context, dragging: bool) {
        if !self.scopes.open || self.program_shown.is_none() {
            return;
        }
        let polled = self.with_scopes(|s, d| (s.poll(d), s.busy()));
        let (landed, busy) = polled.unwrap_or((None, false));
        if let Some(a) = landed {
            scopes::set(ctx, &a);
            self.analysis = Some(a);
        }
        let moving = dragging || self.program.transport.speed() != 0;
        let now = ctx.input(|i| i.time);
        // A copy in flight lands within a frame or two; draw again to pick it up.
        if self.scopes.due(self.program_gen, now, moving, busy) || busy {
            ctx.request_repaint();
        }
    }

    /// Runs `f` on the scopes' renderer; `None` without wgpu.
    fn with_scopes<R>(&self, f: impl FnOnce(&mut Scopes, &wgpu::Device) -> R) -> Option<R> {
        let rs = self.render_state.as_ref()?;
        let mut r = rs.renderer.write();
        let gpu = r.callback_resources.get_mut::<Gpu>()?;
        Some(f(&mut gpu.scopes, &rs.device))
    }

    /// The Effects or Text tab opened: refresh the thumbnail frame once the playhead settles.
    pub fn refresh_thumb(&mut self) {
        self.thumb.wanted = true;
    }

    /// The thumbnail frame as an egui texture, and its [`Thumbs::generation`]; `None` without
    /// wgpu. The Program frame is copied in as it's drawn, so a change shows the frame after.
    pub fn thumb_frame(&self) -> Option<(egui::TextureId, u64)> {
        Some((self.thumb.id?, self.with_thumbs(|t, _, _| t.generation)?))
    }

    /// Draws a thumbnail with `f`, such as [`Thumbs::effect`] or [`Thumbs::text`], into egui
    /// texture `id` (or a new one) and returns it; `None` without wgpu.
    pub fn draw_thumb(
        &self,
        id: Option<egui::TextureId>,
        f: impl FnOnce(&mut Thumbs, &wgpu::Device, &wgpu::Queue) -> wgpu::Texture,
    ) -> Option<egui::TextureId> {
        let tex = self.with_thumbs(f)?;
        let rs = self.render_state.as_ref()?;
        let view = tex.create_view(&Default::default());
        let (mut r, filter) = (rs.renderer.write(), wgpu::FilterMode::Linear);
        Some(match id {
            Some(id) => {
                r.update_egui_texture_from_wgpu_texture(&rs.device, &view, filter, id);
                id
            }
            None => r.register_native_texture(&rs.device, &view, filter),
        })
    }

    /// Runs `f` on the thumbnail renderer; `None` without wgpu.
    pub fn with_thumbs<R>(
        &self,
        f: impl FnOnce(&mut Thumbs, &wgpu::Device, &wgpu::Queue) -> R,
    ) -> Option<R> {
        let rs = self.render_state.as_ref()?;
        let mut r = rs.renderer.write();
        let gpu = r.callback_resources.get_mut::<Gpu>()?;
        Some(f(&mut gpu.thumbs, &rs.device, &rs.queue))
    }

    /// Where the project frame sits in the viewer `ui` draws into, at the current zoom.
    pub fn project_rect(&self, ui: &Ui, settings: &ProjectSettings) -> Rect {
        let rect = ui.max_rect();
        let view = [
            rect.left(),
            rect.top(),
            rect.width(),
            rect.height() - size::HEADER,
        ];
        let zoom = ZOOMS[self.zoom].1.map(|z| z / ui.pixels_per_point());
        let size = [settings.width as f32, settings.height as f32];
        let [x, y, w, h] = render::place(size, view, zoom);
        Rect::from_min_size(pos2(x, y), vec2(w, h))
    }

    /// Draws `source` at a time, decoding it first if it changed and has video; an `offline`
    /// source shows a placeholder instead. With `program`, draws the composited timeline frame instead, or
    /// with its flag set, the generated asset open in the Source tab.
    /// Returns a transport button's action id when one was clicked.
    fn show(
        &mut self,
        ui: &mut Ui,
        source: Option<(&Path, Time, bool)>,
        offline: bool,
        row: Option<&SourceRow>,
        program: Option<(&ProjectSettings, bool)>,
        keymap: &Keymap,
    ) -> TransportInput {
        // Only the timeline's own row records.
        let records = matches!(program, Some((_, false)));
        let path = source.map(|(p, ..)| p);
        if self.source.as_deref() != path {
            self.source = path.map(Path::to_path_buf);
            (self.frame, self.still, self.error, self.requested) = (None, None, None, None);
        }
        if offline {
            // Decodes (and warns) again once it's back.
            (self.frame, self.still, self.error, self.requested) = (None, None, None, None);
        }
        let source = source.map(|(path, t, video)| {
            if jvid_media::stills::is_still(path) {
                (path, Time(0), video)
            } else {
                (path, t, video)
            }
        });
        if let Some((path, t, true)) = source
            && !offline
            && self.requested != Some(t)
            && !self.pending
        {
            (self.requested, self.pending) = (Some(t), true);
            let (tx, ctx, p) = (self.tx.clone(), ui.ctx().clone(), path.to_path_buf());
            self.source_job = Some(if jvid_media::stills::is_still(path) {
                self.jobs.submit(
                    format!("source-still:{:?}:{}", p, self.generation),
                    Priority::Interactive,
                    move |_| {
                        let r = jvid_media::stills::cached(&p).map(Pixels::Still);
                        let _ = tx.send((p, t, r));
                        ctx.request_repaint();
                    },
                )
            } else {
                self.pool.submit(
                    &self.jobs,
                    self.proxies.resolve(path),
                    t,
                    Priority::Interactive,
                    move |r| {
                        let _ = tx.send((
                            p,
                            t,
                            r.map(|f| Pixels::Frame(Arc::new(f)))
                                .map_err(|e| e.to_string()),
                        ));
                        ctx.request_repaint();
                    },
                )
            });
        }
        while let Ok((path, t, r)) = self.rx.try_recv() {
            self.source_job = None;
            self.pending = false;
            if self.source.as_deref() != Some(&path) {
                continue;
            }
            // A newer time is wanted; go again next frame.
            if self.requested != Some(t) {
                ui.ctx().request_repaint();
                continue;
            }
            match r {
                Ok(f) => {
                    self.error = None;
                    match f {
                        Pixels::Frame(f) => {
                            self.frame = Some(f);
                            self.still = None;
                        }
                        Pixels::Still(f) => {
                            self.still = Some(f);
                            self.frame = None;
                        }
                        _ => unreachable!("only file pixels are decoded"),
                    }
                    self.generation += 1;
                }
                Err(e) => {
                    // Once per source, not once per frame asked for.
                    if self.error.is_none() {
                        tracing::warn!("preview decode of {} failed: {e}", path.display());
                    }
                    self.error = Some(e);
                }
            }
        }
        if program.is_none() && self.source.is_none() {
            return TransportInput::default();
        }

        let rect = ui.max_rect();
        let viewer = viewer_rect(ui);
        ui.painter().rect_filled(viewer, 0.0, color::VIEWER);
        if offline {
            ui.painter().text(
                viewer.center(),
                Align2::CENTER_BOTTOM,
                theme::icon::CIRCLE_ALERT,
                theme::icon_font(),
                color::red(),
            );
            ui.painter().text(
                viewer.center() + vec2(0.0, space::S8),
                Align2::CENTER_TOP,
                tr!("preview.offline"),
                theme::body(),
                color::ink_2(),
            );
        } else if let Some(e) = &self.error {
            ui.painter().text(
                viewer.center(),
                Align2::CENTER_CENTER,
                tr!("preview.frame_error", error = e),
                theme::body(),
                color::ink_2(),
            );
        }
        let still_settings = self.still.as_ref().map(|i| ProjectSettings {
            width: i.width(),
            height: i.height(),
            ..Default::default()
        });
        let still_shown = self.still.as_ref().map(|i| {
            Arc::new(Shown {
                index: 0,
                layers: vec![(Pixels::Still(i.clone()), Default::default())],
                exact: true,
            })
        });
        let program = program.or_else(|| still_settings.as_ref().map(|s| (s, true)));
        let source = matches!(program, Some((_, true)));
        let shown = if let Some(shown) = still_shown.as_ref().filter(|_| source) {
            Some((shown, self.generation))
        } else {
            match program {
                Some((_, true)) => self
                    .source_shown
                    .as_ref()
                    .map(|(_, s)| (s, self.generation)),
                _ => self.program_shown.as_ref().map(|s| (s, self.program_gen)),
            }
        };
        let program = program.map(|(s, _)| s);
        let drawn = match (program, shown) {
            (Some(s), Some(_)) => Some([s.width as f32, s.height as f32]),
            (Some(_), None) => None,
            (None, _) => self
                .frame
                .as_ref()
                .map(|f| [f.width as f32, f.height as f32]),
        };
        if let Some(drawn) = drawn {
            let view = [viewer.left(), viewer.top(), viewer.width(), viewer.height()];
            // 100% is one frame pixel per screen pixel.
            let zoom = ZOOMS[self.zoom].1.map(|z| z / ui.pixels_per_point());
            let image = render::place(drawn, view, zoom);
            let scale = match QUALITIES[self.quality].1 {
                PreviewQuality::Full => 1,
                PreviewQuality::Half => 2,
                PreviewQuality::Quarter => 4,
            };
            let clip = render::to_clip(image, view);
            let callback = match (program, shown, &self.frame) {
                (Some(s), Some((shown, g)), _) => egui_wgpu::Callback::new_paint_callback(
                    viewer,
                    ProgramPaint {
                        shown: shown.clone(),
                        settings: s.clone(),
                        // Source and program counters overlap; the source flag keeps them apart.
                        generation: (g, scale, source),
                        before: if source { 0.0 } else { self.compare.before() },
                        clip,
                        thumb: !source && std::mem::take(&mut self.thumb.copy),
                        scope: !source && std::mem::take(&mut self.scopes.copy),
                    },
                ),
                (_, _, Some(f)) => egui_wgpu::Callback::new_paint_callback(
                    viewer,
                    Paint {
                        frame: f.clone(),
                        generation: self.generation,
                        scale,
                        clip,
                    },
                ),
                _ => unreachable!("drawn is set"),
            };
            ui.painter().add(callback);
            let image = Rect::from_min_size(pos2(image[0], image[1]), vec2(image[2], image[3]));
            let (rects, lines) = self.overlays.shapes(image);
            let painter = ui.painter().with_clip_rect(viewer);
            let stroke = Stroke::new(1.0, color::ink().gamma_multiply(0.6));
            for r in rects {
                painter.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Middle);
            }
            for l in lines {
                painter.line_segment(l, stroke);
            }
            if program.is_some() && !source && self.compare.split && !self.compare.bypass {
                self.drag_split(ui, image);
            }
        }

        let bar =
            Rect::from_x_y_ranges(rect.x_range(), rect.bottom() - size::HEADER..=rect.bottom());
        let strip = Rect::from_x_y_ranges(rect.x_range(), viewer.bottom()..=bar.top());
        let mut action = None;
        let mut seek = None;
        if let Some(r) = row {
            seek = r.marks(ui, strip);
            let inner = bar.shrink2(vec2(space::INSET, 0.0));
            let time_width = ui
                .painter()
                .layout_no_wrap(
                    format_timecode(r.frame as u64, r.rate),
                    theme::timecode(),
                    color::ink(),
                )
                .size()
                .x;
            let record_extra = if records { 32.0 } else { 0.0 };
            let take_extra = if records {
                self.recording.map_or(0.0, |take| take_width(ui, take.0))
            } else {
                0.0
            };
            // Dropdowns have fixed widths except for the translated checkbox label.
            // Reserve its measured text plus the box, guides and all intervening gaps.
            let proxy_text = ui
                .painter()
                .layout_no_wrap(tr!("preview.proxies").into(), theme::body(), color::ink())
                .size()
                .x;
            let secondary_width = 88.0 + 72.0 + proxy_text + 24.0 + 32.0 + 5.0 * space::S8;
            let full_width = time_width
                + 5.0 * 28.0
                + record_extra
                + take_extra
                + secondary_width
                + 3.0 * space::S8;
            if inner.width() < full_width {
                return TransportInput {
                    action: self.compact_transport(ui, inner, r, records, keymap),
                    seek,
                };
            }
            let timecode = ui.painter().text(
                inner.left_center(),
                Align2::LEFT_CENTER,
                format_timecode(r.frame as u64, r.rate),
                theme::timecode(),
                color::ink(),
            );
            let records = records.then_some(self.recording);
            // Five buttons, then Record after a wider gap; a take adds its time and meter.
            let width = 5.0 * 28.0 + records.map_or(0.0, |_| 28.0 + space::S4);
            let extra = match records {
                Some(Some(take)) => take_width(ui, take.0),
                _ => 0.0,
            };
            // The dropdowns first, so the buttons can stay clear of them in a narrow panel.
            let rtl = Layout::right_to_left(Align::Center);
            let right = ui.scope_builder(UiBuilder::new().max_rect(inner).layout(rtl), |ui| {
                ui.spacing_mut().item_spacing.x = space::S8;
                self.dropdowns(ui);
                let text = egui::RichText::new(format_timecode(r.end as u64, r.rate))
                    .font(theme::body())
                    .color(color::ink_2());
                let galley = ui.painter().layout_no_wrap(
                    text.text().to_string(),
                    theme::body(),
                    color::ink_2(),
                );
                let room =
                    ui.available_width() - timecode.width() - width - extra - 4.0 * space::S8;
                if galley.size().x <= room {
                    ui.add(egui::Label::new(text).selectable(false))
                        .on_hover_text(tr!("bin.sort_duration"));
                }
                ui.min_rect().left()
            });
            let x = bar
                .center()
                .x
                .min(right.inner - space::S8 - width / 2.0 - extra)
                .max(timecode.right() + space::S8 + width / 2.0);
            // Centred on the buttons alone, so a take's readout doesn't shift them.
            let buttons = Rect::from_min_size(
                pos2(x - width / 2.0, bar.top()),
                vec2(width + extra, bar.height()),
            );
            ui.scope_builder(
                UiBuilder::new()
                    .id_salt("transport_controls")
                    .max_rect(buttons)
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = space::S4;
                    for (glyph, name, id) in transport_actions(r.playing) {
                        let name = transport_hint(ui.ctx(), keymap, id, name);
                        if ui
                            .add(Button::icon(glyph, &name))
                            .on_hover_text(&name)
                            .clicked()
                        {
                            action = Some(id);
                        }
                    }
                    if let Some(take) = records {
                        ui.add_space(space::S4);
                        self.record_clicked |= record_button(ui, take, true, keymap);
                    }
                },
            );
        } else {
            ui.scope_builder(UiBuilder::new().max_rect(bar), |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = space::S8;
                    self.dropdowns(ui);
                });
            });
        }
        TransportInput { action, seek }
    }

    /// The Program row's Bypass Grade and before/after split toggles.
    /// The Program header's compare toggles, each a glyph and name, and which are on.
    pub fn compare_actions(&self) -> ([(&'static str, &'static str); 2], [bool; 2]) {
        let actions = [
            (icon::EYE_OFF, tr!("preview.bypass_grade")),
            (icon::COMPARE, tr!("preview.split")),
        ];
        (actions, [self.compare.bypass, self.compare.split])
    }

    /// Flips the [`Self::compare_actions`] toggle at `i`.
    pub fn toggle_compare(&mut self, i: usize) {
        let c = &mut self.compare;
        let on = if i == 0 { &mut c.bypass } else { &mut c.split };
        *on = !*on;
    }

    /// The before/after split's line over `image` (the frame's on-screen rect), dragged sideways.
    fn drag_split(&mut self, ui: &Ui, image: Rect) {
        let x = image.left() + image.width() * self.compare.at;
        let hit = Rect::from_x_y_ranges(x - space::S4..=x + space::S4, image.y_range());
        let r = ui.interact(hit, ui.id().with("compare split"), egui::Sense::drag());
        if let Some(p) = r.interact_pointer_pos().filter(|_| r.dragged()) {
            self.compare.at = ((p.x - image.left()) / image.width()).clamp(0.0, 1.0);
        }
        if r.hovered() || r.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let x = image.left() + image.width() * self.compare.at;
        let line = [pos2(x, image.top()), pos2(x, image.bottom())];
        ui.painter()
            .with_clip_rect(viewer_rect(ui))
            .line_segment(line, Stroke::new(1.0, color::ink()));
    }

    /// A bounded row: keep the primary controls visible and move secondary controls
    /// into the same discoverable menu at every compact width.
    fn compact_transport(
        &mut self,
        ui: &mut Ui,
        inner: Rect,
        row: &SourceRow,
        records: bool,
        keymap: &Keymap,
    ) -> Option<&'static str> {
        let timecode = format_timecode(row.frame as u64, row.rate);
        let time_width = ui
            .painter()
            .layout_no_wrap(timecode.clone(), theme::timecode(), color::ink())
            .size()
            .x;
        let navigation =
            inner.width() >= time_width + 5.0 * 28.0 + if records { 32.0 } else { 0.0 } + 48.0;
        let buttons_width =
            if navigation { 5.0 * 28.0 } else { 24.0 } + if records { 32.0 } else { 0.0 };
        let controls_width = buttons_width + space::S8 + size::CONTROL;
        let info = inner.with_max_x(inner.right() - controls_width - space::S8);
        let take = records.then_some(self.recording).flatten();
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(info)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = space::S8;
                let show_timecode =
                    take.is_none() || info.width() >= time_width + TAKE_WIDTH + space::S8;
                if show_timecode {
                    ui.add_sized(
                        vec2(time_width.min(info.width()), size::CONTROL),
                        egui::Label::new(egui::RichText::new(&timecode).font(theme::timecode()))
                            .truncate(),
                    );
                }
                if let Some(take) = take {
                    let width = ui.available_width().max(0.0);
                    take_readout(ui, take, width);
                }
            },
        );
        let controls = inner.with_min_x(inner.right() - controls_width);
        let mut action = None;
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("transport_controls")
                .max_rect(controls)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = space::S4;
                for (glyph, name, id) in transport_actions(row.playing) {
                    if navigation || id == "play_stop" {
                        let hint = transport_hint(ui.ctx(), keymap, id, name);
                        if ui
                            .add(Button::icon(glyph, &hint))
                            .on_hover_text(&hint)
                            .clicked()
                        {
                            action = Some(id);
                        }
                    }
                }
                if records {
                    ui.add_space(space::S4);
                    self.record_clicked |= record_button(ui, self.recording, false, keymap);
                }
                ui.add_space(space::S4);
                let more = ui
                    .add(Button::icon(icon::ELLIPSIS, tr!("preview.more")))
                    .on_hover_text(tr!("preview.more"));
                egui::Popup::menu(&more).show(|ui| {
                    if !navigation {
                        for (_, name, id) in transport_actions(row.playing) {
                            if id != "play_stop" {
                                let hint = transport_hint(ui.ctx(), keymap, id, name);
                                if ui.add(Button::ghost(&hint)).clicked() {
                                    action = Some(id);
                                    ui.close();
                                }
                            }
                        }
                        ui.separator();
                    }
                    self.dropdowns(ui);
                });
            },
        );
        action
    }

    fn dropdowns(&mut self, ui: &mut Ui) {
        let zooms = ZOOMS.map(|z| z.0());
        ui.add(Dropdown::new(&mut self.zoom, &zooms).width(88.0))
            .on_hover_text(tr!("zoom.label"));
        let qualities = QUALITIES.map(|q| q.0());
        if ui
            .add(Dropdown::new(&mut self.quality, &qualities).width(72.0))
            .on_hover_text(tr!("preview.resolution"))
            .changed()
        {
            self.generation += 1;
        }
        let mut on = self.proxies.enabled();
        let hint = match self.suggested {
            0 => tr!("preview.proxies_tooltip").to_string(),
            n => trn!("preview.proxies_suggested", n),
        };
        if ui
            .add(Checkbox::new(&mut on, tr!("preview.proxies")))
            .on_hover_text(hint)
            .changed()
        {
            self.proxies.set_enabled(on);
            // Frames decoded from the other files are stale now.
            self.program.cache = playback::FrameCache::new(playback::DEFAULT_BUDGET);
            self.program.refresh();
            self.requested = None;
        }
        let glyph = egui::RichText::new(icon::GRID).font(theme::icon_font());
        let guides = ui.menu_button(glyph, |ui| {
            let o = &mut self.overlays;
            ui.add(Checkbox::new(&mut o.safe, tr!("preview.safe_areas")));
            ui.add(Checkbox::new(&mut o.thirds, tr!("preview.thirds")));
            ui.add(Checkbox::new(&mut o.center, tr!("preview.center")));
            for (on, (ratio, _)) in o.aspects.iter_mut().zip(ASPECTS) {
                ui.add(Checkbox::new(
                    on,
                    &tr!("preview.aspect_guide", ratio = ratio),
                ));
            }
        });
        guides.response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr!("preview.guides"))
        });
        guides.response.on_hover_text(tr!("preview.guides"));
    }
}

/// Shared transport actions, in visual and keyboard-navigation order.
fn transport_actions(playing: bool) -> [(&'static str, &'static str, &'static str); 5] {
    [
        (icon::SKIP_BACK, tr!("action.go_to_start"), "go_to_start"),
        (icon::REWIND, tr!("action.shuttle_left"), "shuttle_left"),
        (
            if playing { icon::PAUSE } else { icon::PLAY },
            if playing {
                tr!("preview.stop")
            } else {
                tr!("preview.play")
            },
            "play_stop",
        ),
        (
            icon::FAST_FORWARD,
            tr!("action.shuttle_right"),
            "shuttle_right",
        ),
        (icon::SKIP_FORWARD, tr!("action.go_to_end"), "go_to_end"),
    ]
}

/// The same live, localized hint labels both the tooltip and accessible control.
fn transport_hint(ctx: &egui::Context, keymap: &Keymap, id: &str, name: &str) -> String {
    let index = ACTIONS
        .iter()
        .position(|action| action.id == id)
        .expect("transport action");
    match keymap.bindings[index].as_ref() {
        Some(binding) => tr!(
            "preview.with_key",
            action = name,
            key = ctx.format_shortcut(binding)
        ),
        None => name.to_owned(),
    }
}

fn take_width(ui: &Ui, elapsed: std::time::Duration) -> f32 {
    let s = elapsed.as_secs();
    ui.painter()
        .layout_no_wrap(
            format!("{}:{:02}", s / 60, s % 60),
            theme::body(),
            color::ink(),
        )
        .size()
        .x
        + 32.0
        + 3.0 * space::S4
}

/// Truncate the elapsed label within its budget; the full time remains in its hint.
fn take_readout(ui: &mut Ui, (elapsed, level): (std::time::Duration, f32), width: f32) {
    let s = elapsed.as_secs();
    let text = format!("{}:{:02}", s / 60, s % 60);
    let meter_width = 32.0_f32.min(width * 0.3);
    let text_width = (width - meter_width - space::S4).max(0.0);
    ui.spacing_mut().item_spacing.x = space::S4;
    ui.add_sized(
        vec2(text_width, size::CONTROL),
        egui::Label::new(
            egui::RichText::new(&text)
                .font(theme::body())
                .color(color::ink()),
        )
        .truncate(),
    )
    .on_hover_text(&text);
    let (bar, _) = ui.allocate_exact_size(vec2(meter_width, 4.0), egui::Sense::hover());
    crate::timeline::meter(ui.painter(), bar, level);
}

/// Room for a take's elapsed time and meter after the Record button.
const TAKE_WIDTH: f32 = 84.0;

/// The Program row's Record button: a dot, or while `take` records a Red stop square with its
/// elapsed time and input meter. Returns whether it was clicked.
fn record_button(
    ui: &mut Ui,
    take: Option<(std::time::Duration, f32)>,
    readout: bool,
    keymap: &Keymap,
) -> bool {
    let name = match take {
        Some(_) => tr!("timeline.stop_recording"),
        None => tr!("action.record"),
    };
    let name = transport_hint(ui.ctx(), keymap, "record", name);
    let b = ui.add(Button::icon("", &name)).on_hover_text(&name);
    crate::app::tutorial::anchor(ui.ctx(), "record", b.rect);
    let c = b.rect.center();
    let Some((elapsed, level)) = take else {
        let ink = if b.hovered() {
            color::ink()
        } else {
            color::ink_2()
        };
        ui.painter().circle_filled(c, 5.0, ink);
        return b.clicked();
    };
    let square = Rect::from_center_size(c, vec2(10.0, 10.0));
    ui.painter()
        .rect_filled(square, egui::CornerRadius::same(2), color::red());
    if !readout {
        return b.clicked();
    }
    let s = elapsed.as_secs();
    let text = egui::RichText::new(format!("{}:{:02}", s / 60, s % 60))
        .font(theme::body())
        .color(color::ink());
    ui.add(egui::Label::new(text).selectable(false));
    let (bar, _) = ui.allocate_exact_size(vec2(32.0, 4.0), egui::Sense::hover());
    crate::timeline::meter(ui.painter(), bar, level);
    b.clicked()
}

/// The video area of a viewer `ui`: its rect above the transport row.
pub fn viewer_rect(ui: &Ui) -> Rect {
    let rect = ui.max_rect();
    Rect::from_min_max(rect.min, rect.max - vec2(0.0, size::HEADER + SEEK_HEIGHT))
}

/// One UI pass coalesces pointer movement to its latest selected position.
#[derive(Default)]
struct TransportInput {
    action: Option<&'static str>,
    seek: Option<Time>,
}

const SEEK_HEIGHT: f32 = 16.0;

/// What the Source and Program tabs' transport row shows.
struct SourceRow {
    frame: i64,
    /// Duration in frames.
    end: i64,
    rate: jvid_core::time::Rate,
    playing: bool,
    in_point: Option<Time>,
    out_point: Option<Time>,
    duration: Time,
    position: Time,
}

impl SourceRow {
    /// The picture and transform handles end above this dedicated scrub hit area.
    fn marks(&self, ui: &Ui, hit: Rect) -> Option<Time> {
        if self.duration.0 <= 0 || hit.width() <= 0.0 {
            return None;
        }
        let response = ui.interact(
            hit,
            ui.id().with("preview_seek"),
            egui::Sense::click_and_drag(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::slider(
                true,
                self.position.0 as f64 / jvid_core::time::FLICKS_PER_SECOND as f64,
                tr!("preview.position"),
            )
        });
        let response = response.on_hover_text(tr!("preview.position"));
        let x = |t: Time| hit.left() + hit.width() * (t.0 as f64 / self.duration.0 as f64) as f32;
        let strip =
            Rect::from_x_y_ranges(hit.x_range(), hit.center().y - 1.5..=hit.center().y + 1.5);
        let painter = ui.painter();
        painter.rect_filled(strip, 0.0, color::border_strong());
        if self.in_point.is_some() || self.out_point.is_some() {
            let from = x(self.in_point.unwrap_or(Time(0)));
            let to = x(self.out_point.unwrap_or(self.duration));
            painter.rect_filled(
                Rect::from_x_y_ranges(from..=to, strip.y_range()),
                0.0,
                color::light_blue().gamma_multiply(0.5),
            );
        }
        let head = x(self.position);
        painter.rect_filled(
            Rect::from_x_y_ranges(head - 1.0..=head + 1.0, strip.y_range()),
            0.0,
            color::light_blue(),
        );
        let pointer = response
            .interact_pointer_pos()
            .filter(|_| response.clicked() || response.dragged());
        pointer.map(|p| {
            let fraction = ((p.x - hit.left()) as f64 / hit.width() as f64).clamp(0.0, 1.0);
            let time = Time((fraction * self.duration.0 as f64) as i64);
            let last = self
                .rate
                .index_floor(Time(self.duration.0.saturating_sub(1)))
                .max(0);
            self.rate
                .time_floor(self.rate.index_floor(time).clamp(0, last))
        })
    }
}

/// The GPU half, kept in egui-wgpu's callback resources.
struct Gpu {
    preview: render::Preview,
    /// The generation of the uploaded image.
    shown: Option<u64>,
    program: playback::Gpu,
    program_view: render::Preview,
    /// The program generation and preview scale last composited, and its compare split.
    program_drawn: Option<((u64, u32, bool), f32)>,
    thumbs: Thumbs,
    scopes: Scopes,
}

/// The Program tab's draw. Composites only when the frame or preview scale changed.
struct ProgramPaint {
    shown: Arc<Shown>,
    settings: ProjectSettings,
    generation: (u64, u32, bool),
    /// The frame's ungraded part, from [`Compare::before`].
    before: f32,
    clip: [f32; 4],
    /// Copy the frame into the thumbnail frame too.
    thumb: bool,
    /// Start a copy of the frame for the scopes too.
    scope: bool,
}

impl CallbackTrait for ProgramPaint {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(gpu) = resources.get_mut::<Gpu>() {
            if gpu.program_drawn != Some((self.generation, self.before)) {
                gpu.program_drawn = Some((self.generation, self.before));
                let scale = self.generation.1;
                gpu.program.before = self.before;
                let tex = gpu
                    .program
                    .draw(device, queue, &self.settings, scale, &self.shown);
                gpu.program_view.show_image(device, tex.clone());
            }
            if self.thumb
                && let Some(img) = gpu.program_view.image()
            {
                gpu.thumbs.set_frame(device, queue, img);
            }
            if self.scope
                && let Some(img) = gpu.program_view.image()
            {
                gpu.scopes.capture(device, queue, img);
            }
            gpu.program_view.set_dst(queue, self.clip);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        if let Some(gpu) = resources.get::<Gpu>() {
            gpu.program_view.paint(pass);
        }
    }
}

/// One frame's draw. Uploads (and converts) only when the frame or preview scale changed.
struct Paint {
    frame: Arc<Frame>,
    generation: u64,
    scale: u32,
    /// The image rect in the callback viewport's clip space.
    clip: [f32; 4],
}

impl CallbackTrait for Paint {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(gpu) = resources.get_mut::<Gpu>() {
            if gpu.shown != Some(self.generation) {
                gpu.shown = Some(self.generation);
                let f = &self.frame;
                let yuv = Yuv {
                    width: f.width,
                    height: f.height,
                    planes: [&f.planes[0], &f.planes[1], &f.planes[2]],
                    bt709: f.bt709,
                    full_range: f.full_range,
                    hdr: f.hdr,
                };
                gpu.preview.upload(device, queue, &yuv, self.scale);
            }
            gpu.preview.set_dst(queue, self.clip);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        if let Some(gpu) = resources.get::<Gpu>() {
            gpu.preview.paint(pass);
        }
    }
}

#[cfg(test)]
impl Preview {
    pub fn source_frame(&self) -> Option<i64> {
        self.player
            .as_ref()
            .map(|p| p.transport.frame(self.clock()))
    }

    pub fn source_speed(&self) -> Option<i32> {
        self.player.as_ref().map(|p| p.transport.speed())
    }

    pub fn program_speed(&self) -> i32 {
        self.program.transport.speed()
    }

    pub fn program_playing(&self) -> bool {
        self.program.transport.speed() != 0
    }

    /// The Program tab has a composited frame with at least one layer.
    pub fn has_program_frame(&self) -> bool {
        self.program_shown
            .as_ref()
            .is_some_and(|s| !s.layers.is_empty())
    }

    /// The decode for the current source or program frame has come back, with a frame or an error.
    pub fn settled(&self) -> bool {
        self.frame.is_some()
            || self.still.is_some()
            || self.error.is_some()
            || self.program_shown.is_some()
    }

    /// Plays through `out` instead of the audio device, so a test owns the clock.
    pub fn set_output(&mut self, out: Output) {
        self.output = Some(out);
    }

    /// The Program shows the exact frame at the playback clock.
    pub fn program_caught_up(&self) -> bool {
        let want = self.program.transport.frame(self.clock());
        self.program_shown
            .as_ref()
            .is_some_and(|s| s.index == want && s.exact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_shapes() {
        let image = Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 900.0));
        assert_eq!(Overlays::default().shapes(image), (vec![], vec![]));
        let o = Overlays {
            safe: true,
            thirds: true,
            center: true,
            aspects: [true, true],
        };
        let (rects, lines) = o.shapes(image);
        let c = image.center();
        assert_eq!(rects[0], Rect::from_center_size(c, vec2(1488.0, 837.0)));
        assert_eq!(rects[1], Rect::from_center_size(c, vec2(1440.0, 810.0)));
        // 9:16 and 1:1 inside 16:9 are full height.
        assert_eq!(
            rects[2],
            Rect::from_center_size(c, vec2(900.0 * 9.0 / 16.0, 900.0))
        );
        assert_eq!(rects[3], Rect::from_center_size(c, vec2(900.0, 900.0)));
        assert_eq!(lines.len(), 6);
        assert!((lines[0][0].x - 1600.0 / 3.0).abs() < 0.01);
        assert_eq!((lines[0][0].y, lines[0][1].y), (0.0, 900.0));
        // A guide wider than a portrait frame is full width instead.
        let tall = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 1600.0));
        let (rects, _) = o.shapes(tall);
        assert_eq!(
            rects[3],
            Rect::from_center_size(tall.center(), vec2(900.0, 900.0))
        );
    }
}

#[cfg(test)]
mod program_tests {
    use jvid_core::history::History;
    use jvid_core::model::Transform;
    use jvid_core::time::FLICKS_PER_SECOND;

    use super::*;
    use crate::playback::Pixels;
    use crate::playback::tests::add_solid;

    fn color(p: &Preview) -> Vec<[u8; 4]> {
        let shown = p.program_shown.as_ref().expect("shown");
        let solid = |px: &Pixels| match px {
            Pixels::Solid(c) => *c,
            _ => panic!("not a solid"),
        };
        shown.layers.iter().map(|(px, _)| solid(px)).collect()
    }

    #[test]
    fn parked_seeks_and_edits_redraw_without_playing() {
        let mut h = History::new(Project::default());
        let red = add_solid(&mut h, [1.0, 0.0, 0.0, 1.0], Time(0));
        add_solid(&mut h, [0.0, 0.0, 1.0, 1.0], Time(FLICKS_PER_SECOND));
        let mut p = Preview::new(None);
        assert_eq!(p.sync_program(h.project(), Time(0), false), None);
        assert_eq!(color(&p), [[255, 0, 0, 255]]);
        // Moving the playhead while paused shows the frame there and doesn't play.
        assert_eq!(
            p.sync_program(h.project(), Time(FLICKS_PER_SECOND * 3 / 2), false),
            None
        );
        assert_eq!(color(&p), [[0, 0, 255, 255]]);
        assert_eq!(p.program.transport.speed(), 0);
        // An edit under the parked playhead redraws it.
        let shown = p.program_gen;
        assert_eq!(p.sync_program(h.project(), Time(0), false), None);
        let half = Transform {
            opacity: 0.5,
            ..Transform::default()
        };
        h.apply(Command::SetClipProps(vec![(
            red,
            half.clone(),
            Default::default(),
        )]))
        .unwrap();
        let before = p.program_gen;
        assert!(before > shown);
        p.sync_program(h.project(), Time(0), false);
        assert_eq!(p.program_gen, before + 1);
        assert_eq!(p.program_shown.as_ref().unwrap().layers[0].1, half);
        // Nothing changed: nothing new to composite.
        p.sync_program(h.project(), Time(0), false);
        assert_eq!(p.program_gen, before + 1);
    }

    #[test]
    fn editing_a_titles_text_redraws_a_parked_playhead() {
        use jvid_core::history::Created;
        use jvid_core::model::{NewClip, Text, TrackKind};
        let mut h = History::new(Project::default());
        let Ok(Created::Asset(id)) = h.apply(Command::AddText {
            name: "Title".into(),
            text: Text::default(),
        }) else {
            panic!("text")
        };
        let Ok(Created::Track(track)) = h.apply(Command::AddTrack {
            kind: TrackKind::Video,
            name: "V1".into(),
        }) else {
            panic!("track")
        };
        let clip = NewClip {
            asset: id,
            source_in: Time(0),
            source_out: Time(FLICKS_PER_SECOND),
            start: Time(0),
            link: None,
        };
        h.apply(Command::AddClip { track, clip }).unwrap();
        let mut p = Preview::new(None);
        p.sync_program(h.project(), Time(0), false);
        let before = p.program_gen;
        let text = Text {
            content: "Edited".into(),
            ..Text::default()
        };
        h.apply(Command::SetText { id, text }).unwrap();
        p.sync_program(h.project(), Time(0), false);
        assert_eq!(p.program_gen, before + 1, "a text edit recomposites");
    }

    #[test]
    fn thumbnails_wait_for_playback_and_scrubbing_to_stop() {
        let mut h = History::new(Project::default());
        let red = add_solid(&mut h, [1.0, 0.0, 0.0, 1.0], Time(0));
        // A gap from 1 s to 2 s.
        add_solid(&mut h, [0.0, 0.0, 1.0, 1.0], Time(FLICKS_PER_SECOND * 2));
        let mut p = Preview::new(None);
        // Settled on a frame: one copy, then nothing until something changes.
        p.sync_program(h.project(), Time(0), false);
        assert!(p.update_thumb(false, false));
        assert_eq!((p.thumb.from, p.thumb.copy), (Some(p.program_gen), true));
        p.thumb.copy = false;
        assert!(!p.update_thumb(false, false));
        // The Effects tab is open; playing or scrubbing through new frames does no thumbnail work.
        p.refresh_thumb();
        p.program.transport.toggle(0);
        let half = Transform {
            opacity: 0.5,
            ..Transform::default()
        };
        let props = Command::SetClipProps(vec![(red, half, Default::default())]);
        h.apply(props).unwrap();
        p.sync_program(h.project(), Time(0), false);
        assert!(!p.update_thumb(false, false));
        p.program.transport.stop(0);
        p.sync_program(h.project(), Time(FLICKS_PER_SECOND / 2), true);
        assert!(!p.update_thumb(false, true));
        assert!(!p.thumb.copy);
        // Released: the frame it settled on.
        assert!(p.update_thumb(false, false));
        assert_eq!((p.thumb.from, p.thumb.copy), (Some(p.program_gen), true));
        // Reopening the tab refreshes even an unchanged frame.
        p.refresh_thumb();
        assert!(p.update_thumb(false, false));
        // Nothing at the playhead, or nothing on the timeline: the fallback.
        p.sync_program(h.project(), Time(FLICKS_PER_SECOND * 3 / 2), false);
        assert!(p.update_thumb(false, false));
        assert_eq!(p.thumb.from, None);
        assert!(!p.update_thumb(true, false), "already the fallback");
    }

    /// The Program tab of `history`, at its start, in an app with wgpu.
    pub(super) struct Program {
        pub(super) preview: Preview,
        pub(super) history: History,
        pub(super) fresh: bool,
    }

    impl eframe::App for Program {
        fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
            if suite_ui::test_support::install_theme(ui, &mut self.fresh) {
                return;
            }
            let project = self.history.project();
            self.preview
                .show_program(ui, project, Time(0), None, false, &Keymap::default());
        }
    }

    #[test]
    fn the_thumbnail_frame_is_the_fallback_then_the_program_frame() {
        use jvid_render::composite::read_back;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = egui_kittest::Harness::builder()
            .with_size([640.0, 360.0])
            .wgpu()
            .build_eframe(|cc| Program {
                preview: Preview::new(cc.wgpu_render_state.as_ref()),
                history: History::new(Project::default()),
                fresh: true,
            });
        let frame = |h: &egui_kittest::Harness<'_, Program>| {
            h.state()
                .preview
                .with_thumbs(|t, d, q| read_back(d, q, t.frame()))
                .unwrap()
        };
        h.run();
        h.render().unwrap();
        let (d, q) = h.state().preview.gpu().unwrap();
        let fallback = read_back(&d, &q, Thumbs::new(&d, &q).frame());
        assert_eq!(frame(&h), fallback, "an empty project shows the fallback");
        let (id, g) = h.state().preview.thumb_frame().expect("an egui texture");
        add_solid(&mut h.state_mut().history, [1.0, 0.0, 0.0, 1.0], Time(0));
        // Drawn frame by frame, as the app does: the copy happens as a frame draws, and the
        // repaint asked for then shows it in the thumbnails drawn on it.
        for _ in 0..3 {
            h.step();
            h.render().unwrap();
        }
        assert!(frame(&h).chunks(4).all(|p| p == [255, 0, 0, 255]));
        assert_eq!(h.state().preview.thumb_frame(), Some((id, g + 1)));
        // A drawn thumbnail gets its own egui texture, kept on redraw.
        let effect = jvid_core::model::Effect {
            shader: "none".into(),
            params: Default::default(),
            enabled: true,
            mask: None,
        };
        let draw = |id| {
            h.state()
                .preview
                .draw_thumb(id, |t, d, q| t.effect(d, q, &effect))
        };
        let mine = draw(None).unwrap();
        assert_ne!(mine, id);
        assert_eq!(draw(Some(mine)), Some(mine));
    }

    /// Each item's ink in the transport row, left to right, as (left x, vertical centre, top,
    /// bottom) in physical pixels, against the row's centre. Ink is anything not the row's
    /// background; items are runs of inked columns more than 3 points apart.
    fn transport_ink(
        size: [f32; 2],
        ppp: f32,
        recording: Option<(std::time::Duration, f32)>,
    ) -> (f32, Vec<(u32, f32, u32, u32)>) {
        let mut h = egui_kittest::Harness::builder()
            .with_size(size)
            .with_pixels_per_point(ppp)
            .wgpu()
            .build_eframe(|cc| Program {
                preview: Preview::new(cc.wgpu_render_state.as_ref()),
                history: History::new(Project::default()),
                fresh: true,
            });
        // A clip, so the row has its timecodes and buttons.
        add_solid(&mut h.state_mut().history, [0.0, 0.0, 0.0, 1.0], Time(0));
        h.state_mut().preview.recording = recording;
        h.run();
        let img = h.render().unwrap();
        let px = |pt: f32| (pt * ppp).round() as u32;
        let (top, bottom) = (px(size[1] - size::HEADER), px(size[1]));
        let bg = *img.get_pixel(0, bottom - 1);
        let ink = |x, y| {
            let p = img.get_pixel(x, y);
            (0..3).any(|i| p[i].abs_diff(bg[i]) > 8)
        };
        let gap = px(3.0);
        let mut items: Vec<(u32, u32, u32, u32)> = Vec::new();
        for x in 0..img.width() {
            let ys: Vec<u32> = (top..bottom).filter(|&y| ink(x, y)).collect();
            let (Some(&t), Some(&b)) = (ys.first(), ys.last()) else {
                continue;
            };
            match items.last_mut() {
                Some(i) if x - i.1 <= gap => *i = (i.0, x, i.2.min(t), i.3.max(b)),
                _ => items.push((x, x, t, b)),
            }
        }
        let items = items
            .into_iter()
            .map(|(l, _, t, b)| (l, (t + b + 1) as f32 / 2.0, t, b))
            .collect();
        ((top + bottom) as f32 / 2.0, items)
    }

    /// DESIGN.md Layout > Checks, for the Program transport row: every item's ink (timecodes,
    /// buttons, Record, the end time, the dropdowns, the Preview checkbox, the guides menu) is centred
    /// within 1 physical pixel of the row's centre, at 100% and 150% scale.
    #[test]
    fn transport_items_share_the_rows_centre_line() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        for ppp in [1.0, 1.5] {
            let (centre, items) = transport_ink([960.0, 360.0], ppp, None);
            // Timecode, 5 buttons, Record, end time, guides, checkbox box and label, 2
            // dropdowns.
            assert_eq!(items.len(), 13, "@{ppp}x: {items:?}");
            // While recording: Stop, its elapsed time and meter join the row.
            let take = Some((std::time::Duration::from_secs(72), 0.5));
            let (_, recording) = transport_ink([960.0, 360.0], ppp, take);
            assert_eq!(recording.len(), 15, "@{ppp}x: {recording:?}");
            for i in items.iter().chain(&recording) {
                assert!(
                    (i.1 - centre).abs() <= 1.0,
                    "@{ppp}x: {i:?} off the centre {centre}, all {items:?}"
                );
            }
        }
    }

    #[test]
    fn open_scopes_copy_the_program_frame_as_it_draws() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = egui_kittest::Harness::builder()
            .with_size([640.0, 360.0])
            .wgpu()
            .build_eframe(|cc| Program {
                preview: Preview::new(cc.wgpu_render_state.as_ref()),
                history: History::new(Project::default()),
                fresh: true,
            });
        let textures = |h: &egui_kittest::Harness<'_, Program>| {
            h.ctx
                .data(|d| d.get_temp::<[egui::TextureHandle; 3]>(scopes::id()))
        };
        add_solid(&mut h.state_mut().history, [1.0, 0.0, 0.0, 1.0], Time(0));
        for _ in 0..3 {
            h.step();
            h.render().unwrap();
        }
        assert!(textures(&h).is_none(), "closed: nothing copied");
        h.state_mut().preview.scopes_open(true);
        // The copy starts as a frame draws and is picked up, without waiting, a frame or two on.
        for _ in 0..4 {
            h.step();
            h.render().unwrap();
        }
        let [wave, vector, hist] = textures(&h).expect("scopes drawn");
        assert_eq!(
            wave.size(),
            [256, 256],
            "a 256-wide copy of the 1080p frame"
        );
        assert_eq!(vector.size(), [jvid_render::scope::VECTOR; 2]);
        assert_eq!(hist.size()[0], 256);
        let busy = h.state().preview.with_scopes(|s, _| s.busy());
        assert_eq!(
            busy,
            Some(false),
            "nothing left in flight on a parked frame"
        );
    }

    #[test]
    fn a_title_opens_in_the_source_tab_without_decoding() {
        use jvid_core::history::Created;
        use jvid_core::model::Text;
        let mut h = History::new(Project::default());
        let Ok(Created::Asset(id)) = h.apply(Command::AddText {
            name: "Title".into(),
            text: Text::default(),
        }) else {
            panic!("text")
        };
        let mut p = Preview::new(None);
        p.open(h.project(), id);
        let project = h.project().clone();
        let mut fresh = true;
        let mut harness = egui_kittest::Harness::builder()
            .with_size([640.0, 360.0])
            .build_ui_state(
                move |ui, p: &mut Preview| {
                    if suite_ui::test_support::install_theme(ui, &mut fresh) {
                        return;
                    }
                    p.show_source(ui, &project, &HashSet::new(), &Keymap::default());
                },
                p,
            );
        harness.run();
        let p = harness.state();
        assert_eq!((&p.error, p.pending), (&None, false), "nothing decoded");
        let (asset, shown) = p.source_shown.as_ref().expect("drawn");
        assert_eq!(asset.id, id);
        assert!(matches!(shown.layers[..], [(Pixels::Text(_), _)]));
    }

    #[test]
    fn an_imported_still_uses_the_same_rgba_as_its_poster_and_program() {
        use jvid_core::history::Created;
        let dir = std::env::temp_dir().join(format!("jvid-b07-source-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("source.png");
        let original = image::RgbaImage::from_fn(12, 8, |x, y| {
            image::Rgba([
                x as u8 * 20,
                y as u8 * 30,
                91,
                [0, 128, 255][x as usize % 3],
            ])
        });
        original.save(&path).unwrap();
        let mut h = History::new(Project::default());
        let Ok(Created::Asset(id)) = h.apply(Command::AddAsset(path.clone())) else {
            panic!("asset")
        };
        let mut preview = Preview::new(None);
        preview.open(h.project(), id);
        let project = h.project().clone();
        let mut fresh = true;
        let mut harness = egui_kittest::Harness::builder()
            .with_size([640.0, 360.0])
            .build_ui_state(
                move |ui, p: &mut Preview| {
                    if suite_ui::test_support::install_theme(ui, &mut fresh) {
                        return;
                    }
                    p.show_source(ui, &project, &HashSet::new(), &Keymap::default());
                },
                preview,
            );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while harness.state().still.is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "Source decode timed out"
            );
            harness.run();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let source = harness.state().still.as_ref().unwrap().clone();
        assert_eq!(*source, original);
        assert!(Arc::ptr_eq(
            &source,
            &jvid_media::stills::cached(&path).unwrap()
        ));
        let posters = jvid_media::thumbnails::Cache::new(dir.join("posters"), 1 << 20);
        let poster = posters.poster(&path, Time(0)).unwrap();
        assert_eq!(poster, image::imageops::thumbnail(&original, 160, 106));
        assert!(
            harness.state().frame.is_none(),
            "no opaque video conversion"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The Source tab with `probe` imported at a path that doesn't exist, scrubbed across 20
    /// frames, each left to settle. Returns the preview and every log line written.
    fn scrub_source(probe: jvid_core::model::ProbeInfo) -> (Preview, String) {
        use std::sync::{Arc, Mutex};
        #[derive(Clone, Default)]
        struct Log(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Log {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let log = Log::default();
        let writer = log.clone();
        let sub = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .finish();
        let mut h = History::new(Project::default());
        h.apply(Command::Import(vec![(
            "/nonexistent/Voiceover 2.wav".into(),
            probe,
        )]))
        .unwrap();
        let mut p = Preview::new(None);
        p.open(h.project(), h.project().assets()[0].id);
        let project = h.project().clone();
        let mut fresh = true;
        // Building the harness runs frames too, so it's inside the capture.
        let p = tracing::subscriber::with_default(sub, || {
            let mut harness = egui_kittest::Harness::builder()
                .with_size([640.0, 360.0])
                .build_ui_state(
                    move |ui, p: &mut Preview| {
                        if suite_ui::test_support::install_theme(ui, &mut fresh) {
                            return;
                        }
                        p.show_source(ui, &project, &HashSet::new(), &Keymap::default());
                    },
                    p,
                );
            for i in 0..20 {
                let t = Time(i * FLICKS_PER_SECOND / 10);
                let player = harness.state_mut().player.as_mut().unwrap();
                player.transport.seek(t, 0);
                harness.run();
                for _ in 0..500 {
                    if !harness.state().pending {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    harness.run();
                }
            }
            harness.into_state()
        });
        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        (p, text)
    }

    #[test]
    fn an_audio_only_source_never_asks_for_a_video_frame() {
        let (p, log) = scrub_source(jvid_core::model::ProbeInfo {
            duration: Time(3 * FLICKS_PER_SECOND),
            audio_streams: 1,
            ..Default::default()
        });
        assert_eq!(
            (p.requested, p.pending),
            (None, false),
            "no decode submitted"
        );
        assert!(
            p.frame.is_none() && p.error.is_none(),
            "no frame and no error text"
        );
        assert!(!log.contains("preview decode"), "{log}");
    }

    #[test]
    fn a_failing_video_source_warns_once() {
        let (p, log) = scrub_source(jvid_core::model::ProbeInfo {
            duration: Time(3 * FLICKS_PER_SECOND),
            width: Some(1920),
            height: Some(1080),
            ..Default::default()
        });
        assert!(p.error.is_some(), "the decode failed");
        assert!(
            p.requested.is_some_and(|t| t.0 > 0),
            "it kept asking as it scrubbed"
        );
        assert_eq!(log.matches("preview decode of").count(), 1, "{log}");
    }
}

#[cfg(test)]
mod frame_export_tests {
    use super::*;
    use jvid_core::history::{Created, History};
    use jvid_core::model::{NewClip, Text, TrackKind};
    use jvid_core::time::FLICKS_PER_SECOND;

    #[test]
    fn export_frame_renders_a_title_without_ever_opening_source() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut harness = egui_kittest::Harness::builder()
            .with_size([640.0, 360.0])
            .wgpu()
            .build_eframe(|cc| super::program_tests::Program {
                preview: Preview::new(cc.wgpu_render_state.as_ref()),
                history: History::new(Project::default()),
                fresh: true,
            });
        let h = &mut harness.state_mut().history;
        h.apply(Command::SetSettings(ProjectSettings {
            width: 96,
            height: 64,
            ..Default::default()
        }))
        .unwrap();
        let Ok(Created::Asset(asset)) = h.apply(Command::AddText {
            name: "Title".into(),
            text: Text {
                content: "Title".into(),
                size: 24.0,
                ..Default::default()
            },
        }) else {
            panic!("title")
        };
        let Ok(Created::Track(track)) = h.apply(Command::AddTrack {
            kind: TrackKind::Video,
            name: "V1".into(),
        }) else {
            panic!("track")
        };
        h.apply(Command::AddClip {
            track,
            clip: NewClip {
                asset,
                start: Time(0),
                source_in: Time(0),
                source_out: Time(FLICKS_PER_SECOND),
                link: None,
            },
        })
        .unwrap();
        let state = harness.state();
        assert!(state.preview.player.is_none());
        let path =
            std::env::temp_dir().join(format!("jvid-b07-title-frame-{}.png", std::process::id()));
        state
            .preview
            .export_frame(state.history.project(), Time(0), &path)
            .unwrap();
        let png = image::open(&path).unwrap().into_rgba8();
        assert_eq!(png.dimensions(), (96, 64));
        assert!(png.pixels().any(|p| p[0] > 100), "the title is visible");
        let h = &mut harness.state_mut().history;
        let Ok(Created::Asset(source)) = h.apply(Command::AddSolid {
            name: "Different Source".into(),
            color: [0.0, 0.0, 1.0, 1.0],
        }) else {
            panic!("source")
        };
        let project = h.project().clone();
        let preview = &mut harness.state_mut().preview;
        preview.open(&project, source);
        let expected = png.clone();
        for quality in 0..3 {
            preview.quality = quality;
            for proxy in [false, true] {
                preview.proxies.set_enabled(proxy);
                preview.export_frame(&project, Time(0), &path).unwrap();
                assert_eq!(
                    image::open(&path).unwrap().into_rgba8(),
                    expected,
                    "Source selection, quality {quality}, proxy {proxy}"
                );
            }
        }
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod audit_transport_tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    struct RowState {
        preview: Preview,
        bounds: Rect,
        program: bool,
        playing: bool,
        keymap: crate::keymap::Keymap,
    }

    fn row_harness(
        width: f32,
        scale: f32,
        program: bool,
        playing: bool,
        take: Option<(std::time::Duration, f32)>,
    ) -> Harness<'static, RowState> {
        let mut preview = Preview::new(None);
        preview.recording = take;
        let mut fresh = true;
        let mut h = Harness::builder()
            .with_size([width, 300.0])
            .with_pixels_per_point(scale)
            .build_ui_state(
                move |ui, s: &mut RowState| {
                    if suite_ui::test_support::install_theme(ui, &mut fresh) {
                        return;
                    }
                    s.bounds = ui.max_rect();
                    let rate = jvid_core::time::Rate::new(30, 1);
                    let row = SourceRow {
                        frame: 12345,
                        end: 54000,
                        rate,
                        playing: s.playing,
                        in_point: None,
                        out_point: None,
                        duration: Time(1800 * jvid_core::time::FLICKS_PER_SECOND),
                        position: rate.time_floor(12345),
                    };
                    s.preview.show(
                        ui,
                        None,
                        false,
                        Some(&row),
                        Some((&ProjectSettings::default(), !s.program)),
                        &s.keymap,
                    );
                },
                RowState {
                    preview,
                    bounds: Rect::NOTHING,
                    program,
                    playing,
                    keymap: crate::keymap::Keymap::default(),
                },
            );
        h.run_steps(4);
        h
    }

    #[test]
    fn audit_454_transport_hit_bounds_fit_without_intersections() {
        for width in [240.0, 480.0, 640.0, 800.0] {
            for scale in [1.0, 1.5] {
                for program in [false, true] {
                    for (playing, take) in [
                        (false, None),
                        (true, None),
                        (
                            true,
                            Some((std::time::Duration::from_secs(99_999_999), 0.75)),
                        ),
                    ] {
                        let h = row_harness(width, scale, program, playing, take);
                        let mut controls = Vec::new();
                        for role in [
                            egui::accesskit::Role::Button,
                            egui::accesskit::Role::ComboBox,
                            egui::accesskit::Role::CheckBox,
                            egui::accesskit::Role::Slider,
                        ] {
                            controls.extend(h.query_all_by_role(role).map(|n| n.rect()));
                        }
                        for (i, a) in controls.iter().enumerate() {
                            assert!(
                                h.state().bounds.expand(0.01).contains_rect(*a),
                                "{width}@{scale} program={program} take={take:?}: {a:?} outside {:?}",
                                h.state().bounds
                            );
                            for b in &controls[i + 1..] {
                                let overlap = a.intersect(*b);
                                assert!(
                                    overlap.width() <= 0.01 || overlap.height() <= 0.01,
                                    "{width}@{scale} program={program} take={take:?}: {a:?} overlaps {b:?}"
                                );
                            }
                        }
                        let play = tr!(
                            "preview.with_key",
                            action = if playing {
                                tr!("preview.stop")
                            } else {
                                tr!("preview.play")
                            },
                            key = verbatim("Space")
                        );
                        assert!(h.state().bounds.contains_rect(h.get_by_label(&play).rect()));
                        if program && let Some((elapsed, _)) = take {
                            let seconds = elapsed.as_secs();
                            let label = h
                                .get_by_label(&format!("{}:{:02}", seconds / 60, seconds % 60))
                                .rect();
                            assert!(h.state().bounds.contains_rect(label), "elapsed: {label:?}");
                            for control in &controls {
                                let overlap = label.intersect(*control);
                                assert!(
                                    overlap.width() <= 0.01 || overlap.height() <= 0.01,
                                    "elapsed overlaps {control:?}"
                                );
                            }
                        }
                        if program {
                            let record = tr!(
                                "preview.with_key",
                                action = if take.is_some() {
                                    tr!("timeline.stop_recording")
                                } else {
                                    tr!("action.record")
                                },
                                key = verbatim("Shift+R")
                            );
                            assert!(
                                h.state()
                                    .bounds
                                    .contains_rect(h.get_by_label(&record).rect())
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn audit_454_compact_overflow_and_record_controls_remain_clickable() {
        for width in [240.0, 640.0] {
            for take in [
                None,
                Some((std::time::Duration::from_secs(99_999_999), 0.8)),
            ] {
                let mut h = row_harness(width, 1.0, true, take.is_some(), take);
                let name = tr!(
                    "preview.with_key",
                    action = if take.is_some() {
                        tr!("timeline.stop_recording")
                    } else {
                        tr!("action.record")
                    },
                    key = verbatim("Shift+R")
                );
                h.get_by_label(&name).click();
                h.run_steps(2);
                assert!(h.state().preview.record_clicked);
                if let Some(more) = h.query_by_label(tr!("preview.more")) {
                    more.click();
                    h.run_steps(2);
                } else {
                    assert_eq!(width, 640.0, "the minimum width must use overflow");
                }
                h.get_by_label(tr!("preview.proxies"));
                h.get_by_label(tr!("preview.full"));
                h.get_by_label(tr!("preview.fit"));
                h.get_by_label(tr!("preview.guides"));
            }
        }
    }

    #[test]
    fn audit_463_source_seek_flushes_audio_and_coalesces_pending_decode() {
        use jvid_core::{history::History, model::ProbeInfo, time::FLICKS_PER_SECOND as S};
        use std::sync::Mutex;
        #[derive(Clone, Default)]
        struct Backend(Arc<Mutex<Option<Arc<jvid_audio::Feed>>>>);
        impl jvid_audio::Backend for Backend {
            fn devices(&self) -> Vec<String> {
                vec!["test".into()]
            }
            fn open(
                &mut self,
                _: Option<&str>,
                feed: Arc<jvid_audio::Feed>,
            ) -> Result<(String, Box<dyn std::any::Any>), String> {
                *self.0.lock().unwrap() = Some(feed);
                Ok(("test".into(), Box::new(())))
            }
        }
        let backend = Backend::default();
        let mut history = History::new(Project::default());
        let path = PathBuf::from("/nonexistent/seek-coalescing.mp4");
        history
            .apply(Command::Import(vec![(
                path.clone(),
                ProbeInfo {
                    duration: Time(1800 * S),
                    width: Some(1920),
                    height: Some(1080),
                    frame_rate: Some(jvid_core::time::Rate::new(30, 1)),
                    ..Default::default()
                },
            )]))
            .unwrap();
        let mut preview = Preview::new(None);
        preview.open(history.project(), history.project().assets()[0].id);
        preview.set_output(Output::new(Box::new(backend.clone()), None));
        assert_eq!(preview.output.as_mut().unwrap().push(&[0.5; 64]), 64);
        preview.source = Some(path.clone());
        preview.pending = true;
        preview.requested = Some(Time(0));
        for seconds in [200, 400, 800, 900] {
            preview.seek_source(Time(seconds * S));
            assert!(preview.pending);
            assert!(
                preview.source_job.is_none(),
                "no extra decode queued while one is in flight"
            );
        }
        let mut output = [1.0; 64];
        backend
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .fill(&mut output, 2);
        assert_eq!(output, [0.0; 64], "old audio was flushed");
        preview
            .tx
            .send((path, Time(0), Err("stale decode".into())))
            .unwrap();
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut frame_output = ctx.run_ui(Default::default(), |ui| {
            preview.show_source(ui, history.project(), &HashSet::new(), &Keymap::default());
        });
        frame_output.textures_delta.clear();
        assert!(!preview.pending);
        assert!(
            preview.error.is_none(),
            "discard stale replies after seeking"
        );
        assert_eq!(preview.source_frame(), Some(27000));
        let mut frame_output = ctx.run_ui(Default::default(), |ui| {
            preview.show_source(ui, history.project(), &HashSet::new(), &Keymap::default());
        });
        frame_output.textures_delta.clear();
        assert_eq!(
            preview.requested,
            Some(Time(900 * S)),
            "decode the latest position next"
        );
    }

    #[test]
    fn audit_471_live_hints_follow_custom_and_unbound_keys_in_both_monitors() {
        for program in [false, true] {
            let mut h = row_harness(800.0, 1.0, program, false, None);
            h.state_mut().keymap =
                crate::keymap::Keymap::from_toml("play_stop = \"P\"\nrecord = \"\"").unwrap();
            h.run_steps(2);
            let hint = tr!("preview.with_key", action = tr!("preview.play"), key = "P");
            let button = h.get_by_role_and_label(egui::accesskit::Role::Button, &hint);
            button.hover();
            h.ctx
                .global_style_mut(|s| s.interaction.tooltip_delay = 0.0);
            h.run_steps(3);
            assert!(
                h.query_all_by_label(&hint).count() >= 2,
                "tooltip and accessible button agree"
            );
            assert!(
                h.query_by_label(&tr!(
                    "preview.with_key",
                    action = tr!("preview.play"),
                    key = verbatim("Space")
                ))
                .is_none()
            );
            if program {
                h.get_by_role_and_label(egui::accesskit::Role::Button, tr!("action.record"));
                assert!(
                    h.query_by_label(&tr!(
                        "preview.with_key",
                        action = tr!("action.record"),
                        key = verbatim("Shift+R")
                    ))
                    .is_none()
                );
            }
            h.state_mut().keymap =
                Keymap::from_toml("play_stop = \"\"\nrecord = \"Ctrl+Alt+R\"").unwrap();
            h.run_steps(2);
            h.get_by_role_and_label(egui::accesskit::Role::Button, tr!("preview.play"));
            if program {
                let key = h
                    .ctx
                    .format_shortcut(&crate::keymap::parse("Ctrl+Alt+R").unwrap());
                h.get_by_role_and_label(
                    egui::accesskit::Role::Button,
                    &tr!("preview.with_key", action = tr!("action.record"), key = key),
                );
            }
            h.state_mut().keymap = crate::keymap::Keymap::default();
            h.run_steps(2);
            h.get_by_role_and_label(
                egui::accesskit::Role::Button,
                &tr!(
                    "preview.with_key",
                    action = tr!("preview.play"),
                    key = verbatim("Space")
                ),
            );
        }
    }

    #[test]
    fn audit_471_hint_formats_command_keys_for_the_current_platform() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |_| {});
        output.textures_delta.clear();
        let keymap = Keymap::from_toml("play_stop = \"Ctrl+P\"").unwrap();
        ctx.set_os(egui::os::OperatingSystem::Nix);
        assert!(transport_hint(&ctx, &keymap, "play_stop", tr!("preview.play")).contains("Ctrl"));
        ctx.set_os(egui::os::OperatingSystem::Mac);
        let mac = transport_hint(&ctx, &keymap, "play_stop", tr!("preview.play"));
        assert!(mac.contains("Cmd") || mac.contains('⌘'), "{mac}");
        assert!(!mac.contains("Ctrl"));
    }

    #[test]
    #[ignore = "changes global locale; run this test in isolation"]
    fn audit_471_hints_follow_language_switch_without_restart() {
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                suite_ui::i18n::set_language("en");
            }
        }
        let _restore = Restore;
        let mut h = row_harness(800.0, 1.0, true, false, None);
        h.state_mut().keymap = Keymap::from_toml("play_stop = \"P\"\nrecord = \"\"").unwrap();
        for language in ["en", "de", "fr"] {
            suite_ui::i18n::set_language(language);
            for program in [false, true] {
                h.state_mut().program = program;
                h.run_steps(2);
                h.get_by_role_and_label(
                    egui::accesskit::Role::Button,
                    &tr!("preview.with_key", action = tr!("preview.play"), key = "P"),
                );
                if program {
                    h.get_by_role_and_label(egui::accesskit::Role::Button, tr!("action.record"));
                }
            }
        }
    }

    // Locale changes are process-global; explicitly run in isolation alongside #472's test.
    #[test]
    #[ignore = "changes global locale; run this test in isolation"]
    fn audit_454_translated_transport_hit_bounds() {
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                suite_ui::i18n::set_pseudo(false);
                suite_ui::i18n::set_language("en");
            }
        }
        let _restore = Restore;
        for language in ["de", "fr", "pt-BR", "en"] {
            suite_ui::i18n::set_language(language);
            suite_ui::i18n::set_pseudo(language == "en");
            audit_454_transport_hit_bounds_fit_without_intersections();
        }
    }
}
