//! The Inspector: transform, opacity, crop and color of the selected clips, with keyframes.

use std::collections::{BTreeMap, HashSet};

use egui::{Color32, Pos2, Sense, Shape, Stroke, Ui, vec2};
use jvid_core::history::{Command, GestureKey};
use jvid_core::keyframe::{Easing, Keyframe, Value};
use jvid_core::model::{Blend, Clip, ClipId, MediaAsset, Project, TrackKind, Transform};
use jvid_core::time::Time;
use suite_ui::theme::icon;
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{Button, DragField, Dropdown, SectionHeader};

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
    /// Bounds of the stored value.
    range: (f64, f64),
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
        range: (f64::NEG_INFINITY, f64::INFINITY),
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
            range: (0.0, 1.0),
            ..prop("opacity", "Opacity", ONE)
        }],
    ),
    (
        "Crop",
        &[
            Prop {
                range: (0.0, f64::INFINITY),
                ..prop("crop_left", "Left", ONE)
            },
            Prop {
                range: (0.0, f64::INFINITY),
                ..prop("crop_top", "Top", ONE)
            },
            Prop {
                range: (0.0, f64::INFINITY),
                ..prop("crop_right", "Right", ONE)
            },
            Prop {
                range: (0.0, f64::INFINITY),
                ..prop("crop_bottom", "Bottom", ONE)
            },
        ],
    ),
];

/// Undo gesture for drags in the inspector, xored with the property, field and drag count.
const GESTURE: GestureKey = 0x696e_7370_0000; // "insp"

/// Undo gesture for easing handle drags, xored with the property and drag count.
const EASING_GESTURE: GestureKey = 0x6561_7365_0000; // "ease"

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
        // Keeps a non-uniform scale's ratio.
        ("scale", Value::Scalar(v)) if x.scale[0] != 0.0 => {
            x.scale = [v, x.scale[1] * v / x.scale[0]]
        }
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

/// The clip's source time at timeline time `t`, where its keyframes live, clamped to the
/// clip so a playhead outside it never keys beyond its media range.
fn source(c: &Clip, t: Time) -> Time {
    (c.source_in + (t - c.start)).clamp(c.source_in, c.source_out)
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
        let names: Vec<&str> = props.iter().map(|p| p.name).collect();
        let (open, head) = section(ui, title, true);
        match head {
            Some(Head::Reset) => edit = Some((reset(&clips, &names), None)),
            Some(Head::Key) => edit = Some((key_all(&clips, &names, t), None)),
            None => {}
        }
        if !open {
            index += props.len() as u64;
            continue;
        }
        for p in props {
            index += 1;
            if let Some(e) = row(ui, &clips, p, t, index, &mut seek) {
                edit = Some(e);
            }
        }
        if title == "Opacity"
            && let Some(e) = blend_row(ui, &clips)
        {
            edit = Some(e);
        }
    }
    if section(ui, "Color", false).0 {
        // Only when every selected clip shows the same solid, so the picker edits them all.
        let same = clips.iter().all(|c| c.asset == clips[0].asset);
        let solid = clips
            .first()
            .filter(|_| same)
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

const BLENDS: [&str; 8] = [
    "Normal",
    "Multiply",
    "Screen",
    "Overlay",
    "Add",
    "Darken",
    "Lighten",
    "Difference",
];

/// Sets every clip's blend mode; one undo step.
fn set_blend(clips: &[&Clip], blend: Blend) -> Command {
    rewrite(clips, |_, x, _| x.blend = blend)
}

/// The blend mode dropdown, showing the first clip's mode when the selection is mixed.
fn blend_row(ui: &mut Ui, clips: &[&Clip]) -> Option<Edit> {
    let first = clips[0].transform.blend;
    let mut i = Blend::ALL.iter().position(|b| *b == first).unwrap_or(0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S4;
        ui.allocate_ui_with_layout(
            vec2(size::INSPECTOR_LABEL, size::CONTROL),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(size::INSPECTOR_LABEL);
                ui.label(egui::RichText::new("Blend").color(color::INK_2))
            },
        );
        ui.add(Dropdown::new(&mut i, &BLENDS).width(132.0))
            .changed()
            .then(|| (set_blend(clips, Blend::ALL[i]), None))
    })
    .inner
}

