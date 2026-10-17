//! The timeline panel: a timecode ruler, one lane per track, clips with filmstrips and
//! waveforms, zoom, scroll and a draggable playhead. Only what's in view is drawn.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{
    Align2, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, TextureHandle, Ui, pos2, vec2,
};
use jvid_core::model::{Clip, MediaAsset, Project, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND, Rate, Time, format_timecode};
use jvid_jobs::Priority;
use jvid_media::thumbnails;
use jvid_media::waveforms::Pyramid;
use suite_ui::theme;
use suite_ui::tokens::{color, radius, size, space};

use crate::media_bin::Caches;
use crate::transport::Transport;

const TRACK: f32 = size::TRACK;
/// Zoom limits in points per second: about four hours across 1000pt, to a frame 40pt wide.
const MIN_ZOOM: f64 = 0.07;
const MAX_ZOOM: f64 = 2400.0;
/// `=` / `-` zoom step.
const ZOOM_STEP: f64 = 1.5;
/// Labelled ticks at least this far apart, unlabelled ones at least `MINOR_PX`.
const MAJOR_PX: f64 = 90.0;
const MINOR_PX: f64 = 8.0;
/// Name strip at the top of a clip.
const NAME_STRIP: f32 = 16.0;

pub struct Timeline {
    /// The sequence playhead; #32 gives it a clock, until then it only seeks.
    transport: Transport,
    /// Points per second.
    zoom: f64,
    /// Seconds at the lanes' left edge.
    scroll: f64,
    /// Points the lanes are scrolled up.
    vscroll: f32,
    media: Media,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            transport: Transport::new(Time(0), Rate::new(30, 1)),
            zoom: 20.0,
            scroll: 0.0,
            vscroll: 0.0,
            media: Media::default(),
        }
    }
}

/// Maps between time and x on the lanes.
#[derive(Clone, Copy, Debug)]
struct View {
    /// x of `scroll`.
    left: f32,
    scroll: f64,
    zoom: f64,
}

impl View {
    fn x(&self, t: Time) -> f32 {
        self.left + ((secs(t) - self.scroll) * self.zoom) as f32
    }

    fn time(&self, x: f32) -> Time {
        let s = (x - self.left) as f64 / self.zoom + self.scroll;
        Time((s * FLICKS_PER_SECOND as f64).round() as i64)
    }
}

fn secs(t: Time) -> f64 {
    t.0 as f64 / FLICKS_PER_SECOND as f64
}

/// Major (labelled) and minor tick spacing in frames: the first "round" step at least
/// `MAJOR_PX` apart, and the finest step dividing it that's at least `MINOR_PX` apart.
fn tick_steps(zoom: f64, rate: Rate) -> (i64, i64) {
    let base = rate.timecode_base() as i64;
    let steps: Vec<i64> = [1, 2, 5, 10]
        .into_iter()
        .filter(|&f| f < base)
        .chain(
            [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600]
                .into_iter()
                .map(|s| s * base),
        )
        .collect();
    let px = |frames: i64| frames as f64 * rate.den as f64 / rate.num as f64 * zoom;
    let major = steps
        .iter()
        .copied()
        .find(|&s| px(s) >= MAJOR_PX)
        .unwrap_or(*steps.last().unwrap());
    let minor = steps
        .iter()
        .copied()
        .filter(|&s| s < major && major % s == 0 && px(s) >= MINOR_PX)
        .min()
        .unwrap_or(major);
    (major, minor)
}

/// The clips of a start-sorted, non-overlapping track that intersect `[start, end)`.
fn visible(clips: &[Clip], start: Time, end: Time) -> &[Clip] {
    let a = clips.partition_point(|c| c.end() <= start);
    let b = a + clips[a..].partition_point(|c| c.start < end);
    &clips[a..b]
}

impl Timeline {
    pub fn playhead(&self) -> Time {
        self.transport.position(0)
    }

    fn seek(&mut self, t: Time) {
        let r = self.transport.rate;
        self.transport.seek(r.time_floor(r.index_nearest(t)), 0);
    }

