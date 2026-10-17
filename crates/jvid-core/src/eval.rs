//! Timeline evaluation: what is seen at a time and heard over a range. Playback,
//! export and the CLI all go through here so preview matches output.

use std::collections::BTreeMap;

use crate::model::{AssetId, Clip, ClipId, Sequence, Track, TrackId, TrackKind, Transform};
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
    /// The clip's audio stream; `None` mixes all of them.
    pub stream: Option<u32>,
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

/// Clips on `track` intersecting `[start, end)`; clips are sorted and never overlap.
fn clips_in(track: &Track, start: Time, end: Time) -> impl Iterator<Item = &Clip> {
    let clips = track.clips();
    let first = clips.partition_point(|c| c.end() <= start);
    let clips = if start < end { &clips[first..] } else { &[] };
    clips
        .iter()
        .take_while(move |c| c.start < end)
        .filter(|c| c.enabled)
}

/// Visible video layers at `t`, top to bottom.
pub fn video_at(seq: &Sequence, t: Time) -> Vec<Layer<'_>> {
    active(seq, TrackKind::Video, |t| t.hidden)
        .flat_map(|tr| {
            clips_in(tr, t, t + Time(1)).map(move |c| Layer {
                track: tr.id,
                clip: c.id,
                asset: c.asset,
                source_time: c.source_in + (t - c.start),
                params: &c.params,
                transform: c.transform_at(t),
                color: match c.value_at("color", t) {
                    Some(crate::keyframe::Value::Color(c)) => Some(c),
                    _ => None,
                },
            })
        })
        .collect()
}

/// Audible segments within `[start, end)`, in track order then time order.
pub fn audio_in(seq: &Sequence, start: Time, end: Time) -> Vec<AudioSegment> {
    active(seq, TrackKind::Audio, |t| t.muted)
        .flat_map(|tr| {
            clips_in(tr, start, end).map(move |c| {
                let s = start.max(c.start);
                AudioSegment {
                    track: tr.id,
                    clip: c.id,
                    asset: c.asset,
                    start: s,
                    end: end.min(c.end()),
                    source_start: c.source_in + (s - c.start),
                    stream: c.stream,
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
    use crate::model::NewClip;

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
}
