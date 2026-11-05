//! Optional jplay metadata: https://github.com/jarradbanks/jplay/blob/main/docs/sidecar.md.

use std::path::Path;

use jvid_core::model::{ProbeInfo, SourceMarker, StreamInfo};
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(default)]
struct Sidecar {
    version: u32,
    audio_tracks: Vec<String>,
    markers: Vec<Marker>,
}

impl Default for Sidecar {
    fn default() -> Self {
        Self {
            version: 1,
            audio_tracks: Vec::new(),
            markers: Vec::new(),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Marker {
    t: u64,
    kind: String,
    label: String,
}

pub(crate) fn import(media: &Path, info: &mut ProbeInfo) {
    let path = media.with_extension("jplay.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), "could not read jplay sidecar: {e}");
            }
            return;
        }
    };
    let sidecar: Sidecar = match serde_json::from_slice(&bytes) {
        Ok(sidecar) => sidecar,
        Err(e) => {
            tracing::warn!(path = %path.display(), "invalid jplay sidecar: {e}");
            return;
        }
    };
    if sidecar.version > 1 {
        tracing::warn!(
            version = sidecar.version,
            "newer jplay sidecar version; reading known fields"
        );
    }
    let audio = info.streams.iter_mut().filter_map(|s| match s {
        StreamInfo::Audio(a) => Some(a),
        _ => None,
    });
    for (audio, name) in audio.zip(sidecar.audio_tracks) {
        if !name.trim().is_empty() {
            audio.title = Some(name);
        }
    }
    info.markers = sidecar
        .markers
        .into_iter()
        .filter_map(|m| {
            let flicks = m.t.checked_mul((FLICKS_PER_SECOND / 1000) as u64)?;
            let time = Time(i64::try_from(flicks).ok()?);
            if time >= info.duration {
                return None;
            }
            let name = if m.label.is_empty() {
                m.kind
                    .split('_')
                    .map(|word| {
                        let mut chars = word.chars();
                        match chars.next() {
                            Some(first) => {
                                first.to_uppercase().collect::<String>() + chars.as_str()
                            }
                            None => String::new(),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                m.label
            };
            Some(SourceMarker { time, name })
        })
        .collect();
    info.markers.sort_by_key(|m| m.time);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_invalid_and_forward_compatible_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("my.clip.mp4");
        let path = media.with_extension("jplay.json");
        let original = ProbeInfo {
            duration: Time(FLICKS_PER_SECOND),
            ..Default::default()
        };
        for bytes in [None, Some("{"), Some(r#"{"markers":[{"t":-1}]}"#)] {
            if let Some(bytes) = bytes {
                std::fs::write(&path, bytes).unwrap();
            }
            let mut info = original.clone();
            import(&media, &mut info);
            assert_eq!(info, original);
        }
        std::fs::write(
            &path,
            r#"{
            "version":2,"kind":"future","extra":true,
            "markers":[
                {"t":1,"kind":"future_kind","extra":true},
                {},
                {"t":18446744073709551615},
                {"t":13071520000000},
                {"t":1000}
            ]
        }"#,
        )
        .unwrap();
        let mut info = original.clone();
        import(&media, &mut info);
        assert_eq!(
            info.markers,
            [
                SourceMarker {
                    time: Time(0),
                    name: String::new()
                },
                SourceMarker {
                    time: Time(705_600),
                    name: "Future Kind".into()
                },
            ]
        );
        std::fs::write(path, "{}").unwrap();
        let mut info = original.clone();
        import(&media, &mut info);
        assert_eq!(info, original, "missing version and fields default");
    }
}
