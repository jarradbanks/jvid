use std::path::PathBuf;

use jvid_core::history::{Command, History};
use jvid_core::model::{Project, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND, Time};

#[test]
fn jplay_markers_import_and_place_with_separate_audio_lanes() {
    jvid_media::ffmpeg::init().unwrap();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let path = dir.join("jplay.mp4");
    let probe = jvid_media::probe::probe(&path).unwrap();
    assert_eq!(probe.audio_streams, 2);
    let ms = |n| Time(n * (FLICKS_PER_SECOND / 1000));
    let expected = [ms(0), ms(1250), ms(3750)];
    assert_eq!(
        probe.markers.iter().map(|m| m.time).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        probe
            .markers
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        ["Start", "First kill", "Round won"]
    );

    let mut history = History::new(Project::default());
    history.apply(Command::Import(vec![(path, probe)])).unwrap();
    // Source markers survive a project round trip before the clip is placed.
    let json = jvid_core::file::to_json(history.project(), &dir).unwrap();
    let project = jvid_core::file::from_json(&json, &dir, |p| p.exists()).unwrap();
    history = History::new(project);
    let asset = history.project().assets()[0].id;
    for (start, source_in, source_out, times) in [
        (ms(0), ms(0), ms(4000), expected.to_vec()),
        (
            ms(10000),
            ms(0),
            ms(4000),
            vec![ms(10000), ms(11250), ms(13750)],
        ),
        (ms(10000), ms(1000), ms(3750), vec![ms(10250)]),
        (ms(10000), ms(1250), ms(3750), vec![ms(10000)]),
    ] {
        let mut placement = history
            .project()
            .placement(asset, start, false, None, false)
            .unwrap();
        placement.source_in = source_in;
        placement.source_out = source_out;
        history.apply(Command::Place(placement)).unwrap();
        let sequence = history.project().sequence();
        assert_eq!(
            sequence
                .markers()
                .iter()
                .map(|m| m.time)
                .collect::<Vec<_>>(),
            times,
            "markers appear once per placement, not once per audio/video stream"
        );
        let audio: Vec<_> = sequence
            .tracks()
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .collect();
        assert_eq!(audio.len(), 2);
        assert_eq!(audio[0].clips()[0].stream, Some(0));
        assert_eq!(audio[1].clips()[0].stream, Some(1));
        assert!(history.undo());
        assert!(history.project().sequence().markers().is_empty());
        assert!(history.redo());
        assert_eq!(history.project().sequence().markers().len(), times.len());
        assert!(history.undo());
    }
}
