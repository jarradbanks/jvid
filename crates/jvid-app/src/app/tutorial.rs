//! Interactive lessons in Help > Tutorials. A lesson is a list of steps, each pointing at an
//! [`anchor`] the editor draws, checked against the project or UI state, and able to do itself
//! (Show Me) through the normal undoable edits. It runs in a sandbox project; the user's
//! project, undo history and path wait in a [`Stash`] until the lesson ends.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use egui::{
    Align, Align2, CornerRadius, Id, Key, LayerId, Layout, Modifiers, Order, Rect, Stroke, pos2,
    vec2,
};
use jvid_core::history::{Command, History};
use jvid_core::keyframe::{Easing, Keyframe, Value};
use jvid_core::model::{AssetId, Project, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use suite_ui::theme;
use suite_ui::tokens::{color, radius, space};
use suite_ui::widgets::{Button, State};

use super::{App, Tab};
use crate::i18n::{tr, verbatim};
use crate::import::Importer;
use crate::timeline::Timeline;

mod audio;
mod colour;
mod markers;
mod masks;

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
    /// The [`anchor`] the step points at.
    pub anchor: &'static str,
    /// The instruction, one or two short sentences. Shortcuts come from `app.keymap`, so a
    /// remapped key reads right.
    pub text: fn(&App) -> String,
    /// Whether the step's goal holds in the project or UI. Checked every frame; once it passes,
    /// the lesson moves on.
    pub done: fn(&App) -> bool,
    /// Does the step the way the user would.
    pub show_me: fn(&mut App),
    /// Brings the anchor into view when it isn't drawn, such as by switching tabs. The Render
    /// Queue reveals itself.
    pub reveal: Option<fn(&mut App)>,
    /// A target ellipse, its centre and size in project pixels from the frame's centre,
    /// drawn dashed on the Program view while the step shows.
    pub ghost: Option<[[f64; 2]; 2]>,
    /// More anchors left undimmed beside `anchor`, such as a scope to read while editing in
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
            show_me: |a| match sample(TOUR_CLIP) {
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
            show_me: |a| {
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
            also: &[],
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
            show_me: |a| {
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
            show_me: mark,
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "timeline",
            text: |a| tr!("tutorial.tour_cut", key = a.keymap.shortcut("extract")),
            done: |a| shortened(a, TOUR_CLIP),
            show_me: |a| {
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
            show_me: add_title,
            reveal: Some(|a| a.bin.tab = 2),
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "export",
            text: |_| tr!("tutorial.tour_export").into(),
            done: |a| queued(a).next().is_some(),
            show_me: |a| {
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
            done: |a| shortened(a, SCREEN_CLIP),
            show_me: |a| {
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
            anchor: "inspector",
            text: |_| tr!("tutorial.screen_zoom").into(),
            done: zoomed,
            show_me: |a| {
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
            also: &[],
        },
        Step {
            anchor: "bin.titles",
            text: |_| tr!("tutorial.screen_callout").into(),
            done: has_title,
            show_me: add_title,
            reveal: Some(|a| a.bin.tab = 2),
            ghost: None,
            also: &[],
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
            show_me: |a| {
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
            also: &[],
        },
        Step {
            anchor: "inspector",
            text: |_| tr!("tutorial.green_screen_key").into(),
            done: |a| key_index(a).is_some(),
            show_me: |a| {
                if let Some((_, id)) = keyed_clip(a) {
                    a.timeline.select_clip(a.history.project().sequence(), id);
                }
                add_key(a);
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "inspector",
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
            show_me: |a| {
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
            anchor: "inspector",
            text: |_| tr!("tutorial.green_screen_mask").into(),
            done: |a| {
                let Some((_, id)) = keyed_clip(a) else {
                    return false;
                };
                let seq = a.history.project().sequence();
                seq.clip(id)
                    .is_some_and(|(_, c)| c.transform.masks.iter().any(|m| m.enabled))
            },
            show_me: |a| {
                use jvid_core::model::{Mask, MaskShape};
                // Keeps the left 65% of the frame, which ends just before the board.
                edit_keyed(a, |[w, h], x| {
                    let points = vec![[-0.2 * w, 0.0], [0.7 * w, 1.1 * h]];
                    x.masks.push(Mask::new(MaskShape::Rectangle, points));
                });
            },
            reveal: None,
            ghost: None,
            also: &[],
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
            anchor: "inspector",
            text: |_| tr!("tutorial.keys_add").into(),
            done: |a| {
                title(a).is_some_and(|c| {
                    let n = |p| c.keyframes.get(p).map_or(0, Vec::len);
                    n("position") >= 2 && n("opacity") >= 2
                })
            },
            show_me: |a| {
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
            also: &[],
        },
        Step {
            anchor: "inspector",
            text: |_| tr!("tutorial.keys_previous").into(),
            done: |a| first_key(a).is_some_and(|t| a.timeline.playhead() == t),
            show_me: |a| {
                if let Some(t) = first_key(a) {
                    a.timeline.seek(t);
                }
            },
            reveal: None,
            ghost: None,
            also: &[],
        },
        Step {
            anchor: "inspector",
            text: |_| tr!("tutorial.keys_ease").into(),
            done: |a| {
                let eased = |e| e != Easing::Linear;
                title_keys(a).any(|k| eased(k.easing))
            },
            show_me: |a| ease_first(a, Easing::EaseOut),
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
            show_me: |a| {
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
            show_me: |a| {
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
    if let Err(e) = app.history.apply(c) {
        tracing::warn!("lesson edit failed: {e}");
    }
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
    /// A passing check moves on. Off after Back, so the step stays until its check fails.
    armed: bool,
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
        (self.revealed, self.warned) = (false, false);
    }
}

struct Stash {
    history: History,
    path: Option<PathBuf>,
    missing: HashSet<PathBuf>,
    match_source: bool,
    importer: Importer,
    /// The playhead, selection and clipboard: clip ids repeat across projects.
    timeline: Timeline,
    bin_tab: usize,
    bin_selected: Option<AssetId>,
    /// Started from the Welcome screen, so exit goes back there.
    on_welcome: bool,
}

#[derive(Clone, Copy)]
enum Click {
    Back,
    ShowMe,
    Skip,
    Keep,
    Exit,
}

impl App {
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
        self.leave_project();
        // Autosave is off in a lesson, so the edits it hasn't written yet are written now.
        if let Some(target) = self.autosave_target() {
            self.autosave.flush(&self.history, &target);
        }
        let (importer, timeline) = (self.importer.sibling(), self.timeline.sibling());
        let stash = Stash {
            history: std::mem::replace(&mut self.history, History::new(Project::default())),
            path: self.path.take(),
            missing: std::mem::take(&mut self.missing),
            match_source: std::mem::replace(&mut self.match_source, true),
            importer: std::mem::replace(&mut self.importer, importer),
            timeline: std::mem::replace(&mut self.timeline, timeline),
            bin_tab: self.bin.tab,
            bin_selected: self.bin.selected.take(),
            on_welcome: std::mem::take(&mut self.on_welcome),
        };
        self.import(media);
        self.lesson = Some(Run {
            lesson,
            step: 0,
            armed: true,
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
        self.leave_project();
        // The lesson's playback would run on into the user's timeline.
        if self.preview.playing() {
            self.preview.program_action("shuttle_stop");
        }
        let s = run.stash;
        self.history = s.history;
        self.path = s.path;
        self.missing = s.missing;
        self.match_source = s.match_source;
        self.importer = s.importer;
        self.timeline = s.timeline;
        self.bin.tab = s.bin_tab;
        self.bin.selected = s.bin_selected;
        self.on_welcome = s.on_welcome;
    }

    /// Esc exits the lesson unless a dialog, menu or text field would take it. Runs before
    /// anything else this frame, so it sees what was open last frame.
    pub(super) fn lesson_keys(&mut self, ctx: &egui::Context) {
        if self.lesson.is_none() {
            return;
        }
        let free = ctx.memory(|m| m.top_modal_layer().is_none() && m.focused().is_none())
            && !egui::Popup::is_any_open(ctx);
        if free && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.end_lesson();
        }
    }

    /// Runs the lesson a frame, after the panels drew their anchors: moves on when the step's
    /// check passes, reveals a hidden anchor, and draws the overlay and step card.
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
        if let Some(step) = steps.get(run.step) {
            // Back on `self` for the check, which reads the baseline.
            self.lesson = Some(run);
            let done = (step.done)(self);
            run = self.lesson.take().expect("the check can't end the lesson");
            if done && run.armed {
                run.completed = true;
                self.next_step(&mut run);
                ctx.request_repaint();
            }
            run.armed |= !done;
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
        let target = step.and_then(|s| anchor_rect(ctx, s.anchor));
        if let Some(s) = step.filter(|_| target.is_none()) {
            let queue =
                (s.anchor == Tab::RenderQueue.anchor()).then_some(App::show_queue as fn(&mut App));
            match s.reveal.or(queue) {
                Some(reveal) if !run.revealed => {
                    run.revealed = true;
                    reveal(self);
                    ctx.request_repaint();
                }
                _ if !run.warned => {
                    run.warned = true;
                    let (id, n, anchor) = (run.lesson.id, run.step, s.anchor);
                    tracing::warn!("lesson {id} step {n}: anchor `{anchor}` isn't drawn");
                }
                _ => {}
            }
        }
        if let Some(t) = target {
            let also = step.map_or(&[][..], |s| s.also);
            let more = also.iter().filter_map(|a| anchor_rect(ctx, a));
            dim(ctx, &[t].into_iter().chain(more).collect::<Vec<_>>());
        }
        let text = step.map_or_else(|| tr!("tutorial.complete").into(), |s| (s.text)(self));
        let modal = ctx.memory(|m| m.top_modal_layer().is_some());
        let Some(click) = card(ctx, &run, &text, target, !modal) else {
            self.lesson = Some(run);
            return;
        };
        ctx.request_repaint();
        match click {
            Click::Back => {
                run.go(run.step.saturating_sub(1));
                run.armed = false;
            }
            Click::ShowMe => {
                if let Some(s) = step {
                    (s.show_me)(self);
                }
            }
            Click::Skip => self.next_step(&mut run),
            Click::Keep => {
                self.save(true);
            }
            Click::Exit => {
                self.lesson = Some(run);
                return self.end_lesson();
            }
        }
        self.lesson = Some(run);
    }

    fn next_step(&mut self, run: &mut Run) {
        run.go(run.step + 1);
        run.armed = true;
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

/// Over the panels, under windows, menus and the step card.
const DIM: Order = Order::Background;

/// Dims the window around `target`, outlined in Light Blue. Painted only, so every click
/// still reaches the editor.
fn dim(ctx: &egui::Context, targets: &[Rect]) {
    let screen = ctx.content_rect();
    let holes: Vec<Rect> = targets
        .iter()
        .map(|t| t.expand(space::S4).intersect(screen))
        .collect();
    let painter = ctx.layer_painter(LayerId::new(DIM, Id::new("lesson_dim")));
    let shade = color::ground().gamma_multiply(0.8);
    for r in shaded(screen, &holes) {
        painter.rect_filled(r, 0, shade);
    }
    for &hole in &holes {
        painter.rect_stroke(
            hole,
            CornerRadius::same(radius::PANEL),
            Stroke::new(2.0, color::light_blue()),
            egui::StrokeKind::Inside,
        );
    }
}

/// `screen` less `holes`, as rects: in each band between hole edges, the gaps between the
/// holes crossing it.
fn shaded(screen: Rect, holes: &[Rect]) -> Vec<Rect> {
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

const CARD_W: f32 = 320.0;
/// Roughly a card's height, to tell whether it fits above or below the target.
const CARD_H: f32 = 140.0;

/// The step card beside `target`, or centred when there is none: the lesson and step, the
/// instruction `text` and the buttons. Returns the button clicked, never while not `live`.
fn card(
    ctx: &egui::Context,
    run: &Run,
    text: &str,
    target: Option<Rect>,
    live: bool,
) -> Option<Click> {
    let screen = ctx.content_rect().shrink(space::S16);
    let gap = space::S12;
    let (at, pivot) = match target {
        Some(t) if t.right() + gap + CARD_W <= screen.right() => {
            (pos2(t.right() + gap, t.top()), Align2::LEFT_TOP)
        }
        Some(t) if t.left() - gap - CARD_W >= screen.left() => {
            (pos2(t.left() - gap, t.top()), Align2::RIGHT_TOP)
        }
        Some(t) if t.top() - gap - CARD_H >= screen.top() => {
            (pos2(t.center().x, t.top() - gap), Align2::CENTER_BOTTOM)
        }
        Some(t) if t.bottom() + gap + CARD_H <= screen.bottom() => {
            (pos2(t.center().x, t.bottom() + gap), Align2::CENTER_TOP)
        }
        _ => (screen.center(), Align2::CENTER_CENTER),
    };
    let steps = run.lesson.steps;
    let step = steps.get(run.step);
    let mut click = None;
    egui::Area::new(Id::new("lesson_card"))
        .order(Order::Foreground)
        .interactable(live)
        .fixed_pos(at.max(screen.min))
        .pivot(pivot)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(color::raised())
                .inner_margin(space::S12)
                .show(ui, |ui| {
                    ui.set_width(CARD_W);
                    ui.horizontal(|ui| {
                        let title = egui::RichText::new((run.lesson.title)())
                            .font(theme::body_strong())
                            .color(color::ink());
                        ui.label(title);
                        if step.is_some() {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let n = tr!("tutorial.step", n = run.step + 1, total = steps.len());
                                let n = egui::RichText::new(n)
                                    .font(theme::small())
                                    .color(color::ink_3());
                                ui.label(n);
                            });
                        }
                    });
                    ui.add_space(space::S8);
                    let text = egui::RichText::new(text)
                        .font(theme::body())
                        .color(color::ink());
                    ui.add(egui::Label::new(text).wrap());
                    ui.add_space(space::S12);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(space::S4, space::S4);
                        let mut button = |b: Button<'_>, c: Click| {
                            if ui.add(b).clicked() {
                                click = Some(c);
                            }
                        };
                        if step.is_some() {
                            button(Button::secondary(tr!("tutorial.show_me")), Click::ShowMe);
                            let back = if run.step == 0 {
                                State::Disabled
                            } else {
                                State::Live
                            };
                            button(Button::ghost(tr!("tutorial.back")).state(back), Click::Back);
                            button(Button::ghost(tr!("tutorial.skip_step")), Click::Skip);
                        } else {
                            button(Button::secondary(tr!("tutorial.keep_project")), Click::Keep);
                        }
                        button(Button::ghost(tr!("tutorial.exit")), Click::Exit);
                    });
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
                show_me: |a| {
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
                show_me: |a| {
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
                show_me: |a| {
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
                show_me: |a| {
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
                show_me: |_| {},
                reveal: None,
                ghost: None,
                also: &[],
            },
            Step {
                anchor: "nowhere",
                text: |_| String::new(),
                done: |_| false,
                show_me: |_| {},
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

    /// Exports need a GPU, so the tour's last step does too.
    #[test]
    fn every_lesson_runs_by_show_me_and_draws_each_steps_anchor() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let dir = tmp("show-me");
        for lesson in LESSONS.iter().chain([&BASICS]) {
            let mut h = harness_with(Harness::builder().wgpu(), &Default::default());
            // Where Show Me's export lands.
            h.state_mut().export_dialog.set_path(dir.join("x.mp4"));
            h.state_mut().start_lesson(lesson);
            for (i, step) in lesson.steps.iter().enumerate() {
                settle(&mut h);
                let at = h.state().lesson.as_ref().map(|r| r.step);
                assert_eq!(at, Some(i), "{}: on step {i}", lesson.id);
                // A renamed or removed control fails here instead of in the lesson.
                for anchor in [step.anchor].iter().chain(step.also) {
                    assert!(
                        drawn(&h, anchor).is_some(),
                        "{} step {i}: `{anchor}` never drawn",
                        lesson.id,
                    );
                }
                // A take needs a microphone, so the run skips it.
                if step.anchor == "record" {
                    click(&mut h, tr!("tutorial.skip_step"));
                    continue;
                }
                h.get_by_label(tr!("tutorial.show_me")).click();
                // Each frame renders, as in the app: the scopes count a rendered frame, so a
                // step reading them passes a few frames on.
                for _ in 0..100 {
                    h.step();
                    h.render().unwrap();
                    if (step.done)(h.state()) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                assert!(
                    (step.done)(h.state()),
                    "{} step {i}: Show Me didn't pass",
                    lesson.id
                );
            }
            settle(&mut h);
            let at = h.state().lesson.as_ref().map(|r| r.step);
            assert_eq!(at, Some(lesson.steps.len()), "{}: finished", lesson.id);
            assert!(h.state().tutorials_done.iter().any(|d| d == lesson.id));
            click(&mut h, tr!("tutorial.exit"));
            assert!(h.state().lesson.is_none());
            assert!(!h.state().preview.playing(), "{}: still playing", lesson.id);
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
            show_me: |a| fake_export(a),
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
        click(&mut h, tr!("tutorial.show_me"));
        settle(&mut h);
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
        click(&mut h, tr!("tutorial.show_me"));
        click(&mut h, tr!("tutorial.show_me"));
        // Custom in the diamond's easing menu passes the easing step but not the graph step.
        ease_first(h.state_mut(), Easing::Bezier([0.1, 0.0, 0.3, 1.0]));
        settle(&mut h);
        assert_eq!(step(&h), Some(3));
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
        click(&mut h, tr!("tutorial.show_me"));
        settle(&mut h);
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
    fn back_stays_on_a_done_step_and_skip_moves_on() {
        let mut h = harness();
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        click(&mut h, tr!("tutorial.show_me"));
        let step = |h: &Harness<'_, App>| h.state().lesson.as_ref().map(|r| r.step);
        assert_eq!(step(&h), Some(1));
        click(&mut h, tr!("tutorial.back"));
        assert_eq!(step(&h), Some(0), "a done step waits after Back");
        click(&mut h, tr!("tutorial.skip_step"));
        click(&mut h, tr!("tutorial.skip_step"));
        assert_eq!(step(&h), Some(2));
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
        click(&mut h, tr!("tutorial.show_me"));
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
        click(&mut h, tr!("tutorial.show_me"));
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
            click(&mut h, tr!("tutorial.show_me"));
        }
        assert!(!h.state().timeline.selected().is_empty());
        assert!(h.state().bin.selected.is_some());
        click(&mut h, tr!("tutorial.exit"));
        assert!(h.state().timeline.selected().is_empty());
        assert_eq!(h.state().bin.selected, None);
    }

    #[test]
    fn show_me_selects_the_new_title_like_a_click() {
        let mut h = harness();
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        click(&mut h, tr!("tutorial.show_me"));
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
        click(&mut h, tr!("tutorial.show_me"));
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
        (GREEN_SCREEN.steps[0].show_me)(a);
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
    fn esc_exits_unless_a_dialog_is_open() {
        let mut h = harness();
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        h.key_press(Key::Escape);
        h.run();
        assert!(h.state().lesson.is_none(), "Esc exits");

        // A card shown after Project Settings opened sits above it, but neither its Exit nor
        // Esc leave the sandbox under the dialog, whose Apply would land on the user's project.
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
            h.state().lesson.is_some(),
            "Esc closes the dialog, not the lesson"
        );
        under_settings(&mut h);
        click(&mut h, tr!("tutorial.exit"));
        assert!(
            h.state().lesson.is_some(),
            "the card is inert under a dialog"
        );
    }

    #[test]
    fn a_steps_ghost_shows_only_while_it_does() {
        const GHOSTED: Lesson = Lesson {
            steps: &[Step {
                anchor: "preview",
                text: |_| String::new(),
                done: |_| false,
                show_me: |_| {},
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

    /// Show Me cuts `DEAD_AIR` without waiting on peaks, so it has to be what Detect Silence
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

    #[test]
    fn the_dim_stays_under_windows() {
        assert!(DIM < Order::Middle);
    }

    #[test]
    fn the_dim_shades_everything_but_each_hole() {
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
