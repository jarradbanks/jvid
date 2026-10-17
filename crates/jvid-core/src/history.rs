//! Commands and undo/redo. Every project change goes through [`History::apply`].
//!
//! Undo steps are project snapshots taken before each step: the project is
//! plain data with no media in it, so a clone is cheap and revert is exact.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use crate::keyframe::{self, Keyframe};
use crate::model::{
    AssetId, ClipId, LinkId, MarkerId, ModelError, NewClip, ProbeInfo, Project, ProjectSettings,
    TrackId, TrackKind,
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
    /// Replaces the track's settings; clips are untouched.
    SetTrack {
        id: TrackId,
        name: String,
        hidden: bool,
        locked: bool,
        muted: bool,
        solo: bool,
        volume: f32,
        pan: f32,
    },
    AddClip {
        track: TrackId,
        clip: NewClip,
    },
    RemoveClip(ClipId),
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
    AddMarker {
        time: Time,
        duration: Time,
        name: String,
    },
    RemoveMarker(MarkerId),
    SetInOut {
        in_point: Option<Time>,
        out_point: Option<Time>,
    },
    SetSettings(ProjectSettings),
    AddAsset(PathBuf),
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
    /// Removes the asset and every clip that uses it.
    RemoveAsset(AssetId),
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
    /// Applies to `p`; on error `p` is unchanged.
    fn apply(self, p: &mut Project) -> Result<Created, ModelError> {
        let s = &mut p.sequence;
        Ok(match self {
            Self::AddTrack { kind, name } => Created::Track(s.add_track(kind, name)),
            Self::RemoveTrack(id) => {
                s.remove_track(id)?;
                Created::Nothing
            }
            Self::SetTrack {
                id,
                name,
                hidden,
                locked,
                muted,
                solo,
                volume,
                pan,
            } => {
                let t = s.track_mut(id).ok_or(ModelError::UnknownTrack(id))?;
                t.name = name;
                t.hidden = hidden;
                t.locked = locked;
                t.muted = muted;
                t.solo = solo;
                t.volume = volume;
                t.pan = pan;
                Created::Nothing
            }
            Self::AddClip { track, clip } => Created::Clip(s.add_clip(track, clip)?),
            Self::RemoveClip(id) => {
                s.remove_clip(id)?;
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
                s.with_clip_mut(id, |c| {
                    keyframe::insert(c.keyframes.entry(param).or_default(), key)
                })?;
                Created::Nothing
            }
            Self::RemoveKeyframe { id, param, time } => {
                s.with_clip_mut(id, |c| take_key(c.keyframes, &param, time))?
                    .ok_or(ModelError::UnknownKeyframe(id, time))?;
                Created::Nothing
            }
            Self::MoveKeyframe {
                id,
                param,
                from,
                to,
            } => {
                s.with_clip_mut(id, |c| {
                    let mut k = take_key(c.keyframes, &param, from)?;
                    k.time = to;
                    keyframe::insert(c.keyframes.entry(param).or_default(), k);
                    Some(())
                })?
                .ok_or(ModelError::UnknownKeyframe(id, from))?;
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
            Self::SetInOut {
                in_point,
                out_point,
            } => {
                s.in_point = in_point;
                s.out_point = out_point;
                Created::Nothing
            }
            Self::SetSettings(settings) => {
                p.settings = settings;
                Created::Nothing
            }
            Self::AddAsset(path) => Created::Asset(p.add_asset(path)),
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
            Self::RemoveAsset(id) => {
                let i = p.assets.iter().position(|a| a.id == id);
                p.assets.remove(i.ok_or(ModelError::UnknownAsset(id))?);
                let s = &mut p.sequence;
                let used = s.tracks().iter().flat_map(|t| t.clips());
                let used: Vec<_> = used.filter(|c| c.asset == id).map(|c| c.id).collect();
                for c in used {
                    s.remove_clip(c)?;
                }
                Created::Nothing
            }
        })
    }
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
            let r = self.apply_step(cmd);
            if r.is_ok() && self.txn.is_none() {
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
        if self.txn.is_some() {
            let r = cmd.apply(&mut self.project)?;
            self.bump();
            return Ok(r);
        }
        let before = (self.project.clone(), self.rev);
        let r = cmd.apply(&mut self.project)?;
        self.push_undo(before);
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

    pub fn mark_saved(&mut self) {
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
        h.redo();
        assert!(!h.is_dirty());
        h.undo();
        track(&mut h); // a different edit from the saved state stays dirty
        assert!(h.is_dirty());
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
            (id.clone(), any::<[bool; 4]>(), 0f32..2.0).prop_map(|(i, f, v)| Command::SetTrack {
                id: TrackId(i),
                name: "t".into(),
                hidden: f[0],
                locked: f[1],
                muted: f[2],
                solo: f[3],
                volume: v,
                pan: -v / 2.0,
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
}
