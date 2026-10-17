//! The in-memory project: settings, media assets, a sequence of tracks and
//! clips, markers and in/out points. No I/O; probing fills `MediaAsset::probe`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::keyframe::Keyframe;
use crate::time::{Rate, Time};

macro_rules! id {
    ($name:ident) => {
        /// Stable id, unique within its project or sequence and never reused.
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);
    };
}
id!(AssetId);
id!(TrackId);
id!(ClipId);
id!(MarkerId);
id!(LinkId);

/// Project-wide output settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSettings {
    pub width: u32,
    pub height: u32,
    pub frame_rate: Rate,
    pub sample_rate: u32,
    /// Straight RGBA.
    pub background: [u8; 4],
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            frame_rate: Rate::new(30, 1),
            sample_rate: 48_000,
            background: [0, 0, 0, 255],
        }
    }
}

/// What probing found in a media file.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ProbeInfo {
    pub duration: Time,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<Rate>,
    pub sample_rate: Option<u32>,
    pub audio_streams: u32,
    /// Demuxer name(s), e.g. `mov,mp4,m4a,3gp,3g2,mj2`.
    #[serde(default)]
    pub container: String,
    /// Every stream in file order.
    #[serde(default)]
    pub streams: Vec<StreamInfo>,
    /// Bytes on disk when probed; relink uses it to tell same-named files apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size: Option<u64>,
}

/// One stream of a probed file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamInfo {
    Video(VideoInfo),
    Audio(AudioInfo),
    /// Subtitles, data, attachments: kept so stream indices line up, otherwise ignored.
    Other {
        codec: String,
    },
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VideoInfo {
    pub codec: String,
    /// Coded size, before rotation.
    pub width: u32,
    pub height: u32,
    pub sample_aspect: Option<Rate>,
    pub display_aspect: Option<Rate>,
    /// Frames over duration.
    pub avg_frame_rate: Option<Rate>,
    /// Lowest rate that represents every timestamp (FFmpeg's `r_frame_rate`).
    pub real_frame_rate: Option<Rate>,
    /// Variable frame rate: average and real rates disagree.
    pub vfr: bool,
    /// Clockwise degrees to turn decoded frames upright: 0, 90, 180 or 270.
    pub rotation: u32,
    /// FFmpeg names, e.g. `bt709`; `None` when unspecified.
    pub color_primaries: Option<String>,
    pub color_transfer: Option<String>,
    /// `tv` (limited) or `pc` (full).
    pub color_range: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AudioInfo {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    /// FFmpeg channel mask (`AV_CH_*` bits); 0 when unspecified.
    pub channel_layout: u64,
    /// Stream title tag, e.g. "Mic" in an OBS recording.
    pub title: Option<String>,
}

/// A source file referenced by clips.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaAsset {
    pub id: AssetId,
    pub path: PathBuf,
    /// `None` until the file has been probed.
    pub probe: Option<ProbeInfo>,
    /// The bin name; `None` shows the file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Source in/out marks, set in the Source viewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_point: Option<Time>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_point: Option<Time>,
    /// A generated solid of this straight RGBA (0..=1) instead of a file; `path` is empty.
    /// Clips override it with a `color` keyframe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solid: Option<[f64; 4]>,
}

impl MediaAsset {
    pub fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            self.path.file_name().map_or_else(
                || self.path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    pub hidden: bool,
    pub locked: bool,
    pub muted: bool,
    pub solo: bool,
    /// Linear gain, 1.0 = unity.
    pub volume: f32,
    /// -1.0 (left) to 1.0 (right).
    pub pan: f32,
    /// Lane height in points, [`Track::MIN_HEIGHT`] to [`Track::MAX_HEIGHT`].
    #[serde(default = "Track::default_height")]
    pub height: f32,
    /// Insert and overwrite land here; at most one track of each kind.
    #[serde(default)]
    pub target: bool,
    /// Sorted by `start`, never overlapping. Mutate through `Sequence`.
    clips: Vec<Clip>,
}

/// A track's settings: everything but its kind and clips.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackSettings {
    pub id: TrackId,
    pub name: String,
    pub hidden: bool,
    pub locked: bool,
    pub muted: bool,
    pub solo: bool,
    pub volume: f32,
    pub pan: f32,
    pub height: f32,
    pub target: bool,
}

impl Track {
    pub const MIN_HEIGHT: f32 = 24.0;
    pub const MAX_HEIGHT: f32 = 120.0;

    pub fn default_height() -> f32 {
        40.0
    }

    pub fn settings(&self) -> TrackSettings {
        TrackSettings {
            id: self.id,
            name: self.name.clone(),
            hidden: self.hidden,
            locked: self.locked,
            muted: self.muted,
            solo: self.solo,
            volume: self.volume,
            pan: self.pan,
            height: self.height,
            target: self.target,
        }
    }

    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    /// The first clip other than `ignore` intersecting `[start, end)`.
    fn overlap(&self, start: Time, end: Time, ignore: Option<ClipId>) -> Option<ClipId> {
        self.clips
            .iter()
            .find(|c| Some(c.id) != ignore && c.start < end && start < c.end())
            .map(|c| c.id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub asset: AssetId,
    pub source_in: Time,
    pub source_out: Time,
    /// Where the clip starts on the timeline.
    pub start: Time,
    /// Clips sharing a link group move and select together.
    pub link: Option<LinkId>,
    pub params: BTreeMap<String, f64>,
    /// Animated params by name, sorted by time; see [`crate::keyframe`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keyframes: BTreeMap<String, Vec<Keyframe>>,
    /// Static transform, crop and opacity; keyframes of the same names override each field.
    #[serde(default, skip_serializing_if = "Transform::is_default")]
    pub transform: Transform,
}

/// Where a clip lands in the frame, in project pixels. The clip is first fitted whole and
/// centred, as in the compositor; this then applies in the fitted image's pixels.
///
/// Keyframe names: `position`, `anchor` (`Vec2`), `scale` (`Scalar` for linked, `Vec2` for
/// separate X/Y), `rotation`, `opacity`, `crop_left`, `crop_top`, `crop_right`, `crop_bottom`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transform {
    /// Where the anchor lands, as an offset from the frame centre, y down.
    pub position: [f64; 2],
    pub scale: [f64; 2],
    /// Degrees clockwise about the anchor.
    pub rotation: f64,
    /// Offset from the image centre, y down; the point that position places and rotation pivots on.
    pub anchor: [f64; 2],
    /// Pixels removed from the left, top, right and bottom; the rest stays where it was.
    pub crop: [f64; 4],
    /// 0..=1.
    pub opacity: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0; 2],
            scale: [1.0; 2],
            rotation: 0.0,
            anchor: [0.0; 2],
            crop: [0.0; 4],
            opacity: 1.0,
        }
    }
}

