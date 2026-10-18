//! Lossless export: when the range is only cuts of one source format, its clips' source ranges
//! are copied straight into the output instead of rendered and re-encoded.

use jvid_core::model::{Clip, ProbeInfo, Project, StreamInfo, TrackKind, Transform};
use jvid_core::time::Time;
use jvid_jobs::Ctx;
use jvid_media::remux::{self, Cut};

use super::{Error, Partial, Request};

/// How a lossless export handles cut points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lossless {
    /// Pure stream copy; each cut snaps to its nearest keyframe.
    Copy,
    /// Re-encodes the partial GOPs at each cut so it lands on the exact frame.
    Smart,
}

/// The source ranges `range` plays, in order, or why it can't be copied: one line per clip or
/// change that needs a render.
pub fn check(project: &Project, (start, end): (Time, Time)) -> Result<Vec<Cut>, Vec<String>> {
    let seq = project.sequence();
    let mut why = Vec::new();
    let name = |c: &Clip| {
        project
            .asset(c.asset)
            .map_or("A clip".into(), |a| a.display_name())
    };
    let tracks = || {
        seq.tracks()
            .iter()
            .filter(|t| !t.hidden || t.kind == TrackKind::Audio)
    };
    let in_range = |c: &&Clip| c.enabled && c.start < end && start < c.end();
    let mut video: Vec<&Clip> = Vec::new();
    let mut audio: Vec<&Clip> = Vec::new();
    let mut video_tracks = 0;
    for t in tracks() {
        let clips: Vec<_> = t.clips().iter().filter(in_range).collect();
        if clips.is_empty() {
            continue;
        }
        if t.kind == TrackKind::Video {
            video_tracks += 1;
            video.extend(clips);
        } else {
            if t.muted || t.solo || t.volume != 1.0 || t.pan != 0.0 {
                why.push(format!(
                    "Track {} has a volume, pan, mute or solo change",
                    t.name
                ));
            }
            audio.extend(clips);
        }
    }
    if video_tracks > 1 {
        why.push("Clips are stacked on more than one video track".into());
    }
    video.sort_by_key(|c| c.start);
    for c in video.iter().chain(&audio) {
        let n = name(c);
        if project.asset(c.asset).is_none_or(|a| a.generated()) {
            why.push(format!("{n} is a title or generated clip"));
        } else if c.speed != 1.0 || c.reverse || c.freeze.is_some() {
            why.push(format!("{n} has a speed change"));
        } else if c.transform != Transform::default() || !c.keyframes.is_empty() {
            why.push(format!(
                "{n} has transforms, effects, fades or audio changes"
            ));
        }
    }
    // Copied video carries all its audio, so each video clip needs its own audio, as placed.
    for c in &video {
        let partners = audio.iter().filter(|a| same_range(a, c)).count();
        let streams = project
            .asset(c.asset)
            .and_then(|a| a.probe.as_ref())
            .map_or(0, |p| p.audio_streams);
        if (partners == 0) != (streams == 0) {
            why.push(format!("{} has its audio removed or changed", name(c)));
        }
    }
    for a in &audio {
        if !video.iter().any(|c| same_range(a, c)) {
            why.push(format!("{} is audio not from the video under it", name(a)));
        }
    }
    let mut at = start;
    for c in &video {
        if c.start > at {
            break;
        }
        at = at.max(c.end());
    }
    if video.is_empty() || at < end {
        why.push("The range has gaps with no video".into());
    }
    let formats: Vec<_> = video
        .iter()
        .filter_map(|c| Some((name(c), project.asset(c.asset)?.probe.as_ref())))
        .collect();
    for (n, p) in &formats {
        match (p, formats[0].1) {
            (None, _) => why.push(format!("{n} hasn't been read yet")),
            (Some(p), Some(first)) if format(p) != format(first) => why.push(format!(
                "{n} is in a different format from {}",
                formats[0].0
            )),
            _ => {}
        }
    }
    if !why.is_empty() {
        why.dedup();
        return Err(why);
    }
    Ok(video
        .iter()
        .map(|c| {
            let path = project.asset(c.asset).expect("checked").path.clone();
            let from = start.max(c.start);
            let to = end.min(c.end());
            Cut {
                path,
                start: c.source_in + (from - c.start),
                end: c.source_in + (to - c.start),
            }
        })
        .collect())
}

fn same_range(a: &Clip, b: &Clip) -> bool {
    (a.asset, a.start, a.source_in, a.source_out) == (b.asset, b.start, b.source_in, b.source_out)
}

/// What has to match for copied packets to play as one stream.
fn format(p: &ProbeInfo) -> String {
    let streams: Vec<_> = p
        .streams
        .iter()
        .map(|s| match s {
            StreamInfo::Video(v) => format!("{} {}x{}", v.codec, v.width, v.height),
            StreamInfo::Audio(a) => format!("{} {} {}", a.codec, a.sample_rate, a.channels),
            StreamInfo::Other { codec } => codec.clone(),
        })
        .collect();
    format!("{} {:?} {streams:?}", p.container, p.frame_rate)
}

