//! Commands and undo/redo. Every project change goes through [`History::apply`].
//!
//! Undo steps are project snapshots taken before each step: the project is
//! plain data with no media in it, so a clone is cheap and revert is exact.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use crate::keyframe::{self, Keyframe};
use crate::model::{
    AssetId, Clip, ClipId, Edge, LinkId, Marker, MarkerId, ModelError, NewClip, Placement,
    ProbeInfo, Project, ProjectSettings, Sequence, SequenceId, Text, TrackId, TrackKind,
    TrackSettings, Transform,
};
use crate::time::Time;

/// One edit to a project.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    AddTrack {
        kind: TrackKind,
        name: String,
    },
    RemoveTrack(TrackId),
    /// Replaces the track's settings; clips are untouched. Allowed while locked, so it
    /// can unlock.
    SetTrack(TrackSettings),
    /// Moves a track to `index` among the tracks of its kind (0 = top).
    MoveTrack {
        id: TrackId,
        index: usize,
    },
    AddClip {
        track: TrackId,
        clip: NewClip,
    },
    RemoveClip(ClipId),
    /// Insert or overwrite edit; creates the video clip first when there are two.
    Place(Placement),
    /// Moves a clip and its linked clips, overwriting what they land on.
    DragClip {
        id: ClipId,
        track: TrackId,
        start: Time,
    },
    /// Moves the selection and all linked partners by a common time/track offset.
    /// All moving clips are removed before any overwrite placement occurs.
    MoveSelection {
        ids: Vec<ClipId>,
        anchor: ClipId,
        track: TrackId,
        start: Time,
    },
    /// Removes clips; `ripple` closes the gaps on their tracks.
    RemoveClips {
        ids: Vec<ClipId>,
        ripple: bool,
    },
    /// Overwrites copies of clips onto their tracks, the earliest at `at`, keeping links.
    Paste {
        clips: Vec<(TrackId, Clip)>,
        at: Time,
    },
    /// Links the clips into one new group, or unlinks them.
    LinkClips {
        ids: Vec<ClipId>,
        linked: bool,
    },
    /// Enables or disables clips; disabled clips are skipped by evaluation.
    SetClipsEnabled {
        ids: Vec<ClipId>,
        enabled: bool,
    },
    MoveClip {
        id: ClipId,
        track: TrackId,
        start: Time,
    },
    TrimClip {
        id: ClipId,
        start: Time,
        source_in: Time,
        source_out: Time,
    },
    /// Moves clip edges by the given amounts, held to media and neighbours; see
    /// [`Project::trim_limit`]. Linked clips trim with them.
    Trim {
        trims: Vec<(ClipId, Edge, Time)>,
        ripple: bool,
    },
    /// Sets a clip's speed and direction, its linked clips' with it; see
    /// [`Sequence::set_speed`](crate::model::Sequence).
    SetClipSpeed {
        id: ClipId,
        speed: f64,
        reverse: bool,
        ripple: bool,
    },
    /// Keys a clip's speed, its linked clips' with it, with `speed` the constant speed they
    /// play at without keys; see [`Sequence::set_speed_keys`](crate::model::Sequence). The
    /// keyframe commands and `SetClipProps` retime the same way when they change the `speed`
    /// keys, with ripple.
    SetSpeedKeys {
        id: ClipId,
        speed: f64,
        keys: Vec<Keyframe>,
        ripple: bool,
    },
    /// Ripple deletes each `(clip, start, end)` timeline span from its clip and linked clips
    /// as one step, or with `speed` splits it out and plays it at that speed instead.
    CutSpans {
        spans: Vec<(ClipId, Time, Time)>,
        speed: Option<f64>,
    },
    /// Moves clips into a new sequence named `name` and puts one clip of it in their place;
    /// see [`Project::nest`]. `outer` names the open sequence in the bin if it isn't there.
    Nest {
        ids: Vec<ClipId>,
        name: String,
        outer: String,
    },
    /// Holds the frame shown at `at` for the rest of the clip.
    FreezeFrame {
        id: ClipId,
        at: Time,
    },
    /// Cuts the listed clips that cross `at` in two.
    Split {
        ids: Vec<ClipId>,
        at: Time,
    },
    /// Ripple deletes the gap containing `at` on the track.
    CloseGap {
        track: TrackId,
        at: Time,
    },
    NewLink,
    SetClipLink {
        id: ClipId,
        link: Option<LinkId>,
    },
    SetClipAsset {
        id: ClipId,
        asset: AssetId,
    },
    /// `None` removes the param.
    SetClipParam {
        id: ClipId,
        key: String,
        value: Option<f64>,
    },
    /// Adds a keyframe to a clip param, replacing one at the same time.
    AddKeyframe {
        id: ClipId,
        param: String,
        key: Keyframe,
    },
    /// Removes the keyframe at source time `time`.
    RemoveKeyframe {
        id: ClipId,
        param: String,
        time: Time,
    },
    /// Moves a keyframe to source time `to`, replacing one already there.
    MoveKeyframe {
        id: ClipId,
        param: String,
        from: Time,
        to: Time,
    },
    /// Replaces clips' static transforms and keyframes as one step, e.g. an inspector edit
    /// across a multi-clip selection.
    SetClipProps(Vec<(ClipId, Transform, BTreeMap<String, Vec<Keyframe>>)>),
    AddMarker {
        time: Time,
        duration: Time,
        name: String,
    },
    RemoveMarker(MarkerId),
    /// Replaces a marker's fields (name, color, note, range) by id.
    SetMarker(Marker),
    SetInOut {
        in_point: Option<Time>,
        out_point: Option<Time>,
    },
    /// Lifts the in/out range off every unlocked track, or with `ripple` extracts it.
    RemoveRange {
        ripple: bool,
    },
    SetSettings(ProjectSettings),
    AddAsset(PathBuf),
    /// Adds a solid color asset to the bin.
    AddSolid {
        name: String,
        color: [f64; 4],
    },
    SetSolidColor {
        id: AssetId,
        color: [f64; 4],
    },
    /// Adds a text layer asset to the bin.
    AddText {
        name: String,
        text: Text,
    },
    SetText {
        id: AssetId,
        text: Text,
    },
    /// Adds a subtitle track named `name` with a text clip in `style` per cue; a cue that
    /// overlaps the one before starts where it ends.
    ImportSubtitles {
        name: String,
        cues: Vec<crate::srt::Cue>,
        style: Text,
    },
    SetAssetProbe {
        id: AssetId,
        probe: Option<ProbeInfo>,
    },
    /// Adds probed media as one step, skipping paths already in the project.
    Import(Vec<(PathBuf, ProbeInfo)>),
    /// Sets the bin name; `None` or blank goes back to the file name.
    RenameAsset {
        id: AssetId,
        name: Option<String>,
    },
    /// Removes the asset and every clip that uses it; a sequence's takes the sequence too.
    RemoveAsset(AssetId),
    /// Points assets at new paths as one step, e.g. after their media moved.
    RelinkAssets(Vec<(AssetId, PathBuf)>),
    /// Sets the asset's source in/out marks.
    SetAssetInOut {
        id: AssetId,
        in_point: Option<Time>,
        out_point: Option<Time>,
    },
    /// Applies each command in turn as one step; if one fails, none do.
    Batch(Vec<Command>),
}

/// The id a command created, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Created {
    Nothing,
    Track(TrackId),
    Clip(ClipId),
    Link(LinkId),
    Marker(MarkerId),
    Asset(AssetId),
}

impl Command {
    /// The first locked track this edit would change. Track settings and order stay
    /// editable; clips on a locked track, and the track itself, do not.
    fn locked(&self, p: &Project) -> Option<TrackId> {
        let s = p.sequence();
        let track = |id: TrackId| s.track(id).filter(|t| t.locked).map(|t| t.id);
        let clip = |id: ClipId| s.clip(id).and_then(|(t, _)| track(t));
        let linked = |id: ClipId| s.linked(id).iter().find_map(|(t, _)| track(*t));
        // A speed key edit retimes the linked clips too.
        let keyed = |id: ClipId, param: &str| match param {
            keyframe::SPEED => linked(id),
            _ => clip(id),
        };
        match self {
            Self::RemoveTrack(id)
            | Self::AddClip { track: id, .. }
            | Self::CloseGap { track: id, .. } => track(*id),
            Self::MoveSelection {
                ids,
                anchor,
                track: to,
                ..
            } => ids
                .iter()
                .copied()
                .chain(std::iter::once(*anchor))
                .find_map(linked)
                .or_else(|| track(*to)),
            Self::MoveClip { id, track: to, .. } => clip(*id).or_else(|| track(*to)),
            Self::RemoveClip(id)
            | Self::TrimClip { id, .. }
            | Self::SetClipLink { id, .. }
            | Self::SetClipAsset { id, .. }
            | Self::SetClipParam { id, .. }
            | Self::FreezeFrame { id, .. } => clip(*id),
            Self::AddKeyframe { id, param, .. }
            | Self::RemoveKeyframe { id, param, .. }
            | Self::MoveKeyframe { id, param, .. } => keyed(*id, param),
            Self::Place(p) => {
                // Insert leaves locked tracks where they are rather than failing.
                [p.video, p.audio].into_iter().flatten().find_map(track)
            }
            Self::DragClip { id, track: to, .. } => s
                .linked(*id)
                .iter()
                .find_map(|(t, _)| track(*t))
                .or_else(|| track(*to)),
            Self::SetClipProps(edits) => edits.iter().find_map(|(id, _, keys)| match s.clip(*id) {
                Some((_, c)) if c.keyframes.get(keyframe::SPEED) != keys.get(keyframe::SPEED) => {
                    linked(*id)
                }
                _ => clip(*id),
            }),
            Self::SetClipSpeed { id, .. } | Self::SetSpeedKeys { id, .. } => linked(*id),
            Self::Trim { trims, .. } => trims
                .iter()
                .flat_map(|(id, ..)| s.linked(*id))
                .find_map(|(t, _)| track(t)),
            Self::CutSpans { spans, .. } => spans
                .iter()
                .flat_map(|(id, ..)| s.linked(*id))
                .find_map(|(t, _)| track(t)),
            Self::RemoveClips { ids, .. }
            | Self::Nest { ids, .. }
            | Self::LinkClips { ids, .. }
            | Self::SetClipsEnabled { ids, .. }
            | Self::Split { ids, .. } => ids.iter().find_map(|id| clip(*id)),
            Self::Paste { clips, .. } => clips.iter().find_map(|(t, _)| track(*t)),
            Self::RemoveAsset(a) => s
                .tracks()
                .iter()
                .find(|t| t.locked && t.clips().iter().any(|c| c.asset == *a))
                .map(|t| t.id),
            Self::AddTrack { .. }
            | Self::SetTrack(_)
            | Self::MoveTrack { .. }
            | Self::NewLink
            | Self::AddMarker { .. }
            | Self::RemoveMarker(_)
            | Self::SetMarker(_)
            | Self::SetInOut { .. }
            | Self::RemoveRange { .. }
            | Self::SetSettings(_)
            | Self::AddAsset(_)
            | Self::AddSolid { .. }
            | Self::SetSolidColor { .. }
            | Self::AddText { .. }
            | Self::SetText { .. }
            | Self::ImportSubtitles { .. }
            | Self::SetAssetProbe { .. }
            | Self::Import(_)
            | Self::RenameAsset { .. }
            | Self::RelinkAssets(_)
            | Self::SetAssetInOut { .. } => None,
            Self::Batch(cmds) => cmds.iter().find_map(|c| c.locked(p)),
        }
    }