impl Transform {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl Clip {
    pub fn duration(&self) -> Time {
        self.source_out - self.source_in
    }

    /// The animated value of `param` at timeline time `t`.
    pub fn value_at(&self, param: &str, t: Time) -> Option<crate::keyframe::Value> {
        let keys = self.keyframes.get(param)?;
        crate::keyframe::value_at(keys, self.source_in + (t - self.start))
    }

    /// The transform at timeline time `t`, keyframes applied over the static values.
    pub fn transform_at(&self, t: Time) -> Transform {
        use crate::keyframe::Value::{Scalar, Vec2};
        let mut x = self.transform;
        let vec2 = |name, v: &mut [f64; 2]| {
            if let Some(Vec2(n)) = self.value_at(name, t) {
                *v = n;
            }
        };
        vec2("position", &mut x.position);
        vec2("anchor", &mut x.anchor);
        match self.value_at("scale", t) {
            Some(Scalar(s)) => x.scale = [s; 2],
            Some(Vec2(s)) => x.scale = s,
            _ => {}
        }
        let scalar = |name, v: &mut f64| {
            if let Some(Scalar(n)) = self.value_at(name, t) {
                *v = n;
            }
        };
        let crop = ["crop_left", "crop_top", "crop_right", "crop_bottom"];
        for (name, v) in crop.into_iter().zip(&mut x.crop) {
            scalar(name, v);
        }
        scalar("rotation", &mut x.rotation);
        scalar("opacity", &mut x.opacity);
        x.opacity = x.opacity.clamp(0.0, 1.0);
        x
    }

    /// Exclusive end on the timeline.
    pub fn end(&self) -> Time {
        self.start + self.duration()
    }
}

/// A point marker, or a ranged one when `duration` is non-zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: MarkerId,
    pub time: Time,
    pub duration: Time,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelError {
    UnknownTrack(TrackId),
    UnknownClip(ClipId),
    /// The clip would overlap another clip on the same track.
    Overlap(ClipId),
    /// `source_out` is not after `source_in`.
    EmptyClip,
    UnknownMarker(MarkerId),
    UnknownAsset(AssetId),
    /// No keyframe on the clip's param at that source time.
    UnknownKeyframe(ClipId, Time),
    /// The edit touches a locked track.
    Locked(TrackId),
    /// The clip can't go on a track of that kind.
    WrongKind(TrackId),
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTrack(id) => write!(f, "unknown track {}", id.0),
            Self::UnknownClip(id) => write!(f, "unknown clip {}", id.0),
            Self::Overlap(id) => write!(f, "clip would overlap clip {}", id.0),
            Self::EmptyClip => f.write_str("clip source out must be after source in"),
            Self::UnknownMarker(id) => write!(f, "unknown marker {}", id.0),
            Self::UnknownAsset(id) => write!(f, "unknown asset {}", id.0),
            Self::UnknownKeyframe(id, t) => write!(f, "no keyframe at {} on clip {}", t.0, id.0),
            Self::Locked(id) => write!(f, "track {} is locked", id.0),
            Self::WrongKind(id) => write!(f, "track {} is the wrong kind", id.0),
        }
    }
}

impl std::error::Error for ModelError {}

/// Tracks (video top-first, then audio), markers and in/out points.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Sequence {
    tracks: Vec<Track>,
    pub(crate) markers: Vec<Marker>,
    pub(crate) in_point: Option<Time>,
    pub(crate) out_point: Option<Time>,
    next_id: u64,
}

/// A clip to place; the sequence assigns its id.
#[derive(Debug, Clone, PartialEq)]
pub struct NewClip {
    pub asset: AssetId,
    pub source_in: Time,
    pub source_out: Time,
    pub start: Time,
    pub link: Option<LinkId>,
}

/// A source range to put on the timeline at `start`, as linked video and audio clips.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub asset: AssetId,
    pub source_in: Time,
    pub source_out: Time,
    pub start: Time,
    pub video: Option<TrackId>,
    pub audio: Option<TrackId>,
    /// Make a new track for `video` or `audio` when it's `None`, for a file with no track of
    /// that kind to land on.
    pub add_video: bool,
    pub add_audio: bool,
    /// Insert pushes every unlocked track right to make room; overwrite replaces what's there.
    pub insert: bool,
}

impl Sequence {
    fn alloc(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }

    pub fn in_point(&self) -> Option<Time> {
        self.in_point
    }

