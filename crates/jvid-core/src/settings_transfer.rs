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
//! source-coordinate interpolation. Boundary samples restart the original segment easing;
//! they do not subdivide Bezier handles. Trimmed curves and different speed ramps can change
//! interpolation between reconstructed keys; mapped key values/boundaries are guaranteed,
//! not an identical continuous time curve. Reverse Hold jumps include a probe one source
//! flick before the discontinuity, subject to the same time quantization.
//! Reversed clips reverse easing; a frozen destination rejects varying animation because
//! its single source coordinate cannot represent it. Quantized collisions keep the later
//! playback offset. Neither clip's speed animation is replaced.
//!
//! Transform owns geometry, crop, opacity, blend and masks. Grade owns `GRADE_KEYS` and LUT.
//! Effects owns the ordered effect stack, parameters and bypass flags; masked effects require
//! Transform too, so mask IDs retain their meaning. Audio owns volume, pan and audio effects.
//! Transform without Effects rejects targets whose effects refer to masks, so unselected
//! effects cannot be orphaned or silently rebound by a mask replacement.
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
    MaskedTarget(ClipId),
    InvalidEffectMask,
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
                if let Some(id) = effect.mask
                    && !self.sampled.masks.iter().any(|mask| mask.id == id)
                {
                    return Err(Error::InvalidEffectMask);
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
            if self.categories.contains(&Category::Transform)
                && !self.categories.contains(&Category::Effects)
                && clip
                    .transform
                    .effects
                    .iter()
                    .any(|effect| effect.mask.is_some())
            {
                return Err(Error::MaskedTarget(id));
            }
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

/// Portable-style failures are separate from transfer errors for existing consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetError {
    Invalid,
    UnsupportedCategory,
    Transfer(Error),
}

impl From<Error> for PresetError {
    fn from(error: Error) -> Self {
        Self::Transfer(error)
    }
}

impl crate::personal_presets::Preset {
    /// Capture one supported style. Only selected properties leave the source project.
    pub fn capture(
        project: &Project,
        source: ClipId,
        category: Category,
        at: Time,
        animation: Animation,
    ) -> Result<Self, PresetError> {
        use crate::personal_presets::Style;
        let snapshot = Settings::extract(project, source, &[category], at, Animation::Static)?;
        let sampled = &snapshot.sampled;
        let style = match category {
            Category::Text => Style::Text(snapshot.text.clone().ok_or(PresetError::Invalid)?),
            Category::Audio => Style::Audio {
                volume: sampled.volume,
                pan: sampled.pan,
                fx: sampled.audio_fx,
            },
            Category::Effects => {
                if sampled.effects.iter().any(|e| e.mask.is_some()) {
                    return Err(Error::MaskNeedsTransform.into());
                }
                Style::Effects {
                    effects: sampled.effects.clone(),
                }
            }
            _ => return Err(PresetError::UnsupportedCategory),
        };
        let mut preset = Self {
            style,
            animation_duration: None,
            keyframes: BTreeMap::new(),
        };
        if animation == Animation::Relative {
            let source = &snapshot.source;
            let duration = source.duration();
            let normalized = preset_source(duration);
            for (name, keys) in &source.keyframes {
                if owns(&snapshot.categories, name) && !keys.is_empty() {
                    preset
                        .keyframes
                        .insert(name.clone(), map_keys(source, &normalized, keys)?);
                }
            }
            if !preset.keyframes.is_empty() {
                preset.animation_duration = Some(duration);
            }
        }
        preset.validate_definition()?;
        Ok(preset)
    }

