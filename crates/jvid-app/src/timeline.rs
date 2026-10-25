//! The timeline panel: a timecode ruler, one lane per track, clips with filmstrips and
//! waveforms, zoom, scroll and a draggable playhead. Only what's in view is drawn. Track
//! headers on the left hold the lock, hide or mute, and solo toggles, the target, a height
//! grip, and a menu to add, rename, reorder and remove tracks. Media dropped from the bin
//! overwrites at the drop point, on a new track when dropped off every lane; dragged clips
//! overwrite where they land. Clicks and a marquee select clips, which copy, cut, paste at
//! the playhead, duplicate and delete.
//! Clip edges drag to trim (Ctrl ripples), the razor tool and Ctrl+K split, a clicked gap
//! ripple deletes, and Q/W ripple trim to the playhead. In/out marks shade the ruler and
//! lanes, drag by their ruler flags, and lift (`;`) or extract (`'`) their range.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{
    Align2, Color32, CornerRadius, CursorIcon, FontId, Key, Pos2, Rect, Sense, Stroke,
    TextureHandle, Ui, pos2, vec2,
};
use jvid_core::history::{Command, GestureKey, History};
use jvid_core::model::{
    AssetId, Clip, ClipId, Edge, MediaAsset, ModelError, Placement, ProbeInfo, Project, Sequence,
    SequenceId, StreamInfo, Track, TrackId, TrackKind, Transform,
};
use jvid_core::time::{FLICKS_PER_SECOND, Rate, Time, format_timecode};
use jvid_jobs::Priority;
use jvid_media::thumbnails;
use jvid_media::waveforms::{self, BUCKET, Peak, Pyramid};
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, radius, size, space};
use suite_ui::widgets::{Button, TextInput};

use crate::i18n::{tr, trn, verbatim};
use crate::keymap::Keymap;
use crate::media_bin::Caches;
use crate::transport::Transport;

mod fades;
mod graph;
mod keyframes;
mod markers;
pub mod menus;
mod rubber;
mod silence;
mod transitions;

/// The undo gesture a track-height drag coalesces under.
pub const RESIZE_GESTURE: GestureKey = 0x7472_6163_6b68; // "trackh"
/// Track heights snap to this step.
const HEIGHT_STEP: f32 = 4.0;
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
    /// The track being renamed and the name typed so far.
    renaming: Option<(TrackId, String)>,
    /// The track whose Audio Effects popover is open.
    fx_popover: Option<TrackId>,
    /// Where the dragged clip would land; the press origin is gone by the release frame.
    drag_to: Option<(TrackId, Time)>,
    selected: HashSet<ClipId>,
    /// Clicking a clip selects its linked partners too.
    linked_selection: bool,
    /// Copied clips with the tracks they came from.
    clipboard: Vec<(TrackId, Clip)>,
    /// Copy Grade's grade, from a video clip at the playhead.
    grade: Option<Transform>,
    /// An edit from a keymap action, handed out by the next `show`.
    pending: Option<Command>,
    /// Where M went down; its release adds the marker.
    marker_hold: Option<Time>,
    /// Shift+M or Ctrl+Shift+M jumped to a marker, for the markers lesson.
    pub jumped: bool,
    /// Where Q puts the playhead once its trim applies.
    seek_after: Option<Time>,
    /// Copy or cut ran since the last `show`.
    copied: bool,
    /// The razor tool (C) is on instead of selection (V).
    razor: bool,
    /// The clip edge being dragged.
    trim: Option<Trim>,
    /// The clip being moved or trimmed, kept interactive after it scrolls out of view.
    holding: Option<ClipId>,
    /// Height of the New Track slot above the top track last frame, open while a video
    /// asset is dragged at the top edge.
    slot: f32,
    /// `scroll` and `vscroll` when the pointer went down, so drags track time while the
    /// view auto-scrolls.
    press_scroll: (f64, f32),
    /// The clicked gap: its track and a time inside it.
    gap: Option<(TrackId, Time)>,
    /// Lanes width last frame, for actions that fit to it.
    width: f32,
    /// The playhead last frame; a change is what makes the view follow it.
    followed: Time,
    /// Moves and trims snap to edges, the playhead, markers and in/out (S toggles).
    snapping: bool,
    /// Where the current drag snapped, for the indicator line.
    snapped: Option<Time>,
    /// An action picked from a menu or the toolbar this frame.
    menu: Option<&'static str>,
    /// The in (true) or out flag being dragged, and where it is.
    mark_drag: Option<(bool, Time)>,
    /// "Reveal in Media Bin" asked for this asset; the app takes it.
    pub reveal: Option<AssetId>,
    /// A message for the app to show, such as why a transition couldn't be added.
    pub notice: Option<String>,
    /// The sequence picked in the header to show instead; the app opens it.
    pub open_sequence: Option<SequenceId>,
    /// The sequence shown last frame, to drop a selection made in another.
    shown_sequence: SequenceId,
    /// Keyframe lanes: expanded clips, picked keys and their clipboard.
    keys: keyframes::Keys,
    /// The Detect Silence dialog, while open.
    silence: Option<silence::Silence>,
    /// The audio track a voiceover records onto; disarming it ends a take.
    pub armed: Option<TrackId>,
    /// The input level while a take records onto `armed`, 0..=1; the app sets it.
    pub level: Option<f32>,
    /// The take recording onto `armed`, drawn as a clip; the app keeps it up to date.
    pub live: Option<Live>,
}

/// A voiceover take while it records: drawn on the armed track, not in the project.
pub struct Live {
    pub start: Time,
    /// The file's name, as its clip will show it.
    pub name: String,
    /// The input's sample rate; each peak spans [`BUCKET`] of its frames.
    pub rate: u32,
    pub peaks: Vec<Peak>,
}

/// Snap reach in points.
const SNAP_PX: f64 = 8.0;

/// Points inside a lanes edge where a drag starts auto-scrolling.
const EDGE_PX: f32 = 24.0;

/// Points into the top track's upper edge that open the New Track slot.
const SLOT_PX: f32 = 8.0;

/// Auto-scroll speed in points per second for a pointer at `p` along `lo..=hi`: zero
/// away from the edges, growing with depth into (and past) either edge zone.
fn edge_speed(p: f32, lo: f32, hi: f32) -> f32 {
    let depth = (p - (hi - EDGE_PX)).max(0.0) - ((lo + EDGE_PX) - p).max(0.0);
    depth * 20.0
}

/// A clip edge drag: how far, already held to what the media and neighbours allow.
#[derive(Clone, Copy, Debug)]
struct Trim {
    clip: ClipId,
    edge: Edge,
    by: Time,
    ripple: bool,
}

/// An edit made in the timeline, and the gesture it coalesces under, if any.
pub type Edit = (Command, Option<GestureKey>);

impl Default for Timeline {
    fn default() -> Self {
        Self {
            transport: Transport::new(Time(0), Rate::new(30, 1)),
            zoom: 20.0,
            scroll: 0.0,
            vscroll: 0.0,
            holding: None,
            slot: 0.0,
            press_scroll: (0.0, 0.0),
            media: Media::default(),
            renaming: None,
            fx_popover: None,
            drag_to: None,
            selected: HashSet::new(),
            linked_selection: true,
            clipboard: Vec::new(),
            grade: None,
            pending: None,
            marker_hold: None,
            jumped: false,
            seek_after: None,
            copied: false,
            razor: false,
            trim: None,
            gap: None,
            width: 800.0,
            followed: Time(0),
            snapping: true,
            snapped: None,
            menu: None,
            mark_drag: None,
            reveal: None,
            notice: None,
            open_sequence: None,
            shown_sequence: SequenceId::default(),
            keys: keyframes::Keys::default(),
            silence: None,
            armed: None,
            level: None,
            live: None,
        }
    }
}

/// In/out keymap actions the timeline runs when it or the Program tab has focus.
pub const MARK_ACTIONS: &[&str] = &[
    "mark_in",
    "mark_out",
    "clear_in",
    "clear_out",
    "clear_in_out",
    "go_to_in",
    "go_to_out",
    "mark_clip",
    "mark_selection",
    "lift",
    "extract",
    "add_marker",
    "add_range_marker",
    "next_marker",
    "prev_marker",
];

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

