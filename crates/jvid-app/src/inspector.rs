//! The Inspector: transform, opacity, crop and color of the selected clips, with keyframes.

use std::collections::{BTreeMap, HashSet};

use egui::{Color32, Pos2, Sense, Shape, Stroke, Ui, vec2};
use jvid_core::history::{Command, GestureKey};
use jvid_core::keyframe::{Easing, Keyframe, Value};
use jvid_core::model::{Clip, ClipId, MediaAsset, Project, TrackKind, Transform};
use jvid_core::time::Time;
use suite_ui::theme::icon;
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{Button, DragField, SectionHeader};

use crate::timeline::Edit;

/// One animatable property: its keyframe name and how its fields show.
struct Prop {
    name: &'static str,
    label: &'static str,
    fields: &'static [&'static str],
    /// Shown value per stored value, e.g. 100 for a percentage.
    display: f64,
    speed: f64,
    decimals: usize,
    suffix: &'static str,
}

const fn prop(name: &'static str, label: &'static str, fields: &'static [&'static str]) -> Prop {
    Prop {
        name,
        label,
        fields,
        display: 1.0,
        speed: 1.0,
        decimals: 1,
        suffix: "",
    }
}

const XY: &[&str] = &["X", "Y"];
const ONE: &[&str] = &[""];

const SECTIONS: [(&str, &[Prop]); 3] = [
    (
        "Transform",
        &[
            prop("position", "Position", XY),
            Prop {
                display: 100.0,
                suffix: "%",
                ..prop("scale", "Scale", ONE)
            },
            Prop {
                suffix: "°",
                ..prop("rotation", "Rotation", ONE)
            },
            prop("anchor", "Anchor", XY),
        ],
    ),
    (
        "Opacity",
        &[Prop {
            display: 100.0,
            decimals: 0,
            suffix: "%",
            ..prop("opacity", "Opacity", ONE)
        }],
    ),
    (
        "Crop",
        &[
            prop("crop_left", "Left", ONE),
            prop("crop_top", "Top", ONE),
            prop("crop_right", "Right", ONE),
            prop("crop_bottom", "Bottom", ONE),
        ],
    ),
];

/// Undo gesture for drags in the inspector, xored with the property and field.
const GESTURE: GestureKey = 0x696e_7370_0000; // "insp"

/// Undo gesture for picker edits to a solid's color, xored with its asset id.
const SOLID_GESTURE: GestureKey = 0x736f_6c69_6400; // "solid"

/// `name`'s value in `x`; scale reads as linked, X for both.
fn get(x: &Transform, name: &str) -> Value {
    let crop = |i: usize| Value::Scalar(x.crop[i]);
    match name {
        "position" => Value::Vec2(x.position),
        "anchor" => Value::Vec2(x.anchor),
        "scale" => Value::Scalar(x.scale[0]),
        "rotation" => Value::Scalar(x.rotation),
        "opacity" => Value::Scalar(x.opacity),
        "crop_left" => crop(0),
        "crop_top" => crop(1),
        "crop_right" => crop(2),
        _ => crop(3),
    }
}

