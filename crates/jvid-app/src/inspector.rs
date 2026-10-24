//! The Inspector: transform, opacity, crop, masks, color adjustments, effects and solid color of the selected clips, with keyframes.

use std::collections::{BTreeMap, HashSet};

use egui::{Align, Layout, Rect, Sense, Ui, UiBuilder, Vec2, pos2, vec2};
use jvid_core::history::{Command, GestureKey};
use jvid_core::keyframe::{Easing, Keyframe, SPEED, Value};
use jvid_core::model::{
    AudioFx, Blend, Clip, ClipId, Compressor, Effect, Equalizer, Mask, MaskMode, MaskShape,
    MediaAsset, Project, Text, TrackKind, Transform,
};
use jvid_core::time::Time;
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{
    Button, Checkbox, ColorField, DragField, Dropdown, KeyButton, Keyed, SectionHeader, Segmented,
    Slider, TextInput,
};

use crate::i18n::{CATALOG, Label, tr, verbatim};
use crate::timeline::Edit;

/// One animatable property: its keyframe name and how its fields show.
struct Prop<'a> {
    name: &'a str,
    /// Shown name; `None` for built-ins, which [`caption`] names.
    label: Option<&'a str>,
    /// Each field's prefix.
    fields: &'static [Label],
    /// Each field's own property, for a row of several; empty when the fields are `name`'s.
    parts: &'static [&'static str],
    /// Shown value per stored value, e.g. 100 for a percentage.
    display: f64,
    speed: f64,
    decimals: usize,
    suffix: &'static str,
    /// Bounds of the stored value.
    range: (f64, f64),
    /// The slider's bounds where `range` has none.
    slider: Option<(f64, f64)>,
    /// What reset sets; `None` for [`Transform::default`]'s value.
    default: Option<f64>,
    /// Whether the row has reset; a mask's shape has no default to go back to.
    resets: bool,
}

impl Prop<'_> {
    fn label(&self) -> &str {
        self.label.unwrap_or_else(|| caption(self.name))
    }

    /// The properties the row edits.
    fn names(&self) -> Vec<&str> {
        match self.parts {
            [] => vec![self.name],
            parts => parts.to_vec(),
        }
    }

    /// Field `i`'s property, and its index in that property's value.
    fn field(&self, i: usize) -> (&str, usize) {
        self.parts.get(i).map_or((self.name, i), |n| (n, 0))
    }

    /// The slider's bounds, for a one-field row with any.
    fn slider(&self) -> Option<(f64, f64)> {
        let (lo, hi) = self.range;
        let ranged = lo.is_finite() && hi.is_finite();
        let one = self.fields.len() == 1;
        self.slider.or(ranged.then_some(self.range)).filter(|_| one)
    }

    fn default(&self, name: &str, i: usize) -> f64 {
        self.default
            .unwrap_or_else(|| fields(get(&Transform::default(), name))[i])
    }
}

/// A built-in section's or property's name in the UI language, by its id (`inspector.<id>`).
fn caption(id: &str) -> &str {
    CATALOG.get(&format!("inspector.{id}")).unwrap_or(id)
}

const fn prop<'a>(name: &'a str, fields: &'static [Label]) -> Prop<'a> {
    Prop {
        name,
        label: None,
        fields,
        parts: &[],
        slider: None,
        display: 1.0,
        speed: 1.0,
        decimals: 1,
        suffix: "",
        range: (f64::NEG_INFINITY, f64::INFINITY),
        default: None,
        resets: true,
    }
}

const XY: &[Label] = &[|| verbatim("X"), || verbatim("Y")];
const ONE: &[Label] = &[|| ""];
const CROP: &[Label] = &[
    || tr!("inspector.crop_l"),
    || tr!("inspector.crop_t"),
    || tr!("inspector.crop_r"),
    || tr!("inspector.crop_b"),
];

/// Sections by id, which names them through [`caption`] and keeps their open state.
const SECTIONS: [(&str, &[Prop<'static>]); 4] = [
    (
        "transform",
        &[
            prop("position", XY),
            Prop {
                display: 100.0,
                suffix: "%",
                slider: Some((0.0, 4.0)),
                ..prop("scale", ONE)
            },
            Prop {
                suffix: "°",
                slider: Some((-180.0, 180.0)),
                ..prop("rotation", ONE)
            },
            prop("anchor", XY),
            Prop {
                parts: &["crop_left", "crop_top", "crop_right", "crop_bottom"],
                range: (0.0, f64::INFINITY),
                // Whole pixels, so four fit on one row.
                decimals: 0,
                ..prop("crop", CROP)
            },
        ],
    ),
    (
        "opacity",
        &[Prop {
            display: 100.0,
            decimals: 0,
            suffix: "%",
            range: (0.0, 1.0),
            ..prop("opacity", ONE)
        }],
    ),
    (
        "color",
        &[
            Prop {
                speed: 0.02,
                decimals: 2,
                range: (-4.0, 4.0),
                ..prop("exposure", ONE)
            },
            grade("contrast"),
            grade("saturation"),
            grade("temperature"),
            grade("tint"),
            Prop {
                display: 100.0,
                decimals: 0,
                suffix: "%",
                range: (0.0, 1.0),
                ..prop("lut_intensity", ONE)
            },
        ],
    ),
    // Audio clips only.
    (
        "audio",
        &[
            Prop {
                display: 100.0,
                decimals: 0,
                suffix: "%",
                range: (0.0, 2.0),
                ..prop("volume", ONE)
            },
            Prop {
                display: 100.0,
                decimals: 0,
                speed: 0.5,
                range: (-1.0, 1.0),
                ..prop("pan", ONE)
            },
        ],
    ),
];

/// A -100..100% color adjustment.
const fn grade(name: &'static str) -> Prop<'static> {
    Prop {
        display: 100.0,
        decimals: 0,
        range: (-1.0, 1.0),
        ..prop(name, ONE)
    }
}

/// Undo gesture for drags in the inspector, xored with the property, field and drag count.
const GESTURE: GestureKey = 0x696e_7370_0000; // "insp"

/// Undo gesture for easing handle drags, xored with the property and drag count.
const EASING_GESTURE: GestureKey = 0x6561_7365_0000; // "ease"

/// Undo gesture for picker edits to a solid's color, xored with its asset id.
const SOLID_GESTURE: GestureKey = 0x736f_6c69_6400; // "solid"
/// Undo gesture for edits to a text asset's style, xored with its asset id.
const TEXT_GESTURE: GestureKey = 0x7465_7874_0000; // "text"

/// The effect index and parameter of an effect keyframe name (see [`Effect::key`]).
pub(crate) fn fx_param(name: &str) -> Option<(usize, &str)> {
    let (i, p) = name.strip_prefix("fx")?.split_once('.')?;
    Some((i.parse().ok()?, p))
}

/// The mask index and parameter of a mask keyframe name (see [`Mask::key`]).
fn mask_param(name: &str) -> Option<(usize, &str)> {
    let (i, p) = name.strip_prefix("mask")?.split_once('.')?;
    Some((i.parse().ok()?, p))
}

/// `name`'s value in `x`; scale reads as linked, X for both.
fn get(x: &Transform, name: &str) -> Value {
    if let Some((i, p)) = fx_param(name) {
        let v = x.effects.get(i).and_then(|e| e.params.get(p));
        return Value::Scalar(v.copied().unwrap_or(0.0));
    }
    if let Some((i, p)) = mask_param(name) {
        let m = x.masks.get(i);
        return match p {
            "feather" => Value::Scalar(m.map_or(0.0, |m| m.feather)),
            "opacity" => Value::Scalar(m.map_or(1.0, |m| m.opacity)),
            p => {
                let n = p.strip_prefix('p').and_then(|n| n.parse::<usize>().ok());
                let v = m.zip(n).and_then(|(m, n)| m.points.get(n));
                Value::Vec2(v.copied().unwrap_or_default())
            }
        };
    }
    let crop = |i: usize| Value::Scalar(x.crop[i]);
    match name {
        "position" => Value::Vec2(x.position),
        "anchor" => Value::Vec2(x.anchor),
        "scale" => Value::Scalar(x.scale[0]),
        "rotation" => Value::Scalar(x.rotation),
        "opacity" => Value::Scalar(x.opacity),
        "exposure" => Value::Scalar(x.exposure),
        "contrast" => Value::Scalar(x.contrast),
        "saturation" => Value::Scalar(x.saturation),
        "temperature" => Value::Scalar(x.temperature),
        "tint" => Value::Scalar(x.tint),
        "lut_intensity" => Value::Scalar(x.lut_intensity),
        "crop_left" => crop(0),
        "crop_top" => crop(1),
        "crop_right" => crop(2),
        "volume" => Value::Scalar(x.volume),
        "pan" => Value::Scalar(x.pan),
        _ => crop(3),
    }
}

fn set(x: &mut Transform, name: &str, v: Value) {
    if let (Some((i, p)), Value::Scalar(v)) = (fx_param(name), v) {
        if let Some(e) = x.effects.get_mut(i) {
            e.params.insert(p.to_owned(), v);
        }
        return;
    }
    if let Some((i, p)) = mask_param(name) {
        let Some(m) = x.masks.get_mut(i) else {
            return;
        };
        match (p, v) {
            ("feather", Value::Scalar(v)) => m.feather = v.max(0.0),
            ("opacity", Value::Scalar(v)) => m.opacity = v.clamp(0.0, 1.0),
            (p, Value::Vec2(v)) => {
                let n = p.strip_prefix('p').and_then(|n| n.parse::<usize>().ok());
                if let Some(pt) = n.and_then(|n| m.points.get_mut(n)) {
                    *pt = v;
                }
            }
            _ => {}
        }
        return;
    }
    match (name, v) {
        ("position", Value::Vec2(v)) => x.position = v,
        ("anchor", Value::Vec2(v)) => x.anchor = v,
        // Keeps a non-uniform scale's ratio.
        ("scale", Value::Scalar(v)) if x.scale[0] != 0.0 => {
            x.scale = [v, x.scale[1] * v / x.scale[0]]
        }
        ("scale", Value::Scalar(v)) => x.scale = [v; 2],
        ("rotation", Value::Scalar(v)) => x.rotation = v,
        ("opacity", Value::Scalar(v)) => x.opacity = v.clamp(0.0, 1.0),
        ("exposure", Value::Scalar(v)) => x.exposure = v.clamp(-4.0, 4.0),
        ("contrast", Value::Scalar(v)) => x.contrast = v.clamp(-1.0, 1.0),
        ("saturation", Value::Scalar(v)) => x.saturation = v.clamp(-1.0, 1.0),
        ("temperature", Value::Scalar(v)) => x.temperature = v.clamp(-1.0, 1.0),
        ("tint", Value::Scalar(v)) => x.tint = v.clamp(-1.0, 1.0),
        ("lut_intensity", Value::Scalar(v)) => x.lut_intensity = v.clamp(0.0, 1.0),
        ("crop_left", Value::Scalar(v)) => x.crop[0] = v,
        ("crop_top", Value::Scalar(v)) => x.crop[1] = v,
        ("crop_right", Value::Scalar(v)) => x.crop[2] = v,
        ("crop_bottom", Value::Scalar(v)) => x.crop[3] = v,
        ("volume", Value::Scalar(v)) => x.volume = v.clamp(0.0, 2.0),
        ("pan", Value::Scalar(v)) => x.pan = v.clamp(-1.0, 1.0),
        _ => {}
    }
}

fn fields(v: Value) -> Vec<f64> {
    match v {
        Value::Scalar(s) => vec![s],
        Value::Vec2(v) => v.to_vec(),
        Value::Color(c) => c.to_vec(),
    }
}

fn with_field(v: Value, i: usize, n: f64) -> Value {
    match v {
        Value::Scalar(_) => Value::Scalar(n),
        Value::Vec2(mut v) => {
            v[i] = n;
            Value::Vec2(v)
        }
        Value::Color(mut c) => {
            c[i] = n;
            Value::Color(c)
        }
    }
}

/// The clip's source time at timeline time `t`, where its keyframes live, clamped to the
/// clip so a playhead outside it never keys beyond its media range.
fn source(c: &Clip, t: Time) -> Time {
    c.source_at(t)
}

fn value(c: &Clip, name: &str, t: Time) -> Value {
    get(&c.transform_at(t), name)
}

fn animated(c: &Clip, name: &str) -> bool {
    c.keyframes.get(name).is_some_and(|k| !k.is_empty())
}

fn key_at(c: &Clip, name: &str, t: Time) -> bool {
    let s = source(c, t);
    c.keyframes
        .get(name)
        .is_some_and(|k| k.iter().any(|k| k.time == s))
}

/// One step rewriting each clip's transform and keyframes with `f`.
fn rewrite(
    clips: &[&Clip],
    mut f: impl FnMut(&Clip, &mut Transform, &mut BTreeMap<String, Vec<Keyframe>>),
) -> Command {
    Command::SetClipProps(
        clips
            .iter()
            .map(|c| {
                let (mut x, mut keys) = (c.transform.clone(), c.keyframes.clone());
                f(c, &mut x, &mut keys);
                (c.id, x, keys)
            })
            .collect(),
    )
}

/// Adds or replaces the keyframe of `name` at timeline time `t`.
fn key(c: &Clip, keys: &mut BTreeMap<String, Vec<Keyframe>>, name: &str, t: Time, v: Value) {
    let list = keys.entry(name.into()).or_default();
    let time = source(c, t);
    match list.binary_search_by(|k| k.time.cmp(&time)) {
        Ok(i) => list[i].value = v,
        Err(i) => list.insert(
            i,
            Keyframe {
                time,
                value: v,
                easing: Easing::Linear,
            },
        ),
    }
}

/// Keys `v` at `t` on an animated property, or sets it as the static value.
fn put(
    c: &Clip,
    x: &mut Transform,
    keys: &mut BTreeMap<String, Vec<Keyframe>>,
    name: &str,
    t: Time,
    v: Value,
) {
    if keys.get(name).is_some_and(|k| !k.is_empty()) {
        key(c, keys, name, t, v);
    } else {
        set(x, name, v);
    }
}

/// Sets field `i` of `name` to `f` of its old value on every clip, keeping their other fields.
fn set_field(clips: &[&Clip], name: &str, i: usize, f: impl Fn(f64) -> f64, t: Time) -> Command {
    rewrite(clips, |c, x, keys| {
        let old = value(c, name, t);
        let v = with_field(old, i, f(fields(old)[i]));
        put(c, x, keys, name, t, v);
    })
}

/// Sets each of `names` to its value in `x` on every clip, keying animated ones at `t`. Once
/// a clip animates any of them it keys them all, a static one first at the others' key times
/// with its old value, so a group such as a mask's centre and size moves together.
pub fn set_props(clips: &[&Clip], names: &[&str], x: &Transform, t: Time) -> Command {
    rewrite(clips, |c, st, keys| {
        let times: std::collections::BTreeSet<Time> = names
            .iter()
            .filter_map(|n| keys.get(*n))
            .flatten()
            .map(|k| k.time)
            .collect();
        for name in names {
            if !times.is_empty() && !animated(c, name) {
                let v = value(c, name, t);
                let seed = times.iter().map(|&time| Keyframe {
                    time,
                    value: v,
                    easing: Easing::Linear,
                });
                keys.insert(name.to_string(), seed.collect());
            }
            put(c, st, keys, name, t, get(x, name));
        }
    })
}

/// Whether any clip animates any of `names`.
fn any_animated(clips: &[&Clip], names: &[&str]) -> bool {
    names.iter().any(|n| clips.iter().any(|c| animated(c, n)))
}

/// Whether every clip has a keyframe of each of `names` at `t`.
fn all_keyed(clips: &[&Clip], names: &[&str], t: Time) -> bool {
    names.iter().all(|n| clips.iter().all(|c| key_at(c, n, t)))
}

/// Animates `names` from their values at `t`, or, if any clip animates one, stops
/// animating them and keeps the values at `t`.
fn toggle_animate(clips: &[&Clip], names: &[&str], t: Time) -> Command {
    let on = any_animated(clips, names);
    rewrite(clips, |c, x, keys| {
        for name in names {
            let v = value(c, name, t);
            if on {
                keys.remove(*name);
                set(x, name, v);
            } else {
                key(c, keys, name, t, v);
            }
        }
    })
}

/// Removes the keyframes of `names` at `t` if every clip has them all there, else adds them.
fn toggle_key(clips: &[&Clip], names: &[&str], t: Time) -> Command {
    let all = all_keyed(clips, names, t);
    rewrite(clips, |c, x, keys| {
        for name in names {
            let v = value(c, name, t);
            if !all {
                key(c, keys, name, t, v);
                continue;
            }
            let list = keys.get_mut(*name).expect("all have a key");
            list.retain(|k| k.time != source(c, t));
            if list.is_empty() {
                // The last key gone: hold its value rather than jump back to the old static one.
                keys.remove(*name);
                set(x, name, v);
            }
        }
    })
}

fn reset(clips: &[&Clip], names: &[&str]) -> Command {
    rewrite(clips, |_, x, keys| {
        for name in names {
            keys.remove(*name);
            set(x, name, get(&Transform::default(), name));
        }
    })
}

/// Keys every one of `names` at `t`, from its current value.
fn key_all(clips: &[&Clip], names: &[&str], t: Time) -> Command {
    rewrite(clips, |c, _, keys| {
        for name in names {
            key(c, keys, name, t, value(c, name, t));
        }
    })
}

/// The nearest keyframe of `names` before (`dir` < 0) or after `t` across the clips, in
/// timeline time.
fn neighbour(clips: &[&Clip], names: &[&str], t: Time, dir: i64) -> Option<Time> {
    let times = clips.iter().flat_map(|c| {
        names.iter().flat_map(move |name| {
            let keys = c
                .keyframes
                .get(*name)
                .map(Vec::as_slice)
                .unwrap_or_default();
            keys.iter().map(move |k| c.timeline_at(k.time))
        })
    });
    if dir < 0 {
        times.filter(|k| *k < t).max()
    } else {
        times.filter(|k| *k > t).min()
    }
}

/// An Inspector tab, in the order they show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Text,
    Video,
    Audio,
    Effects,
}

/// What the Inspector edits: the selected clips by track kind, and the asset they share.
pub struct Selection<'a> {
    clips: Vec<&'a Clip>,
    audio: Vec<&'a Clip>,
    /// Only when every selected video clip shows it, so its editors edit them all.
    shared: Option<&'a MediaAsset>,
    same: bool,
    /// Each video clip's image size fitted to the frame, which a new mask's size follows.
    fitted: Vec<[f64; 2]>,
}

impl<'a> Selection<'a> {
    /// The selected clips, or the solid or text selected in the bin.
    pub fn new(
        project: &'a Project,
        selected: &HashSet<ClipId>,
        bin_selected: Option<&'a MediaAsset>,
    ) -> Self {
        let seq = project.sequence();
        // Audio clips have no picture to transform, only the Audio section.
        let of = |kind| {
            let mut clips: Vec<&Clip> = selected
                .iter()
                .filter_map(|id| seq.clip(*id))
                .filter(|(t, _)| seq.track(*t).is_some_and(|t| t.kind == kind))
                .map(|(_, c)| c)
                .collect();
            clips.sort_by_key(|c| c.id.0);
            clips
        };
        let (clips, audio) = (of(TrackKind::Video), of(TrackKind::Audio));
        let same = clips.iter().all(|c| c.asset == clips[0].asset);
        let shared = clips
            .first()
            .filter(|_| same)
            .and_then(|c| project.asset(c.asset))
            .or(bin_selected.filter(|a| a.generated()));
        let fitted = clips
            .iter()
            .map(|c| crate::masks::fitted(project, c))
            .collect();
        Self {
            clips,
            audio,
            shared,
            same,
            fitted,
        }
    }

    /// The tabs this selection has; none when there's nothing to inspect.
    pub fn panes(&self) -> Vec<Pane> {
        let video = !self.clips.is_empty();
        [
            (Pane::Text, self.shared.is_some_and(|a| a.text.is_some())),
            (
                Pane::Video,
                video || self.shared.is_some_and(|a| a.solid.is_some()),
            ),
            (Pane::Audio, !self.audio.is_empty()),
            (Pane::Effects, video),
        ]
        .into_iter()
        .filter_map(|(p, on)| on.then_some(p))
        .collect()
    }

    /// The effects every video clip has, when they all have the same stack.
    fn effect_count(&self) -> Option<usize> {
        let n = self.clips.first()?.transform.effects.len();
        let shaders = |c: &Clip| {
            c.transform
                .effects
                .iter()
                .map(|e| e.shader.clone())
                .collect::<Vec<_>>()
        };
        let same = self
            .clips
            .iter()
            .all(|c| shaders(c) == shaders(self.clips[0]));
        same.then_some(n)
    }

    /// A tab's label; Effects carries its count once there are any.
    pub fn label(&self, pane: Pane) -> String {
        match pane {
            Pane::Text => caption("text").to_owned(),
            Pane::Video => caption("video").to_owned(),
            Pane::Audio => caption("audio").to_owned(),
            Pane::Effects => match self.effect_count() {
                Some(n) if n > 0 => format!("{} {n}", caption("effects")),
                _ => caption("effects").to_owned(),
            },
        }
    }
}

fn pane_id() -> egui::Id {
    egui::Id::new("inspector_pane")
}

/// The tab to show: the one last chosen while the selection has it, else the first.
pub fn active(ctx: &egui::Context, panes: &[Pane]) -> usize {
    let chosen = ctx.data(|d| d.get_temp::<Pane>(pane_id()));
    panes.iter().position(|p| Some(*p) == chosen).unwrap_or(0)
}

/// Remembers a clicked tab, so it stays chosen across selections that have it.
pub fn choose(ctx: &egui::Context, pane: Pane) {
    ctx.data_mut(|d| d.insert_temp(pane_id(), pane));
}

/// Tab `pane` of `sel` at playhead `t`; an edit, and where to seek the playhead.
pub fn show(
    ui: &mut Ui,
    sel: &Selection,
    pane: Option<Pane>,
    t: Time,
) -> (Option<Edit>, Option<Time>) {
    // A gutter keeps the row buttons clear of the scroll bar.
    let gutter = ui.spacing().scroll.allocated_width() + space::S4;
    ui.set_max_width(ui.available_width() - gutter);
    let Some(pane) = pane else {
        ui.label(egui::RichText::new(tr!("inspector.empty")).color(color::ink_3()));
        return (None, None);
    };
    let (clips, audio, shared, same) = (&sel.clips, &sel.audio, sel.shared, sel.same);
    let (mut edit, mut seek) = (None, None);
    let mut index = 0u64;
    for (id, props) in SECTIONS {
        let (clips, here) = if id == "audio" {
            (audio, Pane::Audio)
        } else {
            (clips, Pane::Video)
        };
        if clips.is_empty() || pane != here {
            index += props.len() as u64;
            continue;
        }
        if id == "color"
            && let Some(e) = masks_rows(ui, clips, &sel.fitted, t, &mut index, &mut seek)
        {
            edit = Some(e);
        }
        // LUT Mix waits for a LUT.
        let lut = clips.iter().any(|c| c.transform.lut.is_some());
        let shown = |p: &Prop| p.name != "lut_intensity" || lut;
        let names: Vec<&str> = props
            .iter()
            .filter(|p| shown(p))
            .flat_map(Prop::names)
            .collect();
        let changed = props.iter().any(|p| shown(p) && differs(clips, p, t));
        let (open, head) = section(ui, id, Some(changed));
        match head {
            Some(Head::Reset) => edit = Some((reset(clips, &names), None)),
            Some(Head::Key) => edit = Some((key_all(clips, &names, t), None)),
            None => {}
        }
        if !open {
            index += props.len() as u64;
            continue;
        }
        for p in props {
            index += 1;
            if p.name == "lut_intensity"
                && let Some(e) = lut_row(ui, clips)
            {
                edit = Some(e);
            }
            if shown(p)
                && let Some(e) = row(ui, clips, p, t, index, &mut seek)
            {
                edit = Some(e);
            }
        }
        if id == "opacity"
            && let Some(e) = blend_row(ui, clips)
        {
            edit = Some(e);
        }
    }
    if pane == Pane::Effects
        && let Some(e) = effects_rows(ui, clips, t, &mut index, &mut seek)
    {
        edit = Some(e);
    }
    if pane == Pane::Audio && !audio.is_empty() {
        let fxs: Vec<AudioFx> = audio.iter().map(|c| c.transform.audio_fx).collect();
        if let Some((f, gesture)) = audio_fx(ui, &fxs) {
            edit = Some((rewrite(audio, |_, x, _| f(&mut x.audio_fx)), gesture));
        }
    }
    // Speed sits with the picture, or with the sound when there's no picture.
    let speed = if clips.is_empty() {
        Pane::Audio
    } else {
        Pane::Video
    };
    if pane == speed
        && let Some(c) = clips.first().or(audio.first())
        && section(ui, "speed", None).0
        && let Some(e) = speed_row(ui, c, t, index, &mut seek)
    {
        edit = Some(e);
    }
    if pane == Pane::Text
        && let Some(a) = shared.filter(|a| a.text.is_some())
        && let Some(e) = text_props(ui, a, if same { &clips[..] } else { &[] }, t, &mut seek)
    {
        edit = Some(e);
    }
    if pane == Pane::Video
        && let Some(a) = shared.filter(|a| a.solid.is_some())
        && section(ui, "color", None).0
        && let Some(e) = solid_color(ui, a)
    {
        edit = Some(e);
    }
    (edit, seek)
}

/// Blend mode names, in [`Blend::ALL`] order.
fn blends() -> [&'static str; 8] {
    [
        tr!("blend.normal"),
        tr!("blend.multiply"),
        tr!("blend.screen"),
        tr!("blend.overlay"),
        tr!("blend.add"),
        tr!("blend.darken"),
        tr!("blend.lighten"),
        tr!("blend.difference"),
    ]
}

/// Sets every clip's blend mode; one undo step.
fn set_blend(clips: &[&Clip], blend: Blend) -> Command {
    rewrite(clips, |_, x, _| x.blend = blend)
}

/// The blend mode dropdown, showing the first clip's mode when the selection is mixed.
fn blend_row(ui: &mut Ui, clips: &[&Clip]) -> Option<Edit> {
    let first = clips[0].transform.blend;
    let mut i = Blend::ALL.iter().position(|b| *b == first).unwrap_or(0);
    let tools = Tools::default();
    let (changed, _) = prop_row(ui, tr!("inspector.blend"), tools, |ui| {
        let w = ui.available_width();
        ui.add(Dropdown::new(&mut i, &blends()).width(w)).changed()
    });
    changed.then(|| (set_blend(clips, Blend::ALL[i]), None))
}

/// Sets every clip's `.cube` LUT, or clears it; one undo step.
fn set_lut(clips: &[&Clip], lut: Option<String>) -> Command {
    rewrite(clips, |_, x, _| x.lut.clone_from(&lut))
}

/// The LUT picker, showing the first clip's file name, with a clear button when set.
fn lut_row(ui: &mut Ui, clips: &[&Clip]) -> Option<Edit> {
    let current = clips[0].transform.lut.as_deref();
    let name = current.map_or(tr!("inspector.lut_none").into(), |p| {
        std::path::Path::new(p)
            .file_name()
            .map_or(p.into(), |n| n.to_string_lossy())
    });
    prop_row(ui, tr!("inspector.lut"), Tools::default(), |ui| {
        let mut edit = None;
        let clear = size::CONTROL + space::S4;
        let w = ui.available_width() - if current.is_some() { clear } else { 0.0 };
        if ui.add(Button::secondary(&name).width(w)).clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_title(tr!("inspector.choose_lut"))
                .add_filter(tr!("inspector.cube_filter"), &["cube"])
                .pick_file()
        {
            edit = Some((set_lut(clips, Some(path.to_string_lossy().into())), None));
        }
        let clear = Button::icon(icon::X, tr!("inspector.clear"));
        if current.is_some() && ui.add(clear).clicked() {
            edit = Some((set_lut(clips, None), None));
        }
        edit
    })
    .0
}

/// Rebuilds every clip's effect stack from old indices `f` gives, renumbering the effects'
/// keyframes to match and dropping those of effects left out.
fn restack(clips: &[&Clip], f: impl Fn(&mut Vec<usize>)) -> Command {
    rewrite(clips, |_, x, keys| {
        let mut order: Vec<usize> = (0..x.effects.len()).collect();
        f(&mut order);
        let old = std::mem::take(&mut x.effects);
        x.effects = order.iter().filter_map(|&j| old.get(j).cloned()).collect();
        *keys = std::mem::take(keys)
            .into_iter()
            .filter_map(|(k, v)| match fx_param(&k) {
                Some((j, p)) => order
                    .iter()
                    .position(|&o| o == j)
                    .map(|n| (Effect::key(n, p), v)),
                None => Some((k, v)),
            })
            .collect();
    })
}

/// What an effect block's header drags: the block's index in the stack.
#[derive(Clone, Copy)]
struct FxDrag(usize);

/// Moves the effect at `from` into gap `to`: 0 is above the first, `order.len()` below the last.
fn move_to(order: &mut Vec<usize>, from: usize, to: usize) {
    let fx = order.remove(from);
    order.insert(if to > from { to - 1 } else { to }, fx);
}

/// Bypasses effect `i` on every clip, or turns it back on when the first clip's is bypassed.
fn bypass(clips: &[&Clip], i: usize) -> Command {
    let on = !clips[0].transform.effects[i].enabled;
    rewrite(clips, |_, x, _| {
        if let Some(e) = x.effects.get_mut(i) {
            e.enabled = on;
        }
    })
}

/// Sets every one of an effect's parameter rows `props` back to its default, dropping keys.
fn reset_fx(clips: &[&Clip], props: &[Prop]) -> Command {
    rewrite(clips, |_, x, keys| {
        for p in props {
            keys.remove(p.name);
            set(x, p.name, Value::Scalar(p.default.unwrap_or_default()));
        }
    })
}

/// What an effect block's header asks for.
enum FxAction {
    Bypass,
    /// Into this gap, as [`move_to`] takes it.
    Move(usize),
    Reset,
    Remove,
}

/// The effect stack, shown when every clip has the same one: a block per effect, a header
/// then its parameters as keyframeable rows, and Add Effect at the end.
fn effects_rows(
    ui: &mut Ui,
    clips: &[&Clip],
    t: Time,
    index: &mut u64,
    seek: &mut Option<Time>,
) -> Option<Edit> {
    let stack = |c: &Clip| {
        c.transform
            .effects
            .iter()
            .map(|e| e.shader.clone())
            .collect::<Vec<_>>()
    };
    let effects = &clips[0].transform.effects;
    if clips.iter().any(|c| stack(c) != stack(clips[0])) {
        ui.label(egui::RichText::new(tr!("inspector.mixed_effects")).color(color::ink_3()));
        return None;
    }
    let lib: Vec<(String, crate::effects::Entry, bool)> = crate::effects::with(|l| {
        l.entries()
            .map(|(k, e)| (k.to_owned(), e.clone(), l.shipped(k)))
            .collect()
    });
    let mut edit = None;
    let n = effects.len();
    // Each block's top, then the stack's bottom: the gaps a dragged block drops into.
    let mut gaps = Vec::with_capacity(n + 1);
    for (i, e) in effects.iter().enumerate() {
        gaps.push(ui.cursor().top());
        if i > 0 {
            let y = ui.cursor().top() - ui.spacing().item_spacing.y / 2.0;
            let line = egui::Stroke::new(1.0 / ui.pixels_per_point(), color::border());
            ui.painter().hline(ui.max_rect().x_range(), y, line);
        }
        let found = lib.iter().find(|(k, ..)| *k == e.shader);
        let (entry, shipped) = (found.map(|(_, e, _)| e), found.is_some_and(|f| f.2));
        let (title, problem) = match entry {
            Some(Ok(s)) => (crate::effects::name(&e.shader, shipped, s), None),
            Some(Err(err)) => (e.shader.clone(), Some(err.clone())),
            None => (
                e.shader.clone(),
                Some(tr!(
                    "inspector.effect_missing",
                    file = format!("{}.wgsl", e.shader)
                )),
            ),
        };
        let params: Vec<_> = match entry {
            Some(Ok(shader)) => shader.params.iter().collect(),
            _ => Vec::new(),
        };
        let names: Vec<(String, String)> = params
            .iter()
            .map(|q| {
                let label = crate::effects::param(&e.shader, shipped, &q.name);
                (Effect::key(i, &q.name), label)
            })
            .collect();
        let props: Vec<Prop> = params
            .iter()
            .zip(&names)
            .map(|(q, (name, label))| Prop {
                speed: (q.max - q.min) / 200.0,
                decimals: 2,
                range: (q.min, q.max),
                default: Some(q.default),
                label: Some(label),
                ..prop(name, ONE)
            })
            .collect();
        let changed = props.iter().any(|p| differs(clips, p, t));
        // Keyed by shader and which of that shader it is, so it follows its block when moved.
        let nth = effects[..i].iter().filter(|x| x.shader == e.shader).count();
        let id = ui.id().with(("inspector fx", &e.shader, nth));
        let mut open = ui.data_mut(|d| *d.get_temp_mut_or(id, true));
        let head = FxHead {
            title: &title,
            problem,
            on: e.enabled,
            changed,
            stack: true,
        };
        let cmd = match fx_head(ui, head, &mut open, i, n) {
            Some(FxAction::Bypass) => Some(bypass(clips, i)),
            Some(FxAction::Move(to)) => Some(restack(clips, |o| move_to(o, i, to))),
            Some(FxAction::Reset) => Some(reset_fx(clips, &props)),
            Some(FxAction::Remove) => Some(restack(clips, |o| {
                o.remove(i);
            })),
            None => None,
        };
        if let Some(cmd) = cmd {
            edit = Some((cmd, None));
        }
        ui.data_mut(|d| d.insert_temp(id, open));
        let first = *index;
        *index += props.len() as u64;
        if open {
            // Bypassed: the settings stay, greyed out, until it's back on.
            ui.add_enabled_ui(e.enabled, |ui| {
                for (k, p) in (first + 1..).zip(&props) {
                    if let Some(e) = row(ui, clips, p, t, k, seek) {
                        edit = Some(e);
                    }
                }
            });
        }
    }
    gaps.push(ui.cursor().top());
    if let Some(e) = drop_block(ui, clips, &gaps) {
        edit = Some(e);
    }
    if let Some(e) = add_menu(ui, clips) {
        edit = Some(e);
    }
    edit
}

/// What an effect block's header shows.
struct FxHead<'a> {
    title: &'a str,
    /// Why the effect passes the image through unchanged, when it does.
    problem: Option<String>,
    on: bool,
    /// Whether any parameter is off its default, which shows reset.
    changed: bool,
    /// In the effect stack: dragging moves it, and More has Move and Remove.
    stack: bool,
}