/// The shift that lands the nearest of `edges` on one of `targets`, if any is within
/// `reach`, with the target it hit.
fn snap_shift(edges: &[Time], targets: &[Time], reach: Time) -> Option<(Time, Time)> {
    let pairs = edges
        .iter()
        .flat_map(|e| targets.iter().map(move |t| (*t - *e, *t)));
    pairs
        .filter(|(d, _)| d.0.abs() <= reach.0)
        .min_by_key(|(d, _)| d.0.abs())
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

    /// The recording take and where it ends: at the playhead, which plays on as it records.
    fn live_span(&self) -> Option<(&Live, Time)> {
        let live = self.live.as_ref().filter(|_| self.armed.is_some())?;
        Some((live, self.playhead().max(live.start)))
    }

    /// The waveform a finished take showed, kept for its clip at `path` instead of a rescan.
    pub fn recorded(&mut self, path: PathBuf, peaks: Pyramid) {
        self.media.peaks.insert((path, 0), Some(peaks));
    }

    pub fn selected(&self) -> &HashSet<ClipId> {
        &self.selected
    }

    /// Selects only `id`, as a plain click on it would.
    pub fn select_clip(&mut self, seq: &Sequence, id: ClipId) {
        self.select(seq, id, egui::Modifiers::NONE);
    }

    pub fn seek(&mut self, t: Time) {
        let r = self.transport.rate;
        self.transport.seek(r.time_floor(r.index_nearest(t)), 0);
    }

    /// Tells the timeline whether its last edit applied, so Q only moves the playhead
    /// when its trim did.
    pub fn applied(&mut self, ok: bool) {
        if let Some(t) = self.seek_after.take()
            && ok
        {
            self.seek(t);
        }
    }

    /// Where a playhead navigation key goes from here, if anywhere.
    fn nav_target(&self, id: &str, seq: &Sequence) -> Option<Time> {
        let (here, r) = (self.playhead(), self.transport.rate);
        let edits = seq
            .tracks()
            .iter()
            .flat_map(|t| t.clips())
            .flat_map(|c| [c.start, c.end()]);
        // The selected clip under the playhead, else the topmost one.
        let clip = || {
            let mut under = seq.clips_at(here).map(|(_, c)| c);
            let first = seq.clips_at(here).next().map(|(_, c)| c);
            under.find(|c| self.selected.contains(&c.id)).or(first)
        };
        match id {
            "step_back_second" => Some((here - Time(FLICKS_PER_SECOND)).max(Time(0))),
            "step_forward_second" => {
                Some((here + Time(FLICKS_PER_SECOND)).min(self.transport.duration))
            }
            "prev_edit" | "prev_edit_alt" => edits.filter(|t| *t < here).max(),
            "next_edit" | "next_edit_alt" => edits.filter(|t| *t > here).min(),
            "clip_start" => clip().map(|c| c.start),
            // End is exclusive: the clip's last frame, so the playhead stays on it.
            "clip_end" => clip().map(|c| r.time_floor(r.index_floor(c.end() - Time(1)))),
            _ => None,
        }
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
            // The sequence end, not the scrollable tail past it.
            "go_to_end" => t.seek(project.sequence().duration(), 0),
            "step_back_second"
            | "step_forward_second"
            | "prev_edit"
            | "next_edit"
            | "prev_edit_alt"
            | "next_edit_alt"
            | "clip_start"
            | "clip_end" => {
                if let Some(to) = self.nav_target(id, project.sequence()) {
                    self.seek(to);
                }
            }
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
            "zoom_fit" => match &mut self.keys.graph {
                Some(g) => g.fit = true,
                None => self.zoom_fit(project.sequence()),
            },
            "go_to_in" => t.seek(project.sequence().in_point().unwrap_or_default(), 0),
            // Out is exclusive: the last marked frame.
            "go_to_out" => {
                let end = project.sequence().out_point().unwrap_or(t.duration);
                t.seek(t.rate.time_floor(t.rate.index_floor(end - Time(1))), 0);
            }
            "lift" | "extract" => {
                let seq = project.sequence();
                let marked = seq.in_point().is_some() || seq.out_point().is_some();
                let ripple = id == "extract";
                self.pending = marked.then_some(Command::RemoveRange { ripple });
            }
            "mark_in" | "mark_out" | "clear_in" | "clear_out" | "clear_in_out" | "mark_clip"
            | "mark_selection" => self.pending = self.mark_action(id, project.sequence()),
            _ if self.marker_action(id, project.sequence()) => {}
            _ if self.key_action(id, project.sequence()) => {}
            "linked_selection" => self.linked_selection = !self.linked_selection,
            "snapping" => self.snapping = !self.snapping,
            "razor_tool" | "selection_tool" => self.razor = id == "razor_tool",
            "select_all" => {
                let clips = project.sequence().tracks().iter().flat_map(|t| t.clips());
                self.selected = clips.map(|c| c.id).collect();
                self.keys.selected.clear();
            }
            "add_edit" | "ripple_trim_prev" | "ripple_trim_next" => {
                self.pending = self.cut_action(id, project.sequence());
            }
            "delete" | "ripple_delete" if self.selected.is_empty() && self.gap.is_some() => {
                let (track, at) = self.gap.take().expect("checked");
                self.pending = Some(Command::CloseGap { track, at });
            }
            "detect_silence" => {
                self.silence = silence::Silence::open(project.sequence(), &self.selected);
            }
            "crossfade" | "dip_to_black" | "dip_to_white" => {
                match self.transition_action(id, project) {
                    Ok(c) => self.pending = Some(c),
                    Err(e) => self.notice = Some(e),
                }
            }
            "nest" if !self.selected.is_empty() => {
                let mut ids: Vec<_> = self.selected.iter().copied().collect();
                ids.sort();
                let nest = Command::Nest {
                    ids,
                    name: tr!("timeline.nest_name", n = project.next_sequence().0),
                    outer: tr!("timeline.main_sequence").to_owned(),
                };
                // Tried on a copy, so a clip in the way says why rather than failing quietly.
                match History::new(project.clone()).apply(nest.clone()) {
                    Err(ModelError::Overlap(_)) => {
                        self.notice = Some(tr!("timeline.nest_overlap").to_owned());
                    }
                    Err(ModelError::SplitTransition) => {
                        self.notice = Some(tr!("timeline.nest_transition").to_owned());
                    }
                    _ => self.pending = Some(nest),
                }
            }
            "nest" => {}
            "copy_grade" => {
                let seq = project.sequence();
                if let Some(c) = self.video_chosen(seq).first() {
                    self.grade = Some(c.transform_at(self.playhead()));
                }
            }
            "copy" | "cut" | "paste" | "duplicate" | "delete" | "ripple_delete" | "link"
            | "enable" | "reverse" | "freeze_frame" | "paste_grade" => {
                self.pending = self.clip_action(id, project.sequence());
            }
            _ => return false,
        }
        true
    }

    /// The new in/out marks for a marking action, if they change. A mark past the other one
    /// clears it.
    fn mark_action(&self, id: &str, seq: &Sequence) -> Option<Command> {
        let r = self.transport.rate;
        let here = self.playhead();
        // Out is exclusive, like a clip's end: after the playhead's frame.
        let after = r.time_floor(r.index_floor(here) + 1);
        let (i, o) = (seq.in_point(), seq.out_point());
        let span = |clips: Vec<&Clip>| {
            let start = clips.iter().map(|c| c.start).min()?;
            Some((Some(start), clips.iter().map(|c| c.end()).max()))
        };
        let (i, o) = match id {
            "mark_in" => (Some(here), o.filter(|o| *o > here)),
            "mark_out" => (i.filter(|i| *i < after), Some(after)),
            "clear_in" => (None, o),
            "clear_out" => (i, None),
            "clear_in_out" => (None, None),
            // The topmost clip under the playhead.
            "mark_clip" => span(seq.clips_at(here).take(1).map(|(_, c)| c).collect())?,
            _ => span(
                seq.tracks()
                    .iter()
                    .flat_map(|t| t.clips())
                    .filter(|c| self.selected.contains(&c.id))
                    .collect(),
            )?,
        };
        ((i, o) != (seq.in_point(), seq.out_point())).then_some(Command::SetInOut {
            in_point: i,
            out_point: o,
        })
    }

    /// The selected clips on video tracks, top track first.
    fn video_chosen<'a>(&self, seq: &'a Sequence) -> Vec<&'a Clip> {
        let video = seq.tracks().iter().filter(|t| t.kind == TrackKind::Video);
        let clips = video.flat_map(|t| t.clips());
        clips.filter(|c| self.selected.contains(&c.id)).collect()
    }

    /// The edit for a selection or clipboard action, if it changes anything.
    fn clip_action(&mut self, id: &str, seq: &Sequence) -> Option<Command> {
        let all = seq
            .tracks()
            .iter()
            .flat_map(|t| t.clips().iter().map(|c| (t.id, c)));
        let chosen: Vec<_> = all
            .filter(|(_, c)| self.selected.contains(&c.id))
            .map(|(t, c)| (t, c.clone()))
            .collect();
        let ids: Vec<_> = chosen.iter().map(|(_, c)| c.id).collect();
        if matches!(id, "copy" | "cut") && !chosen.is_empty() {
            self.keys.clipboard.clear();
            self.clipboard = chosen.clone();
            self.copied = true;
        }
        let cmd = match id {
            "paste" if !self.clipboard.is_empty() => Command::Paste {
                clips: to_targets(seq, &self.clipboard),
                at: self.playhead(),
            },
            "duplicate" if !chosen.is_empty() => Command::Paste {
                at: chosen
                    .iter()
                    .map(|(_, c)| c.end())
                    .max()
                    .expect("not empty"),
                clips: chosen,
            },
            "cut" | "delete" | "ripple_delete" if !ids.is_empty() => Command::RemoveClips {
                ids,
                ripple: id == "ripple_delete",
            },
            // Unlinks when the selection is already one link group.
            "link" if !ids.is_empty() => {
                let first = chosen[0].1.link;
                let one = first.is_some() && chosen.iter().all(|(_, c)| c.link == first);
                let whole = one && seq.linked(ids[0]).len() == ids.len();
                Command::LinkClips {
                    ids,
                    linked: !whole,
                }
            }
            // Disables unless every chosen clip is already disabled.
            "enable" if !ids.is_empty() => Command::SetClipsEnabled {
                enabled: chosen.iter().all(|(_, c)| !c.enabled),
                ids,
            },
            // The first chosen clip and its links; its speed stays.
            "reverse" if !ids.is_empty() => {
                let c = &chosen[0].1;
                Command::SetClipSpeed {
                    id: c.id,
                    speed: c.speed,
                    reverse: !c.reverse,
                    ripple: false,
                }
            }
            // Every chosen video clip gets the grade, static, in one undo step.
            "paste_grade" => {
                let g = self.grade.as_ref()?;
                let video = self.video_chosen(seq);
                (!video.is_empty()).then_some(())?;
                Command::SetClipProps(
                    video
                        .iter()
                        .map(|c| {
                            let (mut x, mut keys) = (c.transform.clone(), c.keyframes.clone());
                            x.paste_grade(g);
                            for name in Transform::GRADE_KEYS {
                                keys.remove(name);
                            }
                            (c.id, x, keys)
                        })
                        .collect(),
                )
            }
            // The chosen clip under the playhead, video first.
            "freeze_frame" => {
                let at = self.playhead();
                let video = |t: TrackId| seq.track(t).is_some_and(|t| t.kind == TrackKind::Video);
                let under = chosen.iter().filter(|(_, c)| c.start <= at && at < c.end());
                let (_, c) = under.min_by_key(|(t, _)| !video(*t))?;
                Command::FreezeFrame { id: c.id, at }
            }
            _ => return None,
        };
        if matches!(cmd, Command::RemoveClips { .. }) {
            self.selected.clear();
        }
        Some(cmd)
    }

    /// Ctrl+K splits the selected clips under the playhead, or every unlocked track's when
    /// none are selected. Q/W ripple trim the clips under it to the playhead, Q then moving
    /// the playhead back to where they now start.
    fn cut_action(&mut self, id: &str, seq: &Sequence) -> Option<Command> {
        let at = self.playhead();
        let unlocked = |t: TrackId| seq.track(t).is_some_and(|t| !t.locked);
        let under = seq
            .clips_at(at)
            .filter(|(t, c)| c.start < at && unlocked(*t));
        let under: Vec<_> = under.map(|(_, c)| c).collect();
        let cmd = match id {
            "add_edit" => {
                let picked = under.iter().filter(|c| {
                    let partners = || seq.linked(c.id).into_iter();
                    let partners = self.linked_selection
                        && partners().any(|(_, p)| self.selected.contains(&p.id));
                    self.selected.contains(&c.id) || partners
                });
                let ids: Vec<_> = if self.selected.is_empty() {
                    under.iter().map(|c| c.id).collect()
                } else {
                    picked.map(|c| c.id).collect()
                };
                (!ids.is_empty()).then_some(Command::Split { ids, at })?
            }
            _ if under.is_empty() => return None,
            "ripple_trim_prev" => {
                let back = under.iter().map(|c| c.start).min().expect("not empty");
                let trims = under.iter().map(|c| (c.id, Edge::Head, at - c.start));
                let cmd = Command::Trim {
                    trims: trims.collect(),
                    ripple: true,
                };
                self.seek_after = Some(back);
                cmd
            }
            _ => Command::Trim {
                trims: under
                    .iter()
                    .map(|c| (c.id, Edge::Tail, at - c.end()))
                    .collect(),
                ripple: true,
            },
        };
        Some(cmd)
    }

    /// Edge grips inside a clip in the selection tool; dragging one trims, Ctrl ripples.
    /// Returns the edit on release.
    fn trim_handles(
        &mut self,
        ui: &Ui,
        project: &Project,
        view: View,
        clip: &Clip,
        r: Rect,
    ) -> Option<Edit> {
        if self.razor || locked(project.sequence(), clip.id) {
            return None;
        }
        let w = 6.0f32.min(r.width() / 3.0);
        let rate = self.transport.rate;
        for (edge, zone) in [
            (Edge::Head, r.left()..=r.left() + w),
            (Edge::Tail, r.right() - w..=r.right()),
        ] {
            let zone = Rect::from_x_y_ranges(zone, r.y_range());
            let key = ("trim", clip.id, edge == Edge::Head);
            let h = ui.interact(zone, ui.id().with(key), Sense::drag());
            if h.hovered() || h.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            if h.is_pointer_button_down_on() {
                self.holding = Some(clip.id);
            }
            if h.dragged()
                && let Some(p) = h.interact_pointer_pos()
            {
                let by = view.time(p.x) - self.press_time(ui, view, p);
                let by = rate.time_floor(rate.index_nearest(by));
                let at = match edge {
                    Edge::Head => clip.start + by,
                    Edge::Tail => clip.end() + by,
                };
                let seq = project.sequence();
                let group: Vec<_> = seq.linked(clip.id).iter().map(|(_, c)| c.id).collect();
                let by = by + self.snap(seq, &group, &[at]);
                let ripple = ui.input(|i| i.modifiers.command);
                let by = project.trim_limit(clip.id, edge, by, ripple);
                let (clip, ripple) = (clip.id, ripple);
                self.trim = Some(Trim {
                    clip,
                    edge,
                    by,
                    ripple,
                });
            }
            if h.drag_stopped() {
                self.snapped = None;
            }
            if h.drag_stopped()
                && let Some(t) = self.trim.take()
                && t.by != Time(0)
            {
                let trims = vec![(t.clip, t.edge, t.by)];
                let ripple = t.ripple;
                return Some((Command::Trim { trims, ripple }, None));
            }
        }
        None
    }

    /// Shifts `edges` onto the nearest snap target within reach when snapping is on: other
    /// clips' edges (not `moving`'s), the playhead, markers and in/out. Records the hit.
    fn snap(&mut self, seq: &Sequence, moving: &[ClipId], edges: &[Time]) -> Time {
        self.snapped = None;
        if !self.snapping {
            return Time(0);
        }
        let mut targets = vec![self.playhead()];
        let flag = self.mark_drag.map(|(is_in, _)| is_in);
        if flag != Some(true) {
            targets.extend(seq.in_point());
        }
        if flag != Some(false) {
            targets.extend(seq.out_point());
        }
        for m in seq.markers() {
            targets.extend([m.time, m.time + m.duration]);
        }
        for t in seq.tracks() {
            let others = t.clips().iter().filter(|c| !moving.contains(&c.id));
            targets.extend(others.flat_map(|c| [c.start, c.end()]));
        }
        let reach = Time((SNAP_PX / self.zoom * FLICKS_PER_SECOND as f64) as i64);
        let hit = snap_shift(edges, &targets, reach);
        self.snapped = hit.map(|h| h.1);
        hit.map_or(Time(0), |h| h.0)
    }

    /// Where the trimmed clips would end up, as (track, start, end).
    fn trim_ghosts(&self, seq: &Sequence) -> Vec<(Option<TrackId>, Time, Time)> {
        let Some(t) = self.trim else {
            return Vec::new();
        };
        let span = |c: &Clip| match (t.edge, t.ripple) {
            (Edge::Head, false) => (c.start + t.by, c.end()),
            (Edge::Head, true) => (c.start, c.end() - t.by),
            (Edge::Tail, _) => (c.start, c.end() + t.by),
        };
        let group = seq.linked(t.clip).into_iter();
        group
            .map(|(tr, c)| (Some(tr), span(c).0, span(c).1))
            .collect()
    }

    /// The source frame at the edge being trimmed, for the viewer: the video partner's when
    /// the dragged clip is audio.
    pub fn trim_frame(&self, project: &Project) -> Option<(AssetId, Time)> {
        let t = self.trim?;
        let seq = project.sequence();
        let group = seq.linked(t.clip);
        let video = |tr: TrackId| seq.track(tr).is_some_and(|x| x.kind == TrackKind::Video);
        let c = group.iter().find(|(tr, _)| video(*tr)).or(group.first())?.1;
        let frame = match t.edge {
            Edge::Head => c.source_beyond(c.start + t.by),
            Edge::Tail => c.source_beyond(c.end() + t.by - self.transport.rate.time_floor(1)),
        };
        Some((c.asset, frame))
    }

    /// A click on empty lane space picks the gap there, if any.
    fn pick_gap(
        &mut self,
        seq: &Sequence,
        view: View,
        lanes: &[(TrackId, TrackKind, Rect)],
        p: Pos2,
    ) {
        self.keys.selected.clear();
        let lane = lanes.iter().find(|l| l.2.contains(p));
        let at = view.time(p.x);
        self.gap = lane
            .filter(|l| seq.gap_at(l.0, at).is_some())
            .map(|l| (l.0, at));
    }

    /// Razor click: splits the clip, and its linked partners under linked selection.
    fn razor_cut(&self, seq: &Sequence, view: View, clip: ClipId, x: f32) -> Edit {
        let rate = self.transport.rate;
        let at = rate.time_floor(rate.index_nearest(view.time(x)));
        let ids = if self.linked_selection {
            seq.linked(clip).into_iter().map(|(_, c)| c.id).collect()
        } else {
            vec![clip]
        };
        (Command::Split { ids, at }, None)
    }

    /// Click selection: the clip (and its partners under linked selection) replaces the
    /// selection, Shift adds it, Ctrl toggles it.
    fn select(&mut self, seq: &Sequence, id: ClipId, mods: egui::Modifiers) {
        let group: Vec<_> = if self.linked_selection {
            seq.linked(id).into_iter().map(|(_, c)| c.id).collect()
        } else {
            vec![id]
        };
        if mods.command && self.selected.contains(&id) {
            group.iter().for_each(|c| _ = self.selected.remove(c));
            self.gap = None;
            return;
        }
        if !mods.command && !mods.shift {
            self.selected.clear();
        }
        self.selected.extend(group);
        self.gap = None;
    }

    fn sync(&mut self, project: &Project) {
        // Clip ids are per sequence, so a selection doesn't carry over to another.
        if self.shown_sequence != project.sequence().id() {
            self.shown_sequence = project.sequence().id();
            self.selected.clear();
            self.keys.selected.clear();
        }
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

    /// An empty timeline on the same caches, for another project.
    pub fn sibling(&self) -> Self {
        let mut t = Self::default();
        t.media.caches = self.media.caches.clone();
        t
    }

    /// No filmstrip, waveform or thumbnail job is queued or running on the shared pool.
    #[cfg(test)]
    pub fn jobs_idle(&self) -> bool {
        self.media.caches.as_ref().is_none_or(|c| c.jobs.is_idle())
    }

    /// The x of `t` on lanes whose left edge is at `left`, at the current scroll and zoom.
    #[cfg(test)]
    pub fn x_at(&self, t: Time, left: f32) -> f32 {
        self.view(left).x(t)
    }

    fn view(&self, left: f32) -> View {
        View {
            left,
            scroll: self.scroll,
            zoom: self.zoom,
        }
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        missing: &HashSet<PathBuf>,
        keymap: &Keymap,
    ) -> Option<Edit> {
        let mut edit = self.pending.take().map(|c| (c, None));
        if std::mem::take(&mut self.copied) {
            // Ctrl+V only reaches us as a paste event when the system clipboard has text.
            let n = self.clipboard.len();
            ui.ctx().copy_text(trn!("timeline.copied", n));
        }
        self.sync(project);
        if self.keys.graph.is_some() {
            return edit.or(self.graph(ui, ui.max_rect(), project.sequence()));
        }
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
        self.width = lanes.width();
        self.follow();

        // Input first, so this frame draws the result.
        let view = self.view(lanes_left);
        let id = ui.id().with("timeline");
        let scrub = ui.interact(ruler, id.with("ruler"), Sense::click_and_drag());
        let marquee = ui.interact(lanes, id.with("marquee"), Sense::click_and_drag());
        let seq = project.sequence();
        for (is_in, mark) in [(true, seq.in_point()), (false, seq.out_point())] {
            let Some(mark) = mark else { continue };
            let x = view.x(mark);
            let flag = Rect::from_x_y_ranges(x - 5.0..=x + 5.0, ruler.y_range());
            let h = ui.interact(flag, id.with(("mark", is_in)), Sense::drag());
            if h.hovered() || h.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            if h.dragged()
                && let Some(p) = h.interact_pointer_pos()
            {
                let t = rate.time_floor(rate.index_nearest(view.time(p.x)).max(0));
                // Set first so the snap skips this flag's own mark.
                self.mark_drag = Some((is_in, t));
                let t = t + self.snap(seq, &[], &[t]);
                // Held a frame clear of the other mark.
                let t = match (is_in, seq.in_point(), seq.out_point()) {
                    (true, _, Some(o)) => t.min(rate.time_floor(rate.index_floor(o) - 1)),
                    (false, Some(i), _) => t.max(rate.time_floor(rate.index_floor(i) + 1)),
                    _ => t,
                };
                self.mark_drag = Some((is_in, t));
            }
            if h.drag_stopped() {
                self.snapped = None;
            }
            if h.drag_stopped()
                && let Some((_, t)) = self.mark_drag.take()
                && t != mark
            {
                let (in_point, out_point) = if is_in {
                    (Some(t), seq.out_point())
                } else {
                    (seq.in_point(), Some(t))
                };
                edit = edit.or(Some((
                    Command::SetInOut {
                        in_point,
                        out_point,
                    },
                    None,
                )));
            }
        }
        let head_x = view.x(self.playhead());
        let grip = Rect::from_x_y_ranges(head_x - 4.0..=head_x + 4.0, lanes.y_range());
        let drag = ui.interact(grip, id.with("playhead"), Sense::drag());
        if let Some(p) = scrub.interact_pointer_pos().or(drag.interact_pointer_pos())
            && (scrub.is_pointer_button_down_on() || drag.dragged())
        {
            self.scrub(ui, p.x, lanes);
        }
        if drag.hovered() || drag.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        let tracks = project.sequence().tracks();
        let content_h: f32 = tracks.iter().map(|t| t.height + self.keys.extra(t)).sum();
        // Not `hovered()`: clips, the marquee and buttons sit on top of the body.
        if ui.rect_contains_pointer(area)
            && let Some(p) = ui.input(|i| i.pointer.hover_pos())
        {
            let (zoom, mut scroll, shift) =
                ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.modifiers.shift));
            // egui maps Shift+wheel to x itself; cover backends that don't.
            if shift && scroll.x == 0.0 {
                scroll = egui::vec2(scroll.y, 0.0);
            }
            if zoom != 1.0 {
                self.zoom_by(zoom as f64, view, p.x.max(lanes_left));
            } else {
                self.scroll = (self.scroll - scroll.x as f64 / self.zoom).max(0.0);
                self.vscroll -= scroll.y;
            }
        }
        let (pressed, down, dt) =
            ui.input(|i| (i.pointer.any_pressed(), i.pointer.any_down(), i.stable_dt));
        if pressed {
            self.press_scroll = (self.scroll, self.vscroll);
        }
        // Drags that began in the lanes, and bin drags over the timeline, scroll at the
        // edges; ruler and playhead scrubs scroll themselves, and a refused drag stays put.
        let scrubbing = scrub.is_pointer_button_down_on() || drag.dragged();
        let refused = self.holding.is_some_and(|c| locked(seq, c));
        let ours = !refused
            && ui.ctx().dragged_id().is_some()
            && ui
                .input(|i| i.pointer.press_origin())
                .is_some_and(|o| lanes.contains(o));
        let bin = egui::DragAndDrop::has_payload_of_type::<AssetId>(ui.ctx())
            && ui.rect_contains_pointer(area);
        if !scrubbing
            && (ours || bin)
            && let Some(p) = ui.input(|i| i.pointer.interact_pos())
        {
            let v = edge_speed(p.y, lanes.top(), lanes.bottom()) * dt;
            let (dx, dy) = (edge_speed(p.x, lanes.left(), lanes.right()) * dt, v);
            let dy = if content_h > lanes.height() { dy } else { 0.0 };
            if dx != 0.0 || dy != 0.0 {
                let was = (self.scroll, self.vscroll);
                self.scroll += dx as f64 / self.zoom;
                self.vscroll += dy;
                self.vscroll = self
                    .vscroll
                    .clamp(0.0, (content_h - lanes.height()).max(0.0));
                self.clamp_scroll();
                // Only while it moves, so a drag held at a clamped edge can idle.
                if (self.scroll, self.vscroll) != was {
                    ui.ctx().request_repaint();
                }
            }
        }
        self.vscroll = self
            .vscroll
            .clamp(0.0, (content_h - lanes.height()).max(0.0));
        self.clamp_scroll();
        let view = self.view(lanes_left);
        let (start, end) = (view.time(lanes.left()), view.time(lanes.right()));

        let painter = ui.painter_at(area);
        // Lanes, clipped below the ruler.
        let lane_painter = painter.with_clip_rect(lanes.union(headers).intersect(area));
        let clip_painter = painter.with_clip_rect(lanes.intersect(area));
        lane_painter.rect_filled(lanes, 0.0, color::field());
        // Empty header space: add tracks.
        ui.interact(headers, id.with("headers"), Sense::click())
            .context_menu(|ui| {
                add_menu(ui, &mut edit);
                ui.separator();
                ui.checkbox(&mut self.linked_selection, tr!("timeline.linked_selection"));
            });
        let spans = self.silence_spans(project, ui.ctx());
        // Each span on every clip it cuts, linked partners included, for the shading.
        let shaded: Vec<_> = spans
            .iter()
            .flatten()
            .flat_map(|&(id, a, b)| seq.linked(id).into_iter().map(move |(_, c)| (c.id, a, b)))
            .collect();
        // A video asset dragged at the top edge opens a slot for a new track above the rest.
        let top_y = lanes.top() - self.vscroll;
        let video = egui::DragAndDrop::payload::<AssetId>(ui.ctx()).is_some_and(|a| {
            let pl = project.placement(*a, Time(0), false, None, false);
            pl.is_some_and(|pl| pl.video.is_some() || pl.add_video)
        });
        let open = video
            && !tracks.is_empty()
            && ui
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|p| lanes.contains(p) && p.y < top_y + self.slot + SLOT_PX);
        // Reduce Motion zeroes the style's animation time; the slot then snaps.
        let t = match () {
            _ if ui.style().animation_time == 0.0 => 0.0,
            _ if open => 0.15,
            _ => 0.1,
        };
        let slot_id = id.with("slot");
        let ease = egui::emath::easing::cubic_out;
        let shown = ui
            .ctx()
            .animate_bool_with_time_and_easing(slot_id, open, t, ease);
        self.slot = shown * Track::default_height();
        let slot = Rect::from_x_y_ranges(lanes.x_range(), top_y..=top_y + self.slot);
        if self.slot > 0.0 {
            let tint = color::blue().gamma_multiply(0.12 * shown);
            lane_painter.rect_filled(slot.with_min_x(area.left()), 0.0, tint);
            lane_painter.rect_filled(
                Rect::from_x_y_ranges(area.x_range(), slot.bottom() - line..=slot.bottom()),
                0.0,
                color::border(),
            );
            let name = slot.with_min_x(headers.left()).with_max_x(headers.right());
            lane_painter
                .with_clip_rect(name.intersect(lanes.union(headers)))
                .text(
                    pos2(name.left() + space::S8, name.center().y),
                    Align2::LEFT_CENTER,
                    tr!("timeline.new_track"),
                    theme::body(),
                    color::light_blue().gamma_multiply(shown),
                );
        }
        let mut y = top_y + self.slot;
        let mut lane_rects = Vec::new();
        // The clip being dragged, its track, and its response.
        let mut held = None;
        let mut shown_clips = Vec::new();
        for (i, track) in tracks.iter().enumerate() {
            let extra = self.keys.extra(track);
            let lane = Rect::from_x_y_ranges(lanes.x_range(), y..=y + track.height + extra);
            y += track.height + extra;
            lane_rects.push((track.id, track.kind, lane));
            let holds = track.clips().iter().any(|c| Some(c.id) == self.holding);
            if (lane.bottom() < lanes.top() || lane.top() > lanes.bottom()) && !holds {
                continue;
            }
            let header = Rect::from_x_y_ranges(headers.x_range(), lane.y_range());
            if let Some(e) = self.header(ui, &lane_painter, header, headers, project, i) {
                edit = Some(e);
            }
            lane_painter.rect_filled(
                Rect::from_x_y_ranges(area.x_range(), lane.bottom() - line..=lane.bottom()),
                0.0,
                color::border(),
            );
            let shown = visible(track.clips(), start, end);
            let held_clip = track.clips().iter().find(|c| Some(c.id) == self.holding);
            let off = held_clip.filter(|c| !shown.iter().any(|v| v.id == c.id));
            for clip in shown.iter().chain(off) {
                // Clamped just past the lanes so far off-screen ends don't make huge coordinates.
                let (l, r) = (lanes.left() - 8.0, lanes.right() + 8.0);
                let x = view.x(clip.start).clamp(l, r)..=view.x(clip.end()).clamp(l, r);
                // 2px gaps between clips and 2px above and below inside the lane.
                let rows = lane.top()..=lane.top() + track.height;
                let r = Rect::from_x_y_ranges(x, rows).shrink2(vec2(1.0, 2.0));
                let asset = project.asset(clip.asset);
                let offline = asset.is_some_and(|a| missing.contains(&a.path));
                // Clips on hidden tracks and disabled clips drop to 40%.
                let mut painter = clip_painter.clone();
                if track.hidden || !clip.enabled {
                    painter.multiply_opacity(0.4);
                }
                self.paint_clip(
                    ui, &painter, r, lanes, view, clip, asset, track.kind, offline,
                );
                // What Detect Silence would take out sinks back toward the ground.
                for &(_, a, b) in shaded.iter().filter(|s| s.0 == clip.id) {
                    let x = view.x(a).max(r.left())..=view.x(b).min(r.right());
                    let dim = color::ground().gamma_multiply(0.7);
                    painter.rect_filled(Rect::from_x_y_ranges(x, r.y_range()), 0.0, dim);
                }
                if self.selected.contains(&clip.id) {
                    let corner = CornerRadius::same(radius::SMALL);
                    let stroke = Stroke::new(2.0, color::light_blue());
                    painter.rect_stroke(r, corner, stroke, egui::StrokeKind::Inside);
                }
                shown_clips.push((clip.id, r));
                let cr = ui.interact(r, id.with(("clip", clip.id)), Sense::click_and_drag());
                if self.razor && cr.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                }
                let band = (track.kind == TrackKind::Audio && !offline).then(|| {
                    let body =
                        Rect::from_x_y_ranges(r.x_range(), r.top() + NAME_STRIP..=r.bottom());
                    self.rubber_band(ui, &painter, view, clip, body, &cr, id)
                });
                if let Some(e) = band.flatten() {
                    edit = Some(e);
                } else if cr.clicked()
                    && self.razor
                    && let Some(p) = cr.interact_pointer_pos()
                {
                    edit = Some(self.razor_cut(seq, view, clip.id, p.x));
                } else if cr.clicked() {
                    self.keys.selected.clear();
                    self.select(seq, clip.id, ui.input(|i| i.modifiers));
                }
                if cr.secondary_clicked() && !self.selected.contains(&clip.id) {
                    self.select(seq, clip.id, egui::Modifiers::NONE);
                }
                cr.context_menu(|ui| self.clip_menu(ui, seq, keymap, clip.asset));
                if let Some(e) = self.trim_handles(ui, project, view, clip, r) {
                    edit = Some(e);
                }
                // Fades and keys change only this clip, so only its own track's lock counts.
                let fixed = track.locked;
                if let Some(e) = self.fade_handles(ui, &painter, view, clip, r, id, fixed) {
                    edit = Some(e);
                }
                if let Some(e) = self.transition_block(ui, &painter, project, view, clip, r) {
                    edit = Some(e);
                }
                // A locked clip's keys can't change, so its lanes and their toggle hide until
                // the track unlocks.
                if !fixed {
                    self.key_toggle(ui, &painter, view, clip, r, id);
                }
                if !fixed && self.keys.expanded.contains(&clip.id) {
                    let under =
                        Rect::from_x_y_ranges(r.x_range(), r.bottom() + 2.0..=lane.bottom());
                    if let Some(e) = self.key_lanes(ui, &painter, seq, view, clip, under, id) {
                        edit = Some(e);
                    }
                }
                if cr.is_pointer_button_down_on() {
                    self.holding = Some(clip.id);
                }
                // A clip tied to a locked track can't move: no ghost, a refusal instead.
                if cr.dragged() && locked(seq, clip.id) {
                    ui.ctx().set_cursor_icon(CursorIcon::NotAllowed);
                    egui::Tooltip::for_widget(&cr)
                        .at_pointer()
                        .show(|ui| ui.label(tr!("timeline.track_locked")));
                } else if cr.dragged() || cr.drag_stopped() {
                    held = Some((clip, track, cr));
                }
            }
            if self.armed == Some(track.id)
                && let Some((live, end)) = self.live_span()
            {
                let (l, r) = (lanes.left() - 8.0, lanes.right() + 8.0);
                let x = view.x(live.start).clamp(l, r)..=view.x(end).clamp(l, r);
                let rows = lane.top()..=lane.top() + track.height;
                let r = Rect::from_x_y_ranges(x, rows).shrink2(vec2(1.0, 2.0));
                paint_live(&clip_painter, r, lanes, view, live);
            }
        }
        // After the clips, so the release frame still reaches an off-screen held clip.
        if !down {
            self.holding = None;
            // A key drag whose clip scrolled away never sees its drag_stopped.
            self.keys.drag = None;
        }
        let mods = ui.input(|i| i.modifiers);
        if marquee.clicked() && !mods.shift && !mods.command {
            self.selected.clear();
        }
        if marquee.clicked()
            && let Some(p) = marquee.interact_pointer_pos()
        {
            self.pick_gap(seq, view, &lane_rects, p);
        }
        if marquee.secondary_clicked()
            && let Some(p) = marquee.interact_pointer_pos()
        {
            self.selected.clear();
            self.pick_gap(seq, view, &lane_rects, p);
        }
        marquee.context_menu(|ui| self.lane_menu(ui, seq, keymap, &mut edit));
        if let Some((t, at)) = self.gap
            && let Some((a, b)) = seq.gap_at(t, at)
            && let Some((_, _, lane)) = lane_rects.iter().find(|l| l.0 == t)
        {
            let r = Rect::from_x_y_ranges(view.x(a)..=view.x(b), lane.y_range())
                .shrink2(vec2(1.0, 2.0));
            clip_painter.rect_filled(r, 0.0, color::blue().gamma_multiply(0.2));
        }
        if let (Some(a), Some(b)) = (
            ui.input(|i| i.pointer.press_origin()),
            marquee.interact_pointer_pos(),
        ) && (marquee.dragged() || marquee.drag_stopped())
        {
            // The press point moves with the content as the view auto-scrolls.
            let (ps, pv) = self.press_scroll;
            let a = a + vec2(((ps - self.scroll) * self.zoom) as f32, pv - self.vscroll);
            let band = Rect::from_two_pos(a, b);
            if marquee.drag_stopped() {
                self.keys.selected.clear();
                if !mods.shift && !mods.command {
                    self.selected.clear();
                }
                let hit = shown_clips.iter().filter(|(_, r)| r.intersects(band));
                self.selected.extend(hit.map(|(c, _)| *c));
            } else {
                clip_painter.rect_filled(band, 0.0, color::blue().gamma_multiply(0.15));
                let stroke = Stroke::new(1.0, color::light_blue());
                clip_painter.rect_stroke(band, 0.0, stroke, egui::StrokeKind::Inside);
            }
        }
        // A visible way to add tracks, under the last header.
        let plus = Rect::from_min_size(
            pos2(headers.left() + space::S4, y.max(lanes.top()) + space::S4),
            vec2(headers.width() - 2.0 * space::S4, size::CONTROL),
        );
        if plus.bottom() <= lanes.bottom() {
            let r = ui.put(
                plus,
                Button::ghost(tr!("timeline.track")).with_icon(icon::PLUS),
            );
            egui::Popup::menu(&r).show(|ui| add_menu(ui, &mut edit));
        }
        let moving = held.as_ref().map(|h| h.0.id).or(self.trim.map(|t| t.clip));
        let moving: Vec<_> = moving.map_or(Vec::new(), |id| {
            seq.linked(id).iter().map(|(_, c)| c.id).collect()
        });
        let mut top = false;
        let mut ghosts = self.drops(
            ui,
            project,
            view,
            lanes,
            &lane_rects,
            held,
            &mut edit,
            &mut top,
            open,
        );
        ghosts.extend(self.trim_ghosts(seq));
        // Dropped into the slot: the new track takes its place, so it closes at once.
        if open && edit.is_some() {
            ui.ctx().animate_bool_with_time(slot_id, false, 0.0);
        }
        // Where a drop's new video track goes in: the top of the stack.
        if top && !open {
            let y = (lanes.top() - self.vscroll).max(lanes.top());
            let r = Rect::from_x_y_ranges(lanes.x_range(), y..=y + 2.0);
            clip_painter.rect_filled(r, 0.0, color::light_blue());
        }
        // Ghosts for tracks a drop will add go in rows below the last lane, a new top video
        // track's (it comes first) in the open slot.
        let mut below = y;
        let mut slot_rows = (top && open).then(|| slot.y_range());
        for (t, from, to) in ghosts {
            let rows = match t {
                Some(t) => lane_rects.iter().find(|l| l.0 == t).map(|l| l.2.y_range()),
                None if slot_rows.is_some() => slot_rows.take(),
                None => {
                    below += Track::default_height();
                    Some((below - Track::default_height()..=below).into())
                }
            };
            if let Some(rows) = rows {
                let x = view.x(from)..=view.x(to);
                let r = Rect::from_x_y_ranges(x, rows).shrink2(vec2(1.0, 2.0));
                let corner = CornerRadius::same(radius::SMALL);
                clip_painter.rect_filled(r, corner, color::blue().gamma_multiply(0.3));
                clip_painter.rect_stroke(
                    r,
                    corner,
                    Stroke::new(1.0, color::light_blue()),
                    egui::StrokeKind::Inside,
                );
                // What the drop overwrites on that track, in red.
                let track = t.and_then(|t| seq.track(t));
                let hit = track.into_iter().flat_map(|tr| tr.clips());
                let hit = hit.filter(|c| !moving.contains(&c.id));
                for c in hit.filter(|c| c.start < to && c.end() > from) {
                    let x = view.x(c.start.max(from))..=view.x(c.end().min(to));
                    let o = Rect::from_x_y_ranges(x, rows).shrink2(vec2(1.0, 2.0));
                    clip_painter.rect_filled(o, 0.0, color::red().gamma_multiply(0.45));
                }
            }
        }
        // The in/out range, a missing mark standing for the start or end.
        let (mut mark_in, mut mark_out) = (seq.in_point(), seq.out_point());
        match self.mark_drag {
            Some((true, t)) => mark_in = Some(t),
            Some((false, t)) => mark_out = Some(t),
            None => {}
        }
        let range = (mark_in.is_some() || mark_out.is_some()).then(|| {
            let (a, b) = (
                mark_in.unwrap_or_default(),
                mark_out.unwrap_or(seq.duration()),
            );
            Rect::from_x_y_ranges(view.x(a)..=view.x(b), area.y_range())
        });
        if let Some(r) = range {
            clip_painter.rect_filled(r, 0.0, color::light_blue().gamma_multiply(0.08));
        }
        if let Some(t) = self.snapped {
            let x = view.x(t);
            let r = Rect::from_x_y_ranges(x - 0.5..=x + 0.5, lanes.y_range());
            clip_painter.rect_filled(r, 0.0, color::amber());
        }
        if tracks.is_empty() {
            painter.text(
                lanes.center(),
                Align2::CENTER_CENTER,
                tr!("timeline.empty"),
                theme::body(),
                color::ink_3(),
            );
        }

        // Ruler.
        painter.rect_filled(ruler, 0.0, color::panel());
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
                color::border_strong(),
            );
            if is_major {
                painter.text(
                    pos2(x + 3.0, ruler.top() + 2.0),
                    Align2::LEFT_TOP,
                    format_timecode(f as u64, rate),
                    theme::small(),
                    color::ink_3(),
                );
            }
            f += minor;
        }
        painter.rect_filled(
            Rect::from_x_y_ranges(area.x_range(), ruler.bottom() - line..=ruler.bottom()),
            0.0,
            color::border(),
        );
        let ruler_painter = painter.with_clip_rect(ruler);
        if let Some(r) = range {
            let band = Rect::from_x_y_ranges(r.x_range(), ruler.y_range());
            ruler_painter.rect_filled(band, 0.0, color::light_blue().gamma_multiply(0.25));
        }
        edit = edit.or(self.markers(ui, seq, view, ruler, id));
        // Flags: a bracket at each set mark, opening toward the range.
        for (mark, dir) in [(mark_in, 1.0), (mark_out, -1.0)] {
            if let Some(t) = mark {
                let x = view.x(t);
                let stroke = Stroke::new(2.0, color::light_blue());
                let (top, bottom) = (ruler.top() + 2.0, ruler.bottom() - 1.0);
                ruler_painter.line_segment([pos2(x, top), pos2(x, bottom)], stroke);
                for y in [top, bottom] {
                    ruler_painter.line_segment([pos2(x, y), pos2(x + 5.0 * dir, y)], stroke);
                }
            }
        }

        // Corner cell: the playhead's timecode and the zoom slider.
        let playhead = self.playhead();
        painter.text(
            pos2(area.left() + space::INSET, ruler.center().y),
            Align2::LEFT_CENTER,
            format_timecode(rate.index_floor(playhead).max(0) as u64, rate),
            theme::body_strong(),
            color::ink(),
        );
        let slider = Rect::from_x_y_ranges(
            lanes_left - 64.0..=lanes_left - space::INSET,
            ruler.y_range(),
        );
        // Logarithmic: the suite slider is linear, so it moves the zoom's log.
        let mut z = self.zoom.ln();
        let r = ui.put(
            slider,
            suite_ui::widgets::Slider::new(
                &mut z,
                MIN_ZOOM.ln()..=MAX_ZOOM.ln(),
                tr!("timeline.zoom_tooltip"),
            ),
        );
        if r.changed() {
            let x = view.x(playhead).clamp(lanes.left(), lanes.right());
            self.zoom_by(z.exp() / self.zoom, view, x);
        }
        r.on_hover_text(tr!("timeline.zoom_tooltip"));

        self.scrollbars(ui, &painter, lanes, content_h, id);

        // Playhead: never eased, it reports time.
        let x = view.x(playhead);
        if lanes.x_range().contains(x) {
            let x = (x * ppp).round() / ppp;
            painter.rect_filled(
                Rect::from_x_y_ranges(x..=x + line, ruler.top() + 12.0..=area.bottom()),
                0.0,
                color::light_blue(),
            );
            playhead_head(&painter, pos2(x + line / 2.0, ruler.top()));
        }
        if let Some(s) = &mut self.silence {
            let at = pos2(area.right() - space::S16, area.top() - space::S8);
            let (cmd, open) = s.show(ui, at, spans.as_deref());
            if !open {
                self.silence = None;
            }
            edit = edit.or(cmd.map(|c| (c, None)));
        }
        if let Some(id) = self.menu.take() {
            self.action(id, project);
            edit = edit.or(self.pending.take().map(|c| (c, None)));
        }
        edit
    }

    /// The open Detect Silence dialog's spans, loading the peaks it reads; `None` while it's
    /// closed or they load.
    fn silence_spans(
        &mut self,
        project: &Project,
        ctx: &egui::Context,
    ) -> Option<Vec<silence::Span>> {
        let s = self.silence.as_ref()?;
        let seq = project.sequence();
        // shortcut: a merged clip reads its first stream, as its waveform draws.
        let key = |c: &Clip| Some((project.asset(c.asset)?, c.stream.unwrap_or(0) as usize));
        for &id in s.clips() {
            if let Some((a, nth)) = seq.clip(id).and_then(|(_, c)| key(c)) {
                self.media.pyramid(ctx, a, nth);
            }
        }
        let media = &self.media;
        // shortcut: rescans every clip's peaks each frame; cache by settings if hour-long
        // recordings make the dialog lag.
        s.spans(seq, &|c| {
            let (a, nth) = key(c)?;
            let p = media.peaks.get(&(a.path.clone(), nth))?.as_ref()?;
            Some(p.finest())
        })
    }

    /// The sequence time under the press origin, as the view stood when it was pressed.
    fn press_time(&self, ui: &Ui, view: View, p: Pos2) -> Time {
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
        let then = View {
            scroll: self.press_scroll.0,
            ..view
        };
        then.time(origin.x)
    }

    /// Seconds of sequence the lanes show.
    fn span(&self) -> f64 {
        self.width as f64 / self.zoom
    }

    /// No further than one viewport past the end of the sequence (and its tail).
    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.clamp(0.0, secs(self.transport.duration));
    }

    /// Fits the whole sequence across the lanes.
    fn zoom_fit(&mut self, seq: &Sequence) {
        let end = secs(seq.duration());
        if end > 0.0 {
            self.zoom = (self.width as f64 * 0.95 / end).clamp(MIN_ZOOM, MAX_ZOOM);
            self.scroll = 0.0;
        }
    }

    /// When the playhead moved (playback, seeks, steps) out of view, pages the view so it
    /// sits at the left edge. A playhead that stays put never overrides manual scrolling.
    fn follow(&mut self) {
        let t = self.playhead();
        if std::mem::replace(&mut self.followed, t) == t {
            return;
        }
        let s = secs(t);
        if s < self.scroll || s >= self.scroll + self.span() {
            self.scroll = s;
            self.clamp_scroll();
        }
    }

    /// Seeks to pointer x while scrubbing, scrolling when it's dragged past either edge.
    fn scrub(&mut self, ui: &Ui, x: f32, lanes: Rect) {
        let inside = x.clamp(lanes.left(), lanes.right());
        if x != inside {
            // Faster the further past the edge.
            self.scroll += (x - inside) as f64 * 0.2 / self.zoom;
            self.clamp_scroll();
            ui.ctx().request_repaint();
        }
        self.seek(self.view(lanes.left()).time(inside));
        // Scrubbing scrolls itself; don't page as well.
        self.followed = self.playhead();
    }

    /// Horizontal and (when tracks overflow) vertical scrollbars along the lanes' edges.
    fn scrollbars(
        &mut self,
        ui: &Ui,
        painter: &egui::Painter,
        lanes: Rect,
        content_h: f32,
        id: egui::Id,
    ) {
        let thick = 10.0;
        let tall = content_h > lanes.height();
        let right = if tall {
            lanes.right() - thick
        } else {
            lanes.right()
        };
        let span = self.span();
        let total = secs(self.transport.duration).max(self.scroll + span);
        let bar = Rect::from_x_y_ranges(
            lanes.left()..=right,
            lanes.bottom() - thick..=lanes.bottom(),
        );
        self.scroll = scrollbar(
            ui,
            painter,
            bar,
            id.with("hbar"),
            self.scroll,
            span,
            total,
            true,
        );
        if tall {
            let bar = Rect::from_x_y_ranges(right..=lanes.right(), lanes.y_range());
            let (h, v) = (lanes.height() as f64, self.vscroll as f64);
            self.vscroll = scrollbar(
                ui,
                painter,
                bar,
                id.with("vbar"),
                v,
                h,
                content_h as f64,
                false,
            ) as f32;
        }
    }

    /// Bin drops and clip drags: the ghosts to draw as (track, start, end), and the edit
    /// once the pointer is released. `top` is set when a bin drop adds a video track on top;
    /// `slot` drops into the open New Track slot.
    #[allow(clippy::too_many_arguments)]
    fn drops(
        &mut self,
        ui: &Ui,
        project: &Project,
        view: View,
        area: Rect,
        lanes: &[(TrackId, TrackKind, Rect)],
        held: Option<(&Clip, &Track, egui::Response)>,
        edit: &mut Option<Edit>,
        top: &mut bool,
        slot: bool,
    ) -> Vec<(Option<TrackId>, Time, Time)> {
        let rate = self.transport.rate;
        let snap = |t: Time| rate.time_floor(rate.index_nearest(t)).max(Time(0));
        let Some(p) = ui.input(|i| i.pointer.interact_pos()) else {
            return Vec::new();
        };
        let lane_at = |kind: Option<TrackKind>| {
            let hit = lanes.iter().find(|l| l.2.contains(p));
            hit.filter(|l| kind.is_none_or(|k| k == l.1)).map(|l| l.0)
        };
        if let Some(fx) = egui::DragAndDrop::payload::<crate::effects::Drag>(ui.ctx()) {
            let t = view.time(p.x);
            let clip = lane_at(Some(TrackKind::Video))
                .and_then(|id| project.sequence().track(id))
                .and_then(|tr| tr.clips().iter().find(|c| c.start <= t && t < c.end()));
            if area.contains(p)
                && ui.input(|i| i.pointer.any_released())
                && let Some(c) = clip
                && let Some(cmd) = crate::inspector::add_effect(project, &[c.id].into(), &fx.0)
            {
                *edit = Some((cmd, None));
            }
            return Vec::new();
        }
        if let Some(asset) = egui::DragAndDrop::payload::<AssetId>(ui.ctx())
            && area.contains(p)
            // Alt merges a multi-stream file's audio into one clip.
            && let merge = ui.input(|i| i.modifiers.alt)
            && let on = lane_at(None).filter(|_| !slot)
            && let Some(mut pl) = project.placement(*asset, snap(view.time(p.x)), false, on, merge)
        {
            *top = on.is_none() && onto_new_track(&mut pl);
            let end = pl.start + (pl.source_out - pl.source_in);
            let ghosts = [(pl.video, pl.add_video), (pl.audio, pl.add_audio)].into_iter();
            let ghosts = ghosts.filter(|(t, add)| t.is_some() || *add);
            let ghosts = ghosts.map(|(t, _)| (t, pl.start, end)).collect();
            if ui.input(|i| i.pointer.any_released()) {
                *edit = Some((Command::Place(pl), None));
            }
            return ghosts;
        }
        let Some((clip, from, r)) = held else {
            return Vec::new();
        };
        if r.drag_stopped() {
            self.snapped = None;
            if let Some((to, start)) = self.drag_to.take()
                && (start != clip.start || to != from.id)
            {
                let (id, track) = (clip.id, to);
                *edit = Some((Command::DragClip { id, track, start }, None));
            }
            return Vec::new();
        }
        let start = snap(clip.start + (view.time(p.x) - self.press_time(ui, view, p)));
        let seq = project.sequence();
        let group = seq.linked(clip.id);
        let ids: Vec<_> = group.iter().map(|(_, c)| c.id).collect();
        let edges: Vec<_> = group.iter().flat_map(|(_, c)| [c.start, c.end()]).collect();
        let edges: Vec<_> = edges.iter().map(|e| *e + start - clip.start).collect();
        let start = (start + self.snap(seq, &ids, &edges)).max(Time(0));
        let to = lane_at(Some(from.kind)).unwrap_or(from.id);
        self.drag_to = Some((to, start));
        // Linked clips follow on their own tracks; the core clamps the move at zero.
        let by = start - clip.start;
        group
            .into_iter()
            .map(|(t, c)| {
                (
                    Some(if c.id == clip.id { to } else { t }),
                    c.start + by,
                    c.end() + by,
                )
            })
            .collect()
    }

    /// One track's header: ID, name or rename field, toggles, menu and height grip.
    /// `track`'s Audio Effects beside its `header`, open until its close button, Escape or a
    /// click outside it.
    fn fx_popover(&mut self, ui: &Ui, header: Rect, track: &Track, label: &str) -> Option<Edit> {
        let id = ui.id().with(("fx popover", track.id));
        // Picking from its Preset menu clicks outside it, so that click mustn't close it.
        let menu = egui::Popup::is_any_open(ui.ctx());
        let area = ui.data(|d| d.get_temp::<Rect>(id)).unwrap_or(Rect::NOTHING);
        let away = ui.input(|i| {
            i.pointer.any_pressed() && i.pointer.press_origin().is_some_and(|p| !area.contains(p))
        });
        let mut open = !(away && !menu);
        let mut edit = None;
        let frame = egui::Frame::popup(ui.style()).inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 0,
            bottom: 4,
        });
        let shown = egui::Popup::new(id, ui.ctx().clone(), header, ui.layer_id())
            .open_bool(&mut open)
            .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
            .align(egui::RectAlign::RIGHT_START)
            .gap(space::S4)
            .frame(frame)
            .show(|ui| {
                ui.set_width(320.0 - 24.0);
                let (row, _) = ui
                    .allocate_exact_size(vec2(ui.available_width(), size::HEADER), Sense::hover());
                let p = ui.painter();
                let title = p.text(
                    pos2(row.left(), row.center().y),
                    Align2::LEFT_CENTER,
                    tr!("timeline.audio_effects"),
                    theme::body_strong(),
                    color::ink(),
                );
                let sub = match track.name.as_str() {
                    "" => label.to_owned(),
                    name if name == label => label.to_owned(),
                    name => format!("{label} {name}"),
                };
                let at = pos2(title.right() + 6.0, row.center().y);
                p.text(at, Align2::LEFT_CENTER, sub, theme::body(), color::ink_3());
                let x = Rect::from_center_size(
                    pos2(
                        row.right() + space::S4 - size::CONTROL / 2.0,
                        row.center().y,
                    ),
                    vec2(size::CONTROL, size::CONTROL),
                );
                let close = ui.put(
                    x,
                    Button::icon(icon::X, tr!("timeline.close_audio_effects")),
                );
                if let Some((f, gesture)) = crate::inspector::audio_fx(ui, &[track.fx]) {
                    let mut s = track.settings();
                    f(&mut s.fx);
                    edit = Some((Command::SetTrack(s), gesture.map(|g| g ^ track.id.0)));
                }
                close.clicked()
            });
        if let Some(r) = &shown {
            ui.data_mut(|d| d.insert_temp(id, r.response.rect));
        }
        if !open || shown.is_some_and(|r| r.inner) {
            self.fx_popover = None;
        }
        edit
    }

    fn header(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        rect: Rect,
        column: Rect,
        project: &Project,
        index: usize,
    ) -> Option<Edit> {
        let tracks = project.sequence().tracks();
        let track = &tracks[index];
        let id = ui.id().with(("track_header", track.id));
        let mut edit = None;
        let set = |f: &dyn Fn(&mut jvid_core::model::TrackSettings)| {
            let mut s = track.settings();
            f(&mut s);
            Some((Command::SetTrack(s), None))
        };
        // Clipped to the column so a part-scrolled header can't cover the ruler.
        let shown = rect.intersect(column);
        let body = ui.interact(shown, id, Sense::click());
        if body.hovered() {
            painter.rect_filled(rect, 0.0, color::raised().gamma_multiply(0.5));
        }

        // The ID; clicking it makes this the target for insert and overwrite.
        let label = track_label(tracks, index);
        let mid = rect.top() + rect.height().min(size::CONTROL + 8.0) / 2.0;
        let id_rect = Rect::from_center_size(
            pos2(rect.left() + space::INSET + 10.0, mid),
            vec2(28.0, size::SMALL_CONTROL),
        );
        let target = ui.interact(id_rect.intersect(column), id.with("target"), Sense::click());
        if track.target {
            painter.rect_filled(
                Rect::from_x_y_ranges(rect.left()..=rect.left() + 2.0, rect.y_range()),
                0.0,
                color::light_blue(),
            );
        }
        // Green while the track's EQ or compressor is on; the tooltip and label say so too.
        let fx = track.kind == TrackKind::Audio && !track.fx.is_default();
        let ink = match () {
            _ if track.target => color::light_blue(),
            _ if fx => color::green(),
            _ => color::ink(),
        };
        painter.text(
            id_rect.center(),
            Align2::CENTER_CENTER,
            &label,
            theme::body_strong(),
            ink,
        );
        target.widget_info(|| {
            let name = match fx {
                true => format!("{}, {}", tr!("timeline.target"), tr!("timeline.fx_on")),
                false => tr!("timeline.target").to_owned(),
            };
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, track.target, name)
        });
        if target.clicked() {
            edit = set(&|s| s.target = !s.target);
        }
        let tip = if track.target {
            tr!("timeline.target_on")
        } else {
            tr!("timeline.target_off")
        };
        let target = match fx {
            true => target.on_hover_text(format!("{tip}\n{}", tr!("timeline.fx_on"))),
            false => target.on_hover_text(tip),
        };
        if target.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }

        // Toggles, right-aligned: lock, hide or mute, solo.
        let (hide_glyph, hide_on, hide_font, hide_tip) = match track.kind {
            TrackKind::Video if track.hidden => (
                icon::EYE_OFF,
                true,
                theme::icon_font(),
                tr!("timeline.show"),
            ),
            TrackKind::Video => (icon::EYE, false, theme::icon_font(), tr!("timeline.hide")),
            // M and S are mixer notation, kept like keycaps.
            TrackKind::Audio => (
                verbatim("M"),
                track.muted,
                theme::body_strong(),
                tr!("timeline.mute"),
            ),
        };
        let toggles = [
            (
                icon::LOCK,
                track.locked,
                theme::icon_font(),
                tr!("timeline.lock"),
                false,
            ),
            (hide_glyph, hide_on, hide_font, hide_tip, false),
            (
                verbatim("S"),
                track.solo,
                theme::body_strong(),
                tr!("timeline.solo"),
                true,
            ),
        ];
        let step = size::SMALL_CONTROL + space::S4;
        let mut x = rect.right() - space::S8 - toggles.len() as f32 * step + space::S4;
        // Audio tracks put record arm before the rest, so the columns line up across kinds.
        let audio = track.kind == TrackKind::Audio;
        let arm_x = x - step;
        let name_right = if audio { arm_x } else { x } - space::S4;
        for (n, (glyph, on, font, tip, solo)) in toggles.into_iter().enumerate() {
            let r = Rect::from_min_size(
                pos2(x, mid - size::SMALL_CONTROL / 2.0),
                vec2(size::SMALL_CONTROL, size::SMALL_CONTROL),
            );
            x += step;
            if toggle(
                ui,
                painter,
                r,
                column,
                id.with(n),
                glyph,
                font,
                on,
                solo,
                tip,
            ) {
                edit = match n {
                    0 => set(&|s| s.locked = !s.locked),
                    1 if track.kind == TrackKind::Video => set(&|s| s.hidden = !s.hidden),
                    1 => set(&|s| s.muted = !s.muted),
                    _ => set(&|s| s.solo = !s.solo),
                };
            }
        }
        let armed = self.armed == Some(track.id);
        let level = self.level.filter(|_| armed);
        if audio {
            let r = Rect::from_min_size(
                pos2(arm_x, mid - size::SMALL_CONTROL / 2.0),
                vec2(size::SMALL_CONTROL, size::SMALL_CONTROL),
            );
            let tip = if level.is_some() {
                tr!("timeline.stop_recording")
            } else {
                tr!("timeline.arm")
            };
            let font = theme::body_strong();
            if toggle(
                ui,
                painter,
                r,
                column,
                id.with("arm"),
                "",
                font,
                armed,
                false,
                tip,
            ) {
                self.armed = (!armed).then_some(track.id);
            }
            // A Red dot once armed, a Red stop square while recording.
            let ink = match armed {
                true => color::red(),
                false if ui.rect_contains_pointer(r.intersect(column)) => color::ink_2(),
                false => color::ink_3(),
            };
            if level.is_some() {
                let square = Rect::from_center_size(r.center(), vec2(8.0, 8.0));
                painter.rect_filled(square, CornerRadius::same(2), ink);
            } else {
                painter.circle_filled(r.center(), 4.0, ink);
            }
        }
        if let Some(level) = level {
            // The input meter under the header row, -60 to 0 dBFS.
            let top = (mid + size::SMALL_CONTROL / 2.0 + 2.0).min(rect.bottom() - 4.0);
            let bar = Rect::from_x_y_ranges(
                rect.left() + space::INSET..=rect.right() - space::S8,
                top..=top + 3.0,
            );
            meter(painter, bar, level);
        }

        // Name, or the rename field.
        let name_rect =
            Rect::from_x_y_ranges(id_rect.right() + space::S4..=name_right, id_rect.y_range());
        if let Some((rid, text)) = &mut self.renaming
            && *rid == track.id
        {
            let field =
                Rect::from_center_size(name_rect.center(), vec2(name_rect.width(), size::CONTROL));
            ui.put(field, TextInput::new(text).width(field.width()))
                .request_focus();
            let (enter, esc) = ui.input(|i| {
                // Clicking away commits, like Enter.
                let away = i.pointer.any_pressed()
                    && i.pointer.press_origin().is_some_and(|p| !field.contains(p));
                (
                    i.key_pressed(Key::Enter) || away,
                    i.key_pressed(Key::Escape),
                )
            });
            if enter {
                let name = text.trim().to_owned();
                edit = set(&|s| s.name = name.clone());
            }
            if enter || esc {
                self.renaming = None;
            }
        } else if !track.name.is_empty() && track.name != label {
            let mut job = egui::text::LayoutJob::single_section(
                track.name.clone(),
                egui::TextFormat::simple(theme::body(), color::ink_2()),
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(name_rect.width().max(0.0));
            let g = painter.layout_job(job);
            let pos = pos2(name_rect.left(), mid - g.size().y / 2.0);
            painter.galley(pos, g, color::ink_2());
        }
        if body.double_clicked() {
            self.renaming = Some((track.id, track.name.clone()));
        }

        // Menu: rename, reorder, remove, add.
        let same_kind: Vec<_> = tracks.iter().filter(|t| t.kind == track.kind).collect();
        let pos = same_kind.iter().position(|t| t.id == track.id).unwrap_or(0);
        body.context_menu(|ui| {
            if ui.button(tr!("timeline.rename")).clicked() {
                self.renaming = Some((track.id, track.name.clone()));
            }
            if track.kind == TrackKind::Audio && ui.button(tr!("timeline.audio_effects")).clicked()
            {
                self.fx_popover = Some(track.id);
            }
            let mv = |index| {
                Some((
                    Command::MoveTrack {
                        id: track.id,
                        index,
                    },
                    None,
                ))
            };
            if ui
                .add_enabled(pos > 0, egui::Button::new(tr!("timeline.move_up")))
                .clicked()
            {
                edit = mv(pos - 1);
            }
            let last = pos + 1 >= same_kind.len();
            if ui
                .add_enabled(!last, egui::Button::new(tr!("timeline.move_down")))
                .clicked()
            {
                edit = mv(pos + 1);
            }
            let remove = egui::Button::new(tr!("timeline.remove_track"));
            if ui.add_enabled(!track.locked, remove).clicked() {
                edit = Some((Command::RemoveTrack(track.id), None));
            }
            ui.separator();
            add_menu(ui, &mut edit);
        });

        if self.fx_popover == Some(track.id) {
            edit = self.fx_popover(ui, rect, track, &label).or(edit);
        }

        // Height grip along the bottom edge.
        let grip = Rect::from_x_y_ranges(rect.x_range(), rect.bottom() - 3.0..=rect.bottom() + 2.0);
        let g = ui.interact(grip.intersect(column), id.with("height"), Sense::drag());
        if g.hovered() || g.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        if g.dragged()
            && let Some(p) = g.interact_pointer_pos()
        {
            // The header spans the keyframe lanes too; the grip sizes the clip row only.
            let h = snap_height(p.y - rect.top() - self.keys.extra(track));
            if h != track.height {
                let mut s = track.settings();
                s.height = h;
                edit = Some((Command::SetTrack(s), Some(RESIZE_GESTURE)));
            }
        }
        edit
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
            clipped.rect_filled(r, corner, color::red().gamma_multiply(0.3));
        } else if let Some(a) = asset
            && content.height() >= 8.0
            && content.width() >= 1.0
        {
            let source = |x: f32| clip.source_at(view.time(x));
            match kind {
                TrackKind::Video if !still => {
                    self.media
                        .filmstrip(ui, &clipped, content, view.x(clip.start), a, &source)
                }
                TrackKind::Audio if !a.generated() => {
                    let nth = clip.stream.unwrap_or(0) as usize;
                    self.media.waveform(ui, &clipped, content, a, nth, &source)
                }
                _ => {}
            }
        }
        let keyed = clip.keyframes.values().any(|k| !k.is_empty());
        let mut name = asset.map_or(
            tr!("timeline.missing_asset").into(),
            MediaAsset::display_name,
        );
        // A multi-stream file's audio clips say which stream they play.
        if let Some(p) = asset.and_then(|a| a.probe.as_ref())
            && kind == TrackKind::Audio
            && p.audio_streams > 1
        {
            let stream = clip
                .stream
                .map_or(tr!("timeline.all_audio").into(), |n| stream_name(p, n));
            name = format!("{name} · {stream}");
        }
        let name = if offline {
            tr!("timeline.offline_clip", name = name)
        } else {
            name
        };
        clipped.text(
            // Past the keyframe toggle when there is one.
            pos2(
                shown.left().max(r.left()) + if keyed { 24.0 } else { 6.0 },
                r.top() + NAME_STRIP / 2.0,
            ),
            Align2::LEFT_CENTER,
            name,
            theme::body_strong(),
            color::ink(),
        );
    }
}

