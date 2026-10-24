//! Mixing audio: balance a voice over music, duck the music under it with volume keys, EQ
//! the voice and record a voiceover.

use jvid_core::history::{Command, Created};
use jvid_core::model::{Clip, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND, Time};

use super::{App, Lesson, Step, apply};
use crate::i18n::{tr, verbatim};

pub(super) const VOICE: &str = "voice.m4a";
pub(super) const MUSIC: &str = "music.m4a";

/// Balance a voice over music: set the music's level, duck it under the voice, EQ the voice
/// and record a voiceover. The voice starts 2 s in, over the music.
pub const AUDIO: Lesson = Lesson {
    id: "audio",
    title: || tr!("tutorial.audio"),
    minutes: 5,
    media: &[VOICE, MUSIC],
    setup: |a| {
        let s = FLICKS_PER_SECOND;
        let asset = |a: &App, name| {
            a.history
                .project()
                .assets()
                .iter()
                .find(|x| x.path.ends_with(name))
                .map(|x| x.id)
        };
        if let Some(voice) = asset(a, VOICE)
            && let Some(p) = a
                .history
                .project()
                .placement(voice, Time(2 * s), false, None, false)
        {
            apply(a, Command::Place(p));
        }
        let kind = TrackKind::Audio;
        let Ok(Created::Track(track)) = a.history.apply(Command::AddTrack {
            kind,
            name: String::new(),
        }) else {
            return;
        };
        if let Some(music) = asset(a, MUSIC)
            && let Some(p) =
                a.history
                    .project()
                    .placement(music, Time(0), false, Some(track), false)
        {
            apply(a, Command::Place(p));
        }
    },
    steps: &[
        Step {
            anchor: "inspector",
            text: |_| tr!("tutorial.audio_levels").into(),
            done: |a| clip_of(a, MUSIC).is_some_and(|c| c.transform.volume < 1.0),
            show_me: |a| {
                let Some(c) = clip_of(a, MUSIC) else { return };
                let (id, mut x, keys) = (c.id, c.transform.clone(), c.keyframes.clone());
                x.volume = 0.6;
                a.timeline.select_clip(a.history.project().sequence(), id);
                apply(a, Command::SetClipProps(vec![(id, x, keys)]));
            },
            reveal: None,
            ghost: None,
        },
        Step {
            anchor: "timeline",
            text: |_| tr!("tutorial.audio_duck", key = verbatim("Ctrl")),
            done: |a| clip_of(a, MUSIC).is_some_and(ducked),
            show_me: |a| {
                use jvid_core::keyframe::{Easing, Keyframe, Value};
                let Some(c) = clip_of(a, MUSIC) else { return };
                let s = FLICKS_PER_SECOND;
                let v = c.transform.volume;
                let key = |t, v| Keyframe {
                    time: Time(t),
                    value: Value::Scalar(v),
                    easing: Easing::Linear,
                };
                // Down over the half second before the voice comes in at 2 s.
                let mut keys = c.keyframes.clone();
                let duck = vec![
                    key(c.source_in.0 + 3 * s / 2, v),
                    key(c.source_in.0 + 2 * s, v * 0.3),
                ];
                keys.insert("volume".into(), duck);
                apply(
                    a,
                    Command::SetClipProps(vec![(c.id, c.transform.clone(), keys)]),
                );
            },
            reveal: None,
            ghost: None,
        },
        Step {
            anchor: "inspector",
            text: |_| tr!("tutorial.audio_eq").into(),
            done: |a| {
                clip_of(a, VOICE).is_some_and(|c| {
                    let eq = c.transform.audio_fx.eq;
                    eq.enabled && eq.bands.iter().any(|b| *b != 0.0)
                })
            },
            show_me: |a| {
                let Some(c) = clip_of(a, VOICE) else { return };
                let (id, mut x, keys) = (c.id, c.transform.clone(), c.keyframes.clone());
                // Rumble out, a little presence in.
                let bands = &mut x.audio_fx.eq.bands;
                (bands[0], bands[1], bands[7]) = (-12.0, -6.0, 3.0);
                a.timeline.select_clip(a.history.project().sequence(), id);
                apply(a, Command::SetClipProps(vec![(id, x, keys)]));
            },
            reveal: None,
            ghost: None,
        },
        // Needs a microphone, so the Show Me test skips it; without one the user skips it too.
        Step {
            anchor: "record",
            text: |a| tr!("tutorial.audio_record", key = a.keymap.shortcut("record")),
            // Takes are WAVs; the lesson's samples aren't.
            done: |a| {
                let mut assets = a.history.project().assets().iter();
                assets.any(|x| x.path.extension().is_some_and(|e| e == "wav"))
            },
            // As pressing Record does: starts a take, or stops the one running.
            show_me: |a| match a.take.is_some() {
                true => a.end_take(),
                false => a.start_take(),
            },
            reveal: Some(|a| a.preview_active = 1),
            ghost: None,
        },
    ],
};

/// The clip of sample `name` on the timeline.
pub(super) fn clip_of<'a>(app: &'a App, name: &str) -> Option<&'a Clip> {
    let project = app.history.project();
    let tracks = project.sequence().tracks().iter();
    tracks.flat_map(|t| t.clips()).find(|c| {
        project
            .asset(c.asset)
            .is_some_and(|x| x.path.ends_with(name))
    })
}

/// Whether the clip's volume keys dip somewhere.
pub(super) fn ducked(c: &Clip) -> bool {
    use jvid_core::keyframe::Value;
    let keys = c.keyframes.get("volume").map_or(&[][..], Vec::as_slice);
    let v = keys.iter().filter_map(|k| match k.value {
        Value::Scalar(v) => Some(v),
        _ => None,
    });
    let (lo, hi) = v.fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
    hi - lo > 0.05
}
