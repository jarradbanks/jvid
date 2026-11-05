//! Interactive lessons in Help > Tutorials. A lesson is a list of steps, each pointing at an
//! [`anchor`] the editor draws and advancing when its task changes the project or UI state.
//! Guidance leaves the editor usable. It runs in a sandbox project; the user's
//! project, undo history and path wait in a [`Stash`] until the lesson ends.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use egui::{
    Align, Align2, CornerRadius, Id, Key, LayerId, Modifiers, Order, Rect, Stroke, pos2, vec2,
};
use jvid_core::history::{Command, History};
use jvid_core::keyframe::{Easing, Keyframe, Value};
use jvid_core::model::{Project, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use suite_ui::theme;
use suite_ui::tokens::{color, radius, space};
use suite_ui::widgets::Button;

use super::{App, Tab};
use crate::i18n::{tr, verbatim};
use crate::timeline::Timeline;

mod audio;
mod colour;
mod markers;
mod masks;
mod subtitles;

/// One lesson. Its `id` is the progress key in `settings.toml`.
pub struct Lesson {
    pub id: &'static str,
    pub title: fn() -> &'static str,
    pub minutes: u32,
    /// The sample clips the sandbox project imports, by file name.
    pub media: &'static [&'static str],
    /// Prepares the starting project once `media` has imported. It can't be undone.
    pub setup: fn(&mut App),
    pub steps: &'static [Step],
}

pub struct Step {
    /// The [`anchor`] the step points at. An Inspector row's, `inspector.<row>`, falls back to
    /// the whole panel while the row isn't drawn.
    pub anchor: &'static str,
    /// The instruction, one or two short sentences. Shortcuts come from `app.keymap`, so a
    /// remapped key reads right.
    pub text: fn(&App) -> String,
    /// Whether the task is complete. A newly satisfied goal advances automatically; a goal
    /// already satisfied on entry (including Back) waits for Next.
    pub done: fn(&App) -> bool,
    /// A reproducible edit for the lesson regression tests, through the normal commands.
    #[cfg_attr(not(test), allow(dead_code))]
    pub example: fn(&mut App),
    /// Brings the anchor into view when it isn't drawn, such as by switching tabs. The Render
    /// Queue reveals itself.
    pub reveal: Option<fn(&mut App)>,
    /// A target ellipse, its centre and size in project pixels from the frame's centre,
    /// drawn dashed on the Program view while the step shows.
    pub ghost: Option<[[f64; 2]; 2]>,
    /// More controls highlighted beside `anchor`, such as a scope to read while editing in
    /// the Inspector.
    pub also: &'static [&'static str],
}

pub const LESSONS: &[Lesson] = &[
    TOUR,
    masks::MASKS,
    audio::AUDIO,
    KEYFRAMES,
    colour::COLOUR,
    GREEN_SCREEN,
    SCREEN,
    markers::MARKERS,
    subtitles::SUBTITLES,
];

/// The sample the tour edits.
const TOUR_CLIP: &str = "subject.mp4";

