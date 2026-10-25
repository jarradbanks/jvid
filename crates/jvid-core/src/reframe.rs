//! Reframe for Shorts: a copy of a project at another frame size, its video scaled to fill
//! the new frame and its titles kept inside the title-safe area.

use crate::keyframe::{Easing, Keyframe, Value};
use crate::model::{Clip, Project, StreamInfo, Text, TrackKind};

/// Title-safe as a fraction of the frame, as the Program monitor's guide draws it.
pub const TITLE_SAFE: f64 = 0.9;

/// The frame for `aspect` (width:height) made from a `width`×`height` one: its short side
/// becomes the new frame's short side, rounded to even pixels for the encoders.
pub fn frame_size(width: u32, height: u32, aspect: [u32; 2]) -> (u32, u32) {
    let short = width.min(height) as f64;
    let [aw, ah] = aspect.map(f64::from);
    let (w, h) = if aw <= ah {
        (short, short * ah / aw)
    } else {
        (short * aw / ah, short)
    };
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    (even(w), even(h))
}

/// How much to scale a clip whose visible part is `size` project pixels (fitted whole into
/// the frame as the compositor does, then cropped) so, turned `rotation` degrees about its
/// centre on the frame's centre, it covers a `frame`-sized frame with no bars.
pub fn fill_scale(size: [f64; 2], frame: [f64; 2], rotation: f64) -> f64 {
    let (sin, cos) = rotation.to_radians().sin_cos();
    let (sin, cos) = (sin.abs(), cos.abs());
    // The frame's extent along each of the turned clip's axes.
    let across = frame[0] * cos + frame[1] * sin;
    let down = frame[0] * sin + frame[1] * cos;
    (across / size[0]).max(down / size[1])
}

/// The scale fitting an `image`-sized picture whole into `frame`, as the compositor does.
fn fit(image: [f64; 2], frame: [f64; 2]) -> f64 {
    (frame[0] / image[0]).min(frame[1] / image[1])
}

/// How far to move `bounds` (`[x0, y0, x1, y1)`, pixels) so it lies inside the title-safe
/// area of a `frame`-sized frame; centred on an axis it is too big for.
pub fn into_safe(bounds: [f64; 4], frame: (u32, u32)) -> [f64; 2] {
    let axis = |lo: f64, hi: f64, size: u32| {
        let size = size as f64;
        let margin = size * (1.0 - TITLE_SAFE) / 2.0;
        if hi - lo > size - 2.0 * margin {
            size / 2.0 - (lo + hi) / 2.0
        } else {
            (margin - lo).max(0.0) - (hi - (size - margin)).max(0.0)
        }
    };
    [
        axis(bounds[0], bounds[2], frame.0),
        axis(bounds[1], bounds[3], frame.1),
    ]
}

