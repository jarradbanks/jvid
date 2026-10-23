//! Timeline evaluation: what is seen at a time and heard over a range. Playback,
//! export and the CLI all go through here so preview matches output.

use std::collections::BTreeMap;

use crate::model::{
    AssetId, Clip, ClipId, Project, Sequence, Track, TrackId, TrackKind, Transform, TransitionKind,
};
use crate::time::Time;

/// One visible clip at an instant.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer<'a> {
    pub track: TrackId,
    pub clip: ClipId,
    pub asset: AssetId,
    /// Time within the asset to show.
    pub source_time: Time,
    pub params: &'a BTreeMap<String, f64>,
    /// The clip's transform evaluated at this instant.
    pub transform: Transform,
    /// The clip's `color` keyframes at this instant, straight RGBA; a solid asset's color
    /// applies when `None`.
    pub color: Option<[f64; 4]>,
    /// The clip's keyed text style at this instant, applied over a text asset's with
    /// [`crate::model::Text::with`].
    pub text: Vec<(&'static str, crate::keyframe::Value)>,
    /// A sequence clip's sequence at `source_time`, top first; filled by [`frame_at`].
    pub nested: Vec<Layer<'a>>,
}

/// A piece of one clip to mix, clamped to the requested range.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSegment {
    pub track: TrackId,
    pub clip: ClipId,
    pub asset: AssetId,
    /// Half-open timeline span `[start, end)`.
    pub start: Time,
    pub end: Time,
    /// Asset time matching `start`.
    pub source_start: Time,
    /// Source time per timeline time at `start`; 1 plays as recorded, other rates resample
    /// (pitch shifts with them).
    pub speed: f64,
    /// Asset time matching `end` on a speed ramp, whose speed glides from `speed` to reach it;
    /// `None` at a constant speed.
    pub source_end: Option<Time>,
    /// The source plays backwards from `source_start`.
    pub reverse: bool,
    /// The clip's audio stream; `None` mixes all of them.
    pub stream: Option<u32>,
    /// The segment plays through a transition, so its gain ([`gain_at`]) changes within it.
    pub crossfade: bool,
    pub volume: f32,
    pub pan: f32,
}

/// Tracks of `kind` that contribute: not `off`, and soloed if any track of that kind is.
fn active(
    seq: &Sequence,
    kind: TrackKind,
    off: fn(&Track) -> bool,
) -> impl Iterator<Item = &Track> {
    let tracks = seq.tracks().iter().filter(move |t| t.kind == kind);
    let any_solo = tracks.clone().any(|t| t.solo);
    tracks.filter(move |t| !off(t) && (!any_solo || t.solo))
}

/// The longest a transition reaches past its cut, as [`crate::model::Project::transition_room`]
/// allows.
const REACH: Time = Time(60 * crate::time::FLICKS_PER_SECOND);

/// The transition window `[start, end)` at the cut between `out` and the clip after it, `inc`,
/// if one plays there: both enabled and touching.
fn window(out: &Clip, inc: &Clip) -> Option<(Time, Time)> {
    let x = out.transform.transition?;
    let ok = out.enabled && inc.enabled && inc.start == out.end() && x.duration > Time(0);
    let start = out.end() - Time(x.duration.0 / 2);
    ok.then_some((start, start + x.duration))
}

/// How far through `[start, end)` `t` is, 0..1.
fn progress((start, end): (Time, Time), t: Time) -> f64 {
    (t - start).0 as f64 / (end - start).0 as f64
}

/// A clip with the transition windows at its head and tail.
type Windowed<'a> = (&'a Clip, Option<(Time, Time)>, Option<(Time, Time)>);

/// The span `c` covers: its own, carried past an edge into its media by a transition there.
fn span((c, before, after): Windowed) -> (Time, Time) {
    (
        before.map_or(c.start, |w| w.0),
        after.map_or(c.end(), |w| w.1),
    )
}

