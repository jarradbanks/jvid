//! Voiceover takes: the input device records into a WAV in the project's media folder, and the
//! file lands on the armed track at the record start as one undo step.

use std::path::{Path, PathBuf};

use jvid_audio::{InputBackend, Recorder};
use jvid_core::history::{Command, History};
use jvid_core::model::{AudioInfo, ProbeInfo, StreamInfo, TrackId};
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use jvid_media::waveforms::Pyramid;

use crate::i18n::tr;
use crate::timeline::Live;

const _: () = assert!(jvid_audio::PEAK_FRAMES as i64 == jvid_media::waveforms::BUCKET);

/// `media` beside a saved project; the app data folder's `recordings` for an untitled one.
pub fn media_dir(project: Option<&Path>) -> Option<PathBuf> {
    match project.and_then(Path::parent) {
        Some(dir) => Some(dir.join("media")),
        None => {
            directories::ProjectDirs::from("", "", "jvid").map(|d| d.data_dir().join("recordings"))
        }
    }
}

/// The first `Voiceover <n>.wav` not already in `dir`.
fn free_path(dir: &Path) -> PathBuf {
    (1..)
        .map(|n| {
            dir.join(tr!("timeline.recording_file", n = n))
                .with_extension("wav")
        })
        .find(|p| !p.exists())
        .expect("some name is free")
}

pub struct Take {
    recorder: Recorder,
    pub track: TrackId,
    pub start: Time,
    pub path: PathBuf,
    /// When recording began, for the transport row's elapsed time.
    pub began: std::time::Instant,
    /// The meter: the input peak, falling back slowly.
    level: f32,
}

impl Take {
    pub fn start(
        backend: &mut dyn InputBackend,
        device: Option<&str>,
        dir: &Path,
        track: TrackId,
        start: Time,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let path = free_path(dir);
        let recorder = Recorder::start(backend, device, &path)?;
        Ok(Self {
            recorder,
            track,
            start,
            path,
            began: std::time::Instant::now(),
            level: 0.0,
        })
    }

    /// Call every UI frame: writes what was captured and returns the meter level, 0..=1. An
    /// error (a write failed, the device went away) means the take should end.
    pub fn poll(&mut self) -> Result<f32, String> {
        self.recorder.poll().map_err(|e| e.to_string())?;
        if self.recorder.lost() {
            return Err(tr!("timeline.record_lost").into());
        }
        self.level = self.recorder.level().max(self.level * 0.9);
        Ok(self.level)
    }

    /// Brings `live`, the timeline's drawing of this take, up to what's been written.
    pub fn show(&self, live: &mut Option<Live>) {
        let live = live.get_or_insert_with(|| Live {
            start: self.start,
            name: self
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            rate: self.recorder.format.rate,
            peaks: Vec::new(),
        });
        let peaks = self.recorder.peaks();
        // Only the new peaks are copied; the last one shown may have grown since.
        live.peaks.truncate(live.peaks.len().saturating_sub(1));
        live.peaks.extend_from_slice(&peaks[live.peaks.len()..]);
    }

