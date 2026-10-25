use super::*;
use crate::subtitles::{AudioSources, targets};
use egui_kittest::kittest::Queryable;
use jvid_core::time::{FLICKS_PER_SECOND as S, Time};
use jvid_transcribe::{Source, Transcript, Word};
use std::collections::HashSet;

#[test]
fn audit_442_caption_and_transcript_sources_agree_with_selected_microphone() {
    for mic_first in [false, true] {
        let (p, mic, game) = crate::subtitles::test_support::two_streams(mic_first);
        let selected = HashSet::from([mic]);
        let caption = targets(&p, &selected, false);
        assert_eq!(caption.len(), 1);
        assert_eq!((caption[0].clip.id, caption[0].source.nth), (mic, 1));
        assert_eq!(caption[0].source.range, 48_000..240_000);
        let mut panel = crate::transcript::TranscriptPanel::default();
        let missing = panel.missing(&p, &selected);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].nth, 1);
        // The fake source has no speech on Game and one word on Mic.
        let speech = Word {
            text: "Microphone".into(),
            start: Time(2 * S),
            end: Time(3 * S),
            confidence: 1.0,
        };
        for (nth, words) in [(0, vec![]), (1, vec![speech.clone()])] {
            panel.cache.insert(
                &Source {
                    path: p.assets()[0].path.clone(),
                    nth,
                    range: 0..i64::MAX,
                },
                Transcript {
                    language: Some("en".into()),
                    words,
                },
            );
        }
        assert!(panel.missing(&p, &selected).is_empty());
        let Some(Command::CutSpans { spans, .. }) = panel.cut(&p, &selected, 0..1) else {
            panic!()
        };
        assert_eq!(spans, [(mic, Time(3 * S), Time(4 * S))]);
        assert_eq!(
            caption[0].place(&[speech]).next().unwrap().text,
            "Microphone"
        );
        assert_ne!(game, mic);
    }
}

#[test]
fn audit_442_whole_group_source_is_visible_and_changeable() {
    let (p, mic, game) = crate::subtitles::test_support::two_streams(false);
    let selected: HashSet<_> = p.sequence().linked(mic).iter().map(|(_, c)| c.id).collect();
    let mut fresh = true;
    let mut h = Harness::builder().with_size([480.0, 220.0]).build_ui_state(
        move |ui, sources: &mut AudioSources| {
            if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                sources.show(ui, p.sequence(), &selected, false);
            }
        },
        AudioSources::default(),
    );
    h.run();
    h.get_by_label("Game · Stream 1").click();
    h.run();
    h.get_by_label("Mic · Stream 2").click();
    h.run();
    assert!(h.query_by_label("Mic · Stream 2").is_some());
    let (p, _, _) = crate::subtitles::test_support::two_streams(false);
    let all = p.sequence().linked(mic).iter().map(|(_, c)| c.id).collect();
    assert_eq!(h.state().clips(p.sequence(), &all, false)[0].id, mic);
    assert_eq!(
        h.state().clips(p.sequence(), &HashSet::from([game]), false)[0].id,
        game,
        "an explicit selection overrides a previous whole-group preference"
    );
}