/// Enabled clips on `track` heard or seen within `[start, end)`.
fn clips_in(track: &Track, start: Time, end: Time) -> impl Iterator<Item = Windowed<'_>> {
    let clips = track.clips();
    let first = clips.partition_point(|c| c.end() <= start - REACH);
    let range = if start < end {
        first..clips.len()
    } else {
        0..0
    };
    range
        .take_while(move |&i| clips[i].start < end + REACH)
        .filter(|&i| clips[i].enabled)
        .filter_map(move |i| {
            let c = &clips[i];
            let before = i.checked_sub(1).and_then(|j| window(&clips[j], c));
            let after = clips.get(i + 1).and_then(|n| window(c, n));
            let (s, e) = span((c, before, after));
            (s < end && start < e).then_some((c, before, after))
        })
}

/// `c`'s source time at `t`, into its handles outside the clip.
fn source(c: &Clip, t: Time) -> Time {
    if c.start <= t && t < c.end() {
        c.source_at(t)
    } else {
        c.source_beyond(t)
    }
}

/// `c`'s fades at `t`, held at the clip's edge value through its handles.
fn fade(c: &Clip, t: Time) -> f64 {
    c.fade_at(t.clamp(c.start, Time(c.end().0 - 1).max(c.start)))
}

/// `c`'s transform at `t` with its fades on the opacity.
fn faded(c: &Clip, t: Time) -> Transform {
    let mut x = c.transform_at(t);
    x.opacity *= fade(c, t);
    x
}

fn layer<'a>(tr: TrackId, c: &'a Clip, t: Time) -> Layer<'a> {
    Layer {
        track: tr,
        clip: c.id,
        asset: c.asset,
        source_time: source(c, t),
        params: &c.params,
        transform: faded(c, t),
        color: match c.value_at("color", t) {
            Some(crate::keyframe::Value::Color(c)) => Some(c),
            _ => None,
        },
        text: c.text_keys(t),
        nested: Vec::new(),
    }
}

/// Visible video layers at `t`, top to bottom. Through a crossfade the incoming clip fades in
/// over the outgoing one; a dip covers the outgoing clip with a full-frame solid that then
/// uncovers the incoming one.
pub fn video_at(seq: &Sequence, t: Time) -> Vec<Layer<'_>> {
    active(seq, TrackKind::Video, |t| t.hidden)
        .flat_map(|tr| {
            let near: Vec<_> = clips_in(tr, t, t + Time(1)).map(|(c, ..)| c).collect();
            // Overlapping windows around a clip shorter than them show each clip whole.
            let (out, inc, w) = match near.as_slice() {
                &[out, inc] if let Some(w) = window(out, inc) => (out, inc, w),
                _ => return near.into_iter().map(|c| layer(tr.id, c, t)).collect(),
            };
            let p = progress(w, t);
            let kind = out.transform.transition.expect("windowed").kind;
            let rgb = match kind {
                TransitionKind::Crossfade => {
                    let mut top = layer(tr.id, inc, t);
                    top.transform.opacity *= p;
                    return vec![top, layer(tr.id, out, t)];
                }
                TransitionKind::DipToBlack => 0.0,
                TransitionKind::DipToWhite => 1.0,
            };
            let (under, a) = if p < 0.5 {
                (out, 2.0 * p)
            } else {
                (inc, 2.0 - 2.0 * p)
            };
            // shortcut: the dip is the clip's layer drawn as a solid, so a text clip dips as
            // its text in the dip color; give Layer a solid of its own if that matters.
            let dip = Layer {
                transform: Transform {
                    opacity: a,
                    ..Transform::default()
                },
                color: Some([rgb, rgb, rgb, 1.0]),
                text: Vec::new(),
                ..layer(tr.id, under, t)
            };
            vec![dip, layer(tr.id, under, t)]
        })
        .collect()
}

/// [`video_at`] on the open sequence, with each sequence clip's layers in
/// [`Layer::nested`] however deep; edits refuse cycles, so it ends. Playback, export and the
/// CLI draw this.
pub fn frame_at(p: &Project, t: Time) -> Vec<Layer<'_>> {
    nest(p, video_at(p.sequence(), t))
}

fn nest<'a>(p: &'a Project, mut layers: Vec<Layer<'a>>) -> Vec<Layer<'a>> {
    for l in &mut layers {
        if let Some(s) = p.nested(l.asset) {
            l.nested = nest(p, video_at(s, l.source_time));
        }
    }
    layers
}

