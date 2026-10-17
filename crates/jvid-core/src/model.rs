//! The in-memory project: settings, media assets, a sequence of tracks and
//! clips, markers and in/out points. No I/O; probing fills `MediaAsset::probe`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::time::{Rate, Time};

macro_rules! id {
    ($name:ident) => {
        /// Stable id, unique within its project or sequence and never reused.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u64);
    };
}
id!(AssetId);
id!(TrackId);
id!(ClipId);
id!(MarkerId);
id!(LinkId);

/// Project-wide output settings.
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProbeInfo {
    pub duration: Time,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<Rate>,
    pub sample_rate: Option<u32>,
    pub audio_streams: u32,
}

/// A source file referenced by clips.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaAsset {
    pub id: AssetId,
    pub path: PathBuf,
    /// `None` until the file has been probed.
    pub probe: Option<ProbeInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq)]
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
    /// Sorted by `start`, never overlapping. Mutate through `Sequence`.
    clips: Vec<Clip>,
}

impl Track {
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

#[derive(Debug, Clone, PartialEq)]
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
}

impl Clip {
    pub fn duration(&self) -> Time {
        self.source_out - self.source_in
    }

    /// Exclusive end on the timeline.
    pub fn end(&self) -> Time {
        self.start + self.duration()
    }
}

/// A point marker, or a ranged one when `duration` is non-zero.
#[derive(Debug, Clone, PartialEq)]
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
        }
    }
}

impl std::error::Error for ModelError {}

/// Tracks (video top-first, then audio), markers and in/out points.
#[derive(Debug, Clone, PartialEq, Default)]
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

    pub(crate) fn add_track(&mut self, kind: TrackKind, name: impl Into<String>) -> TrackId {
        let id = TrackId(self.alloc());
        self.tracks.push(Track {
            id,
            kind,
            name: name.into(),
            hidden: false,
            locked: false,
            muted: false,
            solo: false,
            volume: 1.0,
            pan: 0.0,
            clips: Vec::new(),
        });
        id
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
}

fn insert_sorted(clips: &mut Vec<Clip>, c: Clip) {
    let i = clips.partition_point(|x| x.start < c.start);
    clips.insert(i, c);
}

#[derive(Debug, Clone, PartialEq, Default)]
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
        });
        id
    }

    pub fn asset(&self, id: AssetId) -> Option<&MediaAsset> {
        self.assets.iter().find(|a| a.id == id)
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
