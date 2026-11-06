//! Caption workspace contract. Subtitle-track text clips are the only store; hidden and
//! disabled captions remain editable. Edits detach shared assets, never changing a title
//! or another placed instance. Route every edit through `Command::Caption` for one undo.
//!
//! Shared styles are materialized Text values, so legacy readers, playback and sidecars
//! need no second model. `Keep` treats each value differing from the reference, and each
//! animated text property, as an override. `Replace` explicitly resets all style values
//! and text animation, preserving content, transforms and other clip properties.

use crate::history::Created;
use crate::model::{Clip, ClipId, ModelError, Project, Text, Track, TrackId, TrackKind};
use crate::time::Time;

#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    Sequence,
    Track(TrackId),
    Selection(Vec<ClipId>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overrides {
    #[default]
    Keep,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NotCaption(ClipId),
    InvalidTime,
    InvalidSplit,
    InvalidMerge,
    InvalidStyle,
    /// Changing the duration of a speed-ramped caption needs a timeline trim instead.
    AnimatedTiming,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotCaption(_) => "The selected clip is not a caption.",
            Self::InvalidTime => "Caption end must follow its nonnegative start.",
            Self::InvalidSplit => "Split inside both the caption text and its time range.",
            Self::InvalidMerge => "Select consecutive captions on one track to merge.",
            Self::InvalidStyle => "Caption style contains an invalid value.",
            Self::AnimatedTiming => "Trim speed-animated captions in the timeline.",
        })
    }
}

impl std::error::Error for Error {}

impl From<Error> for ModelError {
    fn from(error: Error) -> Self {
        Self::Caption(error)
    }
}

/// Commands resolve their scope against the current open sequence and validate every
/// target before committing. Search is literal, case-sensitive UTF-8, never a regex.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    Row {
        id: ClipId,
        text: String,
        start: Time,
        end: Time,
    },
    /// `character` counts Unicode scalar values, not UTF-8 bytes. Neither side is empty.
    Split {
        id: ClipId,
        at: Time,
        character: usize,
    },
    /// Keep the earliest caption's style/identity, join text with newlines, cover gaps.
    Merge { ids: Vec<ClipId> },
    Replace {
        scope: Scope,
        find: String,
        replacement: String,
    },
    Style {
        scope: Scope,
        reference: Box<Text>,
        style: Box<Text>,
        overrides: Overrides,
    },
}

pub struct Row<'a> {
    pub track: &'a Track,
    pub clip: &'a Clip,
    pub text: &'a Text,
}

/// Timeline order, including hidden/disabled tracks and clips. Visibility is view state;
/// the existing evaluation and subtitle export paths decide what is rendered.
pub fn rows(project: &Project) -> Vec<Row<'_>> {
    let mut rows: Vec<_> = project
        .sequence()
        .tracks()
        .iter()
        .filter(|t| t.kind == TrackKind::Video && t.subtitles)
        .flat_map(|track| {
            track.clips().iter().filter_map(move |clip| {
                Some(Row {
                    track,
                    clip,
                    text: project.asset(clip.asset)?.text.as_ref()?,
                })
            })
        })
        .collect();
    rows.sort_by_key(|r| (r.clip.start, r.track.id.0, r.clip.id.0));
    rows
}

impl Scope {
    pub fn ids(&self, project: &Project) -> Vec<ClipId> {
        rows(project)
            .iter()
            .filter(|r| match self {
                Self::Sequence => true,
                Self::Track(id) => r.track.id == *id,
                Self::Selection(ids) => ids.contains(&r.clip.id),
            })
            .map(|r| r.clip.id)
            .collect()
    }

    fn validate(&self, project: &Project) -> Result<Vec<ClipId>, ModelError> {
        // A stale/invalid explicit selection must fail the entire operation.
        if let Self::Selection(ids) = self {
            for &id in ids {
                caption(project, id)?;
            }
        }
        if let Self::Track(id) = self {
            let track = project
                .sequence()
                .track(*id)
                .ok_or(ModelError::UnknownTrack(*id))?;
            if !track.subtitles || track.kind != TrackKind::Video {
                return Err(ModelError::WrongKind(*id));
            }
        }
        let ids = self.ids(project);
        for &id in &ids {
            caption(project, id)?;
        }
        Ok(ids)
    }
}