/// `c`'s gain from its fades and its transitions at `t` on `track`, 0..=1; transitions are
/// equal-power crossfades whatever their kind.
pub fn gain_at(track: &Track, c: &Clip, t: Time) -> f64 {
    let clips = track.clips();
    let i = clips.iter().position(|o| o.id == c.id);
    let before = i
        .and_then(|i| i.checked_sub(1))
        .and_then(|j| window(&clips[j], c));
    let after = i.and_then(|i| clips.get(i + 1)).and_then(|n| window(c, n));
    let quarter = std::f64::consts::FRAC_PI_2;
    let mut g = fade(c, t);
    if let Some(w) = before.filter(|w| w.0 <= t && t < w.1) {
        g *= (progress(w, t) * quarter).sin();
    }
    if let Some(w) = after.filter(|w| w.0 <= t && t < w.1) {
        g *= (progress(w, t) * quarter).cos();
    }
    g
}

/// Audible segments within `[start, end)`, in track order then time order.
pub fn audio_in(seq: &Sequence, start: Time, end: Time) -> Vec<AudioSegment> {
    active(seq, TrackKind::Audio, |t| t.muted)
        .flat_map(|tr| {
            // A freeze is silent.
            clips_in(tr, start, end)
                .filter(|(c, ..)| c.freeze.is_none())
                .map(move |w @ (c, before, after)| {
                    let (cs, ce) = span(w);
                    let (s, e) = (start.max(cs), end.min(ce));
                    AudioSegment {
                        track: tr.id,
                        clip: c.id,
                        asset: c.asset,
                        start: s,
                        end: e,
                        source_start: source(c, s),
                        speed: c.speed_at(s),
                        source_end: c.ramp().map(|_| source(c, e)),
                        reverse: c.reverse,
                        stream: c.stream,
                        crossfade: before.is_some_and(|w| s < w.1)
                            || after.is_some_and(|w| w.0 < e),
                        volume: tr.volume,
                        pan: tr.pan,
                    }
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelError, NewClip};

    fn clip(start: i64, len: i64) -> NewClip {
        NewClip {
            asset: AssetId(1),
            source_in: Time(100),
            source_out: Time(100 + len),
            start: Time(start),
            link: None,
        }
    }

    fn ids(layers: &[Layer]) -> Vec<ClipId> {
        layers.iter().map(|l| l.clip).collect()
    }

    #[test]
    fn video_stack_cut_points_and_source_time() {
        let mut s = Sequence::default();
        let v1 = s.add_track(TrackKind::Video, "V1");
        let v2 = s.add_track(TrackKind::Video, "V2");
        s.add_track(TrackKind::Audio, "A1");
        let a = s.add_clip(v1, clip(0, 10)).unwrap();
        let b = s.add_clip(v1, clip(10, 10)).unwrap();
        let top = s.add_clip(v2, clip(5, 10)).unwrap();
        s.with_clip_mut(top, |p| p.params.insert("opacity".into(), 0.5))
            .unwrap();

        assert_eq!(ids(&video_at(&s, Time(0))), [a]);
        assert_eq!(ids(&video_at(&s, Time(9))), [top, a]);
        // exact cut: outgoing clip is gone, incoming starts at its source_in
        let at10 = video_at(&s, Time(10));
        assert_eq!(ids(&at10), [top, b]);
        assert_eq!(at10[1].source_time, Time(100));
        assert_eq!(at10[0].source_time, Time(105));
        assert_eq!(at10[0].params["opacity"], 0.5);
        assert_eq!(ids(&video_at(&s, Time(15))), [b]);
        assert_eq!(ids(&video_at(&s, Time(20))), []);
        assert_eq!(ids(&video_at(&s, Time(-1))), []);

        s.track_mut(v2).unwrap().hidden = true;
        assert_eq!(ids(&video_at(&s, Time(9))), [a]);
        s.track_mut(v2).unwrap().hidden = false;
        s.track_mut(v2).unwrap().solo = true;
        assert_eq!(ids(&video_at(&s, Time(9))), [top]);
    }

    #[test]
    fn audio_segments_clamped_and_filtered() {
        let mut s = Sequence::default();
        s.add_track(TrackKind::Video, "V1");
        let a1 = s.add_track(TrackKind::Audio, "A1");
        let a2 = s.add_track(TrackKind::Audio, "A2");
        let x = s.add_clip(a1, clip(0, 10)).unwrap();
        let y = s.add_clip(a1, clip(10, 10)).unwrap();
        let z = s.add_clip(a2, clip(5, 10)).unwrap();
        s.track_mut(a2).unwrap().volume = 0.5;

        let segs = audio_in(&s, Time(8), Time(12));
        let got: Vec<_> = segs
            .iter()
            .map(|g| (g.clip, g.start, g.end, g.source_start))
            .collect();
        assert_eq!(
            got,
            [
                (x, Time(8), Time(10), Time(108)),
                (y, Time(10), Time(12), Time(100)),
                (z, Time(8), Time(12), Time(103)),
            ]
        );
        assert_eq!(segs[2].volume, 0.5);
        // ranges ending or starting exactly on a cut touch only one side
        let only = |s: &Sequence, a, b| -> Vec<_> {
            audio_in(s, Time(a), Time(b))
                .iter()
                .map(|g| g.clip)
                .collect()
        };
        assert_eq!(only(&s, 0, 5), [x]);
        assert_eq!(only(&s, 15, 20), [y]);
        assert_eq!(only(&s, 20, 30), Vec::<ClipId>::new());
        assert_eq!(only(&s, 5, 5), Vec::<ClipId>::new());

        s.track_mut(a1).unwrap().muted = true;
        assert_eq!(only(&s, 8, 12), [z]);
        s.track_mut(a1).unwrap().muted = false;
        s.track_mut(a1).unwrap().solo = true;
        assert_eq!(only(&s, 8, 12), [x, y]);
        // solo on a muted track stays silent
        s.track_mut(a1).unwrap().muted = true;
        assert_eq!(only(&s, 8, 12), Vec::<ClipId>::new());
    }

    #[test]
    fn fades_scale_opacity_and_survive_trims() {
        use crate::model::FadeCurve;
        let mut s = Sequence::default();
        let v = s.add_track(TrackKind::Video, "V1");
        let c = s.add_clip(v, clip(0, 100)).unwrap();
        s.with_clip_mut(c, |c| {
            c.transform.fade_in = Time(20);
            c.transform.fade_out = Time(50);
        })
        .unwrap();
        let op = |s: &Sequence, t| video_at(s, Time(t))[0].transform.opacity;
        assert_eq!(op(&s, 0), 0.0);
        assert_eq!(op(&s, 10), 0.5);
        assert_eq!(op(&s, 30), 1.0);
        assert_eq!(op(&s, 75), 0.5);
        let (_, k) = s.clip(c).unwrap();
        assert!(k.gain_animated());
        // Trimming the head shortens the clip; the fade-in still starts at the new edge.
        s.set_clip_range(c, Time(40), Time(140), Time(200)).unwrap();
        assert_eq!(op(&s, 40), 0.0);
        assert_eq!(op(&s, 50), 0.5);
        s.with_clip_mut(c, |c| c.transform.fade_curve = FadeCurve::EqualPower)
            .unwrap();
        assert!((op(&s, 50) - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
    }

    #[test]
    fn color_keyframes_animate_the_layer_color() {
        use crate::keyframe::{Easing, Keyframe, Value};
        let mut s = Sequence::default();
        let v = s.add_track(TrackKind::Video, "V1");
        let c = s.add_clip(v, clip(0, 10)).unwrap();
        assert_eq!(video_at(&s, Time(0))[0].color, None);
        let key = |time, c| Keyframe {
            time: Time(time),
            value: Value::Color(c),
            easing: Easing::Linear,
        };
        s.with_clip_mut(c, |c| {
            c.keyframes.insert(
                "color".into(),
                vec![
                    key(100, [0.0, 0.0, 0.0, 1.0]),
                    key(110, [1.0, 0.5, 0.0, 1.0]),
                ],
            )
        })
        .unwrap();
        assert_eq!(video_at(&s, Time(5))[0].color, Some([0.5, 0.25, 0.0, 1.0]));
    }

    #[test]
    fn text_keyframes_animate_the_style() {
        use crate::keyframe::{Easing, Keyframe, Value};
        use crate::model::Text;
        let mut s = Sequence::default();
        let v = s.add_track(TrackKind::Video, "V1");
        let c = s.add_clip(v, clip(0, 10)).unwrap();
        let key = |time, n| Keyframe {
            time: Time(time),
            value: Value::Scalar(n),
            easing: Easing::Linear,
        };
        s.with_clip_mut(c, |c| {
            c.keyframes
                .insert("text_size".into(), vec![key(100, 20.0), key(110, 40.0)]);
            c.keyframes.insert("outline".into(), vec![key(100, 4.0)]);
        })
        .unwrap();
        let t = Text::default().with(&video_at(&s, Time(5))[0].text);
        assert_eq!((t.size, t.outline), (30.0, 4.0));
        assert_eq!(t.box_padding, Text::default().box_padding, "unkeyed stays");
        for name in Text::ANIMATED {
            let mut u = Text::default();
            u.set(name, u.get(name).unwrap());
            assert_eq!(u, Text::default(), "{name} round-trips");
        }
    }

    #[test]
    fn speed_reverse_and_freeze_remap_time() {
        let mut s = Sequence::default();
        let v = s.add_track(TrackKind::Video, "V1");
        let a = s.add_track(TrackKind::Audio, "A1");
        let c = s.add_clip(v, clip(0, 100)).unwrap();
        let src = |s: &Sequence, t| video_at(s, Time(t))[0].source_time;
        let end = |s: &Sequence| s.clip(c).unwrap().1.end();

        s.set_speed(c, 2.0, false, false).unwrap();
        assert_eq!(end(&s), Time(50), "200% is half as long");
        assert_eq!(src(&s, 10), Time(120));
        s.set_speed(c, 0.5, false, false).unwrap();
        assert_eq!(end(&s), Time(200));
        assert_eq!(src(&s, 10), Time(105));
        s.set_speed(c, 1.0, true, false).unwrap();
        assert_eq!(src(&s, 0), Time(199), "reverse starts on the last instant");
        assert_eq!(src(&s, 99), Time(100));
        assert_eq!(s.set_speed(c, 0.0, false, false), Err(ModelError::BadSpeed));

        // Slowing down without ripple can't run over the next clip; with it the next moves.
        s.set_speed(c, 1.0, false, false).unwrap();
        let next = s.add_clip(v, clip(100, 10)).unwrap();
        assert!(matches!(
            s.set_speed(c, 0.5, false, false),
            Err(ModelError::Overlap(_))
        ));
        s.set_speed(c, 0.5, false, true).unwrap();
        assert_eq!(s.clip(next).unwrap().1.start, Time(200));
        s.set_speed(c, 2.0, false, true).unwrap();
        assert_eq!(s.clip(next).unwrap().1.start, Time(50));

        // A split keeps both halves mapping as the whole did, reversed too.
        s.set_speed(c, 2.0, true, true).unwrap();
        assert_eq!(src(&s, 10), Time(179));
        s.split_at(&[c], Time(20)).unwrap();
        assert_eq!(src(&s, 10), Time(179));
        assert_eq!(src(&s, 30), Time(139));
        let tail = s.clips_at(Time(30)).next().unwrap().1.clone();
        assert_eq!((tail.start, tail.end()), (Time(20), Time(50)));
        assert_eq!((tail.source_in, tail.source_out), (Time(100), Time(160)));

        // Freeze holds the frame at the playhead for the rest of the clip and is silent.
        let f = s.add_clip(a, clip(0, 100)).unwrap();
        s.set_speed(f, 2.0, false, false).unwrap();
        s.freeze_frame(f, Time(10)).unwrap();
        let segs = audio_in(&s, Time(0), Time(100));
        assert_eq!(segs.len(), 1);
        assert_eq!((segs[0].end, segs[0].speed), (Time(10), 2.0));
        let held = s
            .clips_at(Time(30))
            .find(|(t, _)| *t == a)
            .unwrap()
            .1
            .clone();
        assert_eq!(
            (held.start, held.end()),
            (Time(10), Time(50)),
            "keeps its length"
        );
        assert_eq!(
            (held.source_at(Time(10)), held.source_at(Time(49))),
            (Time(120), Time(120))
        );
        assert_eq!(s.freeze_frame(f, Time(60)), Err(ModelError::OutsideClip(f)));
    }

    #[test]
    fn trims_at_speed_follow_the_media() {
        use crate::model::{Edge, Project};
        use crate::time::FLICKS_PER_SECOND as S;
        let t = |n: i64| Time(n * S);
        let mut p = Project::default();
        let mut s = p.sequence().clone();
        let v = s.add_track(TrackKind::Video, "V1");
        let new = NewClip {
            asset: AssetId(1),
            source_in: t(100),
            source_out: t(200),
            start: t(10),
            link: None,
        };
        let c = s.add_clip(v, new).unwrap();
        s.set_speed(c, 2.0, false, false).unwrap();
        p.sequence = s;
        // 100s of source before source_in is 50s on the timeline at 200%; zero stops it at 10s.
        assert_eq!(p.trim_limit(c, Edge::Head, t(-80), true), t(-50));
        assert_eq!(p.trim_limit(c, Edge::Head, t(-80), false), t(-10));
        p.trim(&[(c, Edge::Head, t(10))], false).unwrap();
        let k = p.sequence().clip(c).unwrap().1.clone();
        assert_eq!((k.start, k.end(), k.source_in), (t(20), t(60), t(120)));
        p.trim(&[(c, Edge::Tail, t(-10))], false).unwrap();
        let k = p.sequence().clip(c).unwrap().1.clone();
        assert_eq!((k.end(), k.source_out), (t(50), t(180)));
    }

    fn transition(s: &mut Sequence, c: ClipId, kind: TransitionKind, d: i64) {
        use crate::model::Transition;
        s.with_clip_mut(c, |c| {
            c.transform.transition = Some(Transition {
                kind,
                duration: Time(d),
            })
        })
        .unwrap();
    }

    #[test]
    fn crossfade_overlaps_the_cut_from_the_handles() {
        let mut s = Sequence::default();
        let v = s.add_track(TrackKind::Video, "V1");
        let a = s.add_clip(v, clip(0, 100)).unwrap();
        let b = s.add_clip(v, clip(100, 100)).unwrap();
        transition(&mut s, a, TransitionKind::Crossfade, 20);
        let at = |t| video_at(&s, Time(t));
        assert_eq!(ids(&at(89)), [a]);
        assert_eq!(ids(&at(90)), [b, a], "incoming on top");
        // b shows 10 before its source_in, a 10 past its source_out.
        assert_eq!(at(90)[0].source_time, Time(90));
        assert_eq!(at(105)[1].source_time, Time(205));
        assert_eq!(at(95)[0].transform.opacity, 0.25);
        assert_eq!(at(95)[1].transform.opacity, 1.0);
        assert_eq!(ids(&at(110)), [b]);
        // Moving b off the cut or disabling it drops the transition.
        s.with_clip_mut(b, |c| *c.enabled = false).unwrap();
        let at = |t| ids(&video_at(&s, Time(t)));
        assert_eq!(at(95), [a]);
        assert_eq!(at(105), []);
    }

    #[test]
    fn dips_cover_the_cut_with_a_solid() {
        let mut s = Sequence::default();
        let v = s.add_track(TrackKind::Video, "V1");
        let a = s.add_clip(v, clip(0, 100)).unwrap();
        let b = s.add_clip(v, clip(100, 100)).unwrap();
        transition(&mut s, a, TransitionKind::DipToWhite, 40);
        let at = |t| video_at(&s, Time(t));
        let l = at(90);
        assert_eq!(ids(&l), [a, a]);
        assert_eq!(l[0].color, Some([1.0; 4]));
        assert_eq!(l[0].transform.opacity, 0.5);
        assert_eq!(l[1].color, None);
        assert_eq!(at(100)[0].transform.opacity, 1.0, "solid at the cut");
        let l = at(110);
        assert_eq!(ids(&l), [b, b]);
        assert_eq!(l[0].transform.opacity, 0.5);
        transition(&mut s, a, TransitionKind::DipToBlack, 40);
        assert_eq!(video_at(&s, Time(110))[0].color, Some([0.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn transitions_follow_speed_and_reverse_into_the_handles() {
        let mut s = Sequence::default();
        let v = s.add_track(TrackKind::Video, "V1");
        let a = s.add_clip(v, clip(0, 200)).unwrap();
        s.set_speed(a, 2.0, false, false).unwrap();
        let b = s.add_clip(v, clip(100, 100)).unwrap();
        s.set_speed(b, 1.0, true, false).unwrap();
        transition(&mut s, a, TransitionKind::Crossfade, 20);
        let l = video_at(&s, Time(105));
        assert_eq!(ids(&l), [b, a]);
        // a at 200% runs 10 past its end as 20 of source; b reversed shows source after its out.
        assert_eq!(l[1].source_time, Time(310));
        assert_eq!(video_at(&s, Time(95))[0].source_time, Time(204));
        assert_eq!(l[0].source_time, Time(194));
    }

    #[test]
    fn audio_crossfades_at_equal_power() {
        let mut s = Sequence::default();
        let t = s.add_track(TrackKind::Audio, "A1");
        let a = s.add_clip(t, clip(0, 100)).unwrap();
        let b = s.add_clip(t, clip(100, 100)).unwrap();
        transition(&mut s, a, TransitionKind::DipToBlack, 20);
        let segs = audio_in(&s, Time(80), Time(120));
        let got: Vec<_> = segs
            .iter()
            .map(|g| (g.clip, g.start, g.end, g.source_start, g.crossfade))
            .collect();
        assert_eq!(
            got,
            [
                (a, Time(80), Time(110), Time(180), true),
                (b, Time(90), Time(120), Time(90), true),
            ]
        );
        let tr = &s.tracks()[0];
        let (ca, cb) = (&tr.clips()[0], &tr.clips()[1]);
        for x in 90..110 {
            let (ga, gb) = (gain_at(tr, ca, Time(x)), gain_at(tr, cb, Time(x)));
            assert!(
                (ga * ga + gb * gb - 1.0).abs() < 1e-9,
                "constant power at {x}"
            );
        }
        assert_eq!(gain_at(tr, ca, Time(50)), 1.0);
        assert_eq!(gain_at(tr, cb, Time(90)), 0.0);
        assert!(!audio_in(&s, Time(0), Time(50))[0].crossfade);
    }

    #[test]
    fn transition_room_needs_handles_and_splits_keep_it_at_the_cut() {
        use crate::model::Project;
        let mut p = Project::default();
        let mut s = p.sequence().clone();
        let v = s.add_track(TrackKind::Video, "V1");
        let a = s.add_clip(v, clip(0, 100)).unwrap();
        let mut from_zero = clip(100, 100);
        from_zero.source_in = Time(0);
        let b = s.add_clip(v, from_zero).unwrap();
        let lone = s.add_clip(v, clip(300, 10)).unwrap();
        p.sequence = s.clone();
        assert_eq!(p.transition_room(a), Err(ModelError::NoHandles(a)));
        assert_eq!(p.transition_room(lone), Err(ModelError::NoHandles(lone)));
        let sec = crate::time::FLICKS_PER_SECOND;
        s.set_clip_range(b, Time(100), Time(sec), Time(sec + 100))
            .unwrap();
        p.sequence = s.clone();
        let room = p.transition_room(a).unwrap();
        assert!(room > Time(0) && room <= Time(2 * sec));
        let r = p.settings().frame_rate;
        assert_eq!(r.time_floor(r.index_floor(room)), room, "whole frames");

        transition(&mut s, b, TransitionKind::Crossfade, 4);
        s.split_at(&[b], Time(150)).unwrap();
        let clips = s.tracks()[0].clips();
        assert_eq!(clips[1].transform.transition, None);
        assert!(clips[2].transform.transition.is_some());
    }
}