/// Block `i` of `n`'s header: collapse and bypass, the name, then reset and More in the row
/// tool slots. Nothing in it moves or vanishes as the stack changes; menu items disable
/// instead. Dragging it moves the block, see [`drop_block`].
fn fx_head(ui: &mut Ui, head: FxHead, open: &mut bool, i: usize, n: usize) -> Option<FxAction> {
    let w = ui.available_width();
    let sense = if head.stack {
        Sense::drag()
    } else {
        Sense::hover()
    };
    let (rect, drag) = ui.allocate_exact_size(vec2(w, size::CONTROL), sense);
    if head.stack {
        drag.dnd_set_drag_payload(FxDrag(i));
    }
    if drag.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    let mut ui = ui.new_child(UiBuilder::new().max_rect(rect));
    let left = |k: f32| {
        let x = rect.left() + k * (size::SMALL_CONTROL + GAP);
        let y = rect.center().y - size::SMALL_CONTROL / 2.0;
        Rect::from_min_size(pos2(x, y), Vec2::splat(size::SMALL_CONTROL))
    };
    let mut act = None;
    let (glyph, name) = match *open {
        true => (icon::CHEVRON_DOWN, tr!("inspector.fx_collapse")),
        false => (icon::CHEVRON_RIGHT, tr!("inspector.fx_expand")),
    };
    if ui
        .put(left(0.0), Button::icon(glyph, name).small())
        .clicked()
    {
        *open = !*open;
    }
    let (glyph, name) = match head.on {
        true => (icon::EYE, tr!("inspector.fx_bypass")),
        false => (icon::EYE_OFF, tr!("inspector.fx_turn_on")),
    };
    let eye = Button::icon(glyph, name).small().on(!head.on);
    if ui.put(left(1.0), eye).clicked() {
        act = Some(FxAction::Bypass);
    }
    let ink = if head.on {
        color::ink()
    } else {
        color::ink_3()
    };
    let mut job = egui::text::LayoutJob::default();
    let format = |font, color| egui::TextFormat::simple(font, color);
    job.append(head.title, 0.0, format(theme::body_strong(), ink));
    if head.problem.is_some() {
        let pass = tr!("inspector.passthrough");
        job.append(pass, space::S4, format(theme::body(), color::red()));
    }
    let x = left(2.0).left() + space::S4;
    let text = Rect::from_x_y_ranges(x..=rect.right() - TOOLS - space::S4, rect.y_range());
    let layout = Layout::left_to_right(Align::Center);
    let mut name = ui.new_child(UiBuilder::new().max_rect(text).layout(layout));
    let label = name.add(egui::Label::new(job).truncate().selectable(false));
    if let Some(p) = &head.problem {
        label.on_hover_text(p);
    }
    let reset = tr!("inspector.reset_section", section = head.title);
    if head.changed
        && ui
            .put(slot(rect, 0.0), Button::icon(icon::UNDO, &reset).small())
            .clicked()
    {
        act = Some(FxAction::Reset);
    }
    if !head.stack {
        return act;
    }
    let more = Button::icon(icon::ELLIPSIS, tr!("inspector.more")).small();
    let more = ui.put(slot(rect, 3.0), more);
    egui::Popup::menu(&more).show(|ui| {
        let item = |ui: &mut Ui, label: &str, enabled| {
            ui.add_enabled(enabled, egui::Button::new(label)).clicked()
        };
        if item(ui, tr!("inspector.fx_up"), i > 0) {
            act = Some(FxAction::Move(i - 1));
        }
        if item(ui, tr!("inspector.fx_down"), i + 1 < n) {
            act = Some(FxAction::Move(i + 2));
        }
        if item(ui, tr!("inspector.reset"), head.changed) {
            act = Some(FxAction::Reset);
        }
        ui.separator();
        if item(ui, tr!("inspector.fx_remove"), true) {
            act = Some(FxAction::Remove);
        }
    });
    act
}

/// While a block's header is dragged over the stack, a line at the gap it would drop into
/// among `gaps` (see [`effects_rows`]); dropping there moves it, one undo step.
fn drop_block(ui: &mut Ui, clips: &[&Clip], gaps: &[f32]) -> Option<Edit> {
    let from = egui::DragAndDrop::payload::<FxDrag>(ui.ctx())?.0;
    let pos = ui.ctx().pointer_latest_pos()?;
    let (top, bottom) = (gaps[0], gaps[gaps.len() - 1]);
    let area = Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=bottom);
    // Below a block's middle drops after it.
    let to = gaps
        .windows(2)
        .filter(|w| (w[0] + w[1]) / 2.0 < pos.y)
        .count();
    if !area.contains(pos) || to == from || to == from + 1 {
        return None;
    }
    let y = gaps[to] - ui.spacing().item_spacing.y / 2.0;
    let line = egui::Stroke::new(2.0, color::light_blue());
    ui.painter().hline(area.x_range(), y, line);
    let dropped = ui.input(|i| i.pointer.any_released());
    dropped.then(|| (restack(clips, |o| move_to(o, from, to)), None))
}

/// Add Effect: a button opening a search field over each usable effect's name and
/// description; picking one adds it to the end of every clip's stack.
fn add_menu(ui: &mut Ui, clips: &[&Clip]) -> Option<Edit> {
    let w = ui.available_width();
    let add = Button::secondary(tr!("inspector.add_effect")).with_icon(icon::PLUS);
    let r = ui.add(add.width(w));
    let query = ui.id().with("inspector fx query");
    if r.clicked() {
        ui.data_mut(|d| d.remove::<String>(query));
    }
    let mut pick = None;
    egui::Popup::menu(&r)
        .width(w)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            let mut q = ui.data(|d| d.get_temp::<String>(query)).unwrap_or_default();
            let hint = tr!("inspector.search_effects");
            let field = TextInput::search(&mut q).hint(hint);
            let field = ui.add(field.width(ui.available_width()));
            if ui.memory(|m| m.focused().is_none()) {
                field.request_focus();
            }
            let rows: Vec<_> = crate::effects::with(|l| l.rows(&q))
                .into_iter()
                .filter(|r| r.error.is_none() && !r.disabled)
                .collect();
            if rows.is_empty() {
                ui.label(egui::RichText::new(tr!("bin.fx_no_match")).color(color::ink_3()));
            }
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                pick = rows.first().map(|r| r.stem.clone());
            }
            crate::effects::want_thumbs(ui.ctx());
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for row in &rows {
                        if menu_entry(ui, row).clicked() {
                            pick = Some(row.stem.clone());
                        }
                    }
                });
            ui.data_mut(|d| d.insert_temp(query, q));
            if pick.is_some() {
                ui.close();
            }
        });
    let stem = pick?;
    let shader = crate::effects::with(|l| l.get(&stem).and_then(|e| e.clone().ok()))?;
    Some((push_effect(clips, &stem, &shader), None))
}

