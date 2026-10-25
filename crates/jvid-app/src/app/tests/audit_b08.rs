use super::*;
use crate::import::test_support::GatedProbes;
use crate::media_bin;
use jvid_core::model::ProbeInfo;
use jvid_core::time::Rate;

#[test]
fn audit_462_welcome_match_source_uses_input_order_despite_reverse_completion() {
    // Compare forward and reverse completion, both with and without non-video leaders.
    for leading_non_video in [false, true] {
        for reverse in [false, true] {
            let mut h = plain_harness();
            let mut names = Vec::new();
            if leading_non_video {
                names.extend(["broken.mp4", "audio.wav"]);
            }
            names.extend(["phone.mp4", "screen-1440p60.mp4"]);
            let mut gates = GatedProbes::new(&names);
            h.state_mut().importer = gates.importer();
            h.input_mut().dropped_files = gates
                .paths
                .iter()
                .chain([&gates.paths[0]]) // The duplicate must not acquire a new ordinal.
                .map(|path| std::sync::Arc::new(Dropped(path.clone())) as _)
                .collect();
            h.step();
            gates.capture_jobs(&h.state().importer);
            assert!(!h.state().on_welcome);
            assert!(h.state().match_source);
            assert!(h.state().importer.is_busy());
            assert!(h.state().history.project().assets().is_empty());

            let mut order: Vec<_> = (0..names.len()).collect();
            if reverse {
                order.reverse();
            }
            for (step, index) in order.into_iter().enumerate() {
                let result = match names[index] {
                    "broken.mp4" => Err("not media".into()),
                    "audio.wav" => Ok(ProbeInfo::default()),
                    _ => fake_probe(&gates.paths[index]),
                };
                gates.finish(index, result);
                h.step();
                if step + 1 < names.len() {
                    assert!(h.state().history.project().assets().is_empty());
                    assert!(h.state().match_source, "wait for the complete batch");
                }
            }
            let app = h.state_mut();
            assert!(!app.importer.is_busy());
            assert!(!app.match_source);
            let settings = app.history.project().settings();
            assert_eq!(
                (settings.width, settings.height, settings.frame_rate),
                (1080, 1920, Rate::new(30, 1)),
                "leading_non_video={leading_non_video}, reverse={reverse}"
            );
            let expected: Vec<_> = gates
                .paths
                .iter()
                .zip(&names)
                .filter(|(_, name)| **name != "broken.mp4")
                .map(|(path, _)| path)
                .collect();
            assert_eq!(
                media_bin::visible(app.history.project().assets(), "", 0)
                    .iter()
                    .map(|asset| &asset.path)
                    .collect::<Vec<_>>(),
                expected,
                "Added order is stable across probe schedules"
            );
            if leading_non_video {
                assert_eq!(app.toasts.len(), 1, "the failed probe reports an error");
            }
            assert!(app.history.undo(), "undo Match source settings");
            assert_eq!(app.history.project().assets().len(), expected.len());
            assert!(app.history.undo(), "one undo removes every imported asset");
            assert!(app.history.project().assets().is_empty());
            assert!(!app.history.undo(), "no per-file undo entries");
        }
    }
}