    /// Validate untrusted disk data before constructing a transfer snapshot. No resources
    /// are resolved here: a missing font or shader must not destroy a stored definition.
    pub fn validate_definition(&self) -> Result<(), PresetError> {
        use crate::personal_presets::Style;
        let invalid = || PresetError::Invalid;
        fn finite(value: &serde_json::Value) -> bool {
            match value {
                serde_json::Value::Null => false,
                serde_json::Value::Array(values) => values.iter().all(finite),
                serde_json::Value::Object(values) => values.values().all(finite),
                _ => true,
            }
        }
        if !finite(&serde_json::to_value(&self.style).map_err(|_| invalid())?)
            || !finite(&serde_json::to_value(&self.keyframes).map_err(|_| invalid())?)
        {
            return Err(invalid());
        }
        match &self.style {
            Style::Text(text) => {
                if !text.content.is_empty()
                    || text.size < 1.0
                    || text.size > 100_000.0
                    || !(100..=900).contains(&text.weight)
                    || text.family.len() > 512
                    || text.outline < 0.0
                    || text.box_padding < 0.0
                {
                    return Err(invalid());
                }
            }
            Style::Audio { volume, pan, .. } => {
                if !(0.0..=2.0).contains(volume) || !(-1.0..=1.0).contains(pan) {
                    return Err(invalid());
                }
            }
            Style::Effects { effects } => {
                for effect in effects {
                    if effect.mask.is_some() {
                        return Err(Error::MaskNeedsTransform.into());
                    }
                    if effect.shader.is_empty()
                        || effect.shader.len() > 16_384
                        || effect.shader.chars().any(char::is_control)
                    {
                        return Err(invalid());
                    }
                }
            }
        }
        let duration = match (self.animation_duration, self.keyframes.is_empty()) {
            (None, true) => return Ok(()),
            (Some(duration), false) if duration > Time(0) => duration,
            _ => return Err(invalid()),
        };
        for (name, keys) in &self.keyframes {
            let expected = match &self.style {
                Style::Text(text) => {
                    if name == "color" {
                        Some(Value::Color(text.color))
                    } else {
                        text.get(name)
                    }
                }
                Style::Audio { .. } => {
                    matches!(name.as_str(), "volume" | "pan").then_some(Value::Scalar(0.0))
                }
                Style::Effects { effects } => {
                    effects.iter().enumerate().find_map(|(index, effect)| {
                        effect
                            .params
                            .keys()
                            .any(|param| crate::model::Effect::key(index, param) == *name)
                            .then_some(Value::Scalar(0.0))
                    })
                }
            }
            .ok_or_else(invalid)?;
            if keys.is_empty()
                || keys.windows(2).any(|pair| pair[0].time >= pair[1].time)
                || keys.iter().any(|key| {
                    key.time < Time(0)
                        || key.time >= duration
                        || std::mem::discriminant(&key.value) != std::mem::discriminant(&expected)
                })
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    /// Reconstruct an identity-free source and delegate every target/resource check and
    /// undo command to P1. Static remains the caller's default, even for animated presets.
    pub fn settings(&self, animation: Animation) -> Result<Settings, PresetError> {
        use crate::personal_presets::Style;
        self.validate_definition()?;
        let mut source = preset_source(self.animation_duration.unwrap_or(Time(1)));
        let mut sampled = Transform::default();
        let text = match &self.style {
            Style::Text(text) => Some(text.clone()),
            Style::Audio { volume, pan, fx } => {
                sampled.volume = *volume;
                sampled.pan = *pan;
                sampled.audio_fx = *fx;
                None
            }
            Style::Effects { effects } => {
                sampled.effects.clone_from(effects);
                None
            }
        };
        source.transform = sampled.clone();
        if animation == Animation::Relative {
            source.keyframes.clone_from(&self.keyframes);
        }
        Ok(Settings {
            categories: BTreeSet::from([self.style.category()]),
            animation,
            source,
            sampled,
            text,
        })
    }
}

// No original identity, timing, speed, links or params survive normalization.
fn preset_source(duration: Time) -> Clip {
    Clip {
        id: ClipId(0),
        asset: AssetId(0),
        source_in: Time(0),
        source_out: duration,
        start: Time(0),
        link: None,
        params: BTreeMap::new(),
        keyframes: BTreeMap::new(),
        transform: Transform::default(),
        stream: None,
        enabled: true,
        speed: 1.0,
        reverse: false,
        freeze: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::History;
    use crate::model::{Effect, LinkId, NewClip, ProbeInfo, TrackId};

    fn clip(p: &mut Project, track: TrackId, start: i64, from: i64, to: i64) -> ClipId {
        let asset = p.add_asset(format!("clip-{start}.mkv"));
        p.asset_mut(asset).unwrap().probe = Some(ProbeInfo {
            duration: Time(10000),
            ..Default::default()
        });
        p.sequence
            .add_clip(
                track,
                NewClip {
                    asset,
                    start: Time(start),
                    source_in: Time(from),
                    source_out: Time(to),
                    link: Some(LinkId(start as u64)),
                },
            )
            .unwrap()
    }

    fn key(time: i64, value: f64) -> Keyframe {
        Keyframe {
            time: Time(time),
            value: Value::Scalar(value),
            easing: Easing::Linear,
        }
    }

    fn extract(p: &Project, source: ClipId, categories: &[Category], mode: Animation) -> Settings {
        Settings::extract(p, source, categories, Time(50), mode).unwrap()
    }

    #[test]
    fn personal_presets_round_trip_without_identity_or_words() {
        use crate::personal_presets::{Preset, Style};
        let mut p = Project::default();
        let video = p.sequence.add_track(TrackKind::Video, "V1");
        let source = clip(&mut p, video, 1234, 100, 300);
        let asset = p.sequence.clip(source).unwrap().1.asset;
        p.asset_mut(asset).unwrap().text = Some(Text {
            content: "private source words".into(),
            family: "Missing Family".into(),
            size: 64.0,
            ..Text::default()
        });
        let preset =
            Preset::capture(&p, source, Category::Text, Time(1234), Animation::Static).unwrap();
        let encoded = toml::to_string(&preset).unwrap();
        for excluded in [
            "private source words",
            "content",
            "asset",
            "source_in",
            "link",
            "start",
            "speed",
            "volume",
            "exposure",
        ] {
            assert!(!encoded.contains(excluded), "{excluded}: {encoded}");
        }
        let loaded: Preset = toml::from_str(&encoded).unwrap();
        assert_eq!(loaded, preset);
        let mut other = Project::default();
        let track = other.sequence.add_track(TrackKind::Video, "Destination");
        let target = clip(&mut other, track, 500, 1000, 1200);
        let target_asset = other.sequence.clip(target).unwrap().1.asset;
        other.asset_mut(target_asset).unwrap().text = Some(Text {
            content: "destination words".into(),
            ..Text::default()
        });
        let mut h = History::new(other);
        let before = h.project().clone();
        let command = loaded
            .settings(Animation::Static)
            .unwrap()
            .command(h.project(), &[target], |_| true)
            .unwrap();
        h.apply(command).unwrap();
        let text = h
            .project()
            .asset(target_asset)
            .unwrap()
            .text
            .as_ref()
            .unwrap();
        assert_eq!(text.content, "destination words");
        assert_eq!(text.size, 64.0);
        assert_eq!(text.family, "Missing Family");
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        let poisoned = encoded.replace("family =", "content = 'injected words'\nfamily =");
        assert!(toml::from_str::<Preset>(&poisoned).is_err());
        let mut invalid = preset;
        if let Style::Text(text) = &mut invalid.style {
            text.size = f32::NAN;
        }
        assert_eq!(invalid.validate_definition(), Err(PresetError::Invalid));
    }

    #[test]
    fn personal_presets_normalize_animation_and_reuse_overflow_policy() {
        use crate::personal_presets::Preset;
        let mut p = Project::default();
        let audio = p.sequence.add_track(TrackKind::Audio, "A1");
        let source = clip(&mut p, audio, 700, 100, 300);
        let target = clip(&mut p, audio, 2000, 1000, 1100);
        p.sequence.set_speed(source, 2.0, false, false).unwrap();
        p.sequence
            .with_clip_mut(source, |c| {
                c.keyframes.insert(
                    "volume".into(),
                    vec![key(100, 0.2), key(200, 1.0), key(299, 0.5)],
                );
                c.transform.pan = -0.25;
            })
            .unwrap();
        p.sequence.set_speed(target, 2.0, false, false).unwrap();
        let preset =
            Preset::capture(&p, source, Category::Audio, Time(725), Animation::Relative).unwrap();
        let encoded = toml::to_string(&preset).unwrap();
        let loaded: Preset = toml::from_str(&encoded).unwrap();
        assert_eq!(loaded.animation_duration, Some(Time(100)));
        assert!(
            loaded.keyframes["volume"]
                .iter()
                .any(|k| k.time == Time(50) && k.value == Value::Scalar(1.0))
        );
        let before = p.sequence.clip(target).unwrap().1.clone();
        let mut h = History::new(p);
        h.apply(
            loaded
                .settings(Animation::Relative)
                .unwrap()
                .command(h.project(), &[target], |_| true)
                .unwrap(),
        )
        .unwrap();
        let current = h.project().sequence().clip(target).unwrap().1;
        assert_eq!(
            (
                current.speed,
                current.source_in,
                current.source_out,
                current.start,
                current.link
            ),
            (
                before.speed,
                before.source_in,
                before.source_out,
                before.start,
                before.link
            )
        );
        assert!(
            current.keyframes["volume"]
                .iter()
                .all(|k| k.time >= Time(1000) && k.time < Time(1100))
        );
        assert_eq!(current.transform_at(Time(2000)).volume, 0.2);
        assert!(h.undo());
        h.apply(
            loaded
                .settings(Animation::Static)
                .unwrap()
                .command(h.project(), &[target], |_| true)
                .unwrap(),
        )
        .unwrap();
        assert!(
            !h.project()
                .sequence()
                .clip(target)
                .unwrap()
                .1
                .keyframes
                .contains_key("volume")
        );
        let mut invalid = loaded;
        invalid.keyframes.insert("speed".into(), vec![key(0, 2.0)]);
        assert_eq!(invalid.validate_definition(), Err(PresetError::Invalid));
    }

    #[test]
    fn static_replace_is_selective_atomic_undoable_and_survives_reopen() {
        let mut p = Project::default();
        let video = p.sequence.add_track(TrackKind::Video, "V1");
        let audio = p.sequence.add_track(TrackKind::Audio, "A1");
        let source = clip(&mut p, video, 0, 100, 300);
        let a = clip(&mut p, video, 500, 1000, 1200);
        let b = clip(&mut p, video, 1000, 3000, 3100);
        let incompatible = clip(&mut p, audio, 0, 0, 200);
        p.sequence
            .with_clip_mut(source, |c| {
                c.transform.exposure = 2.0;
                c.transform.position = [23.0, 45.0];
                c.keyframes
                    .insert("exposure".into(), vec![key(100, 1.0), key(300, 3.0)]);
            })
            .unwrap();
        for id in [a, b] {
            p.sequence
                .with_clip_mut(id, |c| {
                    c.transform.position = [9.0, 8.0];
                    c.transform.volume = 0.25;
                    c.keyframes.insert("exposure".into(), vec![key(0, -1.0)]);
                    c.keyframes.insert("rotation".into(), vec![key(0, 30.0)]);
                })
                .unwrap();
        }
        let settings = extract(&p, source, &[Category::Grade], Animation::default());
        let before = p.clone();
        assert!(matches!(
            settings.command(&p, &[a, incompatible], |_| true),
            Err(Error::Incompatible { .. })
        ));
        assert_eq!(p, before);
        let mut history = History::new(p);
        history
            .apply(
                settings
                    .command(history.project(), &[a, b, a], |_| true)
                    .unwrap(),
            )
            .unwrap();
        for id in [a, b] {
            let old = before.sequence().clip(id).unwrap().1;
            let current = history.project().sequence().clip(id).unwrap().1;
            let mut expected = old.clone();
            expected.transform.exposure = 1.5;
            expected.keyframes.remove("exposure");
            assert_eq!(
                current, &expected,
                "identity, links, timing, other categories survive"
            );
        }
        let edited = history.project().clone();
        let dir = std::path::Path::new("/project");
        let json = crate::file::to_json(&edited, dir).unwrap();
        let reopened = crate::file::from_json(&json, dir, |_| false).unwrap();
        assert_eq!(reopened, edited);
        assert_eq!(
            crate::eval::frame_at(&reopened, Time(510)),
            crate::eval::frame_at(&edited, Time(510))
        );
        assert!(history.undo());
        assert_eq!(history.project(), &before);
        assert!(!history.can_undo(), "one batch, one undo");
        assert!(history.redo());
        assert_eq!(history.project(), &edited);
    }

    #[test]
    fn relative_animation_maps_trim_and_speed_drops_overflow_and_samples_end() {
        let mut p = Project::default();
        let v = p.sequence.add_track(TrackKind::Video, "V1");
        let source = clip(&mut p, v, 0, 100, 300);
        let target = clip(&mut p, v, 500, 1000, 1120);
        p.sequence.set_speed(source, 2.0, false, false).unwrap();
        p.sequence.set_speed(target, 2.0, false, false).unwrap();
        p.sequence
            .with_clip_mut(source, |c| {
                c.keyframes.insert(
                    "rotation".into(),
                    vec![
                        key(0, -10.0),
                        key(100, 0.0),
                        key(200, 10.0),
                        key(280, 20.0),
                        key(400, 30.0),
                    ],
                );
            })
            .unwrap();
        let settings = extract(&p, source, &[Category::Transform], Animation::Relative);
        let mut h = History::new(p);
        h.apply(settings.command(h.project(), &[target], |_| true).unwrap())
            .unwrap();
        let c = h.project().sequence().clip(target).unwrap().1;
        assert_eq!(c.duration(), Time(60));
        let keys = &c.keyframes["rotation"];
        assert_eq!(
            keys.iter().map(|k| k.time).collect::<Vec<_>>(),
            [Time(1000), Time(1100), Time(1118)]
        );
        assert_eq!(c.transform_at(Time(550)).rotation, 10.0);
        assert_eq!(c.transform_at(Time(559)).rotation, 12.25);
        assert!(keys.iter().all(|k| k.time < c.source_out));
    }

    #[test]
    fn resources_and_locks_validate_before_any_target_changes() {
        let mut p = Project::default();
        let v = p.sequence.add_track(TrackKind::Video, "V1");
        let source = clip(&mut p, v, 0, 0, 100);
        let target = clip(&mut p, v, 500, 0, 100);
        p.sequence
            .with_clip_mut(source, |c| {
                c.transform.effects.push(Effect {
                    shader: "missing".into(),
                    params: BTreeMap::new(),
                    enabled: false,
                    mask: None,
                });
                c.transform.lut = Some("missing.cube".into());
            })
            .unwrap();
        let fx = extract(&p, source, &[Category::Effects], Animation::Static);
        assert_eq!(
            fx.command(&p, &[target], |_| false),
            Err(Error::UnavailableEffect("missing".into()))
        );
        let grade = extract(&p, source, &[Category::Grade], Animation::Static);
        assert_eq!(
            grade.command(&p, &[target], |_| false),
            Err(Error::UnavailableLut("missing.cube".into()))
        );
        let mut track = p.sequence.track(v).unwrap().settings();
        track.locked = true;
        p.sequence.set_track(track).unwrap();
        assert_eq!(
            grade.command(&p, &[target], |_| true),
            Err(Error::Model(ModelError::Locked(v)))
        );
    }

    #[test]
    fn text_style_preserves_words_and_rejects_unselected_shared_uses() {
        let mut p = Project::default();
        let v = p.sequence.add_track(TrackKind::Video, "V1");
        let source = clip(&mut p, v, 0, 0, 100);
        let target = clip(&mut p, v, 500, 0, 100);
        for (id, words, size) in [
            (source, "Source words", 32.0),
            (target, "Target words", 64.0),
        ] {
            let asset = p.sequence.clip(id).unwrap().1.asset;
            p.asset_mut(asset).unwrap().text = Some(Text {
                content: words.into(),
                size,
                ..Text::default()
            });
        }
        let asset = p.sequence.clip(target).unwrap().1.asset;
        let duplicate = p
            .sequence
            .add_clip(
                v,
                NewClip {
                    asset,
                    start: Time(1000),
                    source_in: Time(0),
                    source_out: Time(100),
                    link: None,
                },
            )
            .unwrap();
        let settings = extract(&p, source, &[Category::Text], Animation::Static);
        assert_eq!(
            settings.command(&p, &[target], |_| true),
            Err(Error::SharedText(asset))
        );
        let mut h = History::new(p);
        h.apply(
            settings
                .command(h.project(), &[target, duplicate], |_| true)
                .unwrap(),
        )
        .unwrap();
        let text = h.project().asset(asset).unwrap().text.as_ref().unwrap();
        assert_eq!((&text.content[..], text.size), ("Target words", 32.0));
        assert!(h.undo());
        assert_eq!(
            h.project()
                .asset(asset)
                .unwrap()
                .text
                .as_ref()
                .unwrap()
                .size,
            64.0
        );
        h.apply(Command::Nest {
            ids: vec![duplicate],
            name: "Nested".into(),
            outer: "Timeline".into(),
        })
        .unwrap();
        assert_eq!(
            settings.command(h.project(), &[target], |_| true),
            Err(Error::SharedText(asset)),
            "closed-sequence uses cannot be accidentally changed"
        );
    }

    #[test]
    fn audio_replaces_only_audio_settings() {
        let mut p = Project::default();
        let a = p.sequence.add_track(TrackKind::Audio, "A1");
        let source = clip(&mut p, a, 0, 0, 100);
        let target = clip(&mut p, a, 500, 0, 100);
        p.sequence
            .with_clip_mut(source, |c| {
                c.transform.volume = 0.5;
                c.transform.pan = -0.5;
                c.transform.audio_fx.high_pass.freq = 100.0;
            })
            .unwrap();
        let settings = extract(&p, source, &[Category::Audio], Animation::Static);
        let before = p.sequence.clip(target).unwrap().1.clone();
        let mut h = History::new(p);
        h.apply(settings.command(h.project(), &[target], |_| true).unwrap())
            .unwrap();
        let current = h.project().sequence().clip(target).unwrap().1;
        let mut expected = before;
        expected.transform.volume = 0.5;
        expected.transform.pan = -0.5;
        expected.transform.audio_fx.high_pass.freq = 100.0;
        assert_eq!(current, &expected);
        assert!(matches!(
            Settings::extract(
                h.project(),
                source,
                &[Category::Grade],
                Time(0),
                Animation::Static
            ),
            Err(Error::Incompatible { .. })
        ));
    }

    #[test]
    fn animation_preserves_destination_speed_ramp_and_longer_targets_hold_last_value() {
        let mut p = Project::default();
        let v = p.sequence.add_track(TrackKind::Video, "V1");
        let source = clip(&mut p, v, 0, 100, 200);
        let target = clip(&mut p, v, 500, 1000, 1600);
        p.sequence
            .with_clip_mut(source, |c| {
                c.keyframes
                    .insert("rotation".into(), vec![key(100, 0.0), key(150, 10.0)]);
            })
            .unwrap();
        let speed = vec![key(1000, 1.0), key(1600, 3.0)];
        p.sequence
            .set_speed_keys(target, 1.0, speed.clone(), false)
            .unwrap();
        let settings = extract(&p, source, &[Category::Transform], Animation::Relative);
        let old = p.sequence.clip(target).unwrap().1.clone();
        let mut h = History::new(p);
        h.apply(settings.command(h.project(), &[target], |_| true).unwrap())
            .unwrap();
        let current = h.project().sequence().clip(target).unwrap().1;
        assert_eq!(current.keyframes["speed"], speed);
        assert_eq!(
            (
                current.source_in,
                current.source_out,
                current.start,
                current.duration()
            ),
            (old.source_in, old.source_out, old.start, old.duration())
        );
        let key = current.keyframes["rotation"]
            .iter()
            .find(|k| k.value == Value::Scalar(10.0))
            .unwrap();
        assert!((current.timeline_at(key.time).0 - current.start.0 - 50).abs() <= 1);
        assert_eq!(current.transform_at(current.end() - Time(1)).rotation, 10.0);
    }

    #[test]
    fn reversed_mapping_and_frozen_animation_have_explicit_outcomes() {
        let mut p = Project::default();
        let v = p.sequence.add_track(TrackKind::Video, "V1");
        let source = clip(&mut p, v, 0, 100, 200);
        let target = clip(&mut p, v, 500, 1000, 1100);
        p.sequence
            .with_clip_mut(source, |c| {
                c.keyframes
                    .insert("rotation".into(), vec![key(100, 0.0), key(199, 99.0)]);
            })
            .unwrap();
        p.sequence.set_speed(target, 1.0, true, false).unwrap();
        let settings = extract(&p, source, &[Category::Transform], Animation::Relative);
        let mut h = History::new(p);
        h.apply(settings.command(h.project(), &[target], |_| true).unwrap())
            .unwrap();
        let c = h.project().sequence().clip(target).unwrap().1;
        assert_eq!(c.transform_at(Time(500)).rotation, 0.0);
        assert_eq!(c.transform_at(Time(550)).rotation, 50.0);
        let mut frozen = h.project().clone();
        frozen.sequence.freeze_frame(target, Time(500)).unwrap();
        assert_eq!(
            settings.command(&frozen, &[target], |_| true),
            Err(Error::FrozenAnimation(target))
        );
        assert!(
            extract(&frozen, source, &[Category::Transform], Animation::Static)
                .command(&frozen, &[target], |_| true)
                .is_ok()
        );
    }

    #[test]
    fn reversing_hold_animation_keeps_its_jump_at_the_mapped_offset() {
        let mut p = Project::default();
        let v = p.sequence.add_track(TrackKind::Video, "V1");
        let source = clip(&mut p, v, 0, 100, 201);
        let target = clip(&mut p, v, 500, 1000, 1101);
        p.sequence
            .with_clip_mut(source, |c| {
                let mut held = key(100, 0.0);
                held.easing = Easing::Hold;
                c.keyframes
                    .insert("rotation".into(), vec![held, key(200, 10.0)]);
            })
            .unwrap();
        p.sequence.set_speed(source, 1.0, true, false).unwrap();
        let settings = extract(&p, source, &[Category::Transform], Animation::Relative);
        let mut h = History::new(p);
        h.apply(settings.command(h.project(), &[target], |_| true).unwrap())
            .unwrap();
        let c = h.project().sequence().clip(target).unwrap().1;
        assert_eq!(c.transform_at(Time(500)).rotation, 10.0);
        assert_eq!(c.transform_at(Time(501)).rotation, 0.0);
        assert_eq!(c.transform_at(Time(599)).rotation, 0.0);
    }

    #[test]
    fn speed_rounding_keeps_explicit_key_values() {
        let mut p = Project::default();
        let v = p.sequence.add_track(TrackKind::Video, "V1");
        let source = clip(&mut p, v, 0, 100, 200);
        let target = clip(&mut p, v, 500, 1000, 1200);
        p.sequence.set_speed(source, 2.0, false, false).unwrap();
        p.sequence
            .with_clip_mut(source, |c| {
                c.keyframes.insert(
                    "rotation".into(),
                    vec![key(100, 0.0), key(151, 10.0), key(199, 0.0)],
                );
            })
            .unwrap();
        let settings = extract(&p, source, &[Category::Transform], Animation::Relative);
        let mut h = History::new(p);
        h.apply(settings.command(h.project(), &[target], |_| true).unwrap())
            .unwrap();
        assert_eq!(
            h.project()
                .sequence()
                .clip(target)
                .unwrap()
                .1
                .transform_at(Time(525))
                .rotation,
            10.0
        );
    }

    #[test]
    fn replacing_referenced_masks_requires_effects_and_keeps_combined_stack_coherent() {
        use crate::model::{Mask, MaskShape};
        let mut p = Project::default();
        let v = p.sequence.add_track(TrackKind::Video, "V1");
        let source = clip(&mut p, v, 0, 0, 100);
        let plain = clip(&mut p, v, 500, 1000, 1100);
        let masked = clip(&mut p, v, 1000, 2000, 2100);
        for (id, mask_id, shader) in [(source, 7, "source"), (masked, 3, "destination")] {
            p.sequence
                .with_clip_mut(id, |c| {
                    let mut mask =
                        Mask::new(MaskShape::Rectangle, vec![[0.0, 0.0], [100.0, 100.0]]);
                    mask.id = mask_id;
                    c.transform.masks = vec![mask];
                    c.transform.effects = vec![Effect {
                        shader: shader.into(),
                        params: BTreeMap::new(),
                        enabled: true,
                        mask: Some(mask_id),
                    }];
                })
                .unwrap();
        }
        p.sequence
            .with_clip_mut(source, |c| {
                c.keyframes.insert(
                    "mask0.p0".into(),
                    vec![
                        Keyframe {
                            time: Time(0),
                            value: Value::Vec2([20.0, 30.0]),
                            easing: Easing::Linear,
                        },
                        Keyframe {
                            time: Time(99),
                            value: Value::Vec2([40.0, 50.0]),
                            easing: Easing::Linear,
                        },
                    ],
                );
            })
            .unwrap();
        let before = p.clone();
        let transform = extract(&p, source, &[Category::Transform], Animation::Relative);
        assert_eq!(
            transform.command(&p, &[plain, masked], |_| true),
            Err(Error::MaskedTarget(masked))
        );
        assert_eq!(
            p, before,
            "every target remains unchanged on preflight failure"
        );
        let effects = extract(&p, source, &[Category::Effects], Animation::Relative);
        assert_eq!(
            effects.command(&p, &[masked], |_| true),
            Err(Error::MaskNeedsTransform)
        );
        let combined = extract(
            &p,
            source,
            &[Category::Transform, Category::Effects],
            Animation::Relative,
        );
        let mut h = History::new(p);
        h.apply(
            combined
                .command(h.project(), &[plain, masked], |_| true)
                .unwrap(),
        )
        .unwrap();
        for id in [plain, masked] {
            let c = h.project().sequence().clip(id).unwrap().1;
            assert_eq!(c.transform.effects[0].mask, Some(c.transform.masks[0].id));
            assert_eq!(c.transform.effects[0].shader, "source");
            assert_eq!(
                c.value_at("mask0.p0", c.start),
                Some(Value::Vec2([20.0, 30.0]))
            );
            assert_eq!(
                c.value_at("mask0.p0", c.end() - Time(1)),
                Some(Value::Vec2([40.0, 50.0]))
            );
        }
        assert!(h.undo());
        assert_eq!(h.project(), &before);
        let mut invalid = before;
        invalid
            .sequence
            .with_clip_mut(source, |c| c.transform.effects[0].mask = Some(99))
            .unwrap();
        let settings = extract(
            &invalid,
            source,
            &[Category::Transform, Category::Effects],
            Animation::Relative,
        );
        assert_eq!(
            settings.command(&invalid, &[plain, masked], |_| true),
            Err(Error::InvalidEffectMask)
        );
    }

    #[test]
    fn trimmed_bezier_reverse_and_hold_reconstruct_explicit_boundary_values() {
        for reverse in [false, true] {
            let mut p = Project::default();
            let v = p.sequence.add_track(TrackKind::Video, "V1");
            let source = clip(&mut p, v, 0, 150, 250);
            let target = clip(&mut p, v, 500, 1000, if reverse { 1065 } else { 1100 });
            let mut eased = key(100, 0.0);
            eased.easing = Easing::Bezier([0.42, 0.0, 0.58, 1.0]);
            let mut held = key(200, 10.0);
            held.easing = Easing::Hold;
            let keys = vec![eased, held, key(300, 20.0)];
            p.sequence
                .with_clip_mut(source, |c| {
                    c.keyframes.insert("rotation".into(), keys.clone());
                })
                .unwrap();
            p.sequence.set_speed(source, 1.0, reverse, false).unwrap();
            p.sequence.set_speed(target, 1.0, reverse, false).unwrap();
            let source_clip = p.sequence.clip(source).unwrap().1.clone();
            let settings = extract(&p, source, &[Category::Transform], Animation::Relative);
            let mut h = History::new(p);
            h.apply(settings.command(h.project(), &[target], |_| true).unwrap())
                .unwrap();
            let target = h.project().sequence().clip(target).unwrap().1;
            for offset in [Time(0), target.duration() - Time(1)] {
                assert_eq!(
                    target.value_at("rotation", target.start + offset),
                    keyframe::value_at(&keys, source_clip.source_at(source_clip.start + offset))
                );
            }
            let offset = source_clip.timeline_at(Time(200)) - source_clip.start;
            assert!(
                target.keyframes["rotation"]
                    .iter()
                    .any(|k| k.time == target.source_at(target.start + offset)
                        && k.value == Value::Scalar(10.0))
            );
            assert!(
                target.keyframes["rotation"]
                    .iter()
                    .all(|k| k.time >= target.source_in && k.time < target.source_out)
            );
        }
    }
}
