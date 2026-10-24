//! Sample clips for the tutorials: embedded in the binary, so any install carries them, and
//! written to the app data folder the first time a lesson asks for one. Sources and licences
//! are in `assets/samples/LICENSES.toml`; `scripts/gen-samples` rebuilds the clips.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Each clip's file name and bytes. Lessons name clips by these file names.
pub const CLIPS: &[(&str, &[u8])] = &[
    (
        "subject.mp4",
        include_bytes!("../assets/samples/subject.mp4"),
    ),
    (
        "green-screen.mp4",
        include_bytes!("../assets/samples/green-screen.mp4"),
    ),
    (
        "dark-warm.mp4",
        include_bytes!("../assets/samples/dark-warm.mp4"),
    ),
    (
        "screen-recording.mp4",
        include_bytes!("../assets/samples/screen-recording.mp4"),
    ),
    ("music.m4a", include_bytes!("../assets/samples/music.m4a")),
    ("voice.m4a", include_bytes!("../assets/samples/voice.m4a")),
];

const LICENSES: &str = include_str!("../assets/samples/LICENSES.toml");

/// Bump when a clip changes, so installs write the new set beside the old instead of mixing them.
const VERSION: &str = "v1";

/// One clip's entry in `LICENSES.toml`.
#[derive(Deserialize)]
struct Clip {
    #[cfg(test)]
    file: String,
    #[cfg(test)]
    licence: String,
    /// The attribution About shows; empty for clips made for jvid.
    #[serde(default)]
    credit: String,
}

fn licenses() -> Vec<Clip> {
    #[derive(Deserialize)]
    struct File {
        clip: Vec<Clip>,
    }
    toml::from_str::<File>(LICENSES)
        .expect("LICENSES.toml parses")
        .clip
}

/// The attribution lines About lists, one per sourced clip, without repeats.
pub fn credits() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in licenses() {
        if !c.credit.is_empty() && !out.contains(&c.credit) {
            out.push(c.credit);
        }
    }
    out
}

/// `name`'s path in the app data folder, writing it out first if it is missing or differs in
/// size. `None` for an unknown name or a system without a data folder.
pub fn path(name: &str) -> std::io::Result<Option<PathBuf>> {
    let Some(dirs) = directories::ProjectDirs::from("", "", "jvid") else {
        return Ok(None);
    };
    path_in(&dirs.data_dir().join("samples").join(VERSION), name)
}

/// The folder holding every clip, each written out first if needed; see [`path`].
pub fn dir() -> std::io::Result<Option<PathBuf>> {
    let mut dir = None;
    for (name, _) in CLIPS {
        dir = path(name)?.and_then(|p| Some(p.parent()?.to_owned()));
    }
    Ok(dir)
}

fn path_in(dir: &Path, name: &str) -> std::io::Result<Option<PathBuf>> {
    let Some(&(_, bytes)) = CLIPS.iter().find(|(n, _)| *n == name) else {
        return Ok(None);
    };
    let path = dir.join(name);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) {
        return Ok(Some(path));
    }
    std::fs::create_dir_all(dir)?;
    // Written beside and renamed, so a decoder never opens half a clip.
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::time::{FLICKS_PER_SECOND, Time};

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jvid-samples-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn every_clip_probes_and_decodes_at_spec() {
        jvid_media::ffmpeg::init().unwrap();
        let dir = tmp("decode");
        let mut total = 0;
        for &(name, bytes) in CLIPS {
            total += bytes.len();
            let path = path_in(&dir, name).unwrap().unwrap();
            let info = jvid_media::probe::probe(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(
                info.duration <= Time(10 * FLICKS_PER_SECOND),
                "{name} is over 10 s"
            );
            if name.ends_with(".mp4") {
                assert_eq!((info.width, info.height), (Some(1280), Some(720)), "{name}");
                let mut d = jvid_media::decode::VideoDecoder::open(&path).unwrap();
                d.frame_at(Time(FLICKS_PER_SECOND))
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
            } else {
                let mut d = jvid_media::audio::AudioDecoder::open(&path, 0).unwrap();
                assert!(d.next_chunk().unwrap().is_some(), "{name} has audio");
            }
        }
        assert!(total <= 12 << 20, "samples are {total} bytes, over 12 MB");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_clip_has_a_licence_and_cc_by_ones_a_credit() {
        let clips = licenses();
        let mut listed: Vec<_> = clips.iter().map(|c| c.file.as_str()).collect();
        let mut embedded: Vec<_> = CLIPS.iter().map(|(n, _)| *n).collect();
        listed.sort();
        embedded.sort();
        assert_eq!(listed, embedded);
        for c in &clips {
            assert!(!c.licence.is_empty(), "{} has no licence", c.file);
            if c.licence.starts_with("CC BY") {
                assert!(credits().contains(&c.credit), "{} is not credited", c.file);
                assert!(!c.credit.is_empty(), "{} is not credited", c.file);
            }
        }
    }

    #[test]
    fn writes_a_missing_or_changed_clip_and_keeps_a_good_one() {
        let dir = tmp("write");
        assert!(path_in(&dir, "nope.mp4").unwrap().is_none());
        let p = path_in(&dir, "voice.m4a").unwrap().unwrap();
        let len = std::fs::metadata(&p).unwrap().len();
        std::fs::write(&p, b"truncated").unwrap();
        path_in(&dir, "voice.m4a").unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().len(), len);
        let modified = std::fs::metadata(&p).unwrap().modified().unwrap();
        path_in(&dir, "voice.m4a").unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), modified);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
