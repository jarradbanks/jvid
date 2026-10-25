//! Masks: spotlight the subject by darkening a copy under a masked one, then key the mask
//! so it follows them.

use jvid_core::history::Command;
use jvid_core::model::{Clip, Mask, MaskMode, MaskShape, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND, Time};

use super::{App, Lesson, Step, apply};
use crate::i18n::tr;

/// The subject's head in the first frame and near the last, as a mask's centre and size in
/// project pixels from the frame's centre. The sandbox matches the 1280x720 clip, which fills
/// the frame, so these are its masks' pixels too.
const START: [[f64; 2]; 2] = [[110.0, -60.0], [320.0, 480.0]];
const END: [[f64; 2]; 2] = [[140.0, -60.0], [320.0, 480.0]];
/// How much of the outline the mask must share with it to cover it.
const COVERS: f64 = 0.6;
const DARK: f64 = -1.0;
const SOFT: f64 = 50.0;
const SECOND: Time = Time(FLICKS_PER_SECOND);

pub const MASKS: Lesson = Lesson {
    id: "masks",
    title: || tr!("tutorial.masks"),
    minutes: 5,
    media: &["subject.mp4"],
    setup: |a| {
        let project = a.history.project();
        let Some(asset) = project.assets().first() else {
            return;
        };
        if let Some(p) = project.placement(asset.id, Time(0), false, None, false) {
            apply(a, Command::Place(p));
        }
        let (kind, name) = (TrackKind::Video, String::new());
        apply(a, Command::AddTrack { kind, name });
    },
    steps: &[
        Step {
            anchor: "timeline",
            text: |_| tr!("tutorial.masks_select").into(),
            done: |a| lower(a).is_some_and(|c| a.timeline.selected().contains(&c.id)),
            show_me: |a| {
                if let Some(id) = lower(a).map(|c| c.id) {
                    a.timeline.select_clip(a.history.project().sequence(), id);
                }
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "timeline",
            text: |a| {
                tr!(
                    "tutorial.masks_duplicate",
                    key = a.keymap.shortcut("duplicate")
                )
            },
            done: |a| top(a).is_some(),
            show_me: |a| {
                let seq = a.history.project().sequence();
                let Some(c) = lower(a) else { return };
                let Some((track, _)) = seq.clip(c.id) else {
                    return;
                };
                let tracks = seq.tracks();
                let below = tracks.iter().position(|t| t.id == track).unwrap_or(0);
                let Some(above) = below.checked_sub(1).map(|i| &tracks[i]) else {
                    return;
                };
                let paste = Command::Paste {
                    clips: vec![(above.id, c.clone())],
                    at: c.start,
                };
                apply(a, paste);
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "inspector.exposure",
            text: |_| tr!("tutorial.masks_darken").into(),
            done: |a| lower(a).is_some_and(|c| c.transform.exposure <= DARK),
            show_me: |a| {
                let Some(c) = lower(a) else { return };
                let (mut x, keys) = (c.transform.clone(), c.keyframes.clone());
                x.exposure = DARK * 1.5;
                let id = c.id;
                a.timeline.select_clip(a.history.project().sequence(), id);
                apply(a, Command::SetClipProps(vec![(id, x, keys)]));
            },
            reveal: None,
            ghost: None,
            also: &["timeline"],
        },
        Step {
            anchor: "inspector.add_mask",
            text: |_| tr!("tutorial.masks_add").into(),
            done: |a| {
                top(a).is_some_and(|c| {
                    matches!(c.transform.masks.as_slice(), [m] if m.shape == MaskShape::Ellipse)
                })
            },
            show_me: |a| {
                let Some(c) = top(a) else { return };
                let [w, h] = crate::masks::fitted(a.history.project(), c);
                // As Add Mask > Ellipse makes it: centred, half the image's size.
                let mask = Mask::new(MaskShape::Ellipse, vec![[0.0, 0.0], [w / 2.0, h / 2.0]]);
                let id = c.id;
                a.timeline.select_clip(a.history.project().sequence(), id);
                edit_mask(a, |m| *m = mask.clone());
            },
            reveal: None,
            ghost: None,
            also: &["timeline"],
        },
        Step {
            anchor: "preview",
            text: |_| tr!("tutorial.masks_place").into(),
            done: |a| mask(a).is_some_and(|m| covers(&m, START)),
            show_me: |a| edit_mask(a, |m| m.points = START.to_vec()),
            reveal: None,
            ghost: Some(START),
            also: &[],
        },
        Step {
            anchor: "inspector.feather",
            text: |_| tr!("tutorial.masks_feather").into(),
            done: |a| mask(a).is_some_and(|m| m.feather >= SOFT),
            show_me: |a| edit_mask(a, |m| m.feather = SOFT * 2.0),
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "inspector.invert",
            text: |_| tr!("tutorial.masks_invert").into(),
            // On one mask, Subtract cuts it out as Invert does, so both together spotlight
            // the subject again.
            done: |a| mask(a).is_some_and(|m| m.invert && m.mode == MaskMode::Subtract),
            show_me: |a| {
                edit_mask(a, |m| {
                    (m.invert, m.mode) = (true, MaskMode::Subtract);
                })
            },
            reveal: None,
            ghost: None,
            also: &["preview"],
        },
        Step {
            anchor: "inspector.mask_position",
            text: |_| tr!("tutorial.masks_key").into(),
            done: |a| !key_times(a).is_empty(),
            show_me: |a| {
                let Some(c) = top(a) else { return };
                let name = Mask::point(0, 0);
                let t = a.timeline.playhead();
                let cmd = crate::inspector::toggle_animate(&[c], &[&name], t);
                apply(a, cmd);
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "timeline",
            text: |_| tr!("tutorial.masks_playhead").into(),
            done: |a| {
                let (Some(c), keys) = (top(a), key_times(a)) else {
                    return false;
                };
                let here = c.source_at(a.timeline.playhead());
                !keys.is_empty() && keys.iter().all(|k| (k.0 - here.0).abs() >= SECOND.0)
            },
            show_me: |a| {
                if let Some(c) = top(a) {
                    a.timeline.seek(Time(c.start.0 + 4 * SECOND.0).min(c.end()));
                }
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "preview",
            text: |_| tr!("tutorial.masks_follow").into(),
            done: |a| key_times(a).len() >= 2,
            show_me: |a| {
                let Some(c) = top(a) else { return };
                let t = a.timeline.playhead();
                let mut x = c.transform_at(t);
                if let Some(m) = x.masks.first_mut() {
                    m.points = END.to_vec();
                }
                let names = [Mask::point(0, 0), Mask::point(0, 1)];
                let names = names.each_ref().map(String::as_str);
                let cmd = crate::inspector::set_props(&[c], &names, &x, t);
                apply(a, cmd);
            },
            reveal: None,
            ghost: Some(END),
            also: &[],
        },
        Step {
            anchor: "preview",
            text: |a| tr!("tutorial.masks_play", key = a.keymap.shortcut("play_stop")),
            done: |a| a.preview.playing(),
            show_me: |a| {
                if let Some(c) = top(a) {
                    a.timeline.seek(c.start);
                }
                a.preview.program_action("play_stop");
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
    ],
};

/// The original: the first clip on the lowest video track that has one.
fn lower(a: &App) -> Option<&Clip> {
    let seq = a.history.project().sequence();
    let video = seq.tracks().iter().filter(|t| t.kind == TrackKind::Video);
    video.rev().find_map(|t| t.clips().first())
}

/// A copy of [`lower`] on a video track above it, overlapping it.
fn top(a: &App) -> Option<&Clip> {
    let low = lower(a)?;
    let seq = a.history.project().sequence();
    let tracks = seq.tracks().iter().filter(|t| t.kind == TrackKind::Video);
    let above = tracks.take_while(|t| !t.clips().iter().any(|c| c.id == low.id));
    above
        .flat_map(|t| t.clips())
        .find(|c| c.asset == low.asset && c.start < low.end() && low.start < c.end())
}

/// [`top`]'s first mask at the playhead.
fn mask(a: &App) -> Option<Mask> {
    let x = top(a)?.transform_at(a.timeline.playhead());
    x.masks.into_iter().next()
}

/// Changes [`top`]'s first mask, or adds `f`'s mask if it has none, as one undo step.
fn edit_mask(a: &mut App, f: impl Fn(&mut Mask)) {
    let Some(c) = top(a) else { return };
    let (mut x, keys) = (c.transform.clone(), c.keyframes.clone());
    if x.masks.is_empty() {
        x.masks.push(Mask::new(MaskShape::Ellipse, START.to_vec()));
    }
    f(&mut x.masks[0]);
    apply(a, Command::SetClipProps(vec![(c.id, x, keys)]));
}

/// The source times [`top`]'s first mask's path is keyed at.
fn key_times(a: &App) -> Vec<Time> {
    let Some(c) = top(a) else { return Vec::new() };
    let n = c.transform.masks.first().map_or(0, |m| m.points.len());
    let mut times: Vec<Time> = (0..n)
        .filter_map(|p| c.keyframes.get(&Mask::point(0, p)))
        .flatten()
        .map(|k| k.time)
        .collect();
    times.sort();
    times.dedup();
    times
}

/// Whether ellipse mask `m` covers `target` (centre and size), sharing [`COVERS`] of their
/// combined area.
fn covers(m: &Mask, target: [[f64; 2]; 2]) -> bool {
    m.shape == MaskShape::Ellipse && overlap([m.points[0], m.points[1]], target) >= COVERS
}

/// Ellipses `a` and `b`'s (centre and size) intersection over their union, 0..=1, sampled
/// on a grid over both.
fn overlap(a: [[f64; 2]; 2], b: [[f64; 2]; 2]) -> f64 {
    const N: usize = 100;
    let inside = |[c, s]: [[f64; 2]; 2], p: [f64; 2]| {
        let d = [0, 1].map(|i| (p[i] - c[i]) / (s[i] / 2.0).max(1e-9));
        d[0] * d[0] + d[1] * d[1] <= 1.0
    };
    let lo = [0, 1].map(|i| (a[0][i] - a[1][i] / 2.0).min(b[0][i] - b[1][i] / 2.0));
    let hi = [0, 1].map(|i| (a[0][i] + a[1][i] / 2.0).max(b[0][i] + b[1][i] / 2.0));
    let (mut both, mut either) = (0, 0);
    for y in 0..N {
        for x in 0..N {
            let at = |i: usize, k: usize| lo[i] + (k as f64 + 0.5) / N as f64 * (hi[i] - lo[i]);
            let p = [at(0, x), at(1, y)];
            let (i, j) = (inside(a, p), inside(b, p));
            both += usize::from(i && j);
            either += usize::from(i || j);
        }
    }
    if either == 0 {
        return 0.0;
    }
    both as f64 / either as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ellipse(c: [f64; 2], s: [f64; 2]) -> Mask {
        Mask::new(MaskShape::Ellipse, vec![c, s])
    }

    #[test]
    fn the_mask_covers_the_outline_from_sixty_percent_overlap() {
        let [c, s] = START;
        assert!(covers(&ellipse(c, s), START));
        // Shifted sideways: the overlap falls as the mask slides off the outline.
        let shifted = |dx: f64| ellipse([c[0] + dx, c[1]], s);
        let at = |dx| overlap([shifted(dx).points[0], s], START);
        assert!(at(50.0) >= COVERS, "{}", at(50.0));
        assert!(covers(&shifted(50.0), START));
        assert!(at(110.0) < COVERS, "{}", at(110.0));
        assert!(!covers(&shifted(110.0), START));
        // Too big counts against it as much as off to the side.
        assert!(!covers(&ellipse(c, [s[0] * 2.0, s[1] * 2.0]), START));
        // Add Mask's ellipse, centred and half the 1280x720 frame, doesn't start covered.
        assert!(!covers(&ellipse([0.0, 0.0], [640.0, 360.0]), START));
        let rect = Mask::new(MaskShape::Rectangle, START.to_vec());
        assert!(!covers(&rect, START), "only an ellipse");
    }

    #[test]
    fn overlap_is_area_shared_over_area_covered() {
        let a = [[0.0, 0.0], [100.0, 100.0]];
        assert!((overlap(a, a) - 1.0).abs() < 1e-9);
        assert_eq!(overlap(a, [[500.0, 0.0], [100.0, 100.0]]), 0.0);
        // A circle inside one twice as wide: a quarter of the area.
        let quarter = overlap(a, [[0.0, 0.0], [200.0, 200.0]]);
        assert!((quarter - 0.25).abs() < 0.02, "{quarter}");
    }
}