    /// Multiplies the zoom, keeping the time under `anchor` (an x on `view`) still.
    fn zoom_by(&mut self, factor: f64, view: View, anchor: f32) {
        let t = secs(view.time(anchor));
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.scroll = (t - (anchor - view.left) as f64 / self.zoom).max(0.0);
    }

    /// Runs a timeline-focused keymap action; false if it isn't one of ours.
    pub fn action(&mut self, id: &str, project: &Project) -> bool {
        self.sync(project);
        let t = &mut self.transport;
        match id {
            "step_back" => t.step(-1, 0),
            "step_forward" => t.step(1, 0),
            "go_to_start" => t.seek(Time(0), 0),
            "go_to_end" => t.seek(t.duration, 0),
            "zoom_in" | "zoom_out" => {
                let f = if id == "zoom_in" {
                    ZOOM_STEP
                } else {
                    1.0 / ZOOM_STEP
                };
                // Anchored at the playhead, which stays where it is on screen.
                let view = self.view(0.0);
                let x = view.x(self.playhead());
                self.zoom_by(f, view, x);
            }
            _ => return false,
        }
        true
    }

    fn sync(&mut self, project: &Project) {
        self.transport.rate = project.settings().frame_rate;
        // Room past the last clip, so the end of the sequence can be reached and seen.
        let tail = Time(10 * FLICKS_PER_SECOND);
        self.transport.duration = project.sequence().duration() + tail;
    }

    pub fn new(caches: Arc<Caches>) -> Self {
        let mut t = Self::default();
        t.media.caches = Some(caches);
        t
    }

    fn view(&self, left: f32) -> View {
        View {
            left,
            scroll: self.scroll,
            zoom: self.zoom,
        }
    }

