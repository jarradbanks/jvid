//! CPU-context, sidecar publication and phase regressions for the export batch.
use super::*;
use std::time::Duration;

fn context<T: Send + 'static>(work: impl FnOnce(&Ctx) -> T + Send + 'static) -> T {
    let pool = Pool::new(1);
    let (tx, rx) = channel();
    let _handle = pool.submit("sidecar regression", Priority::Interactive, move |ctx| {
        tx.send(work(ctx)).ok();
    });
    rx.recv_timeout(Duration::from_secs(30)).unwrap()
}

fn audio_request(path: PathBuf) -> Request {
    Request {
        path,
        range: (Time(0), Time(FLICKS_PER_SECOND)),
        preset: presets()
            .into_iter()
            .find(|p| p.name == "Audio Only (WAV)")
            .unwrap(),
        loudness: None,
        lossless: None,
        sidecar: true,
    }
}

#[test]
fn sidecar_directory_failure_preserves_previous_media() {
    let files = AttemptFiles::new(&std::env::temp_dir().join("sidecar-safety.wav")).unwrap();
    let req = audio_request(files.output.clone());
    std::fs::write(&req.path, b"completed media sentinel").unwrap();
    std::fs::create_dir(req.path.with_extension("srt")).unwrap();
    let request = req.clone();
    let result = context(move |ctx| run_with_gpu(&Project::default(), &request, None, ctx));
    assert!(matches!(result, Err(Error::Failed(_))));
    assert_eq!(
        std::fs::read(&req.path).unwrap(),
        b"completed media sentinel"
    );
    assert!(req.path.with_extension("srt").is_dir());
    assert_eq!(std::fs::read_dir(&files.dir).unwrap().count(), 2);
}

#[test]
fn sidecar_second_publication_failure_restores_both_outputs() {
    for existing in [false, true] {
        let root = AttemptFiles::new(&std::env::temp_dir().join("sidecar-rollback.wav")).unwrap();
        let req = audio_request(root.output.clone());
        let sidecar = req.path.with_extension("srt");
        if existing {
            std::fs::write(&req.path, b"prior media").unwrap();
            std::fs::write(&sidecar, b"prior captions").unwrap();
        }
        let request = req.clone();
        context(move |ctx| {
            let mut files = AttemptFiles::new(&request.path).unwrap();
            std::fs::write(&files.output, b"new media").unwrap();
            std::fs::write(files.output.with_extension("srt"), b"new captions").unwrap();
            let mut calls = 0;
            let result = publish_sidecar(
                &Project::default(),
                &request,
                &mut files,
                ctx,
                |from, to| {
                    calls += 1;
                    if calls == 2 {
                        assert_eq!(std::fs::read(&request.path).unwrap(), b"new media");
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "injected second publish failure",
                        ));
                    }
                    std::fs::rename(from, to)
                },
            );
            assert!(matches!(result, Err(Error::Failed(ref e)) if e.contains("injected second")));
            assert_eq!(calls, 2);
        });
        if existing {
            assert_eq!(std::fs::read(&req.path).unwrap(), b"prior media");
            assert_eq!(std::fs::read(sidecar).unwrap(), b"prior captions");
        } else {
            assert!(!req.path.exists());
            assert!(!sidecar.exists());
        }
        assert_eq!(
            std::fs::read_dir(&root.dir).unwrap().count(),
            if existing { 2 } else { 0 }
        );
    }
}

#[test]
fn sidecar_source_aliases_are_rejected_and_sources_unchanged() {
    use jvid_core::history::{Command, History};
    for alias in ["same", "hard", "symbolic"] {
        let files = AttemptFiles::new(&std::env::temp_dir().join("sidecar-alias.wav")).unwrap();
        let source = files.dir.join("source.srt");
        std::fs::write(&source, b"imported source bytes").unwrap();
        let mut history = History::new(Project::default());
        history.apply(Command::AddAsset(source.clone())).unwrap();
        let path = if alias == "same" {
            files.dir.join("source.wav")
        } else {
            files.output.clone()
        };
        let req = audio_request(path);
        std::fs::write(&req.path, b"previous media").unwrap();
        let companion = req.path.with_extension("srt");
        match alias {
            "hard" => std::fs::hard_link(&source, &companion).unwrap(),
            #[cfg(unix)]
            "symbolic" => std::os::unix::fs::symlink(&source, &companion).unwrap(),
            #[cfg(not(unix))]
            "symbolic" => continue,
            _ => {}
        }
        let request = req.clone();
        let result = context(move |ctx| run_with_gpu(history.project(), &request, None, ctx));
        assert!(matches!(result, Err(Error::Failed(_))), "{alias}");
        assert_eq!(std::fs::read(source).unwrap(), b"imported source bytes");
        assert_eq!(std::fs::read(req.path).unwrap(), b"previous media");
    }
}