/// An Add Effect menu row: the effect's thumbnail, then its name in Ink over one line of its
/// description, with the menu rows' hover fill.
fn menu_entry(ui: &mut Ui, row: &crate::effects::Row) -> egui::Response {
    const THUMB: Vec2 = vec2(48.0, 27.0);
    let w = ui.available_width();
    let (rect, r) = ui.allocate_exact_size(vec2(w, 40.0), Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &row.name));
    if r.hovered() || r.has_focus() {
        let corner = suite_ui::tokens::radius::CONTROL;
        ui.painter().rect_filled(rect, corner, color::border());
    }
    let thumb = Rect::from_min_size(
        pos2(rect.left() + space::S4, rect.center().y - THUMB.y / 2.0),
        THUMB,
    );
    crate::effects::paint_thumb(ui, thumb, row);
    let left = thumb.right() + space::S8;
    let width = rect.right() - space::S8 - left;
    let name = ui
        .painter()
        .layout(row.name.clone(), theme::body(), color::ink(), width);
    let about = crate::effects::about(row, width, 1, color::ink_3());
    let about = ui.fonts_mut(|f| f.layout_job(about));
    let top = rect.center().y - (name.size().y + about.size().y) / 2.0;
    ui.painter()
        .galley(pos2(left, top), name.clone(), color::ink());
    let at = pos2(left, top + name.size().y);
    ui.painter().galley(at, about, color::ink_3());
    r
}

fn push_effect(clips: &[&Clip], stem: &str, shader: &jvid_render::effect::Shader) -> Command {
    let fx = Effect {
        shader: stem.to_owned(),
        params: shader
            .params
            .iter()
            .map(|q| (q.name.clone(), q.default))
            .collect(),
        enabled: true,
    };
    rewrite(clips, |_, x, _| x.effects.push(fx.clone()))
}

/// Adds effect `stem` from the effects folder to the end of each of the video clips in `ids`;
/// `None` when there are none or the effect doesn't load.
pub fn add_effect(project: &Project, ids: &HashSet<ClipId>, stem: &str) -> Option<Command> {
    let clips = video_clips(project, ids);
    let shader = crate::effects::with(|l| l.get(stem).and_then(|e| e.clone().ok()))?;
    (!clips.is_empty()).then(|| push_effect(&clips, stem, &shader))
}

/// The video clips among `ids`, which effects go on, in id order.
pub fn video_clips<'a>(project: &'a Project, ids: &HashSet<ClipId>) -> Vec<&'a Clip> {
    let seq = project.sequence();
    let mut clips: Vec<&Clip> = ids
        .iter()
        .filter_map(|id| seq.clip(*id))
        .filter(|(t, _)| seq.track(*t).is_some_and(|t| t.kind == TrackKind::Video))
        .map(|(_, c)| c)
        .collect();
    clips.sort_by_key(|c| c.id.0);
    clips
}

/// A mask shape's name.
fn shape_name(s: MaskShape) -> &'static str {
    match s {
        MaskShape::Rectangle => tr!("inspector.mask_rectangle"),
        MaskShape::Ellipse => tr!("inspector.mask_ellipse"),
        MaskShape::Bezier => tr!("inspector.mask_bezier"),
    }
}

/// Rebuilds every clip's masks from old indices `f` gives, renumbering their keyframes to
/// match and dropping those of masks left out, as [`restack`] does for effects.
fn restack_masks(clips: &[&Clip], f: impl Fn(&mut Vec<usize>)) -> Command {
    rewrite(clips, |_, x, keys| {
        let mut order: Vec<usize> = (0..x.masks.len()).collect();
        f(&mut order);
        let old = std::mem::take(&mut x.masks);
        x.masks = order.iter().filter_map(|&j| old.get(j).cloned()).collect();
        *keys = std::mem::take(keys)
            .into_iter()
            .filter_map(|(k, v)| match mask_param(&k) {
                Some((j, p)) => order
                    .iter()
                    .position(|&o| o == j)
                    .map(|n| (Mask::key(n, p), v)),
                None => Some((k, v)),
            })
            .collect();
    })
}

/// Mask `i` of every clip after `f`; one undo step.
fn edit_mask(clips: &[&Clip], i: usize, f: impl Fn(&mut Mask)) -> Command {
    rewrite(clips, |_, x, _| {
        if let Some(m) = x.masks.get_mut(i) {
            f(m);
        }
    })
}

/// Sets mask `i`'s feather, opacity, invert and mode back to a new mask's, dropping keys.
fn reset_mask(clips: &[&Clip], i: usize) -> Command {
    rewrite(clips, |_, x, keys| {
        keys.remove(&Mask::key(i, "feather"));
        keys.remove(&Mask::key(i, "opacity"));
        if let Some(m) = x.masks.get_mut(i) {
            *m = Mask {
                enabled: m.enabled,
                ..Mask::new(m.shape, std::mem::take(&mut m.points))
            };
        }
    })
}

/// A rectangle or ellipse of `shape` on every clip, centred and half its fitted image's size.
fn push_mask(clips: &[&Clip], fitted: &[[f64; 2]], shape: MaskShape) -> Command {
    let mut size = fitted.iter();
    rewrite(clips, |_, x, _| {
        let [w, h] = size.next().copied().unwrap_or([100.0; 2]);
        let points = vec![[0.0, 0.0], [w / 2.0, h / 2.0]];
        x.masks.push(Mask::new(shape, points));
    })
}

/// The Masks section: a block per mask when every clip has the same shapes, its header as
/// an effect block's, then Add Mask. A bezier is drawn on the Program view, so adding one
/// starts drawing there on the first clip, seeking into it.
fn masks_rows(
    ui: &mut Ui,
    clips: &[&Clip],
    fitted: &[[f64; 2]],
    t: Time,
    index: &mut u64,
    seek: &mut Option<Time>,
) -> Option<Edit> {
    if !section(ui, "masks", None).0 {
        return None;
    }
    let shapes = |c: &Clip| {
        c.transform
            .masks
            .iter()
            .map(|m| m.shape)
            .collect::<Vec<_>>()
    };
    if clips.iter().any(|c| shapes(c) != shapes(clips[0])) {
        ui.label(egui::RichText::new(tr!("inspector.mixed_masks")).color(color::ink_3()));
        return None;
    }
    let masks = &clips[0].transform.masks;
    let n = masks.len();
    let mut edit = None;
    for (i, m) in masks.iter().enumerate() {
        if i > 0 {
            let y = ui.cursor().top() - ui.spacing().item_spacing.y / 2.0;
            let line = egui::Stroke::new(1.0 / ui.pixels_per_point(), color::border());
            ui.painter().hline(ui.max_rect().x_range(), y, line);
        }
        let names = [0, 1, 2, 3].map(|j| match j {
            0 | 1 => Mask::point(i, j),
            2 => Mask::key(i, "feather"),
            _ => Mask::key(i, "opacity"),
        });
        let xy = |name, label| Prop {
            label: Some(label),
            resets: false,
            ..prop(name, XY)
        };
        let shape = [
            xy(&names[0], caption("position")),
            Prop {
                range: (0.0, f64::INFINITY),
                ..xy(&names[1], tr!("inspector.mask_size"))
            },
        ];
        let props = [
            Prop {
                label: Some(tr!("inspector.feather")),
                range: (0.0, f64::INFINITY),
                slider: Some((0.0, 200.0)),
                default: Some(0.0),
                ..prop(&names[2], ONE)
            },
            Prop {
                label: Some(caption("opacity")),
                display: 100.0,
                decimals: 0,
                suffix: "%",
                range: (0.0, 1.0),
                default: Some(1.0),
                ..prop(&names[3], ONE)
            },
        ];
        let changed = props.iter().any(|p| differs(clips, p, t))
            || clips.iter().any(|c| {
                let m = &c.transform.masks[i];
                m.invert || m.mode != MaskMode::Add
            });
        let title = tr!(
            "inspector.mask_title",
            shape = shape_name(m.shape),
            n = i + 1
        );
        let id = ui.id().with(("inspector mask", i));
        let mut open = ui.data_mut(|d| *d.get_temp_mut_or(id, true));
        let head = FxHead {
            title: &title,
            problem: None,
            on: m.enabled,
            changed,
            stack: true,
        };
        let on = !m.enabled;
        let cmd = match fx_head(ui, head, &mut open, i, n) {
            Some(FxAction::Bypass) => Some(edit_mask(clips, i, |m| m.enabled = on)),
            Some(FxAction::Move(to)) => Some(restack_masks(clips, |o| move_to(o, i, to))),
            Some(FxAction::Reset) => Some(reset_mask(clips, i)),
            Some(FxAction::Remove) => Some(restack_masks(clips, |o| {
                o.remove(i);
            })),
            None => None,
        };
        if let Some(cmd) = cmd {
            edit = Some((cmd, None));
        }
        ui.data_mut(|d| d.insert_temp(id, open));
        let first = *index;
        *index += 6;
        if !open {
            continue;
        }
        ui.add_enabled_ui(m.enabled, |ui| {
            let shape = match m.shape {
                MaskShape::Bezier => &[][..],
                _ => &shape[..],
            };
            if m.shape == MaskShape::Bezier
                && let Some(e) = path_row(ui, clips, i, t, first + 1, seek)
            {
                edit = Some(e);
            }
            // Shape rows take first + 1 and 2 (a Path row only 1), so props start at 3.
            let rows = (first + 1..).zip(shape).chain((first + 3..).zip(&props));
            for (k, p) in rows {
                if let Some(e) = row(ui, clips, p, t, k, seek) {
                    edit = Some(e);
                }
            }
            let modes = [MaskMode::Add, MaskMode::Subtract];
            let mut k = usize::from(m.mode == MaskMode::Subtract);
            let labels = [tr!("inspector.mask_add"), tr!("inspector.mask_subtract")];
            let tools = Tools::default();
            let (changed, _) = prop_row(ui, tr!("inspector.mask_mode"), tools, |ui| {
                let w = ui.available_width();
                ui.add(Dropdown::new(&mut k, &labels).width(w)).changed()
            });
            if changed {
                edit = Some((edit_mask(clips, i, |m| m.mode = modes[k]), None));
            }
            let mut invert = m.invert;
            let (changed, _) = prop_row(ui, "", Tools::default(), |ui| {
                ui.add(Checkbox::new(&mut invert, tr!("inspector.invert")))
                    .changed()
            });
            if changed {
                edit = Some((edit_mask(clips, i, |m| m.invert = invert), None));
            }
        });
    }
    if let Some(e) = add_mask_menu(ui, clips, fitted, t, seek) {
        edit = Some(e);
    }
    edit
}

/// A bezier mask's Path row: its point count, and keyframe tools that key every point
/// together, so the shape animates as one.
fn path_row(
    ui: &mut Ui,
    clips: &[&Clip],
    i: usize,
    t: Time,
    index: u64,
    seek: &mut Option<Time>,
) -> Option<Edit> {
    let n = clips[0].transform.masks[i].points.len();
    let names: Vec<String> = (0..n).map(|j| Mask::point(i, j)).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let label = tr!("inspector.mask_path");
    let anim = any_animated(clips, &names);
    let key = match anim {
        false => tr!("inspector.animate", name = label),
        true => tr!("inspector.key_here").to_owned(),
    };
    let keyed = match () {
        _ if !anim => Keyed::Static,
        _ if all_keyed(clips, &names, t) => Keyed::Here,
        _ => Keyed::Animated,
    };
    let tools = Tools {
        reset: None,
        nav: anim,
        key: Some((keyed, &key)),
        compact: false,
    };
    let points = tr!("inspector.mask_points", n = n / 3);
    let (_, c) = prop_row(ui, label, tools, |ui| {
        ui.label(egui::RichText::new(points).color(color::ink_3()))
    });
    key_clicks(clips, &names, c, t, index, seek)
}

/// Add Mask: a button opening the shapes; a rectangle or ellipse goes on every clip, a bezier
/// starts drawing on the Program view.
fn add_mask_menu(
    ui: &mut Ui,
    clips: &[&Clip],
    fitted: &[[f64; 2]],
    t: Time,
    seek: &mut Option<Time>,
) -> Option<Edit> {
    let w = ui.available_width();
    let add = Button::secondary(tr!("inspector.add_mask")).with_icon(icon::PLUS);
    let r = ui.add(add.width(w));
    let mut pick = None;
    egui::Popup::menu(&r).width(w).show(|ui| {
        for s in [MaskShape::Rectangle, MaskShape::Ellipse, MaskShape::Bezier] {
            if ui.add(egui::Button::new(shape_name(s))).clicked() {
                pick = Some(s);
            }
        }
    });
    match pick? {
        MaskShape::Bezier => {
            let c = clips[0];
            crate::masks::draw(ui.ctx(), c.id);
            if !(c.start <= t && t < c.end()) {
                *seek = Some(c.start);
            }
            None
        }
        s => Some((push_mask(clips, fitted, s), None)),
    }
}

/// Undo gesture for speed drags, xored with the clip id.
const SPEED_GESTURE: GestureKey = 0x7370_6565_6400; // "speed"

/// Speed percentage, reverse and ripple for the first selected clip and its links. Once
/// animated, the percentage and the diamond edit the speed keys at playhead `t`.
fn speed_row(ui: &mut Ui, c: &Clip, t: Time, index: u64, seek: &mut Option<Time>) -> Option<Edit> {
    let ripple_id = egui::Id::new("speed_ripple");
    let mut ripple = ui.data(|d| d.get_temp(ripple_id)).unwrap_or(true);
    let names = [SPEED];
    let keys = c.ramp().unwrap_or_default();
    let (anim, s) = (!keys.is_empty(), source(c, t));
    let keyed = match () {
        _ if !anim => Keyed::Static,
        _ if keys.iter().any(|k| k.time == s) => Keyed::Here,
        _ => Keyed::Animated,
    };
    let label = match keyed {
        Keyed::Static => tr!("inspector.animate", name = tr!("inspector.speed")),
        _ => tr!("inspector.key_here").to_owned(),
    };
    // A freeze ignores its speed, so it gets no diamond.
    let tools = Tools {
        reset: None,
        nav: anim,
        key: c.freeze.is_none().then_some((keyed, label.as_str())),
        compact: false,
    };
    // The keys with speed `v` keyed at source time `at`.
    let with_key = |at: Time, v: f64| {
        let mut keys = keys.to_vec();
        match keys.binary_search_by(|k| k.time.cmp(&at)) {
            Ok(i) => keys[i].value = Value::Scalar(v),
            Err(i) => keys.insert(
                i,
                Keyframe {
                    time: at,
                    value: Value::Scalar(v),
                    easing: Easing::Linear,
                },
            ),
        }
        keys
    };
    let gesture = Some(SPEED_GESTURE ^ c.id.0);
    let speed = if anim { c.speed_at(t) } else { c.speed };
    let (mut pct, mut reverse) = (speed * 100.0, c.reverse);
    let ((changed, flipped, at), tool) = prop_row(ui, tr!("inspector.speed"), tools, |ui| {
        let drag = DragField::new(&mut pct)
            .range(1.0..=10000.0)
            .suffix("%")
            .width(64.0);
        let r = ui.add_enabled(c.freeze.is_none(), drag);
        ui.add_space(space::S4);
        let b = ui
            .add(Checkbox::new(&mut reverse, tr!("inspector.reverse")))
            .changed();
        ui.add_space(space::S4);
        ui.add(Checkbox::new(&mut ripple, tr!("inspector.ripple")))
            .on_hover_text(tr!("inspector.ripple_tooltip"));
        // The ramp moves the playhead's source time as it changes, so a drag keeps keying
        // the one it started on.
        let at = ui.id().with("speed key");
        if r.drag_started() {
            ui.data_mut(|d| d.insert_temp(at, s));
        }
        let at = r.dragged().then(|| ui.data(|d| d.get_temp(at))).flatten();
        (r.changed(), b, at.unwrap_or(s))
    });
    ui.data_mut(|d| d.insert_temp(ripple_id, ripple));
    // Every speed edit is one retime, rippling as the checkbox says.
    let retime = |speed, keys| Command::SetSpeedKeys {
        id: c.id,
        speed,
        keys,
        ripple,
    };
    let v = pct / 100.0;
    let mut edit = match () {
        _ if changed && anim => Some((retime(c.speed, with_key(at, v)), gesture)),
        _ if changed || flipped => {
            let speed = if changed { v } else { c.speed };
            let cmd = Command::SetClipSpeed {
                id: c.id,
                speed,
                reverse,
                ripple,
            };
            Some((cmd, gesture))
        }
        _ => None,
    };
    if tool.previous {
        *seek = neighbour(&[c], &names, t, -1);
    }
    if tool.next {
        *seek = neighbour(&[c], &names, t, 1);
    }
    let diamond = tool.key?;
    let here = c.speed_at(t);
    if diamond.clicked() {
        let cmd = match keyed {
            Keyed::Static => retime(c.speed, with_key(s, c.speed)),
            Keyed::Animated => retime(c.speed, with_key(s, here)),
            // The last key gone, its speed holds.
            Keyed::Here => match keys
                .iter()
                .filter(|k| k.time != s)
                .copied()
                .collect::<Vec<_>>()
            {
                rest if rest.is_empty() => retime(here, rest),
                rest => retime(c.speed, rest),
            },
        };
        edit = Some((cmd, None));
    }
    if anim {
        egui::Popup::context_menu(&diamond)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                if keyed == Keyed::Here
                    && let Some((Command::SetClipProps(mut v), g)) =
                        easing_menu(ui, &[c], &names, t, index)
                {
                    let keys = v.pop().and_then(|(.., mut k)| k.remove(SPEED));
                    edit = Some((retime(c.speed, keys.unwrap_or_default()), g));
                }
                if ui
                    .add(Button::ghost(tr!("inspector.stop_animating")))
                    .clicked()
                {
                    edit = Some((retime(here, Vec::new()), None));
                    ui.close();
                }
            });
    }
    edit
}

/// Undo gesture for audio effect drags, xored with the setting and drag count.
const AUDIO_FX_GESTURE: GestureKey = 0x6175_6678_0000; // "aufx"

/// One audio effect setting: its id (`inspector.<id>` names it), bounds and units.
struct Knob {
    id: &'static str,
    lo: f32,
    hi: f32,
    suffix: &'static str,
    decimals: usize,
    /// The slider moves by ratios (octaves, for hertz) rather than steps.
    log: bool,
}

const fn knob(id: &'static str, lo: f32, hi: f32, suffix: &'static str) -> Knob {
    Knob {
        id,
        lo,
        hi,
        suffix,
        decimals: 1,
        log: false,
    }
}

/// The EQ then Compressor blocks, by id (`inspector.<id>` names them), in processing order.
const AUDIO_FX: [(&str, &[Knob]); 2] = [
    (
        "eq",
        &[knob(
            "eq_post_gain",
            -Equalizer::RANGE,
            Equalizer::RANGE,
            verbatim(" dB"),
        )],
    ),
    (
        "compressor",
        &[
            knob("comp_threshold", -60.0, 0.0, verbatim(" dB")),
            knob("comp_ratio", 1.0, 20.0, ":1"),
            Knob {
                log: true,
                ..knob("comp_attack", 0.1, 200.0, verbatim(" ms"))
            },
            Knob {
                decimals: 0,
                log: true,
                ..knob("comp_release", 5.0, 2_000.0, verbatim(" ms"))
            },
            knob("comp_makeup", 0.0, 24.0, verbatim(" dB")),
        ],
    ),
];

fn knob_mut<'a>(fx: &'a mut AudioFx, id: &str) -> &'a mut f32 {
    let (e, c) = (&mut fx.eq, &mut fx.compressor);
    match id {
        "eq_post_gain" => &mut e.post_gain,
        "comp_threshold" => &mut c.threshold,
        "comp_ratio" => &mut c.ratio,
        "comp_attack" => &mut c.attack,
        "comp_release" => &mut c.release,
        _ => &mut c.makeup,
    }
}

fn knob_value(mut fx: AudioFx, id: &str) -> f32 {
    *knob_mut(&mut fx, id)
}

fn block_on<'a>(fx: &'a mut AudioFx, block: &str) -> &'a mut bool {
    match block {
        "eq" => &mut fx.eq.enabled,
        _ => &mut fx.compressor.enabled,
    }
}

