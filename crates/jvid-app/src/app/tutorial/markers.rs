//! Markers and highlights: mark moments while watching, jump between them, name one, then
//! export them as highlight clips.

use jvid_core::history::Command;
use jvid_core::model::Marker;
use jvid_core::time::{FLICKS_PER_SECOND, Time};

use super::{App, Lesson, STEP, Step, Via, apply};
use crate::i18n::tr;

/// The clips the markers lesson plays, end to end.
const MARKERS_MEDIA: &[&str] = &["subject.mp4", "green-screen.mp4", "dark-warm.mp4"];

/// Mark moments while watching, then export them as highlight clips.
pub const MARKERS: Lesson = Lesson {
    id: "markers",
    title: || tr!("tutorial.markers"),
    minutes: 5,
    media: MARKERS_MEDIA,
    setup: |a| {
        for name in MARKERS_MEDIA {
            let project = a.history.project();
            let end = project.sequence().duration();
            let asset = project.assets().iter().find(|x| x.path.ends_with(name));
            if let Some(p) = asset.and_then(|x| project.placement(x.id, end, false, None, false)) {
                apply(a, Command::Place(p));
            }
        }
    },
    steps: &[
        Step {
            anchor: "preview.play",
            text: |_| tr!("tutorial.markers_play").into(),
            done: |a| a.preview.playing() || markers(a).next().is_some(),
            example: |a| {
                a.preview_active = 1;
                a.preview.program_action("play_stop");
            },
            ..STEP
        },
        Step {
            anchor: "timeline.markers",
            text: |a| {
                tr!(
                    "tutorial.markers_tap",
                    key = a.keymap.shortcut("add_marker")
                )
            },
            done: |a| markers(a).any(|m| m.duration == Time(0)),
            example: |a| {
                for s in [2, 5] {
                    add_marker(a, secs(s), Time(0));
                }
            },
            ..STEP
        },
        Step {
            anchor: "timeline.markers",
            text: |a| {
                tr!(
                    "tutorial.markers_hold",
                    key = a.keymap.shortcut("add_marker")
                )
            },
            done: |a| markers(a).any(|m| m.duration > Time(0)),
            // Held from 8 s to 11 s while playing, leaving the playhead where M went up.
            example: |a| {
                add_marker(a, secs(8), secs(3));
                a.timeline.seek(secs(11));
            },
            ..STEP
        },
        Step {
            anchor: "timeline.markers",
            text: |a| {
                let (next, prev) = (
                    a.keymap.shortcut("next_marker"),
                    a.keymap.shortcut("prev_marker"),
                );
                tr!("tutorial.markers_jump", next = next, prev = prev)
            },
            done: |a| a.timeline.jumped,
            example: |a| {
                a.timeline.action("prev_marker", a.history.project());
            },
            ..STEP
        },
        Step {
            anchor: "marker",
            text: |_| tr!("tutorial.markers_edit").into(),
            done: |a| markers(a).any(|m| !m.name.is_empty() && m.color != 0),
            example: |a| {
                let Some(m) = markers(a).next() else {
                    return;
                };
                let (name, color) = (tr!("tutorial.markers_name").into(), 2);
                let m = Marker {
                    name,
                    color,
                    ..m.clone()
                };
                apply(a, Command::SetMarker(m));
            },
            ..STEP
        },
        Step {
            anchor: "export.action",
            text: |_| tr!("tutorial.markers_export").into(),
            done: highlights_queued,
            example: |a| {
                let Some(gpu) = a.preview.gpu() else {
                    return;
                };
                let name = a.name();
                let seq = a.history.project().sequence();
                let reqs = a.export_dialog.highlights(seq, &name);
                for req in reqs {
                    a.queue_export(req, gpu.clone(), 0.0);
                }
            },
            via: &[
                super::OPEN_EXPORT,
                Via {
                    anchor: "export.range",
                    item: None,
                    text: |_| tr!("tutorial.markers_highlights").into(),
                    ready: |a, _| a.export_dialog.is_highlights(),
                },
            ],
            ..STEP
        },
    ],
};

/// Whether an export as long as a point marker padded by Before and After is queued:
/// Highlights.
fn highlights_queued(a: &App) -> bool {
    let (pad, end) = (
        a.export_dialog.pad(),
        a.history.project().sequence().duration(),
    );
    let points = markers(a).filter(|m| m.duration == Time(0));
    let lengths: Vec<_> = points
        .map(|m| {
            let (from, to) = crate::export::dialog::highlight(m, pad, end);
            to - from
        })
        .collect();
    super::queued(a).any(|d| lengths.contains(&d))
}

fn markers(app: &App) -> std::slice::Iter<'_, Marker> {
    app.history.project().sequence().markers().iter()
}

fn add_marker(app: &mut App, time: Time, duration: Time) {
    let name = String::new();
    apply(
        app,
        Command::AddMarker {
            time,
            duration,
            name,
        },
    );
}

fn secs(s: i64) -> Time {
    Time(s * FLICKS_PER_SECOND)
}
