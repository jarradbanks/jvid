//! Color and Scopes: fix a dark, too-warm shot by reading the scopes, then apply a look. The
//! scope steps check the Scopes tab's own counts, not the sliders.

use jvid_core::history::Command;
use jvid_core::model::{Clip, Transform};
use jvid_core::time::Time;
use jvid_render::scope::{Analysis, VECTOR};

use super::{App, Lesson, Step, apply, sample};
use crate::i18n::{tr, verbatim};

const CLIP: &str = "dark-warm.mp4";
const LOOK: &str = "look.cube";
/// The Media panel's Scopes tab.
const SCOPES: usize = 3;
/// The waveform's brightest parts: the luma code the top 1% of pixels sit above.
const BRIGHTEST: f64 = 0.99;
/// The brightest parts are near 100% from 85% and flatten against it past 97%.
const NEAR: usize = 217;
const FLAT: usize = 247;
/// Share of pixels in the vectorscope's centre cells once something grey sits there; about
/// 1% with the cast.
const CENTRED: f64 = 0.02;
/// Most the R, G and B histograms' top ends may differ by to line up, in code values.
const LINED_UP: usize = 10;

pub const COLOUR: Lesson = Lesson {
    id: "color",
    title: || tr!("tutorial.color"),
    minutes: 5,
    media: &[CLIP],
    setup: |a| {
        let project = a.history.project();
        let Some(asset) = project.assets().first() else {
            return;
        };
        if let Some(p) = project.placement(asset.id, Time(0), false, None, false) {
            apply(a, Command::Place(p));
        }
        if let Some(c) = clip(a).map(|c| c.id) {
            a.timeline.select_clip(a.history.project().sequence(), c);
        }
    },
    steps: &[
        Step {
            anchor: "scope.waveform",
            text: |_| tr!("tutorial.color_exposure").into(),
            done: |a| a.preview.analysis.as_ref().is_some_and(exposed),
            show_me: |a| grade(a, |x| x.exposure = 1.0),
            reveal: Some(open_scopes),
            ghost: None,
            also: &["inspector.exposure"],
        },
        Step {
            anchor: "scope.vectorscope",
            text: |_| tr!("tutorial.color_balance").into(),
            done: |a| a.preview.analysis.as_ref().is_some_and(centred),
            show_me: |a| grade(a, |x| (x.temperature, x.tint) = (-0.4, 0.0)),
            reveal: Some(open_scopes),
            ghost: None,
            also: &["inspector.temperature", "inspector.tint"],
        },
        Step {
            anchor: "scope.histogram",
            text: |_| tr!("tutorial.color_histogram").into(),
            done: |a| a.preview.analysis.as_ref().is_some_and(lined_up),
            show_me: |a| grade(a, |x| (x.temperature, x.tint) = (-0.7, -0.25)),
            reveal: Some(open_scopes),
            ghost: None,
            also: &["inspector.temperature", "inspector.tint"],
        },
        Step {
            anchor: "inspector.lut",
            text: |_| {
                let none = tr!("inspector.lut_none");
                tr!("tutorial.color_look", none = none, file = verbatim(LOOK))
            },
            done: |a| {
                clip(a)
                    .is_some_and(|c| c.transform.lut.is_some() && c.transform.lut_intensity < 1.0)
            },
            show_me: |a| match sample(LOOK) {
                Ok(path) => grade(a, |x| {
                    x.lut = Some(path.to_string_lossy().into());
                    x.lut_intensity = 0.5;
                }),
                Err(e) => a.error(tr!("app.open_failed", path = verbatim(LOOK), error = e)),
            },
            reveal: None,
            ghost: None,
            also: &["preview"],
        },
    ],
};

fn open_scopes(app: &mut App) {
    app.bin.tab = SCOPES;
}

/// The lesson's clip on the timeline.
fn clip(app: &App) -> Option<&Clip> {
    let seq = app.history.project().sequence();
    seq.tracks().iter().find_map(|t| t.clips().first())
}