impl Project {
    /// A copy at `width`×`height`. Each video clip from a file is scaled to fill the frame by
    /// its cropped size at its rotation, centred, with a position key at its start so a drag
    /// pans it from there. Titles keep their size and move inside the title-safe area,
    /// measured by `measure` (the pixel rect a text draws into on a frame of a size, as
    /// `jvid_render::text::drawn_bounds`): their anchors and anchor keys, then each clip's
    /// position and position keys, measured at the clip's largest text size.
    // shortcut: a title is measured at its static style and largest `text_size`, unscaled and
    // unturned; measure each key's full style and the clip's scale and rotation if titles use them.
    pub fn reframed(
        &self,
        width: u32,
        height: u32,
        measure: impl Fn(&Text, (u32, u32)) -> Option<[u32; 4]>,
    ) -> Project {
        let mut p = self.clone();
        p.settings.width = width;
        p.settings.height = height;
        let frame = (width, height);
        let (w, h) = (width as f64, height as f64);
        let old = [self.settings.width, self.settings.height].map(f64::from);
        // Where `text` draws with its centre at `anchor`. Measured centred, since drawn bounds
        // stop at the frame's edges, then moved: the anchor is the block's centre.
        let bounds = |text: &Text, anchor: [f64; 2]| {
            let mut t = text.clone();
            t.anchor = [0.5; 2];
            let [x0, y0, x1, y1] = measure(&t, frame)?.map(f64::from);
            let (dx, dy) = (anchor[0] * w - w / 2.0, anchor[1] * h - h / 2.0);
            Some([x0 + dx, y0 + dy, x1 + dx, y1 + dy])
        };
        let safe = |text: &Text, anchor: [f64; 2]| {
            let Some(b) = bounds(text, anchor) else {
                return anchor;
            };
            let [mx, my] = into_safe(b, frame);
            [anchor[0] + mx / w, anchor[1] + my / h]
        };
        for a in &mut p.assets {
            if let Some(t) = &a.text {
                let anchor = safe(t, t.anchor.map(f64::from));
                a.text.as_mut().unwrap().anchor = anchor.map(|v| v as f32);
            }
        }
        let seqs = std::iter::once(&mut p.sequence).chain(&mut p.sequences);
        for seq in seqs {
            let video = seq.tracks().iter().filter(|t| t.kind == TrackKind::Video);
            let clips: Vec<Clip> = video.flat_map(|t| t.clips()).cloned().collect();
            for mut c in clips {
                let Some(asset) = self.asset(c.asset) else {
                    continue;
                };
                if let Some(text) = &asset.text {
                    // The biggest the clip draws its text: a bigger block contains a smaller.
                    let sizes = keyed(&c, "text_size").chain([text.size.into()]);
                    let big = Text {
                        size: sizes.fold(1.0, f64::max) as f32,
                        ..text.clone()
                    };
                    if let Some(keys) = c.keyframes.get_mut("text_anchor") {
                        for k in keys {
                            if let Value::Vec2(v) = k.value {
                                k.value = Value::Vec2(safe(&big, v));
                            }
                        }
                    }
                    let anchors: Vec<[f64; 2]> = match c.keyframes.get("text_anchor") {
                        Some(keys) => keys.iter().filter_map(|k| vec2(k.value)).collect(),
                        // The asset's anchor, as made safe above.
                        None => vec![safe(text, text.anchor.map(f64::from))],
                    };
                    let blocks: Vec<_> = anchors.iter().filter_map(|&a| bounds(&big, a)).collect();
                    keep_safe(&mut c, &blocks, frame);
                } else if let Some(image) = asset.probe.as_ref().and_then(upright) {
                    fill(&mut c, image, old, [w, h]);
                }
                let set = |m: &mut crate::model::ClipProps<'_>| {
                    *m.transform = c.transform;
                    *m.keyframes = c.keyframes;
                };
                seq.with_clip_mut(c.id, set)
                    .expect("the clip was just listed");
            }
        }
        p
    }
}

/// The values of `c`'s scalar keys for `param`.
fn keyed<'a>(c: &'a Clip, param: &str) -> impl Iterator<Item = f64> + 'a {
    let keys = c.keyframes.get(param).into_iter().flatten();
    keys.filter_map(|k| match k.value {
        Value::Scalar(v) => Some(v),
        _ => None,
    })
}

fn vec2(v: Value) -> Option<[f64; 2]> {
    match v {
        Value::Vec2(v) => Some(v),
        _ => None,
    }
}

