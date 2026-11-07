//! Mixing audio: balance a voice over music, duck the music under it with volume keys, EQ
//! the voice and record a voiceover.

use jvid_core::history::{Command, Created};
use jvid_core::model::{Clip, ClipId, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND, Time};

use super::{App, Item, Lesson, STEP, Step, apply, select};
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
            anchor: "inspector.volume",
            text: |_| tr!("tutorial.audio_levels").into(),
            done: |a| clip_of(a, MUSIC).is_some_and(|c| c.transform.volume < 1.0),
            example: |a| {
                let Some(c) = clip_of(a, MUSIC) else { return };
                let (id, mut x, keys) = (c.id, c.transform.clone(), c.keyframes.clone());
                x.volume = 0.6;
                a.timeline.select_clip(a.history.project().sequence(), id);
                apply(a, Command::SetClipProps(vec![(id, x, keys)]));
            },
            via: &[select!(music)],
            ..STEP
        },
        Step {
            anchor: "clip",
            item: Some(|a| music(a).map(Item::Clip)),
            text: |_| tr!("tutorial.audio_point", key = verbatim("Ctrl")),
            done: |a| clip_of(a, MUSIC).is_some_and(|c| volume_keys(c) >= 1),
            example: |a| duck(a, 1.0, 1),
            ..STEP
        },
        Step {
            anchor: "clip",
            item: Some(|a| music(a).map(Item::Clip)),
            text: |_| tr!("tutorial.audio_point_again", key = verbatim("Ctrl")),
            done: |a| clip_of(a, MUSIC).is_some_and(|c| volume_keys(c) >= 2),
            example: |a| duck(a, 1.0, 2),
            ..STEP
        },
        Step {
            anchor: "clip",
            item: Some(|a| music(a).map(Item::Clip)),
            text: |_| tr!("tutorial.audio_duck").into(),
            done: |a| clip_of(a, MUSIC).is_some_and(ducked),
            example: |a| duck(a, 0.3, 2),
            ..STEP
        },
        Step {
            anchor: "inspector.eq",
            text: |_| tr!("tutorial.audio_eq").into(),
            done: |a| {
                clip_of(a, VOICE).is_some_and(|c| {
                    let eq = c.transform.audio_fx.eq;
                    eq.enabled && eq.bands.iter().any(|b| *b != 0.0)
                })
            },
            example: |a| {
                let Some(c) = clip_of(a, VOICE) else { return };
                let (id, mut x, keys) = (c.id, c.transform.clone(), c.keyframes.clone());
                // Rumble out, a little presence in.
                let bands = &mut x.audio_fx.eq.bands;
                (bands[0], bands[1], bands[7]) = (-12.0, -6.0, 3.0);
                a.timeline.select_clip(a.history.project().sequence(), id);
                apply(a, Command::SetClipProps(vec![(id, x, keys)]));
            },
            via: &[select!(voice)],
            ..STEP
        },
        // Needs a microphone, so the Show Me test skips it; without one the user skips it too.
        Step {
            anchor: "record",
            text: |_| tr!("tutorial.audio_record").into(),
            done: |a| a.take.is_some() || recorded(a),
            // As pressing Record does: starts a take, or stops the one running.
            example: |a| match a.take.is_some() {
                true => a.end_take(),
                false => a.start_take(),
            },
            reveal: Some(|a| a.preview_active = 1),
            ..STEP
        },
        Step {
            anchor: "record",
            text: |_| tr!("tutorial.audio_stop").into(),
            done: recorded,
            example: |a| match a.take.is_some() {
                true => a.end_take(),
                false => a.start_take(),
            },
            reveal: Some(|a| a.preview_active = 1),
            ..STEP
        },
    ],
};

fn music(a: &App) -> Option<ClipId> {
    clip_of(a, MUSIC).map(|c| c.id)
}

fn voice(a: &App) -> Option<ClipId> {
    clip_of(a, VOICE).map(|c| c.id)
}

/// Whether a take landed: takes are WAVs; the lesson's samples aren't.
fn recorded(a: &App) -> bool {
    let mut assets = a.history.project().assets().iter();
    assets.any(|x| x.path.extension().is_some_and(|e| e == "wav"))
}

fn volume_keys(c: &Clip) -> usize {
    c.keyframes.get("volume").map_or(0, Vec::len)
}

/// Keys the music's volume down to `level` of its gain over the half second before the voice
/// comes in at 2 s, as Ctrl+clicking the line twice and dragging the second point does; the
/// first `n` of those.
fn duck(a: &mut App, level: f64, n: usize) {
    use jvid_core::keyframe::{Easing, Keyframe, Value};
    let Some(c) = clip_of(a, MUSIC) else { return };
    let s = FLICKS_PER_SECOND;
    let v = c.transform.volume;
    let key = |t, v| Keyframe {
        time: Time(t),
        value: Value::Scalar(v),
        easing: Easing::Linear,
    };
    let mut all = c.keyframes.clone();
    let mut duck = vec![
        key(c.source_in.0 + 3 * s / 2, v),
        key(c.source_in.0 + 2 * s, v * level),
    ];
    duck.truncate(n);
    all.insert("volume".into(), duck);
    let cmd = Command::SetClipProps(vec![(c.id, c.transform.clone(), all)]);
    apply(a, cmd);
}

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
