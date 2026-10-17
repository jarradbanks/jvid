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
use jvid_core::model::{AssetId, Project, ProjectSettings, Sequence};
use jvid_core::settings::PreviewQuality;
use jvid_core::time::{Time, format_timecode};
use jvid_jobs::Priority;
use jvid_media::decode::{DecoderPool, Frame};
use jvid_render::preview::{self as render, Yuv};
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{Button, Checkbox, Dropdown};

use crate::playback::{self, Playback, Shown};
use crate::transport::{AudioSource, Transport};

const ZOOMS: [(&str, Option<f32>); 4] = [
    ("Fit", None),
    ("50%", Some(0.5)),
    ("100%", Some(1.0)),
    ("200%", Some(2.0)),
];
const QUALITIES: [(&str, PreviewQuality); 3] = [
    ("Full", PreviewQuality::Full),
    ("½", PreviewQuality::Half),
    ("¼", PreviewQuality::Quarter),
];
/// Action-safe and title-safe as fractions of the frame (SMPTE ST 2046-1).
const SAFE: [f32; 2] = [0.93, 0.9];
/// Aspect guides, width over height.
const ASPECTS: [(&str, f32); 2] = [("9:16 guide", 9.0 / 16.0), ("1:1 guide", 1.0)];

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

type Decoded = (PathBuf, Time, Result<Frame, String>);

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

/// The clip open in the Source tab.
struct Player {
    asset: AssetId,
    path: PathBuf,
    transport: Transport,
    audio: AudioSource,
}

pub struct Preview {
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
    frame: Option<Arc<Frame>>,
    error: Option<String>,
    /// Bumped when `frame` or the preview scale changes; the GPU side re-uploads when it sees a new one.
    generation: u64,
    zoom: usize,
    quality: usize,
    overlays: Overlays,
    player: Option<Player>,
    /// Opened on first play, so tests and idle sessions never touch the audio device.
    output: Option<Output>,
    /// The timeline's playback, shown in the Program tab.
    program: Playback,
    /// The composited frame last returned, and a count bumped with it.
    program_shown: Option<Arc<Shown>>,
    program_gen: u64,
    /// The timeline playhead last frame; a change is a seek.
    program_head: Time,
    /// The sequence last frame; a change redraws a parked playhead.
    program_seq: Option<Sequence>,
    /// For full-size renders outside the paint callback, such as frame export.
    render_state: Option<egui_wgpu::RenderState>,
}

impl Preview {
    /// Puts the GPU half in egui-wgpu's callback resources; without wgpu the viewer stays empty.
    pub fn new(render_state: Option<&egui_wgpu::RenderState>) -> Self {
        if let Some(rs) = render_state {
            let preview = render::Preview::new(&rs.device, rs.target_format);
            let gpu = Gpu {
                preview,
                shown: None,
                program: playback::Gpu::new(&rs.device),
                program_view: render::Preview::new(&rs.device, rs.target_format),
                program_drawn: None,
            };
            rs.renderer.write().callback_resources.insert(gpu);
        }
        let (tx, rx) = channel();
        Self {
            pool: Arc::default(),
            jobs: jvid_jobs::Pool::new(1),
            tx,
            rx,
            source: None,
            requested: None,
            pending: false,
            frame: None,
            error: None,
            generation: 0,
            zoom: 0,
            quality: 0,
            overlays: Overlays::default(),
            player: None,
            output: None,
            program: Playback::from_files(playback::DEFAULT_BUDGET),
            program_shown: None,
            program_gen: 0,
            program_head: Time(0),
            program_seq: None,
            render_state: render_state.cloned(),
        }
    }

    /// The device and queue for offscreen rendering, such as export; `None` without wgpu.
    pub fn gpu(&self) -> Option<(egui_wgpu::wgpu::Device, egui_wgpu::wgpu::Queue)> {
        self.render_state
            .as_ref()
            .map(|rs| (rs.device.clone(), rs.queue.clone()))
    }