/// Clipboard clips moved onto the target tracks: each kind keeps its track spacing, its top
/// clip's track becoming that kind's first target. Clips with no track there stay put.
/// A bin drop off every lane lands on a new track: a video track on top of the stack, or for
/// sound alone an audio track at the bottom. True when it adds the video track above others.
fn onto_new_track(pl: &mut Placement) -> bool {
    if pl.video.is_some() {
        pl.video = None;
        pl.add_video = true;
        return true;
    }
    if pl.audio.is_some() && !pl.add_video {
        pl.audio = None;
        pl.add_audio = true;
    }
    false
}

/// Whether the clip or a linked partner sits on a locked track, so it can't move or trim.
fn locked(seq: &Sequence, id: ClipId) -> bool {
    let tracks = seq.linked(id).into_iter().map(|(t, _)| seq.track(t));
    tracks.flatten().any(|t| t.locked)
}

fn to_targets(seq: &Sequence, clips: &[(TrackId, Clip)]) -> Vec<(TrackId, Clip)> {
    let tracks = seq.tracks();
    let place = |kind: TrackKind, t: TrackId| {
        let of_kind: Vec<_> = tracks.iter().filter(|x| x.kind == kind).collect();
        let index = |t: TrackId| of_kind.iter().position(|x| x.id == t);
        let target = of_kind.iter().position(|x| x.target)?;
        let top = clips.iter().filter_map(|(t, _)| index(*t)).min()?;
        Some(of_kind.get((index(t)? + target).checked_sub(top)?)?.id)
    };
    clips
        .iter()
        .map(|(t, c)| {
            let kind = seq.track(*t).map(|x| x.kind);
            let to = kind.and_then(|k| place(k, *t)).unwrap_or(*t);
            (to, c.clone())
        })
        .collect()
}