/// A section head button.
enum Head {
    Reset,
    Key,
}

/// A section header remembering whether it's open, with reset and keyframe-all buttons on
/// the right if `buttons`.
fn section(ui: &mut Ui, title: &str, buttons: bool) -> (bool, Option<Head>) {
    let id = ui.id().with(("inspector", title));
    let mut open = ui.data_mut(|d| *d.get_temp_mut_or(id, true));
    let rect = ui.add(SectionHeader::new(title, &mut open)).rect;
    ui.data_mut(|d| d.insert_temp(id, open));
    let mut head = None;
    if buttons {
        let mut right = egui::Rect::from_x_y_ranges(rect.x_range(), rect.y_range());
        right.min.x = right.max.x - KEY_HIT - space::S4 - size::CONTROL;
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(right)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        ui.spacing_mut().item_spacing.x = space::S4;
        if glyph(
            &mut ui,
            Glyph::Diamond,
            false,
            &format!("Keyframe all {title}"),
        )
        .clicked()
        {
            head = Some(Head::Key);
        }
        if ui
            .add(Button::icon(icon::UNDO, &format!("Reset {title}")))
            .clicked()
        {
            head = Some(Head::Reset);
        }
    }
    (open, head)
}

/// The width of a keyframe toggle's hit area.
const KEY_HIT: f32 = 20.0;

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
        ui.allocate_ui_with_layout(
            vec2(size::INSPECTOR_LABEL, size::CONTROL),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(size::INSPECTOR_LABEL);
                ui.label(egui::RichText::new(p.label).color(color::INK_2))
            },
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
            let (lo, hi) = p.range;
            f = f.range(lo * p.display..=hi * p.display);
            let r = ui.add(f);
            // Each drag is its own undo step, even of the field the last one dragged.
            let drags = ui.id().with("inspector drags");
            if r.drag_started() {
                ui.data_mut(|d| *d.get_temp_mut_or(drags, 0u64) += 1);
            }
            if r.changed() {
                let n = shown / p.display;
                // A drag of a mixed field moves each clip by the delta, not to the first's value.
                let cmd = if mixed && r.dragged() {
                    set_field(clips, p.name, i, |v| (v + n - first).clamp(lo, hi), t)
                } else {
                    set_field(clips, p.name, i, |_| n, t)
                };
                let drag = ui.data(|d| d.get_temp(drags).unwrap_or(0u64));
                let key = GESTURE ^ (index << 4 | i as u64) ^ (drag << 16);
                edit = Some((cmd, r.dragged().then_some(key)));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let animate = glyph(ui, Glyph::Diamond, anim, &format!("Animate {}", p.label));
            if animate.clicked() {
                edit = Some((toggle_animate(clips, p.name, t), None));
            }
            if ui.add(Button::icon(icon::UNDO, "Reset")).clicked() {
                edit = Some((reset(clips, &[p.name]), None));
            }
            if !anim {
                return;
            }
            if glyph(ui, Glyph::Right, false, "Next keyframe").clicked() {
                *seek = neighbour(clips, p.name, t, 1);
            }
            let here = clips.iter().all(|c| key_at(c, p.name, t));
            let diamond = glyph(ui, Glyph::Diamond, here, "Keyframe at playhead");
            if diamond.clicked() {
                edit = Some((toggle_key(clips, p.name, t), None));
            }
            if here {
                egui::Popup::context_menu(&diamond)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                        if let Some(e) = easing_menu(ui, clips, p.name, t, index) {
                            edit = Some(e);
                        }
                    });
            }
            if glyph(ui, Glyph::Left, false, "Previous keyframe").clicked() {
                *seek = neighbour(clips, p.name, t, -1);
            }
        });
    });
    edit
}

