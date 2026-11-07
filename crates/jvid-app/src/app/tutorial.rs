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
    /// The [`anchor`] the step spotlights: the one control the user acts on. An Inspector
    /// row's, `inspector.<row>`, falls back to the whole panel while the row isn't drawn.
    /// `clip` and `asset` mean the timeline clip or media item `item` picks; `timeline.ruler`
    /// uses its time to point at the intended seek location.
    pub anchor: &'static str,
    /// The clip, media item or ruler time the anchor means.
    pub item: Option<fn(&App) -> Option<Item>>,
    /// The instruction: one short imperative sentence. Shortcuts come from `app.keymap`, so
    /// a remapped key reads right.
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
    /// What the user does first, in order, while `anchor` can't be used yet: selecting a
    /// clip, opening a menu or the dialog the control is in.
    pub via: &'static [Via],
    /// A target ellipse, its centre and size in project pixels from the frame's centre,
    /// drawn dashed on the Program view while the step shows.
    pub ghost: Option<[[f64; 2]; 2]>,
    /// Anchors left bright but not clickable beside `anchor`: where a drag lands, or a view
    /// to watch while editing.
    pub watch: &'static [&'static str],
}

/// The defaults every lesson step starts from.
pub const STEP: Step = Step {
    anchor: "",
    item: None,
    text: |_| String::new(),
    done: |_| false,
    example: |_| {},
    reveal: None,
    via: &[],
    ghost: None,
    watch: &[],
};

/// An action before a step's own, spotlit instead of what comes after it while that isn't
/// ready.
#[derive(Clone, Copy)]
pub struct Via {
    pub anchor: &'static str,
    pub item: Option<fn(&App) -> Option<Item>>,
    pub text: fn(&App) -> String,
    /// Whether what comes after is ready to use, such as a clip being selected, a dialog
    /// open or a menu item drawn.
    pub ready: fn(&App, &egui::Context) -> bool,
}

/// A timeline clip, media item or ruler time a step spotlights.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item {
    Clip(jvid_core::model::ClipId),
    Asset(jvid_core::model::AssetId),
    Time(Time),
}

/// A [`Via`] that has the user click the clip `item` picks, until it's the only selection.
macro_rules! select {
    ($item:expr) => {
        $crate::app::tutorial::Via {
            anchor: "clip",
            item: Some(|a| $item(a).map($crate::app::tutorial::Item::Clip)),
            text: |_| $crate::i18n::tr!("tutorial.select_clip").into(),
            ready: |a, _| {
                let s = a.timeline.selected();
                $item(a).is_some_and(|id| s.len() == 1 && s.contains(&id))
            },
        }
    };
}
pub(crate) use select;

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
            anchor: "media.import",
            text: |_| tr!("tutorial.tour_import", file = verbatim(TOUR_CLIP)),
            done: |a| clip_named(a, TOUR_CLIP).is_some(),
            example: |a| match sample(TOUR_CLIP) {
                Ok(path) => a.import([path]),
                Err(e) => a.error(tr!(
                    "app.open_failed",
                    path = verbatim(TOUR_CLIP),
                    error = e
                )),
            },
            ..STEP
        },
        Step {
            anchor: "asset",
            item: Some(|a| clip_named(a, TOUR_CLIP).map(|x| Item::Asset(x.id))),
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
            watch: &["timeline.lanes"],
            ..STEP
        },
        Step {
            anchor: "preview.play",
            text: |_| tr!("tutorial.tour_play").into(),
            done: |a| a.timeline.playhead() > Time(0),
            example: |a| {
                let length = a.history.project().sequence().duration();
                a.timeline.seek(Time(length.0 * 2 / 3));
            },
            ..STEP
        },
        Step {
            anchor: "timeline.ruler",
            item: Some(|a| {
                Some(Item::Time(Time(
                    a.history.project().sequence().duration().0 / 3,
                )))
            }),
            text: |_| tr!("tutorial.tour_start").into(),
            done: |a| {
                let length = a.history.project().sequence().duration();
                !a.preview.playing() && a.timeline.playhead() < Time(length.0 / 2)
            },
            example: |a| {
                if a.preview.playing() {
                    a.preview.program_action("shuttle_stop");
                }
                let length = a.history.project().sequence().duration();
                a.timeline.seek(Time(length.0 / 3));
            },
            ..STEP
        },
        Step {
            anchor: "timeline.ruler",
            item: Some(|a| Some(Item::Time(a.timeline.playhead()))),
            text: |a| tr!("tutorial.tour_in", key = a.keymap.shortcut("mark_in")),
            done: |a| a.history.project().sequence().in_point().is_some(),
            example: |a| {
                let at = a.timeline.playhead();
                let out_point = a.history.project().sequence().out_point();
                let in_point = Some(at);
                apply(
                    a,
                    Command::SetInOut {
                        in_point,
                        out_point,
                    },
                );
            },
            ..STEP
        },
        Step {
            anchor: "timeline.ruler",
            item: Some(|a| {
                let seq = a.history.project().sequence();
                let later = seq.in_point().unwrap_or_default() + SECOND;
                Some(Item::Time(Time(seq.duration().0 * 2 / 3).max(later)))
            }),
            text: |_| tr!("tutorial.tour_later").into(),
            done: |a| {
                let in_point = a.history.project().sequence().in_point();
                in_point.is_some_and(|i| a.timeline.playhead() > i + Time(SECOND.0 / 2))
            },
            example: |a| {
                let length = a.history.project().sequence().duration();
                a.timeline.seek(Time(length.0 * 2 / 3));
            },
            ..STEP
        },
        Step {
            anchor: "timeline.ruler",
            item: Some(|a| Some(Item::Time(a.timeline.playhead()))),
            text: |a| tr!("tutorial.tour_out", key = a.keymap.shortcut("mark_out")),
            done: |a| {
                let seq = a.history.project().sequence();
                matches!((seq.in_point(), seq.out_point()), (Some(i), Some(o)) if o > i)
            },
            example: mark,
            ..STEP
        },
        Step {
            anchor: "timeline.range",
            text: |a| tr!("tutorial.tour_cut", key = a.keymap.shortcut("extract")),
            done: |a| shortened(a, TOUR_CLIP),
            example: |a| {
                let seq = a.history.project().sequence();
                if seq.in_point().is_none() || seq.out_point().is_none() {
                    mark(a);
                }
                apply(a, Command::RemoveRange { ripple: true });
            },
            ..STEP
        },
        Step {
            anchor: "bin.title",
            text: |_| {
                tr!(
                    "tutorial.tour_title",
                    title = crate::media_bin::presets()[0].0
                )
            },
            done: has_title,
            example: add_title,
            reveal: Some(|a| a.bin.tab = 2),
            watch: &["timeline.lanes"],
            ..STEP
        },
        Step {
            anchor: "export.action",
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
            via: &[OPEN_EXPORT],
            ..STEP
        },
    ],
};