fn set(x: &mut Transform, name: &str, v: Value) {
    match (name, v) {
        ("position", Value::Vec2(v)) => x.position = v,
        ("anchor", Value::Vec2(v)) => x.anchor = v,
        ("scale", Value::Scalar(v)) => x.scale = [v; 2],
        ("rotation", Value::Scalar(v)) => x.rotation = v,
        ("opacity", Value::Scalar(v)) => x.opacity = v.clamp(0.0, 1.0),
        ("crop_left", Value::Scalar(v)) => x.crop[0] = v,
        ("crop_top", Value::Scalar(v)) => x.crop[1] = v,
        ("crop_right", Value::Scalar(v)) => x.crop[2] = v,
        ("crop_bottom", Value::Scalar(v)) => x.crop[3] = v,
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

/// The clip's source time at timeline time `t`, where its keyframes live.
fn source(c: &Clip, t: Time) -> Time {
    c.source_in + (t - c.start)
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
                let (mut x, mut keys) = (c.transform, c.keyframes.clone());
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

/// Sets field `i` of `name` to `n` on every clip, keeping their other fields.
fn set_field(clips: &[&Clip], name: &str, i: usize, n: f64, t: Time) -> Command {
    rewrite(clips, |c, x, keys| {
        let v = with_field(value(c, name, t), i, n);
        put(c, x, keys, name, t, v);
    })
}

/// Animates `name` from its value at `t`, or, if any clip animates it, stops animating
/// and keeps the value at `t`.
fn toggle_animate(clips: &[&Clip], name: &str, t: Time) -> Command {
    let on = clips.iter().any(|c| animated(c, name));
    rewrite(clips, |c, x, keys| {
        let v = value(c, name, t);
        if on {
            keys.remove(name);
            set(x, name, v);
        } else {
            key(c, keys, name, t, v);
        }
    })
}

/// Removes the keyframes at `t` if every clip has one there, else adds them.
fn toggle_key(clips: &[&Clip], name: &str, t: Time) -> Command {
    let all = clips.iter().all(|c| key_at(c, name, t));
    rewrite(clips, |c, x, keys| {
        let v = value(c, name, t);
        if !all {
            return key(c, keys, name, t, v);
        }
        let list = keys.get_mut(name).expect("all have a key");
        list.retain(|k| k.time != source(c, t));
        if list.is_empty() {
            // The last key gone: hold its value rather than jump back to the old static one.
            keys.remove(name);
            set(x, name, v);
        }
    })
}

fn reset(clips: &[&Clip], name: &str) -> Command {
    let default = get(&Transform::default(), name);
    rewrite(clips, |_, x, keys| {
        keys.remove(name);
        set(x, name, default);
    })
}

/// The nearest keyframe of `name` before (`dir` < 0) or after `t` across the clips, in
/// timeline time.
fn neighbour(clips: &[&Clip], name: &str, t: Time, dir: i64) -> Option<Time> {
    let times = clips.iter().flat_map(|c| {
        let keys = c.keyframes.get(name).map(Vec::as_slice).unwrap_or_default();
        keys.iter().map(move |k| c.start + (k.time - c.source_in))
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
    let seq = project.sequence();
    // Audio clips have no picture to transform.
    let video = |t| seq.track(t).is_some_and(|t| t.kind == TrackKind::Video);
    let mut clips: Vec<&Clip> = selected
        .iter()
        .filter_map(|id| seq.clip(*id))
        .filter_map(|(t, c)| video(t).then_some(c))
        .collect();
    clips.sort_by_key(|c| c.id.0);
    let (mut edit, mut seek) = (None, None);
    if clips.is_empty() && bin_selected.is_none_or(|a| a.solid.is_none()) {
        ui.label(egui::RichText::new("Select a clip to see its properties").color(color::INK_3));
        return (None, None);
    }
    let mut index = 0u64;
    for (title, props) in SECTIONS {
        if clips.is_empty() {
            break;
        }
        if !section(ui, title) {
            index += props.len() as u64;
            continue;
        }
        for p in props {
            index += 1;
            if let Some(e) = row(ui, &clips, p, t, index, &mut seek) {
                edit = Some(e);
            }
        }
    }
    if section(ui, "Color") {
        let solid = clips
            .first()
            .and_then(|c| project.asset(c.asset))
            .or(bin_selected)
            .filter(|a| a.solid.is_some());
        if let Some(e) = solid.and_then(|a| solid_color(ui, a)) {
            edit = Some(e);
        }
        // shortcut: placeholder until M5's color adjustments (#66).
        ui.label(egui::RichText::new("Color adjustments arrive in M5").color(color::INK_3));
    }
    (edit, seek)
}

/// A section header remembering whether it's open.
fn section(ui: &mut Ui, title: &str) -> bool {
    let id = ui.id().with(("inspector", title));
    let mut open = ui.data_mut(|d| *d.get_temp_mut_or(id, true));
    ui.add(SectionHeader::new(title, &mut open));
    ui.data_mut(|d| d.insert_temp(id, open));
    open
}

fn row(
    ui: &mut Ui,
    clips: &[&Clip],
    p: &Prop,
    t: Time,
    index: u64,
    seek: &mut Option<Time>,
) -> Option<Edit> {
    let anim = clips.iter().any(|c| animated(c, p.name));
    let values: Vec<Vec<f64>> = clips.iter().map(|c| fields(value(c, p.name, t))).collect();
    let mut edit = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S4;
        let animate = glyph(ui, Glyph::Diamond, anim, &format!("Animate {}", p.label));
        if animate.clicked() {
            edit = Some((toggle_animate(clips, p.name, t), None));
        }
        ui.allocate_ui_with_layout(
            vec2(size::INSPECTOR_LABEL, size::CONTROL),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| ui.label(egui::RichText::new(p.label).color(color::INK_2)),
        );
        for (i, field) in p.fields.iter().enumerate() {
            let first = values[0][i];
            let mixed = values.iter().any(|v| v[i] != first);
            let mut shown = first * p.display;
            let mut f = DragField::new(&mut shown)
                .speed(p.speed)
                .decimals(p.decimals)
                .suffix(p.suffix)
                .mixed(mixed)
                .width(64.0);
            if !field.is_empty() {
                f = f.prefix(field);
            }
            if ui.add(f).changed() {
                let cmd = set_field(clips, p.name, i, shown / p.display, t);
                edit = Some((cmd, Some(GESTURE ^ (index << 4 | i as u64))));
            }
        }
        if anim {
            if glyph(ui, Glyph::Left, false, "Previous keyframe").clicked() {
                *seek = neighbour(clips, p.name, t, -1);
            }
            let here = clips.iter().all(|c| key_at(c, p.name, t));
            if glyph(ui, Glyph::Diamond, here, "Keyframe at playhead").clicked() {
                edit = Some((toggle_key(clips, p.name, t), None));
            }
            if glyph(ui, Glyph::Right, false, "Next keyframe").clicked() {
                *seek = neighbour(clips, p.name, t, 1);
            }
        }
        if ui.add(Button::icon(icon::UNDO, "Reset")).clicked() {
            edit = Some((reset(clips, p.name), None));
        }
    });
    edit
}

enum Glyph {
    Diamond,
    Left,
    Right,
}

/// A small painted icon button; `on` fills it.
fn glyph(ui: &mut Ui, g: Glyph, on: bool, name: &str) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(16.0, size::CONTROL), Sense::click());
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, name));
    let ink = if r.hovered() {
        color::INK
    } else {
        color::INK_2
    };
    let (c, h) = (rect.center(), 4.5);
    let points: Vec<Pos2> = match g {
        Glyph::Diamond => vec![
            c + vec2(0.0, -h),
            c + vec2(h, 0.0),
            c + vec2(0.0, h),
            c + vec2(-h, 0.0),
        ],
        Glyph::Left => vec![
            c + vec2(h * 0.6, -h),
            c + vec2(h * 0.6, h),
            c + vec2(-h * 0.6, 0.0),
        ],
        Glyph::Right => vec![
            c + vec2(-h * 0.6, -h),
            c + vec2(-h * 0.6, h),
            c + vec2(h * 0.6, 0.0),
        ],
    };
    let (fill, line) = match (g, on) {
        (Glyph::Diamond, false) => (Color32::TRANSPARENT, Stroke::new(1.0, ink)),
        (Glyph::Diamond, true) => (color::BLUE, Stroke::new(1.0, color::BLUE)),
        _ => (ink, Stroke::NONE),
    };
    ui.painter().add(Shape::convex_polygon(points, fill, line));
    r
}

