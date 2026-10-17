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
        let again = from_json(&to_json(&p, dir).unwrap(), dir, |_| true).unwrap();
        assert_eq!(again, p, "{name}");
    }
    let p = load("basic.jvid", dir);
    assert_eq!(p.assets()[0].path, Path::new("/new/proj/media/rec.mkv"));
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
