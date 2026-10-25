//! Codec-family regressions, isolated from composition's shared export test module.

use super::*;
use jvid_core::time::Rate;

fn with_context<T: Send + 'static>(work: impl FnOnce(&Ctx) -> T + Send + 'static) -> T {
    let pool = Pool::new(1);
    let (tx, rx) = channel();
    let _handle = pool.submit("codec family test", Priority::Interactive, move |ctx| {
        tx.send(work(ctx)).ok();
    });
    rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap()
}

fn request(path: PathBuf) -> Request {
    let mut preset = presets()
        .into_iter()
        .find(|p| p.video == Some(VideoCodec::H264))
        .unwrap();
    preset.size = None;
    preset.audio = None;
    Request {
        path,
        range: (Time(0), Rate::new(30, 1).time_floor(3)),
        preset,
        loudness: None,
        lossless: None,
        sidecar: false,
    }
}

#[test]
fn absent_h264_and_software_retry_never_publish_or_replace_output() {
    jvid_media::ffmpeg::init().unwrap();
    for retry in [false, true] {
        let files = AttemptFiles::new(&std::env::temp_dir().join("jvid-codec-family.mp4")).unwrap();
        let req = request(files.output.clone());
        let sentinel = b"previous completed export";
        std::fs::write(&req.path, sentinel).unwrap();
        let (result, attempts) = with_context({
            let req = req.clone();
            move |ctx| {
                let mut attempts = 0;
                let result: Result<(), Error> = replay(&Project::default(), &req, ctx, |attempt| {
                    attempts += 1;
                    assert_eq!(std::fs::read(&req.path).unwrap(), sentinel);
                    if retry && attempts == 1 {
                        std::fs::write(&attempt.path, b"partial hardware attempt").unwrap();
                        return Err(EncodeError::Vaapi((-5).into()).into());
                    }
                    if retry {
                        assert_eq!(
                            attempt.preset.options.get("encoder").map(String::as_str),
                            Some("software")
                        );
                    }
                    let error = match Encoder::create_with_availability(
                        &attempt.path,
                        (64, 64),
                        Rate::new(30, 1),
                        &attempt.preset,
                        &[],
                        |name| name == "mpeg4",
                    ) {
                        Ok(_) => panic!("missing H.264 must not select MPEG-4 Part 2"),
                        Err(error) => error,
                    };
                    assert!(matches!(
                        error,
                        CreateError::NoVideoEncoder(VideoCodec::H264)
                    ));
                    assert!(!attempt.path.exists());
                    Err(create_error(&attempt.path, error).into())
                });
                (result, attempts)
            }
        });
        assert_eq!(attempts, if retry { 2 } else { 1 });
        let Err(Error::Failed(message)) = result else {
            panic!("must fail before publication")
        };
        assert!(message.contains("H.264"), "{message}");
        assert!(
            message.contains("FFmpeg") || message.contains("libopenh264"),
            "{message}"
        );
        assert!(message.contains("preset"), "{message}");
        assert_eq!(std::fs::read(&req.path).unwrap(), sentinel);
        assert_eq!(
            std::fs::read_dir(&files.dir).unwrap().count(),
            1,
            "only original output survives"
        );
    }
}

#[test]
fn encoded_family_is_checked_before_publication() {
    jvid_media::ffmpeg::init().unwrap();
    let files = AttemptFiles::new(&std::env::temp_dir().join("jvid-wrong-family.mov")).unwrap();
    let req = request(files.output.clone());
    let sentinel = b"previous completed export";
    std::fs::write(&req.path, sentinel).unwrap();
    let result = with_context({
        let req = req.clone();
        move |ctx| {
            replay(&Project::default(), &req, ctx, |attempt| {
                let mut actual = attempt.preset.clone();
                actual.video = Some(VideoCodec::ProRes);
                let mut enc = Encoder::create(&attempt.path, (64, 64), Rate::new(30, 1), &actual)
                    .map_err(|error| create_error(&attempt.path, error))?;
                for _ in 0..3 {
                    enc.video(&[128; 64 * 64 * 4])?;
                }
                enc.finish()?;
                validate(&Project::default(), attempt).map_err(Into::into)
            })
        }
    });
    let Err(Error::Failed(message)) = result else {
        panic!("wrong family must be rejected")
    };
    assert!(
        message.contains("H.264") && message.contains("prores"),
        "{message}"
    );
    assert_eq!(std::fs::read(&req.path).unwrap(), sentinel);
    assert_eq!(std::fs::read_dir(&files.dir).unwrap().count(), 1);
}

#[test]
fn mpeg4_part2_is_not_h264_and_each_requested_family_matches_itself() {
    let error = validate_video_family(VideoCodec::H264, "mpeg4").unwrap_err();
    let Error::Failed(message) = error else {
        panic!("wrong codec")
    };
    assert!(
        message.contains("H.264") && message.contains("mpeg4"),
        "{message}"
    );
    for codec in [
        VideoCodec::H264,
        VideoCodec::Hevc,
        VideoCodec::ProRes,
        VideoCodec::Gif,
    ] {
        validate_video_family(codec, jvid_media::encode::video_codec_id(codec).name()).unwrap();
    }
}

/// Generic composition tests request ProRes explicitly, retaining AAC timing and 1080p reads.
pub(super) fn rendering_preset() -> Preset {
    let mut preset = presets()
        .into_iter()
        .find(|p| p.video == Some(VideoCodec::ProRes))
        .unwrap();
    preset.size = Some([1920, 1080]);
    preset.audio = Some(jvid_core::preset::AudioCodec::Aac);
    preset
}

/// Codec-specific content is checked when available; otherwise verify the real, actionable
/// failure and exact destination preservation. Absence is reported as a separate tested path,
/// never as evidence that encoding, target size or sharpness was exercised.
pub(super) fn check_codec_result(
    req: &Request,
    run: impl FnOnce() -> Result<ProbeInfo, Error>,
    check: impl FnOnce(ProbeInfo),
) {
    jvid_media::ffmpeg::init().unwrap();
    let unavailable = req
        .preset
        .video
        .filter(|codec| jvid_media::encode::video_encoders(*codec, &req.preset.options).is_empty());
    if let Some(codec) = unavailable {
        let sentinel = b"previous completed codec-specific test export";
        std::fs::write(&req.path, sentinel).unwrap();
        let Err(Error::Failed(message)) = run() else {
            panic!("unavailable requested family must fail before publication");
        };
        assert!(message.contains(dialog::codec_name(codec)), "{message}");
        assert!(
            message.contains(jvid_media::encode::software_encoder_hint(codec)),
            "{message}"
        );
        assert!(message.contains("preset"), "{message}");
        assert_eq!(std::fs::read(&req.path).unwrap(), sentinel);
        assert!(!PathBuf::from(format!("{}.best", req.path.display())).exists());
        eprintln!(
            "verified unavailable {} encoder and preserved output for {}; content/quality/size assertions require a real encoder (gpl-ffmpeg or OpenH264)",
            dialog::codec_name(codec),
            req.path.display()
        );
        std::fs::remove_file(&req.path).unwrap();
    } else {
        let info = run().unwrap_or_else(|error| panic!("{}: {error:?}", req.preset.name));
        if let Some(codec) = req.preset.video {
            for stream in &info.streams {
                if let jvid_core::model::StreamInfo::Video(video) = stream {
                    assert_eq!(
                        video.codec,
                        jvid_media::encode::video_codec_id(codec).name()
                    );
                }
            }
        }
        check(info);
    }
}
