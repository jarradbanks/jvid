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

    fn default_height() -> f32 {
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
