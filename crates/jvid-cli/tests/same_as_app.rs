//! Acceptance (#61): the CLI and the app write identical files for the same project and range.

use std::process::Command;

use jvid_app::export::{self, Request};
use jvid_core::history::{Command as Edit, Created, History};
use jvid_core::model::{NewClip, Project, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND, Time};

#[test]
fn cli_matches_app_export() {
    let Some(dir) = jvid_media::test_media::corpus() else {
        return;
    };
    if !suite_ui::test_support::gpu() {
        return;
    }
    let mut h = History::new(Project::default());
    let Ok(Created::Asset(asset)) = h.apply(Edit::AddAsset(dir.join("frame-index-1080p30.mp4")))
    else {
        panic!("asset")
    };
    let Ok(Created::Track(track)) = h.apply(Edit::AddTrack {
        kind: TrackKind::Video,
        name: "v".into(),
    }) else {
        panic!("track")
    };
    let clip = NewClip {
        asset,
        source_in: Time(0),
        source_out: Time(3 * FLICKS_PER_SECOND),
        start: Time(0),
        link: None,
    };
    h.apply(Edit::AddClip { track, clip }).unwrap();
    let tmp = std::env::temp_dir().join(format!("jvid-cli-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let file = tmp.join("p.jvid");
    jvid_app::project_file::save(h.project(), &file).unwrap();
    let project = jvid_app::project_file::load(&file).unwrap();

    let preset = jvid_core::preset::builtin().remove(0);
    let app = tmp.join(format!("app.{}", preset.extension));
    let cli = tmp.join(format!("cli.{}", preset.extension));
    let rate = project.settings().frame_rate;
    let req = Request {
        path: app.clone(),
        range: (rate.time_floor(15), rate.time_floor(45)),
        preset: preset.clone(),
        loudness: None,
        lossless: None,
        sidecar: false,
    };
    let pool = jvid_jobs::Pool::new(1);
    let gpu = export::headless_gpu().unwrap();
    let (_h, rx) = export::submit(&pool, project, req, gpu);
    rx.recv().unwrap().unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_jvid"))
        .arg("render")
        .arg(&file)
        .arg("-o")
        .arg(&cli)
        .args([
            "--preset",
            &preset.name,
            "--range",
            "00:00:00:15-00:00:01:15",
            "--json",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let same = std::fs::read(&app).unwrap() == std::fs::read(&cli).unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(same, "CLI output differs from the app's");
}