    pub fn show(&mut self, ui: &mut Ui, project: &Project, missing: &HashSet<PathBuf>) {
        self.sync(project);
        let rate = self.transport.rate;
        let area = ui.max_rect();
        let ppp = ui.pixels_per_point();
        let line = 1.0 / ppp;
        let lanes_left = area.left() + size::TRACK_HEADER;
        let ruler = Rect::from_x_y_ranges(
            lanes_left..=area.right(),
            area.top()..=area.top() + size::RULER,
        );
        let lanes =
            Rect::from_x_y_ranges(lanes_left..=area.right(), ruler.bottom()..=area.bottom());
        let headers = Rect::from_x_y_ranges(area.left()..=lanes_left, lanes.y_range());

        // Input first, so this frame draws the result.
        let view = self.view(lanes_left);
        let id = ui.id().with("timeline");
        let scrub = ui.interact(ruler, id.with("ruler"), Sense::click_and_drag());
        let body = ui.interact(lanes.union(headers), id.with("lanes"), Sense::hover());
        let head_x = view.x(self.playhead());
        let grip = Rect::from_x_y_ranges(head_x - 4.0..=head_x + 4.0, lanes.y_range());
        let drag = ui.interact(grip, id.with("playhead"), Sense::drag());
        if let Some(p) = scrub.interact_pointer_pos().or(drag.interact_pointer_pos())
            && (scrub.is_pointer_button_down_on() || drag.dragged())
        {
            self.seek(view.time(p.x.max(lanes_left)));
        }
        if drag.hovered() || drag.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        let tracks = project.sequence().tracks();
        let content_h = tracks.len() as f32 * TRACK;
        if (body.hovered() || scrub.hovered())
            && let Some(p) = ui.input(|i| i.pointer.hover_pos())
        {
            let (zoom, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
            if zoom != 1.0 {
                self.zoom_by(zoom as f64, view, p.x.max(lanes_left));
            } else {
                self.scroll = (self.scroll - scroll.x as f64 / self.zoom).max(0.0);
                self.vscroll -= scroll.y;
            }
        }
        self.vscroll = self
            .vscroll
            .clamp(0.0, (content_h - lanes.height()).max(0.0));
        let view = self.view(lanes_left);
        let (start, end) = (view.time(lanes.left()), view.time(lanes.right()));

        let painter = ui.painter_at(area);
        // Lanes, clipped below the ruler.
        let lane_painter = painter.with_clip_rect(lanes.union(headers).intersect(area));
        let clip_painter = painter.with_clip_rect(lanes.intersect(area));
        lane_painter.rect_filled(lanes, 0.0, color::FIELD);
        let top = lanes.top() - self.vscroll;
        for (i, track) in tracks.iter().enumerate() {
            let y = top + i as f32 * TRACK;
            let lane = Rect::from_x_y_ranges(lanes.x_range(), y..=y + TRACK);
            if lane.bottom() < lanes.top() || lane.top() > lanes.bottom() {
                continue;
            }
            lane_painter.text(
                pos2(area.left() + space::INSET, lane.center().y),
                Align2::LEFT_CENTER,
                &track.name,
                theme::body_strong(),
                color::INK,
            );
            lane_painter.rect_filled(
                Rect::from_x_y_ranges(area.x_range(), lane.bottom() - line..=lane.bottom()),
                0.0,
                color::BORDER,
            );
            for clip in visible(track.clips(), start, end) {
                // Clamped just past the lanes so far off-screen ends don't make huge coordinates.
                let (l, r) = (lanes.left() - 8.0, lanes.right() + 8.0);
                let x = view.x(clip.start).clamp(l, r)..=view.x(clip.end()).clamp(l, r);
                // 2px gaps between clips and 2px above and below inside the lane.
                let r = Rect::from_x_y_ranges(x, lane.y_range()).shrink2(vec2(1.0, 2.0));
                let asset = project.asset(clip.asset);
                let offline = asset.is_some_and(|a| missing.contains(&a.path));
                self.paint_clip(
                    ui,
                    &clip_painter,
                    r,
                    lanes,
                    view,
                    clip,
                    asset,
                    track.kind,
                    offline,
                );
            }
        }
        if tracks.is_empty() {
            painter.text(
                lanes.center(),
                Align2::CENTER_CENTER,
                "Drag clips here to start editing",
                theme::body(),
                color::INK_3,
            );
        }

        // Ruler.
        painter.rect_filled(ruler, 0.0, color::PANEL);
        let (major, minor) = tick_steps(self.zoom, rate);
        let first = rate.index_floor(start).max(0) / minor * minor;
        let last = rate.index_floor(end) + 1;
        let mut f = first;
        while f <= last {
            let x = (view.x(rate.time_floor(f)) * ppp).round() / ppp;
            let is_major = f % major == 0;
            let h = if is_major { 8.0 } else { 4.0 };
            painter.rect_filled(
                Rect::from_x_y_ranges(x..=x + line, ruler.bottom() - h..=ruler.bottom()),
                0.0,
                color::BORDER_STRONG,
            );
            if is_major {
                painter.text(
                    pos2(x + 3.0, ruler.top() + 2.0),
                    Align2::LEFT_TOP,
                    format_timecode(f as u64, rate),
                    theme::small(),
                    color::INK_3,
                );
            }
            f += minor;
        }
        painter.rect_filled(
            Rect::from_x_y_ranges(area.x_range(), ruler.bottom() - line..=ruler.bottom()),
            0.0,
            color::BORDER,
        );

        // Corner cell: the playhead's timecode and the zoom slider.
        let playhead = self.playhead();
        painter.text(
            pos2(area.left() + space::INSET, ruler.center().y),
            Align2::LEFT_CENTER,
            format_timecode(rate.index_floor(playhead).max(0) as u64, rate),
            theme::body_strong(),
            color::INK,
        );
        let slider = Rect::from_x_y_ranges(
            lanes_left - 64.0..=lanes_left - space::INSET,
            ruler.y_range(),
        );
        let mut z = self.zoom;
        let r = ui
            .scope(|ui| {
                // Its default width would spill past `slider` over the timecode and ruler.
                ui.spacing_mut().slider_width = slider.width();
                ui.put(
                    slider,
                    egui::Slider::new(&mut z, MIN_ZOOM..=MAX_ZOOM)
                        .logarithmic(true)
                        .show_value(false),
                )
            })
            .inner;
        if r.changed() {
            let x = view.x(playhead).clamp(lanes.left(), lanes.right());
            self.zoom_by(z / self.zoom, view, x);
        }
        r.on_hover_text("Zoom (= and -)");

        // Playhead: never eased, it reports time.
        let x = view.x(playhead);
        if lanes.x_range().contains(x) {
            let x = (x * ppp).round() / ppp;
            painter.rect_filled(
                Rect::from_x_y_ranges(x..=x + line, ruler.top() + 12.0..=area.bottom()),
                0.0,
                color::LIGHT_BLUE,
            );
            playhead_head(&painter, pos2(x + line / 2.0, ruler.top()));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_clip(
        &mut self,
        ui: &Ui,
        painter: &egui::Painter,
        r: Rect,
        lanes: Rect,
        view: View,
        clip: &Clip,
        asset: Option<&MediaAsset>,
        kind: TrackKind,
        offline: bool,
    ) {
        let still = asset.is_some_and(|a| image::ImageFormat::from_path(&a.path).is_ok());
        let fill = match (kind, still) {
            (TrackKind::Audio, _) => color::CLIP_GREEN,
            (_, true) => color::CLIP_OCHRE,
            _ => color::CLIP_VIOLET,
        };
        let corner = CornerRadius::same(radius::SMALL);
        painter.rect_filled(r, corner, fill);
        // Drawn content is limited to what's on screen, however long the clip.
        let shown = r.intersect(lanes);
        let content = Rect::from_x_y_ranges(shown.x_range(), r.top() + NAME_STRIP..=r.bottom());
        let clipped = painter.with_clip_rect(r.intersect(painter.clip_rect()));
        if offline {
            clipped.rect_filled(r, corner, color::RED.gamma_multiply(0.3));
        } else if let Some(a) = asset
            && content.height() >= 8.0
            && content.width() >= 1.0
        {
            let source = |x: f32| clip.source_in + (view.time(x) - clip.start);
            match kind {
                TrackKind::Video if !still => {
                    self.media
                        .filmstrip(ui, &clipped, content, view.x(clip.start), a, &source)
                }
                TrackKind::Audio => self.media.waveform(ui, &clipped, content, a, &source),
                TrackKind::Video => {}
            }
        }
        let name = asset.map_or("Missing asset".into(), MediaAsset::display_name);
        let name = if offline {
            format!("{name}  Offline")
        } else {
            name
        };
        clipped.text(
            pos2(shown.left().max(r.left()) + 6.0, r.top() + NAME_STRIP / 2.0),
            Align2::LEFT_CENTER,
            name,
            theme::body_strong(),
            color::INK,
        );
    }
}

/// The 10×12 head: rounded top, pointed bottom, centred on `top_center`.
fn playhead_head(painter: &egui::Painter, top_center: Pos2) {
    let (w, h) = (10.0, 12.0);
    let body = Rect::from_center_size(top_center + vec2(0.0, 4.0), vec2(w, 8.0));
    painter.rect_filled(
        body,
        CornerRadius {
            nw: radius::SMALL,
            ne: radius::SMALL,
            sw: 0,
            se: 0,
        },
        color::LIGHT_BLUE,
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            pos2(body.left(), body.bottom()),
            pos2(body.right(), body.bottom()),
            top_center + vec2(0.0, h),
        ],
        color::LIGHT_BLUE,
        Stroke::NONE,
    ));
}

/// Results finished off-thread, per path, waiting for the UI thread; `None` is a failure.
type Done<T> = Arc<Mutex<Vec<(PathBuf, Option<T>)>>>;

/// Filmstrips and waveform pyramids per asset, loaded on a job pool the first time a clip
/// using it is in view. `None` while loading; a failure is forgotten so the next paint retries.
#[derive(Default)]
struct Media {
    /// `None` in tests, which then load nothing.
    caches: Option<Arc<Caches>>,
    strip_done: Done<Vec<egui::ColorImage>>,
    strips: HashMap<PathBuf, Option<Vec<TextureHandle>>>,
    peaks_done: Done<Pyramid>,
    peaks: HashMap<PathBuf, Option<Pyramid>>,
}

impl Media {
    /// Thumbnails tiled across `rect`, each the frame at its left edge.
    fn filmstrip(
        &mut self,
        ui: &Ui,
        painter: &egui::Painter,
        rect: Rect,
        origin: f32,
        a: &MediaAsset,
        source: &dyn Fn(f32) -> Time,
    ) {
        for (path, imgs) in self.strip_done.lock().unwrap().drain(..) {
            // shortcut: a failed decode retries on the next paint with no backoff; add one if
            // broken files show up as busy job pools.
            let Some(imgs) = imgs else {
                self.strips.remove(&path);
                continue;
            };
            let texs = imgs
                .into_iter()
                .enumerate()
                .map(|(i, img)| {
                    let name = format!("{}#{i}", path.display());
                    ui.ctx().load_texture(name, img, Default::default())
                })
                .collect();
            self.strips.insert(path, Some(texs));
        }
        let Some(probe) = &a.probe else { return };
        let texs = match self.strips.get(&a.path) {
            Some(Some(t)) if !t.is_empty() => t,
            Some(_) => return,
            None => {
                let Some(caches) = &self.caches else { return };
                self.strips.insert(a.path.clone(), None);
                let (done, ctx) = (self.strip_done.clone(), ui.ctx().clone());
                let cache = caches.thumbs.clone();
                let (path, duration) = (a.path.clone(), probe.duration);
                let key = format!("strip:{}", path.display());
                caches.jobs.submit(key, Priority::Preview, move |c| {
                    if c.is_cancelled() {
                        return;
                    }
                    match cache.filmstrip(&path, duration) {
                        Ok(v) => {
                            let imgs = v
                                .iter()
                                .map(|img| {
                                    let size = [img.width() as usize, img.height() as usize];
                                    egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw())
                                })
                                .collect();
                            done.lock().unwrap().push((path, Some(imgs)));
                            ctx.request_repaint();
                        }
                        Err(e) => {
                            tracing::warn!("filmstrip for {}: {e}", path.display());
                            done.lock().unwrap().push((path, None));
                        }
                    }
                });
                return;
            }
        };
        // Same spacing `thumbnails::Cache::filmstrip` samples at.
        let step = FLICKS_PER_SECOND.max(probe.duration.0 / thumbnails::STRIP_MAX);
        let size = texs[0].size_vec2();
        let w = size.x * rect.height() / size.y;
        // Tiles sit on a grid from the clip's start, so they don't swim while scrolling.
        let mut x = origin + ((rect.left() - origin) / w).floor() * w;
        while x < rect.right() {
            let i = (source(x).0 / step).clamp(0, texs.len() as i64 - 1) as usize;
            let tile = Rect::from_min_size(pos2(x, rect.top()), vec2(w, rect.height()));
            painter.image(
                texs[i].id(),
                tile,
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            x += w;
        }
    }