/// Copies the range into `req.path`, which must have the source's extension.
pub(super) fn run(
    project: &Project,
    req: &Request,
    mode: Lossless,
    ctx: &Ctx,
) -> Result<ProbeInfo, Error> {
    let cuts = check(project, req.range).map_err(|why| why.join("; "))?;
    let ext = |p: &std::path::Path| p.extension().map(|e| e.to_ascii_lowercase());
    if ext(&req.path) != ext(&cuts[0].path) {
        return Err(format!(
            "lossless export keeps the source container: name the file .{}",
            cuts[0]
                .path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
        )
        .into());
    }
    jvid_media::ffmpeg::init().map_err(|e| e.to_string())?;
    let partial = Partial(&req.path);
    let mut progress = |f| {
        ctx.set_progress(f);
        !ctx.is_cancelled()
    };
    match remux::export(&cuts, &req.path, mode == Lossless::Smart, &mut progress) {
        Err(_) if ctx.is_cancelled() => return Err(Error::Cancelled),
        r => r?,
    }
    ctx.set_progress(1.0);
    let info = jvid_media::probe::probe(&req.path).map_err(|e| format!("output: {e}"))?;
    std::mem::forget(partial);
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::history::{Command, Created, History};
    use jvid_core::model::{AudioInfo, NewClip, VideoInfo};
    use jvid_core::time::FLICKS_PER_SECOND;

    fn secs(s: i64) -> Time {
        Time(s * FLICKS_PER_SECOND)
    }

    fn probe(codec: &str) -> ProbeInfo {
        ProbeInfo {
            container: "mp4".into(),
            audio_streams: 1,
            streams: vec![
                StreamInfo::Video(VideoInfo {
                    codec: codec.into(),
                    width: 1920,
                    height: 1080,
                    ..Default::default()
                }),
                StreamInfo::Audio(AudioInfo {
                    codec: "aac".into(),
                    sample_rate: 48000,
                    channels: 2,
                    ..Default::default()
                }),
            ],
            ..Default::default()
        }
    }

    /// Two files cut back to back, each as linked video and audio: a.mp4 0-4 s at 0, then b.mp4
    /// 2-6 s at 4 s.
    fn cuts(b_codec: &str) -> History {
        let mut h = History::new(Project::default());
        let mut track = |kind, name: &str| {
            let name = name.into();
            match h.apply(Command::AddTrack { kind, name }) {
                Ok(Created::Track(t)) => t,
                _ => panic!("track"),
            }
        };
        let (v, a) = (track(TrackKind::Video, "V1"), track(TrackKind::Audio, "A1"));
        for (file, codec, source_in, start) in [("a.mp4", "h264", 0, 0), ("b.mp4", b_codec, 2, 4)] {
            let Ok(Created::Asset(id)) = h.apply(Command::AddAsset(file.into())) else {
                panic!("asset")
            };
            let probe = Some(probe(codec));
            h.apply(Command::SetAssetProbe { id, probe }).unwrap();
            for track in [v, a] {
                let clip = NewClip {
                    asset: id,
                    source_in: secs(source_in),
                    source_out: secs(source_in + 4),
                    start: secs(start),
                    link: None,
                };
                h.apply(Command::AddClip { track, clip }).unwrap();
            }
        }
        h
    }

    #[test]
    fn plain_cuts_copy_their_source_ranges() {
        let h = cuts("h264");
        let got = check(h.project(), (secs(1), secs(7))).unwrap();
        let want = [("a.mp4", 1, 4), ("b.mp4", 2, 5)].map(|(p, a, b)| Cut {
            path: p.into(),
            start: secs(a),
            end: secs(b),
        });
        assert_eq!(got, want);
    }

    #[test]
    fn says_what_prevents_lossless() {
        let h = cuts("hevc");
        let why = check(h.project(), (secs(0), secs(8))).unwrap_err();
        assert_eq!(why, ["b.mp4 is in a different format from a.mp4"]);
        // Past the last clip there's no video.
        let why = check(cuts("h264").project(), (secs(0), secs(9))).unwrap_err();
        assert_eq!(why, ["The range has gaps with no video"]);

        let mut h = cuts("h264");
        let clip = h.project().sequence().tracks()[0].clips()[0].clone();
        let mut transform = clip.transform.clone();
        transform.opacity = 0.5;
        let edits = vec![(clip.id, transform, Default::default())];
        h.apply(Command::SetClipProps(edits)).unwrap();
        let why = check(h.project(), (secs(0), secs(8))).unwrap_err();
        assert_eq!(
            why,
            ["a.mp4 has transforms, effects, fades or audio changes"]
        );
        // Outside the changed clip the range still copies.
        assert!(check(h.project(), (secs(4), secs(8))).is_ok());
    }
}