/// The EQ presets (`inspector.eq_preset_<id>` names them): the bands from 31 Hz up, then
/// Post-Gain.
// shortcut: the #263 mockup's placeholder values, tune them by ear before a release.
const EQ_PRESETS: [(&str, [f32; 10], f32); 8] = [
    ("flat", [0.0; 10], 0.0),
    ("voice", [-12., -8., -3., 0., -1., 1., 3., 4., 2., 0.], 0.0),
    (
        "podcast",
        [-15., -9., -2., 2., 0., -1., 2., 3., 1., -1.],
        0.0,
    ),
    ("bass_boost", [6., 6., 4., 2., 0., 0., 0., 0., 0., 0.], -3.0),
    (
        "bass_cut",
        [-8., -6., -4., -2., 0., 0., 0., 0., 0., 0.],
        0.0,
    ),
    (
        "treble_boost",
        [0., 0., 0., 0., 0., 1., 2., 4., 6., 6.],
        -2.0,
    ),
    (
        "low_cut",
        [-15., -15., -6., -1., 0., 0., 0., 0., 0., 0.],
        0.0,
    ),
    ("loudness", [6., 4., 1., 0., -1., -1., 0., 2., 4., 5.], -3.0),
];

/// The Compressor presets (`inspector.comp_preset_<id>`): threshold, ratio, attack, release and
/// makeup. None is the default, which leaves the sound alone.
const COMP_PRESETS: [(&str, [f32; 5]); 5] = [
    ("none", [-20.0, 1.0, 10.0, 100.0, 0.0]),
    ("gentle", [-18.0, 2.0, 20.0, 200.0, 2.0]),
    ("voice", [-22.0, 3.0, 5.0, 120.0, 4.0]),
    ("broadcast", [-26.0, 4.0, 2.0, 80.0, 6.0]),
    ("limiter", [-3.0, 20.0, 0.1, 50.0, 0.0]),
];

fn comp_knobs(c: &mut Compressor) -> [&mut f32; 5] {
    [
        &mut c.threshold,
        &mut c.ratio,
        &mut c.attack,
        &mut c.release,
        &mut c.makeup,
    ]
}

/// The block's preset `fx` matches exactly, by index.
fn preset_of(fx: AudioFx, block: &str) -> Option<usize> {
    let (eq, mut c) = (fx.eq, fx.compressor);
    match block {
        "eq" => EQ_PRESETS
            .iter()
            .position(|(_, b, post)| eq.bands == *b && eq.post_gain == *post),
        _ => COMP_PRESETS
            .iter()
            .position(|(_, k)| comp_knobs(&mut c).map(|v| *v) == *k),
    }
}

/// Sets the block to preset `n`, leaving it on or bypassed.
fn set_preset(fx: &mut AudioFx, block: &str, n: usize) {
    match block {
        "eq" => (fx.eq.bands, fx.eq.post_gain) = (EQ_PRESETS[n].1, EQ_PRESETS[n].2),
        _ => {
            for (v, p) in comp_knobs(&mut fx.compressor)
                .into_iter()
                .zip(COMP_PRESETS[n].1)
            {
                *v = p;
            }
        }
    }
}

/// A Preset row showing the preset every one of `fxs` matches, otherwise Custom. Returns the
/// preset picked.
fn preset_row(ui: &mut Ui, fxs: &[AudioFx], block: &str) -> Option<usize> {
    let names: Vec<_> = match block {
        "eq" => EQ_PRESETS
            .iter()
            .map(|p| caption_of("eq_preset_", p.0))
            .collect(),
        _ => COMP_PRESETS
            .iter()
            .map(|p| caption_of("comp_preset_", p.0))
            .collect(),
    };
    let n = names.len();
    let at = preset_of(fxs[0], block)
        .filter(|at| fxs.iter().all(|fx| preset_of(*fx, block) == Some(*at)));
    let mut options: Vec<&str> = names.iter().map(String::as_str).collect();
    // shortcut: Custom is the dropdown's last option and picking it does nothing; leaving it
    // out of the menu and greying it needs suite-ui's Dropdown (#257's).
    let mut i = at.unwrap_or_else(|| {
        options.push(tr!("inspector.custom"));
        n
    });
    let tools = Tools::default();
    let (changed, _) = prop_row(ui, tr!("inspector.preset"), tools, |ui| {
        // It has no tools, so it takes their slots too.
        let w = ui.available_width() + TOOLS + space::S4;
        ui.add(Dropdown::new(&mut i, &options).width(w)).changed()
    });
    (changed && i < n && Some(i) != at).then_some(i)
}

fn caption_of(prefix: &str, id: &str) -> String {
    caption(&format!("{prefix}{id}")).to_owned()
}

/// Each band's frequency, as the Slider Bank labels it, and its accessible name.
const BANDS: [(&str, &str); 10] = [
    (verbatim("31"), verbatim("31 Hz")),
    (verbatim("62"), verbatim("62 Hz")),
    (verbatim("125"), verbatim("125 Hz")),
    (verbatim("250"), verbatim("250 Hz")),
    (verbatim("500"), verbatim("500 Hz")),
    (verbatim("1K"), verbatim("1 kHz")),
    (verbatim("2K"), verbatim("2 kHz")),
    (verbatim("4K"), verbatim("4 kHz")),
    (verbatim("8K"), verbatim("8 kHz")),
    (verbatim("16K"), verbatim("16 kHz")),
];

/// A gain in dB as the Slider Bank reads it: "0", "+6", "−1.5".
fn db_readout(v: f32) -> String {
    let sign = if v > 0.0 { "+" } else { "\u{2212}" };
    match v {
        0.0 => "0".to_owned(),
        _ if v.fract() == 0.0 => format!("{sign}{}", v.abs()),
        _ => format!("{sign}{:.1}", v.abs()),
    }
}

/// The Slider Bank (#263 option A): a column per band, its dB above and its frequency below,
/// over the response curve and a ±15 dB grid. Pressing or dragging a column sets its band to
/// the pointer in 0.5 dB steps, 0.1 with Shift; double-click resets it; a focused band takes Up
/// and Down. Returns the band, its new gain and the response that set it.
fn slider_bank(ui: &mut Ui, eq: &Equalizer) -> Option<(usize, f32, egui::Response)> {
    use egui::emath::GuiRounding as _;
    let (w, ppp) = (ui.available_width(), ui.pixels_per_point());
    let (rect, _) = ui.allocate_exact_size(vec2(w, 146.0), Sense::hover());
    let plot = Rect::from_min_size(rect.min + vec2(0.0, 16.0), vec2(w, 112.0));
    let range = Equalizer::RANGE;
    let y_of = |v: f32| {
        let v = v.clamp(-range - 1.0, range + 1.0);
        plot.top() + 8.0 + (range - v) / (2.0 * range) * (plot.height() - 16.0)
    };
    let painter = ui.painter().clone();
    let line = theme::hairline(ui.ctx(), color::border());
    painter.rect(plot, 4.0, color::field(), line, egui::StrokeKind::Inside);
    for v in [12.0, 6.0, -6.0, -12.0, 0.0] {
        let ink = if v == 0.0 {
            color::border_strong()
        } else {
            color::border()
        };
        let y = y_of(v).round_to_pixel_center(ppp);
        painter.hline(
            plot.x_range().shrink(1.0),
            y,
            theme::hairline(ui.ctx(), ink),
        );
    }
    // The real response, so the curve shows what plays: band k's centre sits mid-column.
    let col = w / 10.0;
    let zero = y_of(0.0);
    let curve: Vec<_> = (0..=(w / 2.0) as usize)
        .map(|n| {
            let x = (plot.left() + 1.0 + n as f32 * 2.0).min(plot.right() - 1.0);
            let hz = 1_000.0 * 2f64.powf(((x - plot.left()) / col) as f64 - 5.5);
            pos2(
                x,
                y_of(eq.response(hz, jvid_audio::SAMPLE_RATE as f64) as f32),
            )
        })
        .collect();
    let mut area = egui::Mesh::default();
    let fill = color::ink().gamma_multiply(0.05);
    for (n, p) in curve.iter().enumerate() {
        area.colored_vertex(*p, fill);
        area.colored_vertex(pos2(p.x, zero), fill);
        if n > 0 {
            let i = 2 * n as u32;
            area.add_triangle(i - 2, i - 1, i);
            area.add_triangle(i - 1, i, i + 1);
        }
    }
    painter.add(area);
    painter.add(egui::Shape::line(
        curve,
        egui::Stroke::new(1.5, color::ink_3()),
    ));

    let mut out = None;
    let medium = egui::FontId::new(theme::small().size, theme::medium());
    for (k, (label, name)) in BANDS.iter().enumerate() {
        let column = Rect::from_x_y_ranges(
            rect.left() + col * k as f32..=rect.left() + col * (k + 1) as f32,
            rect.y_range(),
        );
        let id = ui.id().with(("eq band", k));
        let r = ui.interact(column, id, Sense::click_and_drag());
        let v = eq.bands[k];
        r.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), v as f64, *name));
        let step = if ui.input(|i| i.modifiers.shift) {
            0.1
        } else {
            0.5
        };
        let snap = |v: f32| ((v / step).round() * step).clamp(-range, range);
        let keys = ui.input(|i| {
            i32::from(i.key_pressed(egui::Key::ArrowUp))
                - i32::from(i.key_pressed(egui::Key::ArrowDown))
        });
        let to = if r.double_clicked() {
            Some(0.0)
        } else if (r.clicked() || r.dragged())
            && let Some(p) = r.interact_pointer_pos()
        {
            let t = (p.y - plot.top() - 8.0) / (plot.height() - 16.0);
            Some(snap(range - t * 2.0 * range))
        } else if r.has_focus() && keys != 0 {
            Some(snap(v + keys as f32 * step))
        } else {
            None
        };
        let (hover, drag) = (r.hovered() || r.has_focus(), r.dragged());
        let x = column.center().x.round_to_pixel_center(ppp);
        if hover || drag {
            let bg = if drag {
                color::light_blue().gamma_multiply(0.08)
            } else {
                color::ink().gamma_multiply(0.04)
            };
            let cell =
                Rect::from_x_y_ranges(column.x_range().shrink(1.0), plot.y_range().shrink(1.0));
            painter.rect_filled(cell, 3.0, bg);
        }
        let track = egui::Rangef::new(plot.top() + 8.0, plot.bottom() - 8.0);
        painter.vline(x, track, egui::Stroke::new(2.0, color::border_strong()));
        let lit = if drag {
            color::light_blue()
        } else {
            color::ink_3()
        };
        if v != 0.0 {
            painter.vline(
                x,
                egui::Rangef::new(zero, y_of(v)),
                egui::Stroke::new(2.0, lit),
            );
        }
        let thumb = Rect::from_center_size(pos2(x, y_of(v)), vec2(14.0, 6.0)).round_to_pixels(ppp);
        let ink = match () {
            _ if drag => color::light_blue(),
            _ if hover => color::ink(),
            _ => color::ink_2(),
        };
        painter.rect_filled(thumb, 2.0, ink);
        if r.has_focus() {
            painter.add(suite_ui::widgets::focus_ring(thumb, 2));
        }
        let (font, ink) = match () {
            _ if drag => (medium.clone(), color::light_blue()),
            _ if v == 0.0 => (theme::small(), color::ink_3()),
            _ => (theme::small(), color::ink_2()),
        };
        let top = pos2(x, rect.top() + 7.0);
        painter.text(top, egui::Align2::CENTER_CENTER, db_readout(v), font, ink);
        let ink = if drag { color::ink() } else { color::ink_3() };
        let bottom = pos2(x, rect.bottom() - 7.0);
        painter.text(
            bottom,
            egui::Align2::CENTER_CENTER,
            *label,
            theme::small(),
            ink,
        );
        if let Some(to) = to.filter(|to| *to != v) {
            out = Some((k, to, r));
        }
    }
    out
}

/// Changes each of the clips' or track's effects the same way.
pub(crate) type FxEdit = Box<dyn Fn(&mut AudioFx)>;

/// The EQ and Compressor blocks for `fxs`, the selected audio clips' or a track's, showing the
/// first one's settings. Returns the edit to make to each, with its undo gesture while dragging.
pub(crate) fn audio_fx(ui: &mut Ui, fxs: &[AudioFx]) -> Option<(FxEdit, Option<GestureKey>)> {
    let first = fxs[0];
    let default = AudioFx::default();
    let mut edit: Option<(FxEdit, Option<GestureKey>)> = None;
    let drags = ui.id().with("audio fx drags");
    for (b, (block, knobs)) in AUDIO_FX.iter().enumerate() {
        if b > 0 {
            let y = ui.cursor().top() - ui.spacing().item_spacing.y / 2.0;
            let line = egui::Stroke::new(1.0 / ui.pixels_per_point(), color::border());
            ui.painter().hline(ui.max_rect().x_range(), y, line);
        }
        let on = *block_on(&mut first.clone(), block);
        let off_default =
            |fx: &AudioFx, k: &Knob| knob_value(*fx, k.id) != knob_value(default, k.id);
        let bands_off = |fx: &AudioFx| *block == "eq" && fx.eq.bands != default.eq.bands;
        let changed = knobs
            .iter()
            .any(|k| fxs.iter().any(|fx| off_default(fx, k)))
            || fxs.iter().any(bands_off);
        let id = ui.id().with(("inspector audio fx", block));
        let mut open = ui.data_mut(|d| *d.get_temp_mut_or(id, true));
        let head = FxHead {
            title: caption(block),
            problem: None,
            on,
            changed,
            stack: false,
        };
        match fx_head(ui, head, &mut open, b, AUDIO_FX.len()) {
            Some(FxAction::Bypass) => {
                let block = *block;
                edit = Some((Box::new(move |fx| *block_on(fx, block) = !on), None));
            }
            Some(FxAction::Reset) => {
                let ids: Vec<_> = knobs.iter().map(|k| k.id).collect();
                let eq = *block == "eq";
                let f = move |fx: &mut AudioFx| {
                    for id in &ids {
                        *knob_mut(fx, id) = knob_value(default, id);
                    }
                    if eq {
                        fx.eq.bands = default.eq.bands;
                    }
                };
                edit = Some((Box::new(f), None));
            }
            _ => {}
        }
        ui.data_mut(|d| d.insert_temp(id, open));
        if !open {
            continue;
        }
        // Bypassed: the settings stay, greyed out, until it's back on.
        ui.add_enabled_ui(on, |ui| {
            if let Some(n) = preset_row(ui, fxs, block) {
                let block = *block;
                edit = Some((Box::new(move |fx| set_preset(fx, block, n)), None));
            }
            if *block == "eq" {
                ui.add_space(space::S4);
                if let Some((k, v, r)) = slider_bank(ui, &first.eq) {
                    if r.drag_started() {
                        ui.data_mut(|d| *d.get_temp_mut_or(drags, 0u64) += 1);
                    }
                    // Clicks share their band's gesture until the next drag, so the click a
                    // double-click starts with and its reset are one step.
                    let drag = ui.data(|d| d.get_temp(drags).unwrap_or(0u64));
                    let slot = 0x80 + k as u64;
                    let gesture = (r.clicked() || r.dragged())
                        .then_some(AUDIO_FX_GESTURE ^ (slot << 4) ^ (drag << 16));
                    edit = Some((Box::new(move |fx| fx.eq.bands[k] = v), gesture));
                }
                ui.add_space(space::S4);
            }
            for (n, k) in knobs.iter().enumerate() {
                let v = knob_value(first, k.id) as f64;
                let mixed = fxs.iter().any(|fx| knob_value(*fx, k.id) as f64 != v);
                let tools = Tools {
                    reset: fxs
                        .iter()
                        .any(|fx| off_default(fx, k))
                        .then_some(tr!("inspector.reset")),
                    ..Tools::default()
                };
                let (lo, hi) = (k.lo as f64, k.hi as f64);
                let (r, c) = prop_row(ui, caption(k.id), tools, |ui| {
                    let mut field = v;
                    let f = DragField::new(&mut field)
                        .speed((hi - lo) / 200.0)
                        .decimals(k.decimals)
                        .suffix(k.suffix)
                        .mixed(mixed)
                        .range(lo..=hi)
                        .width(64.0);
                    let r = ui.add(f);
                    let to = |x: f64| if k.log { x.log10() } else { x };
                    let from = |x: f64| if k.log { 10f64.powf(x) } else { x };
                    let mut slid = to(v);
                    let s = Slider::new(&mut slid, to(lo)..=to(hi), caption(k.id))
                        .centred(lo < 0.0 && hi > 0.0);
                    let s = ui.add(s);
                    match () {
                        _ if r.changed() => Some((r, field)),
                        _ if s.changed() => Some((s, from(slid))),
                        _ => None,
                    }
                });
                let slot = (b * 16 + n) as u64;
                if let Some((r, v)) = r {
                    if r.drag_started() {
                        ui.data_mut(|d| *d.get_temp_mut_or(drags, 0u64) += 1);
                    }
                    let drag = ui.data(|d| d.get_temp(drags).unwrap_or(0u64));
                    let gesture = r
                        .dragged()
                        .then_some(AUDIO_FX_GESTURE ^ (slot << 4) ^ (drag << 16));
                    let (id, v) = (k.id, v.clamp(lo, hi) as f32);
                    edit = Some((Box::new(move |fx| *knob_mut(fx, id) = v), gesture));
                }
                if c.reset {
                    let id = k.id;
                    let f = move |fx: &mut AudioFx| *knob_mut(fx, id) = knob_value(default, id);
                    edit = Some((Box::new(f), None));
                }
            }
        });
    }
    edit
}

/// A section head button.
enum Head {
    Reset,
    Key,
}

/// A section header remembering whether it's open. `Some(changed)` adds the tool slots:
/// keyframe-all, and reset when `changed`.
fn section(ui: &mut Ui, id: &str, tools: Option<bool>) -> (bool, Option<Head>) {
    let title = caption(id);
    let id = ui.id().with(("inspector", id));
    let mut open = ui.data_mut(|d| *d.get_temp_mut_or(id, true));
    let rect = ui.add(SectionHeader::new(title, &mut open)).rect;
    ui.data_mut(|d| d.insert_temp(id, open));
    let head = tools.and_then(|changed| head_tools(ui, rect, title, changed, true, false));
    (open, head)
}

/// A section head's Reset (when `changed`) and, with `key`, its Keyframe All, in `rect`'s
/// tool slots; which was clicked.
fn head_tools(
    ui: &mut Ui,
    rect: Rect,
    title: &str,
    changed: bool,
    key: bool,
    compact: bool,
) -> Option<Head> {
    let mut ui = ui.new_child(UiBuilder::new().max_rect(rect));
    let reset = tr!("inspector.reset_section", section = title);
    let all = tr!("inspector.keyframe_all", section = title);
    let c = slots(
        &mut ui,
        rect,
        Tools {
            reset: changed.then_some(&reset),
            key: key.then_some((Keyed::Static, &all)),
            compact,
            ..Tools::default()
        },
    );
    match () {
        _ if c.reset => Some(Head::Reset),
        _ if c.key.is_some_and(|k| k.clicked()) => Some(Head::Key),
        _ => None,
    }
}

/// Width of a row's tool slots: reset, previous and next keyframe, then the diamond.
const TOOLS: f32 = 4.0 * size::SMALL_CONTROL + 3.0 * GAP;
/// Width of a compact row's: reset, then the diamond.
const COMPACT_TOOLS: f32 = 2.0 * size::SMALL_CONTROL + GAP;
/// Space between tool slots.
const GAP: f32 = 2.0;

/// What a row's tool slots show, with names for screen readers. An empty slot keeps its
/// place, so nothing moves when another appears.
#[derive(Default)]
struct Tools<'a> {
    reset: Option<&'a str>,
    nav: bool,
    key: Option<(Keyed, &'a str)>,
    /// Only reset and the diamond, the keyframe arrows moving to its menu (the Text tab).
    compact: bool,
}

/// What happened in a row's tool slots.
#[derive(Default)]
struct Clicks {
    reset: bool,
    previous: bool,
    next: bool,
    key: Option<egui::Response>,
}

/// Tool slot `i` at the right edge of `rect`: 0 reset, 1 and 2 the keyframe arrows, 3 the
/// diamond (or an effect's More).
fn slot(rect: Rect, i: f32) -> Rect {
    let x = rect.right() - TOOLS + i * (size::SMALL_CONTROL + GAP);
    let y = rect.center().y - size::SMALL_CONTROL / 2.0;
    Rect::from_min_size(pos2(x, y), Vec2::splat(size::SMALL_CONTROL))
}

