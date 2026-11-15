//! The real app's paired native frames and dynamic domain targets, without user data or devices.

use std::time::{Duration, Instant};

use serde_json::Value;

use super::*;

fn snapshot(harness: &mut Harness<'_, App>, ready: impl Fn(&Value) -> bool) -> Value {
    let path = std::env::var_os("JUI_DEBUG_CONNECTION").unwrap();
    let connection: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let url = format!("{}/snapshot", connection["url"].as_str().unwrap());
    let authorization = format!("Bearer {}", connection["token"].as_str().unwrap());
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last = String::new();
    while Instant::now() < deadline {
        harness.run_steps(2);
        std::thread::sleep(Duration::from_millis(20));
        let mut response = match ureq::get(&url)
            .header("Authorization", &authorization)
            .call()
        {
            Ok(response) => response,
            Err(error) => {
                last = error.to_string();
                continue;
            }
        };
        let body = response.body_mut().read_to_vec().unwrap();
        let length = u32::from_le_bytes(body[..4].try_into().unwrap()) as usize;
        let metadata: Value = serde_json::from_slice(&body[4..4 + length]).unwrap();
        let paths: Vec<_> = metadata["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|target| target["path"].clone())
            .take(80)
            .collect();
        last = format!(
            "frame={} size={}x{} ppp={} paths={paths:?}",
            metadata["frame"], metadata["width"], metadata["height"], metadata["ppp"]
        );
        assert!(body[4 + length..].starts_with(b"\x89PNG\r\n\x1a\n"));
        if ready(&metadata) {
            return metadata;
        }
    }
    panic!("native snapshot did not reach the expected editor state: {last}");
}

fn target<'a>(metadata: &'a Value, path: &str) -> Option<&'a Value> {
    metadata["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|target| target["path"] == path)
}

fn click(metadata: &Value, target: &Value) {
    let path = std::env::var_os("JUI_DEBUG_CONNECTION").unwrap();
    let connection: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let url = format!("{}/input", connection["url"].as_str().unwrap());
    let authorization = format!("Bearer {}", connection["token"].as_str().unwrap());
    let rect = target["rect"].as_array().unwrap();
    let x = rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap() / 2.0;
    let y = rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap() / 2.0;
    let payload = serde_json::json!({
        "session": metadata["session"], "generation": metadata["generation"],
        "frame": metadata["frame"], "events": [
            {"kind":"button", "x":x, "y":y, "button":0, "pressed":true},
            {"kind":"button", "x":x, "y":y, "button":0, "pressed":false}
        ]
    });
    ureq::post(url)
        .header("Authorization", &authorization)
        .header("Content-Type", "application/json")
        .send(payload.to_string())
        .unwrap();
}

#[test]
fn native_targets_follow_clip_identity_across_resize_delete_and_undo() {
    const CHILD: &str = "JVID_DEBUG_TARGET_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let dir = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "app::tests::debug_ui::native_targets_follow_clip_identity_across_resize_delete_and_undo",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .env("JUI_DEBUG", "1")
            .env("JUI_DEBUG_PROFILE", dir.path().join("profile"))
            .env("JUI_DEBUG_CONNECTION", dir.path().join("connection.json"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    if !gpu() {
        return;
    }
    jvid_paths::check_developer_profile().unwrap();
    let profile = PathBuf::from(std::env::var_os("JUI_DEBUG_PROFILE").unwrap());
    assert_eq!(
        crate::settings_file::path(),
        Some(profile.join("config/settings.toml"))
    );
    assert_eq!(
        crate::settings_file::themes_dir(),
        Some(profile.join("config/themes"))
    );
    assert_eq!(
        crate::autosave::untitled(),
        Some(profile.join("data/autosave/Untitled.jvid"))
    );
    let mut harness = builder([1280.0, 720.0], 1.0).wgpu().build_eframe(|cc| {
        j_ui_debug::install_from_env(&cc.egui_ctx, "jvid-test").unwrap();
        App::new(cc, &Default::default())
    });
    let app = harness.state_mut();
    app.on_welcome = false;
    let clip = crate::playback::tests::add_solid(&mut app.history, [0.2, 0.3, 0.4, 1.0], Time(0));
    let sequence = app.history.project().sequence();
    let track = sequence.clip(clip).unwrap().0;
    let path = format!(
        "jvid/sequences/{}/tracks/{}/clips/{}",
        sequence.id().0,
        track.0,
        clip.0
    );
    let project = app.history.project().clone();
    let first = snapshot(&mut harness, |metadata| target(metadata, &path).is_some());
    let initial = target(&first, &path).unwrap();
    assert_eq!(initial["label"], "Solid");
    assert!(initial["source"].as_str().unwrap().contains("timeline.rs:"));
    assert!(
        first["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|target| { target["label"] == "Razor Tool" && target["role"] == "Button" })
    );
    assert!(
        first["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|target| target["label"] == "Timeline" && target["role"] == "Tab")
    );
    click(&first, initial);
    harness.run_steps(3);
    assert!(harness.state().timeline.selected().contains(&clip));

    for (size, scale) in configs() {
        harness.set_pixels_per_point(scale);
        // Apply egui's zoom change before setting the new logical window size.
        harness.run_steps(2);
        harness.set_size(vec2(size[0], size[1]));
        let metadata = snapshot(&mut harness, |metadata| {
            metadata["width"].as_u64() == Some((size[0] * scale) as u64)
                && metadata["height"].as_u64() == Some((size[1] * scale) as u64)
                && metadata["ppp"].as_f64() == Some(f64::from(scale))
                && target(metadata, &path).is_some()
        });
        let clip_target = target(&metadata, &path).unwrap();
        assert_eq!(clip_target["id"], initial["id"]);
        let rect = clip_target["rect"].as_array().unwrap();
        let values: Vec<_> = rect.iter().map(|value| value.as_f64().unwrap()).collect();
        assert!(values[0] >= 0.0 && values[1] >= 0.0);
        assert!(values[2] > 0.0 && values[3] > 0.0);
        assert!(values[0] + values[2] <= 1.0001 && values[1] + values[3] <= 1.0001);
        assert_eq!(harness.state().history.project(), &project);
    }
    assert!(harness.state_mut().history.undo());
    snapshot(&mut harness, |metadata| target(metadata, &path).is_none());
    assert!(harness.state_mut().history.redo());
    let restored = snapshot(&mut harness, |metadata| target(metadata, &path).is_some());
    assert_eq!(target(&restored, &path).unwrap()["id"], initial["id"]);
    assert_eq!(harness.state().history.project(), &project);
}
