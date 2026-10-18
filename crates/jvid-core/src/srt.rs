//! SubRip (`.srt`) subtitles: parsing cues and writing them back out.

use crate::time::{FLICKS_PER_SECOND, Time};

/// One subtitle: `text` shown over `[start, end)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: Time,
    pub end: Time,
    pub text: String,
}

const MS: i64 = FLICKS_PER_SECOND / 1000;

/// The cues in `src`, sorted by start. Blocks without a valid timing line, or with an empty
/// or backwards span, are skipped, as players do.
pub fn parse(src: &str) -> Vec<Cue> {
    let src = src.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut cues: Vec<Cue> = src
        .split("\n\n")
        .filter_map(|block| {
            let mut lines = block.lines().skip_while(|l| !l.contains("-->"));
            let (a, b) = lines.next()?.split_once("-->")?;
            // Positioning hints may follow the end time.
            let b = b.split_whitespace().next()?;
            let (start, end) = (stamp(a)?, stamp(b)?);
            let text = lines.collect::<Vec<_>>().join("\n");
            (end > start && !text.trim().is_empty()).then_some(Cue { start, end, text })
        })
        .collect();
    cues.sort_by_key(|c| c.start);
    cues
}

/// `HH:MM:SS,mmm` (a `.` separator is accepted too).
fn stamp(s: &str) -> Option<Time> {
    let (hms, ms) = s.trim().split_once([',', '.'])?;
    let mut parts = hms.split(':').map(|n| n.trim().parse::<i64>().ok());
    let (h, m, sec) = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() || m >= 60 || sec >= 60 {
        return None;
    }
    let ms: i64 = ms.trim().parse().ok()?;
    Some(Time((((h * 60 + m) * 60 + sec) * 1000 + ms) * MS))
}

fn format(t: Time) -> String {
    let ms = t.0.max(0) / MS;
    let (h, m, s) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60);
    format!("{h:02}:{m:02}:{s:02},{:03}", ms % 1000)
}

/// An `.srt` of the cues overlapping `[from, to)`, clipped to it and shifted so `from` is 0.
pub fn write(cues: &[Cue], (from, to): (Time, Time)) -> String {
    let mut out = String::new();
    let shown = cues.iter().filter(|c| c.start < to && from < c.end);
    for (n, c) in shown.enumerate() {
        let (a, b) = (c.start.max(from) - from, c.end.min(to) - from);
        out += &format!("{}\n{} --> {}\n{}\n\n", n + 1, format(a), format(b), c.text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: i64) -> Time {
        Time(n * MS)
    }

    #[test]
    fn parses_blocks_and_skips_bad_ones() {
        let src = "\u{feff}1\r\n00:00:01,000 --> 00:00:02,500\r\nHello\r\nthere\r\n\r\n\
                   2\n00:00:03.000 --> 00:00:04,000 X1:10\nSecond\n\n\
                   3\nnot a time\nJunk\n\n\
                   4\n00:00:05,000 --> 00:00:05,000\nEmpty span\n";
        let cues = parse(src);
        assert_eq!(
            cues,
            [
                Cue {
                    start: ms(1000),
                    end: ms(2500),
                    text: "Hello\nthere".into()
                },
                Cue {
                    start: ms(3000),
                    end: ms(4000),
                    text: "Second".into()
                },
            ]
        );
    }

    #[test]
    fn writes_round_trip() {
        let cues = parse("1\n01:02:03,456 --> 01:02:04,000\nHi\n");
        assert_eq!(
            write(&cues, (Time(0), ms(9_999_999))),
            "1\n01:02:03,456 --> 01:02:04,000\nHi\n\n"
        );
        assert_eq!(parse(&write(&cues, (Time(0), ms(9_999_999)))), cues);
    }

    #[test]
    fn writes_only_the_range_shifted_to_zero() {
        let cue = |a, b, t: &str| Cue {
            start: ms(a),
            end: ms(b),
            text: t.into(),
        };
        let cues = [
            cue(0, 1000, "before"),
            cue(1500, 2500, "edge"),
            cue(3000, 4000, "in"),
            cue(9000, 9500, "after"),
        ];
        assert_eq!(
            write(&cues, (ms(2000), ms(5000))),
            "1\n00:00:00,000 --> 00:00:00,500\nedge\n\n2\n00:00:01,000 --> 00:00:02,000\nin\n\n"
        );
    }
}