    /// Writes the shown frame as a PNG, composited at full project size over its background.
    /// shortcut: one layer, the frame the viewer shows; composite `video_at` at the playhead
    /// once the timeline (#33) and playback (#32) land.
    pub fn export_frame(&self, settings: &ProjectSettings, path: &Path) -> Result<(), String> {
        let (Some(rs), Some(f)) = (&self.render_state, &self.frame) else {
            return Err("there's no frame to export".into());
        };
        let mut rgb = render::Preview::new(&rs.device, rs.target_format);
        let yuv = Yuv {
            width: f.width,
            height: f.height,
            planes: [&f.planes[0], &f.planes[1], &f.planes[2]],
            bt709: f.bt709,
            full_range: f.full_range,
        };
        rgb.upload(&rs.device, &rs.queue, &yuv, 1);
        let image = rgb.image().expect("uploaded");
        jvid_render::composite::export_png(
            &rs.device,
            &rs.queue,
            (settings.width, settings.height),
            settings.background,
            &[(image, Default::default())],
            path,
        )
        .map_err(|e| e.to_string())
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
        self.player = Some(Player {
            asset: id,
            path: a.path.clone(),
            transport: Transport::new(probe.duration, rate),
            audio: AudioSource::open(&a.path, probe.audio_streams > 0),
        });
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
            self.show(ui, None, false, None, None);
            ui.painter().text(
                ui.max_rect().center(),
                Align2::CENTER_CENTER,
                "Double-click a clip in the bin to open it here",
                theme::body(),
                color::INK_3,
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
        let path = p.path.clone();
        let offline = missing.contains(&path);
        let action = self.show(ui, Some((&path, row.position)), offline, Some(&row), None)?;
        self.source_action(action, project)
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
    /// dragged clip edge) instead. Returns where the playhead moved to while playing.
    pub fn show_program(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        playhead: Time,
        trim: Option<(&Path, Time)>,
        offline: bool,
    ) -> Option<Time> {
        if let Some(p) = &mut self.player
            && p.transport.speed() != 0
        {
            p.transport
                .stop(self.output.as_ref().map_or(0, Output::clock));
        }
        // An empty sequence leaves the viewer empty, like a Source tab with nothing open.
        if trim.is_some() || project.sequence().duration() == Time(0) {
            self.show(ui, trim, offline, None, None);
            return None;
        }
        let dragging = ui.input(|i| i.pointer.primary_down());
        let follow = self.sync_program(project, playhead, dragging);
        if self.program.transport.speed() != 0 {
            ui.ctx().request_repaint();
        }
        self.show(ui, None, false, None, Some(project.settings()));
        follow
    }

    /// Seeks timeline playback to a moved `playhead` and runs it a frame; see `show_program`.
    /// A parked seek while `dragging` scrubs audio.
    fn sync_program(&mut self, project: &Project, playhead: Time, dragging: bool) -> Option<Time> {
        let pb = &mut self.program;
        let seq = project.sequence();
        if self.program_seq.as_ref() != Some(seq) {
            self.program_seq = Some(seq.clone());
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

    /// Draws `source` at a time, decoding it first if it changed; an `offline` source shows a
    /// placeholder instead. With `program`, draws the composited timeline frame instead.
    /// Returns a transport button's action id when one was clicked.
    fn show(
        &mut self,
        ui: &mut Ui,
        source: Option<(&Path, Time)>,
        offline: bool,
        row: Option<&SourceRow>,
        program: Option<&ProjectSettings>,
    ) -> Option<&'static str> {
        let path = source.map(|(p, _)| p);
        if self.source.as_deref() != path {
            self.source = path.map(Path::to_path_buf);
            (self.frame, self.error, self.requested) = (None, None, None);
        }
        if offline {
            // Decodes again once it's back.
            (self.frame, self.requested) = (None, None);
        }
        if let Some((path, t)) = source
            && !offline
            && self.requested != Some(t)
            && !self.pending
        {
            (self.requested, self.pending) = (Some(t), true);
            let (tx, ctx, p) = (self.tx.clone(), ui.ctx().clone(), path.to_path_buf());
            self.pool.submit(
                &self.jobs,
                path.to_path_buf(),
                t,
                Priority::Interactive,
                move |r| {
                    let _ = tx.send((p, t, r.map_err(|e| e.to_string())));
                    ctx.request_repaint();
                },
            );
        }
        while let Ok((path, t, r)) = self.rx.try_recv() {
            self.pending = false;
            if self.source.as_deref() != Some(&path) {
                continue;
            }
            // A newer time is wanted; go again next frame.
            if self.requested != Some(t) {
                ui.ctx().request_repaint();
            }
            match r {
                Ok(f) => {
                    self.error = None;
                    self.frame = Some(Arc::new(f));
                    self.generation += 1;
                }
                Err(e) => {
                    tracing::warn!("preview decode of {} failed: {e}", path.display());
                    self.error = Some(e);
                }
            }
        }
        if program.is_none() {
            self.source.as_ref()?;
        }

        let rect = ui.max_rect();
        let viewer = Rect::from_min_max(rect.min, rect.max - vec2(0.0, size::HEADER));
        ui.painter().rect_filled(viewer, 0.0, color::VIEWER);
        if offline {
            ui.painter().text(
                viewer.center(),
                Align2::CENTER_BOTTOM,
                theme::icon::CIRCLE_ALERT,
                theme::icon_font(),
                color::RED,
            );
            ui.painter().text(
                viewer.center() + vec2(0.0, space::S8),
                Align2::CENTER_TOP,
                "Media offline. Relink it from the media bin.",
                theme::body(),
                color::INK_2,
            );
        } else if let Some(e) = &self.error {
            ui.painter().text(
                viewer.center(),
                Align2::CENTER_CENTER,
                format!("Can't show this frame: {e}"),
                theme::body(),
                color::INK_2,
            );
        }
        let drawn = match (program, &self.program_shown) {
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
            let callback = match (program, &self.program_shown, &self.frame) {
                (Some(s), Some(shown), _) => egui_wgpu::Callback::new_paint_callback(
                    viewer,
                    ProgramPaint {
                        shown: shown.clone(),
                        settings: s.clone(),
                        generation: (self.program_gen, scale),
                        clip,
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
            let stroke = Stroke::new(1.0, color::INK.gamma_multiply(0.6));
            for r in rects {
                painter.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Middle);
            }
            for l in lines {
                painter.line_segment(l, stroke);
            }
        }

        let bar = Rect::from_x_y_ranges(rect.x_range(), viewer.bottom()..=rect.bottom());
        let mut action = None;
        if let Some(r) = row {
            r.marks(ui, viewer);
            let inner = bar.shrink2(vec2(space::INSET, 0.0));
            ui.painter().text(
                inner.left_center(),
                Align2::LEFT_CENTER,
                format_timecode(r.frame as u64, r.rate),
                theme::timecode(),
                color::INK,
            );
            let buttons = Rect::from_center_size(bar.center(), vec2(5.0 * 28.0, bar.height()));
            ui.scope_builder(UiBuilder::new().max_rect(buttons), |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = space::S4;
                    let play = if r.playing {
                        (icon::PAUSE, "Stop (Space)")
                    } else {
                        (icon::PLAY, "Play (Space)")
                    };
                    for (glyph, name, id) in [
                        (icon::SKIP_BACK, "Go to start (Home)", "go_to_start"),
                        (icon::REWIND, "Shuttle left (J)", "shuttle_left"),
                        (play.0, play.1, "play_stop"),
                        (icon::FAST_FORWARD, "Shuttle right (L)", "shuttle_right"),
                        (icon::SKIP_FORWARD, "Go to end (End)", "go_to_end"),
                    ] {
                        if ui
                            .add(Button::icon(glyph, name))
                            .on_hover_text(name)
                            .clicked()
                        {
                            action = Some(id);
                        }
                    }
                });
            });
            let rtl = Layout::right_to_left(Align::Center);
            ui.scope_builder(UiBuilder::new().max_rect(inner).layout(rtl), |ui| {
                ui.spacing_mut().item_spacing.x = space::S8;
                self.dropdowns(ui);
                ui.label(
                    egui::RichText::new(format_timecode(r.end as u64, r.rate))
                        .font(theme::body())
                        .color(color::INK_2),
                )
                .on_hover_text("Duration");
            });
        } else {
            ui.scope_builder(UiBuilder::new().max_rect(bar), |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = space::S8;
                    self.dropdowns(ui);
                });
            });
        }
        action
    }

    fn dropdowns(&mut self, ui: &mut Ui) {
        let zooms = ZOOMS.map(|z| z.0);
        ui.add(Dropdown::new(&mut self.zoom, &zooms).width(88.0))
            .on_hover_text("Zoom");
        let qualities = QUALITIES.map(|q| q.0);
        if ui
            .add(Dropdown::new(&mut self.quality, &qualities).width(72.0))
            .on_hover_text("Preview resolution")
            .changed()
        {
            self.generation += 1;
        }
        let glyph = egui::RichText::new(icon::GRID).font(theme::icon_font());
        ui.menu_button(glyph, |ui| {
            let o = &mut self.overlays;
            ui.add(Checkbox::new(&mut o.safe, "Safe areas"));
            ui.add(Checkbox::new(&mut o.thirds, "Rule of thirds"));
            ui.add(Checkbox::new(&mut o.center, "Center cross"));
            for (on, (name, _)) in o.aspects.iter_mut().zip(ASPECTS) {
                ui.add(Checkbox::new(on, name));
            }
        })
        .response
        .on_hover_text("Guides");
    }
}