/// Moves a title clip's position, or each position key, the least that keeps every one of
/// `blocks` (where its text draws unmoved) inside title-safe; blocks too big for it are
/// centred together.
fn keep_safe(c: &mut Clip, blocks: &[[f64; 4]], frame: (u32, u32)) {
    if blocks.is_empty() {
        return;
    }
    let axis = |i: usize, size: u32, p: f64| {
        let size = size as f64;
        let margin = size * (1.0 - TITLE_SAFE) / 2.0;
        let lo = blocks.iter().map(|b| b[i]).fold(f64::INFINITY, f64::min);
        let hi = blocks
            .iter()
            .map(|b| b[i + 2])
            .fold(f64::NEG_INFINITY, f64::max);
        // Each block's edges move with the position, so these bound it for all of them.
        let (min, max) = (margin - lo, size - margin - hi);
        if min > max {
            (min + max) / 2.0
        } else {
            p.clamp(min, max)
        }
    };
    let clamp = |p: [f64; 2]| [axis(0, frame.0, p[0]), axis(1, frame.1, p[1])];
    c.transform.position = clamp(c.transform.position);
    for k in c.keyframes.get_mut("position").into_iter().flatten() {
        if let Some(v) = vec2(k.value) {
            k.value = Value::Vec2(clamp(v));
        }
    }
}
/// A probed file's picture size upright, as frames decode; `None` without video.
fn upright(p: &crate::model::ProbeInfo) -> Option<[f64; 2]> {
    let [w, h] = [p.width? as f64, p.height? as f64];
    let turned = p
        .streams
        .iter()
        .any(|s| matches!(s, StreamInfo::Video(v) if v.rotation % 180 == 90));
    Some(if turned { [h, w] } else { [w, h] })
}