/// Draws `tools` in the slots at the right edge of `rect`.
fn slots(ui: &mut Ui, rect: Rect, tools: Tools) -> Clicks {
    let slot = |i: f32| slot(rect, i);
    let mut c = Clicks::default();
    if let Some(name) = tools.reset {
        let at = if tools.compact { 2.0 } else { 0.0 };
        c.reset = ui
            .put(slot(at), Button::icon(icon::UNDO, name).small())
            .clicked();
    }
    if tools.nav {
        let previous = KeyButton::previous(tr!("inspector.previous_key"));
        c.previous = ui.put(slot(1.0), previous).clicked();
        c.next = ui
            .put(slot(2.0), KeyButton::next(tr!("inspector.next_key")))
            .clicked();
    }
    c.key = tools
        .key
        .map(|(k, name)| ui.put(slot(3.0), KeyButton::diamond(k, name)));
    c
}

/// An inspector row: the label column, `vals` in the width after it, and `tools` in their
/// slots at the right edge. Effect parameters use it too.
fn prop_row<R>(
    ui: &mut Ui,
    label: &str,
    tools: Tools,
    vals: impl FnOnce(&mut Ui) -> R,
) -> (R, Clicks) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), size::CONTROL), Sense::hover());
    let layout = Layout::left_to_right(Align::Center);
    let mut row = ui.new_child(UiBuilder::new().max_rect(rect).layout(layout));
    row.spacing_mut().item_spacing.x = space::S4;
    row.allocate_ui_with_layout(vec2(size::INSPECTOR_LABEL, size::CONTROL), layout, |ui| {
        ui.set_min_width(size::INSPECTOR_LABEL);
        ui.label(egui::RichText::new(label).color(color::ink_2()))
    });
    let tools_w = if tools.compact { COMPACT_TOOLS } else { TOOLS };
    let w = rect.right() - tools_w - space::S4 - row.cursor().left();
    let inner = row
        .allocate_ui_with_layout(vec2(w, size::CONTROL), layout, |ui| {
            ui.set_max_width(w);
            vals(ui)
        })
        .inner;
    (inner, slots(&mut row, rect, tools))
}

/// Whether any clip animates `p` or, at `t`, shows a value other than its default.
fn differs(clips: &[&Clip], p: &Prop, t: Time) -> bool {
    // Off by less than the field shows reads as the default.
    let close = 0.5 * 10f64.powi(-(p.decimals as i32)) / p.display;
    let off = |i| {
        let (name, j) = p.field(i);
        let d = p.default(name, j);
        clips
            .iter()
            .any(|c| (fields(value(c, name, t))[j] - d).abs() >= close)
    };
    any_animated(clips, &p.names()) || (0..p.fields.len()).any(off)
}

/// Sets `p` back to its default and drops its keyframes.
fn reset_prop(clips: &[&Clip], p: &Prop) -> Command {
    match p.default {
        Some(d) => rewrite(clips, |_, x, keys| {
            keys.remove(p.name);
            set(x, p.name, Value::Scalar(d));
        }),
        None => reset(clips, &p.names()),
    }
}

fn row(
    ui: &mut Ui,
    clips: &[&Clip],
    p: &Prop,
    t: Time,
    index: u64,
    seek: &mut Option<Time>,
) -> Option<Edit> {
    let names = p.names();
    let anim = any_animated(clips, &names);
    let keyed = match () {
        _ if !anim => Keyed::Static,
        _ if all_keyed(clips, &names, t) => Keyed::Here,
        _ => Keyed::Animated,
    };
    let reset = tr!("inspector.reset");
    let key = match keyed {
        Keyed::Static => tr!("inspector.animate", name = p.label()),
        _ => tr!("inspector.key_here").to_owned(),
    };
    let tools = Tools {
        reset: (p.resets && differs(clips, p, t)).then_some(reset),
        nav: anim,
        key: Some((keyed, &key)),
        compact: false,
    };
    let mut edit = None;
    let (_, c) = prop_row(ui, p.label(), tools, |ui| {
        // Each drag is its own undo step, even of the field the last one dragged.
        let drags = ui.id().with("inspector drags");
        let mut put = |ui: &Ui, r: &egui::Response, cmd: Command, slot: usize| {
            if r.drag_started() {
                ui.data_mut(|d| *d.get_temp_mut_or(drags, 0u64) += 1);
            }
            let drag = ui.data(|d| d.get_temp(drags).unwrap_or(0u64));
            let key = GESTURE ^ (index << 4 | slot as u64) ^ (drag << 16);
            edit = Some((cmd, r.dragged().then_some(key)));
        };
        let n = p.fields.len();
        let slider = p.slider();
        let width = match slider {
            Some(_) => 64.0,
            None => (ui.available_width() - space::S4 * (n - 1) as f32) / n as f32,
        };
        let (lo, hi) = p.range;
        for (i, field) in p.fields.iter().enumerate() {
            let (name, j) = p.field(i);
            let values: Vec<f64> = clips.iter().map(|c| fields(value(c, name, t))[j]).collect();
            let first = values[0];
            let mixed = values.iter().any(|v| *v != first);
            let mut shown = first * p.display;
            let mut f = DragField::new(&mut shown)
                .speed(p.speed)
                .decimals(p.decimals)
                .suffix(p.suffix)
                .mixed(mixed)
                .range(lo * p.display..=hi * p.display)
                .width(width);
            if !field().is_empty() {
                f = f.prefix(field());
            }
            let r = ui.add(f);
            if r.changed() {
                let v = shown / p.display;
                // A drag of a mixed field moves each clip by the delta, not to the first's value.
                let cmd = if mixed && r.dragged() {
                    set_field(clips, name, j, |o| (o + v - first).clamp(lo, hi), t)
                } else {
                    set_field(clips, name, j, |_| v, t)
                };
                put(ui, &r, cmd, i);
            }
        }
        if let Some((a, b)) = slider {
            let mut shown = fields(value(clips[0], p.name, t))[0] * p.display;
            let s = Slider::new(&mut shown, a * p.display..=b * p.display, p.label())
                .centred(a < 0.0 && b > 0.0);
            let r = ui.add(s);
            if r.changed() {
                let v = shown / p.display;
                put(ui, &r, set_field(clips, p.name, 0, |_| v, t), n);
            }
        }
    });
    if c.reset {
        edit = Some((reset_prop(clips, p), None));
    }
    key_clicks(clips, &names, c, t, index, seek).or(edit)
}

/// What a row's keyframe tools did to `names`: the arrows seek to the next key either side,
/// the diamond animates or keys them, and right-clicking it edits their easing.
fn key_clicks(
    clips: &[&Clip],
    names: &[&str],
    c: Clicks,
    t: Time,
    index: u64,
    seek: &mut Option<Time>,
) -> Option<Edit> {
    let toggles = (
        || toggle_animate(clips, names, t),
        || toggle_key(clips, names, t),
    );
    key_menu(clips, names, c, t, index, seek, false, toggles)
}

/// [`key_clicks`] with the diamond's commands: `animate` starts or stops animating, `key`
/// adds or removes the keyframe at the playhead. With `nav` the menu has the arrows too.
#[allow(clippy::too_many_arguments)]
fn key_menu(
    clips: &[&Clip],
    names: &[&str],
    c: Clicks,
    t: Time,
    index: u64,
    seek: &mut Option<Time>,
    nav: bool,
    (animate, key): (impl Fn() -> Command, impl Fn() -> Command),
) -> Option<Edit> {
    let anim = any_animated(clips, names);
    let keyed = match () {
        _ if !anim => Keyed::Static,
        _ if all_keyed(clips, names, t) => Keyed::Here,
        _ => Keyed::Animated,
    };
    let mut edit = None;
    if c.previous {
        *seek = neighbour(clips, names, t, -1);
    }
    if c.next {
        *seek = neighbour(clips, names, t, 1);
    }
    let diamond = c.key.expect("rows have a diamond");
    if diamond.clicked() {
        let cmd = match anim {
            false => animate(),
            true => key(),
        };
        edit = Some((cmd, None));
    }
    if anim {
        egui::Popup::context_menu(&diamond)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                if keyed == Keyed::Here
                    && let Some(e) = easing_menu(ui, clips, names, t, index)
                {
                    edit = Some(e);
                }
                for (name, dir) in [("inspector.previous_key", -1), ("inspector.next_key", 1)] {
                    let to = neighbour(clips, names, t, dir);
                    let b = Button::ghost(CATALOG.get(name).unwrap_or(name));
                    if nav && ui.add_enabled(to.is_some(), b).clicked() {
                        *seek = to;
                        ui.close();
                    }
                }
                if ui
                    .add(Button::ghost(tr!("inspector.stop_animating")))
                    .clicked()
                {
                    edit = Some((animate(), None));
                    ui.close();
                }
            });
    }
    edit
}

/// The easing editor for the keyframes of `names` at `t`, opened by right-clicking their
/// diamond.
fn easing_menu(ui: &mut Ui, clips: &[&Clip], names: &[&str], t: Time, index: u64) -> Option<Edit> {
    let s = source(clips[0], t);
    let k = clips[0]
        .keyframes
        .get(names[0])?
        .iter()
        .find(|k| k.time == s)?;
    let (e, dragging) = crate::easing::editor(ui, k.easing)?;
    // One undo step per handle drag.
    let drags = ui.id().with("easing drags");
    let drag = ui.data_mut(|d| {
        let n = d.get_temp_mut_or(drags, (0u64, false));
        n.0 += u64::from(dragging && !n.1);
        n.1 = dragging;
        n.0
    });
    let key = EASING_GESTURE ^ index ^ (drag << 16);
    Some((set_easing(clips, names, t, e), dragging.then_some(key)))
}

/// Sets the easing of every clip's keyframes of `names` at `t`.
fn set_easing(clips: &[&Clip], names: &[&str], t: Time, e: Easing) -> Command {
    rewrite(clips, |c, _, keys| {
        let s = source(c, t);
        for name in names {
            for k in keys.get_mut(*name).into_iter().flatten() {
                if k.time == s {
                    k.easing = e;
                }
            }
        }
    })
}

/// The color picker for a solid; the edit once the color changes.
// shortcut: edits the asset's color; per-clip `color` keyframes wait on a later issue.
fn solid_color(ui: &mut Ui, a: &MediaAsset) -> Option<Edit> {
    let mut rgba = jvid_render::solid::rgba8(a.solid?);
    let name = tr!("inspector.color");
    let (changed, _) = prop_row(ui, name, Tools::default(), |ui| {
        ui.add(ColorField::new(&mut rgba, name)).changed()
    });
    let color = rgba.map(|v| v as f64 / 255.0);
    changed.then_some((
        Command::SetSolidColor { id: a.id, color },
        Some(SOLID_GESTURE ^ a.id.0),
    ))
}

/// Text style keyframe names: the animated style and the fill, a clip `color` keyframe.
fn text_names() -> impl Iterator<Item = &'static str> {
    Text::ANIMATED.into_iter().chain(["color"])
}

fn text_get(t: &Text, name: &str) -> Option<Value> {
    match name {
        "color" => Some(Value::Color(t.color)),
        _ => t.get(name),
    }
}

fn text_set(t: &mut Text, name: &str, v: Value) {
    match (name, v) {
        ("color", Value::Color(c)) => t.color = c,
        _ => t.set(name, v),
    }
}

/// Egui id of the Inspector's text content field.
fn content_id() -> egui::Id {
    egui::Id::new("inspector text content")
}

/// The families in `list` containing `query`, ignoring case.
fn font_matches<'a>(list: &'a [String], query: &str) -> Vec<&'a str> {
    let q = query.to_lowercase();
    list.iter()
        .filter(|f| f.to_lowercase().contains(&q))
        .map(String::as_str)
        .collect()
}

/// An open font search: the query, the highlighted match, the family before it opened, and
/// last frame's area, so a click outside cancels.
#[derive(Clone)]
struct FontSearch {
    query: String,
    at: usize,
    before: String,
    area: egui::Rect,
}

/// `text` laid out on one line, cut to `width` with an ellipsis.
fn truncated(
    ui: &Ui,
    text: &str,
    color: egui::Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let format = egui::TextFormat::simple(theme::body(), color);
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), format);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
    ui.painter().layout_job(job)
}

/// The Font field's face: `shown` and a chevron in a field, after an Amber warning when
/// `missing`. Takes the row's width.
fn font_button(ui: &mut Ui, shown: &str, missing: bool, open: bool) -> egui::Response {
    let (rect, r) =
        ui.allocate_exact_size(vec2(ui.available_width(), size::CONTROL), Sense::click());
    let name = match missing {
        true => tr!("inspector.font_missing", family = shown),
        false => shown.to_owned(),
    };
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &name));
    let v = match r.hovered() || open {
        true => &ui.visuals().widgets.hovered,
        false => &ui.visuals().widgets.inactive,
    };
    let p = ui.painter();
    p.rect(
        rect,
        v.corner_radius,
        v.bg_fill,
        v.bg_stroke,
        egui::StrokeKind::Inside,
    );
    let (mut x, y) = (rect.left() + space::S8, rect.center().y);
    if missing {
        let c = pos2(x + size::ICON / 2.0, y);
        let a = egui::Align2::CENTER_CENTER;
        p.text(c, a, icon::CIRCLE_ALERT, theme::icon_font(), color::amber());
        x += size::ICON + space::S4;
    }
    let chevron = rect.right() - space::S4 - size::ICON / 2.0;
    let g = truncated(ui, shown, color::ink(), chevron - size::ICON / 2.0 - x);
    p.galley(pos2(x, y - g.size().y / 2.0), g, color::ink());
    let a = egui::Align2::CENTER_CENTER;
    p.text(
        pos2(chevron, y),
        a,
        icon::CHEVRON_DOWN,
        theme::icon_font(),
        color::ink_2(),
    );
    r
}

/// The Font field: shows `family` (warning when `list` lacks it); clicked, a search over
/// `list` under it that previews the hovered or highlighted family in `family`. Enter or a
/// click keeps it, Esc or a click away restores the one before. True once `family` changed.
fn font_field(ui: &mut Ui, family: &mut String, list: &[String]) -> bool {
    use egui::Key;
    let id = egui::Id::new("inspector font");
    let missing = !family.is_empty() && !list.contains(family);
    let shown = match family.as_str() {
        "" => tr!("inspector.sans_serif").to_owned(),
        f => f.to_owned(),
    };
    let st = ui.data(|d| d.get_temp::<FontSearch>(id));
    let r = font_button(ui, &shown, missing, st.is_some());
    let r = match missing {
        true => r.on_hover_text(tr!("inspector.font_missing_tooltip", family = shown)),
        false => r,
    };
    let Some(mut st) = st else {
        if r.clicked() {
            let st = FontSearch {
                query: String::new(),
                at: 0,
                before: family.clone(),
                area: egui::Rect::NOTHING,
            };
            ui.data_mut(|d| d.insert_temp(id, st));
        }
        return false;
    };
    let matches = font_matches(list, &st.query);
    let (down, up, enter, esc, away) = ui.input_mut(|i| {
        let away = i.pointer.any_pressed()
            && i.pointer
                .press_origin()
                .is_some_and(|p| !st.area.contains(p) && !r.rect.contains(p));
        (
            i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
            i.key_pressed(Key::Enter),
            i.key_pressed(Key::Escape),
            away,
        )
    });
    // Clicking the field again closes the search, like a click away.
    let away = away || r.clicked();
    if down {
        st.at += 1;
    }
    if up {
        st.at = st.at.saturating_sub(1);
    }
    st.at = st.at.min(matches.len().saturating_sub(1));
    let mut pick = matches.get(st.at).copied();
    let mut clicked = false;
    let menu = egui::Area::new(id.with("menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(r.rect.left_bottom() + vec2(0.0, space::S4))
        .show(ui.ctx(), |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_width(r.rect.width().max(160.0));
                let q = ui.add(
                    egui::TextEdit::singleline(&mut st.query)
                        .hint_text(tr!("inspector.search_fonts")),
                );
                q.request_focus();
                if q.changed() {
                    st.at = 0;
                }
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .show(ui, |ui| {
                        for (i, f) in matches.iter().enumerate() {
                            let r = ui.selectable_label(i == st.at, *f);
                            if down || up {
                                if i == st.at {
                                    r.scroll_to_me(None);
                                }
                            } else if r.hovered() {
                                pick = Some(*f);
                            }
                            if r.clicked() {
                                (pick, clicked) = (Some(*f), true);
                            }
                        }
                    });
            });
        });
    st.area = menu.response.rect;
    let next = if esc || (away && !clicked) {
        st.before.clone()
    } else {
        pick.map_or(st.before.clone(), str::to_owned)
    };
    if enter || esc || away || clicked {
        ui.data_mut(|d| d.remove::<FontSearch>(id));
    } else {
        ui.data_mut(|d| d.insert_temp(id, st));
    }
    let changed = *family != next;
    *family = next;
    changed
}

/// The Text tab's sections by id, each with its rows: a style id (a [`text_names`] keyframe
/// name, or `font`, `weight` or `align`) and the id naming its label.
const TEXT_SECTIONS: [(&str, &[(&str, &str)]); 5] = [
    (
        "text",
        &[
            ("font", "font"),
            ("text_size", "text_size"),
            ("weight", "weight"),
            ("align", "align"),
            ("text_anchor", "text_anchor"),
        ],
    ),
    ("fill", &[("color", "color")]),
    (
        "outline",
        &[("outline", "outline_width"), ("outline_color", "color")],
    ),
    (
        "shadow",
        &[("shadow", "shadow_offset"), ("shadow_color", "color")],
    ),
    ("box", &[("box_padding", "padding"), ("box_color", "color")]),
];

/// Value column width under which a row's slider gives way, and X and Y drop their prefixes.
const SLIDER_ROOM: f32 = 112.0;

/// Whether text style `id` can animate.
fn keyable(id: &str) -> bool {
    text_names().any(|n| n == id)
}

/// Sets style `id` of `s` back to [`Text::default`]'s.
fn restore(s: &mut Text, id: &str) {
    let d = Text::default();
    match id {
        "font" => s.family = d.family,
        "weight" => s.weight = d.weight,
        "align" => s.align = d.align,
        n => text_set(s, n, text_get(&d, n).expect("a text name")),
    }
}

/// Whether style `id` of `s` is off its default.
fn off_default(s: &Text, id: &str) -> bool {
    let mut d = s.clone();
    restore(&mut d, id);
    d != *s
}

/// A collapsed section's summary of `s`, such as `4 px · #000000`.
fn text_summary(s: &Text, id: &str) -> String {
    let px = |n: f32| tr!("inspector.px", n = format!("{n:.0}"));
    let color = |c: [f64; 4]| match suite_ui::widgets::color_text(jvid_render::solid::rgba8(c)) {
        None => suite_ui::i18n::CATALOG.tr("color.none").to_owned(),
        Some((hex, None)) => format!("#{hex}"),
        Some((_, Some(alpha))) => alpha,
    };
    match id {
        "text" => {
            let family = match s.family.as_str() {
                "" => tr!("inspector.sans_serif"),
                f => f,
            };
            format!("{family} · {:.0}", s.size)
        }
        "fill" => color(s.color),
        "outline" => format!("{} · {}", px(s.outline), color(s.outline_color)),
        "shadow" => format!(
            "{:.0}, {:.0} · {}",
            s.shadow[0],
            s.shadow[1],
            color(s.shadow_color)
        ),
        _ => format!("{} · {}", px(s.box_padding), color(s.box_color)),
    }
}

/// A Text tab section head remembering whether it's open (first Text and Fill are), with
/// `summary` after the title while it's closed and the compact tool slots.
fn text_section(
    ui: &mut Ui,
    id: &str,
    changed: bool,
    key: bool,
    summary: &str,
) -> (bool, Option<Head>) {
    let title = caption(id);
    let sid = ui.id().with(("inspector text", id));
    let first = matches!(id, "text" | "fill");
    let mut open = ui.data_mut(|d| *d.get_temp_mut_or(sid, first));
    let rect = ui.add(SectionHeader::new(title, &mut open)).rect;
    ui.data_mut(|d| d.insert_temp(sid, open));
    if !open {
        let title_w = ui
            .painter()
            .layout_no_wrap(title.to_owned(), theme::body_strong(), color::ink())
            .size()
            .x;
        let x = rect.left() + size::ICON + space::S4 + title_w + space::S8;
        let w = rect.right() - COMPACT_TOOLS - space::S4 - x;
        let g = truncated(ui, summary, color::ink_3(), w);
        let pos = pos2(x, rect.center().y - g.size().y / 2.0);
        ui.painter().galley(pos, g, color::ink_3());
    }
    (open, head_tools(ui, rect, title, changed, key, true))
}

