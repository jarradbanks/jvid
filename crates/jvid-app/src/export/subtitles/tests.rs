use super::*;
use jvid_core::history::{Command, Created, History};
use jvid_core::model::{NewClip, Text, TrackKind};
use jvid_core::srt::Cue;
use jvid_core::time::{FLICKS_PER_SECOND as S, Time};

fn sec(n: i64) -> Time {
    Time(n * S)
}

fn captions() -> History {
    let mut h = History::new(Project::default());
    let cue = |a, b, text: &str| Cue {
        start: sec(a),
        end: sec(b),
        text: text.into(),
    };
    h.apply(Command::ImportSubtitles {
        name: "Captions".into(),
        cues: vec![
            cue(0, 2, "Before"),
            cue(2, 4, "Original"),
            cue(4, 5, "Inside"),
            cue(5, 8, "Tail\n第二行"),
            cue(8, 9, "After"),
        ],
        style: Text::default(),
    })
    .unwrap();
    let asset = h
        .project()
        .sequence()
        .tracks()
        .iter()
        .find(|t| t.subtitles)
        .unwrap()
        .clips()
        .iter()
        .find(|c| c.start == sec(2))
        .unwrap()
        .asset;
    let mut text = h.project().asset(asset).unwrap().text.clone().unwrap();
    text.content = "¡Edited!\n日本語 🎬".into();
    h.apply(Command::SetText { id: asset, text }).unwrap();
    mark(&mut h, Some(3), Some(6));
    h
}

fn mark(h: &mut History, a: Option<i64>, b: Option<i64>) {
    h.apply(Command::SetInOut {
        in_point: a.map(sec),
        out_point: b.map(sec),
    })
    .unwrap();
}

const RANGED: &str = "1\n00:00:00,000 --> 00:00:01,000\n¡Edited!\n日本語 🎬\n\n2\n00:00:01,000 --> 00:00:02,000\nInside\n\n3\n00:00:02,000 --> 00:00:03,000\nTail\n第二行\n\n";

#[test]
fn edited_unicode_multiline_and_half_open_boundaries_match_sidecar() {
    let h = captions();
    let p = h.project();
    assert_eq!(contents(p, true).unwrap(), RANGED);
    for (in_out, range) in [(false, (sec(0), sec(9))), (true, (sec(3), sec(6)))] {
        let text = contents(p, in_out).unwrap();
        assert_eq!(text, srt::write(&p.subtitle_cues(), range));
        assert_eq!(srt::parse(&text).len(), if in_out { 3 } else { 5 });
    }
    // Either mark alone uses the same sequence-start/end defaults as video export.
    let mut h = h;
    mark(&mut h, Some(2), Some(8));
    let at_boundaries = contents(h.project(), true).unwrap();
    assert!(!at_boundaries.contains("Before") && !at_boundaries.contains("After"));
    mark(&mut h, Some(3), None);
    assert_eq!(
        contents(h.project(), true).unwrap(),
        srt::write(&h.project().subtitle_cues(), (sec(3), sec(9)))
    );
    mark(&mut h, None, Some(6));
    assert_eq!(
        contents(h.project(), true).unwrap(),
        srt::write(&h.project().subtitle_cues(), (sec(0), sec(6)))
    );
    mark(&mut h, None, None);
    assert!(
        contents(h.project(), true).is_err(),
        "In/Out must be explicit"
    );
}