fn caption(project: &Project, id: ClipId) -> Result<Row<'_>, ModelError> {
    let (track, clip) = project
        .sequence()
        .clip(id)
        .ok_or(ModelError::UnknownClip(id))?;
    let track = project.sequence().track(track).expect("clip track");
    if !track.subtitles || track.kind != TrackKind::Video {
        return Err(Error::NotCaption(id).into());
    }
    let text = project
        .asset(clip.asset)
        .and_then(|a| a.text.as_ref())
        .ok_or(Error::NotCaption(id))?;
    if track.locked {
        return Err(ModelError::Locked(track.id));
    }
    Ok(Row { track, clip, text })
}

impl Edit {
    pub(crate) fn apply(self, project: &mut Project) -> Result<Created, ModelError> {
        let mut next = project.clone();
        self.apply_to(&mut next)?;
        *project = next;
        Ok(Created::Nothing)
    }

    fn apply_to(self, project: &mut Project) -> Result<(), ModelError> {
        match self {
            Self::Row {
                id,
                text,
                start,
                end,
            } => {
                if start < Time(0) || end <= start {
                    return Err(Error::InvalidTime.into());
                }
                let row = caption(project, id)?;
                let source_out = source_end(row.clip, end - start)?;
                let mut style = row.text.clone();
                style.content = text;
                project
                    .sequence
                    .set_clip_range(id, start, row.clip.source_in, source_out)?;
                write_text(project, id, style)?;
            }
            Self::Split { id, at, character } => {
                let row = caption(project, id)?;
                let Some((byte, _)) = row.text.content.char_indices().nth(character) else {
                    return Err(Error::InvalidSplit.into());
                };
                if character == 0 || at <= row.clip.start || at >= row.clip.end() {
                    return Err(Error::InvalidSplit.into());
                }
                let track = row.track.id;
                let existing: Vec<_> = row.track.clips().iter().map(|c| c.id).collect();
                let mut left = row.text.clone();
                let mut right = left.clone();
                right.content = left.content.split_off(byte);
                // Existing split preserves trims, speed and animation source coordinates.
                project.sequence.split_at(&[id], at)?;
                let tail = project
                    .sequence
                    .track(track)
                    .expect("caption track")
                    .clips()
                    .iter()
                    .find(|c| !existing.contains(&c.id))
                    .ok_or(Error::InvalidSplit)?
                    .id;
                write_text(project, id, left)?;
                write_text(project, tail, right)?;
            }
            Self::Merge { ids } => {
                let mut ids = Scope::Selection(ids).validate(project)?;
                if ids.len() < 2 {
                    return Err(Error::InvalidMerge.into());
                }
                ids.sort_by_key(|id| project.sequence().clip(*id).expect("validated").1.start);
                let first = caption(project, ids[0])?;
                let track = first.track.id;
                let positions: Vec<_> = first
                    .track
                    .clips()
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| ids.contains(&c.id))
                    .map(|(i, _)| i)
                    .collect();
                if positions.len() != ids.len()
                    || positions.windows(2).any(|w| w[1] != w[0] + 1)
                    || ids
                        .iter()
                        .any(|id| project.sequence().clip(*id).expect("validated").0 != track)
                {
                    return Err(Error::InvalidMerge.into());
                }
                let mut text = first.text.clone();
                let start = first.clip.start;
                let source_in = first.clip.source_in;
                let end = ids
                    .iter()
                    .map(|id| project.sequence().clip(*id).expect("validated").1.end())
                    .max()
                    .expect("nonempty");
                let source_out = source_end(first.clip, end - start)?;
                text.content = ids
                    .iter()
                    .map(|id| {
                        caption(project, *id)
                            .expect("validated")
                            .text
                            .content
                            .as_str()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                for &id in &ids[1..] {
                    project.sequence.remove_clip(id)?;
                }
                project
                    .sequence
                    .set_clip_range(ids[0], start, source_in, source_out)?;
                write_text(project, ids[0], text)?;
            }
            Self::Replace {
                scope,
                find,
                replacement,
            } => {
                let ids = scope.validate(project)?;
                if find.is_empty() {
                    return Ok(());
                }
                for id in ids {
                    let mut text = caption(project, id)?.text.clone();
                    text.content = text.content.replace(&find, &replacement);
                    write_text(project, id, text)?;
                }
            }
            Self::Style {
                scope,
                reference,
                style,
                overrides,
            } => {
                let ids = scope.validate(project)?;
                if !valid_style(&style) {
                    return Err(Error::InvalidStyle.into());
                }
                for id in ids {
                    let row = caption(project, id)?;
                    let mut text = row.text.clone();
                    macro_rules! property {
                        ($($field:ident => $key:literal),+ $(,)?) => {$(
                            if overrides == Overrides::Replace || (text.$field == reference.$field
                                && !row.clip.keyframes.get($key).is_some_and(|keys| !keys.is_empty())) {
                                text.$field.clone_from(&style.$field);
                            }
                        )+};
                    }
                    property!(family => "", size => "text_size", weight => "", color => "color",
                        align => "", anchor => "text_anchor", outline => "outline",
                        outline_color => "outline_color", shadow => "shadow", shadow_color => "shadow_color",
                        box_padding => "box_padding", box_color => "box_color");
                    write_text(project, id, text)?;
                    if overrides == Overrides::Replace {
                        project.sequence.with_clip_mut(id, |c| {
                            for name in Text::ANIMATED.into_iter().chain(["color"]) {
                                c.keyframes.remove(name);
                            }
                        })?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn source_end(clip: &Clip, duration: Time) -> Result<Time, ModelError> {
    if duration == clip.duration() {
        return Ok(clip.source_out);
    }
    if clip.ramp().is_some() {
        return Err(Error::AnimatedTiming.into());
    }
    let rate = if clip.freeze.is_some() {
        1.0
    } else {
        clip.speed
    };
    let length = duration.0 as f64 * rate;
    if !length.is_finite() || length < 1.0 || length >= i64::MAX as f64 {
        return Err(Error::InvalidTime.into());
    }
    let end = clip
        .source_in
        .0
        .checked_add(length.ceil() as i64)
        .map(Time)
        .ok_or(Error::InvalidTime)?;
    // Never silently shift an entered boundary due to speed/flick rounding.
    if clip.timeline_len(clip.source_in, end) != duration {
        return Err(Error::InvalidTime.into());
    }
    Ok(end)
}

fn write_text(project: &mut Project, id: ClipId, text: Text) -> Result<(), ModelError> {
    let asset = project
        .sequence()
        .clip(id)
        .ok_or(ModelError::UnknownClip(id))?
        .1
        .asset;
    let old = project
        .asset(asset)
        .ok_or(ModelError::UnknownAsset(asset))?;
    if old.text.as_ref() == Some(&text) {
        return Ok(());
    }
    let asset = if project.clips_using(asset) > 1 || !old.generated_caption {
        let mut copy = old.clone();
        let fresh = project.add_asset(std::path::PathBuf::new());
        copy.id = fresh;
        copy.generated_caption = true;
        *project.asset_mut(fresh).expect("new asset") = copy;
        project.sequence.with_clip_mut(id, |c| *c.asset = fresh)?;
        fresh
    } else {
        asset
    };
    let asset = project.asset_mut(asset).expect("caption asset");
    if asset.generated_caption
        && asset
            .text
            .as_ref()
            .is_some_and(|t| asset.name.as_deref() == Some(&t.content.replace('\n', " ")))
    {
        asset.name = Some(text.content.replace('\n', " "));
    }
    asset.text = Some(text);
    Ok(())
}

fn valid_style(text: &Text) -> bool {
    text.size.is_finite()
        && text.size > 0.0
        && (100..=900).contains(&text.weight)
        && text.outline.is_finite()
        && text.outline >= 0.0
        && text.box_padding.is_finite()
        && text.box_padding >= 0.0
        && text
            .anchor
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        && text.shadow.iter().all(|v| v.is_finite())
        && [
            text.color,
            text.outline_color,
            text.shadow_color,
            text.box_color,
        ]
        .iter()
        .flatten()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Command, History};
    use crate::keyframe::{Easing, Keyframe, Value};
    use crate::model::NewClip;
    use crate::srt::Cue;
    use crate::time::FLICKS_PER_SECOND as S;

    fn fixture() -> (History, TrackId, Vec<ClipId>) {
        let mut h = History::new(Project::default());
        h.apply(Command::ImportSubtitles {
            name: "Captions".into(),
            cues: [(0, 2, "你好🙂 café"), (2, 4, "café two"), (5, 7, "three")]
                .into_iter()
                .map(|(a, b, text)| Cue {
                    start: Time(a * S),
                    end: Time(b * S),
                    text: text.into(),
                })
                .collect(),
            style: Text {
                size: 36.0,
                ..Text::default()
            },
        })
        .unwrap();
        let track = h.project().sequence().tracks()[0].id;
        let ids = rows(h.project()).iter().map(|r| r.clip.id).collect();
        (History::new(h.project().clone()), track, ids)
    }

    fn apply(h: &mut History, edit: Edit) {
        h.apply(Command::Caption(edit)).unwrap();
    }

    fn reopen(project: &Project) -> Project {
        let path = std::path::Path::new("/caption-project");
        let json = crate::file::to_json(project, path).unwrap();
        assert!(json.contains("\"schema_version\": 1"));
        crate::file::from_json(&json, path, |_| false).unwrap()
    }

    #[test]
    fn unicode_split_is_one_undo_and_reopens_with_srt_and_frame_boundaries() {
        let (mut h, _, ids) = fixture();
        let before = h.project().clone();
        for (at, character) in [(Time(0), 3), (Time(2 * S), 3), (Time(S), 0), (Time(S), 100)] {
            assert!(
                h.apply(Command::Caption(Edit::Split {
                    id: ids[0],
                    at,
                    character
                }))
                .is_err()
            );
            assert_eq!(h.project(), &before);
            assert!(!h.can_undo());
        }
        apply(
            &mut h,
            Edit::Split {
                id: ids[0],
                at: Time(S),
                character: 3,
            },
        );
        let r = rows(h.project());
        assert_eq!(
            (r[0].text.content.as_str(), r[1].text.content.as_str()),
            ("你好🙂", " café")
        );
        assert_eq!((r[0].clip.end(), r[1].clip.start), (Time(S), Time(S)));
        assert_ne!(r[0].clip.asset, r[1].clip.asset);
        for (at, text) in [
            (Time(S - 1), "你好🙂"),
            (Time(S), " café"),
            (Time(2 * S), "café two"),
        ] {
            let layers = crate::eval::frame_at(h.project(), at);
            assert_eq!(layers.len(), 1);
            assert_eq!(
                h.project()
                    .asset(layers[0].asset)
                    .unwrap()
                    .text
                    .as_ref()
                    .unwrap()
                    .content,
                text
            );
        }
        assert_eq!(h.project(), &reopen(h.project()));
        let srt = crate::srt::write(&h.project().subtitle_cues(), (Time(0), Time(7 * S)));
        let cues = crate::srt::parse(&srt);
        assert_eq!(cues[0].text, "你好🙂");
        assert_eq!(cues[0].end, cues[1].start);
        let after = h.project().clone();
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        assert!(!h.can_undo());
        assert!(h.redo());
        assert_eq!(h.project(), &after);
    }

    #[test]
    fn timing_rejects_overlap_and_negative_ranges_without_partial_text_changes() {
        let (mut h, _, ids) = fixture();
        let before = h.project().clone();
        for (start, end) in [(-1, S), (S, S), (0, 3 * S)] {
            assert!(
                h.apply(Command::Caption(Edit::Row {
                    id: ids[0],
                    text: "changed".into(),
                    start: Time(start),
                    end: Time(end),
                }))
                .is_err()
            );
            assert_eq!(h.project(), &before);
            assert!(!h.can_undo());
        }
        apply(
            &mut h,
            Edit::Row {
                id: ids[1],
                text: "touching".into(),
                start: Time(2 * S),
                end: Time(5 * S),
            },
        );
        assert_eq!(rows(h.project())[1].clip.end(), Time(5 * S));
        assert_eq!(h.project(), &reopen(h.project()));
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        h.apply(Command::SetClipSpeed {
            id: ids[0],
            speed: 2.0,
            reverse: true,
            ripple: false,
        })
        .unwrap();
        apply(
            &mut h,
            Edit::Row {
                id: ids[0],
                text: "fast".into(),
                start: Time(0),
                end: Time(2 * S),
            },
        );
        assert_eq!(rows(h.project())[0].clip.end(), Time(2 * S));
        assert_eq!(rows(h.project())[0].clip.speed, 2.0);
        assert!(rows(h.project())[0].clip.reverse);
    }

    #[test]
    fn merge_requires_consecutive_same_track_captions_and_preserves_first_style() {
        let (mut h, _, ids) = fixture();
        let before = h.project().clone();
        assert!(
            h.apply(Command::Caption(Edit::Merge {
                ids: vec![ids[0], ids[2]]
            }))
            .is_err()
        );
        assert_eq!(h.project(), &before);
        apply(&mut h, Edit::Merge { ids: ids.clone() });
        let r = rows(h.project());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].clip.id, ids[0]);
        assert_eq!(r[0].text.content, "你好🙂 café\ncafé two\nthree");
        assert_eq!(r[0].text.size, 36.0);
        assert_eq!(r[0].clip.end(), Time(7 * S));
        assert_eq!(h.project(), &reopen(h.project()));
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        assert!(!h.can_undo());
    }

    fn shared_title(h: &mut History, id: ClipId) -> crate::model::AssetId {
        let asset = h.project().sequence().clip(id).unwrap().1.asset;
        let Created::Track(track) = h
            .apply(Command::AddTrack {
                kind: TrackKind::Video,
                name: "Title".into(),
            })
            .unwrap()
        else {
            panic!()
        };
        h.apply(Command::AddClip {
            track,
            clip: NewClip {
                asset,
                source_in: Time(0),
                source_out: Time(S),
                start: Time(0),
                link: None,
            },
        })
        .unwrap();
        asset
    }

    #[test]
    fn literal_replace_includes_hidden_captions_detaches_titles_and_validates_all_targets() {
        let (mut h, track, ids) = fixture();
        let title = shared_title(&mut h, ids[0]);
        let mut settings = h.project().sequence().track(track).unwrap().settings();
        settings.hidden = true;
        h.apply(Command::SetTrack(settings.clone())).unwrap();
        let mut h = History::new(h.project().clone());
        apply(
            &mut h,
            Edit::Replace {
                scope: Scope::Track(track),
                find: "café".into(),
                replacement: "茶🙂".into(),
            },
        );
        assert_eq!(rows(h.project())[0].text.content, "你好🙂 茶🙂");
        assert_eq!(rows(h.project())[1].text.content, "茶🙂 two");
        assert_eq!(
            h.project()
                .asset(title)
                .unwrap()
                .text
                .as_ref()
                .unwrap()
                .content,
            "你好🙂 café"
        );
        assert!(h.project().subtitle_cues().is_empty());
        assert_eq!(h.project(), &reopen(h.project()));
        assert!(h.undo());
        assert_eq!(rows(h.project())[0].text.content, "你好🙂 café");
        assert!(!h.can_undo());
        settings.locked = true;
        h.apply(Command::SetTrack(settings)).unwrap();
        let before = h.project().clone();
        assert_eq!(
            h.apply(Command::Caption(Edit::Replace {
                scope: Scope::Sequence,
                find: "café".into(),
                replacement: "gone".into(),
            })),
            Err(ModelError::Locked(track))
        );
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn style_keep_and_replace_preserve_content_titles_and_unrelated_animation() {
        let (mut h, _, ids) = fixture();
        let title = shared_title(&mut h, ids[0]);
        let reference = rows(h.project())[0].text.clone();
        let asset = h.project().sequence().clip(ids[1]).unwrap().1.asset;
        h.apply(Command::SetText {
            id: asset,
            text: Text {
                size: 48.0,
                ..reference.clone()
            },
        })
        .unwrap();
        for name in ["text_size", "opacity"] {
            h.apply(Command::AddKeyframe {
                id: ids[1],
                param: name.into(),
                key: Keyframe {
                    time: Time(0),
                    value: Value::Scalar(0.5),
                    easing: Easing::Linear,
                },
            })
            .unwrap();
        }
        let mut h = History::new(h.project().clone());
        let style = Text {
            size: 60.0,
            outline: 3.0,
            ..reference.clone()
        };
        apply(
            &mut h,
            Edit::Style {
                scope: Scope::Sequence,
                reference: reference.clone().into(),
                style: style.clone().into(),
                overrides: Overrides::Keep,
            },
        );
        let r = rows(h.project());
        assert_eq!((r[0].text.size, r[1].text.size), (60.0, 48.0));
        assert!(r.iter().all(|r| r.text.outline == 3.0));
        assert!(r[1].clip.keyframes.contains_key("text_size"));
        assert_eq!(r[0].text.content, reference.content);
        assert_eq!(
            h.project().asset(title).unwrap().text.as_ref(),
            Some(&reference)
        );
        assert!(h.undo());
        assert!(!h.can_undo());
        apply(
            &mut h,
            Edit::Style {
                scope: Scope::Selection(vec![ids[1]]),
                reference: reference.clone().into(),
                style: style.clone().into(),
                overrides: Overrides::Replace,
            },
        );
        let r = rows(h.project());
        assert_eq!(r[1].text.size, 60.0);
        assert!(!r[1].clip.keyframes.contains_key("text_size"));
        assert!(r[1].clip.keyframes.contains_key("opacity"));
        assert_eq!(r[0].text, &reference);
        assert_eq!(h.project(), &reopen(h.project()));
        let before = h.project().clone();
        assert!(
            h.apply(Command::Caption(Edit::Style {
                scope: Scope::Sequence,
                reference: reference.into(),
                style: Text {
                    size: f32::NAN,
                    ..style
                }
                .into(),
                overrides: Overrides::Replace,
            }))
            .is_err()
        );
        assert_eq!(h.project(), &before);
    }

    #[test]
    fn stale_selection_fails_atomically_empty_find_is_noop_and_visibility_matches_export() {
        let (mut h, track, ids) = fixture();
        let before = h.project().clone();
        assert!(
            h.apply(Command::Caption(Edit::Replace {
                scope: Scope::Selection(vec![ids[0], ClipId(u64::MAX)]),
                find: "café".into(),
                replacement: "changed".into(),
            }))
            .is_err()
        );
        apply(
            &mut h,
            Edit::Replace {
                scope: Scope::Sequence,
                find: String::new(),
                replacement: "changed".into(),
            },
        );
        assert_eq!(h.project(), &before);
        assert!(!h.can_undo());
        h.apply(Command::SetClipsEnabled {
            ids: vec![ids[0]],
            enabled: false,
        })
        .unwrap();
        assert_eq!(rows(h.project()).len(), 3);
        assert_eq!(h.project().subtitle_cues().len(), 2);
        assert!(crate::eval::frame_at(h.project(), Time(0)).is_empty());
        let mut settings = h.project().sequence().track(track).unwrap().settings();
        settings.hidden = true;
        h.apply(Command::SetTrack(settings)).unwrap();
        assert_eq!(rows(h.project()).len(), 3);
        assert!(h.project().subtitle_cues().is_empty());
        assert!(crate::eval::frame_at(h.project(), Time(3 * S)).is_empty());
    }

    #[test]
    fn legacy_overlaps_split_only_new_tail_and_merge_covers_maximum_end() {
        let (mut h, track, ids) = fixture();
        let title = shared_title(&mut h, ids[0]);
        // Legacy JSON can contain overlaps despite normal timeline placement validation.
        let mut value = serde_json::to_value(h.project()).unwrap();
        let clips = value["sequence"]["tracks"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|t| t["id"] == serde_json::to_value(track).unwrap())
            .unwrap()["clips"]
            .as_array_mut()
            .unwrap();
        clips[0]["source_out"] = serde_json::to_value(Time(10 * S)).unwrap();
        clips[1]["start"] = serde_json::to_value(Time(S)).unwrap();
        let mut h = History::new(serde_json::from_value(value).unwrap());
        let before = h.project().clone();
        let untouched = rows(h.project())
            .iter()
            .find(|r| r.clip.id == ids[1])
            .unwrap()
            .clip
            .clone();
        apply(
            &mut h,
            Edit::Split {
                id: ids[0],
                at: Time(S),
                character: 3,
            },
        );
        let rows = rows(h.project());
        let tail = rows.iter().find(|r| !ids.contains(&r.clip.id)).unwrap();
        assert_eq!(tail.text.content, " café");
        assert_eq!(
            rows.iter().find(|r| r.clip.id == ids[1]).unwrap().clip,
            &untouched
        );
        assert_eq!(
            rows.iter()
                .find(|r| r.clip.id == ids[1])
                .unwrap()
                .text
                .content,
            "café two"
        );
        assert_eq!(
            h.project()
                .asset(title)
                .unwrap()
                .text
                .as_ref()
                .unwrap()
                .content,
            "你好🙂 café"
        );
        assert_eq!(h.project(), &reopen(h.project()));
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        apply(&mut h, Edit::Merge { ids: ids.clone() });
        let merged = super::rows(h.project());
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].clip.end(), Time(10 * S));
        assert_eq!(h.project(), &reopen(h.project()));
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        assert!(!h.can_undo());
    }

    #[test]
    fn kept_animation_preserves_static_fallback_and_frozen_caption_timing_is_exact() {
        let (mut h, _, ids) = fixture();
        h.apply(Command::AddKeyframe {
            id: ids[0],
            param: "text_size".into(),
            key: Keyframe {
                time: Time(0),
                value: Value::Scalar(80.0),
                easing: Easing::Linear,
            },
        })
        .unwrap();
        let reference = rows(h.project())[0].text.clone();
        apply(
            &mut h,
            Edit::Style {
                scope: Scope::Sequence,
                reference: reference.clone().into(),
                style: Text {
                    size: 60.0,
                    ..reference
                }
                .into(),
                overrides: Overrides::Keep,
            },
        );
        assert_eq!(rows(h.project())[0].text.size, 36.0);
        assert_eq!(rows(h.project())[1].text.size, 60.0);
        assert_eq!(
            crate::eval::frame_at(h.project(), Time(0))[0].text[0].1,
            Value::Scalar(80.0)
        );
        h.apply(Command::FreezeFrame {
            id: ids[2],
            at: Time(6 * S),
        })
        .unwrap();
        let frozen_id = rows(h.project())
            .iter()
            .find(|r| r.clip.freeze.is_some())
            .unwrap()
            .clip
            .id;
        let frozen = h.project().sequence().clip(frozen_id).unwrap().1.freeze;
        apply(
            &mut h,
            Edit::Row {
                id: frozen_id,
                text: "frozen".into(),
                start: Time(8 * S),
                end: Time(12 * S),
            },
        );
        let (_, clip) = h.project().sequence().clip(frozen_id).unwrap();
        assert_eq!(
            (clip.start, clip.end(), clip.freeze),
            (Time(8 * S), Time(12 * S), frozen)
        );
        assert_eq!(h.project(), &reopen(h.project()));
    }

    #[test]
    fn reusable_unplaced_title_assets_detach_and_ramped_timing_refuses_atomically() {
        let (h, _, ids) = fixture();
        let asset = h.project().sequence().clip(ids[0]).unwrap().1.asset;
        let mut project = h.project().clone();
        project.asset_mut(asset).unwrap().generated_caption = false;
        let title = project.asset(asset).unwrap().clone();
        let mut h = History::new(project);
        apply(
            &mut h,
            Edit::Row {
                id: ids[0],
                text: "caption only".into(),
                start: Time(0),
                end: Time(2 * S),
            },
        );
        assert_eq!(h.project().asset(asset), Some(&title));
        assert_ne!(h.project().sequence().clip(ids[0]).unwrap().1.asset, asset);
        assert_eq!(h.project(), &reopen(h.project()));
        h.apply(Command::SetSpeedKeys {
            id: ids[0],
            speed: 1.0,
            keys: vec![Keyframe {
                time: Time(0),
                value: Value::Scalar(1.0),
                easing: Easing::Linear,
            }],
            ripple: false,
        })
        .unwrap();
        let before = h.project().clone();
        assert_eq!(
            h.apply(Command::Caption(Edit::Row {
                id: ids[0],
                text: "not committed".into(),
                start: Time(8 * S),
                end: Time(11 * S),
            })),
            Err(ModelError::Caption(Error::AnimatedTiming))
        );
        assert_eq!(h.project(), &before);
        apply(
            &mut h,
            Edit::Row {
                id: ids[0],
                text: "moved ramp".into(),
                start: Time(8 * S),
                end: Time(10 * S),
            },
        );
        assert_eq!(
            h.project().sequence().clip(ids[0]).unwrap().1.end(),
            Time(10 * S)
        );
        assert_eq!(h.project(), &reopen(h.project()));
    }
}
