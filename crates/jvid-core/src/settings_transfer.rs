//! Selective, replace-only clip settings transfer.
//!
//! Contract for consumers: [`Settings::extract`] snapshots the requested categories at a
//! timeline playhead. [`Settings::command`] validates **all** targets and resources before
//! returning one atomic [`Command::Batch`]. [`Settings::validate`] uses the same rules.
//! Static is the default: sampled values replace the selected properties and remove their
//! old animation. Unselected properties, speed keys, source identity, links, words, track,
//! placement, trims, direction and duration are never copied.
//!
//! Animation is opt-in. Source key times become clip-relative timeline offsets through
//! `Clip::timeline_at`; target offsets become source coordinates through `Clip::source_at`.
//! Source keys outside its visible trim are discarded. Keys at or beyond the target's
//! exclusive end are discarded (never stretched or piled up at the end). Sampled keys at
//! offset zero and the last visible flick preserve trimmed/overflow boundary values. Longer
//! targets hold the source's last visible value. Source-coordinate interpolation remains
//! source-coordinate interpolation: different speed ramps can change the curve between keys.
//! Reversed clips reverse easing; a frozen destination rejects varying animation because
//! its single source coordinate cannot represent it. Quantized collisions keep the later
//! playback offset. Neither clip's speed animation is replaced.
//!
//! Transform owns geometry, crop, opacity, blend and masks. Grade owns `GRADE_KEYS` and LUT.
//! Effects owns the ordered effect stack, parameters and bypass flags; masked effects require
//! Transform too, so mask IDs retain their meaning. Audio owns volume, pan and audio effects.
//! Fades and transitions stay untouched. Text owns style and fill, never content; because text
//! style is asset-wide, every use of a target text asset must be selected, including uses in
//! other sequences (which cannot be selected here and therefore reject the operation).
//! Callers supply effect/LUT availability; core performs no media, UI or filesystem I/O.

use std::collections::{BTreeMap, BTreeSet};

use crate::history::Command;
use crate::keyframe::{self, Easing, Keyframe, Value};
use crate::model::{AssetId, Clip, ClipId, ModelError, Project, Text, TrackKind, Transform};
use crate::time::Time;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Transform,
    Grade,
    Effects,
    Audio,
    Text,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Animation {
    #[default]
    Static,
    /// Map relative key times; discard overflow and sample visible boundaries.
    Relative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource<'a> {
    Effect(&'a str),
    Lut(&'a str),
}

/// Typed failures; UI resolves these to translated messages at display time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Model(ModelError),
    NoCategories,
    NoTargets,
    Incompatible { clip: ClipId, category: Category },
    UnavailableEffect(String),
    UnavailableLut(String),
    MaskNeedsTransform,
    SharedText(AssetId),
    FrozenAnimation(ClipId),
}

impl From<ModelError> for Error {
    fn from(value: ModelError) -> Self {
        Self::Model(value)
    }
}

/// Opaque snapshot: consumers cannot accidentally apply identity/timing with the settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    categories: BTreeSet<Category>,
    animation: Animation,
    source: Clip,
    sampled: Transform,
    text: Option<Text>,
}

impl Settings {
    /// Categories supported by a clip's kind. Copy remains available on locked tracks.
    pub fn supported(project: &Project, source: ClipId) -> Result<Vec<Category>, Error> {
        project
            .sequence()
            .clip(source)
            .ok_or(ModelError::UnknownClip(source))?;
        Ok([
            Category::Transform,
            Category::Grade,
            Category::Effects,
            Category::Audio,
            Category::Text,
        ]
        .into_iter()
        .filter(|category| compatible(project, source, &BTreeSet::from([*category]), false).is_ok())
        .collect())
    }

    /// Narrow a snapshot for a paste dialog; uncaptured categories are rejected.
    pub fn select(&self, categories: &[Category]) -> Result<Self, Error> {
        if categories.is_empty() {
            return Err(Error::NoCategories);
        }
        let mut selected = self.clone();
        selected.categories = categories.iter().copied().collect();
        if let Some(category) = selected.categories.difference(&self.categories).next() {
            return Err(Error::Incompatible {
                clip: self.source.id,
                category: *category,
            });
        }
        Ok(selected)
    }
    pub fn extract(
        project: &Project,
        source: ClipId,
        categories: &[Category],
        at: Time,
        animation: Animation,
    ) -> Result<Self, Error> {
        let categories: BTreeSet<_> = categories.iter().copied().collect();
        if categories.is_empty() {
            return Err(Error::NoCategories);
        }
        let source = compatible(project, source, &categories, false)?.clone();
        let sampled = match animation {
            Animation::Static => source.transform_at(at),
            Animation::Relative => source.transform.clone(),
        };
        let text = project
            .asset(source.asset)
            .and_then(|a| a.text.as_ref())
            .map(|t| {
                let mut text = match animation {
                    Animation::Static => t.with(&source.text_keys(at)),
                    Animation::Relative => t.clone(),
                };
                if animation == Animation::Static
                    && let Some(Value::Color(color)) = source.value_at("color", at)
                {
                    text.color = color;
                }
                text.content.clear();
                text
            });
        Ok(Self {
            categories,
            animation,
            source,
            sampled,
            text,
        })
    }

