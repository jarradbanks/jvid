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