    /// One min/max line per point across `rect`, in a lighter tint of the clip colour.
    fn waveform(
        &mut self,
        ui: &Ui,
        painter: &egui::Painter,
        rect: Rect,
        a: &MediaAsset,
        source: &dyn Fn(f32) -> Time,
    ) {
        for (path, p) in self.peaks_done.lock().unwrap().drain(..) {
            match p {
                Some(p) => self.peaks.insert(path, Some(p)),
                None => self.peaks.remove(&path),
            };
        }
        let pyramid = match self.peaks.get(&a.path) {
            Some(Some(p)) => p,
            Some(None) => return,
            None => {
                let Some(caches) = &self.caches else { return };
                self.peaks.insert(a.path.clone(), None);
                let (done, ctx, path) = (self.peaks_done.clone(), ui.ctx().clone(), a.path.clone());
                caches
                    .peaks
                    .submit(&caches.jobs, path.clone(), 0, move |r| match r {
                        Ok(p) => {
                            done.lock().unwrap().push((path, Some(p)));
                            ctx.request_repaint();
                        }
                        Err(e) => {
                            tracing::warn!("waveform for {}: {e}", path.display());
                            done.lock().unwrap().push((path, None));
                        }
                    });
                return;
            }
        };
        let sample = |t: Time| {
            (t.0 as i128 * jvid_media::audio::SAMPLE_RATE as i128 / FLICKS_PER_SECOND as i128)
                as i64
        };
        let width = rect.width().ceil() as usize;
        let peaks = pyramid.query(
            sample(source(rect.left())),
            sample(source(rect.right())),
            width,
        );
        let (mid, half) = (rect.center().y, rect.height() / 2.0 - 1.0);
        let tint = color::CLIP_GREEN.lerp_to_gamma(color::INK, 0.45);
        for (i, [lo, hi]) in peaks.into_iter().enumerate() {
            let x = rect.left() + i as f32 + 0.5;
            let (y0, y1) = (
                mid - hi.clamp(-1.0, 1.0) * half,
                mid - lo.clamp(-1.0, 1.0) * half,
            );
            painter.line_segment(
                [pos2(x, y0), pos2(x, y1.max(y0 + 1.0))],
                Stroke::new(1.0, tint),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use jvid_core::history::{Command, History};
    use jvid_core::model::{NewClip, ProbeInfo};

    use super::*;

    const S: i64 = FLICKS_PER_SECOND;

    #[test]
    fn x_and_time_round_trip() {
        let v = View {
            left: 100.0,
            scroll: 2.0,
            zoom: 50.0,
        };
        assert_eq!(v.x(Time(2 * S)), 100.0);
        assert_eq!(v.x(Time(4 * S)), 200.0);
        assert_eq!(v.time(200.0), Time(4 * S));
        assert_eq!(v.time(v.x(Time(S / 4))), Time(S / 4));
    }

    #[test]
    fn ruler_ticks_follow_zoom() {
        let r = Rate::new(30, 1);
        // 20pt/s: labels every 5s (100pt), ticks every second (20pt).
        assert_eq!(tick_steps(20.0, r), (150, 30));
        // Zoomed right in: labels every 2 frames, ticks every frame.
        assert_eq!(tick_steps(2400.0, r), (2, 1));
        // Zoomed right out: an hour at most, with 30 minute ticks.
        let (major, minor) = tick_steps(MIN_ZOOM, r);
        assert_eq!((major, minor), (1800 * 30, 120 * 30));
        assert!(major % minor == 0);
        // Every zoom keeps labels apart and minor ticks dividing them.
        for z in [0.5, 3.0, 47.0, 333.0] {
            for r in [
                Rate::new(24, 1),
                Rate::new(25, 1),
                Rate::new(30000, 1001),
                Rate::new(60, 1),
            ] {
                let (major, minor) = tick_steps(z, r);
                assert!(major as f64 * r.den as f64 / r.num as f64 * z >= MAJOR_PX);
                assert_eq!(major % minor, 0);
            }
        }
    }

    fn project(n: usize) -> Project {
        let mut h = History::new(Project::default());
        let _ = h.apply(Command::Import(vec![(
            PathBuf::from("/nonexistent/a.mp4"),
            ProbeInfo {
                duration: Time(3600 * S),
                width: Some(1920),
                height: Some(1080),
                audio_streams: 1,
                ..Default::default()
            },
        )]));
        let asset = h.project().assets()[0].id;
        for (kind, name) in [(TrackKind::Video, "V1"), (TrackKind::Audio, "A1")] {
            let _ = h.apply(Command::AddTrack {
                kind,
                name: name.into(),
            });
        }
        let tracks: Vec<_> = h
            .project()
            .sequence()
            .tracks()
            .iter()
            .map(|t| t.id)
            .collect();
        for i in 0..n {
            let _ = h.apply(Command::AddClip {
                track: tracks[i % 2],
                clip: NewClip {
                    asset,
                    source_in: Time(0),
                    source_out: Time(2 * S),
                    start: Time((i / 2) as i64 * 2 * S),
                    link: None,
                },
            });
        }
        h.project().clone()
    }

    #[test]
    fn only_clips_in_view_are_visible() {
        let p = project(20);
        let clips = p.sequence().tracks()[0].clips();
        assert_eq!(clips.len(), 10);
        // Clips start every 2s and last 2s: [3s, 7s) touches those starting at 2, 4 and 6.
        let v = visible(clips, Time(3 * S), Time(7 * S));
        let starts: Vec<_> = v.iter().map(|c| c.start.0 / S).collect();
        assert_eq!(starts, [2, 4, 6]);
        // Edges are half-open.
        assert_eq!(visible(clips, Time(4 * S), Time(6 * S)).len(), 1);
        assert!(visible(clips, Time(100 * S), Time(200 * S)).is_empty());
    }

    #[test]
    fn keys_zoom_around_the_playhead_and_step_frames() {
        let p = project(4);
        let mut t = Timeline::default();
        assert!(t.action("step_forward", &p));
        assert_eq!(t.playhead(), Rate::new(30, 1).time_floor(1));
        t.seek(Time(3 * S));
        let before = t.view(0.0).x(t.playhead());
        assert!(t.action("zoom_in", &p));
        assert!((t.zoom - 30.0).abs() < 1e-9);
        assert!((t.view(0.0).x(t.playhead()) - before).abs() < 0.01);
        assert!(!t.action("lift", &p));
    }

    fn harness(project: Project) -> egui_kittest::Harness<'static, (Timeline, Project)> {
        let mut fresh = true;
        let mut h = egui_kittest::Harness::builder()
            .with_size([1000.0, 200.0])
            .wgpu()
            .build_ui_state(
                move |ui, (t, p): &mut (Timeline, Project)| {
                    if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                        ui.painter().rect_filled(ui.max_rect(), 0.0, color::PANEL);
                        t.show(ui, p, &HashSet::new());
                    }
                },
                (Timeline::default(), project),
            );
        h.run();
        h
    }

    #[test]
    fn snapshots() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut results = egui_kittest::SnapshotResults::new();
        let mut h = harness(project(12));
        h.state_mut().0.seek(Time(5 * S));
        h.run();
        results.add(h.try_snapshot("timeline"));
        h.state_mut().0.action("zoom_in", &project(0));
        h.state_mut().0.action("zoom_in", &project(0));
        h.run();
        results.add(h.try_snapshot("timeline_zoomed"));
        let mut h = harness(Project::default());
        h.run();
        results.add(h.try_snapshot("timeline_empty"));
    }