    pub fn categories(&self) -> impl Iterator<Item = Category> + '_ {
        self.categories.iter().copied()
    }

    pub fn animation(&self) -> Animation {
        self.animation
    }

    /// The same preflight used by [`Self::command`]; no mutation or history step.
    pub fn validate(
        &self,
        project: &Project,
        targets: &[ClipId],
        available: impl Fn(Resource<'_>) -> bool,
    ) -> Result<(), Error> {
        self.command(project, targets, available).map(|_| ())
    }

    /// Validate the entire batch, then build one undoable command using existing history.
    /// Targets are deduplicated; linked partners are never implicitly added.
    pub fn command(
        &self,
        project: &Project,
        targets: &[ClipId],
        available: impl Fn(Resource<'_>) -> bool,
    ) -> Result<Command, Error> {
        let targets: BTreeSet<_> = targets.iter().copied().collect();
        if targets.is_empty() {
            return Err(Error::NoTargets);
        }
        if self.categories.contains(&Category::Effects) {
            for effect in &self.sampled.effects {
                if !available(Resource::Effect(&effect.shader)) {
                    return Err(Error::UnavailableEffect(effect.shader.clone()));
                }
                if effect.mask.is_some() && !self.categories.contains(&Category::Transform) {
                    return Err(Error::MaskNeedsTransform);
                }
            }
        }
        if self.categories.contains(&Category::Grade)
            && let Some(lut) = &self.sampled.lut
            && !available(Resource::Lut(lut))
        {
            return Err(Error::UnavailableLut(lut.clone()));
        }
        let mut edits = Vec::new();
        let mut texts = BTreeMap::new();
        for &id in &targets {
            let clip = compatible(project, id, &self.categories, true)?;
            if self.categories.contains(&Category::Text) {
                let all_selected = project.sequences().iter().all(|sequence| {
                    sequence.tracks().iter().all(|track| {
                        track.clips().iter().all(|c| {
                            c.asset != clip.asset
                                || (sequence.id() == project.sequence().id()
                                    && targets.contains(&c.id))
                        })
                    })
                });
                if !all_selected {
                    return Err(Error::SharedText(clip.asset));
                }
                let base = project
                    .asset(clip.asset)
                    .and_then(|a| a.text.as_ref())
                    .unwrap();
                let mut style = self.text.clone().expect("validated text source");
                style.content.clone_from(&base.content);
                texts.insert(clip.asset, style);
            }
            let mut transform = clip.transform.clone();
            replace(&mut transform, &self.sampled, &self.categories);
            let mut keys = clip.keyframes.clone();
            keys.retain(|name, _| !owns(&self.categories, name));
            if self.animation == Animation::Relative {
                for (name, source_keys) in &self.source.keyframes {
                    if !owns(&self.categories, name) || source_keys.is_empty() {
                        continue;
                    }
                    let mapped = map_keys(&self.source, clip, source_keys)?;
                    keys.insert(name.clone(), mapped);
                }
            }
            edits.push((id, transform, keys));
        }
        let mut commands = vec![Command::SetClipProps(edits)];
        commands.extend(
            texts
                .into_iter()
                .map(|(id, text)| Command::SetText { id, text }),
        );
        Ok(Command::Batch(commands))
    }
}

fn compatible<'a>(
    project: &'a Project,
    id: ClipId,
    categories: &BTreeSet<Category>,
    editing: bool,
) -> Result<&'a Clip, Error> {
    let sequence = project.sequence();
    let (track, clip) = sequence.clip(id).ok_or(ModelError::UnknownClip(id))?;
    let track = sequence
        .track(track)
        .ok_or(ModelError::UnknownTrack(track))?;
    if clip.duration() <= Time(0) {
        return Err(ModelError::EmptyClip.into());
    }
    if editing && track.locked {
        return Err(ModelError::Locked(track.id).into());
    }
    for &category in categories {
        let valid = match category {
            Category::Audio => track.kind == TrackKind::Audio,
            Category::Text => {
                track.kind == TrackKind::Video
                    && project.asset(clip.asset).is_some_and(|a| a.text.is_some())
            }
            _ => track.kind == TrackKind::Video,
        };
        if !valid {
            return Err(Error::Incompatible { clip: id, category });
        }
    }
    Ok(clip)
}