/// A number field, with a slider over `slider` (snapping to `step`) when the row has room.
fn number(
    ui: &mut Ui,
    v: &mut f64,
    (lo, hi): (f64, f64),
    (slider, step): ((f64, f64), Option<f64>),
    name: &str,
) -> bool {
    let wide = ui.available_width() >= SLIDER_ROOM;
    let speed = step.map_or(1.0, |s| s / 10.0);
    let f = DragField::new(v).range(lo..=hi).speed(speed).width(56.0);
    let mut changed = ui.add(f).changed();
    if wide {
        let mut s = Slider::new(v, slider.0..=slider.1, name);
        if let Some(step) = step {
            s = s.step(step);
        }
        changed |= ui.add(s).changed();
    }
    changed
}

/// An X and a Y field, which drop their prefixes for tooltips in a narrow row.
fn pair(ui: &mut Ui, v: &mut [f32; 2], (lo, hi): (f64, f64), decimals: usize, label: &str) -> bool {
    let wide = ui.available_width() >= SLIDER_ROOM;
    let w = (ui.available_width() - space::S4) / 2.0;
    let speed = 10f64.powi(-(decimals as i32)) / 2.0;
    let mut changed = false;
    for (v, axis) in v.iter_mut().zip(XY.iter().map(|l| l())) {
        let mut x = f64::from(*v);
        let mut f = DragField::new(&mut x)
            .range(lo..=hi)
            .speed(speed)
            .decimals(decimals)
            .width(w);
        if wide {
            f = f.prefix(axis);
        }
        if ui.add(f).on_hover_text(format!("{label} {axis}")).changed() {
            *v = x as f32;
            changed = true;
        }
    }
    changed
}

/// The fields of Text tab row `id` editing `s`; true once one changed it.
fn text_fields(ui: &mut Ui, s: &mut Text, id: &str, label: &str) -> bool {
    use jvid_core::model::Align;
    let mut num = |v: &mut f32, range, slider| {
        let mut x = f64::from(*v);
        let changed = number(ui, &mut x, range, slider, label);
        *v = x as f32;
        changed
    };
    match id {
        "text_size" => num(&mut s.size, (1.0, 1000.0), ((8.0, 400.0), None)),
        "outline" => num(&mut s.outline, (0.0, 32.0), ((0.0, 32.0), None)),
        "box_padding" => num(&mut s.box_padding, (0.0, 200.0), ((0.0, 200.0), None)),
        "weight" => {
            let mut w = f32::from(s.weight);
            let changed = num(&mut w, (100.0, 900.0), ((100.0, 900.0), Some(100.0)));
            s.weight = w.round() as u16;
            changed
        }
        "font" => font_field(ui, &mut s.family, jvid_render::text::families()),
        "align" => {
            let all = [Align::Left, Align::Center, Align::Right];
            let mut i = all.iter().position(|a| *a == s.align).unwrap_or(1);
            let names = [
                tr!("inspector.align_left"),
                tr!("inspector.align_center"),
                tr!("inspector.align_right"),
            ];
            let glyphs = [icon::ALIGN_LEFT, icon::ALIGN_CENTER, icon::ALIGN_RIGHT];
            let changed = ui.add(Segmented::icons(&mut i, &glyphs, &names)).changed();
            s.align = all[i];
            changed
        }
        "text_anchor" => pair(ui, &mut s.anchor, (0.0, 1.0), 2, label),
        "shadow" => pair(ui, &mut s.shadow, (-64.0, 64.0), 0, label),
        _ => {
            let c = match id {
                "color" => &mut s.color,
                "outline_color" => &mut s.outline_color,
                "shadow_color" => &mut s.shadow_color,
                _ => &mut s.box_color,
            };
            let mut rgba = jvid_render::solid::rgba8(*c);
            let changed = ui.add(ColorField::new(&mut rgba, caption(id))).changed();
            if changed {
                *c = rgba.map(|v| f64::from(v) / 255.0);
            }
            changed
        }
    }
}

/// A Text tab row of style `id` in `s`, on the label column: its fields, Reset once it's off
/// its default or animated, and the diamond for a style that animates. True once a field
/// changed `s`; the edit a tool made.
fn text_row(
    ui: &mut Ui,
    clips: &[&Clip],
    s: &mut Text,
    (id, label): (&'static str, &str),
    t: Time,
    index: u64,
    seek: &mut Option<Time>,
) -> (bool, Option<Edit>) {
    let names = [id];
    let keys = keyable(id) && !clips.is_empty();
    let anim = any_animated(clips, &names);
    let keyed = match () {
        _ if !anim => Keyed::Static,
        _ if all_keyed(clips, &names, t) => Keyed::Here,
        _ => Keyed::Animated,
    };
    let key_name = match keyed {
        Keyed::Static => tr!("inspector.animate", name = caption(id)),
        _ => tr!("inspector.key_here").to_owned(),
    };
    let tools = Tools {
        reset: (anim || off_default(s, id)).then_some(tr!("inspector.reset")),
        key: keys.then_some((keyed, &key_name)),
        compact: true,
        ..Tools::default()
    };
    let (changed, c) = prop_row(ui, label, tools, |ui| text_fields(ui, s, id, label));
    if c.reset {
        // shortcut: an animated style's Reset drops its keys, so the asset's value shows and
        // a second Reset restores the default; one step for both needs a compound command.
        if anim {
            return (false, Some((drop_keys(clips, &names), None)));
        }
        restore(s, id);
        return (true, None);
    }
    if !keys {
        return (changed, None);
    }
    let v = text_get(s, id).expect("a text name");
    let toggles = (
        || text_animate(clips, id, v, t),
        || text_toggle_key(clips, id, v, t),
    );
    (
        changed,
        key_menu(clips, &names, c, t, index, seek, true, toggles),
    )
}

/// Drops every clip's keyframes of `names`.
fn drop_keys(clips: &[&Clip], names: &[&str]) -> Command {
    rewrite(clips, |_, _, keys| {
        for n in names {
            keys.remove(*n);
        }
    })
}

/// The editors for a text asset's style at playhead `t`, with `clips` (all showing it) for
/// keyframes; the edit once anything changes, and where to seek. Animated properties key
/// every clip at `t`; the rest edit the asset, so every clip of it changes.
// shortcut: a per-clip caption needs its own asset; add a Duplicate action if users ask.
fn text_props(
    ui: &mut Ui,
    a: &MediaAsset,
    clips: &[&Clip],
    t: Time,
    seek: &mut Option<Time>,
) -> Option<Edit> {
    let base = a.text.clone()?;
    let anim = |name: &str| clips.iter().any(|c| animated(c, name));
    // `clips` share the asset, so the first stands for all at the playhead.
    let mut s = clips.first().map_or(base.clone(), |c| {
        let mut s = base.with(&c.text_keys(t));
        if let Some(Value::Color(f)) = c.value_at("color", t) {
            s.color = f;
        }
        s
    });
    let before = s.clone();
    // Each press starts its own undo step; typing and arrow keys until the next are one.
    let presses = egui::Id::new("inspector text presses");
    let pressed = ui.input(|i| i.pointer.any_pressed());
    let press = ui.data_mut(|d| {
        let n = d.get_temp_mut_or(presses, 0u64);
        *n += u64::from(pressed);
        *n
    });
    let gesture = TEXT_GESTURE ^ a.id.0 ^ (press << 20);
    // At least two lines, growing to eight, then it scrolls.
    let line = ui.fonts_mut(|f| f.row_height(&theme::body()));
    let content = egui::ScrollArea::vertical()
        .id_salt("inspector text box")
        .max_height(8.0 * line + 2.0 * space::S4)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut s.content)
                    .id(content_id())
                    .desired_width(f32::INFINITY)
                    .desired_rows(2)
                    .margin(vec2(space::S8, space::S4)),
            )
        });
    let mut changed = content.inner.changed();
    ui.add_space(space::S4);
    let mut tool = None;
    // Apart from the Video tab's rows, for the easing gesture.
    let mut index = 1u64 << 32;
    for (id, rows) in TEXT_SECTIONS {
        let keys: Vec<&str> = rows.iter().map(|r| r.0).filter(|n| keyable(n)).collect();
        let off = rows.iter().any(|(n, _)| off_default(&s, n) || anim(n));
        let summary = text_summary(&s, id);
        let (open, head) = text_section(ui, id, off, !clips.is_empty(), &summary);
        match head {
            Some(Head::Reset) => {
                let animated: Vec<&str> = keys.iter().copied().filter(|n| anim(n)).collect();
                // shortcut: as a row's Reset, animated styles first lose their keys.
                if animated.is_empty() {
                    rows.iter().for_each(|(n, _)| restore(&mut s, n));
                    changed = true;
                } else {
                    tool = Some((drop_keys(clips, &animated), None));
                }
            }
            Some(Head::Key) => {
                let cmd = rewrite(clips, |c, _, k| {
                    for n in &keys {
                        key(c, k, n, t, text_get(&s, n).expect("a text name"));
                    }
                });
                tool = Some((cmd, None));
            }
            None => {}
        }
        if !open {
            index += rows.len() as u64;
            continue;
        }
        for (n, label) in rows {
            index += 1;
            let (ch, e) = text_row(ui, clips, &mut s, (n, caption(label)), t, index, seek);
            changed |= ch;
            tool = e.or(tool);
        }
    }
    if tool.is_some() {
        return tool;
    }
    if !changed {
        return None;
    }
    let keyed: Vec<_> = text_names()
        .filter(|n| anim(n) && text_get(&before, n) != text_get(&s, n))
        .collect();
    if !keyed.is_empty() {
        let cmd = rewrite(clips, |c, _, keys| {
            for n in &keyed {
                key(c, keys, n, t, text_get(&s, n).expect("a text name"));
            }
        });
        return Some((cmd, Some(gesture ^ 1 << 40)));
    }
    // Animated values stay the asset's; the keys showed over them.
    for n in text_names().filter(|n| anim(n)) {
        text_set(&mut s, n, text_get(&base, n).expect("a text name"));
    }
    (s != base).then_some((Command::SetText { id: a.id, text: s }, Some(gesture)))
}

/// Animates text property `name` from `v`, or, if any clip animates it, drops its keys so the
/// asset's value shows again.
fn text_animate(clips: &[&Clip], name: &str, v: Value, t: Time) -> Command {
    let on = clips.iter().any(|c| animated(c, name));
    rewrite(clips, |c, _, keys| {
        if on {
            keys.remove(name);
        } else {
            key(c, keys, name, t, v);
        }
    })
}