/// Opens the export dialog, for a step whose control is in it.
pub(super) const OPEN_EXPORT: Via = Via {
    anchor: "export",
    item: None,
    text: |_| tr!("tutorial.open_export").into(),
    ready: |a, _| a.export_dialog.is_open(),
};

/// The screen recording lesson's sample.
const SCREEN_CLIP: &str = "screen-recording.mp4";

/// The sample's dead air in seconds, as `scripts/gen-samples` makes it.
const DEAD_AIR: (f64, f64) = (2.4, 7.0);

/// Where the typing sits in the 1280x720 sample, for the zoom.
const TYPING: [f64; 2] = [400.0, 180.0];

/// The screen recording's first clip on a `kind` track.
fn screen_clip(app: &App, kind: TrackKind) -> Option<jvid_core::model::ClipId> {
    let asset = clip_named(app, SCREEN_CLIP)?.id;
    let seq = app.history.project().sequence();
    let mut tracks = seq.tracks().iter().filter(|t| t.kind == kind);
    tracks.find_map(|t| t.clips().iter().find(|c| c.asset == asset).map(|c| c.id))
}

fn screen_video(app: &App) -> Option<jvid_core::model::ClipId> {
    screen_clip(app, TrackKind::Video)
}

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
            anchor: "silence.cut",
            text: |_| tr!("tutorial.silence_cut").into(),
            done: |a| shortened(a, SCREEN_CLIP) && a.lesson.as_ref().is_some_and(|r| r.silence_cut),
            example: |a| {
                a.timeline.dismiss_dialogs();
                let Some(id) = screen_clip(a, TrackKind::Audio) else {
                    return;
                };
                let seq = a.history.project().sequence();
                let Some((_, c)) = seq.clip(id) else {
                    return;
                };
                // The span Detect Silence finds at its defaults: the dead air less 0.15 s padding.
                let at = |s: f64| c.timeline_at(Time((s * FLICKS_PER_SECOND as f64) as i64));
                let span = (c.id, at(DEAD_AIR.0 + 0.15), at(DEAD_AIR.1 - 0.15));
                a.timeline.select_clip(seq, id);
                let spans = vec![span];
                apply(a, Command::CutSpans { spans, speed: None });
            },
            via: &[Via {
                anchor: "clip",
                item: Some(|a| screen_video(a).map(Item::Clip)),
                text: |_| tr!("tutorial.screen_silence").into(),
                ready: |a, _| a.timeline.dialog_open(),
            }],
            ..STEP
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
            via: &[select!(screen_video)],
            watch: &["preview.frame"],
            ..STEP
        },
        Step {
            anchor: "bin.title",
            text: |_| {
                tr!(
                    "tutorial.screen_callout",
                    title = crate::media_bin::presets()[0].0
                )
            },
            done: has_title,
            example: add_title,
            reveal: Some(|a| a.bin.tab = 2),
            watch: &["timeline.lanes"],
            ..STEP
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
            anchor: "asset",
            item: Some(|a| clip_named(a, BACKGROUND).map(|x| Item::Asset(x.id))),
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
            watch: &["timeline.lanes"],
            ..STEP
        },
        Step {
            anchor: "inspector.add_effect",
            text: |_| tr!("tutorial.green_screen_key").into(),
            done: |a| key_index(a).is_some(),
            example: |a| {
                if let Some(id) = keyed_id(a) {
                    a.timeline.select_clip(a.history.project().sequence(), id);
                }
                add_key(a);
            },
            via: &[select!(keyed_id)],
            ..STEP
        },
        Step {
            anchor: "inspector.fx_tolerance",
            text: |_| tr!("tutorial.green_screen_tolerance").into(),
            done: |a| tuned(a, "tolerance"),
            // Clears the green screen while the board on the right still shows through.
            example: |a| tune(a, "tolerance", 0.8),
            via: &[select!(keyed_id)],
            watch: &["preview.frame"],
            ..STEP
        },
        Step {
            anchor: "inspector.fx_edge_softness",
            text: |_| tr!("tutorial.green_screen_softness").into(),
            done: |a| tuned(a, "edge_softness"),
            example: |a| tune(a, "edge_softness", 0.15),
            via: &[select!(keyed_id)],
            watch: &["preview.frame"],
            ..STEP
        },
        Step {
            anchor: "inspector.fx_spill_suppression",
            text: |_| tr!("tutorial.green_screen_spill").into(),
            done: |a| tuned(a, "spill_suppression"),
            example: |a| tune(a, "spill_suppression", 0.7),
            via: &[select!(keyed_id)],
            watch: &["preview.frame"],
            ..STEP
        },
        Step {
            anchor: "inspector.add_mask",
            text: |_| tr!("tutorial.green_screen_mask").into(),
            done: |a| keyed_mask(a).is_some(),
            example: |a| {
                use jvid_core::model::{Mask, MaskShape};
                // As Add Mask > Rectangle makes it: centred, half the image's size.
                edit_keyed(a, |[w, h], x| {
                    let points = vec![[0.0, 0.0], [w / 2.0, h / 2.0]];
                    x.masks.push(Mask::new(MaskShape::Rectangle, points));
                });
            },
            via: &[select!(keyed_id)],
            ..STEP
        },
        Step {
            anchor: "preview.frame",
            text: |_| tr!("tutorial.green_screen_fit").into(),
            done: |a| {
                let Some((m, [w, h])) = keyed_mask(a) else {
                    return false;
                };
                let ([cx, _], [sx, sy]) = (m.points[0], m.points[1]);
                // The board starts at 70% of the width; the man fills most of the height.
                cx + sx.abs() / 2.0 <= 0.2 * w && sy.abs() >= 0.7 * h
            },
            example: |a| {
                // Keeps the left 65% of the frame, which ends just before the board.
                edit_keyed(a, |[w, h], x| {
                    if let Some(m) = x.masks.first_mut() {
                        m.points = vec![[-0.2 * w, 0.0], [0.7 * w, 1.1 * h]];
                    }
                });
            },
            via: &[select!(keyed_id)],
            ..STEP
        },
    ],
};

/// The green-screen clip's id.
fn keyed_id(app: &App) -> Option<jvid_core::model::ClipId> {
    keyed_clip(app).map(|(_, id)| id)
}