const GEOMETRY: [&str; 9] = [
    "position",
    "anchor",
    "scale",
    "rotation",
    "opacity",
    "crop_left",
    "crop_top",
    "crop_right",
    "crop_bottom",
];

fn owns(categories: &BTreeSet<Category>, name: &str) -> bool {
    categories.iter().any(|category| match category {
        Category::Transform => GEOMETRY.contains(&name) || name.starts_with("mask"),
        Category::Grade => Transform::GRADE_KEYS.contains(&name),
        Category::Effects => name.starts_with("fx"),
        Category::Audio => matches!(name, "volume" | "pan"),
        Category::Text => Text::ANIMATED.contains(&name) || name == "color",
    })
}

fn replace(to: &mut Transform, from: &Transform, categories: &BTreeSet<Category>) {
    for category in categories {
        match category {
            Category::Transform => {
                to.position = from.position;
                to.anchor = from.anchor;
                to.scale = from.scale;
                to.rotation = from.rotation;
                to.crop = from.crop;
                to.opacity = from.opacity;
                to.blend = from.blend;
                to.masks.clone_from(&from.masks);
            }
            Category::Grade => to.paste_grade(from),
            Category::Effects => to.effects.clone_from(&from.effects),
            Category::Audio => {
                to.volume = from.volume;
                to.pan = from.pan;
                to.audio_fx = from.audio_fx;
            }
            Category::Text => {}
        }
    }
}

fn reversed(easing: Easing) -> Easing {
    match easing {
        Easing::EaseIn => Easing::EaseOut,
        Easing::EaseOut => Easing::EaseIn,
        Easing::Bezier([x1, y1, x2, y2]) => {
            Easing::Bezier([1.0 - x2, 1.0 - y2, 1.0 - x1, 1.0 - y1])
        }
        other => other,
    }
}

fn map_keys(source: &Clip, target: &Clip, keys: &[Keyframe]) -> Result<Vec<Keyframe>, Error> {
    let last = Time(target.duration().0 - 1);
    let source_last = Time(source.duration().0 - 1);
    let mut offsets = BTreeSet::from([Time(0), last, source_last.min(last)]);
    let mut original_times = BTreeMap::new();
    if source.freeze.is_none() {
        let mut times: Vec<_> = keys.iter().map(|k| k.time).collect();
        // Reversing a Hold needs the value immediately before its discontinuity too.
        if source.reverse || target.reverse {
            times.extend(
                keys.windows(2)
                    .filter(|pair| pair[0].easing == Easing::Hold)
                    .map(|pair| Time(pair[1].time.0.saturating_sub(1))),
            );
        }
        times.sort();
        if source.reverse {
            times.reverse();
        }
        for time in times {
            if time < source.source_in || time >= source.source_out {
                continue;
            }
            let offset = source.timeline_at(time) - source.start;
            if offset >= Time(0) && offset <= last && offset <= source_last {
                offsets.insert(offset);
                original_times.insert(offset, time);
            }
        }
    }
    let mut playback = Vec::new();
    for offset in offsets {
        // Timeline flick rounding must not change an explicitly keyed value.
        let at = original_times
            .get(&offset)
            .copied()
            .unwrap_or_else(|| source.source_at(source.start + offset.min(source_last)));
        let index = keys.partition_point(|k| k.time <= at);
        let index = if source.reverse {
            keys.partition_point(|k| k.time < at)
        } else {
            index
        };
        let easing = index
            .checked_sub(1)
            .map_or(Easing::Linear, |i| keys[i].easing);
        playback.push(Keyframe {
            time: target.source_at(target.start + offset),
            value: keyframe::value_at(keys, at).expect("nonempty animation"),
            easing: if source.reverse {
                reversed(easing)
            } else {
                easing
            },
        });
    }
    if target.freeze.is_some() && playback.iter().any(|k| k.value != playback[0].value) {
        return Err(Error::FrozenAnimation(target.id));
    }
    if target.reverse {
        for i in (1..playback.len()).rev() {
            playback[i].easing = reversed(playback[i - 1].easing);
        }
    }
    let mut mapped = Vec::new();
    for key in playback {
        keyframe::insert(&mut mapped, key);
    }
    Ok(mapped)
}