/// Removes the keyframes of text property `name` at `t` if every clip has one there, else
/// keys `v` there. The last key gone, the asset's value shows again.
fn text_toggle_key(clips: &[&Clip], name: &str, v: Value, t: Time) -> Command {
    let all = all_keyed(clips, &[name], t);
    rewrite(clips, |c, _, keys| {
        if !all {
            key(c, keys, name, t, v);
            return;
        }
        let list = keys.get_mut(name).expect("all have a key");
        list.retain(|k| k.time != source(c, t));
        if list.is_empty() {
            keys.remove(name);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::history::History;
    use jvid_core::model::{AssetId, LinkId};

    fn clip(id: u64, start: i64) -> Clip {
        Clip {
            id: ClipId(id),
            asset: AssetId(1),
            source_in: Time(10),
            source_out: Time(110),
            start: Time(start),
            link: None::<LinkId>,
            params: BTreeMap::new(),
            keyframes: BTreeMap::new(),
            transform: Transform::default(),
            stream: Some(0),
            enabled: true,
            speed: 1.0,
            reverse: false,
            freeze: None,
        }
    }

    /// Applies the `SetClipProps` that `f` makes back onto the clips.
    fn apply(clips: &mut [Clip], f: impl FnOnce(&[&Clip]) -> Command) {
        let cmd = f(&clips.iter().collect::<Vec<_>>());
        let Command::SetClipProps(edits) = cmd else {
            panic!("{cmd:?}")
        };
        for (c, (id, x, keys)) in clips.iter_mut().zip(edits) {
            assert_eq!(c.id, id);
            (c.transform, c.keyframes) = (x, keys);
        }
    }

    fn refs(clips: &[Clip]) -> Vec<&Clip> {
        clips.iter().collect()
    }

    #[test]
    fn every_section_and_property_has_a_name_in_each_language() {
        let ids = SECTIONS
            .iter()
            .flat_map(|(id, props)| std::iter::once(*id).chain(props.iter().map(|p| p.name)))
            .chain(["effects", "speed", "text"])
            .chain(
                AUDIO_FX
                    .iter()
                    .flat_map(|(id, knobs)| std::iter::once(*id).chain(knobs.iter().map(|k| k.id))),
            )
            .chain(text_names())
            .chain(
                TEXT_SECTIONS
                    .iter()
                    .flat_map(|(id, rows)| std::iter::once(*id).chain(rows.iter().map(|r| r.1))),
            );
        for id in ids {
            let key = format!("inspector.{id}");
            for lang in suite_ui::i18n::LANGUAGES
                .iter()
                .filter(|l| !CATALOG.is_stub(l))
            {
                assert!(CATALOG.get_in(lang, &key).is_some(), "{lang} lacks {key}");
            }
        }
    }

    #[test]
    fn effect_params_keyframe_and_follow_their_effect_when_restacked() {
        let fx = |shader: &str| Effect {
            shader: shader.into(),
            params: [("amount".to_owned(), 0.5)].into(),
            enabled: true,
        };
        let mut clips = [clip(1, 0)];
        clips[0].transform.effects = vec![fx("a"), fx("b")];
        let b = Effect::key(1, "amount");
        apply(&mut clips, |c| toggle_animate(c, &[b.as_str()], Time(0)));
        apply(&mut clips, |c| set_field(c, &b, 0, |_| 1.0, Time(100)));
        // Halfway between the keys, give or take the clamp to the clip's last frame.
        let mid = |c: &Clip, i: usize| c.transform_at(Time(50)).effects[i].params["amount"];
        assert!((mid(&clips[0], 1) - 0.75).abs() < 0.01);
        assert_eq!(
            clips[0].transform_at(Time(50)).effects[0].params["amount"],
            0.5
        );

        // Moved to the top, its keys move with it; removing the other drops nothing of it.
        apply(&mut clips, |c| restack(c, |o| o.swap(0, 1)));
        assert_eq!(clips[0].transform.effects[0].shader, "b");
        assert!(clips[0].keyframes.contains_key("fx0.amount"));
        assert!((mid(&clips[0], 0) - 0.75).abs() < 0.01);
        apply(&mut clips, |c| {
            restack(c, |o| {
                o.remove(0);
            })
        });
        assert_eq!(clips[0].transform.effects, [fx("a")]);
        assert!(clips[0].keyframes.is_empty());
    }

    #[test]
    fn mask_keys_follow_their_mask_and_reset_keeps_the_shape() {
        let rect = Mask::new(MaskShape::Rectangle, vec![[0.0; 2], [100.0, 50.0]]);
        let ellipse = Mask::new(MaskShape::Ellipse, vec![[5.0, 5.0], [20.0, 20.0]]);
        let mut clips = [clip(1, 0)];
        clips[0].transform.masks = vec![rect.clone(), ellipse.clone()];
        let feather = Mask::key(1, "feather");
        apply(&mut clips, |c| {
            toggle_animate(c, &[feather.as_str()], Time(0))
        });
        apply(&mut clips, |c| {
            set_field(c, &feather, 0, |_| 10.0, Time(100))
        });
        let mid = |c: &Clip| c.transform_at(Time(50)).masks[0].feather;
        apply(&mut clips, |c| restack_masks(c, |o| o.swap(0, 1)));
        assert_eq!(clips[0].transform.masks[0].shape, MaskShape::Ellipse);
        assert!(clips[0].keyframes.contains_key("mask0.feather"));
        assert!((mid(&clips[0]) - 5.0).abs() < 0.1, "{}", mid(&clips[0]));
        // Reset goes back to a hard, opaque mask of the same shape, without keys.
        apply(&mut clips, |c| {
            edit_mask(c, 0, |m| {
                m.invert = true;
                m.opacity = 0.5;
            })
        });
        apply(&mut clips, |c| reset_mask(c, 0));
        assert_eq!(clips[0].transform.masks[0], ellipse);
        assert!(clips[0].keyframes.is_empty());
        // Removing mask 0 renumbers mask 1's keys down.
        let opacity = Mask::key(1, "opacity");
        apply(&mut clips, |c| {
            toggle_animate(c, &[opacity.as_str()], Time(0))
        });
        apply(&mut clips, |c| {
            restack_masks(c, |o| {
                o.remove(0);
            })
        });
        assert_eq!(clips[0].transform.masks, [rect]);
        let keys: Vec<&String> = clips[0].keyframes.keys().collect();
        assert_eq!(keys, ["mask0.opacity"]);
    }

    #[test]
    fn bypass_keeps_settings_and_keys_and_blocks_move_into_gaps() {
        let fx = |shader: &str| Effect {
            shader: shader.into(),
            params: [("amount".to_owned(), 0.5)].into(),
            enabled: true,
        };
        let mut clips = [clip(1, 0), clip(2, 0)];
        for c in &mut clips {
            c.transform.effects = vec![fx("a"), fx("b")];
        }
        let b = Effect::key(1, "amount");
        apply(&mut clips, |c| toggle_animate(c, &[b.as_str()], Time(0)));
        let before = clips.clone();
        apply(&mut clips, |c| bypass(c, 1));
        for (c, was) in clips.iter().zip(&before) {
            assert!(!c.transform.effects[1].enabled);
            assert_eq!(
                c.transform.effects[1].params,
                was.transform.effects[1].params
            );
            assert_eq!(c.keyframes, was.keyframes);
        }
        apply(&mut clips, |c| bypass(c, 1));
        assert_eq!(clips, before);

        let order = |from, to| {
            let mut o = vec![0, 1, 2];
            move_to(&mut o, from, to);
            o
        };
        // Move Down drops into gap i + 2, Move Up into gap i - 1.
        assert_eq!(order(0, 2), [1, 0, 2]);
        assert_eq!(order(2, 1), [0, 2, 1]);
        assert_eq!(order(0, 3), [1, 2, 0]);
        assert_eq!(order(2, 0), [2, 0, 1]);
    }

    #[test]
    fn text_style_animates_per_clip_and_stops() {
        let mut clips = [clip(1, 0), clip(2, 50)];
        apply(&mut clips, |c| {
            text_animate(c, "outline", Value::Scalar(3.0), Time(60))
        });
        assert_eq!(clips[0].keyframes["outline"][0].time, Time(70));
        assert_eq!(clips[1].keyframes["outline"][0].time, Time(20));
        let t = Text::default().with(&clips[1].text_keys(Time(60)));
        assert_eq!(t.outline, 3.0, "the key shows over the asset");
        apply(&mut clips, |c| {
            text_animate(c, "outline", Value::Scalar(3.0), Time(60))
        });
        assert!(clips.iter().all(|c| c.keyframes.is_empty()));
    }

    #[test]
    fn easing_applies_to_every_clips_key_and_changes_playback() {
        let mut clips = [clip(1, 0), clip(2, 50)];
        apply(&mut clips, |c| toggle_animate(c, &["opacity"], Time(50)));
        apply(&mut clips, |c| {
            set_field(c, "opacity", 0, |_| 0.0, Time(80))
        });
        let before = clips[1].transform_at(Time(65)).opacity;
        apply(&mut clips, |c| {
            set_easing(c, &["opacity"], Time(50), Easing::Hold)
        });
        assert_eq!(clips[0].keyframes["opacity"][0].easing, Easing::Hold);
        assert_eq!(clips[1].keyframes["opacity"][0].easing, Easing::Hold);
        // Other keys keep theirs.
        assert_eq!(clips[0].keyframes["opacity"][1].easing, Easing::Linear);
        assert_ne!(clips[1].transform_at(Time(65)).opacity, before);
    }

    #[test]
    fn field_edits_keep_each_clips_other_field() {
        let mut clips = [clip(1, 0), clip(2, 0)];
        clips[1].transform.position = [5.0, 7.0];
        apply(&mut clips, |c| {
            set_field(c, "position", 0, |_| 3.0, Time(0))
        });
        assert_eq!(clips[0].transform.position, [3.0, 0.0]);
        assert_eq!(clips[1].transform.position, [3.0, 7.0]);
    }

    #[test]
    fn animated_edits_key_at_the_playhead_in_source_time() {
        let mut clips = [clip(1, 100)];
        apply(&mut clips, |c| toggle_animate(c, &["opacity"], Time(120)));
        assert_eq!(clips[0].keyframes["opacity"][0].time, Time(30));
        apply(&mut clips, |c| {
            set_field(c, "opacity", 0, |_| 0.25, Time(150))
        });
        let keys = &clips[0].keyframes["opacity"];
        assert_eq!(keys.len(), 2);
        assert_eq!(clips[0].transform.opacity, 1.0, "static value untouched");
        assert_eq!(clips[0].transform_at(Time(150)).opacity, 0.25);
        assert_eq!(
            neighbour(&refs(&clips), &["opacity"], Time(150), -1),
            Some(Time(120))
        );
        assert_eq!(
            neighbour(&refs(&clips), &["opacity"], Time(120), 1),
            Some(Time(150))
        );
        assert_eq!(neighbour(&refs(&clips), &["opacity"], Time(150), 1), None);
    }

    #[test]
    fn removing_the_last_key_holds_its_value() {
        let mut clips = [clip(1, 0)];
        apply(&mut clips, |c| toggle_animate(c, &["rotation"], Time(5)));
        apply(&mut clips, |c| {
            set_field(c, "rotation", 0, |_| 45.0, Time(5))
        });
        assert!(key_at(&clips[0], "rotation", Time(5)));
        apply(&mut clips, |c| toggle_key(c, &["rotation"], Time(5)));
        assert!(!animated(&clips[0], "rotation"));
        assert_eq!(clips[0].transform.rotation, 45.0);
    }

    #[test]
    fn key_toggle_adds_where_any_clip_lacks_one() {
        let mut clips = [clip(1, 0), clip(2, 0)];
        apply(&mut clips, |c| toggle_animate(c, &["scale"], Time(0)));
        apply(&mut clips, |c| toggle_key(c, &["scale"], Time(0)));
        assert!(!animated(&clips[0], "scale") && !animated(&clips[1], "scale"));
        apply(&mut clips[..1], |c| toggle_animate(c, &["scale"], Time(0)));
        apply(&mut clips, |c| toggle_key(c, &["scale"], Time(0)));
        assert!(clips.iter().all(|c| key_at(c, "scale", Time(0))));
    }

    #[test]
    fn blend_sets_every_clip_and_keeps_the_rest() {
        let mut clips = [clip(1, 0), clip(2, 0)];
        clips[1].transform.opacity = 0.5;
        apply(&mut clips, |c| set_blend(c, Blend::Screen));
        assert!(clips.iter().all(|c| c.transform.blend == Blend::Screen));
        assert_eq!(clips[1].transform.opacity, 0.5);
        let names = Blend::ALL.map(|b| format!("{b:?}"));
        assert_eq!(names, blends(), "labels follow Blend::ALL");
    }

    #[test]
    fn lut_sets_and_clears_every_clip() {
        let mut clips = [clip(1, 0), clip(2, 0)];
        clips[1].transform.lut_intensity = 0.5;
        apply(&mut clips, |c| set_lut(c, Some("a.cube".into())));
        assert!(
            clips
                .iter()
                .all(|c| c.transform.lut.as_deref() == Some("a.cube"))
        );
        assert_eq!(clips[1].transform.lut_intensity, 0.5);
        apply(&mut clips, |c| set_lut(c, None));
        assert!(clips.iter().all(|c| c.transform.lut.is_none()));
    }

    #[test]
    fn reset_clears_keys_and_value() {
        let mut clips = [clip(1, 0)];
        clips[0].transform.crop[2] = 9.0;
        apply(&mut clips, |c| toggle_animate(c, &["crop_right"], Time(0)));
        apply(&mut clips, |c| reset(c, &["crop_right"]));
        assert_eq!(clips[0].transform, Transform::default());
        assert!(clips[0].keyframes.is_empty());
    }

    #[test]
    fn keys_stay_inside_the_clip() {
        // Source 10..110 at timeline 100..200.
        let mut clips = [clip(1, 100)];
        apply(&mut clips, |c| toggle_animate(c, &["rotation"], Time(50)));
        apply(&mut clips, |c| {
            set_field(c, "rotation", 0, |_| 9.0, Time(500))
        });
        let times: Vec<_> = clips[0].keyframes["rotation"]
            .iter()
            .map(|k| k.time)
            .collect();
        assert_eq!(times, [Time(10), Time(109)]);
        apply(&mut clips, |c| key_all(c, &["opacity"], Time(0)));
        assert_eq!(clips[0].keyframes["opacity"][0].time, Time(10));
    }

    #[test]
    fn scale_keeps_its_ratio() {
        let mut x = Transform {
            scale: [2.0, 1.0],
            ..Transform::default()
        };
        set(&mut x, "scale", Value::Scalar(1.0));
        assert_eq!(x.scale, [1.0, 0.5]);
    }

    #[test]
    fn mixed_drags_move_each_clip_by_the_delta() {
        let mut clips = [clip(1, 0), clip(2, 0)];
        clips[1].transform.opacity = 0.5;
        apply(&mut clips, |c| {
            set_field(c, "opacity", 0, |v| (v - 0.25).clamp(0.0, 1.0), Time(0))
        });
        assert_eq!(clips[0].transform.opacity, 0.75);
        assert_eq!(clips[1].transform.opacity, 0.25);
    }

    fn harness() -> egui_kittest::Harness<'static, History> {
        harness_on(Pane::Video)
    }

    /// The inspector's `pane` over a real history, with one video clip selected.
    fn harness_on(pane: Pane) -> egui_kittest::Harness<'static, History> {
        harness_of(TrackKind::Video, pane)
    }

    /// Like [`harness_on`], with the one clip on a track of `kind`.
    fn harness_of(kind: TrackKind, pane: Pane) -> egui_kittest::Harness<'static, History> {
        use jvid_core::model::{NewClip, ProbeInfo};
        use jvid_core::time::FLICKS_PER_SECOND as S;
        let mut h = History::new(Project::default());
        let _ = h.apply(Command::Import(vec![(
            "/nonexistent/a.mp4".into(),
            ProbeInfo {
                duration: Time(60 * S),
                width: Some(1920),
                height: Some(1080),
                ..Default::default()
            },
        )]));
        let asset = h.project().assets()[0].id;
        let _ = h.apply(Command::AddTrack {
            kind,
            name: "V1".into(),
        });
        let track = h.project().sequence().tracks()[0].id;
        let _ = h.apply(Command::AddClip {
            track,
            clip: NewClip {
                asset,
                source_in: Time(0),
                source_out: Time(10 * S),
                start: Time(0),
                link: None,
            },
        });
        let id = h.project().sequence().tracks()[0].clips()[0].id;
        let mut fresh = true;
        let mut harness = egui_kittest::Harness::builder()
            .with_size([340.0, 420.0])
            .wgpu()
            .build_ui_state(
                move |ui, h: &mut History| {
                    if suite_ui::test_support::install_theme(ui, &mut fresh) {
                        return;
                    }
                    ui.painter().rect_filled(ui.max_rect(), 0.0, color::panel());
                    let sel = Selection::new(h.project(), &HashSet::from([id]), None);
                    let (edit, _) = show(ui, &sel, Some(pane), Time(S));
                    match edit {
                        Some((c, Some(k))) => h.apply_coalesced(k, c).unwrap(),
                        Some((c, None)) => h.apply(c).unwrap(),
                        None => jvid_core::history::Created::Nothing,
                    };
                },
                h,
            );
        harness.run();
        harness
    }

    fn asset(text: bool) -> MediaAsset {
        MediaAsset {
            id: AssetId(1),
            path: "/nonexistent/a.mp4".into(),
            probe: None,
            name: None,
            in_point: None,
            out_point: None,
            solid: None,
            text: text.then(Text::default),
            sequence: None,
        }
    }

    #[test]
    fn tabs_follow_the_selection() {
        use Pane::*;
        let (v, a, footage, text) = (clip(1, 0), clip(2, 0), asset(false), asset(true));
        let sel = |clips: Vec<&Clip>, audio: Vec<&Clip>, shared| {
            Selection {
                clips,
                audio,
                shared,
                same: true,
                fitted: vec![],
            }
            .panes()
        };
        assert_eq!(
            sel(vec![&v], vec![&a], Some(&footage)),
            [Video, Audio, Effects]
        );
        assert_eq!(sel(vec![], vec![&a], None), [Audio]);
        assert_eq!(sel(vec![&v], vec![], Some(&text)), [Text, Video, Effects]);
        assert_eq!(sel(vec![], vec![], Some(&text)), [Text]);
        assert_eq!(sel(vec![], vec![], None), []);
    }

    #[test]
    fn the_chosen_tab_stays_while_the_selection_has_it() {
        use Pane::*;
        let ctx = egui::Context::default();
        assert_eq!(active(&ctx, &[Video, Audio, Effects]), 0);
        choose(&ctx, Effects);
        assert_eq!(active(&ctx, &[Text, Video, Effects]), 2);
        // Audio-only falls back to its one tab, and Effects comes back with the video.
        assert_eq!(active(&ctx, &[Audio]), 0);
        assert_eq!(active(&ctx, &[Video, Audio, Effects]), 2);
    }

    #[test]
    fn the_effects_tab_counts_a_shared_stack() {
        let fx = |shaders: &[&str]| {
            let mut c = clip(1, 0);
            c.transform.effects = shaders
                .iter()
                .map(|s| Effect {
                    shader: s.to_string(),
                    params: BTreeMap::new(),
                    enabled: true,
                })
                .collect();
            c
        };
        let label = |clips: &[&Clip]| {
            Selection {
                clips: clips.to_vec(),
                audio: vec![],
                shared: None,
                same: true,
                fitted: vec![],
            }
            .label(Pane::Effects)
        };
        let (none, two, other) = (fx(&[]), fx(&["vignette", "film_grain"]), fx(&["vignette"]));
        assert_eq!(label(&[&none]), "Effects");
        assert_eq!(label(&[&two]), "Effects 2");
        assert_eq!(label(&[&two, &two]), "Effects 2");
        assert_eq!(label(&[&two, &other]), "Effects");
    }

    #[test]
    fn row_buttons_leave_a_gutter_for_the_scroll_bar() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let h = harness();
        let bar = h.ctx.global_style().spacing.scroll.allocated_width();
        for b in h.get_all_by_label_contains("Keyframe All") {
            assert!(
                b.rect().right() <= 340.0 - bar - space::S4 + 0.5,
                "{:?}",
                b.rect()
            );
            assert!(b.rect().width() >= size::SMALL_CONTROL);
        }
    }

    #[test]
    fn snapshot_and_separate_drags_are_separate_steps() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness();
        h.snapshot("inspector_clip");
        let rotation = |h: &egui_kittest::Harness<'_, History>| {
            h.state().project().sequence().tracks()[0].clips()[0]
                .transform
                .rotation
        };
        let field = h.get_all_by_label("").nth(1).unwrap().rect().center();
        for _ in 0..2 {
            h.hover_at(field);
            h.run();
            h.drag_at(field);
            h.run();
            h.hover_at(field + vec2(10.0, 0.0));
            h.run();
            h.drop_at(field + vec2(10.0, 0.0));
            h.run();
        }
        assert_eq!(rotation(&h), 20.0);
        assert!(h.state_mut().undo());
        assert_eq!(rotation(&h), 10.0);
        assert!(h.state_mut().undo());
        assert_eq!(rotation(&h), 0.0);
    }

    /// The scale row, as in [`SECTIONS`].
    fn scale() -> &'static Prop<'static> {
        SECTIONS[0].1.iter().find(|p| p.name == "scale").unwrap()
    }

    #[test]
    fn reset_shows_once_a_value_reads_off_its_default_or_animates() {
        let mut clips = [clip(1, 0)];
        assert!(!differs(&refs(&clips), scale(), Time(0)));
        // Under what the field shows still reads as 100%.
        clips[0].transform.scale = [1.000_1; 2];
        assert!(!differs(&refs(&clips), scale(), Time(0)));
        clips[0].transform.scale = [1.5; 2];
        assert!(differs(&refs(&clips), scale(), Time(0)));
        apply(&mut clips, |c| reset_prop(c, scale()));
        assert_eq!(clips[0].transform, Transform::default());
        apply(&mut clips, |c| toggle_animate(c, &["scale"], Time(0)));
        assert!(differs(&refs(&clips), scale(), Time(0)));
    }

    #[test]
    fn the_crop_row_keys_and_resets_its_four_sides_together() {
        let crop = SECTIONS[0].1.iter().find(|p| p.name == "crop").unwrap();
        let names = crop.names();
        let mut clips = [clip(1, 0)];
        clips[0].transform.crop[1] = 8.0;
        assert!(differs(&refs(&clips), crop, Time(0)));
        apply(&mut clips, |c| toggle_animate(c, &names, Time(0)));
        assert!(all_keyed(&refs(&clips), &names, Time(0)));
        apply(&mut clips, |c| {
            set_field(c, "crop_top", 0, |_| 4.0, Time(50))
        });
        assert_eq!(
            neighbour(&refs(&clips), &names, Time(50), -1),
            Some(Time(0))
        );
        apply(&mut clips, |c| reset_prop(c, crop));
        assert!(clips[0].keyframes.is_empty());
        assert_eq!(clips[0].transform, Transform::default());
    }

    /// Every widget's role, label and rect.
    fn widgets(h: &egui_kittest::Harness<'_, History>) -> Vec<String> {
        use egui_kittest::kittest::{NodeT, Queryable};
        h.query_all_by(|_| true)
            .map(|n| {
                let n = n.accesskit_node();
                format!("{:?} {:?} {:?}", n.role(), n.label(), n.bounding_box())
            })
            .collect()
    }

    #[test]
    fn changing_scale_shows_reset_on_its_row_and_head_and_moves_nothing() {
        use egui_kittest::kittest::{NodeT, Queryable};
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness();
        assert!(h.query_all_by_label_contains("Reset").next().is_none());
        let before = widgets(&h);
        let id = h.state().project().sequence().tracks()[0].clips()[0].id;
        let x = Transform {
            scale: [1.5; 2],
            ..Transform::default()
        };
        let clip = h.state().project().sequence().clip(id).unwrap().1.clone();
        h.state_mut()
            .apply(set_props(&[&clip], &["scale"], &x, Time(0)))
            .unwrap();
        h.run();
        let resets: Vec<_> = h
            .query_all_by_label_contains("Reset")
            .map(|n| n.accesskit_node().label().unwrap_or_default())
            .collect();
        assert_eq!(resets, ["Reset Transform", "Reset"]);
        let after = widgets(&h);
        let moved: Vec<_> = before.iter().filter(|w| !after.contains(w)).collect();
        assert!(moved.is_empty(), "{moved:#?}");
    }

    #[test]
    fn the_diamond_animates_then_keys_and_shows_the_arrows() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness();
        assert_eq!(h.query_all_by_label("Animate Scale").count(), 1);
        assert!(h.query_by_label("Previous Keyframe").is_none());
        h.get_by_label("Animate Scale").click();
        h.run();
        let clip = |h: &egui_kittest::Harness<'_, History>| {
            h.state().project().sequence().tracks()[0].clips()[0].clone()
        };
        assert!(key_at(
            &clip(&h),
            "scale",
            Time(jvid_core::time::FLICKS_PER_SECOND)
        ));
        assert_eq!(h.query_all_by_label("Previous Keyframe").count(), 1);
        assert_eq!(h.query_all_by_label("Next Keyframe").count(), 1);
        h.snapshot("inspector_animated");
        // Keyed at the playhead, the same one diamond removes the key.
        h.get_by_label("Keyframe at Playhead").click();
        h.run();
        assert!(!animated(&clip(&h), "scale"));
    }

    #[test]
    fn the_speed_diamond_keys_the_ramp_in_one_step_each() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness();
        // Speed is the Video tab's last section.
        h.set_size(vec2(340.0, 1600.0));
        h.run();
        let before = h.state().project().clone();
        let clip = |h: &egui_kittest::Harness<'_, History>| {
            h.state().project().sequence().tracks()[0].clips()[0].clone()
        };
        h.get_by_label("Animate Speed").click();
        h.run();
        // Keyed at the playhead's source time, at the speed the clip had.
        let keys = clip(&h).ramp().unwrap().to_vec();
        let s = Time(jvid_core::time::FLICKS_PER_SECOND);
        assert_eq!((keys.len(), keys[0].time), (1, s));
        assert_eq!(keys[0].value, Value::Scalar(1.0));
        assert_eq!(h.query_all_by_label("Next Keyframe").count(), 1);
        h.get_by_label("Keyframe at Playhead").click();
        h.run();
        assert_eq!((clip(&h).ramp(), clip(&h).speed), (None, 1.0));
        assert!(h.state_mut().undo() && h.state_mut().undo());
        assert_eq!(h.state().project(), &before);
    }

    #[test]
    fn a_slider_drag_is_one_undo_step() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness();
        let scale = |h: &egui_kittest::Harness<'_, History>| {
            h.state().project().sequence().tracks()[0].clips()[0]
                .transform
                .scale[0]
        };
        let rect = h.get_by_role_and_label(Role::Slider, "Scale").rect();
        let from = egui::pos2(rect.left() + rect.width() / 4.0, rect.center().y);
        h.hover_at(from);
        h.run();
        h.drag_at(from);
        h.run();
        for dx in [10.0, 20.0, 30.0] {
            h.hover_at(from + vec2(dx, 0.0));
            h.run();
        }
        h.drop_at(from + vec2(30.0, 0.0));
        h.run();
        assert!(scale(&h) > 1.0, "{}", scale(&h));
        assert!(h.state_mut().undo());
        assert_eq!(scale(&h), 1.0);
    }

    /// #263: a band drag, a preset and a double-click reset are one undo step each; Custom shows
    /// once the settings match no preset.
    #[test]
    fn audio_fx_edit_in_one_step_and_bypass() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness_of(TrackKind::Audio, Pane::Audio);
        // A 288px Inspector's content.
        h.set_size(vec2(264.0, 640.0));
        h.run();
        let fx = |h: &egui_kittest::Harness<'_, History>| {
            h.state().project().sequence().tracks()[0].clips()[0]
                .transform
                .audio_fx
        };
        let from = h
            .get_by_role_and_label(Role::Slider, "1 kHz")
            .rect()
            .center();
        h.drag_at(from);
        h.run();
        for dy in [10.0, 20.0] {
            h.hover_at(from - vec2(0.0, dy));
            h.run();
        }
        h.drop_at(from - vec2(0.0, 20.0));
        h.run();
        let eq = fx(&h).eq;
        assert!(eq.bands[5] > 0.0 && eq.bands[5] % 0.5 == 0.0, "{eq:?}");
        assert_eq!(eq.bands.iter().filter(|b| **b != 0.0).count(), 1);
        h.get_by_role_and_label(Role::ComboBox, "Custom");
        assert!(h.state_mut().undo());
        assert_eq!(fx(&h), AudioFx::default());
        h.run();
        // Each preset is one step, keeping the block on.
        for (combo, pick) in [("Flat", "Podcast"), ("None", "Voice")] {
            h.get_by_role_and_label(Role::ComboBox, combo).click();
            h.run();
            h.get_by_role_and_label(Role::Button, pick).click();
            h.run();
        }
        assert_eq!(fx(&h).eq.bands, EQ_PRESETS[2].1);
        assert_eq!(fx(&h).compressor.ratio, 3.0);
        assert!(h.state_mut().undo());
        assert_eq!(fx(&h).eq.bands, EQ_PRESETS[2].1);
        assert_eq!(fx(&h).compressor, AudioFx::default().compressor);
        assert!(h.state_mut().redo());
        h.run();
        h.get_by_role_and_label(Role::ComboBox, "Voice");
        h.remove_cursor();
        h.run();
        h.snapshot("inspector_audio_fx_288");
        h.set_size(vec2(216.0, 640.0));
        h.run();
        h.snapshot("inspector_audio_fx_240");
        // Double-clicking 31 Hz resets it alone. The harness steps 0.25 s per event.
        h.ctx
            .options_mut(|o| o.input_options.max_double_click_delay = 1.0);
        let at = h
            .get_by_role_and_label(Role::Slider, "31 Hz")
            .rect()
            .center();
        h.hover_at(at);
        for pressed in [true, false, true, false] {
            h.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        h.run();
        let eq = fx(&h).eq;
        assert_eq!(eq.bands[0], 0.0);
        assert_eq!(eq.bands[1..], EQ_PRESETS[2].1[1..]);
        assert!(h.state_mut().undo());
        assert_eq!(fx(&h).eq.bands, EQ_PRESETS[2].1);
        // EQ first, then Compressor.
        h.get_all_by_label("Bypass").nth(1).unwrap().click();
        h.run();
        assert!(fx(&h).eq.enabled && !fx(&h).compressor.enabled);
        assert_eq!(h.get_all_by_label("Turn On").count(), 1);
    }

    /// [`harness`] with Vignette then Film Grain on its clip, tall enough to show them.
    fn fx_harness() -> egui_kittest::Harness<'static, History> {
        // One folder per test thread, so parallel tests never scan one another's half-written.
        let tmp = std::env::temp_dir().join(format!(
            "jvid-fx-stack-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        crate::effects::scan_for_test(&tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        let mut h = harness_on(Pane::Effects);
        h.set_size(vec2(340.0, 1000.0));
        let ids = HashSet::from([h.state().project().sequence().tracks()[0].clips()[0].id]);
        for stem in ["vignette", "film_grain"] {
            let cmd = add_effect(h.state().project(), &ids, stem).unwrap();
            h.state_mut().apply(cmd).unwrap();
        }
        h.run();
        h
    }

    fn stack(h: &egui_kittest::Harness<'_, History>) -> Vec<(String, bool)> {
        let c = &h.state().project().sequence().tracks()[0].clips()[0];
        let fx = c.transform.effects.iter();
        fx.map(|e| (e.shader.clone(), e.enabled)).collect()
    }

    fn on(shaders: &[&str]) -> Vec<(String, bool)> {
        shaders.iter().map(|s| (s.to_string(), true)).collect()
    }

    #[test]
    fn the_eye_bypasses_an_effect_in_place_and_undoes() {
        use egui_kittest::kittest::{NodeT, Queryable};
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = fx_harness();
        let rects = |h: &egui_kittest::Harness<'_, History>| -> Vec<_> {
            let nodes = h.query_all_by(|_| true);
            nodes.map(|n| n.accesskit_node().bounding_box()).collect()
        };
        let before = rects(&h);
        h.get_all_by_label("Bypass").next().unwrap().click();
        h.run();
        let params = |h: &egui_kittest::Harness<'_, History>| {
            h.state().project().sequence().tracks()[0].clips()[0]
                .transform
                .effects[0]
                .params
                .clone()
        };
        assert_eq!(stack(&h)[0], ("vignette".into(), false));
        assert_eq!(params(&h)["strength"], 0.5);
        assert_eq!(h.query_all_by_label("Turn On").count(), 1);
        // Same widgets in the same places, only the eye's name and the greyed rows change.
        assert_eq!(rects(&h), before);
        h.snapshot("inspector_fx_bypassed");
        assert!(h.state_mut().undo());
        h.run();
        assert_eq!(stack(&h), on(&["vignette", "film_grain"]));
    }

    #[test]
    fn the_more_menu_disables_moves_at_the_ends_and_moves_in_one_step() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::{NodeT, Queryable};
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = fx_harness();
        // Each menu row's label, whether it's enabled, and its offset from the first row.
        let menu = |h: &mut egui_kittest::Harness<'_, History>, block: usize| {
            h.get_all_by_label("More").nth(block).unwrap().click();
            h.run();
            let labels = ["Move Up", "Move Down", "Reset", "Remove"];
            let rows: Vec<_> = labels
                .iter()
                .map(|l| h.get_by_role_and_label(Role::Button, l))
                .map(|n| (n.rect(), !n.accesskit_node().is_disabled()))
                .collect();
            let top = rows[0].0.min;
            let rows: Vec<_> = rows
                .iter()
                .map(|(r, on)| (r.translate(-top.to_vec2()), *on))
                .collect();
            h.key_press(egui::Key::Escape);
            h.run();
            rows
        };
        let (first, last) = (menu(&mut h, 0), menu(&mut h, 1));
        let enabled = |rows: &[(Rect, bool)]| rows.iter().map(|r| r.1).collect::<Vec<_>>();
        assert_eq!(enabled(&first), [false, true, false, true]);
        assert_eq!(enabled(&last), [true, false, false, true]);
        let at = |rows: &[(Rect, bool)]| rows.iter().map(|r| r.0).collect::<Vec<_>>();
        assert_eq!(at(&first), at(&last));

        h.get_all_by_label("More").next().unwrap().click();
        h.run();
        h.get_by_role_and_label(Role::Button, "Move Down").click();
        h.run();
        assert_eq!(stack(&h), on(&["film_grain", "vignette"]));
        assert!(h.state_mut().undo());
        assert_eq!(stack(&h), on(&["vignette", "film_grain"]));
    }

    #[test]
    fn dragging_a_header_moves_its_block_in_one_step() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = fx_harness();
        let from = h.get_by_label("Vignette").rect().center();
        let to = pos2(from.x, h.get_by_label("Add Effect").rect().top() - 1.0);
        h.hover_at(from);
        h.run();
        h.drag_at(from);
        h.run();
        for y in [from.y + 10.0, (from.y + to.y) / 2.0, to.y] {
            h.hover_at(pos2(from.x, y));
            h.run();
        }
        h.drop_at(to);
        h.run();
        assert_eq!(stack(&h), on(&["film_grain", "vignette"]));
        assert!(h.state_mut().undo());
        assert_eq!(stack(&h), on(&["vignette", "film_grain"]));
    }

    #[test]
    fn removing_a_block_moves_nothing_above_it() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::{NodeT, Queryable};
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = fx_harness();
        let more = h.get_all_by_label("More").nth(1).unwrap();
        let cut = more.rect().top() - space::S4;
        // Every widget above the last block's header, by role, label and rect.
        let above = |h: &egui_kittest::Harness<'_, History>| {
            let nodes = h.query_all_by(|n| n.bounding_box().is_some_and(|b| (b.y1 as f32) < cut));
            let nodes = nodes.map(|n| n.accesskit_node());
            nodes
                .map(|n| format!("{:?} {:?} {:?}", n.role(), n.label(), n.bounding_box()))
                .collect::<Vec<_>>()
        };
        let before = above(&h);
        more.click();
        h.run();
        h.get_by_role_and_label(Role::Button, "Remove").click();
        h.run();
        assert_eq!(stack(&h), on(&["vignette"]));
        assert_eq!(above(&h), before);
    }

    #[test]
    fn masks_add_edit_and_remove_in_one_step_each() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness();
        h.set_size(vec2(340.0, 1000.0));
        h.run();
        let masks = |h: &egui_kittest::Harness<'_, History>| {
            let c = &h.state().project().sequence().tracks()[0].clips()[0];
            c.transform.masks.clone()
        };
        h.get_by_label("Add Mask").click();
        h.run();
        h.get_by_role_and_label(Role::Button, "Ellipse").click();
        h.run();
        // Half the 1920x1080 clip, centred.
        let ellipse = Mask::new(MaskShape::Ellipse, vec![[0.0; 2], [960.0, 540.0]]);
        assert_eq!(masks(&h), std::slice::from_ref(&ellipse));
        h.get_by_label("Invert").click();
        h.run();
        assert!(masks(&h)[0].invert);
        h.snapshot("inspector_masks");
        assert!(h.state_mut().undo());
        assert_eq!(masks(&h), std::slice::from_ref(&ellipse));
        // Mode and a Feather drag are a step each.
        h.get_by_role_and_label(Role::ComboBox, "Add").click();
        h.run();
        h.get_by_role_and_label(Role::Button, "Subtract").click();
        h.run();
        assert_eq!(masks(&h)[0].mode, MaskMode::Subtract);
        assert!(h.state_mut().undo());
        h.run();
        let rect = h.get_by_role_and_label(Role::Slider, "Feather").rect();
        let from = egui::pos2(rect.left() + 2.0, rect.center().y);
        h.hover_at(from);
        h.run();
        h.drag_at(from);
        h.run();
        for dx in [10.0, 20.0, 30.0] {
            h.hover_at(from + vec2(dx, 0.0));
            h.run();
        }
        h.drop_at(from + vec2(30.0, 0.0));
        h.run();
        assert!(masks(&h)[0].feather > 0.0);
        assert!(h.state_mut().undo());
        assert_eq!(masks(&h), [ellipse]);
        h.run();
        h.get_by_label("More").click();
        h.run();
        h.get_by_role_and_label(Role::Button, "Remove").click();
        h.run();
        assert!(masks(&h).is_empty());
        assert!(h.state_mut().undo());
        assert!(h.state_mut().undo());
        assert!(masks(&h).is_empty(), "adding was one step too");
    }

    #[test]
    fn add_effect_searches_names_and_descriptions() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = fx_harness();
        h.get_by_label("Add Effect").click();
        h.run();
        assert!(
            h.query_by_role_and_label(Role::Button, "Chromatic Aberration")
                .is_some()
        );
        h.snapshot("inspector_fx_add");
        // Only Film Grain's description mentions midtones.
        h.get_by_role(Role::TextInput).type_text("midtones");
        h.run();
        assert!(
            h.query_by_role_and_label(Role::Button, "Chromatic Aberration")
                .is_none()
        );
        h.get_by_role_and_label(Role::Button, "Film Grain").click();
        h.run();
        assert_eq!(stack(&h), on(&["vignette", "film_grain", "film_grain"]));
    }

    /// The mockup's sample: a long three-line title, Size 120, a 4px outline, a 3, 3 shadow
    /// and a 50% black box.
    fn sample() -> Text {
        Text {
            content: "TRIPLE KILL\nclutch_round_3 — ranked, season 4 finals, map 2 overtime\nGG WP"
                .into(),
            size: 120.0,
            outline: 4.0,
            shadow: [3.0, 3.0],
            box_color: [0.0, 0.0, 0.0, 0.5],
            ..Text::default()
        }
    }

    /// The Text tab `width` wide (the Inspector's body: its width less the panel's insets),
    /// for one clip of a title in `style`.
    fn text_harness(width: f32, style: Text) -> egui_kittest::Harness<'static, History> {
        use jvid_core::history::Created;
        use jvid_core::model::NewClip;
        use jvid_core::time::FLICKS_PER_SECOND as S;
        let mut h = History::new(Project::default());
        let Ok(Created::Asset(asset)) = h.apply(Command::AddText {
            name: "Title".into(),
            text: style,
        }) else {
            panic!()
        };
        let _ = h.apply(Command::AddTrack {
            kind: TrackKind::Video,
            name: "V1".into(),
        });
        let track = h.project().sequence().tracks()[0].id;
        let _ = h.apply(Command::AddClip {
            track,
            clip: NewClip {
                asset,
                source_in: Time(0),
                source_out: Time(10 * S),
                start: Time(0),
                link: None,
            },
        });
        let id = h.project().sequence().tracks()[0].clips()[0].id;
        let mut fresh = true;
        // Plus the harness's own 8px margins.
        let mut harness = egui_kittest::Harness::builder()
            .with_size([width + 16.0, 720.0])
            .wgpu()
            .build_ui_state(
                move |ui, h: &mut History| {
                    if suite_ui::test_support::install_theme(ui, &mut fresh) {
                        return;
                    }
                    ui.painter().rect_filled(ui.max_rect(), 0.0, color::panel());
                    let sel = Selection::new(h.project(), &HashSet::from([id]), None);
                    let (edit, _) = egui::ScrollArea::vertical()
                        .auto_shrink(false)
                        .show(ui, |ui| show(ui, &sel, Some(Pane::Text), Time(S)))
                        .inner;
                    match edit {
                        Some((c, Some(k))) => h.apply_coalesced(k, c).unwrap(),
                        Some((c, None)) => h.apply(c).unwrap(),
                        None => Created::Nothing,
                    };
                },
                h,
            );
        harness.run();
        harness
    }

    fn title(h: &egui_kittest::Harness<'_, History>) -> Text {
        h.state().project().assets()[0].text.clone().unwrap()
    }

    /// Every Text tab row starts its fields on one column at `width`, with sliders if
    /// `sliders`; snapshot `name`.
    fn lines_up(width: f32, name: &str, sliders: bool) {
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        {
            let mut h = text_harness(width, sample());
            for head in ["Outline", "Shadow", "Box"] {
                h.get_by_label(head).click();
                h.run();
            }
            let column = h.get_by_label("Sans-serif").rect().left();
            let fields: Vec<_> = h
                .get_all_by_role(Role::ColorWell)
                .map(|n| n.rect().left())
                .collect();
            assert_eq!(fields.len(), 4, "Fill, Outline, Shadow and Box");
            assert!(
                fields.iter().all(|x| *x == column),
                "{fields:?} vs {column}"
            );
            for spin in h.get_all_by_role(Role::SpinButton) {
                assert!(spin.rect().left() >= column, "{:?}", spin.rect());
            }
            let first = |r: &str| h.get_by_role_and_label(Role::RadioButton, r).rect().left();
            assert!(first("Left") - column <= 2.0);
            assert_eq!(h.query_all_by_role(Role::Slider).count() > 0, sliders);
            // Off the rows, into the empty foot of the panel.
            h.hover_at(egui::pos2(width / 2.0, 700.0));
            h.run();
            h.snapshot(name);
        }
    }

    // The Inspector at 1280x720 (288) and at its minimum (PANEL_MIN), less 2 x INSET.
    #[test]
    fn the_text_tab_lines_up_at_the_default_width() {
        lines_up(264.0, "inspector_text", true);
    }

    #[test]
    fn the_text_tab_lines_up_at_the_minimum_width_and_sliders_give_way() {
        lines_up(
            size::PANEL_MIN - 2.0 * space::INSET,
            "inspector_text_min",
            false,
        );
    }

    #[test]
    fn text_edits_are_one_undo_step_each() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = text_harness(264.0, Text::default());
        // Two drags of Size's slider are two steps.
        let rect = h.get_by_role_and_label(Role::Slider, "Size").rect();
        let from = egui::pos2(rect.left() + 10.0, rect.center().y);
        for dx in [20.0, 40.0] {
            h.hover_at(from);
            h.run();
            h.drag_at(from);
            h.run();
            for x in [dx / 2.0, dx] {
                h.hover_at(from + vec2(x, 0.0));
                h.run();
            }
            h.drop_at(from + vec2(dx, 0.0));
            h.run();
        }
        let dragged = title(&h).size;
        assert!(dragged > 8.0, "{dragged}");
        // Typing is one more.
        h.get_by_role(Role::MultilineTextInput).click();
        h.run();
        h.get_by_role(Role::MultilineTextInput).type_text("!!");
        h.run();
        assert_eq!(title(&h).content, "Title!!");
        // And Align another.
        h.get_by_role_and_label(Role::RadioButton, "Left").click();
        h.run();
        assert_eq!(title(&h).align, jvid_core::model::Align::Left);
        assert!(h.state_mut().undo());
        assert_eq!(title(&h).align, jvid_core::model::Align::Center);
        assert!(h.state_mut().undo());
        assert_eq!(title(&h).content, "Title");
        assert!(h.state_mut().undo());
        assert!(title(&h).size < dragged && title(&h).size != 96.0);
        assert!(h.state_mut().undo());
        assert_eq!(title(&h).size, 96.0);
    }

    #[test]
    fn the_text_diamond_keys_and_its_menu_has_the_arrows() {
        use egui_kittest::kittest::Queryable;
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = text_harness(264.0, Text::default());
        assert!(h.query_by_label("Previous Keyframe").is_none());
        h.get_by_label("Animate Size").click();
        h.run();
        let keys = |h: &egui_kittest::Harness<'_, History>| {
            let c = &h.state().project().sequence().tracks()[0].clips()[0];
            c.keyframes.get("text_size").map_or(0, Vec::len)
        };
        assert_eq!(keys(&h), 1);
        h.get_by_label("Keyframe at Playhead").click_secondary();
        h.run();
        assert!(h.query_by_label("Previous Keyframe").is_some());
        assert!(h.query_by_label("Next Keyframe").is_some());
        assert!(
            h.query_by_label("Reset").is_some(),
            "animated reads as changed"
        );
    }

    #[test]
    fn restore_sets_each_style_back_and_only_that_one() {
        let mut s = sample();
        s.weight = 300;
        for (_, rows) in TEXT_SECTIONS {
            for (id, _) in rows {
                let was = off_default(&s, id);
                let mut r = s.clone();
                restore(&mut r, id);
                assert!(!off_default(&r, id), "{id}");
                assert_eq!(was, r != s, "{id}");
            }
        }
        assert!(off_default(&s, "weight") && off_default(&s, "box_color"));
        assert!(!off_default(&s, "font") && !off_default(&s, "align"));
    }

    #[test]
    fn collapsed_sections_summarise_their_style() {
        let s = Text {
            shadow_color: [0.0, 0.0, 0.0, 0.6],
            ..sample()
        };
        let sum = |id| text_summary(&s, id);
        assert_eq!(sum("text"), "Sans-serif · 120");
        assert_eq!(sum("fill"), "#FFFFFF");
        assert_eq!(sum("outline"), "4 px · #000000");
        assert_eq!(sum("shadow"), "3, 3 · 60%");
        assert_eq!(sum("box"), "16 px · 50%");
        assert_eq!(text_summary(&Text::default(), "box"), "16 px · None");
    }

    #[test]
    fn a_text_key_toggles_and_the_last_one_leaves_the_asset_value() {
        let mut clips = vec![clip(1, 0)];
        let t = Time(0);
        let v = Value::Scalar(50.0);
        apply(&mut clips, |c| text_animate(c, "text_size", v, t));
        assert_eq!(clips[0].keyframes["text_size"].len(), 1);
        let later = Time(jvid_core::time::FLICKS_PER_SECOND);
        apply(&mut clips, |c| text_toggle_key(c, "text_size", v, later));
        assert_eq!(clips[0].keyframes["text_size"].len(), 2);
        apply(&mut clips, |c| text_toggle_key(c, "text_size", v, later));
        apply(&mut clips, |c| text_toggle_key(c, "text_size", v, t));
        assert!(!clips[0].keyframes.contains_key("text_size"));
    }

    #[test]
    fn font_matches_ignore_case() {
        let list = ["Arial", "Arial Black", "Courier New", "Mariana"].map(String::from);
        assert_eq!(
            font_matches(&list, "ARI"),
            ["Arial", "Arial Black", "Mariana"]
        );
        assert_eq!(font_matches(&list, "").len(), 4);
    }

    fn font_harness(family: &str) -> egui_kittest::Harness<'static, String> {
        let list = ["Arial", "Arial Black", "Courier New"].map(String::from);
        let mut fresh = true;
        let mut h = egui_kittest::Harness::builder().build_ui_state(
            move |ui, f: &mut String| {
                if suite_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                font_field(ui, f, &list);
            },
            family.to_owned(),
        );
        h.run();
        h
    }

    #[test]
    fn typing_narrows_fonts_and_enter_applies() {
        use egui::Key;
        use egui_kittest::kittest::Queryable;
        let mut h = font_harness("Courier New");
        h.get_by_label("Courier New").click();
        h.run();
        h.get_by_role(egui::accesskit::Role::TextInput)
            .type_text("ari");
        h.run();
        assert!(h.query_by_label("Courier New").is_none());
        assert!(h.query_by_label("Arial Black").is_some());
        h.key_press(Key::ArrowDown);
        h.key_press(Key::Enter);
        h.run();
        assert_eq!(h.state(), "Arial Black");
        assert!(h.query_by_label("Arial").is_none(), "closed");
    }

    #[test]
    fn escape_restores_the_previewed_font() {
        use egui::Key;
        use egui_kittest::kittest::Queryable;
        let mut h = font_harness("Courier New");
        h.get_by_label("Courier New").click();
        h.run();
        h.key_press(Key::ArrowDown);
        h.run();
        assert_eq!(h.state(), "Arial Black", "arrowing previews");
        h.key_press(Key::Escape);
        h.run();
        assert_eq!(h.state(), "Courier New");
    }

    #[test]
    fn a_missing_font_says_so() {
        use egui_kittest::kittest::Queryable;
        let h = font_harness("Helvetica");
        assert!(h.query_by_label("Helvetica (missing)").is_some());
    }

    #[test]
    fn spanish_captions_fit_the_label_column_as_well_as_english() {
        let keys = SECTIONS
            .iter()
            .flat_map(|(_, props)| props.iter().map(|p| format!("inspector.{}", p.name)))
            .chain(["inspector.blend".into(), "inspector.lut".into()])
            .chain(
                TEXT_SECTIONS
                    .iter()
                    .flat_map(|(_, rows)| rows.iter().map(|r| format!("inspector.{}", r.1))),
            );
        for key in keys {
            let [en, es] = ["en", "es"].map(|l| crate::i18n::CATALOG.get_in(l, &key).unwrap());
            let [en_w, es_w] = crate::i18n::widths(&[en, es]).try_into().unwrap();
            // English's Temperature already runs a fraction past the column.
            assert!(
                es_w <= size::INSPECTOR_LABEL.max(en_w),
                "{es:?} is {es_w:.1}pt, {en:?} {en_w:.1}pt"
            );
        }
    }
}
