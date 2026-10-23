//! The editor's strings: `i18n/<code>.toml`, read through [`tr!`] and [`trn!`].
//! The language, fallback and catalog format live in [`suite_ui::i18n`].

pub use suite_ui::i18n::{Label, verbatim};

pub static CATALOG: suite_ui::i18n::Catalog = suite_ui::i18n::Catalog::new([
    include_str!("../i18n/en.toml"),
    include_str!("../i18n/es.toml"),
]);

/// `tr!("section.key")` is the current language's text; `tr!("key", name = value)` fills
/// its `{name}` placeholders. Falls back to English, then to the key.
macro_rules! tr {
    ($($args:tt)+) => {
        suite_ui::tr!($crate::i18n::CATALOG, $($args)+)
    };
}

/// `trn!("section.key", count)` is the plural form of a key for `count`, which fills
/// `{count}`; named arguments follow as for [`tr!`].
macro_rules! trn {
    ($($args:tt)+) => {
        suite_ui::trn!($crate::i18n::CATALOG, $($args)+)
    };
}

pub(crate) use {tr, trn};

/// Each text's width in the theme's body font, in points.
#[cfg(test)]
pub(crate) fn widths(texts: &[&str]) -> Vec<f32> {
    let ctx = egui::Context::default();
    suite_ui::theme::install(&ctx);
    let mut out = Vec::new();
    // The theme's fonts load on the first pass.
    for _ in 0..2 {
        let mut pass = ctx.run_ui(Default::default(), |ui| {
            let width = |t: &&str| {
                let g = ui.painter().layout_no_wrap(
                    t.to_string(),
                    suite_ui::theme::body(),
                    egui::Color32::WHITE,
                );
                g.size().x
            };
            out = texts.iter().map(width).collect();
        });
        pass.textures_delta.clear();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use suite_ui::tokens::{size, space};

    #[test]
    fn every_language_has_every_key_with_the_same_placeholders() {
        assert_eq!(CATALOG.mismatches(), Vec::<String>::new());
        // A copied-in English section would pass the check above; only words like Audio,
        // Error and `{value} MB` read the same in both.
        let es = CATALOG.keys("es");
        let same: Vec<_> = es
            .iter()
            .filter(|k| CATALOG.get_in("es", k) == CATALOG.get_in("en", k))
            .collect();
        assert!(same.len() * 10 < es.len(), "{same:?}");
    }

    #[test]
    fn every_language_fits_the_fixed_width_dropdowns() {
        // A dropdown's text starts S8 in and has to stop short of its chevron.
        let room = |w: f32| w - space::S8 - space::S4 - size::ICON - space::S4;
        // Key prefixes, by the width of the dropdown that lists them.
        let fixed = [
            ("preview.fit", room(88.0)),
            ("preview.full", room(72.0)),
            ("bin.sort_", room(96.0)),
            ("blend.", room(132.0)),
            ("size.custom", room(120.0)),
            ("size.vertical", room(120.0)),
            ("size.square", room(120.0)),
            ("preset.", room(260.0)),
            // The welcome screen's size cards, not a dropdown.
            ("size.", 160.0 - 2.0 * space::S8),
        ];
        let mut over = Vec::new();
        let mut check = |lang: &str, texts: &[&str], room: f32| {
            for (t, w) in texts.iter().zip(widths(texts)) {
                if w > room {
                    over.push(format!("{lang}: {t:?} is {w:.0}pt, {room:.0}pt fit"));
                }
            }
        };
        for lang in suite_ui::i18n::LANGUAGES {
            for (prefix, room) in fixed {
                let keys: Vec<&str> = CATALOG
                    .keys(lang)
                    .into_iter()
                    .filter(|k| k.starts_with(prefix))
                    .collect();
                assert!(!keys.is_empty(), "{prefix}");
                let texts: Vec<&str> = keys.iter().flat_map(|k| CATALOG.get_in(lang, k)).collect();
                check(lang, &texts, room);
            }
            // Settings: System (its language), then each language's own name.
            let system = CATALOG.get_in(lang, "settings.system").unwrap();
            let rows: Vec<String> = suite_ui::i18n::LANGUAGES
                .iter()
                .map(|l| suite_ui::i18n::autonym(l))
                .flat_map(|name| [system.replace("{language}", name), name.to_owned()])
                .collect();
            let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
            check(lang, &rows, room(200.0));
        }
        assert_eq!(over, Vec::<String>::new());
    }

    #[test]
    fn every_character_has_a_glyph() {
        let missing = suite_ui::test_support::missing_glyphs(&CATALOG);
        // Inter is subset to Latin-1 (see suite-ui/assets/README.md), so the `≈` before
        // estimated sizes has shown `?` since before translation. Drop this once it's added.
        let (known, other): (Vec<_>, Vec<_>) = missing.iter().partition(|m| m.ends_with("'≈'"));
        assert_eq!(other, Vec::<&String>::new());
        assert!(!known.is_empty(), "≈ draws now; remove the exception");
    }
}