/// "V1" counts video tracks up from the bottom of the video stack, "A1" audio down from the top.
/// A thin, arrowless scrollbar in `bar` for `view` of `total` starting at `offset`;
/// returns the offset after any thumb drag.
#[allow(clippy::too_many_arguments)]
fn scrollbar(
    ui: &Ui,
    painter: &egui::Painter,
    bar: Rect,
    id: egui::Id,
    offset: f64,
    view: f64,
    total: f64,
    horizontal: bool,
) -> f64 {
    let along = |v: egui::Vec2| if horizontal { v.x } else { v.y } as f64;
    let len = along(bar.size());
    let thumb_len = (len * view / total).clamp(24.0_f64.min(len), len);
    let room = (len - thumb_len).max(1.0);
    let travel = (total - view).max(0.0);
    let at = if travel > 0.0 {
        offset / travel * room
    } else {
        0.0
    };
    let (a, b) = (at as f32, (at + thumb_len) as f32);
    let thumb = if horizontal {
        Rect::from_x_y_ranges(bar.left() + a..=bar.left() + b, bar.y_range())
    } else {
        Rect::from_x_y_ranges(bar.x_range(), bar.top() + a..=bar.top() + b)
    };
    if travel <= 0.0 {
        return 0.0;
    }
    let r = ui.interact(thumb, id, Sense::drag());
    let offset = (offset + along(r.drag_delta()) * travel / room).clamp(0.0, travel);
    // Painted 4px thick, fully rounded, per DESIGN.md.
    let paint = if horizontal {
        Rect::from_center_size(thumb.center(), vec2(thumb.width() - 4.0, 4.0))
    } else {
        Rect::from_center_size(thumb.center(), vec2(4.0, thumb.height() - 4.0))
    };
    let ink = if r.hovered() || r.dragged() {
        color::ink_2()
    } else {
        color::ink_3()
    };
    painter.rect_filled(paint, CornerRadius::same(2), ink);
    offset
}