    /// Applies to `p`; on error `p` is unchanged.
    fn apply(self, p: &mut Project) -> Result<Created, ModelError> {
        if let Some(t) = self.locked(p) {
            return Err(ModelError::Locked(t));
        }
        let nest = match &self {
            Self::Place(pl) => p.nest_error(pl.asset),
            Self::AddClip { clip, .. } => p.nest_error(clip.asset),
            Self::SetClipAsset { asset, .. } => p.nest_error(*asset),
            Self::Paste { clips, .. } => clips.iter().find_map(|(_, c)| p.nest_error(c.asset)),
            _ => None,
        };
        if let Some(e) = nest {
            return Err(e);
        }
        let s = &mut p.sequence;
        Ok(match self {
            Self::Batch(cmds) => {
                let before = p.clone();
                for c in cmds {
                    if let Err(e) = c.apply(p) {
                        *p = before;
                        return Err(e);
                    }
                }
                Created::Nothing
            }
            Self::AddTrack { kind, name } => Created::Track(s.add_track(kind, name)),
            Self::RemoveTrack(id) => {
                s.remove_track(id)?;
                Created::Nothing
            }
            Self::SetTrack(settings) => {
                s.set_track(settings)?;
                Created::Nothing
            }
            Self::MoveTrack { id, index } => {
                s.move_track(id, index)?;
                Created::Nothing
            }
            Self::AddClip { track, clip } => Created::Clip(s.add_clip(track, clip)?),
            Self::RemoveClip(id) => {
                s.remove_clip(id)?;
                Created::Nothing
            }
            Self::Place(placement) => {
                let (start, source_in, source_out) =
                    (placement.start, placement.source_in, placement.source_out);
                let markers = p
                    .assets
                    .iter()
                    .find(|a| a.id == placement.asset)
                    .and_then(|a| a.probe.as_ref())
                    .into_iter()
                    .flat_map(|probe| &probe.markers)
                    .filter(|m| source_in <= m.time && m.time < source_out);
                match s.place(placement)?.first() {
                    Some(c) => {
                        for m in markers {
                            s.add_marker(start + (m.time - source_in), Time(0), &m.name);
                        }
                        Created::Clip(*c)
                    }
                    None => Created::Nothing,
                }
            }
            Self::DragClip { id, track, start } => {
                s.drag_clip(id, track, start)?;
                Created::Nothing
            }
            Self::RemoveClips { ids, ripple } => {
                s.remove_clips(&ids, ripple)?;
                Created::Nothing
            }
            Self::Paste { clips, at } => match s.paste(&clips, at)?.first() {
                Some(c) => Created::Clip(*c),
                None => Created::Nothing,
            },
            Self::LinkClips { ids, linked } => {
                s.link_clips(&ids, linked)?;
                Created::Nothing
            }
            Self::SetClipsEnabled { ids, enabled } => {
                if let Some(id) = ids.iter().find(|id| s.clip(**id).is_none()) {
                    return Err(ModelError::UnknownClip(*id));
                }
                for id in ids {
                    s.with_clip_mut(id, |c| *c.enabled = enabled)?;
                }
                Created::Nothing
            }
            Self::MoveSelection {
                ids,
                anchor,
                track,
                start,
            } => {
                s.move_selection(&ids, anchor, track, start)?;
                Created::Nothing
            }
            Self::MoveClip { id, track, start } => {
                s.move_clip(id, track, start)?;
                Created::Nothing
            }
            Self::TrimClip {
                id,
                start,
                source_in,
                source_out,
            } => {
                s.set_clip_range(id, start, source_in, source_out)?;
                Created::Nothing
            }
            Self::Trim { trims, ripple } => {
                p.trim(&trims, ripple)?;
                Created::Nothing
            }
            Self::SetClipSpeed {
                id,
                speed,
                reverse,
                ripple,
            } => {
                s.set_speed(id, speed, reverse, ripple)?;
                Created::Nothing
            }
            Self::SetSpeedKeys {
                id,
                speed,
                keys,
                ripple,
            } => {
                s.set_speed_keys(id, speed, keys, ripple)?;
                Created::Nothing
            }
            Self::FreezeFrame { id, at } => {
                s.freeze_frame(id, at)?;
                Created::Nothing
            }
            Self::Nest { ids, name, outer } => Created::Clip(p.nest(&ids, name, outer)?),
            Self::Split { ids, at } => {
                s.split_at(&ids, at)?;
                Created::Nothing
            }
            Self::CutSpans { spans, speed } => {
                s.cut_spans(spans, speed)?;
                Created::Nothing
            }
            Self::CloseGap { track, at } => {
                s.close_gap(track, at)?;
                Created::Nothing
            }
            Self::NewLink => Created::Link(s.new_link()),
            Self::SetClipLink { id, link } => {
                s.with_clip_mut(id, |c| *c.link = link)?;
                Created::Nothing
            }
            Self::SetClipAsset { id, asset } => {
                s.with_clip_mut(id, |c| *c.asset = asset)?;
                Created::Nothing
            }
            Self::SetClipParam { id, key, value } => {
                s.with_clip_mut(id, |c| match value {
                    Some(v) => c.params.insert(key, v),
                    None => c.params.remove(&key),
                })?;
                Created::Nothing
            }
            Self::AddKeyframe { id, param, key } => {
                edit_keys(s, id, |k| {
                    keyframe::insert(k.entry(param).or_default(), key)
                })?;
                Created::Nothing
            }
            Self::RemoveKeyframe { id, param, time } => {
                edit_keys(s, id, |k| take_key(k, &param, time))?
                    .ok_or(ModelError::UnknownKeyframe(id, time))?;
                Created::Nothing
            }
            Self::MoveKeyframe {
                id,
                param,
                from,
                to,
            } => {
                edit_keys(s, id, |keys| {
                    let mut k = take_key(keys, &param, from)?;
                    k.time = to;
                    keyframe::insert(keys.entry(param).or_default(), k);
                    Some(())
                })?
                .ok_or(ModelError::UnknownKeyframe(id, from))?;
                Created::Nothing
            }
            Self::SetClipProps(edits) => {
                if let Some(id) = edits.iter().find(|(id, ..)| s.clip(*id).is_none()) {
                    return Err(ModelError::UnknownClip(id.0));
                }
                // Speed keys can fail to fit; leave the sequence as it was if they do.
                let mut next = s.clone();
                for (id, transform, keys) in edits {
                    next.with_clip_mut(id, |c| *c.transform = transform)?;
                    edit_keys(&mut next, id, |k| *k = keys)?;
                }
                *s = next;
                Created::Nothing
            }
            Self::AddMarker {
                time,
                duration,
                name,
            } => Created::Marker(s.add_marker(time, duration, name)),
            Self::RemoveMarker(id) => {
                s.remove_marker(id).ok_or(ModelError::UnknownMarker(id))?;
                Created::Nothing
            }
            Self::SetMarker(m) => {
                let id = m.id;
                s.set_marker(m).ok_or(ModelError::UnknownMarker(id))?;
                Created::Nothing
            }
            Self::SetInOut {
                in_point,
                out_point,
            } => {
                s.in_point = in_point;
                s.out_point = out_point;
                Created::Nothing
            }
            Self::RemoveRange { ripple } => {
                s.remove_range(ripple)?;
                Created::Nothing
            }
            Self::SetSettings(settings) => {
                p.settings = settings;
                Created::Nothing
            }
            Self::AddAsset(path) => Created::Asset(p.add_asset(path)),
            Self::AddSolid { name, color } => {
                let id = p.add_asset(PathBuf::new());
                let a = p.asset_mut(id).expect("just added");
                (a.name, a.solid) = (Some(name), Some(color));
                Created::Asset(id)
            }
            Self::AddText { name, text } => {
                let id = p.add_asset(PathBuf::new());
                let a = p.asset_mut(id).expect("just added");
                (a.name, a.text) = (Some(name), Some(text));
                Created::Asset(id)
            }
            Self::ImportSubtitles { name, cues, style } => {
                let track = s.add_track(TrackKind::Video, name);
                s.track_mut(track).expect("just added").subtitles = true;
                let mut end = Time(0);
                for cue in cues {
                    let start = cue.start.max(end);
                    if cue.end <= start {
                        continue;
                    }
                    end = cue.end;
                    let text = Text {
                        content: cue.text,
                        ..style.clone()
                    };
                    let asset = p.add_asset(PathBuf::new());
                    let a = p.asset_mut(asset).expect("just added");
                    (a.name, a.text) = (Some(text.content.replace('\n', " ")), Some(text));
                    let clip = NewClip {
                        asset,
                        source_in: Time(0),
                        source_out: end - start,
                        start,
                        link: None,
                    };
                    p.sequence.add_clip(track, clip)?;
                }
                Created::Track(track)
            }
            Self::SetText { id, text } => {
                let a = p.asset_mut(id).filter(|a| a.text.is_some());
                a.ok_or(ModelError::UnknownAsset(id))?.text = Some(text);
                Created::Nothing
            }
            Self::SetSolidColor { id, color } => {
                let a = p.asset_mut(id).filter(|a| a.solid.is_some());
                a.ok_or(ModelError::UnknownAsset(id))?.solid = Some(color);
                Created::Nothing
            }
            Self::SetAssetProbe { id, probe } => {
                p.asset_mut(id).ok_or(ModelError::UnknownAsset(id))?.probe = probe;
                Created::Nothing
            }
            Self::Import(items) => {
                for (path, probe) in items {
                    if p.assets.iter().all(|a| a.path != path) {
                        let id = p.add_asset(path);
                        p.asset_mut(id).expect("just added").probe = Some(probe);
                    }
                }
                Created::Nothing
            }
            Self::RenameAsset { id, name } => {
                let name = name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
                p.asset_mut(id).ok_or(ModelError::UnknownAsset(id))?.name = name;
                Created::Nothing
            }
            Self::SetAssetInOut {
                id,
                in_point,
                out_point,
            } => {
                let a = p.asset_mut(id).ok_or(ModelError::UnknownAsset(id))?;
                (a.in_point, a.out_point) = (in_point, out_point);
                Created::Nothing
            }
            Self::RemoveAsset(id) => {
                p.remove_asset(id)?;
                Created::Nothing
            }
            Self::RelinkAssets(items) => {
                if let Some((id, _)) = items.iter().find(|(id, _)| p.asset(*id).is_none()) {
                    return Err(ModelError::UnknownAsset(*id));
                }
                for (id, path) in items {
                    p.asset_mut(id).expect("checked above").path = path;
                }
                Created::Nothing
            }
        })
    }
}

/// Applies `edit` to clip `id`'s keyframes. One that changes the speed keys retimes the clip
/// and its linked clips, rippling the clips after them.
fn edit_keys<R>(
    s: &mut Sequence,
    id: ClipId,
    edit: impl FnOnce(&mut BTreeMap<String, Vec<Keyframe>>) -> R,
) -> Result<R, ModelError> {
    let (_, c) = s.clip(id).ok_or(ModelError::UnknownClip(id))?;
    let mut keys = c.keyframes.clone();
    let r = edit(&mut keys);
    let speed = keys.get(keyframe::SPEED);
    if speed != c.keyframes.get(keyframe::SPEED) {
        let keys = speed.cloned().unwrap_or_default();
        s.set_speed_keys(id, c.speed, keys, true)?;
    }
    s.with_clip_mut(id, |c| *c.keyframes = keys)?;
    Ok(r)
}

/// Removes a keyframe, dropping the param's list once empty so add then
/// remove leaves the clip as it was.
fn take_key(
    keys: &mut BTreeMap<String, Vec<Keyframe>>,
    param: &str,
    time: Time,
) -> Option<Keyframe> {
    let list = keys.get_mut(param)?;
    let k = keyframe::remove(list, time)?;
    if list.is_empty() {
        keys.remove(param);
    }
    Some(k)
}

/// Coalescing key for a continuous gesture, e.g. one per drag.
pub type GestureKey = u64;

/// Snapshot of the project plus the revision it was at.
type Snapshot = (Project, u64);

/// Owns the project and its undo/redo stacks.
#[derive(Debug, Clone)]
pub struct History {
    project: Project,
    undo: VecDeque<Snapshot>,
    redo: Vec<Snapshot>,
    limit: usize,
    /// Identifies the current state, for the dirty flag.
    rev: u64,
    next_rev: u64,
    saved_rev: u64,
    /// State before the open transaction.
    txn: Option<Snapshot>,
    /// The gesture the top undo step belongs to, while it may still grow.
    gesture: Option<GestureKey>,
}