#[test]
fn nested_instances_clip_rebase_and_exclude_unplaced_sequences_and_titles() {
    let mut h = captions();
    let track = h
        .project()
        .sequence()
        .tracks()
        .iter()
        .find(|t| t.subtitles)
        .unwrap()
        .id;
    let ids = h
        .project()
        .sequence()
        .tracks()
        .iter()
        .find(|t| t.subtitles)
        .unwrap()
        .clips()
        .iter()
        .map(|c| c.id)
        .collect();
    let Created::Clip(nest) = h
        .apply(Command::Nest {
            ids,
            name: "Captions nest".into(),
            outer: "Main".into(),
        })
        .unwrap()
    else {
        panic!("nest")
    };
    let asset = h.project().sequence().clip(nest).unwrap().1.asset;
    let child = h.project().nested(asset).unwrap().id();
    h.apply(Command::MoveClip {
        id: nest,
        track,
        start: sec(10),
    })
    .unwrap();
    h.apply(Command::AddClip {
        track,
        clip: NewClip {
            asset,
            source_in: sec(2),
            source_out: sec(6),
            start: sec(20),
            link: None,
        },
    })
    .unwrap();
    mark(&mut h, Some(13), Some(16));
    assert_eq!(contents(h.project(), true).unwrap(), RANGED);
    let all = srt::parse(&contents(h.project(), false).unwrap());
    assert_eq!(
        all.len(),
        8,
        "five cues in full nest and three in trimmed instance"
    );
    assert_eq!(all[5].start, sec(20));
    assert_eq!(all.last().unwrap().end, sec(24));

    // Ordinary timeline titles are not subtitle tracks.
    let Created::Asset(title) = h
        .apply(Command::AddText {
            name: "Title".into(),
            text: Text {
                content: "NOT A CAPTION".into(),
                ..Default::default()
            },
        })
        .unwrap()
    else {
        panic!("text")
    };
    let Created::Track(titles) = h
        .apply(Command::AddTrack {
            kind: TrackKind::Video,
            name: "Titles".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    h.apply(Command::AddClip {
        track: titles,
        clip: NewClip {
            asset: title,
            start: sec(0),
            source_in: sec(0),
            source_out: sec(30),
            link: None,
        },
    })
    .unwrap();
    assert!(
        !contents(h.project(), false)
            .unwrap()
            .contains("NOT A CAPTION")
    );
    h.apply(Command::RemoveClips {
        ids: h
            .project()
            .sequence()
            .tracks()
            .iter()
            .flat_map(|t| t.clips())
            .filter(|c| c.asset == asset)
            .map(|c| c.id)
            .collect(),
        ripple: false,
    })
    .unwrap();
    assert!(
        contents(h.project(), false).is_err(),
        "unplaced nested captions are excluded"
    );
    h.open_sequence(child).unwrap();
    let current = srt::parse(&contents(h.project(), false).unwrap());
    assert_eq!(
        current.len(),
        5,
        "export follows the currently open sequence"
    );
    assert_eq!(current[0].start, sec(0));
}

#[test]
fn only_visible_enabled_subtitle_tracks_are_included() {
    let mut h = captions();
    let track = h
        .project()
        .sequence()
        .tracks()
        .iter()
        .find(|t| t.subtitles)
        .unwrap();
    let mut settings = track.settings();
    settings.hidden = true;
    h.apply(Command::SetTrack(settings)).unwrap();
    assert!(contents(h.project(), false).is_err());
    h.undo();
    let Created::Track(other) = h
        .apply(Command::AddTrack {
            kind: TrackKind::Video,
            name: "Solo video".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let mut solo = h.project().sequence().track(other).unwrap().settings();
    solo.solo = true;
    h.apply(Command::SetTrack(solo)).unwrap();
    assert!(
        contents(h.project(), false).is_err(),
        "video solo excludes subtitle tracks just as in sidecars"
    );
    h.undo();
    h.undo();
    let ids = h
        .project()
        .sequence()
        .tracks()
        .iter()
        .find(|t| t.subtitles)
        .unwrap()
        .clips()
        .iter()
        .map(|c| c.id)
        .collect();
    h.apply(Command::SetClipsEnabled {
        ids,
        enabled: false,
    })
    .unwrap();
    assert!(contents(h.project(), false).is_err());
}

#[test]
fn native_cancel_empty_and_invalid_ranges_never_write() {
    let mut dialog = SubtitleExport::default();
    dialog.open("Edit");
    let h = captions();
    assert!(
        dialog
            .save_with(h.project(), None, |name| {
                assert_eq!(name, "Edit.srt");
                None
            })
            .is_none()
    );
    assert!(dialog.open && dialog.error.is_none());
    assert!(
        dialog
            .save_with(&Project::default(), None, |_| panic!(
                "empty must not open native dialog"
            ))
            .is_none()
    );
    assert_eq!(dialog.error.as_deref(), Some(tr!("subtitle_export.empty")));
    let mut h = h;
    mark(&mut h, Some(9), Some(10));
    dialog.in_out = true;
    assert!(
        dialog
            .save_with(h.project(), None, |_| panic!(
                "empty range must not open native dialog"
            ))
            .is_none()
    );
}

#[test]
fn replaces_existing_srt_and_reports_errors_without_changing_project() {
    let h = captions();
    let before = h.project().clone();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("captions.srt");
    std::fs::write(&path, "old file").unwrap();
    let mut dialog = SubtitleExport::default();
    dialog.open("Edit");
    dialog.in_out = true;
    assert_eq!(
        dialog.save_with(h.project(), None, |_| Some(path.clone())),
        Some(path.clone())
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), RANGED);
    assert!(!dialog.open);
    assert_eq!(h.project(), &before);
    dialog.open("Edit");
    let bad = dir.path().join("missing/failed.srt");
    assert!(dialog.save_with(h.project(), None, |_| Some(bad)).is_none());
    assert!(dialog.open && dialog.error.is_some());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), RANGED);
    let directory = dir.path().join("directory.srt");
    std::fs::create_dir(&directory).unwrap();
    assert!(write(h.project(), None, &directory, RANGED).is_err());
    assert!(write(h.project(), None, &dir.path().join("project.jvid"), RANGED).is_err());
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        2,
        "staging is cleaned up"
    );
}

#[test]
fn refuses_source_and_project_paths_and_hard_link_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.srt");
    let saved = dir.path().join("edit.jvid");
    std::fs::write(&source, "source media").unwrap();
    std::fs::write(&saved, "project").unwrap();
    let mut h = captions();
    h.apply(Command::AddAsset(source.clone())).unwrap();
    assert!(write(h.project(), Some(&saved), &source, RANGED).is_err());
    let source_alias = dir.path().join("alias.srt");
    std::fs::hard_link(&source, &source_alias).unwrap();
    assert!(write(h.project(), Some(&saved), &source_alias, RANGED).is_err());
    let project_alias = dir.path().join("project-alias.srt");
    std::fs::hard_link(&saved, &project_alias).unwrap();
    assert!(write(h.project(), Some(&saved), &project_alias, RANGED).is_err());
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "source media");
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), "project");
    #[cfg(unix)]
    {
        let symlink = dir.path().join("symlink.srt");
        std::os::unix::fs::symlink(&source, &symlink).unwrap();
        assert!(write(h.project(), Some(&saved), &symlink, RANGED).is_err());
    }
}
