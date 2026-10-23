//! The checked-in `.jvid` fixtures load, and re-saving them loses nothing.

use std::path::Path;

use jvid_core::file::{from_json, to_json};
use jvid_core::history::{Command, Created, History};

fn load(name: &str, dir: &Path) -> jvid_core::model::Project {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    from_json(&std::fs::read_to_string(path).unwrap(), dir, |p| {
        p.ends_with("media/rec.mkv")
    })
    .unwrap()
}

#[test]
fn fixtures_load_and_round_trip() {
    let dir = Path::new("/new/proj");
    for name in ["empty.jvid", "basic.jvid"] {
        let p = load(name, dir);
        let json = to_json(&p, dir).unwrap();
        let again = from_json(&json, dir, |_| true).unwrap();
        assert_eq!(again, p, "{name}");
        // One sequence saves as before nesting (#83), so older builds still open it.
        assert!(
            !json.contains("sequences") && !json.contains("next_sequence"),
            "{name}"
        );
    }
    let p = load("basic.jvid", dir);
    assert_eq!(p.assets()[0].path, Path::new("/new/proj/media/rec.mkv"));
    // Saved before text layers (#63): no asset has a `text` field.
    assert!(
        p.assets()
            .iter()
            .all(|a| a.text.is_none() && !a.generated())
    );
    // Saved before HDR output (#86).
    assert!(!p.settings().hdr_output);
    let seq = p.sequence();
    assert_eq!(seq.tracks().len(), 2);
    assert_eq!(seq.markers()[0].name, "kill");
    assert_eq!(seq.tracks()[0].clips()[0].params["opacity"], 0.5);
    // ids continue after the saved counters
    let mut h = History::new(p);
    assert_eq!(
        h.apply(Command::NewLink),
        Ok(Created::Link(jvid_core::model::LinkId(7)))
    );
    assert_eq!(
        h.apply(Command::AddAsset("x.mkv".into())),
        Ok(Created::Asset(jvid_core::model::AssetId(2)))
    );
}

#[test]
fn in_out_is_saved() {
    let dir = Path::new("/new/proj");
    let mut h = History::new(load("basic.jvid", dir));
    let marks = (
        Some(jvid_core::time::Time(10)),
        Some(jvid_core::time::Time(90)),
    );
    h.apply(Command::SetInOut {
        in_point: marks.0,
        out_point: marks.1,
    })
    .unwrap();
    let again = from_json(&to_json(h.project(), dir).unwrap(), dir, |_| true).unwrap();
    let seq = again.sequence();
    assert_eq!((seq.in_point(), seq.out_point()), marks);
}

#[test]
fn markers_are_saved() {
    let dir = Path::new("/new/proj");
    let mut h = History::new(load("basic.jvid", dir));
    let Ok(Created::Marker(id)) = h.apply(Command::AddMarker {
        time: jvid_core::time::Time(10),
        duration: jvid_core::time::Time(80),
        name: "round".into(),
    }) else {
        panic!("no marker");
    };
    let mut m = h.project().sequence().markers()[1].clone();
    assert_eq!(m.id, id);
    (m.color, m.note) = (3, "clutch".into());
    h.apply(Command::SetMarker(m.clone())).unwrap();
    let again = from_json(&to_json(h.project(), dir).unwrap(), dir, |_| true).unwrap();
    let seq = again.sequence();
    assert_eq!(seq.markers()[0].name, "kill");
    assert_eq!(seq.markers()[1], m);
}

#[test]
fn bypassed_effects_are_saved_with_their_keyframes() {
    use jvid_core::keyframe::{Easing, Keyframe, Value};
    use jvid_core::model::Effect;
    let dir = Path::new("/new/proj");
    let mut h = History::new(load("basic.jvid", dir));
    let c = h.project().sequence().tracks()[0].clips()[0].clone();
    let fx = |enabled| Effect {
        shader: "vignette".into(),
        params: [("strength".to_owned(), 0.25)].into(),
        enabled,
    };
    let mut x = c.transform.clone();
    x.effects = vec![fx(true), fx(false)];
    let mut keys = c.keyframes.clone();
    keys.insert(
        Effect::key(1, "strength"),
        vec![Keyframe {
            time: c.source_in,
            value: Value::Scalar(0.75),
            easing: Easing::Linear,
        }],
    );
    h.apply(Command::SetClipProps(vec![(c.id, x, keys)]))
        .unwrap();
    let json = to_json(h.project(), dir).unwrap();
    // Only the bypassed one says so, so older files (and unchanged ones) read as on.
    assert_eq!(json.matches("\"enabled\": false").count(), 1);
    let again = from_json(&json, dir, |_| true).unwrap();
    assert_eq!(again, *h.project());
    let saved = &again.sequence().tracks()[0].clips()[0];
    assert_eq!(saved.transform.effects, [fx(true), fx(false)]);
    assert!(saved.keyframes.contains_key("fx1.strength"));
}