    pub fn out_point(&self) -> Option<Time> {
        self.out_point
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// Track settings (name, flags, volume, pan) are free to edit; clips are not.
    pub(crate) fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// A new video track goes on top of the stack, a new audio track at the bottom.
    pub(crate) fn add_track(&mut self, kind: TrackKind, name: impl Into<String>) -> TrackId {
        let id = TrackId(self.alloc());
        let at = match kind {
            TrackKind::Video => 0,
            TrackKind::Audio => self.tracks.len(),
        };
        self.tracks.insert(
            at,
            Track {
                id,
                kind,
                name: name.into(),
                hidden: false,
                locked: false,
                muted: false,
                solo: false,
                volume: 1.0,
                pan: 0.0,
                height: Track::default_height(),
                target: false,
                clips: Vec::new(),
            },
        );
        id
    }

    /// Replaces a track's settings, keeping one target per kind.
    pub(crate) fn set_track(&mut self, s: TrackSettings) -> Result<(), ModelError> {
        let t = self.track_mut(s.id).ok_or(ModelError::UnknownTrack(s.id))?;
        let kind = t.kind;
        t.name = s.name;
        t.hidden = s.hidden;
        t.locked = s.locked;
        t.muted = s.muted;
        t.solo = s.solo;
        t.volume = s.volume;
        t.pan = s.pan;
        t.height = s.height.clamp(Track::MIN_HEIGHT, Track::MAX_HEIGHT);
        t.target = s.target;
        if s.target {
            for o in self
                .tracks
                .iter_mut()
                .filter(|o| o.kind == kind && o.id != s.id)
            {
                o.target = false;
            }
        }
        Ok(())
    }

    /// Moves a track to `index` among the tracks of its kind (0 = top).
    pub(crate) fn move_track(&mut self, id: TrackId, index: usize) -> Result<(), ModelError> {
        let t = self.remove_track(id)?;
        let first = self.tracks.iter().position(|o| o.kind == t.kind);
        let count = self.tracks.iter().filter(|o| o.kind == t.kind).count();
        let first = first.unwrap_or(match t.kind {
            TrackKind::Video => 0,
            TrackKind::Audio => self.tracks.len(),
        });
        self.tracks.insert(first + index.min(count), t);
        Ok(())
    }

    /// Removes the track and its clips.
    pub(crate) fn remove_track(&mut self, id: TrackId) -> Result<Track, ModelError> {
        let i = self
            .tracks
            .iter()
            .position(|t| t.id == id)
            .ok_or(ModelError::UnknownTrack(id))?;
        Ok(self.tracks.remove(i))
    }

    pub(crate) fn new_link(&mut self) -> LinkId {
        LinkId(self.alloc())
    }

    pub(crate) fn add_clip(&mut self, track: TrackId, new: NewClip) -> Result<ClipId, ModelError> {
        if new.source_out <= new.source_in {
            return Err(ModelError::EmptyClip);
        }
        let end = new.start + (new.source_out - new.source_in);
        self.check_free(track, new.start, end, None)?;
        let id = ClipId(self.alloc());
        let t = self.track_mut(track).expect("checked above");
        insert_sorted(
            &mut t.clips,
            Clip {
                id,
                asset: new.asset,
                source_in: new.source_in,
                source_out: new.source_out,
                start: new.start,
                link: new.link,
                params: BTreeMap::new(),
                keyframes: BTreeMap::new(),
                transform: Transform::default(),
            },
        );
        Ok(id)
    }

    pub fn clip(&self, id: ClipId) -> Option<(TrackId, &Clip)> {
        self.tracks
            .iter()
            .find_map(|t| t.clips.iter().find(|c| c.id == id).map(|c| (t.id, c)))
    }

    /// Fields that cannot break the overlap invariant: asset, link, params.
    pub(crate) fn with_clip_mut<R>(
        &mut self,
        id: ClipId,
        f: impl FnOnce(&mut ClipProps<'_>) -> R,
    ) -> Result<R, ModelError> {
        let c = self
            .tracks
            .iter_mut()
            .find_map(|t| t.clips.iter_mut().find(|c| c.id == id))
            .ok_or(ModelError::UnknownClip(id))?;
        Ok(f(&mut ClipProps {
            asset: &mut c.asset,
            link: &mut c.link,
            params: &mut c.params,
            keyframes: &mut c.keyframes,
        }))
    }

    pub(crate) fn remove_clip(&mut self, id: ClipId) -> Result<(TrackId, Clip), ModelError> {
        for t in &mut self.tracks {
            if let Some(i) = t.clips.iter().position(|c| c.id == id) {
                return Ok((t.id, t.clips.remove(i)));
            }
        }
        Err(ModelError::UnknownClip(id))
    }

    /// Moves a clip to `start` on `track` (which may be its current track).
    pub(crate) fn move_clip(
        &mut self,
        id: ClipId,
        track: TrackId,
        start: Time,
    ) -> Result<(), ModelError> {
        let (_, c) = self.clip(id).ok_or(ModelError::UnknownClip(id))?;
        let end = start + c.duration();
        self.check_free(track, start, end, Some(id))?;
        let (_, mut c) = self.remove_clip(id)?;
        c.start = start;
        insert_sorted(&mut self.track_mut(track).expect("checked").clips, c);
        Ok(())
    }

    /// Trims a clip: new source range and timeline start.
    pub(crate) fn set_clip_range(
        &mut self,
        id: ClipId,
        start: Time,
        source_in: Time,
        source_out: Time,
    ) -> Result<(), ModelError> {
        if source_out <= source_in {
            return Err(ModelError::EmptyClip);
        }
        let (track, _) = self.clip(id).ok_or(ModelError::UnknownClip(id))?;
        self.check_free(track, start, start + (source_out - source_in), Some(id))?;
        let (_, mut c) = self.remove_clip(id)?;
        c.start = start;
        c.source_in = source_in;
        c.source_out = source_out;
        insert_sorted(&mut self.track_mut(track).expect("checked").clips, c);
        Ok(())
    }

    fn check_free(
        &self,
        track: TrackId,
        start: Time,
        end: Time,
        ignore: Option<ClipId>,
    ) -> Result<(), ModelError> {
        let t = self.track(track).ok_or(ModelError::UnknownTrack(track))?;
        match t.overlap(start, end, ignore) {
            Some(other) => Err(ModelError::Overlap(other)),
            None => Ok(()),
        }
    }

    pub(crate) fn add_marker(
        &mut self,
        time: Time,
        duration: Time,
        name: impl Into<String>,
    ) -> MarkerId {
        let id = MarkerId(self.alloc());
        self.markers.push(Marker {
            id,
            time,
            duration,
            name: name.into(),
        });
        id
    }

    pub(crate) fn remove_marker(&mut self, id: MarkerId) -> Option<Marker> {
        let i = self.markers.iter().position(|m| m.id == id)?;
        Some(self.markers.remove(i))
    }

    /// Clips covering `t` (start inclusive, end exclusive), in track order.
    pub fn clips_at(&self, t: Time) -> impl Iterator<Item = (TrackId, &Clip)> {
        self.clips_in(t, t + Time(1))
    }

    /// Clips intersecting the half-open range `[start, end)`, in track order.
    pub fn clips_in(&self, start: Time, end: Time) -> impl Iterator<Item = (TrackId, &Clip)> {
        self.tracks.iter().flat_map(move |tr| {
            tr.clips
                .iter()
                .filter(move |c| start < end && c.start < end && start < c.end())
                .map(move |c| (tr.id, c))
        })
    }

    fn edit_points(&self) -> impl Iterator<Item = Time> + '_ {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter().flat_map(|c| [c.start, c.end()]))
    }

    /// The first clip start or end strictly after `t`.
    pub fn next_edit(&self, t: Time) -> Option<Time> {
        self.edit_points().filter(|&p| p > t).min()
    }

    /// The last clip start or end strictly before `t`.
    pub fn previous_edit(&self, t: Time) -> Option<Time> {
        self.edit_points().filter(|&p| p < t).max()
    }

    /// End of the last clip, or zero when empty.
    pub fn duration(&self) -> Time {
        self.edit_points().max().unwrap_or_default()
    }
}

impl Sequence {
    fn index(&self, id: TrackId) -> Result<usize, ModelError> {
        let i = self.tracks.iter().position(|t| t.id == id);
        i.ok_or(ModelError::UnknownTrack(id))
    }

