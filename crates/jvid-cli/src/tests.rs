use super::*;

fn args(s: &str) -> Result<Args, String> {
    parse(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
}

#[test]
fn parses_render_options() {
    let a = args("render p.jvid -o out.mp4 --preset Web --range 00:00:01:00-00:00:02:00 --json")
        .unwrap();
    assert_eq!(a.project, PathBuf::from("p.jvid"));
    assert_eq!(a.output, PathBuf::from("out.mp4"));
    assert_eq!(a.preset.as_deref(), Some("Web"));
    assert_eq!(
        a.range,
        (Some("00:00:01:00".into()), Some("00:00:02:00".into()))
    );
    assert!(a.json);
    assert_eq!(a.lossless, None);
    let a = args("render p.jvid -o out.mp4 --smart").unwrap();
    assert_eq!(a.lossless, Some(Lossless::Smart));
}

#[test]
fn rejects_bad_arguments() {
    for bad in [
        "",
        "export p.jvid -o x",
        "render p.jvid",
        "render -o x",
        "render p.jvid -o",
        "render p.jvid -o x --bogus",
        "render p.jvid q.jvid -o x",
        "render p.jvid -o x --range 1",
        "render p.jvid -o x --loudness loud",
        "render p.jvid -o x --target-size 0",
        "render p.jvid -o x --target-size 2.5",
    ] {
        assert!(args(bad).is_err(), "{bad}");
    }
}

#[test]
fn request_uses_timecodes_or_the_app_defaults() {
    let project = Project::default();
    let presets = jvid_core::preset::builtin();
    let rate = project.settings().frame_rate;
    let a = args("render p.jvid -o o.mp4 --in 00:00:01:00 --out 00:00:02:00").unwrap();
    let r = request(&project, &a, presets.clone()).unwrap();
    assert_eq!(
        r.range,
        (
            rate.time_floor(rate.timecode_base() as i64),
            rate.time_floor(2 * rate.timecode_base() as i64)
        )
    );
    assert_eq!(r.preset, presets[0]);
    let named = args(&format!(
        "render p.jvid -o o.mp4 --in 00:00:00:00 --out 00:00:01:00 --preset {}",
        "x"
    ))
    .unwrap();
    assert!(request(&project, &named, presets.clone()).is_err());
    let backwards = args("render p.jvid -o o.mp4 --in 00:00:02:00 --out 00:00:01:00").unwrap();
    assert!(request(&project, &backwards, presets.clone()).is_err());
    let garbage = args("render p.jvid -o o.mp4 --in 1:2").unwrap();
    assert!(request(&project, &garbage, presets).is_err());
}

#[test]
fn target_size_goes_into_the_preset_in_bytes() {
    let project = Project::default();
    let presets = jvid_core::preset::builtin();
    let a =
        args("render p.jvid -o o.mp4 --range 00:00:00:00-00:00:01:00 --target-size 25").unwrap();
    let r = request(&project, &a, presets.clone()).unwrap();
    assert_eq!(
        r.preset.options.get("target_size").map(String::as_str),
        Some("25000000")
    );
}

#[test]
fn cpu_exports_never_acquire_a_gpu_and_video_reports_code_four() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let root = std::env::temp_dir().join(format!("jvid-cli-cpu-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let pool = jvid_jobs::Pool::new(1);
    jvid_media::ffmpeg::init().unwrap();
    let source = root.join("source.mov");
    let mut preset = jvid_core::preset::builtin()
        .into_iter()
        .find(|p| p.video == Some(VideoCodec::ProRes))
        .unwrap();
    preset.audio = None;
    preset.size = None;
    let rate = jvid_core::time::Rate::new(30, 1);
    let mut encoder =
        jvid_media::encode::Encoder::create(&source, (64, 64), rate, &preset).unwrap();
    for _ in 0..60 {
        encoder.video(&vec![128; 64 * 64 * 4]).unwrap();
    }
    encoder.finish().unwrap();
    let mut history = jvid_core::history::History::new(Project::default());
    use jvid_core::history::{Command, Created};
    let Created::Asset(asset) = history.apply(Command::AddAsset(source.clone())).unwrap() else {
        panic!("asset")
    };
    history
        .apply(Command::SetAssetProbe {
            id: asset,
            probe: Some(jvid_media::probe::probe(&source).unwrap()),
        })
        .unwrap();
    let Created::Track(track) = history
        .apply(Command::AddTrack {
            kind: jvid_core::model::TrackKind::Video,
            name: "Video".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    history
        .apply(Command::AddClip {
            track,
            clip: jvid_core::model::NewClip {
                asset,
                source_in: Time(0),
                source_out: rate.time_floor(60),
                start: Time(0),
                link: None,
            },
        })
        .unwrap();
    for lossless in [None, Some(Lossless::Copy), Some(Lossless::Smart)] {
        let preset = if lossless.is_some() {
            preset.clone()
        } else {
            jvid_core::preset::builtin()
                .into_iter()
                .find(|p| p.name == "Audio Only (WAV)")
                .unwrap()
        };
        let path = root.join(format!("{lossless:?}.{}", preset.extension));
        let req = Request {
            path: path.clone(),
            range: (Time(0), rate.time_floor(30)),
            preset,
            loudness: None,
            lossless,
            sidecar: false,
        };
        let called = AtomicBool::new(false);
        let job = start_export(&pool, history.project().clone(), req, || {
            called.store(true, Ordering::SeqCst);
            Err("injected missing adapter".into())
        });
        assert!(
            !called.load(Ordering::SeqCst),
            "CPU export probed wgpu: {lossless:?}"
        );
        let (_, rx) = job.unwrap();
        let result = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(result.is_ok(), "{result:?}");
        assert!(path.is_file());
    }
    let path = root.join("video.mp4");
    std::fs::write(&path, b"existing output").unwrap();
    let req = Request {
        path: path.clone(),
        range: (Time(0), Time(23_520_000)),
        preset: jvid_core::preset::builtin().remove(0),
        loudness: None,
        lossless: None,
        sidecar: false,
    };
    let result = start_export(&pool, Project::default(), req, || {
        Err("no GPU adapter: injected".into())
    });
    assert!(matches!(result, Err((4, ref e)) if e.contains("no GPU adapter")));
    assert_eq!(std::fs::read(path).unwrap(), b"existing output");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn json_progress_names_phases_passes_retries_and_only_phase_eta() {
    let mut status = export::Snapshot {
        phase: export::Phase::Encoding { pass: 2 },
        progress: Some(0.5),
        retry: 1,
        phase_seconds: 10.0,
    };
    assert_eq!(
        progress_line(&status, true),
        "{\"progress\":0.5000,\"phase\":\"encoding\",\"pass\":2,\"retry\":1,\"phase_eta_seconds\":10.0000}"
    );
    status.phase = export::Phase::Finalizing;
    status.progress = None;
    assert_eq!(
        progress_line(&status, true),
        "{\"progress\":null,\"phase\":\"finalizing\",\"pass\":null,\"retry\":1,\"phase_eta_seconds\":null}"
    );
    assert!(!progress_line(&status, false).contains("100"));
}