/// The easing editor for the keyframes of `name` at `t`, opened by right-clicking their diamond.
fn easing_menu(ui: &mut Ui, clips: &[&Clip], name: &str, t: Time, index: u64) -> Option<Edit> {
    let s = source(clips[0], t);
    let k = clips[0].keyframes.get(name)?.iter().find(|k| k.time == s)?;
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
    Some((set_easing(clips, name, t, e), dragging.then_some(key)))
}

/// Sets the easing of every clip's keyframe of `name` at `t`.
fn set_easing(clips: &[&Clip], name: &str, t: Time, e: Easing) -> Command {
    rewrite(clips, |c, _, keys| {
        let s = source(c, t);
        for k in keys.get_mut(name).into_iter().flatten() {
            if k.time == s {
                k.easing = e;
            }
        }
    })
}

enum Glyph {
    Diamond,
    Left,
    Right,
}

/// A small painted icon button; `on` fills it.
fn glyph(ui: &mut Ui, g: Glyph, on: bool, name: &str) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(KEY_HIT, size::CONTROL), Sense::click());
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, name));
    let ink = if r.hovered() {
        color::INK
    } else {
        color::INK_2
    };
    // 4 makes an 8px diamond, as DESIGN.md asks.
    let (c, h) = (rect.center(), 4.0);
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
        (Glyph::Diamond, false) => (Color32::TRANSPARENT, Stroke::new(1.0, color::INK)),
        (Glyph::Diamond, true) => (color::LIGHT_BLUE, Stroke::new(1.0, color::LIGHT_BLUE)),
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
    fn easing_applies_to_every_clips_key_and_changes_playback() {
        let mut clips = [clip(1, 0), clip(2, 50)];
        apply(&mut clips, |c| toggle_animate(c, "opacity", Time(50)));
        apply(&mut clips, |c| {
            set_field(c, "opacity", 0, |_| 0.0, Time(80))
        });
        let before = clips[1].transform_at(Time(65)).opacity;
        apply(&mut clips, |c| {
            set_easing(c, "opacity", Time(50), Easing::Hold)
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
        apply(&mut clips, |c| toggle_animate(c, "opacity", Time(120)));
        assert_eq!(clips[0].keyframes["opacity"][0].time, Time(30));
        apply(&mut clips, |c| {
            set_field(c, "opacity", 0, |_| 0.25, Time(150))
        });
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
        apply(&mut clips, |c| {
            set_field(c, "rotation", 0, |_| 45.0, Time(5))
        });
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
    fn blend_sets_every_clip_and_keeps_the_rest() {
        let mut clips = [clip(1, 0), clip(2, 0)];
        clips[1].transform.opacity = 0.5;
        apply(&mut clips, |c| set_blend(c, Blend::Screen));
        assert!(clips.iter().all(|c| c.transform.blend == Blend::Screen));
        assert_eq!(clips[1].transform.opacity, 0.5);
        let names = Blend::ALL.map(|b| format!("{b:?}"));
        assert_eq!(names, BLENDS, "labels follow Blend::ALL");
    }

    #[test]
    fn reset_clears_keys_and_value() {
        let mut clips = [clip(1, 0)];
        clips[0].transform.crop[2] = 9.0;
        apply(&mut clips, |c| toggle_animate(c, "crop_right", Time(0)));
        apply(&mut clips, |c| reset(c, &["crop_right"]));
        assert_eq!(clips[0].transform, Transform::default());
        assert!(clips[0].keyframes.is_empty());
    }

    #[test]
    fn keys_stay_inside_the_clip() {
        // Source 10..110 at timeline 100..200.
        let mut clips = [clip(1, 100)];
        apply(&mut clips, |c| toggle_animate(c, "rotation", Time(50)));
        apply(&mut clips, |c| {
            set_field(c, "rotation", 0, |_| 9.0, Time(500))
        });
        let times: Vec<_> = clips[0].keyframes["rotation"]
            .iter()
            .map(|k| k.time)
            .collect();
        assert_eq!(times, [Time(10), Time(110)]);
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
}