/// The green-screen clip's first rectangle mask, while on, with the clip's fitted size.
fn keyed_mask(app: &App) -> Option<(jvid_core::model::Mask, [f64; 2])> {
    use jvid_core::model::MaskShape;
    let project = app.history.project();
    let (_, c) = project.sequence().clip(keyed_id(app)?)?;
    let m = c.transform.masks.iter().find(|m| m.enabled)?;
    (m.shape == MaskShape::Rectangle).then(|| (m.clone(), crate::masks::fitted(project, c)))
}

/// Whether Chroma Key's parameter `name` is off its default.
fn tuned(app: &App, name: &str) -> bool {
    let (Some(i), Some(shader), Some(id)) = (key_index(app), chroma_key(), keyed_id(app)) else {
        return false;
    };
    let Some((_, c)) = app.history.project().sequence().clip(id) else {
        return false;
    };
    let default = shader.params.iter().find(|p| p.name == name);
    c.transform.effects[i].params.get(name).copied() != default.map(|p| p.default)
}

/// Sets Chroma Key's parameter `name`, adding the key first if needed, as its slider does.
fn tune(app: &mut App, name: &str, value: f64) {
    if key_index(app).is_none() {
        add_key(app);
    }
    let Some(i) = key_index(app) else {
        return;
    };
    edit_keyed(app, |_, x| {
        x.effects[i].params.insert(name.into(), value);
    });
}

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
            anchor: "inspector.position_key",
            text: |_| tr!("tutorial.keys_position").into(),
            done: |a| keys(a, "position") >= 1,
            example: |a| key_title(a, "position", Value::Vec2([0.0, 100.0]), Time(0)),
            via: &[select!(first_clip)],
            ..STEP
        },
        Step {
            anchor: "inspector.opacity_key",
            text: |_| tr!("tutorial.keys_opacity").into(),
            done: |a| keys(a, "opacity") >= 1,
            example: |a| key_title(a, "opacity", Value::Scalar(0.0), Time(0)),
            via: &[select!(first_clip)],
            ..STEP
        },
        Step {
            anchor: "timeline.ruler",
            item: Some(|_| Some(Item::Time(SECOND))),
            text: |_| tr!("tutorial.keys_second").into(),
            done: |a| {
                let at = a.timeline.playhead();
                title(a).is_some_and(|c| at >= Time(SECOND.0 / 2) && at < c.end())
            },
            example: |a| a.timeline.seek(SECOND),
            ..STEP
        },
        Step {
            anchor: "inspector.position",
            text: |_| tr!("tutorial.keys_move").into(),
            done: |a| keys(a, "position") >= 2,
            example: |a| {
                let at = a.timeline.playhead();
                key_title(a, "position", Value::Vec2([0.0, 0.0]), at);
            },
            via: &[select!(first_clip)],
            watch: &["preview.frame"],
            ..STEP
        },
        Step {
            anchor: "inspector.opacity",
            text: |_| tr!("tutorial.keys_fade").into(),
            done: |a| keys(a, "opacity") >= 2,
            example: |a| {
                let at = a.timeline.playhead();
                key_title(a, "opacity", Value::Scalar(1.0), at);
            },
            via: &[select!(first_clip)],
            watch: &["preview.frame"],
            ..STEP
        },
        Step {
            anchor: "inspector.position_key",
            text: |_| tr!("tutorial.keys_previous").into(),
            done: |a| first_key(a).is_some_and(|t| a.timeline.playhead() == t),
            example: |a| {
                if let Some(t) = first_key(a) {
                    a.timeline.seek(t);
                }
            },
            via: &[select!(first_clip)],
            ..STEP
        },
        Step {
            anchor: "inspector.position_key",
            text: |_| tr!("tutorial.keys_ease").into(),
            done: |a| {
                let eased = |e| e != Easing::Linear;
                title_keys(a).any(|k| eased(k.easing))
            },
            example: |a| ease_first(a, Easing::EaseOut),
            via: &[select!(first_clip)],
            ..STEP
        },
        Step {
            anchor: "timeline.keys",
            item: Some(|a| first_clip(a).map(Item::Clip)),
            text: |_| tr!("tutorial.keys_graph").into(),
            done: |a| a.timeline.graph_open(),
            example: |a| {
                if let Some(id) = first_clip(a) {
                    a.timeline.open_graph(id);
                }
            },
            ..STEP
        },
        Step {
            anchor: "timeline.graph",
            text: |_| tr!("tutorial.keys_handle").into(),
            done: |a| {
                let custom = |e| matches!(e, Easing::Bezier(_));
                a.timeline.graph_open() && title_keys(a).any(|k| custom(k.easing))
            },
            // A steeper ease out than the preset, as dragging its first handle left gives.
            example: |a| ease_first(a, Easing::Bezier([0.0, 0.0, 0.2, 1.0])),
            reveal: Some(|a| {
                if let Some(id) = first_clip(a) {
                    a.timeline.open_graph(id);
                }
            }),
            ..STEP
        },
        Step {
            anchor: "preview.play",
            text: |_| tr!("tutorial.keys_play").into(),
            done: |a| a.preview.playing(),
            example: |a| {
                if let Some(t) = first_key(a) {
                    a.timeline.seek(t);
                }
                a.preview_active = 1;
                a.preview.program_action("play_stop");
            },
            ..STEP
        },
    ],
};

/// How many keys the title's property `prop` has.
fn keys(app: &App, prop: &str) -> usize {
    title(app).map_or(0, |c| c.keyframes.get(prop).map_or(0, Vec::len))
}

/// Keys the title's `prop` to `value` at timeline time `at`, as its diamond or field does;
/// a position is relative to where the title sits.
fn key_title(app: &mut App, prop: &str, value: Value, at: Time) {
    let Some(c) = title(app) else { return };
    let value = match value {
        Value::Vec2([x, y]) => {
            Value::Vec2([c.transform.position[0] + x, c.transform.position[1] + y])
        }
        v => v,
    };
    let key = Keyframe {
        time: c.source_at(at),
        value,
        easing: Easing::Linear,
    };
    let mut keys = c.keyframes.clone();
    let lane = keys.entry(prop.into()).or_default();
    lane.retain(|k| k.time != key.time);
    lane.push(key);
    lane.sort_by_key(|k| k.time);
    apply(
        app,
        Command::SetClipProps(vec![(c.id, c.transform.clone(), keys)]),
    );
}

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

