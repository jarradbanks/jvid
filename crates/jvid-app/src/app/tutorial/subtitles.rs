//! Subtitles and Edit by Text: make subtitles from the voice sample, fix and restyle one, then
//! cut a phrase by deleting it from the transcript. Without a speech model, Generate shows the
//! normal download choice; offline, each step can be skipped.

use jvid_core::history::Command;
use jvid_core::model::{Clip, MediaAsset, Text};
use jvid_core::time::Time;

use super::audio::{VOICE, clip_of};
use super::{App, Lesson, Step, Tab, apply};
use crate::i18n::{tr, verbatim};

/// Show Me's restyle: the classic yellow.
const YELLOW: [f64; 4] = [1.0, 0.85, 0.0, 1.0];

pub const SUBTITLES: Lesson = Lesson {
    id: "subtitles",
    title: || tr!("tutorial.subtitles"),
    minutes: 5,
    media: &[VOICE],
    setup: |a| {
        let project = a.history.project();
        let Some(asset) = project.assets().first() else {
            return;
        };
        if let Some(p) = project.placement(asset.id, Time(0), false, None, false) {
            apply(a, Command::Place(p));
        }
        select_voice(a);
    },
    steps: &[
        Step {
            anchor: "timeline",
            text: |a| {
                let key = a.keymap.shortcut("generate_subtitles");
                tr!("tutorial.subtitles_generate", key = key)
            },
            done: |a| cues(a).next().is_some(),
            show_me: |a| {
                select_voice(a);
                a.run_action("generate_subtitles", true);
                a.subtitles
                    .start_if_ready(a.history.project(), a.timeline.selected());
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "inspector",
            text: |_| tr!("tutorial.subtitles_fix").into(),
            // Generate names each cue's asset after its text, which a fix leaves behind.
            done: |a| {
                cues(a).any(|(_, x, t)| x.name.as_deref() != Some(&t.content.replace('\n', " ")))
            },
            show_me: |a| {
                edit_first_cue(a, |t| {
                    // Flips the first letter's case, as retyping a word would change it.
                    let mut c = t.content.chars();
                    let first = c.next().map_or(String::new(), |f| match f.is_uppercase() {
                        true => f.to_lowercase().collect(),
                        false => f.to_uppercase().collect(),
                    });
                    t.content = first + c.as_str();
                })
            },
            reveal: None,
            ghost: None,
            also: &["timeline"],
        },
        Step {
            anchor: "inspector",
            text: |_| tr!("tutorial.subtitles_style").into(),
            done: |a| {
                let [.., (_, style)] = Text::presets();
                cues(a).any(|(_, _, t)| {
                    let content = t.content.clone();
                    *t != Text {
                        content,
                        ..style.clone()
                    }
                })
            },
            show_me: |a| edit_first_cue(a, |t| t.color = YELLOW),
            reveal: None,
            ghost: None,
            also: &["timeline"],
        },
        Step {
            anchor: "transcript",
            text: |_| tr!("tutorial.subtitles_transcribe").into(),
            done: |a| {
                let project = a.history.project();
                clip_of(a, VOICE).is_some_and(|c| {
                    let nth = c.stream.unwrap_or(0) as usize;
                    let asset = project.asset(c.asset);
                    asset.is_some_and(|x| a.transcript.heard(&x.path, nth))
                })
            },
            // As clicking Transcribe does.
            show_me: |a| {
                select_voice(a);
                let missing = (a.transcript).missing(a.history.project(), a.timeline.selected());
                if !missing.is_empty() {
                    a.subtitles.open_transcribe(missing);
                    a.subtitles
                        .start_if_ready(a.history.project(), a.timeline.selected());
                }
            },
            reveal: Some(|a| a.show_panel(Tab::Transcript)),
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "transcript",
            text: |_| tr!("tutorial.subtitles_cut", key = verbatim("Delete")),
            done: cut,
            // As picking the second and third words and pressing Delete does.
            show_me: |a| {
                select_voice(a);
                let (project, selected) = (a.history.project(), a.timeline.selected());
                if let Some(c) = a.transcript.cut(project, selected, 1..3) {
                    apply(a, c);
                }
            },
            reveal: Some(|a| a.show_panel(Tab::Transcript)),
            ghost: None,
            also: &["timeline"],
        },
    ],
};

/// Selects the voice clip, so Generate and the Transcript read it.
fn select_voice(a: &mut App) {
    if let Some(id) = clip_of(a, VOICE).map(|c| c.id) {
        a.timeline.select_clip(a.history.project().sequence(), id);
    }
}

/// Each clip on a subtitle track, with its asset and text.
fn cues(a: &App) -> impl Iterator<Item = (&Clip, &MediaAsset, &Text)> {
    let project = a.history.project();
    let tracks = project.sequence().tracks().iter().filter(|t| t.subtitles);
    tracks.flat_map(|t| t.clips()).filter_map(|c| {
        let x = project.asset(c.asset)?;
        Some((c, x, x.text.as_ref()?))
    })
}

/// Selects the first subtitle and edits its text as the Inspector would.
fn edit_first_cue(a: &mut App, edit: impl FnOnce(&mut Text)) {
    let Some((c, x, t)) = cues(a).next() else {
        return;
    };
    let (clip, id, mut text) = (c.id, x.id, t.clone());
    edit(&mut text);
    a.timeline.select_clip(a.history.project().sequence(), clip);
    apply(a, Command::SetText { id, text });
}

/// Whether the voice plays for less than the whole sample: a part of it was cut.
pub(super) fn cut(a: &App) -> bool {
    let project = a.history.project();
    let Some(asset) = project.assets().iter().find(|x| x.path.ends_with(VOICE)) else {
        return false;
    };
    let Some(whole) = asset.probe.as_ref().map(|p| p.duration) else {
        return false;
    };
    let clips = project.sequence().tracks().iter().flat_map(|t| t.clips());
    let voice = clips.filter(|c| c.asset == asset.id);
    let heard: i64 = voice.map(|c| (c.source_out - c.source_in).0).sum();
    heard < whole.0
}
