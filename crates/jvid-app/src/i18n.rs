//! The editor's strings: `i18n/<code>.toml`, read through [`tr!`] and [`trn!`].
//! The language, fallback and catalog format live in [`suite_ui::i18n`].

pub use suite_ui::i18n::{Label, verbatim};

pub static CATALOG: suite_ui::i18n::Catalog = suite_ui::i18n::Catalog::new([
    include_str!("../i18n/en.toml"),
    include_str!("../i18n/es.toml"),
    include_str!("../i18n/fr.toml"),
    include_str!("../i18n/de.toml"),
    include_str!("../i18n/it.toml"),
    include_str!("../i18n/pt-BR.toml"),
    include_str!("../i18n/nl.toml"),
    include_str!("../i18n/pl.toml"),
    include_str!("../i18n/cs.toml"),
    include_str!("../i18n/tr.toml"),
    include_str!("../i18n/ru.toml"),
    include_str!("../i18n/uk.toml"),
    include_str!("../i18n/sv.toml"),
    include_str!("../i18n/da.toml"),
    include_str!("../i18n/nb.toml"),
    include_str!("../i18n/ca.toml"),
    include_str!("../i18n/ja.toml"),
    include_str!("../i18n/zh-Hans.toml"),
    include_str!("../i18n/ko.toml"),
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn every_character_has_a_glyph() {
        let missing = suite_ui::test_support::missing_glyphs(&CATALOG);
        assert_eq!(missing, Vec::<String>::new());
    }
}