/// The first-launch tour: a first edit of a sample clip, from import to export.
const TOUR: Lesson = Lesson {
    id: "first_edit",
    title: || tr!("tutorial.tour"),
    minutes: 5,
    media: &[],
    setup: |_| {},
    steps: &[
        Step {
            anchor: "media",
            text: |a| {
                let key = a.keymap.shortcut("import");
                tr!(
                    "tutorial.tour_import",
                    key = key,
                    file = verbatim(TOUR_CLIP)
                )
            },
            done: |a| clip_named(a, TOUR_CLIP).is_some(),
            example: |a| match sample(TOUR_CLIP) {
                Ok(path) => a.import([path]),
                Err(e) => a.error(tr!(
                    "app.open_failed",
                    path = verbatim(TOUR_CLIP),
                    error = e
                )),
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "timeline",
            text: |_| tr!("tutorial.tour_place", file = verbatim(TOUR_CLIP)),
            done: |a| first_clip(a).is_some(),
            example: |a| {
                let project = a.history.project();
                let Some(asset) = clip_named(a, TOUR_CLIP) else {
                    return;
                };
                if let Some(p) = project.placement(asset.id, Time(0), false, None, false) {
                    apply(a, Command::Place(p));
                }
            },
            reveal: None,
            ghost: None,
            also: &["media"],
        },
        Step {
            anchor: "preview",
            text: |a| {
                let k = |id| a.keymap.shortcut(id);
                let (play, back) = (k("play_stop"), k("shuttle_left"));
                let (stop, forward) = (k("shuttle_stop"), k("shuttle_right"));
                tr!(
                    "tutorial.tour_play",
                    play = play,
                    back = back,
                    stop = stop,
                    forward = forward
                )
            },
            done: |a| a.timeline.playhead() > Time(0),
            example: |a| {
                let length = a.history.project().sequence().duration();
                a.timeline.seek(Time(length.0 / 3));
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "timeline",
            text: |a| {
                let (mark_in, mark_out) =
                    (a.keymap.shortcut("mark_in"), a.keymap.shortcut("mark_out"));
                tr!(
                    "tutorial.tour_marks",
                    mark_in = mark_in,
                    mark_out = mark_out
                )
            },
            done: |a| {
                let seq = a.history.project().sequence();
                matches!((seq.in_point(), seq.out_point()), (Some(i), Some(o)) if o > i)
            },
            example: mark,
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "timeline",
            text: |a| tr!("tutorial.tour_cut", key = a.keymap.shortcut("extract")),
            done: |a| shortened(a, TOUR_CLIP),
            example: |a| {
                let seq = a.history.project().sequence();
                if seq.in_point().is_none() || seq.out_point().is_none() {
                    mark(a);
                }
                apply(a, Command::RemoveRange { ripple: true });
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "bin.titles",
            text: |_| tr!("tutorial.tour_title").into(),
            done: has_title,
            example: add_title,
            reveal: Some(|a| a.bin.tab = 2),
            ghost: None,
            also: &["timeline"],
        },
        Step {
            anchor: "export",
            text: |_| tr!("tutorial.tour_export").into(),
            done: |a| queued(a).next().is_some(),
            example: |a| {
                let Some(gpu) = a.preview.gpu() else {
                    return;
                };
                let name = a.name();
                let req = a
                    .export_dialog
                    .entire(a.history.project().sequence(), &name);
                a.queue_export(req, gpu, 0.0);
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
    ],
};

/// The screen recording lesson's sample.
const SCREEN_CLIP: &str = "screen-recording.mp4";

/// The sample's dead air in seconds, as `scripts/gen-samples` makes it.
const DEAD_AIR: (f64, f64) = (2.4, 7.0);

/// Where the typing sits in the 1280x720 sample, for the zoom.
const TYPING: [f64; 2] = [400.0, 180.0];

/// Tidies a screen recording: cut its dead air, zoom in on the typing, add a callout.
const SCREEN: Lesson = Lesson {
    id: "screen_recording",
    title: || tr!("tutorial.screen"),
    minutes: 5,
    media: &[SCREEN_CLIP],
    setup: |a| {
        let project = a.history.project();
        let Some(asset) = clip_named(a, SCREEN_CLIP) else {
            return;
        };
        if let Some(p) = project.placement(asset.id, Time(0), false, None, false) {
            apply(a, Command::Place(p));
        }
    },
    steps: &[
        Step {
            anchor: "timeline",
            text: |a| {
                tr!(
                    "tutorial.screen_silence",
                    key = a.keymap.shortcut("detect_silence")
                )
            },
            done: |a| shortened(a, SCREEN_CLIP) && a.lesson.as_ref().is_some_and(|r| r.silence_cut),
            example: |a| {
                let Some(asset) = clip_named(a, SCREEN_CLIP).map(|x| x.id) else {
                    return;
                };
                let seq = a.history.project().sequence();
                let audio = seq.tracks().iter().filter(|t| t.kind == TrackKind::Audio);
                let Some(c) = audio.flat_map(|t| t.clips()).find(|c| c.asset == asset) else {
                    return;
                };
                // The span Detect Silence finds at its defaults: the dead air less 0.15 s padding.
                let at = |s: f64| c.timeline_at(Time((s * FLICKS_PER_SECOND as f64) as i64));
                let span = (c.id, at(DEAD_AIR.0 + 0.15), at(DEAD_AIR.1 - 0.15));
                let id = c.id;
                a.timeline.select_clip(seq, id);
                let spans = vec![span];
                apply(a, Command::CutSpans { spans, speed: None });
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "inspector.scale",
            text: |_| tr!("tutorial.screen_zoom").into(),
            done: zoomed,
            example: |a| {
                let Some(asset) = clip_named(a, SCREEN_CLIP).map(|x| x.id) else {
                    return;
                };
                let project = a.history.project();
                let (w, h) = (project.settings().width, project.settings().height);
                let seq = project.sequence();
                let video = seq.tracks().iter().filter(|t| t.kind == TrackKind::Video);
                let clips: Vec<_> = video
                    .flat_map(|t| t.clips())
                    .filter(|c| c.asset == asset)
                    .collect();
                let Some(first) = clips.first().map(|c| c.id) else {
                    return;
                };
                // The fitted image fills the frame, so the anchor is in frame pixels.
                let anchor = [
                    (TYPING[0] / 1280.0 - 0.5) * w as f64,
                    (TYPING[1] / 720.0 - 0.5) * h as f64,
                ];
                let edits = clips
                    .iter()
                    .map(|c| {
                        let mut x = c.transform.clone();
                        (x.scale, x.anchor) = ([1.6; 2], anchor);
                        (c.id, x, c.keyframes.clone())
                    })
                    .collect();
                a.timeline.select_clip(seq, first);
                apply(a, Command::SetClipProps(edits));
            },
            reveal: None,
            ghost: None,
            also: &["timeline"],
        },
        Step {
            anchor: "bin.titles",
            text: |_| tr!("tutorial.screen_callout").into(),
            done: has_title,
            example: add_title,
            reveal: Some(|a| a.bin.tab = 2),
            ghost: None,
            also: &["timeline"],
        },
    ],
};

/// Sample `name` in the project, once imported.
fn clip_named<'a>(app: &'a App, name: &str) -> Option<&'a jvid_core::model::MediaAsset> {
    let assets = app.history.project().assets();
    assets.iter().find(|a| a.path.ends_with(name))
}

/// The green-screen lesson's samples: the clip to key and the background it goes over.
const KEYED: &str = "green-screen.mp4";
const BACKGROUND: &str = "subject.mp4";
/// The bundled effect the lesson adds, by file stem.
const CHROMA_KEY: &str = "chroma_key";

/// Keys the green-screen sample over a background: background under it, Chroma Key, tune
/// it, then mask off the board the key misses.
const GREEN_SCREEN: Lesson = Lesson {
    id: "green_screen",
    title: || tr!("tutorial.green_screen"),
    minutes: 5,
    media: &[KEYED, BACKGROUND],
    // The green-screen clip on the top of two video tracks, with the one under it empty. A new
    // video track goes on top.
    setup: |a| {
        use jvid_core::history::Created;
        let mut track = None;
        for _ in 0..2 {
            let (kind, name) = (jvid_core::model::TrackKind::Video, String::new());
            if let Ok(Created::Track(t)) = a.history.apply(Command::AddTrack { kind, name }) {
                track = Some(t);
            }
        }
        let project = a.history.project();
        let Some(keyed) = clip_named(a, KEYED) else {
            return;
        };
        if let Some(p) = project.placement(keyed.id, Time(0), false, track, false) {
            apply(a, Command::Place(p));
        }
    },
    steps: &[
        Step {
            anchor: "timeline",
            text: |_| {
                tr!(
                    "tutorial.green_screen_background",
                    file = verbatim(BACKGROUND)
                )
            },
            done: |a| {
                let Some((under, _)) = keyed_clip(a) else {
                    return false;
                };
                let Some(bg) = clip_named(a, BACKGROUND) else {
                    return false;
                };
                let tracks = a.history.project().sequence().tracks();
                let mut below = tracks[under + 1..].iter();
                below.any(|t| {
                    t.kind == jvid_core::model::TrackKind::Video
                        && t.clips().iter().any(|c| c.asset == bg.id)
                })
            },
            example: |a| {
                let (Some((at, _)), Some(bg)) = (keyed_clip(a), clip_named(a, BACKGROUND)) else {
                    return;
                };
                let project = a.history.project();
                let tracks = project.sequence().tracks();
                let below = tracks[at + 1..]
                    .iter()
                    .find(|t| t.kind == jvid_core::model::TrackKind::Video);
                let Some(below) = below.map(|t| t.id) else {
                    return;
                };
                if let Some(p) = project.placement(bg.id, Time(0), false, Some(below), false) {
                    apply(a, Command::Place(p));
                }
            },
            reveal: None,
            ghost: None,
            also: &["media"],
        },
        Step {
            anchor: "inspector.add_effect",
            text: |_| tr!("tutorial.green_screen_key").into(),
            done: |a| key_index(a).is_some(),
            example: |a| {
                if let Some((_, id)) = keyed_clip(a) {
                    a.timeline.select_clip(a.history.project().sequence(), id);
                }
                add_key(a);
            },
            reveal: None,
            ghost: None,
            also: &["timeline"],
        },
        Step {
            anchor: "inspector.effect",
            text: |_| tr!("tutorial.green_screen_tune").into(),
            done: |a| {
                let (Some(i), Some(shader)) = (key_index(a), chroma_key()) else {
                    return false;
                };
                let Some((_, id)) = keyed_clip(a) else {
                    return false;
                };
                let Some((_, c)) = a.history.project().sequence().clip(id) else {
                    return false;
                };
                let params = &c.transform.effects[i].params;
                let tuned = |name: &str| {
                    let default = shader.params.iter().find(|p| p.name == name);
                    params.get(name).copied() != default.map(|p| p.default)
                };
                ["tolerance", "edge_softness", "spill_suppression"]
                    .into_iter()
                    .all(tuned)
            },
            example: |a| {
                if key_index(a).is_none() {
                    add_key(a);
                }
                let Some(i) = key_index(a) else {
                    return;
                };
                // Clears the green screen while the board on the right still shows through.
                edit_keyed(a, |_, x| {
                    let params = &mut x.effects[i].params;
                    params.insert("tolerance".into(), 0.8);
                    params.insert("edge_softness".into(), 0.15);
                    params.insert("spill_suppression".into(), 0.7);
                });
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "inspector.add_mask",
            text: |_| tr!("tutorial.green_screen_mask").into(),
            done: |a| {
                let Some((_, id)) = keyed_clip(a) else {
                    return false;
                };
                let seq = a.history.project().sequence();
                seq.clip(id)
                    .is_some_and(|(_, c)| c.transform.masks.iter().any(|m| m.enabled))
            },
            example: |a| {
                use jvid_core::model::{Mask, MaskShape};
                // Keeps the left 65% of the frame, which ends just before the board.
                edit_keyed(a, |[w, h], x| {
                    let points = vec![[-0.2 * w, 0.0], [0.7 * w, 1.1 * h]];
                    x.masks.push(Mask::new(MaskShape::Rectangle, points));
                });
            },
            reveal: None,
            ghost: None,
            also: &["preview"],
        },
    ],
};

/// The topmost green-screen clip: its track's index and its id.
fn keyed_clip(app: &App) -> Option<(usize, jvid_core::model::ClipId)> {
    let keyed = clip_named(app, KEYED)?.id;
    let tracks = app.history.project().sequence().tracks().iter();
    tracks.enumerate().find_map(|(i, t)| {
        let c = t.clips().iter().find(|c| c.asset == keyed)?;
        Some((i, c.id))
    })
}

/// Chroma Key as shipped, for its parameters' defaults.
fn chroma_key() -> Option<jvid_render::effect::Shader> {
    let file = format!("{CHROMA_KEY}.wgsl");
    let (_, src) = jvid_render::effect::BUNDLED
        .iter()
        .find(|(f, _)| *f == file)?;
    jvid_render::effect::parse(src).ok()
}

/// Where Chroma Key is in the green-screen clip's effects.
fn key_index(app: &App) -> Option<usize> {
    let (_, id) = keyed_clip(app)?;
    let (_, c) = app.history.project().sequence().clip(id)?;
    c.transform
        .effects
        .iter()
        .position(|e| e.shader == CHROMA_KEY)
}

/// Adds Chroma Key with its defaults to the green-screen clip, as Add Effect does.
fn add_key(app: &mut App) {
    let Some(shader) = chroma_key() else {
        return;
    };
    edit_keyed(app, |_, x| {
        x.effects.push(jvid_core::model::Effect {
            shader: CHROMA_KEY.into(),
            params: shader
                .params
                .iter()
                .map(|p| (p.name.clone(), p.default))
                .collect(),
            enabled: true,
            mask: None,
        })
    });
}

/// Changes the green-screen clip's transform as one undoable step; `f` also gets the clip's
/// fitted image size, which mask points are in.
fn edit_keyed(app: &mut App, f: impl FnOnce([f64; 2], &mut jvid_core::model::Transform)) {
    let Some((_, id)) = keyed_clip(app) else {
        return;
    };
    let project = app.history.project();
    let Some((_, c)) = project.sequence().clip(id) else {
        return;
    };
    let mut x = c.transform.clone();
    f(crate::masks::fitted(project, c), &mut x);
    let keys = c.keyframes.clone();
    apply(app, Command::SetClipProps(vec![(id, x, keys)]));
}

/// Whether some clip of the screen recording is scaled up at either end, keyframed or not.
fn zoomed(app: &App) -> bool {
    let Some(asset) = clip_named(app, SCREEN_CLIP).map(|x| x.id) else {
        return false;
    };
    let seq = app.history.project().sequence();
    let mut clips = seq.tracks().iter().flat_map(|t| t.clips());
    clips.any(|c| {
        let ends = [c.start, c.end() - Time(1)];
        c.asset == asset
            && ends
                .iter()
                .any(|&t| c.transform_at(t).scale.iter().any(|&s| s > 1.0))
    })
}

/// Whether a title is on the timeline.
fn has_title(app: &App) -> bool {
    let project = app.history.project();
    let tracks = project.sequence().tracks().iter();
    let mut clips = tracks.flat_map(|t| t.clips());
    clips.any(|c| project.asset(c.asset).is_some_and(|x| x.text.is_some()))
}

/// Adds the first title preset at the start, on a new track above the rest, as dropping it
/// over the top track does.
fn add_title(app: &mut App) {
    use jvid_core::history::Created;
    let [(name, _, text), ..] = crate::media_bin::presets();
    let name = name.into();
    let Ok(Created::Asset(title)) = app.history.apply(Command::AddText { name, text }) else {
        return;
    };
    let (kind, name) = (TrackKind::Video, String::new());
    let Ok(Created::Track(track)) = app.history.apply(Command::AddTrack { kind, name }) else {
        return;
    };
    let project = app.history.project();
    if let Some(p) = project.placement(title, Time(0), false, Some(track), false) {
        apply(app, Command::Place(p));
    }
}

/// Sets In and Out around the middle third of the timeline, playhead on Out, as I and O do.
fn mark(app: &mut App) {
    let length = app.history.project().sequence().duration().0;
    let (in_point, out_point) = (Time(length / 3), Time(length * 2 / 3));
    apply(
        app,
        Command::SetInOut {
            in_point: Some(in_point),
            out_point: Some(out_point),
        },
    );
    app.timeline.seek(out_point);
}

/// Whether a section of sample `name` is gone from some track, however it was cut out.
fn shortened(app: &App, name: &str) -> bool {
    let Some(asset) = clip_named(app, name) else {
        return false;
    };
    let full = asset.probe.as_ref().map_or(0, |p| p.duration.0);
    app.history.project().sequence().tracks().iter().any(|t| {
        let clips = t.clips().iter().filter(|c| c.asset == asset.id);
        let kept: i64 = clips.map(|c| c.duration().0).sum();
        kept > 0 && kept < full
    })
}

/// Animates a title in: keys, moving between them, easing, the graph editor and playback.
const KEYFRAMES: Lesson = Lesson {
    id: "keyframes",
    title: || tr!("tutorial.keys"),
    minutes: 5,
    media: &[],
    setup: |a| {
        let [(name, _, text), ..] = crate::media_bin::presets();
        let name = name.into();
        apply(a, Command::AddText { name, text });
        let project = a.history.project();
        let Some(asset) = project.assets().last() else {
            return;
        };
        if let Some(p) = project.placement(asset.id, Time(0), false, None, false) {
            apply(a, Command::Place(p));
        }
        if let Some(id) = first_clip(a) {
            a.timeline.select_clip(a.history.project().sequence(), id);
        }
        a.timeline.seek(Time(0));
    },
    steps: &[
        Step {
            anchor: "inspector.position",
            text: |_| tr!("tutorial.keys_add").into(),
            done: |a| {
                title(a).is_some_and(|c| {
                    let n = |p| c.keyframes.get(p).map_or(0, Vec::len);
                    n("position") >= 2 && n("opacity") >= 2
                })
            },
            example: |a| {
                let Some(c) = title(a) else { return };
                let (start, second) = (c.source_at(c.start), c.source_at(c.start + SECOND));
                let x = &c.transform;
                let [px, py] = x.position;
                let key = |time, value| Keyframe {
                    time,
                    value,
                    easing: Easing::Linear,
                };
                let mut keys = c.keyframes.clone();
                // Rises 100 px into place as it fades in.
                keys.insert(
                    "position".into(),
                    vec![
                        key(start, Value::Vec2([px, py + 100.0])),
                        key(second, Value::Vec2([px, py])),
                    ],
                );
                keys.insert(
                    "opacity".into(),
                    vec![
                        key(start, Value::Scalar(0.0)),
                        key(second, Value::Scalar(x.opacity)),
                    ],
                );
                let at = c.start + SECOND;
                apply(a, Command::SetClipProps(vec![(c.id, x.clone(), keys)]));
                a.timeline.seek(at);
            },
            reveal: None,
            ghost: None,
            also: &["inspector.opacity", "timeline"],
        },
        Step {
            anchor: "inspector.position",
            text: |_| tr!("tutorial.keys_previous").into(),
            done: |a| first_key(a).is_some_and(|t| a.timeline.playhead() == t),
            example: |a| {
                if let Some(t) = first_key(a) {
                    a.timeline.seek(t);
                }
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "inspector.position",
            text: |_| tr!("tutorial.keys_ease").into(),
            done: |a| {
                let eased = |e| e != Easing::Linear;
                title_keys(a).any(|k| eased(k.easing))
            },
            example: |a| ease_first(a, Easing::EaseOut),
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "timeline",
            text: |_| tr!("tutorial.keys_graph").into(),
            done: |a| {
                let custom = |e| matches!(e, Easing::Bezier(_));
                a.timeline.graph_open() && title_keys(a).any(|k| custom(k.easing))
            },
            example: |a| {
                if let Some(id) = first_clip(a) {
                    a.timeline.open_graph(id);
                }
                // A steeper ease out than the preset, as dragging its first handle left gives.
                ease_first(a, Easing::Bezier([0.0, 0.0, 0.2, 1.0]));
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "preview",
            text: |a| tr!("tutorial.keys_play", key = a.keymap.shortcut("play_stop")),
            done: |a| a.preview.playing(),
            example: |a| {
                if let Some(t) = first_key(a) {
                    a.timeline.seek(t);
                }
                a.preview_active = 1;
                a.preview.program_action("play_stop");
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
    ],
};

const SECOND: Time = Time(jvid_core::time::FLICKS_PER_SECOND);

/// The keyframes lesson's title clip.
fn title(app: &App) -> Option<&jvid_core::model::Clip> {
    let id = first_clip(app)?;
    app.history.project().sequence().clip(id).map(|(_, c)| c)
}

/// The title's position and opacity keys.
fn title_keys(app: &App) -> impl Iterator<Item = &Keyframe> {
    let keys = title(app).map(|c| &c.keyframes);
    let lane = move |p| keys.and_then(|k| k.get(p)).into_iter().flatten();
    lane("position").chain(lane("opacity"))
}

/// The timeline time of the title's first position key.
fn first_key(app: &App) -> Option<Time> {
    let c = title(app)?;
    let k = c.keyframes.get("position")?.first()?;
    Some(c.timeline_at(k.time))
}

/// Sets the easing of the title's first position and opacity keys, where it animates in.
fn ease_first(app: &mut App, e: Easing) {
    let Some(c) = title(app) else { return };
    let mut keys = c.keyframes.clone();
    for p in ["position", "opacity"] {
        if let Some(k) = keys.get_mut(p).and_then(|k| k.first_mut()) {
            k.easing = e;
        }
    }
    let cmd = Command::SetClipProps(vec![(c.id, c.transform.clone(), keys)]);
    apply(app, cmd);
}

/// Sample clip `name`'s path, written out first if needed.
fn sample(name: &str) -> std::io::Result<PathBuf> {
    crate::samples::path(name)?.ok_or_else(|| std::io::ErrorKind::NotFound.into())
}

/// The lengths of the exports queued since the lesson started.
fn queued(app: &App) -> impl Iterator<Item = Time> + '_ {
    let mark = app.lesson.as_ref().map_or(0, |r| r.baseline.queued);
    app.queue.since(mark)
}

fn apply(app: &mut App, c: Command) {
    let cut = silence_cut(&c, app.history.project());
    match app.history.apply(c) {
        Ok(_) => app.lesson_silence_cut(cut),
        Err(e) => tracing::warn!("lesson edit failed: {e}"),
    }
}

/// Observe the successful semantic edit, rather than guessing from a shorter timeline.
/// Cut must affect this lesson's recording; Speed Up and unrelated cuts do not count.
pub(super) fn silence_cut(command: &Command, project: &Project) -> bool {
    let Command::CutSpans { spans, speed: None } = command else {
        return false;
    };
    spans.iter().any(|(id, start, end)| {
        start < end
            && project
                .sequence()
                .clip(*id)
                .and_then(|(_, c)| project.asset(c.asset))
                .is_some_and(|a| a.path.ends_with(SCREEN_CLIP))
    })
}

fn first_clip(app: &App) -> Option<jvid_core::model::ClipId> {
    let seq = app.history.project().sequence();
    seq.tracks()
        .iter()
        .find_map(|t| t.clips().first().map(|c| c.id))
}

/// Turned on by a running lesson, so [`anchor`] records nothing otherwise.
const RUNNING: &str = "lesson_running";

/// Whether a lesson runs.
pub(crate) fn running(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(Id::new(RUNNING)).unwrap_or(false))
}

/// Records where anchor `name` was drawn this frame, for a lesson step that points at it.
pub fn anchor(ctx: &egui::Context, name: &'static str, rect: Rect) {
    if !running(ctx) {
        return;
    }
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let a = d.get_temp_mut_or_default::<Anchors>(Id::NULL);
        if a.pass != pass {
            *a = Anchors {
                pass,
                rects: HashMap::new(),
            };
        }
        a.rects.insert(name, rect);
    });
}

/// Where anchor `name` was drawn this frame, if it was.
pub fn anchor_rect(ctx: &egui::Context, name: &str) -> Option<Rect> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| {
        let a = d.get_temp::<Anchors>(Id::NULL)?;
        (a.pass == pass).then(|| a.rects.get(name).copied())?
    })
}

/// The Inspector row a step wants scrolled into view, once it's drawn.
const SCROLL: &str = "lesson_scroll";

/// Records the rows `ui` laid out below `top` as anchor `name`, cut to what shows of them.
/// The first time a step wants them, they scroll into view.
pub fn anchor_below(ui: &egui::Ui, name: &'static str, top: f32) {
    let ctx = ui.ctx();
    if !running(ctx) {
        return;
    }
    let r = ui.min_rect();
    let rows = Rect::from_x_y_ranges(r.x_range(), top..=r.bottom());
    if ctx.data(|d| d.get_temp::<&'static str>(Id::new(SCROLL))) == Some(name) {
        ui.scroll_to_rect(rows, Some(Align::Center));
        ctx.data_mut(|d| d.remove::<&'static str>(Id::new(SCROLL)));
    }
    let rows = rows.intersect(ui.clip_rect());
    if rows.is_positive() {
        anchor(ui.ctx(), name, rows);
    }
}

/// The row in an Inspector row's anchor, `inspector.<row>`.
fn row_name(anchor: &str) -> Option<&str> {
    anchor.strip_prefix("inspector.")
}

/// The Inspector tab Inspector row `row` is on.
fn pane_of(row: &str) -> crate::inspector::Pane {
    use crate::inspector::Pane;
    match row {
        "add_effect" | "effect" => Pane::Effects,
        "volume" | "eq" => Pane::Audio,
        _ => Pane::Video,
    }
}

/// The current step's [`Step::ghost`], for [`ghost`] to draw.
const GHOST: &str = "lesson_ghost";

/// Draws the current step's [`Step::ghost`] as a dashed Ink line over `frame`, the Program view's
/// on-screen project rect, `width` project pixels wide.
pub fn ghost(ui: &egui::Ui, frame: Rect, width: u32) {
    let ctx = ui.ctx();
    let Some([c, size]) = ctx.data(|d| d.get_temp::<[[f64; 2]; 2]>(Id::new(GHOST))) else {
        return;
    };
    if !running(ctx) {
        return;
    }
    let s = frame.width() / width as f32;
    let points: Vec<_> = (0..=64)
        .map(|k| {
            let (sin, cos) = (k as f64 / 64.0 * std::f64::consts::TAU).sin_cos();
            let p = [c[0] + cos * size[0] / 2.0, c[1] + sin * size[1] / 2.0];
            frame.center() + vec2(p[0] as f32, p[1] as f32) * s
        })
        .collect();
    // Ink, so it reads apart from the mask's own Light Blue outline.
    let line = Stroke::new(1.0, color::ink());
    let dashes = egui::Shape::dashed_line(&points, line, space::S8, space::S4);
    ui.painter().with_clip_rect(frame).extend(dashes);
}

#[derive(Clone, Default)]
struct Anchors {
    pass: u64,
    rects: HashMap<&'static str, Rect>,
}

/// The lesson running, and the user's project it set aside.
pub struct Run {
    pub lesson: &'static Lesson,
    /// The current step; `steps.len()` once finished.
    pub step: usize,
    /// The current step's goal, measured on entry and after each frame.
    done: bool,
    /// The goal has been false during this visit, so satisfying it can advance.
    armed: bool,
    /// A successful Detect Silence Cut of the lesson sample, never a trim or deletion.
    silence_cut: bool,
    /// The lesson's `setup` ran; the steps wait for it.
    set_up: bool,
    /// The current step's reveal ran. Once per visit, so a missing anchor can't repaint every
    /// frame.
    revealed: bool,
    /// The current step's missing anchor was logged.
    warned: bool,
    /// A step passed its check rather than being skipped.
    completed: bool,
    /// State outside the sandbox project as the lesson started, for checks to compare against.
    pub baseline: Baseline,
    stash: Stash,
}

/// What a lesson's checks measure from, so state from before it can't pass a step.
pub struct Baseline {
    /// [`crate::export::queue::Queue::pushed`]: exports queued since count, earlier ones don't.
    pub queued: usize,
}

impl Run {
    fn go(&mut self, step: usize) {
        self.step = step.min(self.lesson.steps.len());
        (self.revealed, self.warned, self.done) = (false, false, false);
        self.armed = false;
    }
}

struct Stash {
    history: History,
    path: Option<PathBuf>,
    missing: HashSet<PathBuf>,
    match_source: bool,
    /// The playhead, selection and clipboard: clip ids repeat across projects.
    timeline: Timeline,
    bin_tab: usize,
    /// Started from the Welcome screen, so exit goes back there.
    on_welcome: bool,
}

#[derive(Clone, Copy)]
enum Click {
    Next,
    Back,
    ImportSample,
    Skip,
    Keep,
    Exit,
}

impl App {
    pub(super) fn lesson_silence_cut(&mut self, cut: bool) {
        if cut
            && let Some(run) = &mut self.lesson
            && run.lesson.id == SCREEN.id
            && run.step == 0
        {
            run.silence_cut = true;
        }
    }

    /// Opens `lesson` in a sandbox project built from its media, setting the user's aside.
    /// Shows an error and starts nothing if a clip can't be written out.
    pub(super) fn start_lesson(&mut self, lesson: &'static Lesson) {
        let media: std::io::Result<Vec<_>> = lesson.media.iter().map(|n| sample(n)).collect();
        let media = match media {
            Ok(m) => m,
            Err(e) => {
                let path = lesson.media.join(", ");
                return self.error(tr!("app.open_failed", path = path, error = e));
            }
        };
        self.end_lesson();
        if !self.leave_project() {
            return;
        }
        // Autosave is off in a lesson, so the edits it hasn't written yet are written now.
        if let Some(target) = self.autosave_target() {
            self.autosave.flush(&self.history, &target);
        }
        let timeline = self.timeline.sibling();
        let stash = Stash {
            history: std::mem::replace(&mut self.history, History::new(Project::default())),
            path: self.path.take(),
            missing: std::mem::take(&mut self.missing),
            match_source: std::mem::replace(&mut self.match_source, true),
            timeline: std::mem::replace(&mut self.timeline, timeline),
            bin_tab: self.bin.tab,
            on_welcome: std::mem::take(&mut self.on_welcome),
        };
        self.reset_project_session();
        self.import(media);
        self.lesson = Some(Run {
            lesson,
            step: 0,
            done: false,
            armed: false,
            silence_cut: false,
            set_up: false,
            revealed: false,
            warned: false,
            completed: false,
            baseline: Baseline {
                queued: self.queue.pushed(),
            },
            stash,
        });
    }

    /// Ends the lesson, if one runs, and puts the user's project back as it was.
    pub(super) fn end_lesson(&mut self) {
        let Some(run) = self.lesson.take() else {
            return;
        };
        if !self.leave_project() {
            self.lesson = Some(run);
            return;
        }
        // The lesson's playback would run on into the user's timeline.
        if self.preview.playing() {
            self.preview.program_action("shuttle_stop");
        }
        let s = run.stash;
        self.history = s.history;
        self.path = s.path;
        self.missing = s.missing;
        self.match_source = s.match_source;
        self.timeline = s.timeline;
        self.bin.tab = s.bin_tab;
        self.on_welcome = s.on_welcome;
        self.reset_project_session();
    }

    /// Esc exits the lesson and Enter presses Next, unless a dialog, menu or text field would
    /// take them. Runs before anything else this frame, so it sees what was open last frame.
    pub(super) fn lesson_keys(&mut self, ctx: &egui::Context) {
        if self.lesson.is_none() {
            return;
        }
        let free = ctx.memory(|m| m.top_modal_layer().is_none() && m.focused().is_none())
            && !egui::Popup::is_any_open(ctx)
            && !self.timeline.dialog_open();
        if free && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            return self.end_lesson();
        }
        let done = self.lesson.as_ref().is_some_and(|r| r.done);
        if free && done && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            let mut run = self.lesson.take().expect("checked above");
            self.next_step(&mut run);
            self.lesson = Some(run);
            ctx.request_repaint();
        }
    }

    /// Runs the lesson a frame, after the panels drew their anchors: checks the step, reveals a
    /// hidden anchor, and draws contextual highlights and the task card.
    pub(super) fn lesson_frame(&mut self, ctx: &egui::Context) {
        let recorded = running(ctx);
        if recorded != self.lesson.is_some() {
            ctx.data_mut(|d| d.insert_temp(Id::new(RUNNING), self.lesson.is_some()));
            ctx.request_repaint();
        }
        // Without this frame's anchors, a step would look hidden.
        if !recorded {
            return;
        }
        let Some(mut run) = self.lesson.take() else {
            return;
        };
        if !run.set_up {
            if self.importer.is_busy() {
                self.lesson = Some(run);
                return;
            }
            (run.lesson.setup)(self);
            // The starting project isn't an edit: nothing to undo or save.
            self.history = History::new(self.history.project().clone());
            run.set_up = true;
        }
        let steps = run.lesson.steps;
        run = self.check_lesson_step(run);
        // Let drags finish before switching guidance; keep their undo gesture intact.
        if run.armed && run.done && !ctx.input(|i| i.pointer.any_down()) {
            self.next_step(&mut run);
            run = self.check_lesson_step(run);
            ctx.request_repaint();
        }
        let step = steps.get(run.step);
        let ghost = step.and_then(|s| s.ghost);
        if ghost != ctx.data(|d| d.get_temp(Id::new(GHOST))) {
            ctx.data_mut(|d| match ghost {
                Some(g) => {
                    d.insert_temp(Id::new(GHOST), g);
                }
                None => d.remove::<[[f64; 2]; 2]>(Id::new(GHOST)),
            });
            ctx.request_repaint();
        }
        let guidance = step.map(|s| self.lesson_guidance(ctx, &run, s));
        let anchor = guidance.as_ref().map(|g| g.anchor);
        let exact = anchor.and_then(|a| anchor_rect(ctx, a));
        let panel = || row_name(anchor?).and(anchor_rect(ctx, Tab::Inspector.anchor()));
        let target = exact.or_else(panel);
        if let Some(s) = step.filter(|_| !self.modal_owns_input(ctx)) {
            let reveal = s.reveal.filter(|_| exact.is_none());
            let tab = [
                Tab::Media,
                Tab::Preview,
                Tab::Inspector,
                Tab::Timeline,
                Tab::Transcript,
                Tab::RenderQueue,
            ]
            .into_iter()
            .find(|tab| {
                s.anchor == tab.anchor() || (*tab == Tab::Inspector && row_name(s.anchor).is_some())
            })
            .filter(|_| exact.is_none());
            // A hidden Inspector row the step names: its tab shows, and it scrolls into view.
            let mut names = std::iter::once(s.anchor).chain(s.also.iter().copied());
            let row = names.find(|a| row_name(a).is_some() && anchor_rect(ctx, a).is_none());
            let first = !run.revealed;
            let pending = ctx.data(|d| d.get_temp::<&'static str>(Id::new(SCROLL)));
            // Adding controls can move a previously revealed row below the viewport again.
            let scroll = row.filter(|row| pending != Some(*row));
            if (first && (reveal.is_some() || tab.is_some())) || scroll.is_some() {
                run.revealed = true;
                if first && let Some(tab) = tab {
                    self.show_panel(tab);
                }
                if first && let Some(reveal) = reveal {
                    reveal(self);
                }
                if let Some(row) = scroll {
                    crate::inspector::choose(ctx, pane_of(row_name(row).unwrap_or_default()));
                    ctx.data_mut(|d| d.insert_temp(Id::new(SCROLL), row));
                }
                ctx.request_repaint();
            } else if target.is_none() && !run.warned {
                run.warned = true;
                let (id, n, anchor) = (run.lesson.id, run.step, s.anchor);
                tracing::warn!("lesson {id} step {n}: anchor `{anchor}` isn't drawn");
            }
        }
        let dialogs = dialog_rects(ctx);
        let mut targets = Vec::new();
        // A dialog supersedes its launcher. Unrelated dialogs suppress editor highlights.
        if dialogs.is_empty()
            || anchor.is_some_and(|a| {
                matches!(
                    a,
                    "silence.threshold" | "export.action" | "subtitles.dialog" | "subtitles.action"
                )
            }) && exact.is_some()
        {
            targets.extend(target);
            if dialogs.is_empty()
                && let Some(s) = step
            {
                targets.extend(s.also.iter().filter_map(|a| anchor_rect(ctx, a)));
            }
        }
        if anchor == Some("silence.threshold") {
            targets.extend(anchor_rect(ctx, "silence.cut"));
        }
        highlight(ctx, &targets);
        let text = guidance.map_or_else(|| tr!("tutorial.complete").into(), |g| g.text);
        let Some(click) = card(ctx, &run, &text, &targets, &dialogs) else {
            self.lesson = Some(run);
            return;
        };
        ctx.request_repaint();
        match click {
            Click::Next => self.next_step(&mut run),
            Click::Back => {
                self.dismiss_lesson_dialog(&run);
                run.go(run.step.saturating_sub(1));
            }
            Click::ImportSample => match sample(TOUR_CLIP) {
                Ok(path) => self.import([path]),
                Err(e) => self.error(tr!(
                    "app.open_failed",
                    path = verbatim(TOUR_CLIP),
                    error = e
                )),
            },
            Click::Skip => {
                self.dismiss_lesson_dialog(&run);
                run.done = false;
                self.next_step(&mut run);
            }
            Click::Keep => {
                // Save must still see the sandbox, so it preserves the user's untitled
                // autosave and applies the lesson's normal save behavior.
                self.lesson = Some(run);
                self.save(true);
                run = self.lesson.take().expect("saving keeps the lesson running");
            }
            Click::Exit => {
                self.lesson = Some(run);
                return self.end_lesson();
            }
        }
        self.lesson = Some(run);
    }

    fn lesson_guidance(&self, ctx: &egui::Context, run: &Run, step: &Step) -> Guidance {
        let guide = |anchor, text| Guidance { anchor, text };
        if run.lesson.id == SCREEN.id && run.step == 0 {
            if anchor_rect(ctx, "silence.dialog").is_some() {
                return guide("silence.threshold", tr!("tutorial.silence_cut").into());
            }
            let selected = self.timeline.selected();
            let sample_selected = self
                .history
                .project()
                .sequence()
                .tracks()
                .iter()
                .flat_map(|t| t.clips())
                .any(|c| {
                    selected.contains(&c.id)
                        && self
                            .history
                            .project()
                            .asset(c.asset)
                            .is_some_and(|a| a.path.ends_with(SCREEN_CLIP))
                });
            if !sample_selected {
                return guide("timeline", tr!("tutorial.silence_select").into());
            }
            return guide(
                "menu.edit",
                tr!(
                    "tutorial.silence_open",
                    key = self.keymap.shortcut("detect_silence")
                ),
            );
        }
        if step.anchor == "export" && self.export_dialog.is_open() {
            return guide("export.action", tr!("tutorial.export_settings").into());
        }
        if run.lesson.id == subtitles::SUBTITLES.id && self.subtitles.is_open() {
            let text = if self.subtitles.is_running() {
                tr!("tutorial.speech_running").into()
            } else {
                let action = if step.anchor == "transcript" {
                    tr!("transcript.transcribe")
                } else {
                    tr!("subtitles.generate")
                };
                tr!("tutorial.speech_settings", action = action)
            };
            return guide(
                if self.subtitles.is_running() {
                    "subtitles.dialog"
                } else {
                    "subtitles.action"
                },
                text,
            );
        }
        guide(step.anchor, (step.text)(self))
    }

    /// Establish a fresh baseline on each visit. Back and a prior action satisfying a later
    /// goal must not cascade through lessons, while newly completed tasks need no Next click.
    fn check_lesson_step(&mut self, mut run: Run) -> Run {
        if run.lesson.id == SCREEN.id && run.step == 0 && !shortened(self, SCREEN_CLIP) {
            run.silence_cut = false;
        }
        let Some(step) = run.lesson.steps.get(run.step) else {
            return run;
        };
        self.lesson = Some(run);
        let done = (step.done)(self);
        let mut run = self.lesson.take().expect("the check cannot end the lesson");
        run.done = done;
        run.armed |= !done;
        run
    }

    fn dismiss_lesson_dialog(&mut self, run: &Run) {
        let Some(step) = run.lesson.steps.get(run.step) else {
            return;
        };
        if run.lesson.id == SCREEN.id && run.step == 0 {
            self.timeline.dismiss_dialogs();
        }
        if run.lesson.id == subtitles::SUBTITLES.id {
            self.subtitles.close();
        }
        if step.anchor == "export" {
            self.export_dialog.close();
        }
    }

    /// Next, or Skip Step once `run.done` is cleared: only a step done counts as completed.
    fn next_step(&mut self, run: &mut Run) {
        run.completed |= run.done;
        run.go(run.step + 1);
        // Skipping every step isn't finishing the lesson.
        if run.step == run.lesson.steps.len() && run.completed {
            self.lesson_done(run.lesson.id);
        }
    }

    /// Marks lesson `id` done in Help > Tutorials and saves it to `settings.toml`.
    fn lesson_done(&mut self, id: &str) {
        if self.tutorials_done.iter().any(|d| d == id) {
            return;
        }
        self.tutorials_done.push(id.into());
        let Some(path) = &self.settings_path else {
            return;
        };
        let done = self.tutorials_done.iter().map(|d| d.as_str().into());
        let value = toml::Value::Array(done.collect());
        if let Err(e) = crate::settings_file::save_value_to(path, "tutorials_done", value) {
            tracing::warn!("couldn't save tutorial progress: {e}");
        }
    }

    /// Stops the Welcome screen offering the tour, now and in `settings.toml`.
    pub(super) fn tour_offered(&mut self) {
        self.welcome.tour = false;
        let Some(path) = &self.settings_path else {
            return;
        };
        if let Err(e) = crate::settings_file::save_value_to(path, "tour_offered", true.into()) {
            tracing::warn!("couldn't save the tour offer: {e}");
        }
    }

    /// The Help menu's Tutorials rows: each lesson with its length, and a check once done.
    pub(super) fn tutorials_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button(tr!("menu.tutorials"), |ui| {
            for lesson in LESSONS {
                let done = self.tutorials_done.iter().any(|d| d == lesson.id);
                let check = if done {
                    color::ink_2()
                } else {
                    egui::Color32::TRANSPARENT
                };
                let mark = egui::RichText::new(theme::icon::CHECK)
                    .font(theme::icon_font())
                    .color(check);
                let row = egui::Button::new((mark, (lesson.title)()))
                    .shortcut_text(tr!("tutorial.minutes", count = lesson.minutes));
                if ui.add(row).clicked() {
                    self.start_lesson(lesson);
                }
            }
        });
    }
}

struct Guidance {
    anchor: &'static str,
    text: String,
}

/// Paint only outlines; there is no backdrop and no input-capturing tutorial layer.
fn highlight(ctx: &egui::Context, targets: &[Rect]) {
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("lesson_highlight")));
    for &target in targets {
        painter.rect_stroke(
            target.expand(2.0).intersect(ctx.content_rect()),
            CornerRadius::same(radius::CONTROL),
            Stroke::new(2.0, color::light_blue()),
            egui::StrokeKind::Inside,
        );
    }
}