/// Fills `frame` with `c`, an `image`-sized clip from a project `old`-sized: its crop keeps
/// the same part of the picture, and the scale covers the frame by the cropped size at every
/// rotation the clip takes.
// shortcut: keyed crops are filled by the crop at the clip's start; key the anchor and scale
// to follow them if reframed clips animate their crop.
fn fill(c: &mut Clip, image: [f64; 2], old: [f64; 2], frame: [f64; 2]) {
    // Crops are fitted-picture pixels, so they scale with the fit.
    let ratio = fit(image, frame) / fit(image, old);
    c.transform.crop = c.transform.crop.map(|v| v * ratio);
    for name in ["crop_left", "crop_top", "crop_right", "crop_bottom"] {
        for k in c.keyframes.get_mut(name).into_iter().flatten() {
            if let Value::Scalar(v) = k.value {
                k.value = Value::Scalar(v * ratio);
            }
        }
    }
    let [l, t, r, b] = c.transform_at(c.start).crop;
    let [w, h] = image.map(|v| v * fit(image, frame));
    let size = [w - l - r, h - t - b];
    if size[0] <= 0.0 || size[1] <= 0.0 {
        return;
    }
    // Every angle the clip turns through: its keys and 1° steps between them.
    let keys: Vec<f64> = keyed(c, "rotation").collect();
    let mut angles = vec![keys.first().copied().unwrap_or(c.transform.rotation)];
    for pair in keys.windows(2) {
        let steps = (pair[1] - pair[0]).abs().ceil().max(1.0);
        angles.extend((1..=steps as u32).map(|i| pair[0] + (pair[1] - pair[0]) * i as f64 / steps));
    }
    let k = angles
        .into_iter()
        .map(|a| fill_scale(size, frame, a))
        .fold(0.0, f64::max);
    c.transform.scale = [k; 2];
    c.transform.position = [0.0; 2];
    // The visible part's centre, so it sits on the frame's and turns about it.
    c.transform.anchor = [(l - r) / 2.0, (t - b) / 2.0];
    c.keyframes.remove("scale");
    c.keyframes.remove("anchor");
    let key = Keyframe {
        time: c.source_at(c.start),
        value: Value::Vec2([0.0; 2]),
        easing: Easing::Linear,
    };
    c.keyframes.insert("position".into(), vec![key]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Command, History};
    use crate::model::{NewClip, ProbeInfo};
    use crate::time::{FLICKS_PER_SECOND as S, Time};

    #[test]
    fn frame_sizes_keep_the_short_side() {
        assert_eq!(frame_size(1920, 1080, [9, 16]), (1080, 1920));
        assert_eq!(frame_size(1920, 1080, [1, 1]), (1080, 1080));
        assert_eq!(frame_size(1920, 1080, [4, 5]), (1080, 1350));
        assert_eq!(frame_size(3840, 2160, [9, 16]), (2160, 3840));
        // Odd results round to even.
        assert_eq!(frame_size(1280, 721, [9, 16]), (722, 1282));
    }

    #[test]
    fn fill_scale_covers_the_frame() {
        // 16:9 fitted into 9:16 is 1080x607.5; filling needs 1920 / 607.5.
        let k = fill_scale([1080.0, 607.5], [1080.0, 1920.0], 0.0);
        assert!((k - 1920.0 / 607.5).abs() < 1e-9);
        assert_eq!(fill_scale([1920.0, 1080.0], [1920.0, 1080.0], 0.0), 1.0);
        // A square turned 45° covers a square frame by its diagonal.
        let k = fill_scale([100.0, 100.0], [100.0, 100.0], 45.0);
        assert!((k - 2f64.sqrt()).abs() < 1e-9);
        // A quarter turn swaps the axes.
        let k = fill_scale([1920.0, 1080.0], [1080.0, 1920.0], 90.0);
        assert!((k - 1.0).abs() < 1e-9);
    }

    #[test]
    fn into_safe_moves_text_in_and_centres_what_doesnt_fit() {
        // 5% margins on a 1000x1000 frame: 50..950.
        let safe = |b: [u32; 4]| into_safe(b.map(f64::from), (1000, 1000)).map(f64::round);
        assert_eq!(safe([100, 100, 200, 200]), [0.0, 0.0]);
        assert_eq!(safe([0, 900, 100, 980]), [50.0, -30.0]);
        // 960 wide is wider than the 900 safe width: centred.
        assert_eq!(safe([0, 100, 960, 200]), [20.0, 0.0]);
    }

    /// A 1920x1080 project with a 16:9 clip from 2s of its source, and a title anchored
    /// near the bottom-right with one `text_anchor` key near the left edge.
    fn project() -> Project {
        use crate::history::Created;
        let mut h = History::new(Project::default());
        let probe = ProbeInfo {
            duration: Time(60 * S),
            width: Some(1920),
            height: Some(1080),
            ..Default::default()
        };
        h.apply(Command::Import(vec![("/n/a.mp4".into(), probe)]))
            .unwrap();
        let text = Text {
            anchor: [0.9, 0.9],
            ..Text::default()
        };
        let Ok(Created::Asset(title)) = h.apply(Command::AddText {
            name: "Title".into(),
            text,
        }) else {
            panic!("text")
        };
        for (asset, source_in) in [(h.project().assets()[0].id, 2 * S), (title, 0)] {
            let Ok(Created::Track(track)) = h.apply(Command::AddTrack {
                kind: TrackKind::Video,
                name: "V".into(),
            }) else {
                panic!("track")
            };
            let clip = NewClip {
                asset,
                source_in: Time(source_in),
                source_out: Time(source_in + 8 * S),
                start: Time(0),
                link: None,
            };
            h.apply(Command::AddClip { track, clip }).unwrap();
        }
        let (_, c) = clip_of(h.project(), title);
        let key = Keyframe {
            time: Time(0),
            value: Value::Vec2([0.1, 0.5]),
            easing: Easing::Linear,
        };
        let keys = [("text_anchor".to_string(), vec![key])].into();
        h.apply(Command::SetClipProps(vec![(
            c.id,
            c.transform.clone(),
            keys,
        )]))
        .unwrap();
        h.project().clone()
    }

    fn clip_of(p: &Project, asset: crate::model::AssetId) -> (crate::model::TrackId, &Clip) {
        let tracks = p.sequence().tracks().iter();
        let mut clips = tracks.flat_map(|t| t.clips().iter().map(|c| (t.id, c)));
        clips.find(|(_, c)| c.asset == asset).unwrap()
    }

    /// A 400x100 text block centred on its anchor, clipped to the frame as drawn bounds are.
    fn measure(t: &Text, (w, h): (u32, u32)) -> Option<[u32; 4]> {
        let (x, y) = (t.anchor[0] as f64 * w as f64, t.anchor[1] as f64 * h as f64);
        let r = |v: f64| v.round().max(0.0) as u32;
        Some([r(x - 200.0), r(y - 50.0), r(x + 200.0), r(y + 50.0)])
    }

    #[test]
    fn reframing_fills_keys_and_keeps_titles_safe() {
        let wide = project();
        let tall = wide.reframed(1080, 1920, measure);
        assert_eq!(wide, project(), "the original is unchanged");
        assert_eq!(
            (tall.settings().width, tall.settings().height),
            (1080, 1920)
        );
        let (video, title) = (tall.assets()[0].id, tall.assets()[1].id);
        let (_, c) = clip_of(&tall, video);
        assert!((c.transform.scale[0] - 1920.0 / 607.5).abs() < 1e-9);
        assert_eq!(c.transform.position, [0.0; 2]);
        let keys = &c.keyframes["position"];
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].time, Time(2 * S), "keyed at the clip's first frame");

        // Right edge of the safe area is 1026: the 400-wide block's centre lands at 826.
        let text = tall.assets()[1].text.as_ref().unwrap();
        assert!((text.anchor[0] as f64 * 1080.0 - 826.0).abs() < 1.0);
        assert!(
            (text.anchor[1] as f64 * 1920.0 - 1728.0).abs() < 1.0,
            "already inside"
        );
        assert_eq!(text.size, Text::default().size, "keeps its size");
        let (_, title) = clip_of(&tall, title);
        assert_eq!(title.transform.scale, [1.0; 2], "titles aren't scaled");
        let Value::Vec2(a) = title.keyframes["text_anchor"][0].value else {
            panic!()
        };
        assert!(
            (a[0] * 1080.0 - 254.0).abs() < 1.0,
            "keyed anchors move in too"
        );
    }

    /// `p` with the clip of `asset` changed by `f`.
    fn edit(p: &Project, asset: crate::model::AssetId, f: impl FnOnce(&mut Clip)) -> Project {
        let mut h = History::new(p.clone());
        let mut c = clip_of(p, asset).1.clone();
        f(&mut c);
        h.apply(Command::SetClipProps(vec![(
            c.id,
            c.transform,
            c.keyframes,
        )]))
        .unwrap();
        h.project().clone()
    }

    fn key(time: i64, value: Value) -> Keyframe {
        Keyframe {
            time: Time(time),
            value,
            easing: Easing::Linear,
        }
    }

    /// Whether the 1920x1080 picture of `c` at timeline time `t`, laid out as
    /// `jvid_render::composite::quad` does, covers every corner of a `frame`-sized frame.
    fn covers(c: &Clip, t: Time, frame: [f64; 2]) -> bool {
        let x = c.transform_at(t);
        let image = [1920.0, 1080.0];
        let [w, h] = image.map(|v| v * fit(image, frame));
        let [l, top, r, b] = x.crop;
        let (sin, cos) = x.rotation.to_radians().sin_cos();
        let corners = [[0.0, 0.0], [frame[0], 0.0], [0.0, frame[1]], frame];
        corners.iter().all(|&[px, py]| {
            let dx = px - frame[0] / 2.0 - x.position[0];
            let dy = py - frame[1] / 2.0 - x.position[1];
            // Undo the clockwise turn, then the scale, back into fitted-picture pixels.
            let u = (dx * cos + dy * sin) / x.scale[0] + x.anchor[0];
            let v = (-dx * sin + dy * cos) / x.scale[1] + x.anchor[1];
            let e = 1e-6;
            u >= -w / 2.0 + l - e
                && u <= w / 2.0 - r + e
                && v >= -h / 2.0 + top - e
                && v <= h / 2.0 - b + e
        })
    }

    #[test]
    fn cropped_and_rotated_clips_fill_with_no_bars() {
        let wide = project();
        let video = wide.assets()[0].id;
        let frame = [1080.0, 1920.0];
        let reframe = |p: &Project| p.reframed(1080, 1920, measure);

        let plain = reframe(&wide);
        assert!(covers(clip_of(&plain, video).1, Time(0), frame));

        // Cropping a quarter off the left and top of the 1920x1080 frame.
        let cropped = edit(&wide, video, |c| {
            c.transform.crop = [480.0, 0.0, 0.0, 270.0]
        });
        let tall = reframe(&cropped);
        let c = clip_of(&tall, video).1;
        assert!(covers(c, Time(0), frame));
        // The same quarter of the picture is cropped at the new fit (1080 wide).
        assert_eq!(c.transform.crop, [270.0, 0.0, 0.0, 607.5 / 4.0]);
        // Filling by the uncropped size would leave bars.
        let uncropped = Clip {
            transform: crate::model::Transform {
                scale: clip_of(&plain, video).1.transform.scale,
                ..c.transform.clone()
            },
            ..c.clone()
        };
        assert!(!covers(&uncropped, Time(0), frame));

        let turned = edit(&wide, video, |c| c.transform.rotation = 30.0);
        let c = clip_of(&reframe(&turned), video).1.clone();
        assert!(covers(&c, Time(0), frame));

        // Keyed from 0° to 90°: covered all the way through, 45° included.
        let spun = edit(&wide, video, |c| {
            let keys = vec![
                key(2 * S, Value::Scalar(0.0)),
                key(6 * S, Value::Scalar(90.0)),
            ];
            c.keyframes.insert("rotation".into(), keys);
        });
        let c = clip_of(&reframe(&spun), video).1.clone();
        for t in 0..=8 {
            assert!(covers(&c, Time(t * S / 2), frame), "at {t}/2s");
        }
    }

    #[test]
    fn titles_stay_safe_at_every_position_and_size_key() {
        let wide = project();
        let title = wide.assets()[1].id;
        // Moved off the bottom, then grown to twice the measured block, which is
        // text_size / 96 times 400x100.
        let keyed = edit(&wide, title, |c| {
            let pos = vec![
                key(0, Value::Vec2([0.0, 0.0])),
                key(4 * S, Value::Vec2([300.0, 900.0])),
            ];
            c.keyframes.insert("position".into(), pos);
            let sizes = vec![
                key(0, Value::Scalar(96.0)),
                key(6 * S, Value::Scalar(192.0)),
            ];
            c.keyframes.insert("text_size".into(), sizes);
        });
        let measure = |t: &Text, (w, h): (u32, u32)| {
            let k = t.size as f64 / 96.0;
            let (x, y) = (t.anchor[0] as f64 * w as f64, t.anchor[1] as f64 * h as f64);
            let r = |v: f64| v.round().clamp(0.0, 1e9) as u32;
            Some([
                r(x - 200.0 * k),
                r(y - 50.0 * k),
                r(x + 200.0 * k),
                r(y + 50.0 * k),
            ])
        };
        let tall = keyed.reframed(1080, 1920, measure);
        let text = tall.assets()[1].text.clone().unwrap();
        let c = clip_of(&tall, title).1;
        let times = ["position", "text_size", "text_anchor"]
            .iter()
            .flat_map(|n| c.keyframes[*n].iter().map(|k| k.time));
        for t in times.chain([Time(S)]) {
            let style = text.with(&c.text_keys(t));
            let [px, py] = c.transform_at(t).position;
            // Measured centred, as drawn bounds stop at the frame's edges.
            let centred = Text {
                anchor: [0.5; 2],
                ..style.clone()
            };
            let [x0, y0, x1, y1] = measure(&centred, (1080, 1920)).unwrap().map(f64::from);
            let dx = style.anchor[0] as f64 * 1080.0 - 540.0 + px;
            let dy = style.anchor[1] as f64 * 1920.0 - 960.0 + py;
            let b = [x0 + dx, y0 + dy, x1 + dx, y1 + dy];
            // Title-safe on 1080x1920 is 54..1026 by 96..1824.
            assert!(
                b[0] >= 54.0 - 1.0
                    && b[2] <= 1026.0 + 1.0
                    && b[1] >= 96.0 - 1.0
                    && b[3] <= 1824.0 + 1.0,
                "{b:?} at {t:?}"
            );
        }
    }
}
