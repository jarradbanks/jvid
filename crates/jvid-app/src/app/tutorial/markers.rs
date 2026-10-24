//! Markers and highlights: mark moments while watching, jump between them, name one, then
//! export them as highlight clips.

use jvid_core::history::Command;
use jvid_core::model::Marker;
use jvid_core::time::{FLICKS_PER_SECOND, Time};

use super::{App, Lesson, Step, apply};
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
            anchor: "preview",
            text: |a| {
                let (play, key) = (
                    a.keymap.shortcut("play_stop"),
                    a.keymap.shortcut("add_marker"),
                );
                tr!("tutorial.markers_tap", play = play, key = key)
            },
            done: |a| markers(a).any(|m| m.duration == Time(0)),
            show_me: |a| {
                for s in [2, 5] {
                    add_marker(a, secs(s), Time(0));
                }
            },
            reveal: None,
            ghost: None,
        },
        Step {
            anchor: "preview",
            text: |a| {
                tr!(
                    "tutorial.markers_hold",
                    key = a.keymap.shortcut("add_marker")
                )
            },
            done: |a| markers(a).any(|m| m.duration > Time(0)),
            // Held from 8 s to 11 s while playing, leaving the playhead where M went up.
            show_me: |a| {
                add_marker(a, secs(8), secs(3));
                a.timeline.seek(secs(11));
            },
            reveal: None,
            ghost: None,
        },
        Step {
            anchor: "timeline",
            text: |a| {
                let (next, prev) = (
                    a.keymap.shortcut("next_marker"),
                    a.keymap.shortcut("prev_marker"),
                );
                tr!("tutorial.markers_jump", next = next, prev = prev)
            },
            done: |a| a.timeline.jumped,
            show_me: |a| {
                a.timeline.action("prev_marker", a.history.project());
            },
            reveal: None,
            ghost: None,
        },
        Step {
            anchor: "marker",
            text: |_| tr!("tutorial.markers_edit").into(),
            done: |a| markers(a).any(|m| !m.name.is_empty() && m.color != 0),
            show_me: |a| {
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
            reveal: None,
            ghost: None,
        },
        Step {
            anchor: "export",
            text: |_| tr!("tutorial.markers_export").into(),
            // A queued export as long as a point marker padded by Before and After: Highlights.
            done: |a| {
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
                a.queue.rows().iter().any(|r| lengths.contains(&r.duration))
            },
            show_me: |a| {
                let Some(gpu) = a.preview.gpu() else {
                    return;
                };
                let name = a.name();
                let seq = a.history.project().sequence();
                for req in a.export_dialog.highlights(seq, &name) {
                    a.queue_export(req, gpu.clone(), 0.0);
                }
            },
            reveal: None,
            ghost: None,
        },
    ],
};

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