#[test]
fn sidecar_cancellation_before_commit_preserves_the_pair() {
    let root = AttemptFiles::new(&std::env::temp_dir().join("sidecar-cancel.wav")).unwrap();
    let req = audio_request(root.output.clone());
    std::fs::write(&req.path, b"prior media").unwrap();
    std::fs::write(req.path.with_extension("srt"), b"prior captions").unwrap();
    let pool = Pool::new(1);
    let (started, wait) = channel();
    let (release, go) = channel();
    let (tx, rx) = channel();
    let request = req.clone();
    let handle = pool.submit("cancel staged pair", Priority::Interactive, move |ctx| {
        let mut files = AttemptFiles::new(&request.path).unwrap();
        std::fs::write(&files.output, b"new media").unwrap();
        std::fs::write(files.output.with_extension("srt"), b"new captions").unwrap();
        started.send(()).unwrap();
        go.recv_timeout(Duration::from_secs(10)).unwrap();
        tx.send(publish_sidecar(
            &Project::default(),
            &request,
            &mut files,
            ctx,
            |_, _| panic!("cancelled publication"),
        ))
        .unwrap();
    });
    wait.recv_timeout(Duration::from_secs(10)).unwrap();
    handle.cancel();
    release.send(()).unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        Err(Error::Cancelled)
    );
    drop(pool);
    assert_eq!(std::fs::read(&req.path).unwrap(), b"prior media");
    assert_eq!(
        std::fs::read(req.path.with_extension("srt")).unwrap(),
        b"prior captions"
    );
    assert_eq!(std::fs::read_dir(&root.dir).unwrap().count(), 2);
}