    fn kind_check(&self, id: TrackId, kind: TrackKind) -> Result<usize, ModelError> {
        let i = self.index(id)?;
        if self.tracks[i].kind != kind {
            return Err(ModelError::WrongKind(id));
        }
        Ok(i)
    }

    /// Cuts clip `id` on track `ti` at `at`. The tail gets a new id and, through `links`,
    /// a new link shared with the other tails of the same edit.
    fn split(&mut self, ti: usize, id: ClipId, at: Time, links: &mut BTreeMap<LinkId, LinkId>) {
        let tail_id = ClipId(self.alloc());
        let link = self.tracks[ti]
            .clips
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| c.link);
        let link = link.map(|l| *links.entry(l).or_insert_with(|| LinkId(self.alloc())));
        let clips = &mut self.tracks[ti].clips;
        let i = clips
            .iter()
            .position(|c| c.id == id)
            .expect("caller found it");
        let head = &mut clips[i];
        let cut = head.source_in + (at - head.start);
        let mut tail = head.clone();
        head.source_out = cut;
        (tail.id, tail.source_in, tail.start, tail.link) = (tail_id, cut, at, link);
        clips.insert(i + 1, tail);
    }

    /// Empties `[start, end)` on track `ti`, trimming or splitting clips that cross it.
    fn clear(&mut self, ti: usize, start: Time, end: Time, links: &mut BTreeMap<LinkId, LinkId>) {
        let hit = self.tracks[ti]
            .clips
            .iter()
            .filter(|c| c.start < end && start < c.end());
        let hit: Vec<_> = hit.map(|c| (c.id, c.start, c.end())).collect();
        for (id, s, e) in hit {
            if s < start && e > end {
                self.split(ti, id, end, links);
            }
            let clips = &mut self.tracks[ti].clips;
            let i = clips.iter().position(|c| c.id == id).expect("listed above");
            let c = &mut clips[i];
            if s >= start && e <= end {
                clips.remove(i);
            } else if s < start {
                c.source_out = c.source_in + (start - s);
            } else {
                c.source_in = c.source_in + (end - s);
                c.start = end;
            }
        }
    }

    /// Insert or overwrite edit, as Premiere's `,` and `.`: insert splits every unlocked
    /// track at `start` and pushes the rest right by the clip's length.
    pub(crate) fn place(&mut self, p: Placement) -> Result<Vec<ClipId>, ModelError> {
        if p.source_out <= p.source_in {
            return Err(ModelError::EmptyClip);
        }
        let len = p.source_out - p.source_in;
        let mut s = self.clone();
        let mut links = BTreeMap::new();
        let mut add = |t: Option<TrackId>, new, kind| match t {
            None if new => Some(s.add_track(kind, "")),
            t => t,
        };
        let video = add(p.video, p.add_video, TrackKind::Video);
        let audio = add(p.audio, p.add_audio, TrackKind::Audio);
        let targets = [(video, TrackKind::Video), (audio, TrackKind::Audio)];
        let mut tracks = Vec::new();
        for (t, kind) in targets {
            if let Some(t) = t {
                tracks.push((t, s.kind_check(t, kind)?));
            }
        }
        if p.insert {
            for ti in 0..s.tracks.len() {
                if s.tracks[ti].locked {
                    continue;
                }
                let crossing = s.tracks[ti].overlap(p.start, p.start, None);
                if let Some(id) = crossing {
                    s.split(ti, id, p.start, &mut links);
                }
                for c in &mut s.tracks[ti].clips {
                    if c.start >= p.start {
                        c.start = c.start + len;
                    }
                }
            }
        }
        let link = (tracks.len() > 1).then(|| s.new_link());
        let mut ids = Vec::new();
        for (t, ti) in tracks {
            s.clear(ti, p.start, p.start + len, &mut links);
            let clip = NewClip {
                asset: p.asset,
                source_in: p.source_in,
                source_out: p.source_out,
                start: p.start,
                link,
            };
            ids.push(s.add_clip(t, clip)?);
        }
        *self = s;
        Ok(ids)
    }

    /// The clip and its linked partners.
    pub fn linked(&self, id: ClipId) -> Vec<(TrackId, &Clip)> {
        let link = self.clip(id).and_then(|(_, c)| c.link);
        let all = self
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| (t.id, c)));
        all.filter(|(_, c)| c.id == id || (link.is_some() && c.link == link))
            .collect()
    }

    /// Drags clip `id` to `start` on `track`, its linked clips by the same time on their own
    /// tracks, overwriting whatever they land on. Nothing moves before zero.
    pub(crate) fn drag_clip(
        &mut self,
        id: ClipId,
        track: TrackId,
        start: Time,
    ) -> Result<(), ModelError> {
        let (from, c) = self.clip(id).ok_or(ModelError::UnknownClip(id))?;
        let kind = self.tracks[self.index(from)?].kind;
        self.kind_check(track, kind)?;
        let group: Vec<_> = self
            .linked(id)
            .iter()
            .map(|(t, c)| (*t, c.id, c.start))
            .collect();
        let earliest = group.iter().map(|g| g.2).min().expect("has the clip");
        let by = (start - c.start).max(Time(0) - earliest);
        let mut s = self.clone();
        let mut moved = Vec::new();
        for (t, cid, _) in group {
            let (_, c) = s.remove_clip(cid)?;
            moved.push((if cid == id { track } else { t }, c));
        }
        let mut links = BTreeMap::new();
        for (t, mut c) in moved {
            c.start = c.start + by;
            let ti = s.index(t)?;
            s.clear(ti, c.start, c.end(), &mut links);
            insert_sorted(&mut s.tracks[ti].clips, c);
        }
        *self = s;
        Ok(())
    }
}