/// The color picker for a solid; the edit once the color changes.
// shortcut: edits the asset's color; per-clip `color` keyframes wait on a later issue.
fn solid_color(ui: &mut Ui, a: &MediaAsset) -> Option<Edit> {
    let mut rgba = jvid_render::solid::rgba8(a.solid?);
    let changed = ui
        .horizontal(|ui| {
            ui.label("Color");
            ui.color_edit_button_srgba_unmultiplied(&mut rgba).changed()
        })
        .inner;
    let color = rgba.map(|v| v as f64 / 255.0);
    changed.then_some((
        Command::SetSolidColor { id: a.id, color },
        Some(SOLID_GESTURE ^ a.id.0),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn field_edits_keep_each_clips_other_field() {
        let mut clips = [clip(1, 0), clip(2, 0)];
        clips[1].transform.position = [5.0, 7.0];
        apply(&mut clips, |c| set_field(c, "position", 0, 3.0, Time(0)));
        assert_eq!(clips[0].transform.position, [3.0, 0.0]);
        assert_eq!(clips[1].transform.position, [3.0, 7.0]);
    }

    #[test]
    fn animated_edits_key_at_the_playhead_in_source_time() {
        let mut clips = [clip(1, 100)];
        apply(&mut clips, |c| toggle_animate(c, "opacity", Time(120)));
        assert_eq!(clips[0].keyframes["opacity"][0].time, Time(30));
        apply(&mut clips, |c| set_field(c, "opacity", 0, 0.25, Time(150)));
        let keys = &clips[0].keyframes["opacity"];
        assert_eq!(keys.len(), 2);
        assert_eq!(clips[0].transform.opacity, 1.0, "static value untouched");
        assert_eq!(clips[0].transform_at(Time(150)).opacity, 0.25);
        assert_eq!(
            neighbour(&refs(&clips), "opacity", Time(150), -1),
            Some(Time(120))
        );
        assert_eq!(
            neighbour(&refs(&clips), "opacity", Time(120), 1),
            Some(Time(150))
        );
        assert_eq!(neighbour(&refs(&clips), "opacity", Time(150), 1), None);
    }

    #[test]
    fn removing_the_last_key_holds_its_value() {
        let mut clips = [clip(1, 0)];
        apply(&mut clips, |c| toggle_animate(c, "rotation", Time(5)));
        apply(&mut clips, |c| set_field(c, "rotation", 0, 45.0, Time(5)));
        assert!(key_at(&clips[0], "rotation", Time(5)));
        apply(&mut clips, |c| toggle_key(c, "rotation", Time(5)));
        assert!(!animated(&clips[0], "rotation"));
        assert_eq!(clips[0].transform.rotation, 45.0);
    }

    #[test]
    fn key_toggle_adds_where_any_clip_lacks_one() {
        let mut clips = [clip(1, 0), clip(2, 0)];
        apply(&mut clips, |c| toggle_animate(c, "scale", Time(0)));
        apply(&mut clips, |c| toggle_key(c, "scale", Time(0)));
        assert!(!animated(&clips[0], "scale") && !animated(&clips[1], "scale"));
        apply(&mut clips[..1], |c| toggle_animate(c, "scale", Time(0)));
        apply(&mut clips, |c| toggle_key(c, "scale", Time(0)));
        assert!(clips.iter().all(|c| key_at(c, "scale", Time(0))));
    }

    #[test]
    fn reset_clears_keys_and_value() {
        let mut clips = [clip(1, 0)];
        clips[0].transform.crop[2] = 9.0;
        apply(&mut clips, |c| toggle_animate(c, "crop_right", Time(0)));
        apply(&mut clips, |c| reset(c, "crop_right"));
        assert_eq!(clips[0].transform, Transform::default());
        assert!(clips[0].keyframes.is_empty());
    }
}
