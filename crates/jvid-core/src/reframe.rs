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

/// How much to scale an `image`-sized clip, fitted whole into `frame` as the compositor
/// does, so it covers the frame with no bars.
pub fn fill_scale(image: [f64; 2], frame: [f64; 2]) -> f64 {
    let fit = (frame[0] / image[0]).min(frame[1] / image[1]);
    let fill = (frame[0] / image[0]).max(frame[1] / image[1]);
    fill / fit
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
    /// A copy at `width`×`height`. Each video clip from a file is scaled to fill the frame,
    /// centred, with a position key at its start so a drag pans it from there. Titles keep
    /// their size and move inside the title-safe area, measured by `measure` (the pixel rect
    /// a text draws into on a frame of a size, as `jvid_render::text::drawn_bounds`).
    // shortcut: ignores crop, rotation and a title's own position and text_size keys; handle
    // them if reframed clips use them.
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
        let safe = |text: &Text, anchor: [f64; 2]| {
            // Measured centred, since drawn bounds stop at the frame's edges, then moved to
            // `anchor`: the anchor is the block's centre.
            let mut t = text.clone();
            t.anchor = [0.5; 2];
            let Some(b) = measure(&t, frame) else {
                return anchor;
            };
            let (dx, dy) = (anchor[0] * w - w / 2.0, anchor[1] * h - h / 2.0);
            let [x0, y0, x1, y1] = b.map(f64::from);
            let [mx, my] = into_safe([x0 + dx, y0 + dy, x1 + dx, y1 + dy], frame);
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
                    if let Some(keys) = c.keyframes.get_mut("text_anchor") {
                        for k in keys {
                            if let Value::Vec2(v) = k.value {
                                k.value = Value::Vec2(safe(text, v));
                            }
                        }
                    }
                } else if let Some(image) = asset.probe.as_ref().and_then(upright) {
                    fill(&mut c, image, [w, h]);
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

/// A probed file's picture size upright, as frames decode; `None` without video.
fn upright(p: &crate::model::ProbeInfo) -> Option<[f64; 2]> {
    let [w, h] = [p.width? as f64, p.height? as f64];
    let turned = p
        .streams
        .iter()
        .any(|s| matches!(s, StreamInfo::Video(v) if v.rotation % 180 == 90));
    Some(if turned { [h, w] } else { [w, h] })
}

fn fill(c: &mut Clip, image: [f64; 2], frame: [f64; 2]) {
    let k = fill_scale(image, frame);
    c.transform.scale = [k; 2];
    c.transform.position = [0.0; 2];
    c.transform.anchor = [0.0; 2];
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
        let k = fill_scale([1920.0, 1080.0], [1080.0, 1920.0]);
        assert!((k - 1920.0 / 607.5).abs() < 1e-9);
        assert_eq!(fill_scale([1920.0, 1080.0], [1920.0, 1080.0]), 1.0);
        let k = fill_scale([1920.0, 1080.0], [1080.0, 1080.0]);
        assert!((k - 16.0 / 9.0).abs() < 1e-9);
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
}
