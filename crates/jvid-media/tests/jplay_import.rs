use std::process::Command as Process;

use jvid_core::file::{from_json, to_json};
use jvid_core::history::{Command, History};
use jvid_core::model::{Project, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND, Time};

#[test]
fn jplay_fixture_imports_three_markers_and_separate_audio_lanes() {
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("my.clutch.mkv");
    let sidecar = media.with_extension("jplay.json");
    std::fs::write(&sidecar, include_str!("../testdata/clutch.jplay.json")).unwrap();
    // Tiny real media, with video between the two audio streams. No corpus or GPU needed.
    let output = Process::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "color=s=16x16:r=1:d=30",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=8000:cl=mono",
            "-t",
            "30",
            "-map",
            "1:a",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "ffv1",
            "-c:a",
            "flac",
        ])
        .arg(&media)
        .output()
        .expect("ffmpeg is required for media tests");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    jvid_media::ffmpeg::init().unwrap();
    let probe = jvid_media::probe::probe(&media).unwrap();
    assert_eq!(probe.markers.len(), 3);
    assert_eq!(probe.audio_streams, 2);
    let mut history = History::new(Project::default());
    history
        .apply(Command::Import(vec![(media, probe)]))
        .unwrap();
    let asset = history.project().assets()[0].id;
    // Source metadata survives a project save even if its sidecar is no longer present.
    let json = to_json(history.project(), dir.path()).unwrap();
    std::fs::remove_file(sidecar).unwrap();
    let project = from_json(&json, dir.path(), |p| p.exists()).unwrap();
    let mut history = History::new(project);
    let placement = history
        .project()
        .placement(asset, Time(0), false, None, false)
        .unwrap();
    history.apply(Command::Place(placement)).unwrap();
    let sequence = history.project().sequence();
    let markers: Vec<_> = sequence
        .markers()
        .iter()
        .map(|m| (m.time.0, m.name.as_str()))
        .collect();
    assert_eq!(
        markers,
        [
            (88_200_000, "Start"),
            (8_749_440_000, "3K"),
            (19_756_094_400, "Round Win"),
        ]
    );
    let audio: Vec<_> = sequence
        .tracks()
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .collect();
    assert_eq!(audio.len(), 2);
    for (n, track) in audio.iter().enumerate() {
        assert_eq!(track.name, ["Game", "Mic"][n]);
        assert_eq!(track.clips().len(), 1);
        assert_eq!(track.clips()[0].stream, Some(n as u32));
        assert_eq!(track.clips()[0].start, Time(0));
    }
    let placed = history.project().clone();
    let json = to_json(&placed, dir.path()).unwrap();
    assert_eq!(
        from_json(&json, dir.path(), |p| p.exists()).unwrap(),
        placed
    );
    assert!(history.undo());
    assert!(history.project().sequence().markers().is_empty());
    assert!(history.project().sequence().tracks().is_empty());
    assert!(history.redo());
    assert_eq!(history.project(), &placed);

    history
        .apply(Command::SetAssetInOut {
            id: asset,
            in_point: Some(Time(FLICKS_PER_SECOND)),
            out_point: Some(Time(19_756_094_400)),
        })
        .unwrap();
    let placement = history
        .project()
        .placement(asset, Time(40 * FLICKS_PER_SECOND), false, None, false)
        .unwrap();
    history.apply(Command::Place(placement)).unwrap();
    let markers = history.project().sequence().markers();
    assert_eq!(
        markers.len(),
        4,
        "out-point is exclusive; no duplicates per audio lane"
    );
    assert_eq!(markers[3].time, Time(36_267_840_000)); // 40 + 12.4 - 1 = 51.4 s.
    assert_eq!(markers[3].name, "3K");
}
