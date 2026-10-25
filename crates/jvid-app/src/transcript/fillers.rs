//! Remove Fillers: the um, uh and long pauses in the transcript, found from its words and the
//! gaps between them, and cut out in one ripple delete.

use std::collections::HashSet;

use jvid_core::history::Command;
use jvid_core::model::ClipId;
use jvid_core::time::{FLICKS_PER_SECOND as S, Time};
use jvid_transcribe::Word;

/// A gap this long between two words in one clip is a pause.
pub const PAUSE: Time = Time(S);
/// What's left of a pause once it's cut: this much after the word before and before the next.
const KEEP: Time = Time(S / 8);

/// Per whisper language: the initial prompt that keeps whisper writing fillers down, which it
/// otherwise leaves out, and the hesitation sounds counted as fillers. Only sounds: words such
/// as "so" and "like" are often meant.
const LANGS: [(&str, &str, &[&str]); 19] = [
    (
        "en",
        "Um, uh, so, like, I mean... er, hmm, you know.",
        &["um", "uh", "uhm", "er", "erm", "ah", "hm", "mm"],
    ),
    (
        "es",
        "Eh, este, o sea, mmm... pues, bueno.",
        &["eh", "em", "ehm", "mm"],
    ),
    (
        "fr",
        "Euh, bah, ben, hum... genre, tu vois.",
        &["euh", "heu", "hum", "mm"],
    ),
    (
        "de",
        "Äh, ähm, also, hm... halt, sozusagen.",
        &["äh", "ähm", "öh", "öhm", "hm", "mm"],
    ),
    (
        "it",
        "Ehm, eh, cioè, mmm... insomma, tipo.",
        &["eh", "ehm", "mm"],
    ),
    (
        "pt",
        "Hum, é, tipo, né... então, ahn.",
        &["hum", "ahn", "hã", "mm"],
    ),
    (
        "nl",
        "Eh, ehm, uh, nou... zeg maar, dus.",
        &["eh", "ehm", "uh", "uhm", "mm"],
    ),
    (
        "pl",
        "Yyy, eee, no, hmm... jakby, wiesz.",
        &["yy", "ee", "eem", "hm", "mm"],
    ),
    (
        "cs",
        "Ehm, eee, no, hm... jako, prostě.",
        &["ehm", "ee", "hm", "mm"],
    ),
    (
        "tr",
        "Ee, ıı, şey, hmm... yani, işte.",
        &["ee", "ıı", "hm", "mm"],
    ),
    (
        "ru",
        "Э-э, эм, ну, как бы... то есть, мм.",
        &["ээ", "эм", "мм", "хм"],
    ),
    (
        "uk",
        "Е-е, ем, ну, мм... тобто, як би.",
        &["ее", "ем", "мм", "хм"],
    ),
    (
        "sv",
        "Öh, eh, hm, alltså... liksom, typ.",
        &["öh", "eh", "hm", "mm"],
    ),
    (
        "da",
        "Øh, æh, hm, altså... ligesom, ikke?",
        &["øh", "æh", "hm", "mm"],
    ),
    (
        "no",
        "Eh, øh, hm, altså... liksom, ikke sant.",
        &["eh", "øh", "hm", "mm"],
    ),
    (
        "ca",
        "Eh, ehm, mmm, doncs... o sigui, bé.",
        &["eh", "ehm", "mm"],
    ),
    (
        "ja",
        "えー、あの、えっと、まあ、その…",
        &["えー", "えっと", "あのー"],
    ),
    ("zh", "嗯，呃，那个，就是，然后…", &["嗯", "呃"]),
    ("ko", "음, 어, 그, 저기, 뭐…", &["음", "어"]),
];

/// The prompt for whisper language `code`, English's for one not listed.
pub fn prompt(code: &str) -> &'static str {
    LANGS.iter().find(|l| l.0 == code).unwrap_or(&LANGS[0]).1
}

/// Every prompt, for the transcript cache key: one made with another prompt, or none, may be
/// missing fillers.
pub fn prompts() -> impl Iterator<Item = &'static str> {
    LANGS.iter().map(|l| l.1)
}

