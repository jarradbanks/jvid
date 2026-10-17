//! Hot paths behind every preview frame: timeline evaluation and keyframe interpolation.
//! Budgets: docs/perf.md. Run with `cargo bench -p jvid-core`.

use criterion::{Criterion, criterion_group, criterion_main};
use jvid_core::eval::video_at;
use jvid_core::history::{Command, Created, History};
use jvid_core::keyframe::{Easing, Keyframe, Value, value_at};
use jvid_core::model::{NewClip, Project, TrackKind};
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use std::hint::black_box;

const SEC: i64 = FLICKS_PER_SECOND;

/// 8 video tracks of 500 back-to-back 1s clips, each with a keyframed opacity.
fn project() -> History {
    let mut h = History::new(Project::default());
    let Ok(Created::Asset(asset)) = h.apply(Command::AddAsset("a.mp4".into())) else {
        panic!("asset")
    };
    for n in 0..8 {
        let name = format!("V{n}");
        let Ok(Created::Track(track)) = h.apply(Command::AddTrack {
            kind: TrackKind::Video,
            name,
        }) else {
            panic!("track")
        };
        for i in 0..500 {
            let clip = NewClip {
                asset,
                source_in: Time(0),
                source_out: Time(SEC),
                start: Time(i * SEC),
                link: None,
            };
            let Ok(Created::Clip(id)) = h.apply(Command::AddClip { track, clip }) else {
                panic!("clip")
            };
            for (t, v) in [(0, 0.0), (SEC / 2, 1.0)] {
                let key = Keyframe {
                    time: Time(t),
                    value: Value::Scalar(v),
                    easing: Easing::EaseInOut,
                };
                h.apply(Command::AddKeyframe {
                    id,
                    param: "opacity".into(),
                    key,
                })
                .unwrap();
            }
        }
    }
    h
}

fn benches(c: &mut Criterion) {
    let h = project();
    let seq = h.project().sequence();
    c.bench_function("video_at 8x500 clips", |b| {
        b.iter(|| video_at(seq, black_box(Time(250 * SEC + SEC / 3))))
    });
    let keys: Vec<Keyframe> = (0..1000)
        .map(|i| Keyframe {
            time: Time(i * SEC),
            value: Value::Vec2([i as f64, -(i as f64)]),
            easing: Easing::Bezier([0.25, 0.1, 0.25, 1.0]),
        })
        .collect();
    c.bench_function("keyframe value_at 1000 keys", |b| {
        b.iter(|| value_at(&keys, black_box(Time(512 * SEC + SEC / 3))))
    });
}

criterion_group!(core, benches);
criterion_main!(core);