/// Current-frame dialog bounds, including dialogs unrelated to this lesson. The card must
/// leave their controls and footers clear even when a dialog is moved or resized.
fn dialog_rects(ctx: &egui::Context) -> Vec<Rect> {
    let max_width = ctx.content_rect().width() * 0.95;
    let mut rects: Vec<_> = ["silence.dialog", "export.dialog", "subtitles.dialog"]
        .into_iter()
        .filter_map(|a| anchor_rect(ctx, a))
        .collect();
    rects.extend(ctx.memory(|m| {
        m.areas().visible_layer_ids().into_iter()
        .filter(|l| l.order == Order::Middle || Some(*l) == m.top_modal_layer())
        .filter_map(|l| m.area_rect(l.id))
        // The dock and modal backdrops may cover the whole viewport.
        .filter(|r| r.width() < max_width)
        .collect::<Vec<_>>()
    }));
    rects
}

/// `screen` less `holes`, as rects: in each band between hole edges, the gaps between the
/// holes crossing it.
pub(crate) fn shaded(screen: Rect, holes: &[Rect]) -> Vec<Rect> {
    let edges = holes.iter().flat_map(|h| [h.top(), h.bottom()]);
    let mut ys: Vec<f32> = edges.chain([screen.top(), screen.bottom()]).collect();
    ys.sort_by(f32::total_cmp);
    ys.dedup();
    let mut out = Vec::new();
    for band in ys.windows(2) {
        let (top, bottom) = (band[0], band[1]);
        let crossing = holes
            .iter()
            .filter(|h| h.top() <= top && h.bottom() >= bottom);
        let mut spans: Vec<_> = crossing.map(|h| h.x_range()).collect();
        spans.sort_by(|a, b| a.min.total_cmp(&b.min));
        let mut x = screen.left();
        for s in spans
            .into_iter()
            .chain([egui::Rangef::point(screen.right())])
        {
            if s.min > x {
                out.push(Rect::from_min_max(pos2(x, top), pos2(s.min, bottom)));
            }
            x = x.max(s.max);
        }
    }
    out
}