/// `s` with each run of one character as one: "ummm" is "um".
fn squeeze(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

/// Whether `text` is a filler in any language, drawn out or not ("Ummm," is "um"), but no
/// shorter than it ("e", Italian for "and", isn't "ee").
// shortcut: the cache doesn't keep the transcript's language, so every list is checked; keep
// the language with the words if one list's sound is another's word.
pub fn is_filler(text: &str) -> bool {
    let w: String = text
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    let (n, sq) = (w.chars().count(), squeeze(&w));
    let all = LANGS.iter().flat_map(|l| l.2.iter());
    !w.is_empty()
        && all
            .into_iter()
            .any(|f| n >= f.chars().count() && sq == squeeze(f))
}

/// A filler or a pause: the span to cut, in timeline time, and the word it's at in `words`
/// (a pause is after it).
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub clip: ClipId,
    pub start: Time,
    pub end: Time,
    pub at: usize,
    pub pause: bool,
}

/// The fillers and pauses in `words` (sorted, in timeline time), in order. A pause is cut down
/// to [`KEEP`] each side; one between clips is left, as the clips may not be next to each other.
pub fn find(words: &[(ClipId, Word)]) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (i, (c, w)) in words.iter().enumerate() {
        if is_filler(&w.text) {
            hits.push(Hit {
                clip: *c,
                start: w.start,
                end: w.end,
                at: i,
                pause: false,
            });
        }
        if let Some((n, next)) = words.get(i + 1)
            && n == c
            && next.start - w.end >= PAUSE
        {
            let (start, end) = (w.end + KEEP, next.start - KEEP);
            hits.push(Hit {
                clip: *c,
                start,
                end,
                at: i,
                pause: true,
            });
        }
    }
    hits
}

/// Ripple deletes `hits` but those starting at `unticked`, as one undo step.
pub fn cut(hits: &[Hit], unticked: &HashSet<Time>) -> Option<Command> {
    let ticked = hits.iter().filter(|h| !unticked.contains(&h.start));
    let spans: Vec<_> = ticked.map(|h| (h.clip, h.start, h.end)).collect();
    (!spans.is_empty()).then_some(Command::CutSpans { spans, speed: None })
}

/// The words around `hit`, with the filler or `pause` in their place.
pub fn context(words: &[(ClipId, Word)], hit: &Hit, pause: &str) -> String {
    let text = |r: std::ops::Range<usize>| {
        let r = r.start.min(words.len())..r.end.min(words.len());
        words[r]
            .iter()
            .map(|(_, w)| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let (before, mid, after) = match hit.pause {
        true => (
            text(hit.at.saturating_sub(2)..hit.at + 1),
            pause.to_owned(),
            text(hit.at + 1..hit.at + 4),
        ),
        false => (
            text(hit.at.saturating_sub(3)..hit.at),
            words[hit.at].1.text.clone(),
            text(hit.at + 1..hit.at + 4),
        ),
    };
    [before, mid, after]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: f64) -> Time {
        Time((s * S as f64) as i64)
    }

    #[test]
    fn fillers_are_hesitation_sounds_drawn_out_or_not() {
        for f in [
            "um", "Um,", "Ummm...", "uh", "Hmm.", "euh", "ähm", "Э-э,", "Yyy", "嗯",
        ] {
            assert!(is_filler(f), "{f}");
        }
        for w in ["umbrella", "so", "like", "e", "y", "m", "", "...", "Hello,"] {
            assert!(!is_filler(w), "{w}");
        }
        assert_eq!(prompt("de"), LANGS[3].1);
        assert_eq!(prompt("xx"), LANGS[0].1);
    }

    #[test]
    fn a_pause_between_clips_is_left() {
        let w = |c, text: &str, s| {
            (
                ClipId(c),
                Word {
                    text: text.into(),
                    start: t(s),
                    end: t(s + 0.5),
                    confidence: 1.0,
                },
            )
        };
        let words = [w(1, "a", 0.0), w(2, "b", 3.0)];
        assert!(find(&words).is_empty());
        let words = [w(1, "a", 0.0), w(1, "b", 3.0)];
        let [hit] = &find(&words)[..] else { panic!() };
        assert_eq!((hit.start, hit.end), (t(0.625), t(2.875)));
        assert_eq!(context(&words, hit, "[3 s]"), "a [3 s] b");
    }
}