impl Sequence {
    /// Removes the clips. Ripple closes each gap by pulling later clips on its track left.
    pub(crate) fn remove_clips(&mut self, ids: &[ClipId], ripple: bool) -> Result<(), ModelError> {
        let mut s = self.clone();
        let mut gone = Vec::new();
        for &id in ids {
            gone.push(s.remove_clip(id)?);
        }
        if ripple {
            gone.sort_by_key(|(_, c)| std::cmp::Reverse(c.start));
            for (t, c) in gone {
                let ti = s.index(t)?;
                for later in &mut s.tracks[ti].clips {
                    if later.start >= c.start {
                        later.start = later.start - c.duration();
                    }
                }
            }
        }
        *self = s;
        Ok(())
    }

    /// Overwrites copies of `clips` onto their tracks, the earliest starting at `at`. Copies
    /// get new ids, and new links shared the way the originals shared theirs.
    pub(crate) fn paste(
        &mut self,
        clips: &[(TrackId, Clip)],
        at: Time,
    ) -> Result<Vec<ClipId>, ModelError> {
        let Some(earliest) = clips.iter().map(|(_, c)| c.start).min() else {
            return Ok(Vec::new());
        };
        let mut s = self.clone();
        let mut links = BTreeMap::new();
        let mut fresh = BTreeMap::new();
        let mut ids = Vec::new();
        for (t, c) in clips {
            let ti = s.index(*t)?;
            let mut c = c.clone();
            c.id = ClipId(s.alloc());
            c.start = c.start + (at - earliest);
            c.link = c
                .link
                .map(|l| *fresh.entry(l).or_insert_with(|| s.new_link()));
            s.clear(ti, c.start, c.end(), &mut links);
            ids.push(c.id);
            insert_sorted(&mut s.tracks[ti].clips, c);
        }
        *self = s;
        Ok(ids)
    }

    /// Puts the clips in one new link group, or takes them out of theirs.
    pub(crate) fn link_clips(&mut self, ids: &[ClipId], linked: bool) -> Result<(), ModelError> {
        let mut s = self.clone();
        let link = linked.then(|| s.new_link());
        for &id in ids {
            s.with_clip_mut(id, |c| *c.link = link)?;
        }
        *self = s;
        Ok(())
    }
}

/// The clip fields `Sequence::with_clip_mut` exposes.
pub(crate) struct ClipProps<'a> {
    pub asset: &'a mut AssetId,
    pub link: &'a mut Option<LinkId>,
    pub params: &'a mut BTreeMap<String, f64>,
    pub keyframes: &'a mut BTreeMap<String, Vec<Keyframe>>,
}

