//! A display-free installed-artifact check, using the shipped libraries rather than a
//! system ffmpeg executable. Not a substitute for the interactive clean-machine checklist.

use std::error::Error;
use std::path::Path;

use jvid_core::preset::{AudioCodec, Preset, VideoCodec};
use jvid_core::time::{Rate, Time};
use jvid_media::{decode::VideoDecoder, encode::Encoder};

pub fn run(output: &Path) -> Result<(), Box<dyn Error>> {
    jvid_paths::check_portable_writable()?;
    std::fs::create_dir_all(output)?;
    let info = jvid_media::ffmpeg::init()?;
    if !jvid_media::ffmpeg::GPL_BUILD
        || !info.encoders.contains(&"libx264")
        || !info.encoders.contains(&"libx265")
        || !info.encoders.contains(&"aac")
    {
        return Err("release package requires gpl-ffmpeg, libx264, libx265 and AAC".into());
    }
    let config = crate::settings_file::path().ok_or("no config directory")?;
    let models = jvid_transcribe::models::dir().ok_or("no model directory")?;
    // Exercise precisely the locations the settings UI and downloader use. Do not touch
    // user settings or manufacture a file under a real model's name.
    let settings_probe = config.with_file_name(format!("smoke-{}.toml", std::process::id()));
    crate::settings_file::save_to(&Default::default(), &settings_probe)?;
    assert_eq!(
        crate::settings_file::load_from(&settings_probe),
        Default::default()
    );
    std::fs::remove_file(settings_probe)?;
    std::fs::create_dir_all(&models)?;
    let model_probe = models.join(format!("smoke-{}.part", std::process::id()));
    std::fs::write(&model_probe, b"write probe, not a Whisper model")?;
    std::fs::remove_file(model_probe)?;

    let source = output.join("source.mp4");
    std::fs::write(&source, include_bytes!("../assets/samples/subject.mp4"))?;
    jvid_media::probe::probe(&source)?;
    let mut decoder = VideoDecoder::open(&source)?;
    let pixels = decoder.frame_at(Time(0))?.view_rgba((128, 128));
    for (name, codec) in [
        ("h264.mp4", VideoCodec::H264),
        ("hevc.mp4", VideoCodec::Hevc),
    ] {
        let path = output.join(name);
        let preset = Preset {
            name: "package-smoke".into(),
            extension: "mp4".into(),
            video: Some(codec),
            audio: Some(AudioCodec::Aac),
            size: None,
            frame_rate: None,
            options: [
                ("encoder".into(), "software".into()),
                ("preset".into(), "ultrafast".into()),
            ]
            .into(),
        };
        let mut encoder = Encoder::create(&path, (128, 128), Rate::new(30, 1), &preset)?;
        for _ in 0..8 {
            encoder.video(pixels.as_raw())?;
            encoder.audio(&vec![0.0; 1600 * 2])?;
        }
        encoder.finish()?;
        jvid_media::probe::probe(&path)?;
        VideoDecoder::open(&path)?.frame_at(Time(0))?;
    }
    let report = format!(
        "version={}\nconfig={}\nmodels={}\nportable={}\nffmpeg={}\nencoders={:?}\nimport_decode_export=passed\n",
        env!("CARGO_PKG_VERSION"),
        config.display(),
        models.display(),
        jvid_paths::portable_root().is_some(),
        info.license,
        info.encoders,
    );
    std::fs::write(output.join("smoke-report.txt"), &report)?;
    println!("{report}");
    Ok(())
}
