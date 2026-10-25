fn visible_text(project: &Project, layers: &[crate::eval::Layer<'_>], out: &mut Vec<String>) {
    for layer in layers {
        if let Some(text) = project.asset(layer.asset).and_then(|a| a.text.as_ref()) {
            out.push(text.content.clone());
        }
        visible_text(project, &layer.nested, out);
    }
}

use super::*;
use crate::srt::{self, Cue};
use crate::time::FLICKS_PER_SECOND as S;

fn sec(n: i64) -> Time {
    Time(n * S)
}

fn add_clip(
    h: &mut History,
    track: TrackId,
    asset: AssetId,
    start: Time,
    from: Time,
    to: Time,
) -> ClipId {
    let Created::Clip(id) = h
        .apply(Command::AddClip {
            track,
            clip: NewClip {
                asset,
                start,
                source_in: from,
                source_out: to,
                link: None,
            },
        })
        .unwrap()
    else {
        panic!("clip")
    };
    id
}

fn nested_captions() -> (History, Project, ClipId, TrackId) {
    let mut h = History::new(Project::default());
    let Created::Asset(video) = h.apply(Command::AddAsset("movie.mov".into())).unwrap() else {
        panic!("asset")
    };
    let Created::Track(track) = h
        .apply(Command::AddTrack {
            kind: TrackKind::Video,
            name: "Video".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let video = add_clip(&mut h, track, video, sec(0), sec(0), sec(10));
    let Created::Track(captions) = h
        .apply(Command::ImportSubtitles {
            name: "Captions".into(),
            cues: vec![Cue {
                start: sec(1),
                end: sec(2),
                text: "Caption".into(),
            }],
            style: Text::default(),
        })
        .unwrap()
    else {
        panic!("captions")
    };
    let caption = h.project().sequence().track(captions).unwrap().clips()[0].id;
    let Created::Asset(title) = h
        .apply(Command::AddText {
            name: "Ordinary title".into(),
            text: Text {
                content: "Ordinary title".into(),
                ..Text::default()
            },
        })
        .unwrap()
    else {
        panic!("title")
    };
    let Created::Track(titles) = h
        .apply(Command::AddTrack {
            kind: TrackKind::Video,
            name: "Titles".into(),
        })
        .unwrap()
    else {
        panic!("title track")
    };
    let title = add_clip(&mut h, titles, title, sec(0), sec(0), sec(10));
    let before = h.project().clone();
    let Created::Clip(nest) = h
        .apply(Command::Nest {
            ids: vec![video, caption, title],
            name: "Section".into(),
            outer: "Main".into(),
        })
        .unwrap()
    else {
        panic!("nest")
    };
    (h, before, nest, track)
}

#[test]
fn nested_subtitle_instances_map_trims_speed_reverse_and_deeper_offsets() {
    let (mut h, _, nest, track) = nested_captions();
    let Created::Clip(deeper) = h
        .apply(Command::Nest {
            ids: vec![nest],
            name: "Deeper".into(),
            outer: "Main".into(),
        })
        .unwrap()
    else {
        panic!("deeper")
    };
    assert_eq!(h.project().subtitle_cues()[0].start, sec(1));
    let asset = h.project().sequence().clip(deeper).unwrap().1.asset;
    h.apply(Command::MoveClip {
        id: deeper,
        track,
        start: sec(5),
    })
    .unwrap();
    add_clip(&mut h, track, asset, sec(20), sec(0), sec(10));
    let trimmed = add_clip(
        &mut h,
        track,
        asset,
        sec(40),
        Time(3 * S / 2),
        Time(5 * S / 2),
    );
    let ranges = |h: &History| {
        h.project()
            .subtitle_cues()
            .into_iter()
            .map(|c| (c.start, c.end))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ranges(&h),
        [
            (sec(6), sec(7)),
            (sec(21), sec(22)),
            (sec(40), Time(81 * S / 2))
        ]
    );
    h.apply(Command::SetClipSpeed {
        id: trimmed,
        speed: 2.0,
        reverse: false,
        ripple: false,
    })
    .unwrap();
    assert_eq!(ranges(&h)[2], (sec(40), Time(161 * S / 4)));
    h.apply(Command::SetClipSpeed {
        id: trimmed,
        speed: 2.0,
        reverse: true,
        ripple: false,
    })
    .unwrap();
    assert_eq!(ranges(&h)[2], (Time(161 * S / 4), Time(81 * S / 2)));
    assert_eq!(
        srt::write(&h.project().subtitle_cues(), (sec(40), sec(41))),
        "1\n00:00:00,250 --> 00:00:00,500\nCaption\n\n"
    );
}

#[test]
fn nested_sidecar_copy_hides_all_subtitles_and_keeps_ordinary_titles() {
    let (h, _, _, _) = nested_captions();
    let original = h.project().clone();
    let copy = original.without_subtitles();
    assert!(copy.subtitle_cues().is_empty());
    for seq in copy.sequences() {
        let old = original.find_sequence(seq.id()).unwrap();
        for track in seq.tracks() {
            assert_eq!(
                track.hidden,
                track.subtitles || old.track(track.id).unwrap().hidden
            );
        }
    }
    let mut texts = Vec::new();
    visible_text(
        &copy,
        &crate::eval::frame_at(&copy, Time(3 * S / 2)),
        &mut texts,
    );
    assert_eq!(texts, ["Ordinary title"]);
    assert_eq!(
        h.project(),
        &original,
        "render preparation never edits the project"
    );
}

#[test]
fn nested_caption_visibility_freezes_and_depth_are_bounded() {
    let (h, _, nest, _) = nested_captions();
    let mut frozen = History::new(h.project().clone());
    frozen
        .apply(Command::FreezeFrame {
            id: nest,
            at: Time(3 * S / 2),
        })
        .unwrap();
    assert_eq!(
        frozen.project().subtitle_cues(),
        [
            Cue {
                start: sec(1),
                end: Time(3 * S / 2),
                text: "Caption".into()
            },
            Cue {
                start: Time(3 * S / 2),
                end: sec(10),
                text: "Caption".into()
            },
        ]
    );
    let mut outside = History::new(h.project().clone());
    outside
        .apply(Command::FreezeFrame {
            id: nest,
            at: sec(3),
        })
        .unwrap();
    assert_eq!(
        outside.project().subtitle_cues(),
        h.project().subtitle_cues()
    );
    let mut p = h.project().clone();
    p.sequence
        .with_clip_mut(nest, |c| *c.enabled = false)
        .unwrap();
    assert!(p.subtitle_cues().is_empty());
    p = h.project().clone();
    let track = p.sequence.clip(nest).unwrap().0;
    p.sequence.track_mut(track).unwrap().hidden = true;
    assert!(p.subtitle_cues().is_empty());
    p = h.project().clone();
    for seq in &mut p.sequences {
        let captions: Vec<_> = seq
            .tracks()
            .iter()
            .filter(|t| t.subtitles)
            .map(|t| t.id)
            .collect();
        for track in captions {
            seq.track_mut(track).unwrap().hidden = true;
        }
    }
    assert!(p.subtitle_cues().is_empty());
    p = h.project().clone();
    let asset = p.sequence.clip(nest).unwrap().1.asset;
    let root = p.sequence.id();
    p.asset_mut(asset).unwrap().sequence = Some(root);
    assert!(
        p.subtitle_cues().is_empty(),
        "damaged cycles stop at the nesting limit"
    );
}

#[test]
fn nested_subtitle_cues_preserve_timing_and_nest_undo() {
    let (mut h, before, nest, track) = nested_captions();
    let expected = before.subtitle_cues();
    assert_eq!(expected.len(), 1);
    assert_eq!(h.project().subtitle_cues(), expected);
    let nested = h.project().clone();
    assert!(h.undo());
    assert_eq!(h.project(), &before);
    assert_eq!(h.project().subtitle_cues(), expected);
    assert!(h.redo());
    assert_eq!(h.project(), &nested);
    h.apply(Command::MoveClip {
        id: nest,
        track,
        start: sec(5),
    })
    .unwrap();
    assert_eq!(
        h.project().subtitle_cues(),
        vec![Cue {
            start: sec(6),
            end: sec(7),
            text: "Caption".into(),
        }]
    );
    assert_eq!(
        srt::write(&h.project().subtitle_cues(), (sec(6), sec(8))),
        "1\n00:00:00,000 --> 00:00:01,000\nCaption\n\n"
    );
}

#[test]
fn sidecar_render_preserves_non_subtitle_content_when_nest_lands_on_subtitle_track() {
    let (_, before, _, _) = nested_captions();
    let mut h = History::new(before);
    let caption_track = h
        .project()
        .sequence()
        .tracks()
        .iter()
        .find(|t| t.subtitles)
        .unwrap()
        .id;
    h.apply(Command::MoveTrack {
        id: caption_track,
        index: 0,
    })
    .unwrap();
    let ids = h
        .project()
        .sequence()
        .tracks()
        .iter()
        .flat_map(|t| t.clips().iter().map(|c| c.id))
        .collect();
    let Created::Clip(nest) = h
        .apply(Command::Nest {
            ids,
            name: "Section".into(),
            outer: "Root".into(),
        })
        .unwrap()
    else {
        panic!("nest")
    };
    let track = h.project().sequence().clip(nest).unwrap().0;
    assert!(h.project().sequence().track(track).unwrap().subtitles);
    let caption_asset = h
        .project()
        .assets()
        .iter()
        .find(|a| a.text.as_ref().is_some_and(|t| t.content == "Caption"))
        .unwrap()
        .id;
    add_clip(&mut h, track, caption_asset, sec(11), sec(0), sec(1));
    let original = h.project().clone();
    let copy = original.without_subtitles();
    for seq in std::iter::once(&copy.sequence).chain(&copy.sequences) {
        assert!(
            seq.tracks()
                .iter()
                .filter(|t| t.subtitles
                    && t.clips()
                        .iter()
                        .all(|c| copy.asset(c.asset).is_some_and(|a| a.text.is_some())))
                .all(|t| t.hidden)
        );
    }
    let mut texts = Vec::new();
    visible_text(
        &copy,
        &crate::eval::frame_at(&copy, Time(3 * S / 2)),
        &mut texts,
    );
    assert_eq!(texts, ["Ordinary title"]);
    assert_eq!(
        copy.sequence().track(track).unwrap().settings(),
        original.sequence().track(track).unwrap().settings()
    );
    assert_eq!(copy.sequence().clip(nest).unwrap().0, track);
    let mut later = Vec::new();
    visible_text(
        &copy,
        &crate::eval::frame_at(&copy, Time(23 * S / 2)),
        &mut later,
    );
    assert!(
        later.is_empty(),
        "direct captions on the mixed track are suppressed"
    );
    assert_eq!(h.project(), &original);
    assert!(copy.subtitle_cues().is_empty());
}