fn insert_sorted(clips: &mut Vec<Clip>, c: Clip) {
    let i = clips.partition_point(|x| x.start < c.start);
    clips.insert(i, c);
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Project {
    pub(crate) settings: ProjectSettings,
    pub(crate) assets: Vec<MediaAsset>,
    pub(crate) sequence: Sequence,
    next_asset: u64,
}

/// Read-only from outside the crate; edit it through [`crate::history::History`].
impl Project {
    pub fn settings(&self) -> &ProjectSettings {
        &self.settings
    }

    pub fn assets(&self) -> &[MediaAsset] {
        &self.assets
    }

    pub fn sequence(&self) -> &Sequence {
        &self.sequence
    }

    pub(crate) fn add_asset(&mut self, path: impl Into<PathBuf>) -> AssetId {
        self.next_asset += 1;
        let id = AssetId(self.next_asset);
        self.assets.push(MediaAsset {
            id,
            path: path.into(),
            probe: None,
            name: None,
            in_point: None,
            out_point: None,
            solid: None,
        });
        id
    }

    pub fn asset(&self, id: AssetId) -> Option<&MediaAsset> {
        self.assets.iter().find(|a| a.id == id)
    }

    /// How many clips use the asset; removing it removes them too.
    pub fn clips_using(&self, id: AssetId) -> usize {
        let clips = self.sequence.tracks.iter().flat_map(|t| &t.clips);
        clips.filter(|c| c.asset == id).count()
    }

    /// Places `asset`'s source in/out (default the whole file) at `start`: on `on` for its
    /// kind, and on the targeted track, else the top unlocked one, for the other kind the
    /// file has, adding a track of a kind the sequence lacks. `None` when the file has no
    /// duration.
    pub fn placement(
        &self,
        asset: AssetId,
        start: Time,
        insert: bool,
        on: Option<TrackId>,
    ) -> Option<Placement> {
        let a = self.asset(asset)?;
        let probe = a.probe.as_ref()?;
        let source_in = a.in_point.unwrap_or_default();
        let source_out = a.out_point.unwrap_or(probe.duration);
        if source_out <= source_in {
            return None;
        }
        let tracks = self.sequence.tracks();
        let pick = |kind: TrackKind, has: bool| {
            let of_kind = || tracks.iter().filter(move |t| t.kind == kind);
            has.then(|| {
                of_kind()
                    .find(|t| Some(t.id) == on)
                    .or_else(|| of_kind().find(|t| t.target))
                    .or_else(|| of_kind().find(|t| !t.locked))
                    .map(|t| t.id)
            })
            .flatten()
        };
        let (has_video, has_audio) = (probe.width.is_some(), probe.audio_streams > 0);
        let video = pick(TrackKind::Video, has_video);
        let audio = pick(TrackKind::Audio, has_audio);
        let lacks = |kind| !tracks.iter().any(|t| t.kind == kind);
        let add_video = has_video && lacks(TrackKind::Video);
        let add_audio = has_audio && lacks(TrackKind::Audio);
        (video.is_some() || audio.is_some() || add_video || add_audio).then_some(Placement {
            asset,
            source_in,
            source_out,
            start: start.max(Time(0)),
            video,
            audio,
            add_video,
            add_audio,
            insert,
        })
    }

    pub(crate) fn asset_mut(&mut self, id: AssetId) -> Option<&mut MediaAsset> {
        self.assets.iter_mut().find(|a| a.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(start: i64, len: i64) -> NewClip {
        NewClip {
            asset: AssetId(1),
            source_in: Time(100),
            source_out: Time(100 + len),
            start: Time(start),
            link: None,
        }
    }

    fn seq() -> (Sequence, TrackId, TrackId) {
        let mut s = Sequence::default();
        let v = s.add_track(TrackKind::Video, "V1");
        let a = s.add_track(TrackKind::Audio, "A1");
        (s, v, a)
    }

    fn ids<'a>(it: impl Iterator<Item = (TrackId, &'a Clip)>) -> Vec<ClipId> {
        it.map(|(_, c)| c.id).collect()
    }

    /// (start, source_in, source_out) of each clip on the track.
    fn spans(s: &Sequence, t: TrackId) -> Vec<(i64, i64, i64)> {
        let clips = s.track(t).unwrap().clips();
        clips
            .iter()
            .map(|c| (c.start.0, c.source_in.0, c.source_out.0))
            .collect()
    }

    fn placement(v: TrackId, a: TrackId, start: i64, insert: bool) -> Placement {
        Placement {
            asset: AssetId(2),
            source_in: Time(1000),
            source_out: Time(1010),
            start: Time(start),
            video: Some(v),
            audio: Some(a),
            add_video: false,
            add_audio: false,
            insert,
        }
    }

    // Premiere, one clip at 0..30 on V1 and A1, source 1010..1020 placed at 10:
    // insert gives 0..10 | new 10..20 | tail 20..40; overwrite gives 0..10 | new | 20..30.
    #[test]
    fn insert_and_overwrite_match_premiere() {
        let (mut s, v, a) = seq();
        let l = s.new_link();
        for t in [v, a] {
            s.add_clip(
                t,
                NewClip {
                    link: Some(l),
                    ..clip(0, 30)
                },
            )
            .unwrap();
        }
        let mut ins = s.clone();
        let new = ins.place(placement(v, a, 10, true)).unwrap();
        for t in [v, a] {
            assert_eq!(
                spans(&ins, t),
                [(0, 100, 110), (10, 1000, 1010), (20, 110, 130)]
            );
        }
        // The two tails are linked to each other, not to the heads or the new clips.
        let tails: Vec<_> = [v, a].map(|t| ins.track(t).unwrap().clips()[2].link).into();
        assert_eq!(tails[0], tails[1]);
        assert_ne!(tails[0], Some(l));
        let new_link = ins.clip(new[0]).unwrap().1.link;
        assert!(new_link.is_some() && new_link == ins.clip(new[1]).unwrap().1.link);

        let mut ow = s.clone();
        ow.place(placement(v, a, 10, false)).unwrap();
        for t in [v, a] {
            assert_eq!(
                spans(&ow, t),
                [(0, 100, 110), (10, 1000, 1010), (20, 120, 130)]
            );
        }
        // Overwrite over a whole clip and part of the next.
        ow.place(Placement {
            start: Time(5),
            source_out: Time(1020),
            ..placement(v, a, 0, false)
        })
        .unwrap();
        assert_eq!(
            spans(&ow, v),
            [(0, 100, 105), (5, 1000, 1020), (25, 125, 130)]
        );
    }

    #[test]
    fn insert_skips_locked_tracks_and_checks_kind() {
        let (mut s, v, a) = seq();
        let a2 = s.add_track(TrackKind::Audio, "A2");
        s.add_clip(a2, clip(20, 10)).unwrap();
        s.track_mut(a2).unwrap().locked = true;
        s.add_clip(v, clip(20, 10)).unwrap();
        s.place(Placement {
            audio: None,
            ..placement(v, a, 0, true)
        })
        .unwrap();
        assert_eq!(spans(&s, v), [(0, 1000, 1010), (30, 100, 110)]);
        assert_eq!(spans(&s, a2), [(20, 100, 110)]);
        let wrong = Placement {
            video: Some(a),
            audio: None,
            ..placement(v, a, 0, false)
        };
        let before = s.clone();
        assert_eq!(s.place(wrong), Err(ModelError::WrongKind(a)));
        assert_eq!(s, before);
    }

    #[test]
    fn drag_moves_linked_clips_and_overwrites() {
        let (mut s, v, a) = seq();
        let v2 = s.add_track(TrackKind::Video, "V2");
        let l = s.new_link();
        let cv = s
            .add_clip(
                v,
                NewClip {
                    link: Some(l),
                    ..clip(0, 10)
                },
            )
            .unwrap();
        s.add_clip(
            a,
            NewClip {
                link: Some(l),
                ..clip(0, 10)
            },
        )
        .unwrap();
        s.add_clip(v2, clip(15, 10)).unwrap();
        s.add_clip(a, clip(15, 10)).unwrap();
        s.drag_clip(cv, v2, Time(10)).unwrap();
        assert_eq!(spans(&s, v), []);
        assert_eq!(spans(&s, v2), [(10, 100, 110), (20, 105, 110)]);
        assert_eq!(spans(&s, a), [(10, 100, 110), (20, 105, 110)]);
        // Never before zero; video can't land on audio.
        s.drag_clip(cv, v2, Time(-50)).unwrap();
        assert_eq!(s.clip(cv).unwrap().1.start, Time(0));
        assert_eq!(s.drag_clip(cv, a, Time(0)), Err(ModelError::WrongKind(a)));
    }

    #[test]
    fn delete_ripple_paste_and_link() {
        let (mut s, v, a) = seq();
        let l = s.new_link();
        let linked = NewClip {
            link: Some(l),
            ..clip(0, 10)
        };
        let cv = s.add_clip(v, linked.clone()).unwrap();
        let ca = s.add_clip(a, linked).unwrap();
        let later = s.add_clip(v, clip(20, 10)).unwrap();

        // Pasted copies overwrite at `at` and share a new link of their own.
        let copied: Vec<_> = [cv, ca]
            .map(|c| s.clip(c).map(|(t, c)| (t, c.clone())).unwrap())
            .into();
        let new = s.paste(&copied, Time(25)).unwrap();
        assert_eq!(
            spans(&s, v),
            [(0, 100, 110), (20, 100, 105), (25, 100, 110)]
        );
        let links: Vec<_> = new.iter().map(|c| s.clip(*c).unwrap().1.link).collect();
        assert!(links[0].is_some() && links[0] == links[1] && links[0] != Some(l));

        // Delete leaves the gap; ripple closes it on the clip's own track.
        let mut gap = s.clone();
        gap.remove_clips(&[cv], false).unwrap();
        assert_eq!(spans(&gap, v)[0], (20, 100, 105));
        s.remove_clips(&[cv, ca], true).unwrap();
        assert_eq!(spans(&s, v), [(10, 100, 105), (15, 100, 110)]);
        assert_eq!(spans(&s, a), [(15, 100, 110)]);
        let before = s.clone();
        assert_eq!(
            s.remove_clips(&[later, cv], false),
            Err(ModelError::UnknownClip(cv))
        );
        assert_eq!(s, before);

        // Unlink then relink.
        s.link_clips(&new, false).unwrap();
        assert_eq!(s.linked(new[0]).len(), 1);
        s.link_clips(&new, true).unwrap();
        assert_eq!(s.linked(new[0]).len(), 2);
    }

    #[test]
    fn placement_uses_source_marks_and_target_tracks() {
        let mut p = Project::default();
        let (v1, v2, a1) = (
            p.sequence.add_track(TrackKind::Video, "V1"),
            p.sequence.add_track(TrackKind::Video, "V2"),
            p.sequence.add_track(TrackKind::Audio, "A1"),
        );
        p.sequence.track_mut(v2).unwrap().target = true;
        let id = p.add_asset("a.mp4");
        let probe = ProbeInfo {
            duration: Time(100),
            width: Some(4),
            audio_streams: 1,
            ..Default::default()
        };
        p.asset_mut(id).unwrap().probe = Some(probe.clone());
        let pl = p.placement(id, Time(5), true, None).unwrap();
        assert_eq!((pl.source_in, pl.source_out), (Time(0), Time(100)));
        assert_eq!((pl.video, pl.audio), (Some(v2), Some(a1)));
        // A drop picks the track under the pointer for its kind.
        assert_eq!(
            p.placement(id, Time(5), false, Some(v1)).unwrap().video,
            Some(v1)
        );
        let a = p.asset_mut(id).unwrap();
        (a.in_point, a.out_point) = (Some(Time(10)), Some(Time(20)));
        let pl = p.placement(id, Time(-5), false, None).unwrap();
        assert_eq!(
            (pl.source_in, pl.source_out, pl.start),
            (Time(10), Time(20), Time(0))
        );
        // Audio-only file: no video clip.
        p.asset_mut(id).unwrap().probe = Some(ProbeInfo {
            width: None,
            ..probe
        });
        assert_eq!(p.placement(id, Time(0), false, None).unwrap().video, None);
    }

    #[test]
    fn placement_on_an_empty_sequence_adds_tracks() {
        let mut p = Project::default();
        let id = p.add_asset("a.mp4");
        p.asset_mut(id).unwrap().probe = Some(ProbeInfo {
            duration: Time(100),
            width: Some(4),
            audio_streams: 1,
            ..Default::default()
        });
        let pl = p.placement(id, Time(5), false, None).unwrap();
        assert_eq!(p.sequence.place(pl).unwrap().len(), 2);
        let kinds: Vec<_> = p.sequence.tracks().iter().map(|t| t.kind).collect();
        assert_eq!(kinds, [TrackKind::Video, TrackKind::Audio]);
    }

    #[test]
    fn transform_keyframes_override_static_values() {
        use crate::keyframe::{Easing, Value};
        let key = |t, value| Keyframe {
            time: Time(t),
            value,
            easing: Easing::Linear,
        };
        let mut c = Clip {
            id: ClipId(1),
            asset: AssetId(1),
            source_in: Time(100),
            source_out: Time(200),
            start: Time(0),
            link: None,
            params: BTreeMap::new(),
            keyframes: BTreeMap::new(),
            transform: Transform {
                position: [5.0, 6.0],
                rotation: 30.0,
                ..Default::default()
            },
        };
        assert_eq!(c.transform_at(Time(0)), c.transform);
        let mut k = |name: &str, keys: Vec<Keyframe>| c.keyframes.insert(name.into(), keys);
        // Keyframe times are source time: timeline 10 is source 110.
        k(
            "scale",
            vec![key(100, Value::Scalar(1.0)), key(120, Value::Scalar(3.0))],
        );
        k(
            "anchor",
            vec![
                key(100, Value::Vec2([0.0, 0.0])),
                key(120, Value::Vec2([10.0, -20.0])),
            ],
        );
        k("crop_top", vec![key(100, Value::Scalar(4.0))]);
        k("opacity", vec![key(100, Value::Scalar(2.0))]);
        let x = c.transform_at(Time(10));
        assert_eq!(x.scale, [2.0, 2.0]);
        assert_eq!(x.anchor, [5.0, -10.0]);
        assert_eq!(x.crop, [0.0, 4.0, 0.0, 0.0]);
        assert_eq!((x.position, x.rotation, x.opacity), ([5.0, 6.0], 30.0, 1.0));
        c.keyframes
            .insert("scale".into(), vec![key(0, Value::Vec2([0.5, 2.0]))]);
        assert_eq!(c.transform_at(Time(10)).scale, [0.5, 2.0]);
        // Old files without a transform load with the identity.
        let json =
            r#"{"id":1,"asset":1,"source_in":0,"source_out":1,"start":0,"link":null,"params":{}}"#;
        let old: Clip = serde_json::from_str(json).unwrap();
        assert_eq!(old.transform, Transform::default());
    }

    #[test]
    fn ids_are_unique_and_stable() {
        let (mut s, v, _) = seq();
        let a = s.add_clip(v, clip(0, 10)).unwrap();
        let b = s.add_clip(v, clip(10, 10)).unwrap();
        assert_ne!(a, b);
        s.remove_clip(a).unwrap();
        let c = s.add_clip(v, clip(0, 10)).unwrap();
        assert!(c != a && c != b);
        s.move_clip(b, v, Time(50)).unwrap();
        assert_eq!(s.clip(b).unwrap().1.start, Time(50));
    }

    #[test]
    fn add_rejects_overlap_but_allows_touching() {
        let (mut s, v, a) = seq();
        let x = s.add_clip(v, clip(10, 10)).unwrap();
        assert_eq!(s.add_clip(v, clip(15, 10)), Err(ModelError::Overlap(x)));
        assert_eq!(s.add_clip(v, clip(5, 10)), Err(ModelError::Overlap(x)));
        assert_eq!(s.add_clip(v, clip(0, 100)), Err(ModelError::Overlap(x)));
        assert_eq!(s.add_clip(v, clip(12, 2)), Err(ModelError::Overlap(x)));
        s.add_clip(v, clip(0, 10)).unwrap();
        s.add_clip(v, clip(20, 5)).unwrap();
        s.add_clip(a, clip(15, 10)).unwrap(); // other track is fine
        assert_eq!(s.add_clip(v, clip(30, 0)), Err(ModelError::EmptyClip));
        assert_eq!(
            s.add_clip(TrackId(99), clip(30, 1)),
            Err(ModelError::UnknownTrack(TrackId(99)))
        );
    }

    #[test]
    fn move_and_trim_reject_overlap() {
        let (mut s, v, a) = seq();
        let x = s.add_clip(v, clip(0, 10)).unwrap();
        let y = s.add_clip(v, clip(20, 10)).unwrap();
        assert_eq!(s.move_clip(x, v, Time(15)), Err(ModelError::Overlap(y)));
        s.move_clip(x, v, Time(5)).unwrap(); // overlapping its own old position is fine
        s.move_clip(x, a, Time(25)).unwrap();
        assert_eq!(s.clip(x).unwrap().0, a);
        s.move_clip(x, v, Time(0)).unwrap();
        assert_eq!(
            s.set_clip_range(x, Time(0), Time(0), Time(21)),
            Err(ModelError::Overlap(y))
        );
        s.set_clip_range(x, Time(0), Time(0), Time(20)).unwrap();
        assert_eq!(s.clip(x).unwrap().1.end(), Time(20));
        assert_eq!(
            s.set_clip_range(x, Time(0), Time(5), Time(5)),
            Err(ModelError::EmptyClip)
        );
        // failed mutations leave the clip where it was
        assert_eq!(s.clip(x).unwrap().1.start, Time(0));
        let starts: Vec<_> = s
            .track(v)
            .unwrap()
            .clips()
            .iter()
            .map(|c| c.start)
            .collect();
        assert_eq!(starts, [Time(0), Time(20)]);
    }

    #[test]
    fn clips_at_and_in_range() {
        let (mut s, v, a) = seq();
        let x = s.add_clip(v, clip(0, 10)).unwrap();
        let y = s.add_clip(v, clip(10, 10)).unwrap();
        let z = s.add_clip(a, clip(5, 10)).unwrap();
        assert_eq!(ids(s.clips_at(Time(0))), [x]);
        assert_eq!(ids(s.clips_at(Time(9))), [x, z]);
        assert_eq!(ids(s.clips_at(Time(10))), [y, z]); // end is exclusive
        assert_eq!(ids(s.clips_at(Time(20))), []);
        assert_eq!(ids(s.clips_in(Time(9), Time(11))), [x, y, z]);
        assert_eq!(ids(s.clips_in(Time(15), Time(30))), [y]);
        assert_eq!(ids(s.clips_in(Time(20), Time(30))), []);
        assert_eq!(ids(s.clips_in(Time(5), Time(5))), []);
    }

    #[test]
    fn next_and_previous_edit() {
        let (mut s, v, a) = seq();
        assert_eq!(s.next_edit(Time(0)), None);
        s.add_clip(v, clip(10, 10)).unwrap();
        s.add_clip(a, clip(15, 20)).unwrap();
        assert_eq!(s.next_edit(Time(0)), Some(Time(10)));
        assert_eq!(s.next_edit(Time(10)), Some(Time(15)));
        assert_eq!(s.next_edit(Time(15)), Some(Time(20)));
        assert_eq!(s.next_edit(Time(35)), None);
        assert_eq!(s.previous_edit(Time(35)), Some(Time(20)));
        assert_eq!(s.previous_edit(Time(15)), Some(Time(10)));
        assert_eq!(s.previous_edit(Time(10)), None);
        assert_eq!(s.duration(), Time(35));
    }

    #[test]
    fn markers_in_out_and_project() {
        let mut p = Project::default();
        let asset = p.add_asset("rec.mkv");
        p.asset_mut(asset).unwrap().probe = Some(ProbeInfo {
            duration: Time(1000),
            ..Default::default()
        });
        assert_eq!(
            p.asset(asset).unwrap().probe.as_ref().unwrap().duration,
            Time(1000)
        );
        let s = &mut p.sequence;
        let m = s.add_marker(Time(5), Time(0), "kill");
        s.add_marker(Time(9), Time(3), "round");
        assert_eq!(s.remove_marker(m).unwrap().name, "kill");
        assert_eq!(s.markers.len(), 1);
        s.in_point = Some(Time(1));
        s.out_point = Some(Time(4));
        let t = s.add_track(TrackKind::Video, "V1");
        let c = s
            .add_clip(
                t,
                NewClip {
                    asset,
                    ..clip(0, 10)
                },
            )
            .unwrap();
        let l = s.new_link();
        s.with_clip_mut(c, |p| {
            *p.link = Some(l);
            p.params.insert("opacity".into(), 0.5);
        })
        .unwrap();
        assert_eq!(s.clip(c).unwrap().1.link, Some(l));
        s.track_mut(t).unwrap().muted = true;
        assert_eq!(s.remove_track(t).unwrap().clips().len(), 1);
        assert_eq!(s.clip(c), None);
    }

    #[test]
    fn time_arithmetic() {
        assert_eq!(Time(5) + Time(3), Time(8));
        assert_eq!(Time(5) - Time(8), Time(-3));
    }
}