fn track_label(tracks: &[Track], index: usize) -> String {
    let t = &tracks[index];
    let same = |o: &&Track| o.kind == t.kind;
    match t.kind {
        TrackKind::Video => tr!(
            "timeline.video_track",
            number = tracks[index..].iter().filter(same).count()
        ),
        TrackKind::Audio => tr!(
            "timeline.audio_track",
            number = tracks[..=index].iter().filter(same).count()
        ),
    }
}

/// Audio stream `n`'s title from the file, else "Audio n" counting from 1 in the UI language.
fn stream_name(p: &ProbeInfo, n: u32) -> String {
    let title = p
        .streams
        .iter()
        .filter_map(|s| match s {
            StreamInfo::Audio(a) => Some(a.title.as_ref()),
            _ => None,
        })
        .nth(n as usize)
        .flatten();
    title
        .cloned()
        .unwrap_or_else(|| tr!("timeline.audio_stream", number = n + 1))
}

/// A dragged height, snapped and clamped to the allowed range.
fn snap_height(h: f32) -> f32 {
    ((h / HEIGHT_STEP).round() * HEIGHT_STEP).clamp(Track::MIN_HEIGHT, Track::MAX_HEIGHT)
}

/// An input meter filling `bar` from -60 to 0 dBFS for a 0..=1 `level`, amber above -6 dB
/// and Red at clipping.
pub fn meter(painter: &egui::Painter, bar: Rect, level: f32) {
    let db = 20.0 * level.max(1e-6).log10();
    let fill = match db {
        d if d >= 0.0 => color::red(),
        d if d > -6.0 => color::amber(),
        _ => color::green(),
    };
    let w = ((db + 60.0) / 60.0).clamp(0.0, 1.0) * bar.width();
    let corner = CornerRadius::same(1);
    painter.rect_filled(bar, corner, color::field());
    painter.rect_filled(
        Rect::from_min_size(bar.min, vec2(w, bar.height())),
        corner,
        fill,
    );
}

fn add_menu(ui: &mut Ui, edit: &mut Option<Edit>) {
    for (kind, label) in [
        (TrackKind::Video, tr!("timeline.add_video_track")),
        (TrackKind::Audio, tr!("timeline.add_audio_track")),
    ] {
        if ui.button(label).clicked() {
            let name = String::new();
            *edit = Some((Command::AddTrack { kind, name }, None));
        }
    }
}

/// A 20px ghost toggle: Ink on Raised when on, or Amber for solo.
#[allow(clippy::too_many_arguments)]
fn toggle(
    ui: &Ui,
    painter: &egui::Painter,
    rect: Rect,
    column: Rect,
    id: egui::Id,
    glyph: &str,
    font: FontId,
    on: bool,
    solo: bool,
    name: &str,
) -> bool {
    let r = ui.interact(rect.intersect(column), id, Sense::click());
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, name));
    let (fill, ink) = match (on, solo) {
        (true, true) => (color::amber(), color::ground()),
        (true, false) => (color::raised(), color::ink()),
        _ if r.hovered() => (color::border(), color::ink_2()),
        _ => (Color32::TRANSPARENT, color::ink_3()),
    };
    painter.rect_filled(rect, CornerRadius::same(radius::SMALL), fill);
    painter.text(rect.center(), Align2::CENTER_CENTER, glyph, font, ink);
    if r.has_focus() {
        painter.add(suite_ui::widgets::focus_ring(rect, radius::SMALL));
    }
    // Above the toggle, so the bottom track's tooltips don't cover + Track under it.
    let mut tip = egui::Tooltip::for_enabled(&r);
    tip.popup = tip.popup.align(egui::RectAlign::TOP);
    tip.show(|ui| ui.label(name));
    r.clicked()
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
        color::light_blue(),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            pos2(body.left(), body.bottom()),
            pos2(body.right(), body.bottom()),
            top_center + vec2(0.0, h),
        ],
        color::light_blue(),
        Stroke::NONE,
    ));
}

/// Results finished off-thread, per path, waiting for the UI thread; `None` is a failure.
type Done<T, K = PathBuf> = Arc<Mutex<Vec<(K, Option<T>)>>>;