const CARD_W: f32 = 296.0;
const CARD_ID: &str = "lesson_card";

/// Choose a stable corner clear of the active controls, focused widget and whole dialogs.
/// Try narrower cards beside dialogs before allowing any overlap at small window sizes.
fn card_rect(screen: Rect, size: egui::Vec2, avoid: &[Rect], previous: Option<Rect>) -> Rect {
    let screen = screen.shrink(space::S8);
    let fits = |r: Rect| {
        screen.contains_rect(r) && avoid.iter().all(|a| !r.intersects(a.expand(space::S8)))
    };
    if let Some(r) = previous.map(|r| Rect::from_min_size(r.min, size))
        && fits(r)
    {
        return r;
    }
    let mut candidates = Vec::new();
    for width in [size.x, 208.0_f32.min(size.x)] {
        // Account for extra text and footer wrapping in a narrow card.
        let height = (size.y * size.x / width).min(screen.height());
        let size = vec2(width, height);
        for align in [
            Align2::LEFT_TOP,
            Align2::RIGHT_TOP,
            Align2::LEFT_BOTTOM,
            Align2::RIGHT_BOTTOM,
        ] {
            let r = align.align_size_within_rect(size, screen);
            if fits(r) {
                return r;
            }
            candidates.push(r);
        }
    }
    candidates
        .into_iter()
        .min_by(|a, b| {
            let overlap = |r: Rect| {
                avoid
                    .iter()
                    .filter(|a| r.intersects(a.expand(space::S8)))
                    .map(|a| r.intersect(a.expand(space::S8)).area())
                    .sum::<f32>()
            };
            overlap(*a).total_cmp(&overlap(*b))
        })
        .unwrap_or(screen)
}