impl History {
    pub const DEFAULT_LIMIT: usize = 200;

    pub fn new(project: Project) -> Self {
        Self::with_limit(project, Self::DEFAULT_LIMIT)
    }

    /// `limit` is the number of undo steps kept; older ones are dropped.
    pub fn with_limit(project: Project, limit: usize) -> Self {
        Self {
            project,
            undo: VecDeque::new(),
            redo: Vec::new(),
            limit,
            rev: 0,
            next_rev: 0,
            saved_rev: 0,
            txn: None,
            gesture: None,
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Opens sequence `id` in the Timeline. Navigation, so neither an undo step nor unsaved
    /// work; undo goes back to the sequence open at that step.
    pub fn open_sequence(&mut self, id: SequenceId) -> Result<(), ModelError> {
        if self.txn.is_some() || self.project.sequence().id() == id {
            return Ok(());
        }
        self.project.open(id)?;
        self.gesture = None;
        let saved = !self.is_dirty();
        self.bump();
        if saved {
            self.mark_saved();
        }
        Ok(())
    }

    /// Applies `cmd` as its own undo step, or as part of the open transaction.
    pub fn apply(&mut self, cmd: Command) -> Result<Created, ModelError> {
        self.gesture = None;
        self.apply_step(cmd)
    }

    /// Like [`apply`](Self::apply), but consecutive calls with the same `key`
    /// merge into one undo step (a drag). Any other edit, undo, redo or
    /// [`end_gesture`](Self::end_gesture) ends the gesture.
    pub fn apply_coalesced(
        &mut self,
        key: GestureKey,
        cmd: Command,
    ) -> Result<Created, ModelError> {
        if self.txn.is_some() || self.gesture != Some(key) {
            self.gesture = None;
            let rev = self.rev;
            let r = self.apply_step(cmd);
            // Only a gesture that opened an undo step may coalesce into it.
            if r.is_ok() && self.txn.is_none() && self.rev != rev {
                self.gesture = Some(key);
            }
            return r;
        }
        let r = cmd.apply(&mut self.project)?;
        self.bump();
        Ok(r)
    }

    pub fn end_gesture(&mut self) {
        self.gesture = None;
    }

    fn apply_step(&mut self, cmd: Command) -> Result<Created, ModelError> {
        let before = (self.project.clone(), self.rev);
        let r = cmd.apply(&mut self.project)?;
        // A no-op (a split on a cut, a trim held at its limit) leaves no undo step.
        if self.project == before.0 {
            return Ok(r);
        }
        if self.txn.is_none() {
            self.push_undo(before);
        }
        self.bump();
        Ok(r)
    }

    /// Starts grouping commands into one undo step. Returns false if one is
    /// already open (no nesting).
    pub fn begin(&mut self) -> bool {
        if self.txn.is_some() {
            return false;
        }
        self.gesture = None;
        self.txn = Some((self.project.clone(), self.rev));
        true
    }

    /// Closes the transaction as one undo step; an empty one adds nothing.
    pub fn commit(&mut self) {
        if let Some(before) = self.txn.take()
            && before.1 != self.rev
        {
            self.push_undo(before);
        }
    }

    /// Discards everything since [`begin`](Self::begin).
    pub fn rollback(&mut self) {
        if let Some((p, rev)) = self.txn.take() {
            self.project = p;
            self.rev = rev;
        }
    }

    pub fn can_undo(&self) -> bool {
        self.txn.is_none() && !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        self.txn.is_none() && !self.redo.is_empty()
    }

    /// Returns false when there is nothing to undo or a transaction is open.
    pub fn undo(&mut self) -> bool {
        if !self.can_undo() {
            return false;
        }
        self.gesture = None;
        let s = self.undo.pop_back().expect("checked");
        let cur = self.swap(s);
        self.redo.push(cur);
        true
    }

    pub fn redo(&mut self) -> bool {
        if !self.can_redo() {
            return false;
        }
        self.gesture = None;
        let s = self.redo.pop().expect("checked");
        let cur = self.swap(s);
        self.undo.push_back(cur);
        true
    }

    /// True when the project differs from the last [`mark_saved`](Self::mark_saved).
    pub fn is_dirty(&self) -> bool {
        self.rev != self.saved_rev
    }

    /// Changes on every edit, undo and redo; equal revisions mean equal projects.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Makes the project dirty without an edit, e.g. after restoring an autosave.
    pub fn mark_unsaved(&mut self) {
        self.saved_rev = u64::MAX;
    }

    pub fn mark_saved(&mut self) {
        self.end_gesture();
        self.saved_rev = self.rev;
    }

    fn swap(&mut self, (p, rev): Snapshot) -> Snapshot {
        let old = std::mem::replace(&mut self.project, p);
        (old, std::mem::replace(&mut self.rev, rev))
    }

    fn push_undo(&mut self, before: Snapshot) {
        self.redo.clear();
        self.undo.push_back(before);
        while self.undo.len() > self.limit {
            self.undo.pop_front();
        }
    }

    fn bump(&mut self) {
        self.next_rev += 1;
        self.rev = self.next_rev;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Track;
    use proptest::prelude::*;

    fn track(h: &mut History) -> TrackId {
        match h
            .apply(Command::AddTrack {
                kind: TrackKind::Video,
                name: "V1".into(),
            })
            .unwrap()
        {
            Created::Track(t) => t,
            c => panic!("{c:?}"),
        }
    }

    fn add_clip(track: TrackId, start: i64) -> Command {
        Command::AddClip {
            track,
            clip: NewClip {
                asset: AssetId(1),
                source_in: Time(0),
                source_out: Time(10),
                start: Time(start),
                link: None,
            },
        }
    }

    #[test]
    fn a_batch_is_one_undo_step_and_all_or_nothing() {
        let mut h = History::new(Project::default());
        let t = track(&mut h);
        let before = h.project().clone();
        h.apply(Command::Batch(vec![add_clip(t, 0), add_clip(t, 20)]))
            .unwrap();
        assert_eq!(h.project().sequence().tracks()[0].clips().len(), 2);
        assert!(h.undo());
        assert_eq!(h.project(), &before, "one step");
        let bad = Command::Batch(vec![add_clip(t, 0), Command::RemoveClip(ClipId(999))]);
        assert!(h.apply(bad).is_err());
        // The bad batch's AddClip applies before RemoveClip fails, then rolls back with it.
        assert_eq!(h.project(), &before, "the AddClip stayed");
    }

    #[test]
    fn disabled_clips_leave_evaluation_until_undone() {
        use crate::eval::{audio_in, video_at};
        let mut h = History::new(Project::default());
        let v = track(&mut h);
        let a = match h.apply(Command::AddTrack {
            kind: TrackKind::Audio,
            name: "A1".into(),
        }) {
            Ok(Created::Track(t)) => t,
            r => panic!("{r:?}"),
        };
        let Created::Clip(cv) = h.apply(add_clip(v, 0)).unwrap() else {
            panic!()
        };
        let Created::Clip(ca) = h.apply(add_clip(a, 0)).unwrap() else {
            panic!()
        };
        let before = h.project().clone();
        let seen = |h: &History| {
            let s = h.project().sequence();
            (
                video_at(s, Time(5)).len(),
                audio_in(s, Time(0), Time(10)).len(),
            )
        };
        h.apply(Command::SetClipsEnabled {
            ids: vec![cv, ca],
            enabled: false,
        })
        .unwrap();
        assert_eq!(seen(&h), (0, 0));
        // Disabled survives a save; enabled is the default and not written.
        let off = h.project().sequence().clip(cv).unwrap().1.clone();
        let json = serde_json::to_string(&off).unwrap();
        assert_eq!(serde_json::from_str::<Clip>(&json).unwrap(), off);
        let on = before.sequence().clip(cv).unwrap().1;
        assert!(!serde_json::to_string(on).unwrap().contains("enabled"));
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        assert_eq!(seen(&h), (1, 1));
        // An unknown id changes nothing.
        let bad = Command::SetClipsEnabled {
            ids: vec![cv, ClipId(999)],
            enabled: false,
        };
        assert!(h.apply(bad).is_err());
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn transaction_and_gesture_are_one_step_each() {
        let mut h = History::new(Project::default());
        let t = track(&mut h);
        let after_track = h.project().clone();
        assert!(h.begin());
        h.apply(add_clip(t, 0)).unwrap();
        let Created::Clip(c) = h.apply(add_clip(t, 20)).unwrap() else {
            panic!()
        };
        h.commit();
        let after_txn = h.project().clone();
        for x in 1..=5 {
            h.apply_coalesced(
                7,
                Command::MoveClip {
                    id: c,
                    track: t,
                    start: Time(20 + x),
                },
            )
            .unwrap();
        }
        assert!(h.undo());
        assert_eq!(h.project(), &after_txn);
        assert!(h.undo());
        assert_eq!(h.project(), &after_track);
        assert!(h.undo());
        assert_eq!(h.project(), &Project::default());
        assert!(!h.undo());
    }

    #[test]
    fn import_skips_duplicates_and_undoes_in_one_step() {
        let mut h = History::new(Project::default());
        h.apply(Command::AddAsset("a.mkv".into())).unwrap();
        let before = h.project().clone();
        let item = |p: &str| (PathBuf::from(p), ProbeInfo::default());
        h.apply(Command::Import(vec![
            item("a.mkv"),
            item("b.mkv"),
            item("b.mkv"),
        ]))
        .unwrap();
        let paths: Vec<_> = h
            .project()
            .assets()
            .iter()
            .map(|a| a.path.clone())
            .collect();
        assert_eq!(paths, [PathBuf::from("a.mkv"), PathBuf::from("b.mkv")]);
        assert!(h.project().assets()[1].probe.is_some());
        assert!(h.undo());
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn rename_and_remove_asset_undo() {
        let mut h = History::new(Project::default());
        h.apply(Command::AddAsset("dir/a.mkv".into())).unwrap();
        h.apply(Command::AddAsset("b.mkv".into())).unwrap();
        let a = AssetId(1);
        let rename = |name: &str| Command::RenameAsset {
            id: a,
            name: Some(name.into()),
        };
        h.apply(rename("  Intro ")).unwrap();
        assert_eq!(h.project().asset(a).unwrap().display_name(), "Intro");
        h.apply(rename(" ")).unwrap();
        assert_eq!(h.project().asset(a).unwrap().display_name(), "a.mkv");

        let t = track(&mut h);
        h.apply(add_clip(t, 0)).unwrap();
        h.apply(add_clip(t, 20)).unwrap();
        assert_eq!(h.project().clips_using(a), 2);
        assert_eq!(h.project().clips_using(AssetId(2)), 0);
        let before = h.project().clone();
        h.apply(Command::RemoveAsset(a)).unwrap();
        assert!(h.project().asset(a).is_none());
        assert_eq!(h.project().clips_using(a), 0);
        assert!(h.project().sequence().track(t).unwrap().clips().is_empty());
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        assert_eq!(
            h.apply(Command::RemoveAsset(AssetId(9))),
            Err(ModelError::UnknownAsset(AssetId(9)))
        );
    }

    #[test]
    fn relink_is_one_undo_step_and_unknown_ids_change_nothing() {
        let mut h = History::new(Project::default());
        h.apply(Command::AddAsset("old/a.mkv".into())).unwrap();
        h.apply(Command::AddAsset("old/b.mkv".into())).unwrap();
        let before = h.project().clone();
        let bad = Command::RelinkAssets(vec![(AssetId(1), "x".into()), (AssetId(9), "y".into())]);
        assert_eq!(h.apply(bad), Err(ModelError::UnknownAsset(AssetId(9))));
        assert_eq!(h.project(), &before);
        h.apply(Command::RelinkAssets(vec![
            (AssetId(1), "new/a.mkv".into()),
            (AssetId(2), "new/b.mkv".into()),
        ]))
        .unwrap();
        assert_eq!(h.project().assets()[1].path, PathBuf::from("new/b.mkv"));
        h.undo();
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn asset_in_out_undoes_and_survives_save_load() {
        let mut h = History::new(Project::default());
        h.apply(Command::AddAsset("/m/a.mkv".into())).unwrap();
        let a = AssetId(1);
        h.apply(Command::SetAssetInOut {
            id: a,
            in_point: Some(Time(10)),
            out_point: Some(Time(90)),
        })
        .unwrap();
        let dir = std::path::Path::new("/m");
        let json = crate::file::to_json(h.project(), dir).unwrap();
        let loaded = crate::file::from_json(&json, dir, |_| true).unwrap();
        let asset = loaded.asset(a).unwrap();
        assert_eq!(
            (asset.in_point, asset.out_point),
            (Some(Time(10)), Some(Time(90)))
        );
        assert!(h.undo());
        assert_eq!(h.project().asset(a).unwrap().in_point, None);
    }

    #[test]
    fn failed_command_and_rollback_change_nothing() {
        let mut h = History::new(Project::default());
        assert_eq!(
            h.apply(Command::RemoveClip(ClipId(9))),
            Err(ModelError::UnknownClip(ClipId(9)))
        );
        assert!(!h.can_undo() && !h.is_dirty());
        h.begin();
        track(&mut h);
        assert!(!h.undo()); // refused while a transaction is open
        h.rollback();
        assert_eq!(h.project(), &Project::default());
        assert!(!h.can_undo() && !h.is_dirty());
    }

    #[test]
    fn dirty_flag_follows_saved_state() {
        let mut h = History::new(Project::default());
        track(&mut h);
        assert!(h.is_dirty());
        h.mark_saved();
        assert!(!h.is_dirty());
        h.undo();
        assert!(h.is_dirty());
        assert!(h.redo());
        assert!(!h.is_dirty());
        h.undo();
        track(&mut h); // a different edit from the saved state stays dirty
        assert!(h.is_dirty());
    }

    #[test]
    fn set_clip_props_is_one_step_and_atomic() {
        let mut h = History::new(Project::default());
        let t = track(&mut h);
        let ids: Vec<ClipId> = [0, 200]
            .map(|at| match h.apply(add_clip(t, at)).unwrap() {
                Created::Clip(c) => c,
                _ => panic!(),
            })
            .into();
        let before = h.project().clone();
        let x = Transform {
            opacity: 0.5,
            ..Transform::default()
        };
        let edits = ids
            .iter()
            .map(|&c| (c, x.clone(), BTreeMap::new()))
            .collect();
        h.apply(Command::SetClipProps(edits)).unwrap();
        for c in &ids {
            assert_eq!(h.project().sequence().clip(*c).unwrap().1.transform, x);
        }
        h.undo();
        assert_eq!(h.project(), &before);
        // an unknown clip leaves the known one untouched
        let bad = vec![
            (ids[0], x.clone(), BTreeMap::new()),
            (ClipId(999), x.clone(), BTreeMap::new()),
        ];
        assert!(h.apply(Command::SetClipProps(bad)).is_err());
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn limit_drops_oldest_steps() {
        let mut h = History::with_limit(Project::default(), 2);
        for _ in 0..4 {
            track(&mut h);
        }
        assert!(h.undo() && h.undo() && !h.undo());
        assert_eq!(h.project().sequence().tracks().len(), 2);
    }

    #[test]
    fn keyframes_stay_in_place_when_trimmed_and_move_with_clip() {
        use crate::keyframe::{Easing, Value};
        let mut h = History::new(Project::default());
        let t = track(&mut h);
        let Created::Clip(c) = h.apply(add_clip(t, 100)).unwrap() else {
            panic!()
        };
        let key = |time, v| Command::AddKeyframe {
            id: c,
            param: "opacity".into(),
            key: Keyframe {
                time: Time(time),
                value: Value::Scalar(v),
                easing: Easing::Linear,
            },
        };
        h.apply(key(2, 0.0)).unwrap();
        h.apply(key(8, 1.0)).unwrap();
        let clip = |h: &History| h.project().sequence().clip(c).unwrap().1.clone();
        assert_eq!(
            clip(&h).value_at("opacity", Time(105)),
            Some(Value::Scalar(0.5))
        );
        // trim 3 off the head: the same media frame keeps the same value
        h.apply(Command::TrimClip {
            id: c,
            start: Time(103),
            source_in: Time(3),
            source_out: Time(10),
        })
        .unwrap();
        assert_eq!(clip(&h).keyframes["opacity"][0].time, Time(2));
        assert_eq!(
            clip(&h).value_at("opacity", Time(105)),
            Some(Value::Scalar(0.5))
        );
        h.apply(Command::MoveClip {
            id: c,
            track: t,
            start: Time(203),
        })
        .unwrap();
        assert_eq!(
            clip(&h).value_at("opacity", Time(205)),
            Some(Value::Scalar(0.5))
        );
        h.apply(Command::MoveKeyframe {
            id: c,
            param: "opacity".into(),
            from: Time(8),
            to: Time(4),
        })
        .unwrap();
        assert_eq!(
            clip(&h).value_at("opacity", Time(203)),
            Some(Value::Scalar(0.5))
        );
        assert_eq!(
            h.apply(Command::RemoveKeyframe {
                id: c,
                param: "opacity".into(),
                time: Time(8),
            }),
            Err(ModelError::UnknownKeyframe(c, Time(8)))
        );
        for time in [2, 4] {
            h.apply(Command::RemoveKeyframe {
                id: c,
                param: "opacity".into(),
                time: Time(time),
            })
            .unwrap();
        }
        assert!(clip(&h).keyframes.is_empty());
        while h.undo() {}
        assert_eq!(h.project(), &Project::default());
    }

    fn set(h: &mut History, t: TrackId, f: impl FnOnce(&mut TrackSettings)) {
        let mut s = h.project().sequence().track(t).unwrap().settings();
        f(&mut s);
        h.apply(Command::SetTrack(s)).unwrap();
    }

    #[test]
    fn locked_track_rejects_every_clip_edit() {
        use crate::keyframe::{Easing, Value};
        let mut h = History::new(Project::default());
        h.apply(Command::AddAsset("a.mkv".into())).unwrap();
        let t = track(&mut h);
        let free = track(&mut h);
        let Created::Clip(c) = h.apply(add_clip(t, 0)).unwrap() else {
            panic!()
        };
        let key = Keyframe {
            time: Time(2),
            value: Value::Scalar(1.0),
            easing: Easing::Linear,
        };
        h.apply(Command::AddKeyframe {
            id: c,
            param: "a".into(),
            key,
        })
        .unwrap();
        set(&mut h, t, |s| s.locked = true);
        let Created::Clip(other) = h.apply(add_clip(free, 0)).unwrap() else {
            panic!()
        };
        let edits = [
            Command::RemoveTrack(t),
            add_clip(t, 50),
            Command::RemoveClip(c),
            Command::MoveClip {
                id: c,
                track: free,
                start: Time(40),
            },
            // onto the locked track from a free one
            Command::MoveClip {
                id: other,
                track: t,
                start: Time(40),
            },
            Command::TrimClip {
                id: c,
                start: Time(0),
                source_in: Time(0),
                source_out: Time(5),
            },
            Command::SetClipLink {
                id: c,
                link: Some(LinkId(99)),
            },
            Command::SetClipAsset {
                id: c,
                asset: AssetId(2),
            },
            Command::SetClipParam {
                id: c,
                key: "a".into(),
                value: Some(0.5),
            },
            Command::AddKeyframe {
                id: c,
                param: "b".into(),
                key,
            },
            Command::RemoveKeyframe {
                id: c,
                param: "a".into(),
                time: Time(2),
            },
            Command::MoveKeyframe {
                id: c,
                param: "a".into(),
                from: Time(2),
                to: Time(3),
            },
            Command::SetClipProps(vec![(c, Transform::default(), BTreeMap::new())]),
            Command::RemoveAsset(AssetId(1)),
        ];
        let before = h.project().clone();
        for cmd in edits {
            assert_eq!(h.apply(cmd.clone()), Err(ModelError::Locked(t)), "{cmd:?}");
            assert_eq!(h.project(), &before, "{cmd:?}");
        }
        // Header changes still work, so the track can be unlocked and edited again.
        h.apply(Command::MoveTrack { id: t, index: 0 }).unwrap();
        set(&mut h, t, |s| s.locked = false);
        h.apply(Command::RemoveClip(c)).unwrap();
    }

    #[test]
    fn every_track_change_undoes_and_redoes() {
        let mut h = History::new(Project::default());
        let v1 = track(&mut h);
        let changes: [&dyn Fn(&mut History); 9] = [
            &|h| {
                h.apply(Command::AddTrack {
                    kind: TrackKind::Audio,
                    name: "A1".into(),
                })
                .unwrap();
            },
            &|h| set(h, v1, |s| s.name = "Main".into()),
            &|h| set(h, v1, |s| s.hidden = true),
            &|h| set(h, v1, |s| s.locked = true),
            &|h| set(h, v1, |s| s.muted = true),
            &|h| set(h, v1, |s| s.solo = true),
            &|h| set(h, v1, |s| s.height = 80.0),
            &|h| set(h, v1, |s| s.target = true),
            &|h| {
                track(h);
                h.apply(Command::MoveTrack { id: v1, index: 0 }).unwrap();
            },
        ];
        for change in changes {
            let before = h.project().clone();
            change(&mut h);
            let after = h.project().clone();
            assert_ne!(before, after);
            while h.project() != &before {
                assert!(h.undo());
            }
            while h.redo() {}
            assert_eq!(h.project(), &after);
        }
        set(&mut h, v1, |s| s.locked = false);
        let before = h.project().clone();
        h.apply(Command::RemoveTrack(v1)).unwrap();
        assert!(h.undo());
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn track_order_target_and_height_rules() {
        let mut h = History::new(Project::default());
        let a1 = match h.apply(Command::AddTrack {
            kind: TrackKind::Audio,
            name: "A1".into(),
        }) {
            Ok(Created::Track(t)) => t,
            r => panic!("{r:?}"),
        };
        let v1 = track(&mut h);
        let v2 = track(&mut h);
        let order = |h: &History| -> Vec<_> {
            h.project()
                .sequence()
                .tracks()
                .iter()
                .map(|t| t.id)
                .collect()
        };
        // New video goes on top, audio stays below.
        assert_eq!(order(&h), [v2, v1, a1]);
        // Reordering stays within the kind's group, clamped at its ends.
        h.apply(Command::MoveTrack { id: v2, index: 9 }).unwrap();
        assert_eq!(order(&h), [v1, v2, a1]);
        h.apply(Command::MoveTrack { id: a1, index: 0 }).unwrap();
        assert_eq!(order(&h), [v1, v2, a1]);
        // One target per kind.
        set(&mut h, v1, |s| s.target = true);
        set(&mut h, a1, |s| s.target = true);
        set(&mut h, v2, |s| s.target = true);
        let targets: Vec<_> = h
            .project()
            .sequence()
            .tracks()
            .iter()
            .filter(|t| t.target)
            .map(|t| t.id)
            .collect();
        assert_eq!(targets, [v2, a1]);
        set(&mut h, v1, |s| s.height = 500.0);
        let t = |h: &History, id| h.project().sequence().track(id).unwrap().clone();
        assert_eq!(t(&h, v1).height, Track::MAX_HEIGHT);

        // Every track setting survives save/load, and older files get the defaults.
        set(&mut h, v1, |s| {
            (s.hidden, s.locked, s.muted, s.solo) = (true, true, true, true);
            s.height = 64.0;
        });
        let dir = std::path::Path::new("/p");
        let json = crate::file::to_json(h.project(), dir).unwrap();
        let loaded = crate::file::from_json(&json, dir, |_| true).unwrap();
        assert_eq!(&loaded, h.project());
        let old = json
            .replace("\"height\": 64.0,", "")
            .replace("\"target\": true,", "");
        assert_ne!(old, json);
        let loaded = crate::file::from_json(&old, dir, |_| true).unwrap();
        let v = loaded.sequence().track(v1).unwrap();
        assert_eq!((v.height, v.locked), (40.0, true));
    }

    #[derive(Debug, Clone)]
    enum Op {
        Cmd(Command),
        Drag(GestureKey, Command),
        EndGesture,
        Begin,
        Commit,
        Rollback,
        Undo,
        Redo,
    }

    /// Ids are small so random commands often hit something that exists.
    fn command() -> impl Strategy<Value = Command> {
        let id = 1u64..12;
        let t = || (-50i64..200).prop_map(Time);
        prop_oneof![
            (any::<bool>(), "[a-z]{0,3}").prop_map(|(v, name)| Command::AddTrack {
                kind: if v {
                    TrackKind::Video
                } else {
                    TrackKind::Audio
                },
                name,
            }),
            id.clone().prop_map(|i| Command::RemoveTrack(TrackId(i))),
            (id.clone(), any::<[bool; 5]>(), 0f32..2.0).prop_map(|(i, f, v)| {
                Command::SetTrack(TrackSettings {
                    id: TrackId(i),
                    name: "t".into(),
                    hidden: f[0],
                    locked: f[1],
                    muted: f[2],
                    solo: f[3],
                    volume: v,
                    pan: -v / 2.0,
                    height: v * 80.0,
                    target: f[4],
                    fx: Default::default(),
                })
            }),
            (id.clone(), 0usize..4).prop_map(|(i, index)| Command::MoveTrack {
                id: TrackId(i),
                index,
            }),
            (
                id.clone(),
                t(),
                t(),
                1i64..60,
                proptest::option::of(id.clone())
            )
                .prop_map(|(tr, start, si, len, link)| Command::AddClip {
                    track: TrackId(tr),
                    clip: NewClip {
                        asset: AssetId(1),
                        source_in: si,
                        source_out: si + Time(len),
                        start,
                        link: link.map(LinkId),
                    },
                }),
            id.clone().prop_map(|i| Command::RemoveClip(ClipId(i))),
            (id.clone(), id.clone(), t()).prop_map(|(c, tr, start)| Command::MoveClip {
                id: ClipId(c),
                track: TrackId(tr),
                start,
            }),
            (id.clone(), t(), t(), -5i64..60).prop_map(|(c, start, si, len)| Command::TrimClip {
                id: ClipId(c),
                start,
                source_in: si,
                source_out: si + Time(len),
            }),
            Just(Command::NewLink),
            (id.clone(), proptest::option::of(id.clone())).prop_map(|(c, l)| {
                Command::SetClipLink {
                    id: ClipId(c),
                    link: l.map(LinkId),
                }
            }),
            (id.clone(), id.clone()).prop_map(|(c, a)| Command::SetClipAsset {
                id: ClipId(c),
                asset: AssetId(a),
            }),
            (id.clone(), "[ab]", proptest::option::of(0f64..1.0)).prop_map(|(c, key, value)| {
                Command::SetClipParam {
                    id: ClipId(c),
                    key,
                    value,
                }
            }),
            (id.clone(), "[ab]", t(), 0f64..1.0).prop_map(|(c, param, time, v)| {
                Command::AddKeyframe {
                    id: ClipId(c),
                    param,
                    key: Keyframe {
                        time,
                        value: crate::keyframe::Value::Scalar(v),
                        easing: crate::keyframe::Easing::Bezier([v, 0.0, 1.0, v]),
                    },
                }
            }),
            (id.clone(), "[ab]", t()).prop_map(|(c, param, time)| Command::RemoveKeyframe {
                id: ClipId(c),
                param,
                time,
            }),
            (id.clone(), "[ab]", t(), t()).prop_map(|(c, param, from, to)| {
                Command::MoveKeyframe {
                    id: ClipId(c),
                    param,
                    from,
                    to,
                }
            }),
            (t(), 0i64..20).prop_map(|(time, d)| Command::AddMarker {
                time,
                duration: Time(d),
                name: "m".into(),
            }),
            id.clone().prop_map(|i| Command::RemoveMarker(MarkerId(i))),
            (proptest::option::of(t()), proptest::option::of(t())).prop_map(|(i, o)| {
                Command::SetInOut {
                    in_point: i,
                    out_point: o,
                }
            }),
            (1u32..4000, 1u32..4000).prop_map(|(width, height)| Command::SetSettings(
                ProjectSettings {
                    width,
                    height,
                    ..Default::default()
                }
            )),
            "[a-z]{1,4}".prop_map(|s| Command::AddAsset(s.into())),
            (id.clone(), proptest::option::of("[a-z ]{0,4}")).prop_map(|(a, name)| {
                Command::RenameAsset {
                    id: AssetId(a),
                    name,
                }
            }),
            id.clone().prop_map(|a| Command::RemoveAsset(AssetId(a))),
            (id.clone(), "[a-z]{1,4}")
                .prop_map(|(a, s)| Command::RelinkAssets(vec![(AssetId(a), s.into())])),
            (
                id.clone(),
                proptest::option::of(t()),
                proptest::option::of(t())
            )
                .prop_map(|(a, in_point, out_point)| Command::SetAssetInOut {
                    id: AssetId(a),
                    in_point,
                    out_point,
                }),
            (id, any::<bool>()).prop_map(|(a, some)| Command::SetAssetProbe {
                id: AssetId(a),
                probe: some.then(ProbeInfo::default),
            }),
        ]
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            8 => command().prop_map(Op::Cmd),
            3 => (0u64..2, command()).prop_map(|(k, c)| Op::Drag(k, c)),
            1 => Just(Op::EndGesture),
            1 => Just(Op::Begin),
            1 => Just(Op::Commit),
            1 => Just(Op::Rollback),
            1 => Just(Op::Undo),
            1 => Just(Op::Redo),
        ]
    }

    proptest! {
        #[test]
        fn undo_all_restores_start_and_redo_all_restores_end(
            ops in proptest::collection::vec(op(), 0..80)
        ) {
            let mut h = History::with_limit(Project::default(), usize::MAX);
            for op in ops {
                match op {
                    Op::Cmd(c) => { let _ = h.apply(c); }
                    Op::Drag(k, c) => { let _ = h.apply_coalesced(k, c); }
                    Op::EndGesture => h.end_gesture(),
                    Op::Begin => { h.begin(); }
                    Op::Commit => h.commit(),
                    Op::Rollback => h.rollback(),
                    Op::Undo => { h.undo(); }
                    Op::Redo => { h.redo(); }
                }
            }
            h.commit();
            while h.redo() {} // the end state is the newest one reachable
            let end = h.project().clone();
            while h.undo() {}
            prop_assert_eq!(h.project(), &Project::default());
            prop_assert!(!h.is_dirty());
            while h.redo() {}
            prop_assert_eq!(h.project(), &end);
        }

        #[test]
        fn save_load_is_lossless(ops in proptest::collection::vec(command(), 0..60)) {
            let mut h = History::new(Project::default());
            for c in ops {
                let _ = h.apply(c);
            }
            let dir = std::path::Path::new("/proj");
            let s = crate::file::to_json(h.project(), dir).unwrap();
            let loaded = crate::file::from_json(&s, dir, |_| true).unwrap();
            prop_assert_eq!(&loaded, h.project());
            // the id counters came back too: the next id matches
            let mut a = History::new(loaded);
            prop_assert_eq!(a.apply(Command::NewLink), h.apply(Command::NewLink));
        }
    }

    #[test]
    fn solids_add_recolor_and_reject_files() {
        let mut h = History::new(Project::default());
        let Created::Asset(id) = h
            .apply(Command::AddSolid {
                name: "Black".into(),
                color: [0.0, 0.0, 0.0, 1.0],
            })
            .unwrap()
        else {
            panic!()
        };
        let red = [1.0, 0.0, 0.0, 1.0];
        h.apply(Command::SetSolidColor { id, color: red }).unwrap();
        let a = h.project().asset(id).unwrap();
        assert_eq!((a.display_name().as_str(), a.solid), ("Black", Some(red)));
        let Created::Asset(file) = h.apply(Command::AddAsset("a.mkv".into())).unwrap() else {
            panic!()
        };
        let recolor = Command::SetSolidColor {
            id: file,
            color: red,
        };
        assert!(h.apply(recolor).is_err());
        assert!(h.undo() && h.undo());
        assert_eq!(
            h.project().asset(id).unwrap().solid,
            Some([0.0, 0.0, 0.0, 1.0])
        );
    }

    /// Linked video and audio clips of a 100-frame file: source 10..50 at 5, and 60..70 at 60.
    fn cut_project() -> (History, Time, [ClipId; 2], TrackId) {
        let mut p = Project::default();
        let f = p.settings.frame_rate.time_floor(1);
        let fr = |n: i64| Time(f.0 * n);
        let asset = p.add_asset("rec.mkv");
        p.asset_mut(asset).unwrap().probe = Some(ProbeInfo {
            duration: fr(100),
            ..Default::default()
        });
        let v = p.sequence.add_track(TrackKind::Video, "V1");
        let a = p.sequence.add_track(TrackKind::Audio, "A1");
        let mut h = History::new(p);
        let mut first = None;
        for (i, o, at) in [(10, 50, 5), (60, 70, 60)] {
            let pl = Placement {
                asset,
                source_in: fr(i),
                source_out: fr(o),
                start: fr(at),
                video: Some(v),
                audio: Some(a),
                streams: vec![(Some(0), String::new())],
                insert: false,
                add_video: false,
                add_audio: false,
            };
            let Created::Clip(c) = h.apply(Command::Place(pl)).unwrap() else {
                panic!()
            };
            first.get_or_insert(c);
        }
        let first = first.unwrap();
        let second = h.project().sequence().track(v).unwrap().clips()[1].id;
        (h, f, [first, second], a)
    }

    /// (start, source_in, source_out) in frames on each track.
    fn frames(h: &History, f: Time) -> Vec<Vec<(i64, i64, i64)>> {
        let s = h.project().sequence();
        let n = |t: Time| t.0 / f.0;
        let spans = s.tracks().iter().map(|t| {
            let c = t.clips().iter();
            c.map(|c| (n(c.start), n(c.source_in), n(c.source_out)))
                .collect()
        });
        spans.collect()
    }

    #[test]
    fn trims_hold_to_media_and_neighbours_and_undo_in_one_step() {
        let (mut h, f, [c, _], _) = cut_project();
        let fr = |n: i64| Time(f.0 * n);
        let before = h.project().clone();
        let trim = |edge, by, ripple| Command::Trim {
            trims: vec![(c, edge, by)],
            ripple,
        };
        // Tail out to the next clip, though the media has more; audio follows.
        h.apply(trim(Edge::Tail, fr(100), false)).unwrap();
        let both = |v: Vec<(i64, i64, i64)>| vec![v.clone(), v];
        assert_eq!(frames(&h, f), both(vec![(5, 10, 65), (60, 60, 70)]));
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        // Head back to zero on the timeline, though the media has 10 frames before.
        h.apply(trim(Edge::Head, fr(-100), false)).unwrap();
        assert_eq!(frames(&h, f), both(vec![(0, 5, 50), (60, 60, 70)]));
        assert!(h.undo());
        // Ripple: the head stays and later clips follow; a clip keeps one frame.
        h.apply(trim(Edge::Head, fr(5), true)).unwrap();
        assert_eq!(frames(&h, f), both(vec![(5, 15, 50), (55, 60, 70)]));
        h.apply(trim(Edge::Tail, fr(-100), true)).unwrap();
        assert_eq!(frames(&h, f), both(vec![(5, 15, 16), (21, 60, 70)]));
        h.apply(trim(Edge::Tail, fr(100), true)).unwrap();
        assert_eq!(frames(&h, f), both(vec![(5, 15, 100), (105, 60, 70)]));
        assert!(h.undo() && h.undo() && h.undo());
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn split_and_close_gap_are_one_step_each() {
        let (mut h, f, [c, _], a) = cut_project();
        let fr = |n: i64| Time(f.0 * n);
        let before = h.project().clone();
        let ids = h.project().sequence().linked(c).into_iter();
        let ids = ids.map(|(_, c)| c.id).collect();
        h.apply(Command::Split { ids, at: fr(20) }).unwrap();
        let both = |v: Vec<(i64, i64, i64)>| vec![v.clone(), v];
        assert_eq!(
            frames(&h, f),
            both(vec![(5, 10, 25), (20, 25, 50), (60, 60, 70)])
        );
        // The tails are linked to each other, not to the heads.
        let s = h.project().sequence();
        let tail = s.track(a).unwrap().clips()[1].id;
        assert_eq!(s.linked(tail).len(), 2);
        assert_eq!(s.linked(c).len(), 2);
        assert!(s.linked(c).iter().all(|(_, x)| x.start == fr(5)));
        assert!(h.undo());
        assert_eq!(h.project(), &before);

        assert_eq!(
            h.project().sequence().gap_at(a, fr(50)),
            Some((fr(45), fr(60)))
        );
        h.apply(Command::CloseGap {
            track: a,
            at: fr(50),
        })
        .unwrap();
        // The video partner follows, keeping sync.
        assert_eq!(frames(&h, f), both(vec![(5, 10, 50), (45, 60, 70)]));
        let s = h.project().sequence();
        assert_eq!(s.gap_at(a, fr(10)), None);
        assert!(
            h.apply(Command::CloseGap {
                track: a,
                at: fr(10)
            })
            .is_err()
        );
        assert!(h.undo());
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn cut_spans_ripple_or_speed_up_in_one_step() {
        let (mut h, f, [c, _], _) = cut_project();
        let fr = |n: i64| Time(f.0 * n);
        let before = h.project().clone();
        let both = |v: Vec<(i64, i64, i64)>| vec![v.clone(), v];
        // Given in any order; the linked audio follows.
        let spans = vec![(c, fr(15), fr(20)), (c, fr(30), fr(35))];
        h.apply(Command::CutSpans { spans, speed: None }).unwrap();
        assert_eq!(
            frames(&h, f),
            both(vec![(5, 10, 20), (15, 25, 35), (25, 40, 50), (50, 60, 70)])
        );
        assert_eq!(h.project().sequence().duration(), fr(60));
        assert!(h.undo());
        assert_eq!(h.project(), &before);

        // Ten frames at 2x play in five, so everything after moves five earlier.
        let spans = vec![(c, fr(15), fr(25))];
        h.apply(Command::CutSpans {
            spans,
            speed: Some(2.0),
        })
        .unwrap();
        assert_eq!(
            frames(&h, f),
            both(vec![(5, 10, 20), (15, 20, 30), (20, 30, 50), (55, 60, 70)])
        );
        assert_eq!(h.project().sequence().duration(), fr(65));
        assert!(h.undo());
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn speed_keys_retime_linked_clips_one_step_each() {
        use crate::keyframe::{Easing, SPEED, Value};
        let (mut h, f, [c, next], a) = cut_project();
        let fr = |n: i64| Time(f.0 * n);
        let key = |n, v| Keyframe {
            time: fr(n),
            value: Value::Scalar(v),
            easing: Easing::Linear,
        };
        let add = |key| Command::AddKeyframe {
            id: c,
            param: SPEED.into(),
            key,
        };
        // The clip plays source 10..50 from 5, and the next clip starts 15 frames after it.
        let check = |h: &History, len: f64| {
            let s = h.project().sequence();
            let clip = |t: TrackId| s.track(t).unwrap().clips()[0].clone();
            let (v, au) = (s.clip(c).unwrap().1.clone(), clip(a));
            assert_eq!(v.keyframes, au.keyframes, "linked clips share the ramp");
            assert_eq!((v.start, v.duration()), (au.start, au.duration()));
            assert!(
                (v.duration().0 as f64 - len * f.0 as f64).abs() <= 1.0,
                "{len}"
            );
            assert_eq!(s.clip(next).unwrap().1.start - v.end(), fr(15), "ripples");
        };
        let mut steps = vec![h.project().clone()];
        h.apply(add(key(10, 1.0))).unwrap();
        check(&h, 40.0);
        steps.push(h.project().clone());
        h.apply(add(key(50, 2.0))).unwrap();
        check(&h, 40.0 * 2f64.ln());
        steps.push(h.project().clone());
        h.apply(Command::MoveKeyframe {
            id: c,
            param: SPEED.into(),
            from: fr(50),
            to: fr(30),
        })
        .unwrap();
        check(&h, 20.0 * 2f64.ln() + 10.0);
        steps.push(h.project().clone());
        h.apply(Command::RemoveKeyframe {
            id: c,
            param: SPEED.into(),
            time: fr(10),
        })
        .unwrap();
        check(&h, 20.0);
        // Each edit undoes on its own, back through every state.
        while let Some(want) = steps.pop() {
            assert!(h.undo());
            assert_eq!(h.project(), &want);
        }
        // A bad speed changes nothing.
        let bad = add(key(20, f64::NAN));
        assert!(h.apply(bad).is_err());
        assert_eq!(h.project().sequence().clip(c).unwrap().1.ramp(), None);
    }

    #[test]
    fn lift_and_extract_the_in_out_range() {
        let (mut h, f, _, _) = cut_project();
        let fr = |n: i64| Time(f.0 * n);
        let before = h.project().clone();
        let both = |v: Vec<(i64, i64, i64)>| vec![v.clone(), v];
        let lift = Command::RemoveRange { ripple: false };
        assert!(h.apply(lift.clone()).is_err());
        let marks = Command::SetInOut {
            in_point: Some(fr(20)),
            out_point: Some(fr(30)),
        };
        h.apply(marks.clone()).unwrap();
        h.apply(lift).unwrap();
        assert_eq!(
            frames(&h, f),
            both(vec![(5, 10, 25), (30, 35, 50), (60, 60, 70)])
        );
        assert_eq!(h.project().sequence().in_point(), None);
        assert!(h.undo() && h.undo());
        h.apply(marks).unwrap();
        h.apply(Command::RemoveRange { ripple: true }).unwrap();
        assert_eq!(
            frames(&h, f),
            both(vec![(5, 10, 25), (20, 35, 50), (50, 60, 70)])
        );
        // Split tails stay linked to each other.
        let s = h.project().sequence();
        let tail = s.tracks()[0].clips()[1].id;
        assert_eq!(s.linked(tail).len(), 2);
        assert!(h.undo() && h.undo());
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn ripples_refuse_when_a_partner_would_collide() {
        let (mut h, f, [c, _], a) = cut_project();
        let fr = |n: i64| Time(f.0 * n);
        let v = h.project().sequence().tracks()[0].id;
        let asset = h.project().assets()[0].id;
        let clip = NewClip {
            asset,
            source_in: Time(0),
            source_out: fr(5),
            start: fr(50),
            link: None,
        };
        h.apply(Command::AddClip { track: v, clip }).unwrap();
        let before = h.project().clone();
        let gap = Command::CloseGap {
            track: a,
            at: fr(50),
        };
        assert!(matches!(h.apply(gap), Err(ModelError::Overlap(_))));
        // A ripple trim of the audio alone pulls the later video partner into it too.
        let audio = h.project().sequence().track(a).unwrap().clips()[0].id;
        let trims = vec![(audio, Edge::Tail, fr(-10))];
        h.apply(Command::LinkClips {
            ids: vec![c],
            linked: false,
        })
        .unwrap();
        let ripple = Command::Trim {
            trims,
            ripple: true,
        };
        assert!(matches!(h.apply(ripple), Err(ModelError::Overlap(_))));
        assert!(h.undo());
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn no_op_edits_leave_no_undo_step() {
        let (mut h, f, [c, _], _) = cut_project();
        let fr = |n: i64| Time(f.0 * n);
        // A cut on a clip's edge, and a trim already at its limit.
        h.apply(Command::Split {
            ids: vec![c],
            at: fr(5),
        })
        .unwrap();
        assert!(h.undo());
        // Undo skips the no-op and takes back the last placement.
        assert_eq!(h.project().sequence().clips_at(fr(65)).count(), 0);
        assert!(h.redo());
        let before = h.project().clone();
        let trims = vec![(c, Edge::Head, fr(-100))];
        h.apply(Command::Trim {
            trims: trims.clone(),
            ripple: false,
        })
        .unwrap();
        h.apply(Command::Trim {
            trims,
            ripple: false,
        })
        .unwrap();
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        // A tail whose partner wasn't cut gets no link of its own.
        h.apply(Command::Split {
            ids: vec![c],
            at: fr(20),
        })
        .unwrap();
        let tail = h.project().sequence().tracks()[0].clips()[1].id;
        assert_eq!(h.project().sequence().clip(tail).unwrap().1.link, None);
    }

    #[test]
    fn text_adds_edits_undoes_and_rejects_files() {
        let mut h = History::new(Project::default());
        let [(name, text), ..] = Text::presets();
        let Created::Asset(id) = h
            .apply(Command::AddText {
                name: name.into(),
                text: text.clone(),
            })
            .unwrap()
        else {
            panic!()
        };
        let edited = Text {
            content: "مرحبا".into(),
            ..text.clone()
        };
        h.apply(Command::SetText {
            id,
            text: edited.clone(),
        })
        .unwrap();
        let a = h.project().asset(id).unwrap();
        assert!(a.generated());
        assert_eq!(a.text.as_ref(), Some(&edited));
        let Created::Asset(file) = h.apply(Command::AddAsset("a.mkv".into())).unwrap() else {
            panic!()
        };
        assert!(
            h.apply(Command::SetText {
                id: file,
                text: edited
            })
            .is_err()
        );
        assert!(h.undo() && h.undo());
        assert_eq!(h.project().asset(id).unwrap().text, Some(text));
    }

    #[test]
    fn imported_subtitles_make_a_sidecar_lined_up_with_in_to_out() {
        use crate::srt;
        let src = "1\n00:00:01,000 --> 00:00:03,000\nOne\n\n\
                   2\n00:00:02,000 --> 00:00:04,000\nOverlaps\n\n\
                   3\n00:00:10,000 --> 00:00:12,000\nTwo\nlines\n";
        let mut h = History::new(Project::default());
        let Ok(Created::Track(track)) = h.apply(Command::ImportSubtitles {
            name: "Subtitles".into(),
            cues: srt::parse(src),
            style: Text::default(),
        }) else {
            panic!("import")
        };
        let p = h.project();
        assert!(p.sequence().track(track).unwrap().subtitles);
        assert_eq!(p.sequence().track(track).unwrap().clips().len(), 3);
        let s = |n: i64| Time(n * crate::time::FLICKS_PER_SECOND);
        h.apply(Command::SetInOut {
            in_point: Some(s(2)),
            out_point: Some(s(11)),
        })
        .unwrap();
        let p = h.project();
        let range = (
            p.sequence().in_point().unwrap(),
            p.sequence().out_point().unwrap(),
        );
        assert_eq!(
            srt::write(&p.subtitle_cues(), range),
            "1\n00:00:00,000 --> 00:00:01,000\nOne\n\n\
             2\n00:00:01,000 --> 00:00:02,000\nOverlaps\n\n\
             3\n00:00:08,000 --> 00:00:09,000\nTwo\nlines\n\n"
        );
        assert!(
            p.without_subtitles()
                .sequence()
                .track(track)
                .unwrap()
                .hidden
        );
        assert!(
            h.undo() && h.undo() && h.project().assets().is_empty(),
            "one undo step each"
        );
    }

    fn new_clip(asset: AssetId, start: i64, len: i64) -> NewClip {
        NewClip {
            asset,
            source_in: Time(0),
            source_out: Time(len),
            start: Time(start),
            link: None,
        }
    }

    /// V1 holds clips at 10 and 25 and A1 one at 10, all 10 long; nested into "N".
    fn nested() -> (History, Project, [ClipId; 3], TrackId) {
        let mut h = History::new(Project::default());
        h.apply(Command::AddAsset("a.mkv".into())).unwrap();
        let v = track(&mut h);
        let Ok(Created::Track(a)) = h.apply(Command::AddTrack {
            kind: TrackKind::Audio,
            name: "A1".into(),
        }) else {
            panic!()
        };
        let mut ids = [ClipId(0); 3];
        for (i, (t, start)) in [(v, 10), (v, 25), (a, 10)].into_iter().enumerate() {
            let clip = new_clip(AssetId(1), start, 10);
            let Ok(Created::Clip(c)) = h.apply(Command::AddClip { track: t, clip }) else {
                panic!()
            };
            ids[i] = c;
        }
        let before = h.project().clone();
        let nest = Command::Nest {
            ids: ids.to_vec(),
            name: "N".into(),
            outer: "Main".into(),
        };
        assert!(matches!(h.apply(nest), Ok(Created::Clip(_))));
        (h, before, ids, v)
    }

    #[test]
    fn nest_replaces_clips_in_one_undo_step_and_renders_the_same() {
        use crate::eval::{frame_at, video_at};
        let (mut h, before, [c1, c2, c3], v) = nested();
        let p = h.project();
        let (outer, inner) = (p.sequence(), p.sequences()[1]);
        assert_eq!(outer.id(), SequenceId(0));
        assert_eq!(inner.id(), SequenceId(1));
        let names: Vec<_> = p.assets().iter().map(|a| a.display_name()).collect();
        assert_eq!(names, ["a.mkv", "N", "Main"]);
        // One linked clip per kind over [10, 35), playing the nest from its start.
        let clips: Vec<_> = outer.tracks().iter().flat_map(|t| t.clips()).collect();
        assert_eq!(clips.len(), 2);
        for c in &clips {
            assert_eq!(
                (c.asset, c.start, c.source_in),
                (AssetId(2), Time(10), Time(0))
            );
            assert_eq!(c.source_out, Time(25));
            assert!(c.link.is_some() && c.link == clips[0].link);
        }
        let starts = |id| inner.clip(id).map(|(_, c)| c.start);
        assert_eq!(
            [starts(c1), starts(c2), starts(c3)],
            [Some(Time(0)), Some(Time(15)), Some(Time(0))]
        );
        // The nest shows what the clips showed, however deep.
        for t in [12, 22, 27] {
            let was: Vec<_> = video_at(before.sequence(), Time(t))
                .iter()
                .map(|l| (l.clip, l.source_time))
                .collect();
            let now = frame_at(p, Time(t));
            let now: Vec<_> = now
                .iter()
                .flat_map(|l| &l.nested)
                .map(|l| (l.clip, l.source_time))
                .collect();
            assert_eq!(now, was, "{t}");
        }
        // Survives a save.
        let dir = std::path::Path::new("/p");
        let json = crate::file::to_json(p, dir).unwrap();
        assert_eq!(&crate::file::from_json(&json, dir, |_| true).unwrap(), p);
        // An unchosen clip in the way refuses it and changes nothing.
        let nested = h.project().clone();
        h.apply(Command::AddClip {
            track: v,
            clip: new_clip(AssetId(1), 100, 10),
        })
        .unwrap();
        let Created::Clip(far) = h
            .apply(Command::AddClip {
                track: v,
                clip: new_clip(AssetId(1), 140, 10),
            })
            .unwrap()
        else {
            panic!()
        };
        let edited = h.project().clone();
        let first = edited.sequence().tracks()[0]
            .clips()
            .iter()
            .find(|c| c.start == Time(100))
            .unwrap()
            .id;
        let bad = Command::Nest {
            ids: vec![first, far],
            name: "M".into(),
            outer: "Main".into(),
        };
        h.apply(Command::AddClip {
            track: v,
            clip: new_clip(AssetId(1), 120, 10),
        })
        .unwrap();
        assert!(matches!(h.apply(bad), Err(ModelError::Overlap(_))));
        assert!(h.undo() && h.undo() && h.undo());
        assert_eq!(h.project(), &nested);
        assert!(h.undo(), "nest is one step");
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn nesting_a_sequence_in_itself_is_refused() {
        let (mut h, _, [c1, ..], v) = nested();
        let (n, main) = (AssetId(2), AssetId(3));
        let add = |asset| Command::AddClip {
            track: v,
            clip: new_clip(asset, 100, 5),
        };
        // Directly: Main into Main, N into N.
        assert_eq!(h.apply(add(main)), Err(ModelError::Cycle));
        let p = h.project();
        let place = p.placement(main, Time(100), false, Some(v), false).unwrap();
        assert_eq!(h.apply(Command::Place(place)), Err(ModelError::Cycle));
        h.open_sequence(SequenceId(1)).unwrap();
        assert_eq!(h.apply(add(n)), Err(ModelError::Cycle));
        // Indirectly: Main holds N, which would hold Main; and through a nest inside N.
        assert_eq!(h.apply(add(main)), Err(ModelError::Cycle));
        let deeper = Command::Nest {
            ids: vec![c1],
            name: "N2".into(),
            outer: "N".into(),
        };
        let Ok(Created::Clip(inner)) = h.apply(deeper) else {
            panic!()
        };
        assert_eq!(h.project().assets().len(), 4, "N was already in the bin");
        let set = Command::SetClipAsset {
            id: inner,
            asset: main,
        };
        assert_eq!(h.apply(set), Err(ModelError::Cycle));
        h.open_sequence(SequenceId(2)).unwrap();
        assert_eq!(h.apply(add(main)), Err(ModelError::Cycle));
        assert_eq!(h.apply(add(n)), Err(ModelError::Cycle));
        assert!(h.apply(add(AssetId(1))).is_ok());
        // A file with one anyway doesn't open.
        let mut p = h.project().clone();
        p.sequence.add_clip(v, new_clip(main, 200, 5)).unwrap();
        assert!(p.has_cycle());
        let dir = std::path::Path::new("/p");
        let json = crate::file::to_json(&p, dir).unwrap();
        let opened = crate::file::from_json(&json, dir, |_| true);
        assert!(matches!(opened, Err(crate::file::FileError::Cycle)));
    }

    #[test]
    fn switching_sequences_is_not_an_edit() {
        let (mut h, _, _, v) = nested();
        h.mark_saved();
        assert_eq!(
            h.open_sequence(SequenceId(9)),
            Err(ModelError::UnknownSequence(SequenceId(9)))
        );
        h.open_sequence(SequenceId(1)).unwrap();
        assert_eq!(h.project().sequence().id(), SequenceId(1));
        assert!(!h.is_dirty());
        // The open sequence can't go; another one takes its clips with it.
        assert_eq!(
            h.apply(Command::RemoveAsset(AssetId(2))),
            Err(ModelError::SequenceOpen)
        );
        h.apply(Command::AddClip {
            track: v,
            clip: new_clip(AssetId(1), 100, 5),
        })
        .unwrap();
        assert!(h.undo());
        assert_eq!(
            h.project().sequence().id(),
            SequenceId(1),
            "undo stays in the sequence"
        );
        h.open_sequence(SequenceId(0)).unwrap();
        assert_eq!(h.project().clips_using(AssetId(1)), 3);
        h.apply(Command::RemoveAsset(AssetId(2))).unwrap();
        assert_eq!(h.project().sequences().len(), 1);
        assert!(
            h.project()
                .sequence()
                .tracks()
                .iter()
                .all(|t| t.clips().is_empty())
        );
    }

    #[test]
    fn nesting_deeper_than_the_limit_is_refused() {
        use crate::model::MAX_NEST_DEPTH;
        let mut h = History::new(Project::default());
        h.apply(Command::AddAsset("a.mkv".into())).unwrap();
        let v = track(&mut h);
        let mut add = |start| match h.apply(Command::AddClip {
            track: v,
            clip: new_clip(AssetId(1), start, 10),
        }) {
            Ok(Created::Clip(c)) => c,
            _ => panic!(),
        };
        let (mut c, aside) = (add(0), add(100));
        let nest = |ids| Command::Nest {
            ids,
            name: "N".into(),
            outer: "Main".into(),
        };
        // Sequence 1 sits beside the chain; each nest of the nest clip deepens it by one.
        h.apply(nest(vec![aside])).unwrap();
        let side = h.project().sequence_asset(SequenceId(1)).unwrap().id;
        for _ in 1..MAX_NEST_DEPTH {
            let Ok(Created::Clip(n)) = h.apply(nest(vec![c])) else {
                panic!()
            };
            c = n;
        }
        assert!(!h.project().too_deep());
        assert_eq!(h.apply(nest(vec![c])), Err(ModelError::TooDeep));
        let dir = std::path::Path::new("/p");
        let json = crate::file::to_json(h.project(), dir).unwrap();
        assert!(crate::file::from_json(&json, dir, |_| true).is_ok());
        // The innermost sequence, at the limit, takes no sequence clip.
        h.open_sequence(SequenceId(2)).unwrap();
        let add_side = Command::AddClip {
            track: v,
            clip: new_clip(side, 200, 10),
        };
        assert_eq!(h.apply(add_side), Err(ModelError::TooDeep));
        // A file with one anyway doesn't open.
        let mut p = h.project().clone();
        p.sequence.add_clip(v, new_clip(side, 200, 10)).unwrap();
        assert!(p.too_deep());
        let json = crate::file::to_json(&p, dir).unwrap();
        let opened = crate::file::from_json(&json, dir, |_| true);
        assert!(matches!(opened, Err(crate::file::FileError::TooDeep)));
    }

    #[test]
    fn a_file_with_mismatched_sequence_ids_is_refused() {
        use crate::file::{FileError, from_json, to_json};
        let (h, ..) = nested();
        let dir = std::path::Path::new("/p");
        let json = to_json(h.project(), dir).unwrap();
        let load = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
            edit(&mut v["project"]);
            from_json(&v.to_string(), dir, |_| true)
        };
        assert!(load(&|_| {}).is_ok());
        let bad = |edit: &dyn Fn(&mut serde_json::Value)| {
            assert!(matches!(load(edit), Err(FileError::BadSequence)));
        };
        // Assets: a.mkv, N (sequence 1), Main (sequence 0).
        bad(&|p| p["assets"][1]["sequence"] = 7.into());
        bad(&|p| {
            p["sequence"]["id"] = 1.into();
            p["assets"][2]["sequence"] = 1.into();
        });
        // The next Nest would reuse sequence 1.
        bad(&|p| p["next_sequence"] = 0.into());
    }

    #[test]
    fn nest_refuses_to_split_a_transition() {
        use crate::model::{Transition, TransitionKind};
        let mut h = History::new(Project::default());
        h.apply(Command::AddAsset("a.mkv".into())).unwrap();
        let v = track(&mut h);
        let mut ids = Vec::new();
        for start in [0, 10] {
            let clip = new_clip(AssetId(1), start, 10);
            let Ok(Created::Clip(c)) = h.apply(Command::AddClip { track: v, clip }) else {
                panic!()
            };
            ids.push(c);
        }
        let c = h.project().sequence().clip(ids[0]).unwrap().1.clone();
        let fade = Transform {
            transition: Some(Transition {
                kind: TransitionKind::Crossfade,
                duration: Time(4),
            }),
            ..c.transform
        };
        h.apply(Command::SetClipProps(vec![(ids[0], fade, c.keyframes)]))
            .unwrap();
        for one in [ids[0], ids[1]] {
            let nest = Command::Nest {
                ids: vec![one],
                name: "N".into(),
                outer: "Main".into(),
            };
            assert_eq!(h.apply(nest), Err(ModelError::SplitTransition));
        }
        let both = Command::Nest {
            ids,
            name: "N".into(),
            outer: "Main".into(),
        };
        assert!(h.apply(both).is_ok());
    }

    /// A trimmed nest clip at double speed plays its sequence from the trimmed point, twice as
    /// fast, in picture and sound.
    #[test]
    fn a_trimmed_fast_nest_clip_maps_time_into_its_sequence() {
        use crate::eval::{audio_in, frame_at};
        let (mut h, _, [c1, c2, c3], _) = nested();
        let outer = h.project().sequence().tracks().iter();
        let outer: Vec<_> = outer.flat_map(|t| t.clips()).map(|c| c.id).collect();
        for &id in &outer {
            let trim = Command::TrimClip {
                id,
                start: Time(10),
                source_in: Time(4),
                source_out: Time(24),
            };
            h.apply(trim).unwrap();
        }
        let speed = Command::SetClipSpeed {
            id: outer[0],
            speed: 2.0,
            reverse: false,
            ripple: false,
        };
        h.apply(speed).unwrap();
        let p = h.project();
        // Nest time 4 + 2 × (t - 10): 8 in c1 (at 0), then 16 in c2 (at 15).
        for (t, clip, source) in [(12, c1, 8), (16, c2, 1)] {
            let layers = frame_at(p, Time(t));
            let inner = &layers[0].nested[0];
            assert_eq!((inner.clip, inner.source_time), (clip, Time(source)), "{t}");
        }
        let seg = audio_in(p.sequence(), Time(12), Time(14));
        assert_eq!(
            (seg.len(), seg[0].source_start, seg[0].speed),
            (1, Time(8), 2.0)
        );
        let nest = p.nested(seg[0].asset).unwrap();
        let inner = audio_in(nest, Time(8), Time(12));
        assert_eq!((inner.len(), inner[0].clip), (1, c3));
        assert_eq!((inner[0].source_start, inner[0].end), (Time(8), Time(10)));
    }
}

#[cfg(test)]
mod audit_b04 {
    use super::*;
    use crate::time::FLICKS_PER_SECOND as S;

    fn fixture() -> (History, TrackId, TrackId, ClipId, ClipId, ClipId) {
        let mut p = Project::default();
        let s = &mut p.sequence;
        let v = s.add_track(TrackKind::Video, "V1");
        let a = s.add_track(TrackKind::Audio, "A1");
        let mut add = |t, at, len, link| {
            s.add_clip(
                t,
                NewClip {
                    asset: AssetId(1),
                    start: Time(at * S),
                    source_in: Time(0),
                    source_out: Time(len * S),
                    link,
                },
            )
            .unwrap()
        };
        let intro = add(v, 0, 5, None);
        let cv = add(v, 5, 10, Some(LinkId(900)));
        let ca = add(a, 5, 10, Some(LinkId(900)));
        (History::new(p), v, a, intro, cv, ca)
    }

    #[test]
    fn ripple_commands_refuse_downstream_locked_partners_atomically() {
        let (h, v, a, intro, _, _) = fixture();
        let commands = vec![
            Command::Trim {
                trims: vec![(intro, Edge::Tail, Time(-S))],
                ripple: true,
            },
            Command::SetClipSpeed {
                id: intro,
                speed: 2.0,
                reverse: false,
                ripple: true,
            },
            Command::SetSpeedKeys {
                id: intro,
                speed: 2.0,
                keys: Vec::new(),
                ripple: true,
            },
            Command::CutSpans {
                spans: vec![(intro, Time(S), Time(2 * S))],
                speed: None,
            },
            Command::CutSpans {
                spans: vec![(intro, Time(S), Time(2 * S))],
                speed: Some(2.0),
            },
        ];
        for command in commands {
            let mut p = h.project().clone();
            let mut settings = p.sequence().track(a).unwrap().settings();
            settings.locked = true;
            p.sequence.set_track(settings).unwrap();
            let mut h = History::new(p);
            let before = h.project().clone();
            let bytes = serde_json::to_vec(&before).unwrap();
            assert_eq!(h.apply(command), Err(ModelError::Locked(a)));
            assert_eq!(serde_json::to_vec(h.project()).unwrap(), bytes);
            assert!(!h.can_undo() && !h.is_dirty());
        }
        let mut p = h.project().clone();
        p.sequence.remove_clips(&[intro], false).unwrap();
        let mut settings = p.sequence().track(a).unwrap().settings();
        settings.locked = true;
        p.sequence.set_track(settings).unwrap();
        let mut h = History::new(p.clone());
        assert_eq!(
            h.apply(Command::CloseGap {
                track: v,
                at: Time(S)
            }),
            Err(ModelError::Locked(a))
        );
        assert_eq!(h.project(), &p);
        assert!(!h.can_undo());
    }

    #[test]
    fn ripple_trim_unlocked_preserves_later_sync() {
        let (mut h, _, _, intro, cv, ca) = fixture();
        let before = h.project().clone();
        h.apply(Command::Trim {
            trims: vec![(intro, Edge::Tail, Time(-S))],
            ripple: true,
        })
        .unwrap();
        for id in [cv, ca] {
            assert_eq!(
                h.project().sequence().clip(id).unwrap().1.start,
                Time(4 * S)
            );
        }
        assert!(h.undo());
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn ripple_delete_unlinked_intro_moves_later_linked_group_once() {
        let (mut h, _, _, intro, cv, ca) = fixture();
        let before = h.project().clone();
        h.apply(Command::RemoveClips {
            ids: vec![intro],
            ripple: true,
        })
        .unwrap();
        for id in [cv, ca] {
            assert_eq!(h.project().sequence().clip(id).unwrap().1.start, Time(0));
        }
        let after = h.project().clone();
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        assert!(!h.can_undo());
        assert!(h.redo());
        assert_eq!(h.project(), &after);
    }

    #[test]
    fn ripple_delete_offset_linked_pair_closes_one_interval() {
        let (h, _, a, intro, cv, ca) = fixture();
        let mut p = h.project().clone();
        p.sequence.move_clip(ca, a, Time(6 * S)).unwrap();
        let audio_intro = p
            .sequence
            .add_clip(
                a,
                NewClip {
                    asset: AssetId(1),
                    start: Time(S),
                    source_in: Time(0),
                    source_out: Time(5 * S),
                    link: None,
                },
            )
            .unwrap();
        p.sequence.link_clips(&[intro, audio_intro], true).unwrap();
        let before = p.clone();
        let mut h = History::new(p);
        h.apply(Command::RemoveClips {
            ids: vec![intro, audio_intro],
            ripple: true,
        })
        .unwrap();
        assert_eq!(h.project().sequence().clip(cv).unwrap().1.start, Time(0));
        assert_eq!(h.project().sequence().clip(ca).unwrap().1.start, Time(S));
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        assert!(!h.can_undo());
    }

    #[test]
    fn ripple_delete_multiple_intervals_and_pair_are_not_counted_twice() {
        let (h, v, a, intro, cv, ca) = fixture();
        let mut p = h.project().clone();
        // Delete the intro and the first half of the later pair; leave linked tails.
        p.sequence.split_at(&[cv, ca], Time(10 * S)).unwrap();
        let tails: Vec<_> = p
            .sequence
            .tracks()
            .iter()
            .flat_map(|t| t.clips())
            .filter(|c| c.start == Time(10 * S))
            .map(|c| c.id)
            .collect();
        let before = p.clone();
        let mut h = History::new(p);
        h.apply(Command::RemoveClips {
            ids: vec![intro, cv, ca],
            ripple: true,
        })
        .unwrap();
        for id in tails {
            assert_eq!(h.project().sequence().clip(id).unwrap().1.start, Time(0));
        }
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        assert!(!h.can_undo());
        // A colliding stationary clip on the partner's track refuses the whole delete.
        let mut p = before;
        p.sequence
            .add_clip(
                a,
                NewClip {
                    asset: AssetId(1),
                    start: Time(0),
                    source_in: Time(0),
                    source_out: Time(S),
                    link: None,
                },
            )
            .unwrap();
        let mut h = History::new(p.clone());
        assert!(matches!(
            h.apply(Command::RemoveClips {
                ids: vec![intro],
                ripple: true
            }),
            Err(ModelError::Overlap(_))
        ));
        assert_eq!(h.project(), &p);
        assert!(!h.can_undo());
        assert_eq!(h.project().sequence().track(v).unwrap().clips().len(), 3);
    }

    #[test]
    fn save_finishes_a_coalesced_gesture() {
        let (mut h, v, _, _, _, _) = fixture();
        let setting = |h: &History, volume| {
            let mut t = h.project().sequence().track(v).unwrap().settings();
            t.volume = volume;
            Command::SetTrack(t)
        };
        h.apply_coalesced(123, setting(&h, 0.7)).unwrap();
        h.mark_saved();
        h.apply_coalesced(123, setting(&h, 0.9)).unwrap();
        assert!(h.is_dirty());
        assert!(h.undo());
        assert_eq!(h.project().sequence().track(v).unwrap().volume, 0.7);
        assert!(!h.is_dirty());
        assert!(h.undo());
        assert_eq!(h.project().sequence().track(v).unwrap().volume, 1.0);
    }

    #[test]
    fn move_selection_keeps_linked_and_unlinked_members_and_clamps_as_one() {
        let (h, v, _, _, cv, ca) = fixture();
        let mut p = h.project().clone();
        let title_track = p.sequence.add_track(TrackKind::Video, "Titles");
        let title = p
            .sequence
            .add_clip(
                title_track,
                NewClip {
                    asset: AssetId(1),
                    start: Time(2 * S),
                    source_in: Time(0),
                    source_out: Time(3 * S),
                    link: None,
                },
            )
            .unwrap();
        let before = p.clone();
        let mut h = History::new(p);
        let command = |start| Command::MoveSelection {
            ids: vec![cv, title],
            anchor: cv,
            track: v,
            start,
        };
        h.apply(command(Time(8 * S))).unwrap();
        for (id, expected) in [(cv, 8), (ca, 8), (title, 5)] {
            assert_eq!(
                h.project().sequence().clip(id).unwrap().1.start,
                Time(expected * S)
            );
        }
        assert_eq!(
            h.project().sequence().clip(cv).unwrap().1.link,
            before.sequence().clip(cv).unwrap().1.link
        );
        let after = h.project().clone();
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        assert!(!h.can_undo());
        assert!(h.redo());
        assert_eq!(h.project(), &after);
        assert!(h.undo());
        h.apply(command(Time(-10 * S))).unwrap();
        for (id, expected) in [(cv, 3), (ca, 3), (title, 0)] {
            assert_eq!(
                h.project().sequence().clip(id).unwrap().1.start,
                Time(expected * S)
            );
        }
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        let mut settings = h
            .project()
            .sequence()
            .track(title_track)
            .unwrap()
            .settings();
        settings.locked = true;
        h.apply(Command::SetTrack(settings)).unwrap();
        let locked = h.project().clone();
        let rev = h.rev();
        assert_eq!(
            h.apply(command(Time(8 * S))),
            Err(ModelError::Locked(title_track))
        );
        assert_eq!(h.project(), &locked);
        assert_eq!(h.rev(), rev);
    }

    #[test]
    fn move_selection_keeps_track_offsets_and_checks_all_destinations() {
        let (h, v, _, _, cv, _) = fixture();
        let mut p = h.project().clone();
        let upper = p.sequence.add_track(TrackKind::Video, "V2");
        let title = p
            .sequence
            .add_clip(
                upper,
                NewClip {
                    asset: AssetId(1),
                    start: Time(0),
                    source_in: Time(0),
                    source_out: Time(S),
                    link: None,
                },
            )
            .unwrap();
        let top = p.sequence.add_track(TrackKind::Video, "V3");
        // An unlinked video selection can move up one lane without changing its spacing.
        p.sequence.link_clips(&[cv], false).unwrap();
        let before = p.clone();
        let mut h = History::new(p);
        let cmd = Command::MoveSelection {
            ids: vec![cv, title],
            anchor: cv,
            track: upper,
            start: Time(5 * S),
        };
        h.apply(cmd.clone()).unwrap();
        assert_eq!(h.project().sequence().clip(cv).unwrap().0, upper);
        assert_eq!(h.project().sequence().clip(title).unwrap().0, top);
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        let mut settings = before.sequence().track(top).unwrap().settings();
        settings.locked = true;
        h.apply(Command::SetTrack(settings)).unwrap();
        let locked = h.project().clone();
        assert_eq!(h.apply(cmd), Err(ModelError::Locked(top)));
        assert_eq!(h.project(), &locked);
        assert!(h.undo());
        assert!(
            h.apply(Command::MoveSelection {
                ids: vec![title],
                anchor: title,
                track: v,
                start: Time(0),
            })
            .is_ok()
        );
    }
}
#[cfg(test)]
mod audit_444;