#[test]
fn nested_sidecar_exports_timed_captions_without_burning_them_or_hiding_titles() {
    use jvid_core::history::{Command, Created, History};
    use jvid_core::model::{NewClip, ProjectSettings, Text, TrackKind};
    let Some((device, queue)) = crate::playback::tests::device() else {
        return;
    };
    let seconds = |n| Time(n * FLICKS_PER_SECOND);
    let mut h = History::new(Project::default());
    h.apply(Command::SetSettings(ProjectSettings {
        width: 320,
        height: 180,
        ..Default::default()
    }))
    .unwrap();
    let Created::Asset(title) = h
        .apply(Command::AddText {
            name: "Ordinary Title".into(),
            text: Text {
                content: "Title".into(),
                size: 32.0,
                anchor: [0.5, 0.25],
                ..Default::default()
            },
        })
        .unwrap()
    else {
        panic!("title")
    };
    let Created::Track(track) = h
        .apply(Command::AddTrack {
            kind: TrackKind::Video,
            name: "Titles".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    h.apply(Command::AddClip {
        track,
        clip: NewClip {
            asset: title,
            source_in: Time(0),
            source_out: seconds(10),
            start: Time(0),
            link: None,
        },
    })
    .unwrap();
    // Independent reference created before captions exist; it must keep the ordinary title.
    let title_only = h.project().clone();
    h.apply(Command::ImportSubtitles {
        name: "Captions".into(),
        cues: vec![jvid_core::srt::Cue {
            start: seconds(1),
            end: seconds(2),
            text: "Caption".into(),
        }],
        style: Text {
            size: 32.0,
            anchor: [0.5, 0.8],
            ..Default::default()
        },
    })
    .unwrap();
    let before_nesting = h.project().subtitle_cues();
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
            name: "Nested".into(),
            outer: "Root".into(),
        })
        .unwrap()
    else {
        panic!("nest")
    };
    assert_eq!(h.project().subtitle_cues(), before_nesting);
    let track = h.project().sequence().clip(nest).unwrap().0;
    h.apply(Command::MoveClip {
        id: nest,
        track,
        start: seconds(5),
    })
    .unwrap();
    let cues = h.project().subtitle_cues();
    assert_eq!((cues[0].start, cues[0].end), (seconds(6), seconds(7)));
    let original = h.project().clone();
    let root = AttemptFiles::new(&std::env::temp_dir().join("nested-sidecar.mov")).unwrap();
    let req = Request {
        path: root.output.clone(),
        range: (
            Time(FLICKS_PER_SECOND * 64 / 10),
            Time(FLICKS_PER_SECOND * 66 / 10),
        ),
        preset: Preset {
            size: None,
            audio: None,
            ..presets()
                .into_iter()
                .find(|p| p.video == Some(VideoCodec::ProRes))
                .unwrap()
        },
        loudness: None,
        lossless: None,
        sidecar: true,
    };
    let expected_cues = jvid_core::srt::parse("1\n00:00:00,000 --> 00:00:00,200\nCaption\n\n");
    let request = req.clone();
    let (d, q) = (device.clone(), queue.clone());
    let p = original.clone();
    context(move |ctx| run(&p, &request, &d, &q, ctx)).unwrap();
    assert_eq!(
        h.project(),
        &original,
        "sidecar preparation must not mutate the project"
    );
    let srt = std::fs::read_to_string(req.path.with_extension("srt")).unwrap();
    assert_eq!(jvid_core::srt::parse(&srt), expected_cues);
    let mut renderer = FrameRenderer::new(&device);
    let expected = jvid_render::composite::read_back(
        &device,
        &queue,
        renderer
            .render(&title_only, seconds(1), &device, &queue)
            .unwrap(),
    );
    assert!(
        expected
            .chunks_exact(4)
            .take(320 * 100)
            .filter(|p| p[0] > 200)
            .count()
            > 100,
        "ordinary title in reference"
    );
    let mut decoder = jvid_media::decode::VideoDecoder::open(&req.path).unwrap();
    let actual = decoder.frame_at(Time(0)).unwrap().view_rgba((320, 180));
    let error = actual
        .as_raw()
        .iter()
        .zip(&expected)
        .map(|(a, b)| a.abs_diff(*b) as f64)
        .sum::<f64>()
        / expected.len() as f64;
    assert!(
        error < 2.0,
        "sidecar video differs from independent title-only reference: {error}"
    );
    assert!(
        actual
            .pixels()
            .skip(320 * 100)
            .all(|p| p[0] < 25 && p[1] < 25 && p[2] < 25),
        "nested caption burned into video"
    );
    assert!(h.undo() && h.undo());
    assert_eq!(
        h.project().subtitle_cues(),
        before_nesting,
        "undo nesting preserves text and times"
    );
    assert!(h.redo() && h.redo());
    assert_eq!(h.project(), &original);
    drop(decoder);
    drop(renderer);
}

#[test]
fn sidecar_pair_publication_supports_lossless_copy_and_smart() {
    for mode in [Lossless::Copy, Lossless::Smart] {
        let (_files, project, mut req) = super::tests::lossless_fixture("sidecar-copy.mov");
        req.lossless = Some(mode);
        req.sidecar = true;
        std::fs::write(&req.path, b"prior media").unwrap();
        let companion = req.path.with_extension("srt");
        std::fs::write(&companion, b"prior captions").unwrap();
        let request = req.clone();
        let p = project.clone();
        let info = context(move |ctx| run_with_gpu(&p, &request, None, ctx)).unwrap();
        assert_eq!(info.width, Some(64));
        assert_eq!(std::fs::read(&companion).unwrap(), b"");
        let media = std::fs::read(&req.path).unwrap();
        std::fs::remove_file(&companion).unwrap();
        std::fs::create_dir(&companion).unwrap();
        let path = req.path.clone();
        assert!(context(move |ctx| run_with_gpu(&project, &req, None, ctx)).is_err());
        assert_eq!(std::fs::read(path).unwrap(), media);
    }
}