/// What the Source tab's transport row shows.
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
    /// The in/out range and playhead along the viewer's bottom edge.
    fn marks(&self, ui: &Ui, viewer: Rect) {
        if self.duration.0 <= 0 {
            return;
        }
        let x = |t: Time| viewer.left() + viewer.width() * t.0 as f32 / self.duration.0 as f32;
        let strip =
            Rect::from_x_y_ranges(viewer.x_range(), viewer.bottom() - 3.0..=viewer.bottom());
        let painter = ui.painter();
        if self.in_point.is_some() || self.out_point.is_some() {
            let from = x(self.in_point.unwrap_or(Time(0)));
            let to = x(self.out_point.unwrap_or(self.duration));
            let band = Rect::from_x_y_ranges(from..=to, strip.y_range());
            painter.rect_filled(band, 0.0, color::LIGHT_BLUE.gamma_multiply(0.5));
        }
        let head = x(self.position);
        painter.rect_filled(
            Rect::from_x_y_ranges(head - 1.0..=head + 1.0, strip.y_range()),
            0.0,
            color::LIGHT_BLUE,
        );
    }
}

/// The GPU half, kept in egui-wgpu's callback resources.
struct Gpu {
    preview: render::Preview,
    /// The generation of the uploaded image.
    shown: Option<u64>,
    program: playback::Gpu,
    program_view: render::Preview,
    /// The program generation and preview scale last composited.
    program_drawn: Option<(u64, u32)>,
}

/// The Program tab's draw. Composites only when the frame or preview scale changed.
struct ProgramPaint {
    shown: Arc<Shown>,
    settings: ProjectSettings,
    generation: (u64, u32),
    clip: [f32; 4],
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
            if gpu.program_drawn != Some(self.generation) {
                gpu.program_drawn = Some(self.generation);
                let scale = self.generation.1;
                let tex = gpu
                    .program
                    .draw(device, queue, &self.settings, scale, &self.shown);
                gpu.program_view.show_image(device, tex.clone());
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
    /// The Program tab has a composited frame with at least one layer.
    pub fn has_program_frame(&self) -> bool {
        self.program_shown
            .as_ref()
            .is_some_and(|s| !s.layers.is_empty())
    }

    /// The decode for the current source or program frame has come back, with a frame or an error.
    pub fn settled(&self) -> bool {
        self.frame.is_some() || self.error.is_some() || self.program_shown.is_some()
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
        h.apply(Command::SetClipProps(vec![(red, half, Default::default())]))
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
}