/// The clip or media item the current step means, for [`item_anchor`].
const ITEM: &str = "lesson_item";

/// Records `rect` as anchor `name` when `item` is the one the current step means.
pub fn item_anchor(ctx: &egui::Context, name: &'static str, item: Item, rect: Rect) {
    if ctx.data(|d| d.get_temp::<Item>(Id::new(ITEM))) == Some(item) {
        anchor(ctx, name, rect);
    }
}

/// The intended seek location for a ruler step, mapped by the timeline's live view.
pub(crate) fn ruler_time(ctx: &egui::Context) -> Option<Time> {
    match ctx.data(|d| d.get_temp::<Item>(Id::new(ITEM))) {
        Some(Item::Time(at)) => Some(at),
        _ => None,
    }
}

/// The Inspector row a step wants scrolled into view, once it's drawn.
const SCROLL: &str = "lesson_scroll";

/// Records the rows `ui` laid out below `top` as anchor `name`, through [`anchor_in`].
pub fn anchor_below(ui: &egui::Ui, name: &'static str, top: f32) {
    let r = ui.min_rect();
    anchor_in(
        ui,
        name,
        Rect::from_x_y_ranges(r.x_range(), top..=r.bottom()),
    );
}

/// Records `rect`, laid out in `ui`, as anchor `name`, cut to what shows of it. The first
/// time a step wants it, it scrolls into view.
pub fn anchor_in(ui: &egui::Ui, name: &'static str, rect: Rect) {
    let ctx = ui.ctx();
    if !running(ctx) {
        return;
    }
    if ctx.data(|d| d.get_temp::<&'static str>(Id::new(SCROLL))) == Some(name) {
        ui.scroll_to_rect(rect, Some(Align::Center));
        ctx.data_mut(|d| d.remove::<&'static str>(Id::new(SCROLL)));
    }
    let shown = rect.intersect(ui.clip_rect());
    if shown.is_positive() {
        anchor(ctx, name, shown);
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
        _ if row.starts_with("fx_") => Pane::Effects,
        "volume" | "eq" => Pane::Audio,
        _ if row.starts_with("text_") => Pane::Text,
        _ => Pane::Video,
    }
}

/// The dock panel anchor `name` is drawn in, revealed while it's hidden.
fn tab_of(name: &str) -> Option<Tab> {
    let panel = name.split('.').next().unwrap_or_default();
    [
        Tab::Media,
        Tab::Preview,
        Tab::Inspector,
        Tab::Timeline,
        Tab::Transcript,
        Tab::RenderQueue,
    ]
    .into_iter()
    .find(|t| match panel {
        "clip" | "marker" => *t == Tab::Timeline,
        "asset" | "bin" | "scope" => *t == Tab::Media,
        "record" => *t == Tab::Preview,
        _ => t.anchor() == panel,
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
    /// The current step's goal, measured on entry and after each frame.
    pub(crate) done: bool,
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
    /// hidden anchor, and spotlights the step's control with its card beside it.
    pub(super) fn lesson_frame(&mut self, ctx: &egui::Context) {
        let recorded = running(ctx);
        if recorded != self.lesson.is_some() {
            ctx.data_mut(|d| d.insert_temp(Id::new(RUNNING), self.lesson.is_some()));
            ctx.request_repaint();
        }
        if self.lesson.is_none() {
            ctx.data_mut(|d| {
                d.remove::<Item>(Id::new(ITEM));
                d.remove::<Vec<Rect>>(Id::new(ALLOWED));
            });
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
        let guidance = step.map(|s| self.lesson_guidance(ctx, s));
        let item = guidance.as_ref().and_then(|g| g.item);
        if item != ctx.data(|d| d.get_temp(Id::new(ITEM))) {
            ctx.data_mut(|d| match item {
                Some(i) => {
                    d.insert_temp(Id::new(ITEM), i);
                }
                None => d.remove::<Item>(Id::new(ITEM)),
            });
            ctx.request_repaint();
        }
        let anchor = guidance.as_ref().map(|g| g.anchor);
        let exact = anchor.and_then(|a| anchor_rect(ctx, a));
        let panel = || row_name(anchor?).and(anchor_rect(ctx, Tab::Inspector.anchor()));
        let target = exact.or_else(panel);
        if let Some(g) = guidance.as_ref().filter(|_| !self.modal_owns_input(ctx)) {
            let unseen = g.watch.iter().any(|w| anchor_rect(ctx, w).is_none());
            let reveal = step
                .and_then(|s| s.reveal)
                .filter(|_| exact.is_none() || unseen);
            let tab = tab_of(g.anchor).filter(|_| exact.is_none());
            // A hidden Inspector row the step names: its tab shows, and it scrolls into view.
            let row = Some(g.anchor).filter(|a| row_name(a).is_some() && exact.is_none());
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
                let (id, n, anchor) = (run.lesson.id, run.step, g.anchor);
                tracing::warn!("lesson {id} step {n}: anchor `{anchor}` isn't drawn");
            }
        }
        // A modal the step doesn't use, such as a save prompt, takes over until it closes.
        let modal = ctx.memory(|m| m.top_modal_layer().and_then(|l| m.area_rect(l.id)));
        let spot = target.filter(|t| modal.is_none_or(|m| m.contains(t.center())));
        let popups = spot.map_or_else(Vec::new, |t| popup_rects(ctx, t));
        let watch: Vec<Rect> = match (&guidance, spot) {
            (Some(g), Some(_)) => g.watch.iter().filter_map(|a| anchor_rect(ctx, a)).collect(),
            _ => Vec::new(),
        };
        if let Some(t) = spot {
            let bright: Vec<Rect> = watch.iter().chain(&popups).copied().collect();
            spotlight(ctx, t, &bright);
        }
        let text = guidance.map_or_else(|| tr!("tutorial.complete").into(), |g| g.text);
        let mut avoid: Vec<Rect> = watch.iter().chain(&popups).copied().collect();
        avoid.extend(windows(ctx));
        let (click, card) = card(ctx, &run, &text, spot, &avoid);
        if spot.is_some() {
            // Always straight above the scrim, whichever showed first.
            let layer = |id| LayerId::new(Order::Foreground, Id::new(id));
            ctx.set_sublayer(layer(SCRIM_ID), layer(CARD_ID));
        }
        // Next frame's presses and wheel land only on the control, its popups and the card.
        let allowed = spot.map(|t| {
            let hole = t.expand(space::S4);
            [hole, card].into_iter().chain(popups).collect::<Vec<_>>()
        });
        ctx.data_mut(|d| match allowed {
            Some(a) => {
                d.insert_temp(Id::new(ALLOWED), a);
            }
            None => d.remove::<Vec<Rect>>(Id::new(ALLOWED)),
        });
        let Some(click) = click else {
            self.lesson = Some(run);
            return;
        };
        ctx.request_repaint();
        match click {
            Click::Next => self.next_step(&mut run),
            Click::Back => {
                self.dismiss_lesson_dialogs(&run);
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
                self.dismiss_lesson_dialogs(&run);
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

    /// What the card says and spotlights: the step, or the earliest of its [`Via`]s the user
    /// still has to do.
    fn lesson_guidance(&self, ctx: &egui::Context, step: &Step) -> Guidance {
        let mut guide = Guidance {
            anchor: step.anchor,
            item: step.item.and_then(|f| f(self)),
            text: (step.text)(self),
            watch: step.watch,
        };
        for via in step.via.iter().rev() {
            if (via.ready)(self, ctx) {
                break;
            }
            guide = Guidance {
                anchor: via.anchor,
                item: via.item.and_then(|f| f(self)),
                text: (via.text)(self),
                watch: &[],
            };
        }
        guide
    }

    /// Establish a fresh baseline on each visit. Back and a prior action satisfying a later
    /// goal must not cascade through lessons, while newly completed tasks need no Next click.
    fn check_lesson_step(&mut self, mut run: Run) -> Run {
        if run.lesson.id == SCREEN.id && !shortened(self, SCREEN_CLIP) {
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

    /// Closes the dialogs the current step works in, so Back and Skip Step don't leave them
    /// behind. Others stay for the user to finish.
    fn dismiss_lesson_dialogs(&mut self, run: &Run) {
        let Some(step) = run.lesson.steps.get(run.step) else {
            return;
        };
        let names = std::iter::once(step.anchor).chain(step.via.iter().map(|v| v.anchor));
        let uses = |prefix| names.clone().any(|n: &str| n.starts_with(prefix));
        if uses("silence.") {
            self.timeline.dismiss_dialogs();
        }
        if uses("subtitles.") && self.subtitles.is_open() {
            self.subtitles.close();
        }
        if uses("export.") {
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
    item: Option<Item>,
    text: String,
    watch: &'static [&'static str],
}

/// The input a spotlit step lets through, for [`Blocker`]: its control, the card and the
/// popups the control opened.
const ALLOWED: &str = "lesson_allowed";

/// The spotlit step's hole and card, as last laid out, for layout tests.
#[cfg(test)]
pub(crate) fn spotlit(ctx: &egui::Context) -> Option<(Rect, Rect)> {
    let allowed = ctx.data(|d| d.get_temp::<Vec<Rect>>(Id::new(ALLOWED)))?;
    Some((allowed[0], allowed[1]))
}

/// Drops pointer presses, wheel and touch input outside a spotlit step's [`ALLOWED`] rects
/// before egui sees them, so only the step's control, its popups and the card respond.
/// Moves and releases pass, so a drag that started on the control can land anywhere; keys
/// pass, so steps can teach shortcuts.
pub(crate) struct Blocker;

impl egui::Plugin for Blocker {
    fn debug_name(&self) -> &'static str {
        "lesson_blocker"
    }

    fn input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        let Some(allowed) = ctx.data(|d| d.get_temp::<Vec<Rect>>(Id::new(ALLOWED))) else {
            return;
        };
        let inside = |p: egui::Pos2| allowed.iter().any(|r| r.contains(p));
        let mut at = ctx.input(|i| i.pointer.latest_pos());
        input.events.retain(|e| match *e {
            egui::Event::PointerMoved(p) => {
                at = Some(p);
                true
            }
            egui::Event::PointerButton {
                pos, pressed: true, ..
            } => inside(pos),
            egui::Event::Touch { pos, .. } => inside(pos),
            egui::Event::MouseWheel { .. } | egui::Event::Zoom(_) => at.is_some_and(inside),
            _ => true,
        });
    }
}

/// The open popups and menus outside `target`, such as the one it opened: what the user
/// picks from next. Dialogs, which stay dim unless they hold the target, aren't popups.
fn popup_rects(ctx: &egui::Context, target: Rect) -> Vec<Rect> {
    if !egui::Popup::is_any_open(ctx) {
        return Vec::new();
    }
    let ours = [Id::new(CARD_ID), Id::new(SCRIM_ID)];
    let max_width = ctx.content_rect().width() * 0.95;
    ctx.memory(|m| {
        let modal = m.top_modal_layer();
        m.areas()
            .visible_layer_ids()
            .into_iter()
            .filter(|l| l.order == Order::Foreground && !ours.contains(&l.id))
            .filter(|l| Some(*l) != modal)
            .filter_map(|l| m.area_rect(l.id))
            .filter(|r| r.width() < max_width && !r.contains(target.center()))
            .collect()
    })
}

/// The open windows, dialogs and popups: what the card keeps clear of when it can.
fn windows(ctx: &egui::Context) -> Vec<Rect> {
    let ours = [Id::new(CARD_ID), Id::new(SCRIM_ID)];
    let max_width = ctx.content_rect().width() * 0.95;
    ctx.memory(|m| {
        let layers = m.areas().visible_layer_ids().into_iter();
        layers
            .filter(|l| matches!(l.order, Order::Middle | Order::Foreground))
            .filter(|l| !ours.contains(&l.id))
            .filter_map(|l| m.area_rect(l.id))
            // The dock and modal backdrops may cover the whole window.
            .filter(|r| r.width() < max_width)
            .collect()
    })
}

const SCRIM_ID: &str = "lesson_scrim";

/// Dims the window but `target` and `bright`, and outlines `target` with a slow pulse.
fn spotlight(ctx: &egui::Context, target: Rect, bright: &[Rect]) {
    let screen = ctx.content_rect();
    let hole = |r: Rect| r.expand(space::S4).intersect(screen);
    let target = hole(target);
    let holes: Vec<Rect> = std::iter::once(target)
        .chain(bright.iter().map(|&r| hole(r)))
        .collect();
    let id = Id::new(SCRIM_ID);
    // Reduce Motion sets the animation time to zero; the outline then holds still.
    let motion = ctx.global_style().animation_time > 0.0;
    egui::Area::new(id)
        .order(Order::Foreground)
        .interactable(false)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let painter = ui.painter().with_clip_rect(screen);
            for r in shaded(screen, &holes) {
                painter.rect_filled(r, 0, color::ground().gamma_multiply(SCRIM));
            }
            let corner = CornerRadius::same(radius::CONTROL);
            let line = |width, alpha| Stroke::new(width, color::light_blue().gamma_multiply(alpha));
            painter.rect_stroke(target, corner, line(2.0, 1.0), egui::StrokeKind::Outside);
            if motion {
                let t = ctx.input(|i| i.time) / PULSE;
                let phase = (t - t.floor()) as f32;
                let ring = target.expand(2.0 + space::S8 * phase);
                painter.rect_stroke(
                    ring,
                    corner,
                    line(2.0, 1.0 - phase),
                    egui::StrokeKind::Outside,
                );
                ctx.request_repaint_after(std::time::Duration::from_millis(33));
            }
        });
    ctx.move_to_top(LayerId::new(Order::Foreground, id));
}

/// The scrim's opacity over the ground colour.
const SCRIM: f32 = 0.72;
/// Seconds per pulse of the target's outline.
const PULSE: f64 = 1.6;

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

const CARD_W: f32 = 280.0;
/// The card's width when the full one won't fit clear of a dialog.
const NARROW: f32 = 208.0;
const CARD_ID: &str = "lesson_card";
/// How far the card's pointer reaches toward its target.
const ARROW: f32 = 8.0;

/// The side of its target the card sits on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Side {
    Below,
    Above,
    Right,
    Left,
}

/// Where the card goes: beside `target`, on the first side where it fits on `screen` whole
/// without covering the target, sliding along that edge to stay on screen. It keeps clear of
/// `avoid` if it can. Failing every side, it takes the corner covering the target least and
/// points nowhere.
fn card_rect(
    screen: Rect,
    size: egui::Vec2,
    target: Option<Rect>,
    avoid: &[Rect],
) -> (Rect, Option<Side>) {
    let screen = screen.shrink(space::S8);
    let overlap = |r: Rect, others: &[Rect]| -> f32 {
        let o = others.iter().filter(|a| r.intersects(**a));
        o.map(|a| r.intersect(*a).area()).sum()
    };
    let slide = |r: egui::Rangef, within: egui::Rangef| {
        (within.min - r.min).max(0.0) + (within.max - r.max).min(0.0)
    };
    if let Some(t) = target {
        let gap = ARROW + space::S4;
        for clear in [avoid, &[]] {
            for side in [Side::Below, Side::Above, Side::Right, Side::Left] {
                let min = match side {
                    Side::Below => pos2(t.center().x - size.x / 2.0, t.bottom() + gap),
                    Side::Above => pos2(t.center().x - size.x / 2.0, t.top() - gap - size.y),
                    Side::Right => pos2(t.right() + gap, t.center().y - size.y / 2.0),
                    Side::Left => pos2(t.left() - gap - size.x, t.center().y - size.y / 2.0),
                };
                let r = Rect::from_min_size(min, size);
                let r = match side {
                    Side::Below | Side::Above => {
                        r.translate(vec2(slide(r.x_range(), screen.x_range()), 0.0))
                    }
                    Side::Right | Side::Left => {
                        r.translate(vec2(0.0, slide(r.y_range(), screen.y_range())))
                    }
                };
                if screen.contains_rect(r) && !r.intersects(t) && overlap(r, clear) == 0.0 {
                    return (r, Some(side));
                }
            }
        }
    }
    // A narrower card, its text wrapping taller, fits beside a centred dialog.
    let narrow = vec2(
        NARROW.min(size.x),
        (size.y * size.x / NARROW).min(screen.height()),
    );
    let corners = [size, narrow].into_iter().flat_map(|size| {
        [
            Align2::RIGHT_BOTTOM,
            Align2::LEFT_BOTTOM,
            Align2::RIGHT_TOP,
            Align2::LEFT_TOP,
        ]
        .map(|a| a.align_size_within_rect(size, screen))
    });
    let covered =
        |r: Rect| overlap(r, &[target.unwrap_or(Rect::NOTHING)]) * 4.0 + overlap(r, avoid);
    let best = corners.min_by(|a, b| covered(*a).total_cmp(&covered(*b)));
    (best.unwrap_or(screen), None)
}

/// The task card beside `target`, pointing at it. Buttons retain normal Tab/Enter/Space
/// behavior and let go of focus after activation so editor shortcuts still reach their panel.
/// Returns the button clicked and where the card is.
fn card(
    ctx: &egui::Context,
    run: &Run,
    text: &str,
    target: Option<Rect>,
    avoid: &[Rect],
) -> (Option<Click>, Rect) {
    let id = Id::new(CARD_ID);
    let previous = ctx.memory(|m| m.area_rect(id));
    let size = previous.map_or(vec2(CARD_W, 140.0), |r| vec2(CARD_W, r.height()));
    let mut bounds = ctx.content_rect();
    if let Some(bar) = anchor_rect(ctx, "appbar") {
        bounds.min.y = bounds.min.y.max(bar.bottom());
    }
    let (rect, side) = card_rect(bounds, size, target, avoid);
    let steps = run.lesson.steps;
    let step = steps.get(run.step);
    let mut click = None;
    let layer = LayerId::new(Order::Foreground, id);
    // Above the scrim and any modal's backdrop, without becoming a modal.
    ctx.move_to_top(layer);
    let shown = egui::Area::new(id)
        .order(Order::Foreground)
        .sense(egui::Sense::hover())
        .movable(false)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            // Light Blue edged, like the outline it points at, so it stands off the scrim.
            egui::Frame::popup(ui.style())
                .fill(color::raised())
                .stroke(Stroke::new(1.0, color::light_blue()))
                .corner_radius(radius::PANEL)
                .inner_margin(space::S12)
                .show(ui, |ui| {
                    ui.set_width(rect.width() - 2.0 * space::S12);
                    let heading = match step {
                        Some(_) => tr!("tutorial.step", n = run.step + 1, total = steps.len()),
                        None => (run.lesson.title)().into(),
                    };
                    ui.label(
                        egui::RichText::new(heading)
                            .font(theme::small())
                            .color(color::ink_3()),
                    );
                    ui.add_space(space::S4);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .font(theme::body_strong())
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
    let card = shown.response.rect;
    if let (Some(t), Some(side)) = (target, side) {
        pointer(&ctx.layer_painter(layer), card, t, side);
    }
    (click, card)
}

/// The card's pointer: a triangle on the `card` edge facing `target`, aimed at its centre.
fn pointer(painter: &egui::Painter, card: Rect, target: Rect, side: Side) {
    let inset = f32::from(radius::PANEL) + ARROW;
    let along = |lo: f32, hi: f32, at: f32| at.clamp(lo + inset, (hi - inset).max(lo + inset));
    let (base, tip, across) = match side {
        Side::Below => {
            let x = along(card.left(), card.right(), target.center().x);
            (
                pos2(x, card.top()),
                pos2(x, card.top() - ARROW),
                vec2(ARROW, 0.0),
            )
        }
        Side::Above => {
            let x = along(card.left(), card.right(), target.center().x);
            (
                pos2(x, card.bottom()),
                pos2(x, card.bottom() + ARROW),
                vec2(ARROW, 0.0),
            )
        }
        Side::Right => {
            let y = along(card.top(), card.bottom(), target.center().y);
            (
                pos2(card.left(), y),
                pos2(card.left() - ARROW, y),
                vec2(0.0, ARROW),
            )
        }
        Side::Left => {
            let y = along(card.top(), card.bottom(), target.center().y);
            (
                pos2(card.right(), y),
                pos2(card.right() + ARROW, y),
                vec2(0.0, ARROW),
            )
        }
    };
    let points = vec![base - across, tip, base + across];
    let fill = color::raised();
    painter.add(egui::Shape::convex_polygon(
        points.clone(),
        fill,
        Stroke::NONE,
    ));
    // The card's edge, on the two outer sides only, so the seam stays open.
    let edge = Stroke::new(1.0, color::light_blue());
    painter.line_segment([points[0], points[1]], edge);
    painter.line_segment([points[1], points[2]], edge);
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
                ..STEP
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
                ..STEP
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
                ..STEP
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
                ..STEP
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
                ..STEP
            },
            Step {
                anchor: "nowhere",
                text: |_| String::new(),
                done: |_| false,
                example: |_| {},
                reveal: None,
                ghost: None,
                ..STEP
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
        // A fresh effects folder holding the bundled ones, as tests have no real one.
        crate::effects::scan_for_test(&dir.join("effects"));
        for lesson in LESSONS.iter().chain([&BASICS]) {
            let mut h = harness_with(Harness::builder().wgpu(), &Default::default());
            h.state_mut().set_recognition_cache(Default::default());
            h.state_mut().export_dialog.set_path(dir.join("x.mp4"));
            h.state_mut().start_lesson(lesson);
            for (i, task) in lesson.steps.iter().enumerate() {
                settle(&mut h);
                assert_eq!(step(&h), Some(i), "{}: on task {i}", lesson.id);
                // The step's control, or the first action toward it.
                let vias = task.via.iter().map(|v| v.anchor);
                let mut targets = std::iter::once(task.anchor).chain(vias);
                assert!(
                    targets.any(|a| drawn(&h, a).is_some()),
                    "{} task {i}: `{}` and its vias never drawn",
                    lesson.id,
                    task.anchor
                );
                if drawn(&h, task.anchor).is_some() {
                    for anchor in task.watch {
                        assert!(
                            drawn(&h, anchor).is_some(),
                            "{} task {i}: `{anchor}` never drawn",
                            lesson.id
                        );
                    }
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

    /// Saves every lesson step as `<lesson>-<nn>.png` under `JVID_TUTORIAL_CAPTURE`, walking
    /// each lesson by its reference edits.
    #[test]
    fn capture_every_step() {
        let Some(out) = std::env::var_os("JVID_TUTORIAL_CAPTURE").map(PathBuf::from) else {
            return;
        };
        if !suite_ui::test_support::gpu() {
            return;
        }
        std::fs::create_dir_all(&out).unwrap();
        let dir = tmp("capture");
        crate::effects::scan_for_test(&dir.join("effects"));
        for lesson in LESSONS {
            let mut h = harness_with(Harness::builder().wgpu(), &Default::default());
            h.state_mut().set_recognition_cache(Default::default());
            h.state_mut().export_dialog.set_path(dir.join("x.mp4"));
            h.state_mut().start_lesson(lesson);
            for (i, task) in lesson.steps.iter().enumerate() {
                settle(&mut h);
                for _ in 0..20 {
                    h.step();
                }
                let image = h.render().unwrap();
                image
                    .save(out.join(format!("{}-{:02}.png", lesson.id, i + 1)))
                    .unwrap();
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
            }
            click(&mut h, tr!("tutorial.exit"));
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
            ..STEP
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
        for _ in 0..6 {
            perform(&mut h);
        }
        // Custom in the diamond's easing menu passes the easing step but not the graph steps.
        ease_first(h.state_mut(), Easing::Bezier([0.1, 0.0, 0.3, 1.0]));
        settle(&mut h);
        let done = |h: &Harness<'_, App>| h.state().lesson.as_ref().is_some_and(|r| r.done);
        assert_eq!(step(&h), Some(7));
        assert!(!done(&h));
        let id = first_clip(h.state()).unwrap();
        h.state_mut().timeline.open_graph(id);
        settle(&mut h);
        assert_eq!(step(&h), Some(8));
        assert!(
            done(&h),
            "the earlier custom curve counts once the graph is open"
        );
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
        h.key_press(Key::Escape);
        settle(&mut h);
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
                ..STEP
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

    #[test]
    fn ruler_guidance_tracks_its_time_through_zoom_and_resize_and_accepts_the_click() {
        let mut h = harness();
        h.state_mut().start_lesson(&KEYFRAMES);
        settle(&mut h);
        h.state_mut().lesson.as_mut().unwrap().go(2);
        settle(&mut h);
        let target = drawn(&h, "timeline.ruler").unwrap();
        let lanes = drawn(&h, "timeline.lanes").unwrap();
        let x = h.state().timeline.x_at(SECOND, lanes.left());
        assert!((target.center().x - x).abs() < 1.0);
        assert_eq!(target.width(), 24.0);

        // Zoom far enough to put the intended time outside the viewport; guidance reveals it.
        for _ in 0..20 {
            let a = h.state_mut();
            a.timeline.action("zoom_in", a.history.project());
        }
        h.set_size(vec2(960.0, 600.0));
        settle(&mut h);
        let target = drawn(&h, "timeline.ruler").unwrap();
        let lanes = drawn(&h, "timeline.lanes").unwrap();
        let x = h.state().timeline.x_at(SECOND, lanes.left());
        assert!((target.center().x - x).abs() < 1.0);
        assert!(lanes.x_range().contains(x));
        assert!(h.ctx.content_rect().contains_rect(card_bounds(&h)));
        assert!(!card_bounds(&h).intersects(target));

        let press = |h: &mut Harness<'_, App>, pos| {
            h.hover_at(pos);
            for pressed in [true, false] {
                h.event(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                });
            }
            settle(h);
        };
        press(&mut h, pos2(lanes.right() - 2.0, target.center().y));
        assert_eq!(
            h.state().timeline.playhead(),
            Time(0),
            "unlit ruler took a click"
        );
        assert_eq!(step(&h), Some(2));
        press(&mut h, target.center());
        assert_eq!(h.state().timeline.playhead(), SECOND);
        assert_eq!(step(&h), Some(3), "spotlit seek did not advance");
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
    fn a_step_spotlights_its_control_and_only_it_takes_clicks() {
        let mut h = harness();
        h.state_mut().start_lesson(&BASICS);
        settle(&mut h);
        let target = drawn(&h, "bin.titles").unwrap();
        let hole = target.expand(space::S4).intersect(h.ctx.content_rect());
        let shade = color::ground().gamma_multiply(SCRIM);
        let rects = |fill: Option<egui::Color32>| -> Vec<Rect> {
            let shapes = h.output().shapes.iter().filter_map(|s| match &s.shape {
                egui::Shape::Rect(r) if fill.is_none_or(|f| r.fill == f) => Some(r.clone()),
                _ => None,
            });
            shapes.map(|r| r.rect).collect()
        };
        let dim = rects(Some(shade));
        assert!(
            dim.iter().all(|r| !r.intersect(hole).is_positive()),
            "the target is dimmed"
        );
        let timeline = drawn(&h, "timeline").unwrap().center();
        assert!(dim.iter().any(|r| r.contains(timeline)), "the rest isn't");
        let outlined = h.output().shapes.iter().any(|s| {
            matches!(&s.shape,
            egui::Shape::Rect(r) if r.rect == hole && r.stroke.color == color::light_blue())
        });
        assert!(outlined);
        let card = card_bounds(&h);
        assert!(!card.intersects(target), "the card covers its target");
        assert!(h.ctx.content_rect().contains_rect(card));
        h.run_ok();
        assert_eq!(card_bounds(&h), card, "idle guidance moved");
        // A press outside the spotlight never reaches the menu.
        h.get_by_label(tr!("menu.file")).click();
        settle(&mut h);
        assert!(
            !egui::Popup::is_any_open(&h.ctx),
            "a click outside got through"
        );
        click(&mut h, crate::media_bin::presets()[0].0);
        assert_eq!(
            step(&h),
            Some(1),
            "the spotlit control didn't take the click"
        );
        // Without a lesson nothing is blocked.
        click(&mut h, tr!("tutorial.exit"));
        h.get_by_label(tr!("menu.file")).click();
        h.run_ok();
        assert!(egui::Popup::is_any_open(&h.ctx));
    }

    #[test]
    fn every_step_target_and_its_vias_are_drawn_and_spotlit() {
        // Each step's own target, then each via's while it's still needed.
        let mut h = harness();
        h.state_mut().start_lesson(&SCREEN);
        settle(&mut h);
        assert_eq!(step(&h), Some(0));
        assert!(h.query_by_label(tr!("tutorial.screen_silence")).is_some());
        assert!(drawn(&h, "clip").is_some(), "the via's clip isn't spotlit");
        open_silence(&mut h);
        assert!(h.query_by_label(tr!("tutorial.silence_cut")).is_some());
        assert!(h.query_by_label(tr!("tutorial.screen_silence")).is_none());
    }

    #[test]
    fn silence_guidance_follows_the_dialog_and_its_dismissal_at_960_by_600() {
        let mut h = screen_lesson();
        let pick = tr!("tutorial.screen_silence");
        assert!(h.query_by_label(pick).is_some());
        open_silence(&mut h);
        assert!(h.query_by_label(tr!("tutorial.silence_cut")).is_some());
        let card = card_bounds(&h);
        assert!(
            !card.intersects(drawn(&h, "appbar").unwrap()),
            "guidance covers app menus"
        );
        assert!(
            h.ctx.content_rect().contains_rect(card),
            "card outside viewport: {card:?}"
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
        assert!(
            h.query_by_label(pick).is_some(),
            "the step lost its first action"
        );
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
        let text = tr!("tutorial.speech_start", action = tr!("subtitles.generate"));
        assert!(h.query_by_label(&text).is_some());
        assert_eq!(
            step(&h),
            Some(0),
            "opening a model choice completed generation"
        );
        assert!(!card_bounds(&h).intersects(drawn(&h, "subtitles.action").unwrap()));
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
        assert!(!card_bounds(&h).intersects(drawn(&h, "silence.cut").unwrap()));
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
                    ..STEP
                },
                Step {
                    anchor: "preview",
                    text: |_| String::new(),
                    done: |_| false,
                    example: |_| {},
                    reveal: None,
                    ghost: None,
                    ..STEP
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
    fn the_card_sits_beside_its_target_and_flips_to_stay_on_screen() {
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 600.0));
        let size = vec2(CARD_W, 120.0);
        let at = |min: [f32; 2], s: [f32; 2]| {
            Rect::from_min_size(pos2(min[0], min[1]), vec2(s[0], s[1]))
        };
        // Room below: under it, pointing up, centred on it.
        let button = at([400.0, 100.0], [80.0, 24.0]);
        let (card, side) = card_rect(screen, size, Some(button), &[]);
        assert_eq!(side, Some(Side::Below));
        assert!(card.top() > button.bottom() && (card.center().x - button.center().x).abs() < 1.0);
        // At the bottom edge it flips above; at the right edge it slides left to stay on.
        let low = at([900.0, 560.0], [50.0, 24.0]);
        let (card, side) = card_rect(screen, size, Some(low), &[]);
        assert_eq!(side, Some(Side::Above));
        assert!(screen.shrink(space::S8).contains_rect(card) && card.bottom() < low.top());
        // A tall panel down the left: beside it.
        let panel = at([0.0, 0.0], [300.0, 600.0]);
        assert_eq!(
            card_rect(screen, size, Some(panel), &[]).1,
            Some(Side::Right)
        );
        // What to keep clear of pushes it to another side if one fits.
        let button = at([400.0, 300.0], [80.0, 24.0]);
        let watch = at([300.0, 330.0], [400.0, 150.0]);
        let (card, side) = card_rect(screen, size, Some(button), &[watch]);
        assert_eq!(side, Some(Side::Above));
        assert!(!card.intersects(watch) && !card.intersects(button));
        // The whole window: no side fits, so a corner, pointing nowhere.
        let (card, side) = card_rect(screen, size, Some(screen), &[]);
        assert_eq!(side, None);
        assert!(screen.contains_rect(card));
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