/// Selects the clip and changes its color as the Inspector does.
fn grade(app: &mut App, f: impl FnOnce(&mut Transform)) {
    let Some(c) = clip(app) else { return };
    let (id, mut x, keys) = (c.id, c.transform.clone(), c.keyframes.clone());
    f(&mut x);
    app.timeline
        .select_clip(app.history.project().sequence(), id);
    apply(app, Command::SetClipProps(vec![(id, x, keys)]));
}

/// The code value `share` of the pixels counted in `counts` sit at or below.
fn percentile(counts: &[u32], share: f64) -> usize {
    let total: u64 = counts.iter().map(|&n| n as u64).sum();
    let want = (total as f64 * share).ceil() as u64;
    let mut seen = 0;
    counts
        .iter()
        .position(|&n| {
            seen += n as u64;
            seen >= want.max(1)
        })
        .unwrap_or(0)
}

/// The waveform's brightest parts are near 100% without flattening against it.
fn exposed(a: &Analysis) -> bool {
    let w = a.width.max(1);
    let levels: Vec<u32> = a.waveform.chunks(w).map(|row| row.iter().sum()).collect();
    (NEAR..=FLAT).contains(&percentile(&levels, BRIGHTEST))
}

/// Something grey sits in the vectorscope's centre: its 3x3 centre cells hold enough pixels.
fn centred(a: &Analysis) -> bool {
    let c = VECTOR / 2;
    let cells = (c - 1..=c + 1).flat_map(|y| (c - 1..=c + 1).map(move |x| y * VECTOR + x));
    let n: u32 = cells.map(|i| a.vectorscope[i]).sum();
    n as f64 >= CENTRED * a.pixels.max(1) as f64
}

/// The R, G and B histograms' top ends line up, none pinned at 255.
fn lined_up(a: &Analysis) -> bool {
    let tops = a.histogram.each_ref().map(|h| percentile(h, BRIGHTEST));
    let (lo, hi) = (tops.iter().min(), tops.iter().max());
    matches!((lo, hi), (Some(&lo), Some(&hi)) if hi < 255 && hi - lo <= LINED_UP)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scopes of `pixels`, solid RGB colours each `n` times, side by side.
    fn analysis(pixels: &[([u8; 3], usize)]) -> Analysis {
        let rgba: Vec<u8> = pixels
            .iter()
            .flat_map(|&(rgb, n)| std::iter::repeat_n([rgb[0], rgb[1], rgb[2], 255], n))
            .flatten()
            .collect();
        jvid_render::scope::analyze(&rgba, rgba.len() / 4)
    }

    #[test]
    fn percentile_finds_the_code_a_share_sits_at_or_below() {
        let mut counts = [0; 256];
        (counts[10], counts[200]) = (99, 1);
        assert_eq!(percentile(&counts, 0.99), 10);
        assert_eq!(percentile(&counts, 1.0), 200);
        assert_eq!(percentile(&[0; 256], 0.99), 0);
    }

    #[test]
    fn exposed_wants_the_brightest_near_100_but_not_flat() {
        let with_top = |top: u8| analysis(&[([60; 3], 98), ([top; 3], 2)]);
        assert!(!exposed(&with_top(180)), "too dark");
        assert!(exposed(&with_top(230)));
        assert!(!exposed(&with_top(255)), "clipped");
    }

    #[test]
    fn centred_wants_grey_in_the_middle_of_the_vectorscope() {
        let orange = ([200, 120, 40], 97);
        assert!(!centred(&analysis(&[orange])));
        assert!(centred(&analysis(&[orange, ([128; 3], 3)])));
    }

    #[test]
    fn lined_up_wants_the_channels_top_ends_together_and_unclipped() {
        let base = ([40, 60, 30], 98);
        assert!(lined_up(&analysis(&[base, ([230, 226, 232], 2)])));
        assert!(!lined_up(&analysis(&[base, ([250, 220, 200], 2)])), "warm");
        assert!(!lined_up(&analysis(&[base, ([255; 3], 2)])), "clipped");
    }
}