    /// Closes the file and places it on the track at the record start, as one undo step, and
    /// returns its waveform when it's in the cache's sample rate. On failure the project is
    /// unchanged; the file stays on disk unless it is empty.
    pub fn finish(self, history: &mut History) -> Result<Option<Pyramid>, String> {
        let format = self.recorder.format;
        let (frames, peaks) = self.recorder.finish().map_err(|e| e.to_string())?;
        let path = self.path;
        if frames == 0 {
            let _ = std::fs::remove_file(&path);
            return Ok(None);
        }
        let duration = (frames as i128 * FLICKS_PER_SECOND as i128 / format.rate as i128) as i64;
        let probe = ProbeInfo {
            duration: Time(duration),
            sample_rate: Some(format.rate),
            audio_streams: 1,
            container: "wav".into(),
            streams: vec![StreamInfo::Audio(AudioInfo {
                codec: "pcm_s16le".into(),
                sample_rate: format.rate,
                channels: format.channels.into(),
                ..Default::default()
            })],
            file_size: std::fs::metadata(&path).ok().map(|m| m.len()),
            ..Default::default()
        };
        history.begin();
        let placed = history
            .apply(Command::Import(vec![(path.clone(), probe)]))
            .map_err(|e| e.to_string())
            .and_then(|_| {
                let p = history.project();
                let asset = p.assets().iter().find(|a| a.path == path).map(|a| a.id);
                asset
                    .and_then(|a| p.placement(a, self.start, false, Some(self.track), false))
                    .filter(|pl| pl.audio == Some(self.track))
                    .ok_or_else(|| tr!("timeline.record_no_track").to_string())
            })
            .and_then(|pl| history.apply(Command::Place(pl)).map_err(|e| e.to_string()));
        match placed {
            Ok(_) => history.commit(),
            Err(_) => history.rollback(),
        }
        // shortcut: another rate's peaks aren't resampled, so its clip scans the file instead.
        let shown = (format.rate == jvid_media::audio::SAMPLE_RATE).then(|| Pyramid::new(peaks));
        placed.map(|_| shown)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::any::Any;
    use std::sync::{Arc, Mutex};

    use jvid_audio::{Capture, Format};
    use jvid_core::model::{Project, TrackKind};

    use super::*;

    /// A microphone the test feeds by hand.
    #[derive(Default, Clone)]
    pub struct FakeMic(pub Arc<Mutex<Option<Arc<Capture>>>>);

    impl InputBackend for FakeMic {
        fn devices(&self) -> Vec<String> {
            vec!["mic".into()]
        }
        fn format(&self, _: Option<&str>) -> Result<Format, String> {
            Ok(Format {
                rate: 48_000,
                channels: 1,
            })
        }
        fn open(&mut self, _: Option<&str>, c: Arc<Capture>) -> Result<Box<dyn Any>, String> {
            *self.0.lock().unwrap() = Some(c);
            Ok(Box::new(()))
        }
    }

    #[test]
    fn a_take_lands_on_the_armed_track_at_the_record_start_and_undo_keeps_the_wav() {
        let dir = std::env::temp_dir().join(format!("jvid-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut h = History::new(Project::default());
        h.apply(Command::AddTrack {
            kind: TrackKind::Audio,
            name: String::new(),
        })
        .unwrap();
        let track = h.project().sequence().tracks().last().unwrap().id;
        let start = Time(2 * FLICKS_PER_SECOND);
        let mic = FakeMic::default();
        let mut take = Take::start(&mut mic.clone(), None, &dir, track, start).unwrap();
        // Half a second fed in two device buffers.
        let feed = mic.0.lock().unwrap().clone().unwrap();
        feed.push(&[0.5; 12_000]);
        assert!(take.poll().unwrap() > 0.49);
        feed.push(&[-0.25; 12_000]);
        let path = take.path.clone();
        assert_eq!(path, dir.join("Voiceover 1.wav"));
        let rev = h.rev();
        take.finish(&mut h).unwrap();

        let wav = std::fs::read(&path).unwrap();
        assert_eq!(wav.len(), 44 + 24_000 * 2);
        assert_eq!(&wav[44..46], &16384i16.to_le_bytes());
        assert_eq!(&wav[wav.len() - 2..], &(-8192i16).to_le_bytes());
        let t = h.project().sequence().track(track).unwrap();
        let [clip] = t.clips() else {
            panic!("one clip")
        };
        assert_eq!(clip.start, start);
        assert_eq!(clip.end() - clip.start, Time(FLICKS_PER_SECOND / 2));

        // One undo step removes the clip (and its asset); the file stays.
        assert!(h.undo());
        assert_eq!(h.rev(), rev);
        assert!(
            h.project()
                .sequence()
                .track(track)
                .unwrap()
                .clips()
                .is_empty()
        );
        assert!(path.exists());
        // The next take doesn't overwrite it.
        let next = Take::start(&mut mic.clone(), None, &dir, track, start).unwrap();
        assert_eq!(next.path, dir.join("Voiceover 2.wav"));
        next.finish(&mut h).unwrap();
        assert!(
            !dir.join("Voiceover 2.wav").exists(),
            "an empty take leaves nothing"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_take_onto_a_removed_track_leaves_the_project_unchanged() {
        let dir = std::env::temp_dir().join(format!("jvid-take-gone-{}", std::process::id()));
        let mut h = History::new(Project::default());
        let mic = FakeMic::default();
        let mut take = Take::start(&mut mic.clone(), None, &dir, TrackId(999), Time(0)).unwrap();
        mic.0.lock().unwrap().as_ref().unwrap().push(&[0.1; 480]);
        take.poll().unwrap();
        let before = h.project().clone();
        assert!(take.finish(&mut h).is_err());
        assert_eq!(h.project(), &before);
        assert!(!h.can_undo());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
