//! Every lesson step, walked by its reference edits at the default and minimum window sizes:
//! each spotlights a control that's drawn, and its card stays on screen without covering it.

use egui::Key;

use super::*;

/// Runs frames until imports and the lesson's own frame have settled.
fn settle(h: &mut Harness<'_, App>) {
    for _ in 0..500 {
        h.run_ok();
        if !h.state().importer.is_busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for _ in 0..4 {
        h.step();
    }
}

fn step(h: &Harness<'_, App>) -> Option<usize> {
    h.state().lesson.as_ref().map(|r| r.step)
}

#[test]
fn every_tutorial_step_spotlights_a_drawn_control_beside_its_card() {
    if !gpu() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    crate::effects::scan_for_test(&dir.path().join("effects"));
    for size in [[1280.0, 720.0], [960.0, 600.0]] {
        for lesson in tutorial::LESSONS {
            let mut h = harness(size, 1.0);
            let app = h.state_mut();
            app.set_recognition_cache(Default::default());
            app.export_dialog.set_path(dir.path().join("x.mp4"));
            app.start_lesson(lesson);
            for (i, task) in lesson.steps.iter().enumerate() {
                settle(&mut h);
                let at = format!("{} step {} at {size:?}", lesson.id, i + 1);
                assert_eq!(step(&h), Some(i), "{at}");
                let screen = h.ctx.content_rect();
                let (hole, card) = tutorial::spotlit(&h.ctx).unwrap_or_else(|| {
                    panic!("{at}: nothing spotlit");
                });
                assert!(screen.intersects(hole), "{at}: target {hole:?} off screen");
                assert!(screen.contains_rect(card), "{at}: card {card:?} off screen");
                assert!(!card.intersects(hole), "{at}: card covers its target");
                if task.anchor == "timeline.ruler" {
                    assert!(
                        tutorial::ruler_time(&h.ctx).is_some(),
                        "{at}: no seek target"
                    );
                    // The hole includes 4 points of outline padding on each side.
                    assert!(hole.width() <= 32.0, "{at}: whole ruler highlighted");
                    assert!(
                        (card.center().x - hole.center().x).abs() <= card.width() / 2.0,
                        "{at}: card too far from seek target"
                    );
                }
                let run = h.state_mut().lesson.as_mut().unwrap();
                // Needs a microphone.
                if task.anchor == "record" {
                    run.step += 1;
                    continue;
                }
                let fake = FakeTranscriber {
                    slow: false,
                    asked: Default::default(),
                };
                let fake: std::sync::Arc<dyn jvid_transcribe::Transcriber> =
                    std::sync::Arc::new(fake);
                h.state_mut().subtitles.engine = Some((jvid_transcribe::models::Model::Base, fake));
                (task.example)(h.state_mut());
                for _ in 0..300 {
                    // Rendering runs the Program view, which the scope steps read.
                    h.step();
                    h.render().unwrap();
                    if step(&h) != Some(i) {
                        break;
                    }
                    if h.state().lesson.as_ref().unwrap().done {
                        h.key_press(Key::Enter);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                assert_eq!(step(&h), Some(i + 1), "{at} didn't advance");
            }
            // Stops the lesson's playback: dropping the app mid-playback can stall its pool.
            h.state_mut().end_lesson();
            h.run_ok();
        }
    }
}