/// Filmstrips and waveform pyramids per asset, loaded on a job pool the first time a clip
/// using it is in view. `None` while loading; a failure is forgotten so the next paint retries.
#[derive(Default)]
struct Media {
    /// `None` in tests, which then load nothing.
    caches: Option<Arc<Caches>>,
    strip_done: Done<Vec<egui::ColorImage>>,
    strips: HashMap<PathBuf, Option<Vec<TextureHandle>>>,
    peaks_done: Done<Pyramid, (PathBuf, usize)>,
    /// By file and audio stream.
    peaks: HashMap<(PathBuf, usize), Option<Pyramid>>,
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
            // Frames arrive in order as they're made; each batch extends the strip.
            let texs = self
                .strips
                .entry(path.clone())
                .or_default()
                .get_or_insert_default();
            for img in imgs {
                let name = format!("{}#{}", path.display(), texs.len());
                texs.push(ui.ctx().load_texture(name, img, Default::default()));
            }
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
                    let color = |img: &image::RgbaImage| {
                        let size = [img.width() as usize, img.height() as usize];
                        egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw())
                    };
                    let mut sent = 0;
                    let r = cache.filmstrip_with(&path, duration, |img| {
                        done.lock()
                            .unwrap()
                            .push((path.clone(), Some(vec![color(img)])));
                        ctx.request_repaint();
                        sent += 1;
                    });
                    match r {
                        // From the disk cache, nothing was sent as it was made.
                        Ok(v) => {
                            let rest = v[sent..].iter().map(color).collect();
                            done.lock().unwrap().push((path, Some(rest)));
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
        let n = (probe.duration.0 / step).max(1);
        let size = texs[0].size_vec2();
        let w = size.x * rect.height() / size.y;
        // Tiles sit on a grid from the clip's start, so they don't swim while scrolling.
        let mut x = origin + ((rect.left() - origin) / w).floor() * w;
        while x < rect.right() {
            let i = (source(x).0 / step).clamp(0, n - 1) as usize;
            // Past what's made so far: blank until it fills in.
            let Some(tex) = texs.get(i) else {
                x += w;
                continue;
            };
            let tile = Rect::from_min_size(pos2(x, rect.top()), vec2(w, rect.height()));
            painter.image(
                tex.id(),
                tile,
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            x += w;
        }
    }

    /// The peaks of audio stream `nth` of `a`, starting to load them if they aren't yet.
    fn pyramid(&mut self, ctx: &egui::Context, a: &MediaAsset, nth: usize) -> Option<&Pyramid> {
        for (path, p) in self.peaks_done.lock().unwrap().drain(..) {
            match p {
                Some(p) => self.peaks.insert(path, Some(p)),
                None => self.peaks.remove(&path),
            };
        }
        let path = (a.path.clone(), nth);
        if !self.peaks.contains_key(&path) {
            let caches = self.caches.as_ref()?;
            self.peaks.insert(path.clone(), None);
            let (done, ctx, key) = (self.peaks_done.clone(), ctx.clone(), path.clone());
            caches
                .peaks
                .submit(&caches.jobs, path.0.clone(), nth, move |r| match r {
                    Ok(p) => {
                        done.lock().unwrap().push((key.clone(), Some(p)));
                        ctx.request_repaint();
                    }
                    Err(e) => {
                        tracing::warn!("waveform for {}: {e}", key.0.display());
                        done.lock().unwrap().push((key.clone(), None));
                    }
                });
        }
        self.peaks.get(&path)?.as_ref()
    }

    /// One min/max line per point across `rect`, in a lighter tint of the clip colour.
    fn waveform(
        &mut self,
        ui: &Ui,
        painter: &egui::Painter,
        rect: Rect,
        a: &MediaAsset,
        // shortcut: a merged clip draws its first stream, until merged peaks are worth a scan.
        nth: usize,
        source: &dyn Fn(f32) -> Time,
    ) {
        let Some(pyramid) = self.pyramid(ui.ctx(), a, nth) else {
            return;
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
        let tint = color::CLIP_GREEN.lerp_to_gamma(color::ink(), 0.45);
        peak_lines(painter, rect, peaks, tint);
    }
}

/// One min/max line per pixel column of `rect`, from its left edge.
fn peak_lines(painter: &egui::Painter, rect: Rect, peaks: Vec<Peak>, tint: egui::Color32) {
    let (mid, half) = (rect.center().y, rect.height() / 2.0 - 1.0);
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

/// A take as it records, at `r`: a clip in the recording colour, Red, with the waveform of
/// what's come in so far.
fn paint_live(painter: &egui::Painter, r: Rect, lanes: Rect, view: View, live: &Live) {
    let fill = color::red().lerp_to_gamma(color::field(), 0.55);
    painter.rect_filled(r, CornerRadius::same(radius::SMALL), fill);
    let shown = r.intersect(lanes);
    let content = Rect::from_x_y_ranges(shown.x_range(), r.top() + NAME_STRIP..=r.bottom());
    let clipped = painter.with_clip_rect(r.intersect(painter.clip_rect()));
    if content.height() >= 8.0 && content.width() >= 1.0 {
        let frame = |x: f32| {
            ((view.time(x) - live.start).0 as i128 * live.rate as i128 / FLICKS_PER_SECOND as i128)
                as i64
        };
        let width = content.width().ceil() as usize;
        let (a, b) = (frame(content.left()), frame(content.right()));
        let peaks = waveforms::query(&live.peaks, BUCKET, a, b, width);
        peak_lines(
            &clipped,
            content,
            peaks,
            fill.lerp_to_gamma(color::ink(), 0.45),
        );
    }
    clipped.text(
        pos2(shown.left().max(r.left()) + 6.0, r.top() + NAME_STRIP / 2.0),
        Align2::LEFT_CENTER,
        &live.name,
        theme::body_strong(),
        color::ink(),
    );
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
        for z in [MIN_ZOOM, 0.5, 3.0, 47.0, 333.0, MAX_ZOOM] {
            for r in [
                Rate::new(24, 1),
                Rate::new(25, 1),
                Rate::new(30000, 1001),
                Rate::new(60, 1),
                Rate::new(144, 1),
                Rate::new(240, 1),
            ] {
                let (major, minor) = tick_steps(z, r);
                assert!(major as f64 * r.den as f64 / r.num as f64 * z >= MAJOR_PX);
                assert_eq!(major % minor, 0);
            }
        }
    }

    pub(super) fn project(n: usize) -> Project {
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
    fn reverse_and_freeze_actions_undo() {
        let mut h = History::new(project(2));
        let mut t = Timeline::default();
        t.sync(h.project());
        let seq = |h: &History| h.project().sequence().clone();
        let v = seq(&h).tracks()[0].clips()[0].id;
        assert!(!t.can("reverse", &seq(&h)) && !t.can("freeze_frame", &seq(&h)));
        t.select(&seq(&h), v, egui::Modifiers::NONE);
        t.seek(Time(S / 2));
        assert!(t.can("reverse", &seq(&h)) && t.can("freeze_frame", &seq(&h)));
        let cmd = t.clip_action("reverse", &seq(&h)).unwrap();
        h.apply(cmd).unwrap();
        assert!(seq(&h).clip(v).unwrap().1.reverse);
        let shown = seq(&h).clip(v).unwrap().1.source_at(Time(S / 2));
        let cmd = t.clip_action("freeze_frame", &seq(&h)).unwrap();
        h.apply(cmd).unwrap();
        let clips = seq(&h).tracks()[0].clips().to_vec();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[1].freeze, Some(shown));
        h.undo();
        h.undo();
        let c = seq(&h).clip(v).unwrap().1.clone();
        assert!(!c.reverse && c.freeze.is_none());
        assert_eq!(seq(&h).tracks()[0].clips().len(), 1);
    }

    #[test]
    fn paste_grade_onto_several_clips_is_one_undo_step() {
        use jvid_core::keyframe::{Easing, Keyframe, Value};
        use std::collections::BTreeMap;
        let mut h = History::new(project(6));
        let mut t = Timeline::default();
        t.sync(h.project());
        let seq = |h: &History| h.project().sequence().clone();
        let v: Vec<Clip> = seq(&h).tracks()[0].clips().to_vec();
        let audio = seq(&h).tracks()[1].clips()[0].id;
        // Grade the first clip; the second has an exposure key the paste must replace.
        let mut graded = v[0].transform.clone();
        graded.exposure = 1.0;
        graded.wheels.lift.master = 0.4;
        let key = Keyframe {
            time: v[1].source_in,
            value: Value::Scalar(-2.0),
            easing: Easing::Linear,
        };
        let keyed = BTreeMap::from([("exposure".to_string(), vec![key])]);
        h.apply(Command::SetClipProps(vec![
            (v[0].id, graded.clone(), v[0].keyframes.clone()),
            (v[1].id, v[1].transform.clone(), keyed),
        ]))
        .unwrap();
        assert!(!t.can("copy_grade", &seq(&h)) && !t.can("paste_grade", &seq(&h)));
        t.selected = HashSet::from([v[0].id]);
        assert!(t.action("copy_grade", h.project()));
        t.selected = HashSet::from([v[1].id, v[2].id, audio]);
        assert!(t.can("paste_grade", &seq(&h)));
        let before = h.project().clone();
        let cmd = t.clip_action("paste_grade", &seq(&h)).unwrap();
        h.apply(cmd).unwrap();
        for id in [v[1].id, v[2].id] {
            let c = seq(&h).clip(id).unwrap().1.clone();
            assert_eq!(
                (c.transform.exposure, c.transform.wheels),
                (1.0, graded.wheels)
            );
            assert!(c.keyframes.is_empty(), "the pasted grade is static");
        }
        let a = seq(&h).clip(audio).unwrap().1.clone();
        assert_eq!(a.transform.exposure, 0.0, "audio clips have no grade");
        h.undo();
        assert_eq!(h.project(), &before, "one undo puts every clip back");
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
        assert!(!t.action("insert", &p));
    }

    #[test]
    fn arrow_keys_move_the_playhead() {
        use egui::{Key, Modifiers};
        use egui_kittest::Harness;
        // V1: [0,2) [2,4) [4,6); A1: [0,2) [2,4) [7,9).
        let mut h = History::new(project(5));
        let a1 = h.project().sequence().tracks()[1].id;
        let asset = h.project().assets()[0].id;
        h.apply(Command::AddClip {
            track: a1,
            clip: NewClip {
                asset,
                source_in: Time(0),
                source_out: Time(2 * S),
                start: Time(7 * S),
                link: None,
            },
        })
        .unwrap();
        let p = h.project().clone();
        let frame = Rate::new(30, 1).time_floor(1);
        let run = |map: Keymap, t: Timeline, keys: &[(Modifiers, Key, Time)]| {
            let mut h = Harness::new_ui_state(
                |ui, t: &mut Timeline| {
                    for id in map.pressed(ui.ctx(), true) {
                        assert!(t.action(id, &p), "{id}");
                    }
                },
                t,
            );
            for &(m, k, want) in keys {
                h.key_press_modifiers(m, k);
                h.run();
                assert_eq!(h.state().playhead(), want, "{m:?}+{k:?}");
            }
            h.into_state()
        };
        let (none, shift, ctrl) = (Modifiers::NONE, Modifiers::SHIFT, Modifiers::COMMAND);
        let mut t = Timeline::default();
        t.sync(&p);
        t.seek(Time(3 * S));
        let t = run(
            Keymap::default(),
            t,
            &[
                (none, Key::ArrowRight, Time(3 * S) + frame),
                (none, Key::ArrowLeft, Time(3 * S)),
                (shift, Key::ArrowRight, Time(4 * S)),
                (shift, Key::ArrowLeft, Time(3 * S)),
                (ctrl, Key::ArrowRight, Time(4 * S)),
                (ctrl, Key::ArrowRight, Time(6 * S)),
                // An edit on another track.
                (ctrl, Key::ArrowRight, Time(7 * S)),
                (ctrl, Key::ArrowLeft, Time(6 * S)),
                (none, Key::ArrowDown, Time(7 * S)),
                (none, Key::ArrowUp, Time(6 * S)),
                (none, Key::ArrowLeft, Time(6 * S) - frame),
                (ctrl | shift, Key::ArrowLeft, Time(4 * S)),
                (ctrl | shift, Key::ArrowRight, Time(6 * S) - frame),
                (none, Key::Home, Time(0)),
                (shift, Key::ArrowLeft, Time(0)),
                (none, Key::End, Time(9 * S)),
                (shift, Key::ArrowRight, Time(10 * S)),
            ],
        );
        // Rebound in keymap.toml, the old key does nothing.
        let map = Keymap::from_toml("next_edit = \"N\"").unwrap();
        run(
            map,
            t,
            &[
                (ctrl, Key::ArrowRight, Time(10 * S)),
                (none, Key::Home, Time(0)),
                (none, Key::N, Time(2 * S)),
            ],
        );
    }

    #[test]
    fn in_out_keys_mark_jump_lift_and_extract() {
        let mut h = History::new(project(4));
        let mut t = Timeline::default();
        let frame = Rate::new(30, 1).time_floor(1);
        let run = |t: &mut Timeline, h: &mut History, id| {
            assert!(t.action(id, h.project()));
            if let Some(c) = t.pending.take() {
                h.apply(c).unwrap();
            }
        };
        let marks = |h: &History| {
            let s = h.project().sequence();
            (s.in_point(), s.out_point())
        };
        t.sync(h.project());
        t.seek(Time(S));
        run(&mut t, &mut h, "mark_in");
        t.seek(Time(3 * S));
        run(&mut t, &mut h, "mark_out");
        assert_eq!(marks(&h), (Some(Time(S)), Some(Time(3 * S) + frame)));
        run(&mut t, &mut h, "go_to_in");
        assert_eq!(t.playhead(), Time(S));
        run(&mut t, &mut h, "go_to_out");
        assert_eq!(t.playhead(), Time(3 * S));
        // An in past the out clears it.
        t.seek(Time(5 * S));
        run(&mut t, &mut h, "mark_in");
        assert_eq!(marks(&h), (Some(Time(5 * S)), None));
        run(&mut t, &mut h, "clear_in");
        assert_eq!(marks(&h), (None, None));
        t.seek(Time(3 * S));
        run(&mut t, &mut h, "mark_clip");
        assert_eq!(marks(&h), (Some(Time(2 * S)), Some(Time(4 * S))));
        run(&mut t, &mut h, "clear_in_out");
        assert_eq!(marks(&h), (None, None));
        let first = h.project().sequence().tracks()[0].clips()[0].id;
        t.selected.insert(first);
        run(&mut t, &mut h, "mark_selection");
        assert_eq!(marks(&h), (Some(Time(0)), Some(Time(2 * S))));
        run(&mut t, &mut h, "lift");
        let starts = |h: &History| -> Vec<_> {
            let c = h.project().sequence().tracks()[0].clips().iter();
            c.map(|c| c.start.0 / S).collect()
        };
        assert_eq!(starts(&h), [2]);
        assert_eq!(marks(&h), (None, None));
        h.undo();
        run(&mut t, &mut h, "extract");
        assert_eq!(starts(&h), [0]);
        assert_eq!(h.project().sequence().duration(), Time(2 * S));
    }

    #[test]
    fn select_link_copy_paste_and_delete() {
        use egui::Modifiers as M;
        let mut h = History::new(project(2));
        let mut t = Timeline::default();
        let seq = |h: &History| h.project().sequence().clone();
        let ids = |h: &History| -> Vec<_> {
            let s = seq(h);
            s.tracks()
                .iter()
                .flat_map(|t| t.clips())
                .map(|c| c.id)
                .collect()
        };
        let [v, a] = ids(&h)[..] else { panic!() };
        let run = |t: &mut Timeline, h: &mut History, id| {
            let cmd = t.clip_action(id, h.project().sequence());
            h.apply(cmd.expect("an edit")).unwrap();
        };

        t.select(&seq(&h), v, M::NONE);
        t.select(&seq(&h), a, M::SHIFT);
        assert_eq!(t.selected, HashSet::from([v, a]));
        t.select(&seq(&h), v, M::COMMAND);
        assert_eq!(t.selected, HashSet::from([a]));

        // Link both; a click then selects the pair, and link again unlinks.
        t.select(&seq(&h), v, M::SHIFT);
        run(&mut t, &mut h, "link");
        t.select(&seq(&h), v, M::NONE);
        assert_eq!(t.selected, HashSet::from([v, a]));

        // Shift+E disables the pair, then enables it again.
        let enabled = |h: &History| seq(h).clip(v).unwrap().1.enabled;
        run(&mut t, &mut h, "enable");
        assert!(!enabled(&h) && !seq(&h).clip(a).unwrap().1.enabled);
        run(&mut t, &mut h, "enable");
        assert!(enabled(&h));

        // Pasted at the playhead, the copies are linked to each other only.
        assert!(t.clip_action("copy", h.project().sequence()).is_none());
        t.sync(h.project());
        t.seek(Time(10 * S));
        run(&mut t, &mut h, "paste");
        let s = seq(&h);
        let pasted: Vec<_> = s.clips_at(Time(10 * S)).map(|(_, c)| c.clone()).collect();
        assert_eq!(pasted.len(), 2);
        assert!(pasted[0].link.is_some() && pasted[0].link == pasted[1].link);
        assert_ne!(pasted[0].link, s.clip(v).unwrap().1.link);

        // Ripple delete closes the gap the originals leave.
        run(&mut t, &mut h, "ripple_delete");
        assert_eq!(ids(&h).len(), 2);
        assert!(seq(&h).clips_at(Time(8 * S)).count() == 2);
        assert!(t.clip_action("delete", h.project().sequence()).is_none());
    }

    #[test]
    fn add_edit_razor_q_w_and_gap() {
        let mut h = History::new(project(4));
        let mut t = Timeline::default();
        let all: Vec<_> = h
            .project()
            .sequence()
            .tracks()
            .iter()
            .map(|t| t.id)
            .collect();
        let pair = |h: &History, i: usize| -> Vec<ClipId> {
            let s = h.project().sequence();
            s.tracks().iter().map(|t| t.clips()[i].id).collect()
        };
        for i in 0..2 {
            let ids = pair(&h, i);
            h.apply(Command::LinkClips { ids, linked: true }).unwrap();
        }
        let spans = |h: &History| -> Vec<Vec<(i64, i64)>> {
            let s = h.project().sequence();
            let t = s.tracks().iter();
            t.map(|t| {
                t.clips()
                    .iter()
                    .map(|c| (c.start.0 / S, c.end().0 / S))
                    .collect()
            })
            .collect()
        };
        let run = |t: &mut Timeline, h: &mut History, id, at: i64| {
            t.sync(h.project());
            t.seek(Time(at * S));
            assert!(t.action(id, h.project()));
            h.apply(t.pending.take().expect("an edit")).unwrap();
            t.applied(true);
        };
        let before = h.project().clone();

        // Q at 1s on the linked pair: both lose their first second, the rest follows.
        run(&mut t, &mut h, "ripple_trim_prev", 1);
        assert_eq!(spans(&h), vec![vec![(0, 1), (1, 3)]; 2]);
        assert_eq!(t.playhead(), Time(0));
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        // W at 3s: the second pair loses its last second.
        run(&mut t, &mut h, "ripple_trim_next", 3);
        assert_eq!(spans(&h), vec![vec![(0, 2), (2, 3)]; 2]);
        assert!(h.undo());

        // Ctrl+K with nothing selected cuts every track; with a selection, only it.
        run(&mut t, &mut h, "add_edit", 1);
        assert_eq!(spans(&h), vec![vec![(0, 1), (1, 2), (2, 4)]; 2]);
        assert!(h.undo());
        // Linked selection brings in the partner, as the razor does.
        t.selected.insert(pair(&h, 0)[0]);
        run(&mut t, &mut h, "add_edit", 1);
        assert_eq!(spans(&h), vec![vec![(0, 1), (1, 2), (2, 4)]; 2]);
        assert!(h.undo());
        t.linked_selection = false;
        run(&mut t, &mut h, "add_edit", 1);
        assert_eq!(spans(&h)[0].len(), 3);
        assert_eq!(spans(&h)[1].len(), 2);
        assert!(h.undo());
        t.selected.clear();
        t.linked_selection = true;

        // A refused Q leaves the playhead where it was.
        t.seek(Time(S));
        assert!(t.action("ripple_trim_prev", h.project()));
        t.applied(false);
        assert_eq!(t.playhead(), Time(S));
        t.pending = None;

        // The razor cuts the clip and its partner where it's clicked.
        let view = t.view(0.0);
        let (c, _) = t.razor_cut(h.project().sequence(), view, pair(&h, 1)[0], 60.0);
        h.apply(c).unwrap();
        assert_eq!(spans(&h), vec![vec![(0, 2), (2, 3), (3, 4)]; 2]);
        assert!(h.undo());

        // Delete on a picked gap closes it.
        h.apply(Command::RemoveClips {
            ids: pair(&h, 0),
            ripple: false,
        })
        .unwrap();
        t.gap = Some((all[0], Time(S)));
        // The partner on A1 follows to stay in sync.
        run(&mut t, &mut h, "delete", 0);
        assert_eq!(spans(&h), vec![vec![(0, 2)]; 2]);
    }

    #[test]
    fn dragging_a_clip_edge_trims_in_one_step() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness(project(2));
        let lane_y = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
        let before = h.state().1.project().clone();
        // V1's clip ends 2s (40pt) past the 8pt margin and header; its tail grip is inside.
        let x = 8.0 + size::TRACK_HEADER + 36.0;
        h.hover_at(pos2(x, lane_y));
        h.run();
        h.drag_at(pos2(x, lane_y));
        h.run();
        for dx in [10.0, 20.0] {
            h.hover_at(pos2(x + dx, lane_y));
            h.run();
        }
        assert!(h.state().0.trim_frame(&before).is_some());
        h.drop_at(pos2(x + 20.0, lane_y));
        h.run();
        let s = h.state().1.project().sequence().clone();
        assert_eq!(s.tracks()[0].clips()[0].end(), Time(3 * S));
        assert_eq!(s.tracks()[1].clips()[0].end(), Time(2 * S));
        assert!(h.state_mut().1.undo());
        assert_eq!(h.state().1.project(), &before);
    }

    #[test]
    fn ctrl_dragging_a_tail_ripples_later_clips() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness(project(4));
        let lane_y = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
        let before = h.state().1.project().clone();
        let x = 8.0 + size::TRACK_HEADER + 36.0;
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
        h.hover_at(pos2(x, lane_y));
        h.run();
        h.drag_at(pos2(x, lane_y));
        h.run();
        for dx in [10.0, 20.0] {
            h.hover_at(pos2(x + dx, lane_y));
            h.run();
        }
        h.drop_at(pos2(x + 20.0, lane_y));
        h.run();
        // V1's first clip grows a second and its next clip follows instead of blocking it.
        let s = h.state().1.project().sequence().clone();
        let v1 = s.tracks()[0].clips();
        assert_eq!((v1[0].end(), v1[1].start), (Time(3 * S), Time(3 * S)));
        assert!(h.state_mut().1.undo());
        assert_eq!(h.state().1.project(), &before);
    }

    /// Edits from the timeline go through a real history, as in the app.
    pub(super) fn harness(project: Project) -> egui_kittest::Harness<'static, (Timeline, History)> {
        harness_at(project, 0.25)
    }

    /// [`harness`] stepping `dt` seconds a frame.
    fn harness_at(
        project: Project,
        dt: f32,
    ) -> egui_kittest::Harness<'static, (Timeline, History)> {
        let mut fresh = true;
        let mut h = egui_kittest::Harness::builder()
            .with_size([1000.0, 200.0])
            .with_step_dt(dt)
            .wgpu()
            .build_ui_state(
                move |ui, (t, h): &mut (Timeline, History)| {
                    if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                        ui.painter().rect_filled(ui.max_rect(), 0.0, color::panel());
                        let edit = t.show(ui, h.project(), &HashSet::new(), &Keymap::default());
                        match edit {
                            Some((c, Some(k))) => h.apply_coalesced(k, c).unwrap(),
                            Some((c, None)) => h.apply(c).unwrap(),
                            None => jvid_core::history::Created::Nothing,
                        };
                    }
                },
                (Timeline::default(), History::new(project)),
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
        let mut h = harness(header_states());
        h.run();
        results.add(h.try_snapshot("timeline_headers"));
    }

    /// #263: a track's Audio Effects open in a popover beside its header, which stays open
    /// through a preset pick and closes on a click outside.
    #[test]
    fn audio_effects_popover() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness(project(2));
        h.set_size(vec2(1000.0, 560.0));
        h.run();
        let a1 = h.get_all_by_label("Target").nth(1).unwrap().rect();
        // A1's header, between its Target and Arm for Recording.
        let at = a1.right_center() + vec2(10.0, 0.0);
        h.hover_at(at);
        h.run();
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            h.run();
        }
        assert!(h.query_by_label("Rename").is_some(), "no menu at {at:?}");
        h.get_by_role_and_label(Role::Button, "Audio Effects")
            .click();
        h.run();
        h.get_by_role_and_label(Role::ComboBox, "Flat").click();
        h.run();
        h.get_by_role_and_label(Role::Button, "Voice").click();
        h.run();
        let fx = |h: &egui_kittest::Harness<'_, (Timeline, History)>| {
            h.state().1.project().sequence().tracks()[1].fx
        };
        assert_eq!(fx(&h).eq.bands[0], -12.0);
        h.remove_cursor();
        h.run();
        h.snapshot("timeline_audio_fx_popover");
        h.hover_at(pos2(990.0, 550.0));
        h.run();
        h.drag_at(pos2(990.0, 550.0));
        h.drop_at(pos2(990.0, 550.0));
        h.run();
        assert!(h.query_by_label("Close Audio Effects").is_none());
        assert!(h.state_mut().1.undo());
        assert_eq!(fx(&h), jvid_core::model::AudioFx::default());
    }

    /// Two video tracks and an audio one, with every header state on somewhere.
    fn header_states() -> Project {
        let mut h = History::new(project(6));
        let _ = h.apply(Command::AddTrack {
            kind: TrackKind::Video,
            name: "Titles".into(),
        });
        let t: Vec<_> = h.project().sequence().tracks().to_vec();
        let mut set = |t: &Track, f: &dyn Fn(&mut jvid_core::model::TrackSettings)| {
            let mut s = t.settings();
            f(&mut s);
            h.apply(Command::SetTrack(s)).unwrap();
        };
        set(&t[0], &|s| (s.locked, s.hidden) = (true, true));
        set(&t[1], &|s| (s.target, s.height) = (true, 56.0));
        set(&t[2], &|s| (s.muted, s.solo) = (true, true));
        h.project().clone()
    }

    #[test]
    fn labels_count_video_up_and_audio_down() {
        let p = header_states();
        let t = p.sequence().tracks();
        let labels: Vec<_> = (0..t.len()).map(|i| track_label(t, i)).collect();
        assert_eq!(labels, ["V2", "V1", "A1"]);
        assert_eq!(snap_height(41.0), 40.0);
        assert_eq!(snap_height(43.0), 44.0);
        assert_eq!(snap_height(5.0), Track::MIN_HEIGHT);
        assert_eq!(snap_height(999.0), Track::MAX_HEIGHT);
    }

    /// #326: a take draws as a clip from its start to the playhead, its waveform growing with
    /// the input; on stop the clip lands where it was drawn, with that waveform, in one undo.
    #[test]
    fn a_take_grows_with_the_playhead_and_lands_where_it_was_drawn() {
        use crate::record::{Take, tests::FakeMic};
        let dir = std::env::temp_dir().join(format!("jvid-live-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut h = History::new(project(2));
        let a1 = h.project().sequence().tracks()[1].id;
        let mut t = Timeline::default();
        t.sync(h.project());
        t.armed = Some(a1);
        let start = Time(10 * S);
        t.seek(start);
        let mic = FakeMic::default();
        let mut take = Take::start(&mut mic.clone(), None, &dir, a1, start).unwrap();
        let feed = mic.0.lock().unwrap().clone().unwrap();
        // Two seconds at 30 frames a second: 1600 samples a frame at 48 kHz mono.
        for frame in 1..=60 {
            let level = frame as f32 / 60.0;
            feed.push(&[level, -level].repeat(800));
            take.poll().unwrap();
            take.show(&mut t.live);
            t.seek(start + Time(frame * S / 30));
            let (live, end) = t.live_span().unwrap();
            assert_eq!(live.start, start);
            assert_eq!(end - start, Time(frame * S / 30), "frame {frame}");
            assert_eq!(live.peaks.len(), (frame as usize * 1600).div_ceil(256));
            assert_eq!(live.peaks.last(), Some(&[-level, level]));
        }
        let shown = t.live.take().unwrap().peaks;
        let path = take.path.clone();
        let rev = h.rev();
        let peaks = take.finish(&mut h).unwrap().expect("48 kHz peaks");
        assert_eq!(peaks.finest(), shown);
        t.recorded(path.clone(), peaks.clone());

        let a1_clips = |h: &History| h.project().sequence().track(a1).unwrap().clips().to_vec();
        let clip = a1_clips(&h).into_iter().find(|c| c.start == start).unwrap();
        assert_eq!(clip.end(), start + Time(2 * S), "where the live clip ended");
        let asset = h.project().asset(clip.asset).unwrap().path.clone();
        assert_eq!(t.media.peaks[&(asset, 0)], Some(peaks.clone()));
        // A rescan of the file next session draws the same waveform, to 16-bit rounding.
        let scanned = jvid_media::waveforms::Cache::new(dir.join("cache"), 1 << 30)
            .load(&path, 0)
            .unwrap();
        assert_eq!(scanned.finest().len(), shown.len());
        for (a, b) in scanned.finest().iter().zip(&shown) {
            assert!(
                (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4,
                "{a:?} {b:?}"
            );
        }

        assert!(h.undo());
        assert_eq!(h.rev(), rev);
        assert!(a1_clips(&h).iter().all(|c| c.start != start));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn audio_headers_arm_one_track_for_recording() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness(project(2));
        assert_eq!(
            h.get_all_by_label("Arm for Recording").count(),
            1,
            "audio only"
        );
        h.get_by_label("Arm for Recording").click();
        h.run();
        let a1 = h.state().1.project().sequence().tracks()[1].id;
        assert_eq!(h.state().0.armed, Some(a1));
        assert!(!h.state().1.can_undo(), "arming isn't an edit");
        // While a take records, the toggle stops it, and the take draws from 4s to the playhead.
        h.state_mut().0.level = Some(0.5);
        h.state_mut().0.live = Some(Live {
            start: Time(4 * S),
            name: "Voiceover 1.wav".into(),
            rate: 48_000,
            peaks: (0..3 * 48_000 / BUCKET)
                .map(|i| {
                    let v = 0.2 + 0.6 * ((i as f32 / 40.0).sin() * 0.5 + 0.5);
                    [-v, v]
                })
                .collect(),
        });
        h.state_mut().0.seek(Time(7 * S));
        h.run();
        h.snapshot("timeline_recording");
        h.get_by_label("Stop Recording").click();
        h.run();
        assert_eq!(h.state().0.armed, None);
    }

    #[test]
    fn the_bottom_tracks_tooltips_leave_add_track_clear() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness(project(2));
        let arm = h.get_by_label("Arm for Recording").rect();
        h.hover_at(arm.center());
        for _ in 0..4 {
            h.step();
        }
        h.run();
        h.snapshot("timeline_arm_tooltip");
        let tip = h
            .get_all_by_label("Arm for Recording")
            .map(|n| n.rect())
            .find(|r| *r != arm)
            .expect("the tooltip shows");
        let add = h.get_by_label_contains("Track").rect();
        assert!(!tip.intersects(add), "{tip:?} covers + Track at {add:?}");
    }

    #[test]
    fn header_toggles_and_height_drag_are_undoable() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness(project(2));
        let track = |h: &egui_kittest::Harness<'_, (Timeline, History)>, i: usize| {
            h.state().1.project().sequence().tracks()[i].clone()
        };
        // V1 then A1: lock, hide/mute, solo each.
        h.get_all_by_label("Lock").next().unwrap().click();
        h.run();
        h.get_by_label("Mute").click();
        h.run();
        h.get_all_by_label("Solo").nth(1).unwrap().click();
        h.run();
        h.get_by_label("Hide").click();
        h.run();
        h.get_all_by_label("Target").next().unwrap().click();
        h.run();
        let (v, a) = (track(&h, 0), track(&h, 1));
        assert!(v.locked && v.hidden && v.target && !v.solo);
        assert!(a.muted && a.solo && !a.locked);

        // Dragging V1's bottom edge down 20pt: one undo step, snapped.
        // The toggles sit centred in the header's top 32pt.
        let lock = h.get_all_by_label("Lock").next().unwrap().rect();
        let (x, edge) = (40.0, lock.center().y - 16.0 + 40.0);
        h.hover_at(pos2(x, edge));
        h.run();
        h.drag_at(pos2(x, edge));
        h.run();
        for dy in [5.0, 10.0, 21.0] {
            h.hover_at(pos2(x, edge + dy));
            h.run();
        }
        h.drop_at(pos2(x, edge + 21.0));
        h.run();
        assert_eq!(track(&h, 0).height, 60.0);
        assert!(h.state_mut().1.undo());
        assert_eq!(track(&h, 0).height, 40.0);
        for _ in 0..5 {
            assert!(h.state_mut().1.undo());
        }
        assert_eq!(h.state().1.project(), &project(2));
    }

    #[test]
    fn snap_shift_picks_the_nearest_target_in_reach() {
        let (e, t) = (&[Time(100), Time(200)], &[Time(95), Time(207), Time(500)]);
        assert_eq!(snap_shift(e, t, Time(10)), Some((Time(-5), Time(95))));
        assert_eq!(snap_shift(e, t, Time(4)), None);
    }

    #[test]
    fn dragging_a_clip_near_the_playhead_snaps_onto_it() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let at = Time(5 * S + S / 3);
        for (snapping, want) in [(true, at), (false, Time(5 * S))] {
            let mut h = harness(project(1));
            h.state_mut().0.seek(at);
            if !snapping {
                h.state_mut().0.action("snapping", &Project::default());
            }
            let x = size::TRACK_HEADER + 20.0;
            let y = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
            h.hover_at(pos2(x, y));
            h.run();
            h.drag_at(pos2(x, y));
            h.run();
            // 100pt is 5s; the playhead sits 1/3s (under 8pt) further.
            for dx in [10.0, 100.0] {
                h.hover_at(pos2(x + dx, y));
                h.run();
            }
            h.drop_at(pos2(x + 100.0, y));
            h.run();
            let seq = h.state().1.project().sequence().clone();
            assert_eq!(
                seq.tracks()[0].clips()[0].start,
                want,
                "snapping {snapping}"
            );
        }
    }

    #[test]
    fn dragging_in_out_flags_snaps_to_clip_starts_and_ends() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        // One clip over 0..2s at 20pt/s; In at 1s, Out at 3s, playhead well clear.
        let fifth = Rate::new(30, 1).time_floor(6);
        let x = |t: f32| 8.0 + size::TRACK_HEADER + t * 20.0;
        let y = 8.0 + size::RULER / 2.0;
        let cases = [
            // (flag, drop at secs, snapping, want): 4pt from a boundary snaps, 12pt doesn't.
            (true, 0.2, true, Time(0)),
            (true, 0.2, false, fifth),
            (false, 2.2, true, Time(2 * S)),
            (false, 2.2, false, Time(2 * S) + fifth),
            (
                false,
                2.6,
                true,
                Time(2 * S) + Rate::new(30, 1).time_floor(18),
            ),
        ];
        for (is_in, to, snapping, want) in cases {
            let mut p = History::new(project(1));
            p.apply(Command::SetInOut {
                in_point: Some(Time(S)),
                out_point: Some(Time(3 * S)),
            })
            .unwrap();
            let mut h = harness(p.project().clone());
            h.state_mut().0.seek(Time(30 * S));
            if !snapping {
                h.state_mut().0.action("snapping", &Project::default());
            }
            let from = pos2(x(if is_in { 1.0 } else { 3.0 }), y);
            h.hover_at(from);
            h.run();
            h.drag_at(from);
            h.run();
            for t in [to + 0.5, to] {
                h.hover_at(pos2(x(t), y));
                h.run();
            }
            h.drop_at(pos2(x(to), y));
            h.run();
            let seq = h.state().1.project().sequence().clone();
            let got = if is_in {
                seq.in_point()
            } else {
                seq.out_point()
            };
            assert_eq!(got, Some(want), "in {is_in} to {to} snapping {snapping}");
            assert_eq!(h.state().0.snapped, None);
            // One undo step restores the original marks.
            assert!(h.state_mut().1.undo());
            let seq = h.state().1.project().sequence().clone();
            assert_eq!(
                (seq.in_point(), seq.out_point()),
                (Some(Time(S)), Some(Time(3 * S)))
            );
        }
    }

    #[test]
    fn dropping_an_effect_on_a_clip_adds_it() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let tmp = std::env::temp_dir().join(format!("jvid-fx-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        crate::effects::scan_for_test(&tmp);
        let mut h = harness(project(4));
        let lane_y = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
        let at = pos2(8.0 + size::TRACK_HEADER + 36.0, lane_y);
        h.hover_at(at);
        h.run();
        egui::DragAndDrop::set_payload(&h.ctx, crate::effects::Drag("vignette".into()));
        h.drag_at(at);
        h.run();
        h.drop_at(at);
        h.run();
        let s = h.state().1.project().sequence().clone();
        let fx = &s.tracks()[0].clips()[0].transform.effects;
        assert_eq!(fx.len(), 1);
        assert_eq!(fx[0].shader, "vignette");
        assert!(fx[0].params.contains_key("strength"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn edge_speed_grows_into_and_past_either_edge() {
        assert_eq!(edge_speed(50.0, 0.0, 100.0), 0.0);
        assert_eq!(edge_speed(24.0, 0.0, 100.0), 0.0);
        assert_eq!(edge_speed(76.0, 0.0, 100.0), 0.0);
        assert!(edge_speed(90.0, 0.0, 100.0) > 0.0);
        assert!(edge_speed(120.0, 0.0, 100.0) > edge_speed(90.0, 0.0, 100.0));
        assert!(edge_speed(10.0, 0.0, 100.0) < 0.0);
        assert!(edge_speed(-20.0, 0.0, 100.0) < edge_speed(10.0, 0.0, 100.0));
    }

    #[test]
    fn holding_a_clip_at_the_right_edge_scrolls_and_drops_at_the_scrolled_time() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness(project(1));
        h.state_mut().0.action("snapping", &Project::default());
        let x = size::TRACK_HEADER + 20.0;
        let y = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
        h.hover_at(pos2(x, y));
        h.run();
        h.drag_at(pos2(x, y));
        h.run();
        let edge = pos2(995.0, y);
        h.hover_at(edge);
        h.step();
        let before = h.state().0.scroll;
        // The pointer stays put; the view keeps scrolling on its own (up to the clamp).
        for _ in 0..3 {
            h.step();
        }
        let scrolled = h.state().0.scroll;
        assert!(scrolled > before, "{before} -> {scrolled}");
        h.drop_at(edge);
        h.run();
        let t = &h.state().0;
        let want = t.view(size::TRACK_HEADER).time(edge.x) - Time(S);
        let got = h.state().1.project().sequence().tracks()[0].clips()[0].start;
        // Within a frame or two of scrolling.
        assert!((got.0 - want.0).abs() <= S / 2, "{got:?} vs {want:?}");
    }

    #[test]
    fn bin_drop_and_clip_drag_place_clips() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness(project(2));
        let seq = |h: &egui_kittest::Harness<'_, (Timeline, History)>| {
            h.state().1.project().sequence().clone()
        };
        let lane_y = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
        let x = size::TRACK_HEADER + 20.0; // 1s in at the default 20pt/s.

        // Drag V1's clip 40pt right: 2s later, A1's unlinked clip stays.
        h.hover_at(pos2(x, lane_y));
        h.run();
        h.drag_at(pos2(x, lane_y));
        h.run();
        for dx in [10.0, 40.0] {
            h.hover_at(pos2(x + dx, lane_y));
            h.run();
        }
        h.drop_at(pos2(x + 40.0, lane_y));
        h.run();
        let s = seq(&h);
        let (v1, a1) = (&s.tracks()[0], &s.tracks()[1]);
        assert_eq!(v1.clips()[0].start, Time(2 * S));
        assert_eq!(a1.clips()[0].start, Time(0));

        // A bin item dropped over V1 near 10s: linked video and audio overwrite there.
        let asset = h.state().1.project().assets()[0].id;
        let at = pos2(x + 9.0 * 20.0, lane_y);
        h.hover_at(at);
        h.run();
        egui::DragAndDrop::set_payload(&h.ctx, asset);
        h.drag_at(at);
        h.run();
        h.drop_at(at);
        h.run();
        let s = seq(&h);
        let placed: Vec<_> = s
            .tracks()
            .iter()
            .map(|t| t.clips().last().unwrap().clone())
            .collect();
        assert!(placed[0].link.is_some() && placed[0].link == placed[1].link);
        assert_eq!(placed[0].start, placed[1].start);
        assert!(
            (placed[0].start.0 - 10 * S).abs() <= S,
            "{:?}",
            placed[0].start
        );
        assert!(h.state_mut().1.undo());
        assert_eq!(seq(&h).tracks()[0].clips().len(), 1);
    }

    #[test]
    fn bin_drop_below_the_lanes_adds_a_top_track_in_one_step() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut p = History::new(project(2));
        let _ = p.apply(Command::AddSolid {
            name: "Red".into(),
            color: [1.0, 0.0, 0.0, 1.0],
        });
        let solid = p.project().assets()[1].id;
        let mut h = harness(p.project().clone());
        let drop = |h: &mut egui_kittest::Harness<'_, (Timeline, History)>, at| {
            h.hover_at(at);
            h.run();
            egui::DragAndDrop::set_payload(&h.ctx, solid);
            h.drag_at(at);
            h.run();
            h.drop_at(at);
            h.run();
        };
        let lane_ys = |h: &egui_kittest::Harness<'_, (Timeline, History)>| -> Vec<f32> {
            let locks = h.get_all_by_label("Lock");
            locks.map(|n| n.rect().center().y).collect()
        };
        let at_secs = |s: i64| size::TRACK_HEADER + s as f32 * 20.0;
        let tracks = |h: &egui_kittest::Harness<'_, (Timeline, History)>| {
            h.state().1.project().sequence().tracks().to_vec()
        };

        // Under A1, past the last lane: a new video track on top takes the solid at 5s.
        let below = lane_ys(&h)[1] + Track::default_height();
        drop(&mut h, pos2(at_secs(5), below));
        let t = tracks(&h);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].kind, TrackKind::Video);
        let c = &t[0].clips()[0];
        assert_eq!(c.asset, solid);
        assert!((c.start.0 - 5 * S).abs() <= S, "{:?}", c.start);
        assert_eq!(t[1].clips().len(), 1, "V1 untouched");

        // Straight onto V1 (now second): no new track.
        let v1 = lane_ys(&h)[1];
        drop(&mut h, pos2(at_secs(10), v1));
        let t = tracks(&h);
        assert_eq!(t.len(), 3);
        assert_eq!(t[1].clips().len(), 2);

        // Undo takes the V1 clip, then the new track and its clip together; redo brings both.
        assert!(h.state_mut().1.undo());
        assert!(h.state_mut().1.undo());
        assert_eq!(tracks(&h).len(), 2);
        assert!(h.state_mut().1.redo());
        let t = tracks(&h);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].clips()[0].asset, solid);
    }

    /// With Reduce Motion the New Track slot opens in the frame the drag reaches the edge.
    #[test]
    fn reduce_motion_opens_the_new_track_slot_at_once() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut p = History::new(project(2));
        let _ = p.apply(Command::AddText {
            name: "Title".into(),
            text: Default::default(),
        });
        let title = p.project().assets()[1].id;
        for reduce in [false, true] {
            let mut h = harness_at(p.project().clone(), 1.0 / 60.0);
            crate::preferences::reduce_motion(&h.ctx, reduce);
            let v1 = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
            let x = size::TRACK_HEADER + 100.0;
            h.hover_at(pos2(x, v1));
            h.run();
            egui::DragAndDrop::set_payload(&h.ctx, title);
            h.drag_at(pos2(x, v1));
            h.run();
            h.hover_at(pos2(x, 8.0 + size::RULER + 2.0));
            h.step();
            let full = h.state().0.slot == Track::default_height();
            assert_eq!(full, reduce, "reduce motion {reduce}: {}", h.state().0.slot);
            h.drop_at(pos2(x, 8.0 + size::RULER / 2.0));
            h.run();
        }
    }

    #[test]
    fn bin_drop_at_the_top_edge_opens_a_new_track_slot() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut p = History::new(project(2));
        let _ = p.apply(Command::AddText {
            name: "Title".into(),
            text: Default::default(),
        });
        let title = p.project().assets()[1].id;
        let mut h = harness(p.project().clone());
        let before = h.state().1.project().clone();
        let tracks = |h: &egui_kittest::Harness<'_, (Timeline, History)>| {
            h.state().1.project().sequence().tracks().to_vec()
        };
        let v1 = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
        let edge = 8.0 + size::RULER + 2.0;
        let at = |s: i64, y| pos2(size::TRACK_HEADER + s as f32 * 20.0, y);
        let start = |h: &mut egui_kittest::Harness<'_, (Timeline, History)>, p| {
            h.hover_at(p);
            h.run();
            egui::DragAndDrop::set_payload(&h.ctx, title);
            h.drag_at(p);
            h.run();
        };

        // At the edge the slot opens; leaving up over the ruler closes it and adds nothing.
        start(&mut h, at(5, edge));
        h.hover_at(at(5, edge));
        h.run();
        assert_eq!(h.state().0.slot, Track::default_height());
        let ruler = 8.0 + size::RULER / 2.0;
        h.hover_at(at(5, ruler));
        h.run();
        assert_eq!(h.state().0.slot, 0.0);
        h.drop_at(at(5, ruler));
        h.run();
        assert_eq!(h.state().1.project(), &before);
        assert!(!h.state().1.can_undo());

        // Nor does a drop that wanders from the slot down onto V1, below its open edge.
        start(&mut h, at(5, edge));
        h.hover_at(at(5, edge));
        h.run();
        h.hover_at(at(5, v1 + Track::default_height()));
        h.run();
        h.hover_at(at(5, v1));
        h.run();
        assert_eq!(h.state().0.slot, 0.0);
        h.drop_at(at(5, v1));
        h.run();
        let t = tracks(&h);
        assert_eq!(t.len(), 2, "no track added");
        assert_eq!(t[0].clips().len(), 2, "dropped onto V1");
        assert!(h.state_mut().1.undo());
        assert_eq!(h.state().1.project(), &before);

        // Dropped in the slot: a video track above V1 holds the title at 5s.
        start(&mut h, at(5, edge));
        h.hover_at(at(5, edge));
        h.run();
        h.drop_at(at(5, edge));
        h.run();
        let t = tracks(&h);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].kind, TrackKind::Video);
        let c = &t[0].clips()[0];
        assert_eq!(c.asset, title);
        assert!((c.start.0 - 5 * S).abs() <= S, "{:?}", c.start);
        assert_eq!(t[1].clips().len(), 1, "V1 untouched");
        assert_eq!(h.state().0.slot, 0.0, "the new track replaces the slot");

        // One undo takes the clip and its track together.
        assert!(h.state_mut().1.undo());
        assert_eq!(h.state().1.project(), &before);
    }

    /// `project(2)` with its V1 clip linked to its A1 clip and V1 locked, from a fresh history.
    fn locked_v1() -> Project {
        let mut h = History::new(project(2));
        let s = h.project().sequence();
        let ids = s.tracks().iter().map(|t| t.clips()[0].id).collect();
        h.apply(Command::LinkClips { ids, linked: true }).unwrap();
        let mut v1 = h.project().sequence().tracks()[0].settings();
        v1.locked = true;
        h.apply(Command::SetTrack(v1)).unwrap();
        h.project().clone()
    }

    /// A locked clip keeps its fade line but loses the fade handles and its keyframe lanes.
    #[test]
    fn a_locked_clip_shows_no_fade_or_keyframe_handles() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        // Fade and key V1's clip, then lock V1: a locked track refuses both edits.
        let mut p = History::new(project(2));
        let c = p.project().sequence().tracks()[0].clips()[0].clone();
        let mut x = c.transform.clone();
        x.fade_in = Time(S);
        let props = Command::SetClipProps(vec![(c.id, x, c.keyframes.clone())]);
        let key = jvid_core::keyframe::Keyframe {
            time: Time(0),
            value: jvid_core::keyframe::Value::Scalar(1.0),
            easing: jvid_core::keyframe::Easing::Linear,
        };
        let param = "opacity".into();
        let mut v1 = p.project().sequence().tracks()[0].settings();
        v1.locked = true;
        for cmd in [
            props,
            Command::AddKeyframe {
                id: c.id,
                param,
                key,
            },
            Command::SetTrack(v1),
        ] {
            p.apply(cmd).unwrap();
        }
        let mut h = harness(p.project().clone());
        h.state_mut().0.selected.insert(c.id);
        h.run();
        assert!(!h.state().0.keys.expanded.contains(&c.id), "no lanes");
        let before = h.state().1.project().clone();
        // V1's clip sits 2px into the lane under the ruler.
        let top = 8.0 + size::RULER + 2.0;
        let view = h.state().0.view(8.0 + size::TRACK_HEADER);
        // The fade-in handle: a 7px square on the clip's top edge at the fade's end.
        let at = pos2(view.x(c.start + Time(S)), top + 3.5);
        h.hover_at(at);
        h.run();
        let cursor = h.output().platform_output.cursor_icon;
        assert_ne!(cursor, CursorIcon::ResizeHorizontal, "no fade handle");
        h.drag_at(at);
        h.run();
        h.hover_at(at + vec2(-20.0, 0.0));
        h.run();
        h.drop_at(at + vec2(-20.0, 0.0));
        h.run();
        assert_eq!(h.state().1.project(), &before);
    }

    /// A1's clip, linked to a clip on locked V1, keeps its own fade handles and lanes: they
    /// change only A1's clip, which history allows.
    #[test]
    fn a_clip_linked_to_a_locked_track_keeps_its_fades_and_lanes() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut p = History::new(locked_v1());
        let a1 = p.project().sequence().tracks()[1].clone();
        let c = a1.clips()[0].clone();
        let key = jvid_core::keyframe::Keyframe {
            time: Time(0),
            value: jvid_core::keyframe::Value::Scalar(1.0),
            easing: jvid_core::keyframe::Easing::Linear,
        };
        let param = "volume".into();
        p.apply(Command::AddKeyframe {
            id: c.id,
            param,
            key,
        })
        .unwrap();
        let mut h = harness(p.project().clone());
        h.state_mut().0.selected.insert(c.id);
        h.run();
        assert!(h.state().0.keys.expanded.contains(&c.id), "lanes");
        let v1 = h.state().1.project().sequence().tracks()[0].height;
        // A1's clip sits 2px into its lane, under the ruler and V1.
        let top = 8.0 + size::RULER + v1 + 2.0;
        let view = h.state().0.view(8.0 + size::TRACK_HEADER);
        // The fade-in handle: a 7px square on the clip's top-left corner while there's no fade.
        let at = pos2(view.x(c.start) + 3.5, top + 3.5);
        h.hover_at(at);
        h.run();
        h.drag_at(at);
        h.run();
        h.hover_at(at + vec2(30.0, 0.0));
        h.run();
        h.drop_at(at + vec2(30.0, 0.0));
        h.run();
        let seq = h.state().1.project().sequence().clone();
        let fade = seq.clip(c.id).unwrap().1.transform.fade_in;
        assert!(fade > Time(0), "{fade:?}");
    }

    /// Lanes open when V1 locks hide with its lock and come back when it unlocks.
    #[test]
    fn unlocking_a_track_brings_its_open_lanes_back() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut p = History::new(project(2));
        let c = p.project().sequence().tracks()[0].clips()[0].clone();
        let key = jvid_core::keyframe::Keyframe {
            time: Time(0),
            value: jvid_core::keyframe::Value::Scalar(1.0),
            easing: jvid_core::keyframe::Easing::Linear,
        };
        let param = "opacity".into();
        p.apply(Command::AddKeyframe {
            id: c.id,
            param,
            key,
        })
        .unwrap();
        let mut h = harness(p.project().clone());
        h.state_mut().0.selected.insert(c.id);
        h.run();
        // Deselected, the lanes stay open; only the toggle closes them.
        h.state_mut().0.selected.clear();
        h.run();
        let extra = |h: &egui_kittest::Harness<'_, (Timeline, History)>| {
            let t = &h.state().1.project().sequence().tracks()[0];
            h.state().0.keys.extra(t)
        };
        assert!(extra(&h) > 0.0, "lanes open");
        let lock = |h: &mut egui_kittest::Harness<'_, (Timeline, History)>, locked| {
            let mut v1 = h.state().1.project().sequence().tracks()[0].settings();
            v1.locked = locked;
            h.state_mut().1.apply(Command::SetTrack(v1)).unwrap();
            h.run();
        };
        lock(&mut h, true);
        assert_eq!(extra(&h), 0.0, "locked, no lanes");
        lock(&mut h, false);
        assert!(extra(&h) > 0.0, "unlocked, lanes again");
    }

    /// A refused drag held at the edge leaves the view where it was.
    #[test]
    fn a_refused_drag_at_the_edge_doesnt_scroll() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness(locked_v1());
        let x = size::TRACK_HEADER + 20.0;
        let y = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
        h.hover_at(pos2(x, y));
        h.run();
        h.drag_at(pos2(x, y));
        h.run();
        h.hover_at(pos2(995.0, y));
        for _ in 0..4 {
            h.step();
        }
        assert_eq!(h.state().0.scroll, 0.0);
        h.drop_at(pos2(995.0, y));
        h.run();
    }

    #[test]
    fn dragging_a_clip_tied_to_a_locked_track_is_refused() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness(locked_v1());
        let before = h.state().1.project().clone();
        let lanes: Vec<f32> = h
            .get_all_by_label("Lock")
            .map(|n| n.rect().center().y)
            .collect();
        // V1's own clip, then the unlocked A1 clip linked to it; mid-clip, then on its tail.
        let x = size::TRACK_HEADER + 20.0;
        let tail = 8.0 + size::TRACK_HEADER + 36.0;
        for (x, y) in [(x, lanes[0]), (x, lanes[1]), (tail, lanes[1])] {
            h.hover_at(pos2(x, y));
            h.run();
            h.drag_at(pos2(x, y));
            h.run();
            for dx in [10.0, 40.0] {
                h.hover_at(pos2(x + dx, y));
                h.run();
                let t = &h.state().0;
                assert!(t.drag_to.is_none() && t.trim.is_none(), "no ghost");
                let cursor = h.output().platform_output.cursor_icon;
                assert_eq!(cursor, CursorIcon::NotAllowed);
            }
            h.drop_at(pos2(x + 40.0, y));
            h.run();
            assert_eq!(h.state().1.project(), &before);
            assert!(!h.state().1.can_undo(), "nothing on the undo stack");
        }
    }

    #[test]
    fn wheel_scrolls_and_zooms_over_clips_and_empty_lanes() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness(project(2));
        let lane_y = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
        let wheel =
            |h: &mut egui_kittest::Harness<'_, (Timeline, History)>, at, delta, modifiers| {
                h.hover_at(at);
                h.run();
                h.event(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta,
                    modifiers,
                    phase: egui::TouchPhase::Move,
                });
                h.event(egui::Event::PointerMoved(at));
                for _ in 0..10 {
                    h.run();
                }
            };
        let none = egui::Modifiers::NONE;
        // Over a clip (1s in) and over empty lane far right.
        for x in [size::TRACK_HEADER + 20.0, 900.0] {
            let at = pos2(x, lane_y);
            h.state_mut().0.scroll = 5.0;
            wheel(&mut h, at, egui::vec2(-40.0, 0.0), none);
            assert!(h.state().0.scroll > 5.0, "x scroll at {x}");
            let before = h.state().0.scroll;
            wheel(&mut h, at, egui::vec2(0.0, -40.0), egui::Modifiers::SHIFT);
            assert!(h.state().0.scroll > before, "shift scroll at {x}");
            let zoom = h.state().0.zoom;
            wheel(&mut h, at, egui::vec2(0.0, 40.0), egui::Modifiers::COMMAND);
            assert!(h.state().0.zoom > zoom, "zoom at {x}");
        }
    }

    #[test]
    fn view_pages_with_playback_and_follows_seeks_but_not_manual_scroll() {
        let p = project(40);
        let mut t = Timeline::default();
        t.sync(&p);
        t.width = 400.0; // 20s across at the default zoom.
        // (a) Playback, frame by frame: the playhead never leaves view, and pages at the edge.
        for f in 0..30 * 25 {
            t.seek(Time(f * S / 30));
            t.follow();
            let s = secs(t.playhead());
            assert!(s >= t.scroll && s < t.scroll + t.span(), "{s} out of view");
        }
        assert_eq!(t.scroll, 20.0, "paged so the playhead sat at the left edge");
        // (b) A seek or step out of view brings it back.
        assert!(t.action("go_to_start", &p));
        t.follow();
        assert_eq!(t.scroll, 0.0);
        t.seek(Time(45 * S));
        t.follow();
        assert_eq!(t.scroll, 45.0);
        // Manual scrolling with the playhead still isn't undone.
        t.scroll = 0.0;
        t.follow();
        assert_eq!(t.scroll, 0.0);
    }

    #[test]
    fn scrollbar_drag_scrolls_and_zoom_to_fit_shows_every_clip() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness(project(20));
        h.state_mut().0.zoom = 200.0;
        h.run();
        // The thumb sits at the left of a bar along the lanes' bottom edge.
        let from = pos2(size::TRACK_HEADER + 10.0, 195.0);
        h.event(egui::Event::PointerMoved(from));
        h.event(egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
        for dx in [10.0, 50.0, 100.0] {
            h.event(egui::Event::PointerMoved(from + vec2(dx, 0.0)));
            h.run();
        }
        h.event(egui::Event::PointerButton {
            pos: from + vec2(100.0, 0.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
        assert!(h.state().0.scroll > 1.0, "scroll {}", h.state().0.scroll);

        let p = h.state().1.project().clone();
        assert!(h.state_mut().0.action("zoom_fit", &p));
        h.run();
        let t = &h.state().0;
        assert_eq!(t.scroll, 0.0);
        assert!(
            secs(p.sequence().duration()) <= t.span(),
            "last clip in view"
        );
    }

    /// The bin and the timeline side by side: dragging a tile or row onto a new project's
    /// empty timeline adds V1 and A1 and places linked clips there, as one undo step.
    #[test]
    fn bin_drag_onto_an_empty_timeline_adds_tracks() {
        use crate::media_bin::MediaBin;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut results = egui_kittest::SnapshotResults::new();
        for list in [false, true] {
            let mut p = History::new(Project::default());
            let probe = project(0).assets()[0].probe.clone().unwrap();
            let _ = p.apply(Command::Import(vec![(
                PathBuf::from("/nonexistent/a.mp4"),
                probe,
            )]));
            let p = p.project().clone();
            let mut fresh = true;
            let mut bin = MediaBin::default();
            bin.list = list;
            let mut h = egui_kittest::Harness::builder()
                .with_size([1000.0, 300.0])
                .wgpu()
                .build_ui_state(
                    move |ui, (b, t, h): &mut (MediaBin, Timeline, History)| {
                        if suite_ui::test_support::install_theme(ui, &mut fresh) {
                            return;
                        }
                        let r = ui.max_rect();
                        ui.painter().rect_filled(r, 0.0, color::panel());
                        let (left, right) = r.split_left_right_at_x(300.0);
                        let missing = HashSet::new();
                        ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
                            b.show(ui, h.project(), true, false, &missing)
                        });
                        let edit = ui
                            .scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
                                t.show(ui, h.project(), &missing, &Keymap::default())
                            })
                            .inner;
                        if let Some((c, _)) = edit {
                            h.apply(c).unwrap();
                        }
                    },
                    (bin, Timeline::default(), History::new(p)),
                );
            h.run();
            let from = pos2(80.0, if list { 90.0 } else { 100.0 });
            let to = pos2(300.0 + size::TRACK_HEADER + 100.0, 150.0);
            h.hover_at(from);
            h.run();
            h.drag_at(from);
            h.run();
            for f in [0.1, 0.5, 1.0] {
                h.hover_at(from + (to - from) * f);
                h.run();
            }
            if !list {
                results.add(h.try_snapshot("bin_drag_onto_empty_timeline"));
            }
            h.drop_at(to);
            h.run();
            let s = h.state().2.project().sequence().clone();
            let kinds: Vec<_> = s.tracks().iter().map(|t| t.kind).collect();
            assert_eq!(kinds, [TrackKind::Video, TrackKind::Audio], "list: {list}");
            let clips: Vec<_> = s.tracks().iter().map(|t| t.clips()[0].clone()).collect();
            assert!(clips[0].link.is_some() && clips[0].link == clips[1].link);
            assert!(
                (clips[0].start.0 - 5 * S).abs() <= S,
                "{:?}",
                clips[0].start
            );
            assert!(h.state_mut().2.undo());
            assert!(h.state().2.project().sequence().tracks().is_empty());
        }
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
        let img = egui::ColorImage::filled([160, 90], color::blue());
        let tex = ctx.load_texture("strip", img, Default::default());
        t.media
            .strips
            .insert(path.clone(), Some(vec![tex.clone(); 10]));
        let finest =
            (0..750).map(|i| [-(i as f32 * 0.1).sin().abs(), (i as f32 * 0.1).sin().abs()]);
        t.media
            .peaks
            .insert((path, 0), Some(Pyramid::new(finest.collect())));
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 400.0))),
            ..Default::default()
        };
        let mut frame = || {
            let out = ctx.run_ui(input(), |ui| {
                egui::CentralPanel::default()
                    .show(ui, |ui| t.show(ui, &p, &HashSet::new(), &Keymap::default()));
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

#[cfg(test)]
mod multitrack_tests {
    use jvid_core::history::{Command, History};
    use jvid_core::model::{Edge, Project, TrackKind};
    use jvid_core::time::{FLICKS_PER_SECOND, Time};

    /// The OBS-style corpus file: one video clip and three linked audio clips on three tracks,
    /// named from the stream titles, that move and trim with the video.
    #[test]
    fn three_stream_mkv_places_linked_clip_per_stream() {
        let Some(dir) = jvid_media::test_media::corpus() else {
            return;
        };
        jvid_media::ffmpeg::init().unwrap();
        let placed = |merge| {
            let path = dir.join("three-audio.mkv");
            let probe = jvid_media::probe::probe(&path).unwrap();
            let mut h = History::new(Project::default());
            h.apply(Command::Import(vec![(path, probe)])).unwrap();
            let asset = h.project().assets()[0].id;
            let p = h.project().placement(asset, Time(0), false, None, merge);
            h.apply(Command::Place(p.unwrap())).unwrap();
            h
        };
        let mut h = placed(false);
        let seq = h.project().sequence();
        let audio: Vec<_> = seq
            .tracks()
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .collect();
        let names: Vec<_> = audio.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Game", "Mic", "Voice chat"]);
        let streams: Vec<_> = audio.iter().map(|t| t.clips()[0].stream).collect();
        assert_eq!(streams, [Some(0), Some(1), Some(2)]);
        let video = seq
            .tracks()
            .iter()
            .find(|t| t.kind == TrackKind::Video)
            .unwrap();
        let v = video.clips()[0].id;
        assert_eq!(video.clips().len(), 1);
        assert_eq!(seq.linked(v).len(), 4);

        let sec = Time(FLICKS_PER_SECOND);
        let track = video.id;
        h.apply(Command::DragClip {
            id: v,
            track,
            start: sec,
        })
        .unwrap();
        let trims = vec![(v, Edge::Tail, Time(-FLICKS_PER_SECOND))];
        h.apply(Command::Trim {
            trims,
            ripple: false,
        })
        .unwrap();
        let seq = h.project().sequence();
        let (_, vc) = seq.clip(v).unwrap();
        for (_, c) in seq.linked(v) {
            assert_eq!((c.start, c.end()), (vc.start, vc.end()));
        }
        assert_eq!(vc.start, sec);

        // Merged: one audio clip mixing every stream.
        let h = placed(true);
        let seq = h.project().sequence();
        let audio: Vec<_> = seq
            .tracks()
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .collect();
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0].clips()[0].stream, None);
    }
}
