//! The Inspector: transform, opacity, crop, color adjustments, effects and solid color of the selected clips, with keyframes.

use std::collections::{BTreeMap, HashSet};

use egui::{Align, Layout, Rect, Sense, Ui, UiBuilder, Vec2, pos2, vec2};
use jvid_core::history::{Command, GestureKey};
use jvid_core::keyframe::{Easing, Keyframe, Value};
use jvid_core::model::{
    Blend, Clip, ClipId, Effect, MediaAsset, Project, Text, TrackKind, Transform,
};
use jvid_core::time::Time;
use suite_ui::theme::icon;
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{
    Button, Checkbox, DragField, Dropdown, KeyButton, Keyed, SectionHeader, Slider,
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

/// `name`'s value in `x`; scale reads as linked, X for both.
fn get(x: &Transform, name: &str) -> Value {
    if let Some((i, p)) = fx_param(name) {
        let v = x.effects.get(i).and_then(|e| e.params.get(p));
        return Value::Scalar(v.copied().unwrap_or(0.0));
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

/// Sets each of `names` to its value in `x` on every clip, keying animated ones at `t`.
pub fn set_props(clips: &[&Clip], names: &[&str], x: &Transform, t: Time) -> Command {
    rewrite(clips, |c, st, keys| {
        for name in names {
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

/// The selected clips, or the solid selected in the bin, at playhead `t`; an edit, and
/// where to seek the playhead.
pub fn show(
    ui: &mut Ui,
    project: &Project,
    selected: &HashSet<ClipId>,
    bin_selected: Option<&MediaAsset>,
    t: Time,
) -> (Option<Edit>, Option<Time>) {
    // A gutter keeps the row buttons clear of the scroll bar.
    let gutter = ui.spacing().scroll.allocated_width() + space::S4;
    ui.set_max_width(ui.available_width() - gutter);
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
    let (mut edit, mut seek) = (None, None);
    if clips.is_empty() && audio.is_empty() && bin_selected.is_none_or(|a| !a.generated()) {
        ui.label(egui::RichText::new(tr!("inspector.empty")).color(color::INK_3));
        return (None, None);
    }
    let mut index = 0u64;
    for (id, props) in SECTIONS {
        let clips = if id == "audio" { &audio } else { &clips };
        if clips.is_empty() {
            index += props.len() as u64;
            continue;
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
    if !clips.is_empty()
        && section(ui, "effects", None).0
        && let Some(e) = effects_rows(ui, &clips, t, &mut index, &mut seek)
    {
        edit = Some(e);
    }
    if let Some(c) = clips.first().or(audio.first())
        && section(ui, "speed", None).0
        && let Some(e) = speed_row(ui, c)
    {
        edit = Some(e);
    }
    // Only when every selected clip shows the same asset, so the editors edit them all.
    let same = clips.iter().all(|c| c.asset == clips[0].asset);
    let shared = clips
        .first()
        .filter(|_| same)
        .and_then(|c| project.asset(c.asset))
        .or(bin_selected);
    if let Some(a) = shared.filter(|a| a.text.is_some())
        && section(ui, "text", None).0
        && let Some(e) = text_props(ui, a, if same { &clips } else { &[] }, t)
    {
        edit = Some(e);
    }
    if (audio.is_empty() || !clips.is_empty()) && section(ui, "color", None).0 {
        let solid = shared.filter(|a| a.solid.is_some());
        if let Some(e) = solid.and_then(|a| solid_color(ui, a)) {
            edit = Some(e);
        }
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

/// The effect stack, shown when every clip has the same one: each effect's parameters as
/// keyframeable rows with move and remove buttons, then a picker adding one to the end.
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
        ui.label(egui::RichText::new(tr!("inspector.mixed_effects")).color(color::INK_3));
        return None;
    }
    let lib: Vec<(String, crate::effects::Entry, bool)> = crate::effects::with(|l| {
        l.entries()
            .map(|(k, e)| (k.to_owned(), e.clone(), l.shipped(k)))
            .collect()
    });
    let mut edit = None;
    for (i, e) in effects.iter().enumerate() {
        let found = lib.iter().find(|(k, ..)| *k == e.shader);
        let (entry, shipped) = (found.map(|(_, e, _)| e), found.is_some_and(|f| f.2));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::S4;
            let (title, problem) = match entry {
                Some(Ok(s)) => (crate::effects::name(&e.shader, shipped, s).to_owned(), None),
                Some(Err(err)) => (e.shader.clone(), Some(err.clone())),
                None => (
                    e.shader.clone(),
                    Some(tr!(
                        "inspector.effect_missing",
                        file = format!("{}.wgsl", e.shader)
                    )),
                ),
            };
            let label = ui.label(egui::RichText::new(&title).color(color::INK).strong());
            if let Some(p) = problem {
                label.on_hover_text(&p);
                ui.label(egui::RichText::new(tr!("inspector.passthrough")).color(color::RED));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let n = effects.len();
                if ui.add(Button::ghost(tr!("inspector.fx_remove"))).clicked() {
                    edit = Some((
                        restack(clips, |o| {
                            o.remove(i);
                        }),
                        None,
                    ));
                }
                if i + 1 < n && ui.add(Button::ghost(tr!("inspector.fx_down"))).clicked() {
                    edit = Some((restack(clips, |o| o.swap(i, i + 1)), None));
                }
                if i > 0 && ui.add(Button::ghost(tr!("inspector.fx_up"))).clicked() {
                    edit = Some((restack(clips, |o| o.swap(i - 1, i)), None));
                }
            });
        });
        let Some(Ok(shader)) = entry else { continue };
        for q in &shader.params {
            *index += 1;
            let name = Effect::key(i, &q.name);
            let label = crate::effects::param(&e.shader, shipped, &q.name);
            let p = Prop {
                speed: (q.max - q.min) / 200.0,
                decimals: 2,
                range: (q.min, q.max),
                default: Some(q.default),
                label: Some(&label),
                ..prop(&name, ONE)
            };
            if let Some(e) = row(ui, clips, &p, t, *index, seek) {
                edit = Some(e);
            }
        }
    }
    let disabled = crate::effects::with(|l| l.disabled.clone());
    let usable: Vec<(&str, &jvid_render::effect::Shader, bool)> = lib
        .iter()
        .filter(|(k, ..)| !disabled.contains(k))
        .filter_map(|(k, e, shipped)| Some((k.as_str(), e.as_deref().ok()?, *shipped)))
        .collect();
    let mut names = vec![tr!("inspector.add_effect")];
    names.extend(
        usable
            .iter()
            .map(|(k, s, shipped)| crate::effects::name(k, *shipped, s)),
    );
    let mut pick = 0;
    if ui
        .add(Dropdown::new(&mut pick, &names).width(ui.available_width()))
        .changed()
        && pick > 0
    {
        let (stem, shader, _) = usable[pick - 1];
        edit = Some((push_effect(clips, stem, shader), None));
    }
    edit
}

fn push_effect(clips: &[&Clip], stem: &str, shader: &jvid_render::effect::Shader) -> Command {
    let fx = Effect {
        shader: stem.to_owned(),
        params: shader
            .params
            .iter()
            .map(|q| (q.name.clone(), q.default))
            .collect(),
    };
    rewrite(clips, |_, x, _| x.effects.push(fx.clone()))
}

/// Adds effect `stem` from the effects folder to the end of each of the video clips in `ids`;
/// `None` when there are none or the effect doesn't load.
pub fn add_effect(project: &Project, ids: &HashSet<ClipId>, stem: &str) -> Option<Command> {
    let seq = project.sequence();
    let mut clips: Vec<&Clip> = ids
        .iter()
        .filter_map(|id| seq.clip(*id))
        .filter(|(t, _)| seq.track(*t).is_some_and(|t| t.kind == TrackKind::Video))
        .map(|(_, c)| c)
        .collect();
    clips.sort_by_key(|c| c.id.0);
    let shader = crate::effects::with(|l| l.get(stem).and_then(|e| e.clone().ok()))?;
    (!clips.is_empty()).then(|| push_effect(&clips, stem, &shader))
}

/// Undo gesture for speed drags, xored with the clip id.
const SPEED_GESTURE: GestureKey = 0x7370_6565_6400; // "speed"

/// Speed percentage, reverse and ripple for the first selected clip and its links.
fn speed_row(ui: &mut Ui, c: &Clip) -> Option<Edit> {
    let ripple_id = egui::Id::new("speed_ripple");
    let mut ripple = ui.data(|d| d.get_temp(ripple_id)).unwrap_or(true);
    let (mut pct, mut reverse) = (c.speed * 100.0, c.reverse);
    let (changed, _) = prop_row(ui, tr!("inspector.speed"), Tools::default(), |ui| {
        let drag = DragField::new(&mut pct)
            .range(1.0..=10000.0)
            .suffix("%")
            .width(64.0);
        let a = ui.add_enabled(c.freeze.is_none(), drag).changed();
        ui.add_space(space::S4);
        let b = ui
            .add(Checkbox::new(&mut reverse, tr!("inspector.reverse")))
            .changed();
        ui.add_space(space::S4);
        ui.add(Checkbox::new(&mut ripple, tr!("inspector.ripple")))
            .on_hover_text(tr!("inspector.ripple_tooltip"));
        a || b
    });
    ui.data_mut(|d| d.insert_temp(ripple_id, ripple));
    changed.then(|| {
        let cmd = Command::SetClipSpeed {
            id: c.id,
            speed: pct / 100.0,
            reverse,
            ripple,
        };
        (cmd, Some(SPEED_GESTURE ^ c.id.0))
    })
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
    let mut head = None;
    if let Some(changed) = tools {
        let mut ui = ui.new_child(UiBuilder::new().max_rect(rect));
        let reset = tr!("inspector.reset_section", section = title);
        let key = tr!("inspector.keyframe_all", section = title);
        let c = slots(
            &mut ui,
            rect,
            Tools {
                reset: changed.then_some(&reset),
                nav: false,
                key: Some((Keyed::Static, &key)),
            },
        );
        if c.key.is_some_and(|k| k.clicked()) {
            head = Some(Head::Key);
        }
        if c.reset {
            head = Some(Head::Reset);
        }
    }
    (open, head)
}

/// Width of a row's tool slots: reset, previous and next keyframe, then the diamond.
const TOOLS: f32 = 4.0 * size::SMALL_CONTROL + 3.0 * GAP;
/// Space between tool slots.
const GAP: f32 = 2.0;

/// What a row's tool slots show, with names for screen readers. An empty slot keeps its
/// place, so nothing moves when another appears.
#[derive(Default)]
struct Tools<'a> {
    reset: Option<&'a str>,
    nav: bool,
    key: Option<(Keyed, &'a str)>,
}

/// What happened in a row's tool slots.
#[derive(Default)]
struct Clicks {
    reset: bool,
    previous: bool,
    next: bool,
    key: Option<egui::Response>,
}

/// Draws `tools` in the slots at the right edge of `rect`.
fn slots(ui: &mut Ui, rect: Rect, tools: Tools) -> Clicks {
    let slot = |i: f32| {
        let x = rect.right() - TOOLS + i * (size::SMALL_CONTROL + GAP);
        let y = rect.center().y - size::SMALL_CONTROL / 2.0;
        Rect::from_min_size(pos2(x, y), Vec2::splat(size::SMALL_CONTROL))
    };
    let mut c = Clicks::default();
    if let Some(name) = tools.reset {
        c.reset = ui
            .put(slot(0.0), Button::icon(icon::UNDO, name).small())
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
        ui.label(egui::RichText::new(label).color(color::INK_2))
    });
    let w = rect.right() - TOOLS - space::S4 - row.cursor().left();
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
        reset: differs(clips, p, t).then_some(reset),
        nav: anim,
        key: Some((keyed, &key)),
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
    if c.previous {
        *seek = neighbour(clips, &names, t, -1);
    }
    if c.next {
        *seek = neighbour(clips, &names, t, 1);
    }
    let diamond = c.key.expect("rows have a diamond");
    if diamond.clicked() {
        let cmd = match anim {
            false => toggle_animate(clips, &names, t),
            true => toggle_key(clips, &names, t),
        };
        edit = Some((cmd, None));
    }
    if anim {
        egui::Popup::context_menu(&diamond)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                if keyed == Keyed::Here
                    && let Some(e) = easing_menu(ui, clips, &names, t, index)
                {
                    edit = Some(e);
                }
                if ui
                    .add(Button::ghost(tr!("inspector.stop_animating")))
                    .clicked()
                {
                    edit = Some((toggle_animate(clips, &names, t), None));
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
    let changed = ui
        .horizontal(|ui| {
            ui.label(tr!("inspector.color"));
            ui.color_edit_button_srgba_unmultiplied(&mut rgba).changed()
        })
        .inner;
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

/// The Font field: shows `family` (or "(missing)" when `list` lacks it); clicked, a search
/// over `list` that previews the hovered or highlighted family in `family`. Enter or a click
/// keeps it, Esc or a click away restores the one before. True once `family` changed.
fn font_field(ui: &mut Ui, family: &mut String, list: &[String]) -> bool {
    use egui::Key;
    let id = egui::Id::new("inspector font");
    let Some(mut st) = ui.data(|d| d.get_temp::<FontSearch>(id)) else {
        let missing = !family.is_empty() && !list.contains(family);
        let shown = match family.as_str() {
            "" => tr!("inspector.sans_serif").to_owned(),
            f if missing => tr!("inspector.font_missing", family = f),
            f => f.to_owned(),
        };
        let r = ui.button(shown);
        let r = if missing {
            r.on_hover_text(tr!("inspector.font_missing_tooltip"))
        } else {
            r
        };
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
                .is_some_and(|p| !st.area.contains(p));
        (
            i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
            i.key_pressed(Key::Enter),
            i.key_pressed(Key::Escape),
            away,
        )
    });
    if down {
        st.at += 1;
    }
    if up {
        st.at = st.at.saturating_sub(1);
    }
    st.at = st.at.min(matches.len().saturating_sub(1));
    let mut pick = matches.get(st.at).copied();
    let mut clicked = false;
    let inner = ui.vertical(|ui| {
        let q = ui.add(
            egui::TextEdit::singleline(&mut st.query).hint_text(tr!("inspector.search_fonts")),
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
    st.area = inner.response.rect;
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

/// A text style row's animate toggle, when clips are selected; sets `toggle` once clicked.
fn animate(ui: &mut Ui, clips: &[&Clip], name: &'static str, toggle: &mut Option<&'static str>) {
    let on = match clips.iter().any(|c| animated(c, name)) {
        true => Keyed::Animated,
        false => Keyed::Static,
    };
    let tip = tr!("inspector.animate", name = caption(name));
    if !clips.is_empty() && ui.add(KeyButton::diamond(on, &tip)).clicked() {
        *toggle = Some(name);
    }
}

/// A straight 0..=1 RGBA picker with `name`'s animate toggle; true once changed.
fn color_field(
    ui: &mut Ui,
    label: &str,
    c: &mut [f64; 4],
    (clips, name, toggle): (&[&Clip], &'static str, &mut Option<&'static str>),
) -> bool {
    let mut rgba = jvid_render::solid::rgba8(*c);
    let changed = ui
        .horizontal(|ui| {
            ui.label(label);
            let changed = ui.color_edit_button_srgba_unmultiplied(&mut rgba).changed();
            animate(ui, clips, name, toggle);
            changed
        })
        .inner;
    if changed {
        *c = rgba.map(|v| v as f64 / 255.0);
    }
    changed
}

/// The editors for a text asset's style at playhead `t`, with `clips` (all showing it) for
/// keyframes; the edit once anything changes. Animated properties key every clip at `t`;
/// the rest edit the asset, so every clip of it changes.
// shortcut: a per-clip caption needs its own asset; add a Duplicate action if users ask.
fn text_props(ui: &mut Ui, a: &MediaAsset, clips: &[&Clip], t: Time) -> Option<Edit> {
    use jvid_core::model::Align;
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
    let mut toggle = None;
    let content = ui.add(
        egui::TextEdit::multiline(&mut s.content)
            .id(content_id())
            .desired_rows(2),
    );
    let mut changed = content.changed();
    ui.horizontal(|ui| {
        ui.label(tr!("inspector.font"));
        changed |= font_field(ui, &mut s.family, jvid_render::text::families());
    });
    ui.horizontal(|ui| {
        ui.label(tr!("inspector.text_size"));
        changed |= ui
            .add(egui::DragValue::new(&mut s.size).range(1.0..=1000.0))
            .changed();
        animate(ui, clips, "text_size", &mut toggle);
        ui.label(tr!("inspector.weight"));
        changed |= ui
            .add(
                egui::DragValue::new(&mut s.weight)
                    .range(100..=900)
                    .speed(10),
            )
            .changed();
    });
    changed |= color_field(
        ui,
        tr!("inspector.color"),
        &mut s.color,
        (clips, "color", &mut toggle),
    );
    ui.horizontal(|ui| {
        for (al, name) in [
            (Align::Left, tr!("inspector.align_left")),
            (Align::Center, tr!("inspector.align_center")),
            (Align::Right, tr!("inspector.align_right")),
        ] {
            changed |= ui.selectable_value(&mut s.align, al, name).changed();
        }
    });
    ui.horizontal(|ui| {
        ui.label(tr!("inspector.text_anchor"));
        for v in &mut s.anchor {
            changed |= ui
                .add(egui::DragValue::new(v).range(0.0..=1.0).speed(0.005))
                .changed();
        }
        animate(ui, clips, "text_anchor", &mut toggle);
    });
    ui.horizontal(|ui| {
        ui.label(tr!("inspector.outline"));
        changed |= ui
            .add(egui::DragValue::new(&mut s.outline).range(0.0..=32.0))
            .changed();
        animate(ui, clips, "outline", &mut toggle);
    });
    let outline = (clips, "outline_color", &mut toggle);
    changed |= color_field(
        ui,
        tr!("inspector.outline_color"),
        &mut s.outline_color,
        outline,
    );
    ui.horizontal(|ui| {
        ui.label(tr!("inspector.shadow"));
        for v in &mut s.shadow {
            changed |= ui
                .add(egui::DragValue::new(v).range(-64.0..=64.0))
                .changed();
        }
        animate(ui, clips, "shadow", &mut toggle);
    });
    let shadow = (clips, "shadow_color", &mut toggle);
    changed |= color_field(
        ui,
        tr!("inspector.shadow_color"),
        &mut s.shadow_color,
        shadow,
    );
    ui.horizontal(|ui| {
        ui.label(tr!("inspector.box_padding"));
        changed |= ui
            .add(egui::DragValue::new(&mut s.box_padding).range(0.0..=200.0))
            .changed();
        animate(ui, clips, "box_padding", &mut toggle);
    });
    changed |= color_field(
        ui,
        tr!("inspector.box_color"),
        &mut s.box_color,
        (clips, "box_color", &mut toggle),
    );
    if let Some(name) = toggle {
        return Some((text_animate(clips, &before, name, t), None));
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
        return Some((cmd, Some(TEXT_GESTURE ^ a.id.0 ^ 1 << 40)));
    }
    // Animated values stay the asset's; the keys showed over them.
    for n in text_names().filter(|n| anim(n)) {
        text_set(&mut s, n, text_get(&base, n).expect("a text name"));
    }
    (s != base).then_some((
        Command::SetText { id: a.id, text: s },
        Some(TEXT_GESTURE ^ a.id.0),
    ))
}

/// Animates text property `name` from its value in `shown`, or, if any clip animates it,
/// drops its keys so the asset's value shows again.
fn text_animate(clips: &[&Clip], shown: &Text, name: &str, t: Time) -> Command {
    let on = clips.iter().any(|c| animated(c, name));
    let v = text_get(shown, name).expect("a text name");
    rewrite(clips, |c, _, keys| {
        if on {
            keys.remove(name);
        } else {
            key(c, keys, name, t, v);
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
            .chain(text_names());
        for id in ids {
            let key = format!("inspector.{id}");
            for lang in suite_ui::i18n::LANGUAGES {
                assert!(CATALOG.get_in(lang, &key).is_some(), "{lang} lacks {key}");
            }
        }
    }

    #[test]
    fn effect_params_keyframe_and_follow_their_effect_when_restacked() {
        let fx = |shader: &str| Effect {
            shader: shader.into(),
            params: [("amount".to_owned(), 0.5)].into(),
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
    fn text_style_animates_per_clip_and_stops() {
        let mut clips = [clip(1, 0), clip(2, 50)];
        let shown = Text {
            outline: 3.0,
            ..Text::default()
        };
        apply(&mut clips, |c| text_animate(c, &shown, "outline", Time(60)));
        assert_eq!(clips[0].keyframes["outline"][0].time, Time(70));
        assert_eq!(clips[1].keyframes["outline"][0].time, Time(20));
        let t = Text::default().with(&clips[1].text_keys(Time(60)));
        assert_eq!(t.outline, 3.0, "the key shows over the asset");
        apply(&mut clips, |c| text_animate(c, &shown, "outline", Time(60)));
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

    /// The inspector over a real history, with one video clip selected.
    fn harness() -> egui_kittest::Harness<'static, History> {
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
        let mut harness = egui_kittest::Harness::builder()
            .with_size([340.0, 420.0])
            .wgpu()
            .build_ui_state(
                move |ui, h: &mut History| {
                    if suite_ui::test_support::install_theme(ui, &mut fresh) {
                        return;
                    }
                    ui.painter().rect_filled(ui.max_rect(), 0.0, color::PANEL);
                    let (edit, _) = show(ui, h.project(), &HashSet::from([id]), None, Time(S));
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
        let mut h = egui_kittest::Harness::builder().build_ui_state(
            move |ui, f: &mut String| {
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
            .chain(["inspector.blend".into(), "inspector.lut".into()]);
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
