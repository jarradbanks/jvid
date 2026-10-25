use super::*;
use crate::transport::tests::Fake;
use jvid_core::time::{FLICKS_PER_SECOND, Rate};

/// The strip is shared by Source and Program. A held drag used to submit the same seek
/// every UI pass; once playback left that frame it replayed its first samples indefinitely.
#[test]
fn held_seek_strip_does_not_replay_the_selected_audio_fragment() {
    let ctx = egui::Context::default();
    let fake = Fake::default();
    let mut output = Output::new(Box::new(fake.clone()), None);
    let mut transport = Transport::new(Time(12 * FLICKS_PER_SECOND), Rate::new(30, 1));
    transport.toggle(0);
    let mut pass = |events| {
        transport.tick(&mut output, &mut |start, buf| {
            for (i, pair) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                pair.fill((start + i as i64 + 1) as f32);
            }
        });
        let mut frame = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 100.0))),
                events,
                ..Default::default()
            },
            |ui| {
                let clock = output.clock();
                let row = SourceRow {
                    frame: transport.frame(clock),
                    end: 360,
                    rate: transport.rate,
                    playing: true,
                    in_point: None,
                    out_point: None,
                    duration: transport.duration,
                    position: transport.position(clock),
                };
                if let Some(time) = row.marks(
                    ui,
                    Rect::from_min_size(Pos2::ZERO, vec2(600.0, SEEK_HEIGHT)),
                ) {
                    // Source's same-frame guard; Program also follows the shared strip.
                    if transport.rate.index_floor(time) != transport.frame(clock) {
                        output.flush();
                        transport.seek(time, output.clock());
                    }
                }
            },
        );
        frame.textures_delta.clear();
        fake.pull(800)
    };
    pass(vec![]);
    let from = pos2(100.0, 8.0);
    pass(vec![
        egui::Event::PointerMoved(from),
        egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    for x in [200.0, 400.0, 100.0, 300.0] {
        pass(vec![egui::Event::PointerMoved(pos2(x, 8.0))]);
        // Leave the pointer held after each forward/backward jump while 1x play continues.
        let mut last = None;
        let mut heard = 0;
        for _ in 0..30 {
            for pair in pass(vec![]).as_chunks::<2>().0 {
                if pair[0] == 0.0 {
                    continue; // A flush may leave one callback silent until the next UI pass.
                }
                if let Some(last) = last {
                    assert_eq!(pair[0], last + 1.0, "a held pointer replayed old audio");
                }
                last = Some(pair[0]);
                heard += 1;
            }
        }
        assert_eq!(
            heard,
            30 * 800,
            "steady playback must not contain silent gaps"
        );
        assert_eq!(
            last.unwrap(),
            (x / 600.0 * 12.0 + 0.5) * jvid_audio::SAMPLE_RATE as f32
        );
    }
    let at = pos2(300.0, 8.0);
    for pressed in [false, true, false] {
        pass(vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }]);
    }
    assert_eq!(
        pass(vec![])[0],
        6.0 * jvid_audio::SAMPLE_RATE as f32 + 1.0,
        "a fresh click can revisit the previous gesture's target"
    );
}