    /// The acceptance budget: layout and tessellation of 500 clips inside one 60fps frame, with
    /// filmstrips and waveforms already decoded so their paint paths run.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "timing is only meaningful in release")]
    fn five_hundred_clips_fit_a_frame() {
        let p = project(500);
        let ctx = egui::Context::default();
        suite_ui::theme::install(&ctx);
        // 20pt clips, so a 1920pt-wide panel shows about 175 of them across both tracks.
        let mut t = Timeline {
            zoom: 10.0,
            ..Default::default()
        };
        let path = p.assets()[0].path.clone();
        let img = egui::ColorImage::filled([160, 90], color::BLUE);
        let tex = ctx.load_texture("strip", img, Default::default());
        t.media
            .strips
            .insert(path.clone(), Some(vec![tex.clone(); 10]));
        let finest =
            (0..750).map(|i| [-(i as f32 * 0.1).sin().abs(), (i as f32 * 0.1).sin().abs()]);
        t.media
            .peaks
            .insert(path, Some(Pyramid::new(finest.collect())));
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 400.0))),
            ..Default::default()
        };
        let mut frame = || {
            let out = ctx.run_ui(input(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| t.show(ui, &p, &HashSet::new()));
            });
            let lines = out
                .shapes
                .iter()
                .filter(|s| matches!(s.shape, egui::Shape::LineSegment { .. }))
                .count();
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            let tiles = prims
                .iter()
                .filter(|c| matches!(&c.primitive, egui::epaint::Primitive::Mesh(m) if m.texture_id == tex.id()))
                .count();
            (lines, tiles)
        };
        let (lines, tiles) = frame();
        // About 85 audio clips of ~18 waveform columns, and filmstrip tiles on the video ones.
        assert!(lines > 1000, "{lines} waveform lines");
        assert!(tiles > 0, "no filmstrip tiles");
        let n = 30;
        let start = std::time::Instant::now();
        for _ in 0..n {
            frame();
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("timeline layout+paint, 500 clips ({lines} waveform lines): {ms:.3} ms/frame");
        assert!(ms < 16.6, "{ms:.3} ms/frame");
    }
}