/// A compact, non-modal task card. Buttons retain normal Tab/Enter/Space behavior and let
/// go of focus after activation so subsequent editor shortcuts still reach their panel.
fn card(
    ctx: &egui::Context,
    run: &Run,
    text: &str,
    targets: &[Rect],
    dialogs: &[Rect],
) -> Option<Click> {
    let id = Id::new(CARD_ID);
    let previous = ctx.memory(|m| m.area_rect(id));
    let size = previous.map_or(vec2(CARD_W, 160.0), |r| vec2(CARD_W, r.height()));
    let mut avoid = dialogs.to_vec();
    avoid.extend_from_slice(targets);
    let focused = ctx
        .memory(|m| m.focused())
        .and_then(|id| ctx.read_response(id));
    if let Some(r) = focused
        && r.layer_id.id != id
    {
        avoid.push(r.rect);
    }
    let rect = card_rect(ctx.content_rect(), size, &avoid, previous);
    let steps = run.lesson.steps;
    let step = steps.get(run.step);
    let mut click = None;
    let layer = LayerId::new(Order::Foreground, id);
    // Keep Skip and Exit reachable above a modal's backdrop without becoming a modal.
    ctx.move_to_top(layer);
    egui::Area::new(id)
        .order(Order::Foreground)
        .sense(egui::Sense::hover())
        .movable(false)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(color::raised())
                .inner_margin(space::S12)
                .show(ui, |ui| {
                    ui.set_width((rect.width() - 2.0 * space::S12).max(120.0));
                    ui.label(
                        egui::RichText::new((run.lesson.title)())
                            .font(theme::body_strong())
                            .color(color::ink()),
                    );
                    if step.is_some() {
                        ui.label(
                            egui::RichText::new(tr!(
                                "tutorial.step",
                                n = run.step + 1,
                                total = steps.len()
                            ))
                            .font(theme::small())
                            .color(color::ink_3()),
                        );
                    }
                    ui.add_space(space::S4);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .font(theme::body())
                                .color(color::ink()),
                        )
                        .wrap(),
                    );
                    ui.add_space(space::S8);
                    let mut buttons = Vec::new();
                    if run.step > 0 {
                        buttons.push((Button::ghost(tr!("tutorial.back")), Click::Back));
                    }
                    buttons.push((Button::ghost(tr!("tutorial.exit")), Click::Exit));
                    if step.is_some() {
                        buttons.push((Button::ghost(tr!("tutorial.skip_step")), Click::Skip));
                        if run.lesson.id == TOUR.id && run.step == 0 && !run.done {
                            buttons.push((
                                Button::secondary(tr!("tutorial.import_sample")),
                                Click::ImportSample,
                            ));
                        }
                        if run.done {
                            let next = if run.step + 1 == steps.len() {
                                tr!("tutorial.finish")
                            } else {
                                tr!("tutorial.next")
                            };
                            buttons.push((Button::secondary(next), Click::Next));
                        }
                    } else {
                        buttons
                            .push((Button::secondary(tr!("tutorial.keep_project")), Click::Keep));
                    }
                    let (buttons, actions): (Vec<_>, Vec<_>) = buttons.into_iter().unzip();
                    let responses = suite_ui::dialog::action_footer(ui, buttons);
                    for (response, action) in responses.into_iter().zip(actions) {
                        if response.clicked() {
                            response.surrender_focus();
                            click = Some(action);
                        }
                    }
                });
        });
    click
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// A fresh temp folder for test `tag`.
    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jvid-tutorial-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn harness() -> Harness<'static, App> {
        harness_with(Harness::builder(), &Default::default())
    }

    fn harness_with(
        builder: egui_kittest::HarnessBuilder<App>,
        settings: &jvid_core::settings::Settings,
    ) -> Harness<'static, App> {
        let mut h = builder
            .with_size([1280.0, 720.0])
            .build_eframe(|cc| App::new(cc, settings));
        let state = h.state_mut();
        state.on_welcome = false;
        state.settings_path = None;
        state.untitled = None;
        h.run();
        h
    }

    fn step(h: &Harness<'_, App>) -> Option<usize> {
        h.state().lesson.as_ref().map(|r| r.step)
    }

    /// A short lesson on a title clip that tests the engine without sample media.
    const BASICS: Lesson = Lesson {
        id: "basics",
        title: || "Basics",
        minutes: 1,
        media: &[],
        setup: |_| {},
        steps: &[
            Step {
                anchor: "bin.titles",
                text: |_| String::new(),
                done: |a| {
                    a.history
                        .project()
                        .assets()
                        .iter()
                        .any(|a| a.text.is_some())
                },
                example: |a| {
                    let [(name, _, text), ..] = crate::media_bin::presets();
                    let name = name.into();
                    apply(a, Command::AddText { name, text });
                    // Where clicking a preset leaves the bin: the new title selected on the Media tab.
                    a.bin.selected = a.history.project().assets().last().map(|a| a.id);
                    (a.bin.tab, a.bin.query) = (0, String::new());
                },
                reveal: Some(|a| a.bin.tab = 2),
                ghost: None,
                also: &[],
            },
            Step {
                anchor: "timeline",
                text: |_| String::new(),
                done: |a| first_clip(a).is_some(),
                example: |a| {
                    let project = a.history.project();
                    let Some(asset) = project.assets().iter().find(|a| a.text.is_some()) else {
                        return;
                    };
                    if let Some(p) = project.placement(asset.id, Time(0), false, None, false) {
                        apply(a, Command::Place(p));
                    }
                },
                reveal: None,
                ghost: None,
                also: &[],
            },
            Step {
                anchor: "timeline",
                text: |_| String::new(),
                done: |a| !a.timeline.selected().is_empty(),
                example: |a| {
                    if let Some(id) = first_clip(a) {
                        a.timeline.select_clip(a.history.project().sequence(), id);
                    }
                },
                reveal: None,
                ghost: None,
                also: &[],
            },
            Step {
                anchor: "inspector",
                text: |_| String::new(),
                done: |a| {
                    let preset = &crate::media_bin::presets()[0].2.content;
                    let texts = a.history.project().assets().iter();
                    texts
                        .filter_map(|a| a.text.as_ref())
                        .any(|t| t.content != *preset)
                },
                example: |a| {
                    let project = a.history.project();
                    let Some(asset) = project.assets().iter().find(|a| a.text.is_some()) else {
                        return;
                    };
                    let mut text = asset.text.clone().unwrap_or_default();
                    text.content = "Edited".into();
                    apply(a, Command::SetText { id: asset.id, text });
                },
                reveal: None,
                ghost: None,
                also: &[],
            },
        ],
    };

    /// Prepared by `setup` with an extra video track; its first step points at the closed
    /// Render Queue and its second at nothing.
    const PREPARED: Lesson = Lesson {
        id: "prepared",
        title: || "Prepared",
        minutes: 1,
        media: &[],
        setup: |a| {
            let kind = jvid_core::model::TrackKind::Video;
            let name = String::new();
            apply(a, Command::AddTrack { kind, name });
        },
        steps: &[
            Step {
                anchor: "render_queue",
                text: |_| String::new(),
                done: |_| false,
                example: |_| {},
                reveal: None,
                ghost: None,
                also: &[],
            },
            Step {
                anchor: "nowhere",
                text: |_| String::new(),
                done: |_| false,
                example: |_| {},
                reveal: None,
                ghost: None,
                also: &[],
            },
        ],
    };

    /// Runs frames until the sandbox's media has imported and the card has caught up.
    /// Importing, exporting and playback repaint until they finish, so frames run without
    /// waiting for the UI to go idle.
    fn settle(h: &mut Harness<'_, App>) {
        for _ in 0..500 {
            h.run_ok();
            if !h.state().importer.is_busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        h.run_ok();
    }

    /// Where anchor `name` was drawn in the last frame run.
    fn drawn(h: &Harness<'_, App>, name: &str) -> Option<Rect> {
        let last = h.ctx.cumulative_pass_nr() - 1;
        h.ctx.data(|d| {
            let a = d.get_temp::<Anchors>(Id::NULL)?;
            (a.pass == last).then(|| a.rects.get(name).copied())?
        })
    }

    fn click(h: &mut Harness<'_, App>, label: &str) {
        h.get_by_label(label).click();
        h.run_ok();
        settle(h);
    }

    /// Clicks Next, or Finish on the last step.
    fn next(h: &mut Harness<'_, App>) {
        let label = match h.query_by_label(tr!("tutorial.finish")) {
            Some(_) => tr!("tutorial.finish"),
            None => tr!("tutorial.next"),
        };
        click(h, label);
    }

    /// Exercise a lesson's reference edit, allowing an already satisfied goal to be read.
    fn perform(h: &mut Harness<'_, App>) {
        let run = h.state().lesson.as_ref().unwrap();
        let at = run.step;
        let example = run.lesson.steps[at].example;
        example(h.state_mut());
        settle(h);
        if step(h) == Some(at) && h.state().lesson.as_ref().unwrap().done {
            next(h);
        }
    }

    /// Reference edits exercise all existing lessons, including async scopes and recognition.
    #[test]
    fn every_lesson_advances_on_its_reference_actions_and_reveals_anchors() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let dir = tmp("actions");
        for lesson in LESSONS.iter().chain([&BASICS]) {
            let mut h = harness_with(Harness::builder().wgpu(), &Default::default());
            h.state_mut().set_recognition_cache(Default::default());
            h.state_mut().export_dialog.set_path(dir.join("x.mp4"));
            h.state_mut().start_lesson(lesson);
            for (i, task) in lesson.steps.iter().enumerate() {
                settle(&mut h);
                assert_eq!(step(&h), Some(i), "{}: on task {i}", lesson.id);
                for anchor in [task.anchor].iter().chain(task.also) {
                    assert!(
                        drawn(&h, anchor).is_some()
                            || row_name(anchor).is_some() && drawn(&h, "inspector").is_some(),
                        "{} task {i}: `{anchor}` never drawn",
                        lesson.id
                    );
                }
                if task.anchor == "record" {
                    click(&mut h, tr!("tutorial.skip_step"));
                    continue;
                }
                h.state_mut().subtitles.engine = Some(fake());
                (task.example)(h.state_mut());
                for _ in 0..300 {
                    h.step();
                    h.render().unwrap();
                    if step(&h) != Some(i) {
                        break;
                    }
                    if h.state().lesson.as_ref().unwrap().done {
                        next(&mut h);
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                assert_eq!(
                    step(&h),
                    Some(i + 1),
                    "{}: task {i} did not advance",
                    lesson.id
                );
            }
            assert!(h.state().tutorials_done.iter().any(|d| d == lesson.id));
            click(&mut h, tr!("tutorial.exit"));
            assert!(h.state().lesson.is_none());
            assert!(!h.state().preview.playing());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The Welcome screen with the tour offered, as on first launch.
    fn first_launch(settings: &jvid_core::settings::Settings) -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1280.0, 720.0])
            .build_eframe(|cc| App::new(cc, settings));
        h.run();
        h
    }

    #[test]
    fn take_the_tour_starts_it_from_the_welcome_screen() {
        let mut h = first_launch(&Default::default());
        h.get_by_label(tr!("welcome.tour")).click();
        h.run();
        let state = h.state();
        assert!(!state.on_welcome);
        assert_eq!(state.lesson.as_ref().map(|r| r.lesson.id), Some(TOUR.id));
        assert!(!state.welcome.tour);
    }

    #[test]
    fn declining_the_tour_never_offers_it_again() {
        let dir = tmp("decline");
        let path = dir.join("settings.toml");
        let mut h = first_launch(&Default::default());
        h.state_mut().settings_path = Some(path.clone());
        h.get_by_label(tr!("welcome.no_tour")).click();
        h.run();
        assert!(h.query_by_label(tr!("welcome.tour")).is_none());
        assert!(
            h.state().on_welcome,
            "declining stays on the Welcome screen"
        );

        let saved = crate::settings_file::load_from(&path);
        assert!(saved.tour_offered);
        let h = first_launch(&saved);
        assert!(h.query_by_label(tr!("welcome.tour")).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn leaving_the_welcome_screen_any_way_ends_the_offer() {
        let dir = tmp("leave");
        let path = dir.join("settings.toml");
        let mut h = first_launch(&Default::default());
        h.state_mut().settings_path = Some(path.clone());
        h.get_by_label(tr!("menu.new_project")).click();
        h.run();
        h.run();
        assert!(crate::settings_file::load_from(&path).tour_offered);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// One step that passes once an export is queued.
    const EXPORT: Lesson = Lesson {
        id: "export",
        title: || "Export",
        minutes: 1,
        media: &[],
        setup: |_| {},
        steps: &[Step {
            anchor: "render_queue",
            text: |_| String::new(),
            done: |a| queued(a).next().is_some(),
            example: |a| fake_export(a),
            reveal: None,
            ghost: None,
            also: &[],
        }],
    };

    /// Lists an export in the Render Queue that never renders.
    fn fake_export(app: &mut App) {
        let (_tx, rx) = std::sync::mpsc::channel();
        let handle = app
            .exports
            .submit("fake", jvid_jobs::Priority::Interactive, |_| {});
        let path = PathBuf::from(format!("/m/{}.mp4", app.queue.pushed()));
        app.queue.push(path, String::new(), Time(1), (handle, rx));
    }

    #[test]
    fn an_export_queued_before_the_lesson_doesnt_pass_its_step() {
        let mut h = harness();
        fake_export(h.state_mut());
        h.state_mut().start_lesson(&EXPORT);
        settle(&mut h);
        assert_eq!(step(&h), Some(0));
        assert!(
            h.query_by_label(tr!("tutorial.finish")).is_none(),
            "the old export passed"
        );
        {
            let example = {
                let run = h.state().lesson.as_ref().unwrap();
                run.lesson.steps[run.step].example
            };
            example(h.state_mut());
            settle(&mut h);
        }
        assert_eq!(step(&h), Some(1));
    }

    #[test]
    fn exiting_a_tour_from_the_welcome_screen_shows_it_again() {
        let mut h = first_launch(&Default::default());
        h.state_mut().settings_path = None;
        h.get_by_label(tr!("welcome.tour")).click();
        h.run();
        click(&mut h, tr!("tutorial.exit"));
        assert!(h.state().lesson.is_none());
        assert!(h.state().on_welcome);
        assert!(h.query_by_label(tr!("welcome.tour")).is_none());
    }

    #[test]
    fn a_custom_curve_waits_for_the_graph_editor() {
        let mut h = harness();
        h.state_mut().start_lesson(&KEYFRAMES);
        settle(&mut h);
        perform(&mut h);
        perform(&mut h);
        // Custom in the diamond's easing menu passes the easing step but not the graph step.
        ease_first(h.state_mut(), Easing::Bezier([0.1, 0.0, 0.3, 1.0]));
        settle(&mut h);
        let done = |h: &Harness<'_, App>| h.state().lesson.as_ref().is_some_and(|r| r.done);
        assert_eq!(step(&h), Some(3));
        assert!(!done(&h));
        let id = first_clip(h.state()).unwrap();
        h.state_mut().timeline.open_graph(id);
        settle(&mut h);
        assert_eq!(step(&h), Some(4));
    }

    #[test]
    fn exiting_halfway_leaves_the_project_and_undo_history_as_before() {
        use jvid_core::model::TrackKind;
        let mut h = harness();
        let user = PathBuf::from("/m/mine.jvid");
        let add = |kind| Command::AddTrack {
            kind,
            name: String::new(),
        };
        let edit = |h: &mut History| {
            h.apply(add(TrackKind::Video)).unwrap();
            h.apply(add(TrackKind::Audio)).unwrap();
            h.undo();
        };
        let state = h.state_mut();
        edit(&mut state.history);
        state.path = Some(user.clone());
        let mut expect = History::new(Project::default());
        edit(&mut expect);
        let before = state.history.project().clone();

        state.start_lesson(&BASICS);
        settle(&mut h);
        perform(&mut h);
        assert_eq!(h.state().lesson.as_ref().map(|r| r.step), Some(1));
        assert_ne!(h.state().history.project(), &before);
        click(&mut h, tr!("tutorial.exit"));

        let state = h.state_mut();
        assert!(state.lesson.is_none());
        assert_eq!(state.path, Some(user));
        assert_eq!(state.history.project(), &before);
        assert_eq!(state.history.is_dirty(), expect.is_dirty());
        // The same redo, then the same two undos, and nothing past them.
        for step in [History::redo, History::undo, History::undo, History::undo] {
            assert_eq!(step(&mut state.history), step(&mut expect));
            assert_eq!(state.history.project(), expect.project());
        }
        assert!(!state.history.can_undo());
    }

    #[test]
    fn back_shows_a_done_step_and_skip_moves_on() {
        let mut h = harness();
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        perform(&mut h);
        assert_eq!(step(&h), Some(1));
        click(&mut h, tr!("tutorial.back"));
        assert_eq!(step(&h), Some(0), "a done step waits after Back");
        assert!(h.query_by_label(tr!("tutorial.skip_step")).is_some());
        next(&mut h);
        click(&mut h, tr!("tutorial.skip_step"));
        assert_eq!(step(&h), Some(2));
    }

    #[test]
    fn actual_editor_action_advances_without_next_and_unrelated_actions_do_not() {
        let mut h = harness();
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        assert!(h.query_by_label(tr!("tutorial.show_me")).is_none());
        assert!(h.query_by_label(tr!("tutorial.next")).is_none());
        h.state_mut().timeline.seek(SECOND);
        apply(
            h.state_mut(),
            Command::AddTrack {
                kind: TrackKind::Audio,
                name: String::new(),
            },
        );
        settle(&mut h);
        assert_eq!(step(&h), Some(0), "unrelated seek/track edit advanced");
        h.key_press(Key::Enter);
        settle(&mut h);
        assert_eq!(step(&h), Some(0), "Enter skipped an unfinished task");
        // Click the real title preset while guidance stays visible. It may also place a
        // title, in which case that already satisfied next task remains readable.
        click(&mut h, crate::media_bin::presets()[0].0);
        assert_eq!(step(&h), Some(1));
        assert!(h.state().history.can_undo());
    }

    #[test]
    fn keyboard_navigation_and_editor_shortcuts_still_work() {
        let mut h = harness();
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        perform(&mut h);
        click(&mut h, tr!("tutorial.back"));
        assert_eq!(step(&h), Some(0));
        assert!(h.ctx.memory(|m| m.focused()).is_none());
        h.key_press(Key::Enter);
        settle(&mut h);
        assert_eq!(step(&h), Some(1), "Enter acknowledges an already done task");
        // Keyboard activation returns focus to the editor too.
        h.get_by_label(tr!("tutorial.skip_step")).focus();
        settle(&mut h);
        h.key_press(Key::Space);
        settle(&mut h);
        assert_eq!(step(&h), Some(2));
        assert!(h.ctx.memory(|m| m.focused()).is_none());
        h.key_press(Key::Escape);
        settle(&mut h);
        assert!(h.state().lesson.is_none());
    }

    #[test]
    fn a_hidden_anchor_is_revealed() {
        let mut h = harness();
        h.state_mut().bin.tab = 3;
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        assert_eq!(h.state().bin.tab, 2);
        assert!(drawn(&h, "bin.titles").is_some());
        click(&mut h, tr!("tutorial.exit"));
        assert_eq!(h.state().bin.tab, 3, "the bin's tab is put back");
    }

    #[test]
    fn help_tutorials_starts_a_lesson_and_progress_saves_to_settings() {
        let dir = std::env::temp_dir().join(format!("jvid-tutorials-{}", std::process::id()));
        let path = dir.join("settings.toml");
        let mut h = harness();
        h.state_mut().settings_path = Some(path.clone());
        h.get_by_label(tr!("menu.help")).click();
        h.run();
        h.get_by_label_contains(tr!("menu.tutorials")).click();
        h.run();
        let title = (LESSONS[0].title)();
        h.get_by_label_contains(title).click();
        h.run();
        assert_eq!(
            h.state().lesson.as_ref().map(|r| r.lesson.id),
            Some(LESSONS[0].id)
        );
        settle(&mut h);
        perform(&mut h);
        for _ in &LESSONS[0].steps[1..] {
            click(&mut h, tr!("tutorial.skip_step"));
        }
        let saved = crate::settings_file::load_from(&path);
        assert_eq!(saved.tutorials_done, vec![LESSONS[0].id.to_string()]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn skipping_every_step_isnt_finishing() {
        let mut h = harness();
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        for _ in BASICS.steps {
            click(&mut h, tr!("tutorial.skip_step"));
        }
        assert_eq!(step(&h), Some(BASICS.steps.len()));
        assert!(h.state().tutorials_done.is_empty());
    }

    #[test]
    fn saving_in_a_lesson_keeps_the_untitled_autosave() {
        let dir = tmp("untitled");
        let untitled = dir.join("Untitled.jvid");
        let auto = crate::autosave::slot(&untitled, 1);
        std::fs::write(&auto, "the user's work").unwrap();
        let mut h = harness();
        h.state_mut().untitled = Some(untitled);
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        {
            let example = {
                let run = h.state().lesson.as_ref().unwrap();
                run.lesson.steps[run.step].example
            };
            example(h.state_mut());
            settle(&mut h);
        }
        // Where Keep Project and Ctrl+S both land once the save dialog picks a file.
        assert!(h.state_mut().save_to(dir.join("kept.jvid")));
        assert!(auto.exists(), "Keep Project deleted the untitled autosave");
        h.key_press_modifiers(Modifiers::COMMAND, Key::S);
        h.run();
        assert!(auto.exists(), "Ctrl+S deleted the untitled autosave");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn starting_a_lesson_writes_the_pending_autosave() {
        let dir = tmp("flush");
        let user = dir.join("mine.jvid");
        let mut h = harness();
        let state = h.state_mut();
        state.path = Some(user.clone());
        let kind = jvid_core::model::TrackKind::Audio;
        let name = String::new();
        state
            .history
            .apply(Command::AddTrack { kind, name })
            .unwrap();
        let before = state.history.project().clone();
        h.run();
        h.state_mut().start_lesson(&BASICS);
        let saved = crate::project_file::load(&crate::autosave::slot(&user, 1)).unwrap();
        assert_eq!(saved, before);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_selection_made_in_a_lesson_doesnt_survive_exit() {
        let mut h = harness();
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        for _ in 0..3 {
            perform(&mut h);
        }
        assert!(!h.state().timeline.selected().is_empty());
        assert!(h.state().bin.selected.is_some());
        click(&mut h, tr!("tutorial.exit"));
        assert!(h.state().timeline.selected().is_empty());
        assert_eq!(h.state().bin.selected, None);
    }

    #[test]
    fn reference_edit_selects_the_new_title_like_a_click() {
        let mut h = harness();
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        {
            let example = {
                let run = h.state().lesson.as_ref().unwrap();
                run.lesson.steps[run.step].example
            };
            example(h.state_mut());
            settle(&mut h);
        }
        let state = h.state();
        let title = state
            .history
            .project()
            .assets()
            .iter()
            .find(|a| a.text.is_some());
        assert_eq!(state.bin.selected, title.map(|a| a.id));
        assert_eq!(state.bin.tab, 0);
    }

    #[test]
    fn back_reveals_again() {
        let mut h = harness();
        h.state_mut().bin.tab = 3;
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        assert_eq!(h.state().bin.tab, 2);
        perform(&mut h);
        assert_eq!(step(&h), Some(1));
        h.state_mut().bin.tab = 3;
        click(&mut h, tr!("tutorial.back"));
        assert_eq!(step(&h), Some(0));
        assert_eq!(
            h.state().bin.tab,
            2,
            "Back to step 0 reveals the Text tab again"
        );
    }

    #[test]
    fn setup_prepares_a_project_that_cant_be_undone() {
        let mut h = harness();
        let tracks = |h: &Harness<'_, App>| h.state().history.project().sequence().tracks().len();
        h.state_mut().start_lesson(&PREPARED);
        settle(&mut h);
        let empty = History::new(Project::default());
        assert_eq!(tracks(&h), empty.project().sequence().tracks().len() + 1);
        assert!(!h.state().history.can_undo());
        assert!(!h.state().history.is_dirty());
    }

    #[test]
    fn the_audio_lesson_starts_with_the_voice_over_the_music_and_nothing_ducked() {
        use audio::{AUDIO, MUSIC, VOICE, clip_of, ducked};
        let mut h = harness();
        h.state_mut().start_lesson(&AUDIO);
        settle(&mut h);
        let (voice, music) = (clip_of(h.state(), VOICE), clip_of(h.state(), MUSIC));
        let (Some(voice), Some(music)) = (voice, music) else {
            panic!("both samples placed")
        };
        let seq = h.state().history.project().sequence();
        let track = |id| {
            seq.tracks()
                .iter()
                .position(|t| t.clips().iter().any(|c| c.id == id))
        };
        assert_ne!(track(voice.id), track(music.id));
        assert_eq!(voice.start, Time(2 * jvid_core::time::FLICKS_PER_SECOND));
        assert_eq!(music.start, Time(0));
        assert!(!h.state().history.can_undo());

        // Level keys don't duck; a dip does.
        use jvid_core::keyframe::{Easing, Keyframe, Value};
        let key = |t, v| Keyframe {
            time: Time(t),
            value: Value::Scalar(v),
            easing: Easing::Linear,
        };
        let mut c = music.clone();
        assert!(!ducked(&c));
        c.keyframes
            .insert("volume".into(), vec![key(0, 1.0), key(1, 1.0)]);
        assert!(!ducked(&c));
        c.keyframes
            .insert("volume".into(), vec![key(0, 1.0), key(1, 0.3)]);
        assert!(ducked(&c));
    }

    /// The Fast model, as the fake transcriber of the Generate Subtitles tests.
    fn fake() -> (
        jvid_transcribe::models::Model,
        std::sync::Arc<dyn jvid_transcribe::Transcriber>,
    ) {
        let fake = crate::app::tests::FakeTranscriber {
            slow: false,
            asked: Default::default(),
        };
        (
            jvid_transcribe::models::Model::Base,
            std::sync::Arc::new(fake),
        )
    }

    /// Acceptance: reference actions run the whole lesson on a fake transcriber, without a GPU, a model
    /// or the network.
    #[test]
    fn the_subtitles_lesson_advances_with_a_fake_transcriber() {
        use subtitles::{SUBTITLES, cut};
        let fake = std::sync::Arc::new(crate::app::tests::FakeTranscriber {
            slow: false,
            asked: Default::default(),
        });
        let mut h = harness();
        h.state_mut().set_recognition_cache(Default::default());
        h.state_mut().start_lesson(&SUBTITLES);
        settle(&mut h);
        assert!(!h.state().history.can_undo());
        assert!(!cut(h.state()), "the whole voice plays at the start");
        for (i, s) in SUBTITLES.steps.iter().enumerate() {
            assert_eq!(step(&h), Some(i));
            assert!(!(s.done)(h.state()), "step {i} done before the action");
            h.state_mut().subtitles.engine =
                Some((jvid_transcribe::models::Model::Base, fake.clone()));
            (s.example)(h.state_mut());
            let start = std::time::Instant::now();
            while !(s.done)(h.state()) {
                assert!(
                    start.elapsed().as_secs() < 30,
                    "step {i}: action did not pass"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
                h.step();
            }
            settle(&mut h);
            if step(&h) == Some(i) {
                next(&mut h);
            }
            assert_eq!(step(&h), Some(i + 1));
        }
        assert_eq!(step(&h), Some(SUBTITLES.steps.len()));
        assert!(h.state().tutorials_done.iter().any(|d| d == SUBTITLES.id));
        let project = h.state().history.project();
        let cues: Vec<String> = project
            .subtitle_cues()
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(cues, ["hello there.", "This is a test."]);
        assert!(cut(h.state()));
        assert_eq!(
            fake.asked.lock().unwrap().len(),
            1,
            "Transcript and its cut must reuse the subtitle recognition"
        );
    }

    /// Without a model, the action opens Generate Subtitles on its download choice and starts
    /// nothing; Cancel and Skip Step move on offline.
    #[test]
    fn without_a_model_the_subtitles_lesson_waits_on_the_download_choice() {
        use jvid_transcribe::models::{self, Model};
        // A model on this machine would load for real.
        if models::dir().is_some_and(|d| Model::Base.is_downloaded(&d)) {
            return;
        }
        let mut h = harness();
        h.state_mut().set_recognition_cache(Default::default());
        h.state_mut().start_lesson(&subtitles::SUBTITLES);
        settle(&mut h);
        {
            let example = {
                let run = h.state().lesson.as_ref().unwrap();
                run.lesson.steps[run.step].example
            };
            example(h.state_mut());
            settle(&mut h);
        }
        assert!(h.state().subtitles.is_open());
        let size = (Model::Base.size() as f64 / 1e6).round().to_string();
        let fast = tr!(
            "subtitles.with_size",
            name = tr!("subtitles.fast"),
            size = size
        );
        assert!(h.query_by_label(&fast).is_some(), "the model's size shows");
        assert!(h.query_by_label(tr!("subtitles.downloading")).is_none());
        click(&mut h, tr!("prompt.cancel"));
        assert!(!h.state().subtitles.is_open());
        click(&mut h, tr!("tutorial.skip_step"));
        assert_eq!(step(&h), Some(1));
    }

    #[test]
    fn the_render_queue_reveals_itself_and_a_missing_anchor_is_logged() {
        let mut h = harness();
        assert!(h.state().dock.find_tab(&Tab::RenderQueue).is_none());
        h.state_mut().start_lesson(&PREPARED);
        settle(&mut h);
        assert!(drawn(&h, "render_queue").is_some());
        let warned = |h: &Harness<'_, App>| h.state().lesson.as_ref().is_some_and(|r| r.warned);
        assert!(!warned(&h));
        click(&mut h, tr!("tutorial.skip_step"));
        assert!(warned(&h), "the missing `nowhere` anchor is logged");
    }

    #[test]
    fn a_lesson_activates_an_existing_inactive_dock_tab() {
        let mut h = harness();
        let app = h.state_mut();
        app.show_queue();
        let queue = app.dock.find_tab(&Tab::RenderQueue).unwrap();
        let timeline = app.dock.find_tab(&Tab::Timeline).unwrap().node_path();
        app.dock
            .move_tab(queue, (timeline, egui_dock::TabInsert::Append));
        app.focus_panel(Tab::Timeline);
        app.start_lesson(&PREPARED);
        settle(&mut h);
        assert!(drawn(&h, "render_queue").is_some());
        let dock = &h.state().dock;
        assert_eq!(
            dock.iter_all_tabs()
                .filter(|(_, t)| **t == Tab::RenderQueue)
                .count(),
            1
        );
        assert_eq!(
            dock.find_tab(&Tab::RenderQueue).unwrap().node_path(),
            dock.find_tab(&Tab::Timeline).unwrap().node_path(),
        );
    }

    #[test]
    fn a_background_over_the_green_screen_isnt_under_it() {
        use jvid_core::history::Created;
        let mut h = harness();
        h.state_mut().start_lesson(&GREEN_SCREEN);
        settle(&mut h);
        let a = h.state_mut();
        let under = GREEN_SCREEN.steps[0].done;
        assert!(!under(a));
        let (kind, name) = (jvid_core::model::TrackKind::Video, String::new());
        let Ok(Created::Track(top)) = a.history.apply(Command::AddTrack { kind, name }) else {
            panic!("no track");
        };
        let bg = clip_named(a, BACKGROUND).unwrap().id;
        let p = a
            .history
            .project()
            .placement(bg, Time(0), false, Some(top), false);
        apply(a, Command::Place(p.unwrap()));
        assert!(!under(a), "a background above the key passed");
        (GREEN_SCREEN.steps[0].example)(a);
        assert!(under(a));
    }

    #[test]
    fn a_sample_that_cant_be_found_starts_nothing() {
        const BROKEN: Lesson = Lesson {
            media: &["not-a-sample.mp4"],
            ..PREPARED
        };
        let mut h = harness();
        h.state_mut().start_lesson(&BROKEN);
        assert!(h.state().lesson.is_none());
        assert!(matches!(
            h.state().prompts.as_slice(),
            [super::super::Prompt::Error(_)]
        ));
    }

    #[test]
    fn previous_session_dialogs_do_not_block_esc_or_exit() {
        let mut h = harness();
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        h.key_press(Key::Escape);
        h.run();
        assert!(h.state().lesson.is_none(), "Esc exits");

        // A draft from the previous session must be cleared on sandbox entry. It cannot
        // apply to the sandbox or block its Exit/Escape controls.
        let under_settings = |h: &mut Harness<'_, App>| {
            h.state_mut().end_lesson();
            let state = h.state_mut();
            state.settings_dialog.open(state.history.project());
            h.run();
            h.state_mut().start_lesson(&BASICS);
            settle(h);
        };
        under_settings(&mut h);
        h.key_press(Key::Escape);
        h.run();
        assert!(
            h.state().lesson.is_none(),
            "the previous session's dialog was cleared, so Esc exits the lesson"
        );
        under_settings(&mut h);
        click(&mut h, tr!("tutorial.exit"));
        assert!(
            h.state().lesson.is_none(),
            "the previous session's dialog cannot block Exit"
        );
    }

    #[test]
    fn a_steps_ghost_shows_only_while_it_does() {
        const GHOSTED: Lesson = Lesson {
            steps: &[Step {
                anchor: "preview",
                text: |_| String::new(),
                done: |_| false,
                example: |_| {},
                reveal: None,
                ghost: Some([[10.0, 20.0], [300.0, 400.0]]),
                also: &[],
            }],
            ..PREPARED
        };
        let shown = |h: &Harness<'_, App>| {
            let g = h.ctx.data(|d| d.get_temp::<[[f64; 2]; 2]>(Id::new(GHOST)));
            g.filter(|_| running(&h.ctx))
        };
        let mut h = harness();
        h.state_mut().start_lesson(&GHOSTED);
        settle(&mut h);
        assert_eq!(shown(&h), Some([[10.0, 20.0], [300.0, 400.0]]));
        click(&mut h, tr!("tutorial.skip_step"));
        assert_eq!(shown(&h), None, "the finished card has no ghost");
        h.state_mut().start_lesson(&GHOSTED);
        settle(&mut h);
        click(&mut h, tr!("tutorial.exit"));
        assert_eq!(shown(&h), None, "exiting hides it");
    }

    #[test]
    fn no_anchors_are_recorded_without_a_lesson() {
        let h = harness();
        assert!(h.ctx.data(|d| d.get_temp::<Anchors>(Id::NULL)).is_none());
    }

    /// The reference edit cuts `DEAD_AIR` without waiting on peaks, so it has to be what Detect Silence
    /// finds in the sample at its defaults.
    #[test]
    fn dead_air_is_what_detect_silence_finds_in_the_sample() {
        use jvid_media::audio::SAMPLE_RATE;
        let dir = tmp("peaks");
        let path = sample(SCREEN_CLIP).unwrap();
        let peaks = jvid_media::waveforms::Cache::new(dir.clone(), 1 << 20)
            .load(&path, 0)
            .unwrap();
        let rate = SAMPLE_RATE as f64;
        let n = |secs: f64| (secs * rate) as i64;
        let found = jvid_media::silence::detect(peaks.finest(), -40.0, n(1.0), n(0.15));
        let [(a, b)] = found[..] else {
            panic!("{found:?}")
        };
        let frame = 1.0 / 30.0;
        assert!((a as f64 / rate - (DEAD_AIR.0 + 0.15)).abs() < frame, "{a}");
        assert!((b as f64 / rate - (DEAD_AIR.1 - 0.15)).abs() < frame, "{b}");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn card_bounds(h: &Harness<'_, App>) -> Rect {
        h.ctx.memory(|m| m.area_rect(Id::new(CARD_ID))).unwrap()
    }

    fn screen_lesson() -> Harness<'static, App> {
        let mut h = harness();
        h.set_size(vec2(960.0, 600.0));
        h.state_mut().start_lesson(&SCREEN);
        settle(&mut h);
        h
    }

    fn select_screen(h: &mut Harness<'_, App>) {
        let a = h.state_mut();
        let id = first_clip(a).unwrap();
        a.timeline.select_clip(a.history.project().sequence(), id);
        settle(h);
    }

    fn open_silence(h: &mut Harness<'_, App>) {
        select_screen(h);
        h.state_mut().run_action("detect_silence", true);
        settle(h);
        assert!(drawn(h, "silence.dialog").is_some());
    }

    #[test]
    fn guidance_highlights_automatically_without_dimming_or_blocking_the_editor() {
        let mut h = harness();
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        let target = drawn(&h, "bin.titles")
            .unwrap()
            .expand(2.0)
            .intersect(h.ctx.content_rect());
        assert!(h.output().shapes.iter().any(|s| matches!(&s.shape,
            egui::Shape::Rect(r) if r.rect == target && r.fill == egui::Color32::TRANSPARENT
                && r.stroke == Stroke::new(2.0, color::light_blue()))));
        assert!(!h.output().shapes.iter().any(|s| matches!(&s.shape,
            egui::Shape::Rect(r) if r.fill == color::ground().gamma_multiply(0.8))));
        let before = card_bounds(&h);
        h.run_ok();
        assert_eq!(card_bounds(&h), before, "idle guidance moved");
        click(&mut h, crate::media_bin::presets()[0].0);
        assert_eq!(
            step(&h),
            Some(1),
            "guidance blocked the actual title button"
        );
    }

    #[test]
    fn silence_guidance_tracks_selection_dialog_and_dismissal_at_960_by_600() {
        let mut h = screen_lesson();
        assert!(h.query_by_label(tr!("tutorial.silence_select")).is_some());
        select_screen(&mut h);
        let open_text = tr!(
            "tutorial.silence_open",
            key = h.state().keymap.shortcut("detect_silence")
        );
        assert!(h.query_by_label(&open_text).is_some());
        open_silence(&mut h);
        assert!(h.query_by_label(tr!("tutorial.silence_cut")).is_some());
        assert!(h.query_by_label(&open_text).is_none());
        let dialog = drawn(&h, "silence.dialog").unwrap();
        let card = card_bounds(&h);
        assert!(
            h.ctx.content_rect().contains_rect(card),
            "card outside viewport: {card:?}"
        );
        assert!(
            !card.intersects(dialog),
            "card {card:?} covers dialog {dialog:?}"
        );
        assert!(!card.intersects(drawn(&h, "silence.cut").unwrap()));
        assert_eq!(step(&h), Some(0), "opening the dialog completed a cut");
        h.key_press(Key::Escape);
        settle(&mut h);
        assert_eq!(
            step(&h),
            Some(0),
            "Escape exited rather than dismissing the dialog"
        );
        assert!(!h.state().timeline.dialog_open());
        assert!(h.query_by_label(&open_text).is_some());
        open_silence(&mut h);
        click(&mut h, tr!("prompt.cancel"));
        assert_eq!(step(&h), Some(0));
        assert!(h.query_by_label(&open_text).is_some());
    }

    #[test]
    fn silence_progress_requires_the_successful_cut_action() {
        let mut h = screen_lesson();
        let a = h.state_mut();
        let id = first_clip(a).unwrap();
        a.history
            .apply(Command::SetClipSpeed {
                id,
                speed: 2.0,
                reverse: false,
                ripple: true,
            })
            .unwrap();
        settle(&mut h);
        assert!(shortened(h.state(), SCREEN_CLIP));
        assert_eq!(
            step(&h),
            Some(0),
            "an unrelated speed change completed Detect Silence"
        );
        h.state_mut().history.undo();
        open_silence(&mut h);
        let started = std::time::Instant::now();
        while h.query_by_label(tr!("silence.reading")).is_some() {
            assert!(
                started.elapsed().as_secs() < 15,
                "sample peaks never loaded"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
            h.step();
        }
        click(&mut h, tr!("silence.cut"));
        assert_eq!(step(&h), Some(1), "Cut did not automatically advance");
        assert!(shortened(h.state(), SCREEN_CLIP));
        assert!(
            h.state_mut().history.undo(),
            "the actual cut remains undoable"
        );
        assert!(!shortened(h.state(), SCREEN_CLIP));
        assert!(h.state_mut().history.redo());
    }

    #[test]
    fn skip_back_and_exit_remain_available_with_the_task_dialog_open() {
        let mut h = screen_lesson();
        open_silence(&mut h);
        click(&mut h, tr!("tutorial.skip_step"));
        assert_eq!(step(&h), Some(1));
        assert!(
            !h.state().timeline.dialog_open(),
            "Skip left stale task controls open"
        );
        click(&mut h, tr!("tutorial.back"));
        assert_eq!(step(&h), Some(0));
        open_silence(&mut h);
        click(&mut h, tr!("tutorial.exit"));
        assert!(h.state().lesson.is_none());
        assert!(!h.state().timeline.dialog_open());
        assert!(h.state().tutorials_done.is_empty());
    }

    #[test]
    fn modal_guidance_leaves_export_footer_clear_and_skip_is_reachable() {
        let mut h = harness();
        h.set_size(vec2(960.0, 600.0));
        h.state_mut().start_lesson(&KEYFRAMES);
        settle(&mut h);
        // The export lesson uses the title prepared above, without requiring a GPU.
        h.state_mut().lesson.as_mut().unwrap().go(0);
        let seq = h.state().history.project().sequence().clone();
        h.state_mut().export_dialog.open(&seq, "tutorial");
        settle(&mut h);
        let dialog = drawn(&h, "export.dialog").unwrap();
        let card = card_bounds(&h);
        assert!(h.ctx.content_rect().contains_rect(card));
        assert!(
            !card.intersects(dialog),
            "card {card:?} covers modal {dialog:?}"
        );
        click(&mut h, tr!("tutorial.skip_step"));
        assert_eq!(step(&h), Some(1), "modal blocked Skip");
        // An unrelated dialog is dismissed by Escape, with the lesson still running.
        h.key_press(Key::Escape);
        settle(&mut h);
        assert!(h.state().lesson.is_some());
        assert!(!h.state().export_dialog.is_open());
    }

    #[test]
    fn subtitle_dialog_updates_guidance_and_keyboard_exit_works_above_the_modal() {
        let mut h = harness();
        h.set_size(vec2(960.0, 600.0));
        h.state_mut().start_lesson(&subtitles::SUBTITLES);
        settle(&mut h);
        h.state_mut().run_action("generate_subtitles", true);
        settle(&mut h);
        let text = tr!(
            "tutorial.speech_settings",
            action = tr!("subtitles.generate")
        );
        assert!(h.query_by_label(&text).is_some());
        assert_eq!(
            step(&h),
            Some(0),
            "opening a model choice completed generation"
        );
        assert!(!card_bounds(&h).intersects(drawn(&h, "subtitles.dialog").unwrap()));
        h.get_by_label(tr!("tutorial.exit")).focus();
        settle(&mut h);
        h.key_press(Key::Enter);
        settle(&mut h);
        assert!(
            h.state().lesson.is_none(),
            "keyboard Exit was trapped behind the modal"
        );
        assert!(!h.state().subtitles.is_open());
    }

    #[test]
    fn minimum_layout_renders_guidance_beside_the_open_task_dialog() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut h = harness_with(Harness::builder().wgpu(), &Default::default());
        h.set_size(vec2(960.0, 600.0));
        h.state_mut().start_lesson(&SCREEN);
        settle(&mut h);
        open_silence(&mut h);
        let dialog = drawn(&h, "silence.dialog").unwrap();
        assert!(!card_bounds(&h).intersects(dialog));
        let image = h.render().unwrap();
        assert_eq!(image.dimensions(), (960, 600));
        if let Some(dir) = std::env::var_os("JVID_TUTORIAL_CAPTURE") {
            std::fs::create_dir_all(&dir).unwrap();
            image
                .save(PathBuf::from(dir).join("tutorial-silence-960x600.png"))
                .unwrap();
        }
        click(&mut h, tr!("tutorial.exit"));
    }

    #[test]
    fn already_satisfied_and_no_action_steps_remain_readable() {
        const READ: Lesson = Lesson {
            steps: &[
                Step {
                    anchor: "timeline",
                    text: |_| String::new(),
                    done: |_| true,
                    example: |_| {},
                    reveal: None,
                    ghost: None,
                    also: &[],
                },
                Step {
                    anchor: "preview",
                    text: |_| String::new(),
                    done: |_| false,
                    example: |_| {},
                    reveal: None,
                    ghost: None,
                    also: &[],
                },
            ],
            ..PREPARED
        };
        let mut h = harness();
        h.state_mut().start_lesson(&READ);
        settle(&mut h);
        assert_eq!(step(&h), Some(0));
        h.key_press(Key::Enter);
        settle(&mut h);
        assert_eq!(step(&h), Some(1));
        h.state_mut().timeline.seek(SECOND);
        settle(&mut h);
        assert_eq!(step(&h), Some(1));
        click(&mut h, tr!("tutorial.skip_step"));
        assert_eq!(step(&h), Some(2));
    }

    #[test]
    fn restarting_a_completed_lesson_resets_the_sandbox_and_step_baseline() {
        let mut h = harness();
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        for _ in BASICS.steps {
            perform(&mut h);
        }
        assert!(h.state().tutorials_done.iter().any(|d| d == BASICS.id));
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        assert_eq!(step(&h), Some(0));
        assert!(h.state().history.project().assets().is_empty());
        assert!(h.query_by_label(tr!("tutorial.next")).is_none());
        perform(&mut h);
        assert_eq!(step(&h), Some(1));
    }

    #[test]
    fn card_placement_avoids_controls_dialogs_and_focused_fields() {
        for size in [vec2(960.0, 600.0), vec2(1280.0, 720.0)] {
            let screen = Rect::from_min_size(pos2(0.0, 0.0), size);
            let dialog = Align2::CENTER_CENTER.align_size_within_rect(vec2(480.0, 530.0), screen);
            let control = Rect::from_min_size(pos2(10.0, 12.0), vec2(160.0, 24.0));
            let card = card_rect(screen, vec2(CARD_W, 160.0), &[dialog, control], None);
            assert!(screen.contains_rect(card));
            assert!(!card.intersects(dialog));
            assert!(!card.intersects(control));
            assert_eq!(
                card_rect(screen, card.size(), &[dialog, control], Some(card)),
                card
            );
        }
    }

    #[test]
    fn viewer_shading_excludes_each_hole() {
        let screen = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0));
        let a = Rect::from_min_max(pos2(10.0, 10.0), pos2(30.0, 60.0));
        let b = Rect::from_min_max(pos2(50.0, 40.0), pos2(90.0, 80.0));
        let shade = shaded(screen, &[a, b]);
        let area: f32 = shade.iter().map(|r| r.area()).sum();
        assert_eq!(area, screen.area() - a.area() - b.area());
        let shaded_at = |p| shade.iter().any(|r| r.contains(p));
        for p in [pos2(20.0, 30.0), pos2(70.0, 50.0), pos2(70.0, 70.0)] {
            assert!(!shaded_at(p), "{p:?} in a hole");
        }
        for p in [
            pos2(5.0, 5.0),
            pos2(40.0, 50.0),
            pos2(20.0, 70.0),
            pos2(95.0, 95.0),
        ] {
            assert!(shaded_at(p), "{p:?} not shaded");
        }
    }
}
