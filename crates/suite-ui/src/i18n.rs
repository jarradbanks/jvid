//! UI language and translation catalogs.
//!
//! Each crate keeps its strings in `i18n/<code>.toml`, one file per entry in [`LANGUAGES`],
//! embedded into a [`Catalog`] static and read through that crate's `tr!` macro.
//! [`set_language`] switches every catalog at once; egui redraws from the catalogs each frame,
//! so a switch shows on the next frame. Text missing from a language falls back to English,
//! then to the key itself. See docs/translating.md.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Shipped languages by BCP 47 code: the primary subtag, but for Brazilian Portuguese and
/// Simplified Chinese. English is first: the fallback and the source.
pub const LANGUAGES: [&str; 19] = [
    "en", "es", "fr", "de", "it", "pt-BR", "nl", "pl", "cs", "tr", "ru", "uk", "sv", "da", "nb",
    "ca", "ja", "zh-Hans", "ko",
];

/// The `language` setting that follows the operating system.
pub const SYSTEM: &str = "system";

static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// The language every catalog reads from, from [`LANGUAGES`].
pub fn language() -> &'static str {
    LANGUAGES[CURRENT.load(Ordering::Relaxed)]
}

/// Switches the UI language; an unsupported code switches to English.
pub fn set_language(code: &str) {
    let i = LANGUAGES.iter().position(|l| *l == code).unwrap_or(0);
    CURRENT.store(i, Ordering::Relaxed);
}

/// The shipped language for a locale such as `es-MX`, `es_ES.UTF-8` or `EN`, if any.
pub fn supported(locale: &str) -> Option<&'static str> {
    // `es_ES.UTF-8@euro`: the encoding and modifier say nothing about the language.
    let lower = locale.split(['.', '@']).next()?.to_ascii_lowercase();
    let mut tags = lower.split(['-', '_']);
    let primary = match tags.next()? {
        // Chinese is Simplified unless the locale says Traditional or a region that writes it.
        "zh" if tags.any(|t| matches!(t, "hant" | "tw" | "hk" | "mo")) => return None,
        "zh" => "zh-Hans",
        // Only Brazilian Portuguese ships; Portugal's (and Africa's) spelling differs.
        "pt" if tags.next().is_some_and(|t| t != "br") => return None,
        "pt" => "pt-BR",
        // `no` is Norwegian as a whole, which writes Bokmål by default.
        "no" => "nb",
        p => p,
    };
    LANGUAGES.into_iter().find(|l| *l == primary)
}

/// The first shipped language among `locales` (most preferred first), else English.
pub fn negotiate<S: AsRef<str>>(locales: impl IntoIterator<Item = S>) -> &'static str {
    locales
        .into_iter()
        .find_map(|l| supported(l.as_ref()))
        .unwrap_or(LANGUAGES[0])
}

/// The operating system's preferred shipped language, else English.
pub fn system_language() -> &'static str {
    negotiate(sys_locale::get_locales())
}

/// A shipped language's name in that language ("Español"), for language pickers.
pub fn autonym(code: &str) -> &str {
    CATALOG.get_in(code, "language.name").unwrap_or(code)
}

/// The language a `language` setting means: [`SYSTEM`] (or empty) asks the operating system,
/// a shipped language (`es`, or a locale of one such as `es-MX`) is that language, and
/// anything else is English.
pub fn resolve(setting: &str) -> &'static str {
    match setting.trim() {
        "" | SYSTEM => system_language(),
        code => supported(code).unwrap_or(LANGUAGES[0]),
    }
}

/// One crate's strings in every shipped language.
pub struct Catalog {
    /// TOML sources in [`LANGUAGES`] order.
    sources: [&'static str; LANGUAGES.len()],
    tables: OnceLock<Vec<HashMap<String, String>>>,
}

impl Catalog {
    pub const fn new(sources: [&'static str; LANGUAGES.len()]) -> Self {
        Self {
            sources,
            tables: OnceLock::new(),
        }
    }

    /// Flattened `section.key` tables, parsed on first use. A source that doesn't parse
    /// reads as empty, so its language falls back to English (see [`Catalog::errors`]).
    fn tables(&self) -> &[HashMap<String, String>] {
        self.tables.get_or_init(|| {
            self.sources
                .iter()
                .map(|s| parse(s).unwrap_or_default())
                .collect()
        })
    }

    /// Every TOML or shape error in the sources, for tests.
    pub fn errors(&self) -> Vec<String> {
        LANGUAGES
            .iter()
            .zip(self.sources)
            .filter_map(|(l, s)| parse(s).err().map(|e| format!("{l}: {e}")))
            .collect()
    }

    /// Where each translation is out of step with English, for tests: a key missing or
    /// extra, a placeholder English doesn't have or that it drops, a plural without `other`.
    /// Plural forms (`key.one`, `key.many`, `key.other`) are checked as one group, since a
    /// language may have forms English doesn't and `{count}` is optional in each.
    pub fn mismatches(&self) -> Vec<String> {
        let mut out = self.errors();
        let english = LANGUAGES[0];
        fn base(key: &str) -> Option<&str> {
            PLURAL_FORMS.iter().find_map(|f| key.strip_suffix(f))
        }
        // A plural group's placeholders, `{count}` aside.
        let group = |lang: &str, b: &str| {
            let mut names: Vec<&str> = PLURAL_FORMS
                .iter()
                .filter_map(|f| self.get_in(lang, &format!("{b}{f}")))
                .flat_map(placeholders)
                .filter(|n| *n != "count")
                .collect();
            names.sort_unstable();
            names.dedup();
            names
        };
        for lang in LANGUAGES {
            for key in self.keys(lang) {
                let Some(b) = base(key) else {
                    continue;
                };
                if self.get_in(lang, &format!("{b}.other")).is_none() {
                    out.push(format!("{lang}: `{b}` has no `{b}.other`"));
                }
            }
        }
        for lang in &LANGUAGES[1..] {
            for key in self
                .keys(english)
                .into_iter()
                .filter(|_| !self.is_stub(lang))
            {
                if self.get_in(lang, key).is_none() {
                    out.push(format!("{lang}: `{key}` is missing"));
                }
            }
            for key in self.keys(lang) {
                let (theirs, ours) = match base(key) {
                    Some(b) if self.keys(english).iter().any(|k| base(k) == Some(b)) => {
                        (group(english, b), group(lang, b))
                    }
                    _ => match self.get_in(english, key) {
                        Some(en) => (placeholders(en), placeholders(self.tr_in(lang, key))),
                        None => {
                            out.push(format!("{lang}: `{key}` isn't in English"));
                            continue;
                        }
                    },
                };
                if theirs != ours {
                    out.push(format!("{lang}: `{key}` has {ours:?}, English {theirs:?}"));
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn tr_in<'a>(&'a self, lang: &str, key: &'a str) -> &'a str {
        self.get_in(lang, key).unwrap_or(key)
    }

    /// Whether `lang` is registered but not translated yet: its source holds no key but
    /// `language.name`, so everything else shows in English and parity checks skip it.
    pub fn is_stub(&self, lang: &str) -> bool {
        self.keys(lang).iter().all(|k| *k == "language.name")
    }

    /// Every key of `lang`'s source, sorted.
    pub fn keys(&self, lang: &str) -> Vec<&str> {
        let mut keys: Vec<&str> = self
            .table(lang)
            .map(|t| t.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.sort_unstable();
        keys
    }

    fn table(&self, lang: &str) -> Option<&HashMap<String, String>> {
        let i = LANGUAGES.iter().position(|l| *l == lang)?;
        self.tables().get(i)
    }

    /// `key` in `lang` exactly, without fallback.
    pub fn get_in(&self, lang: &str, key: &str) -> Option<&str> {
        self.table(lang)?.get(key).map(String::as_str)
    }

    /// `key` in the current language, else English.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.get_in(language(), key)
            .or_else(|| self.get_in(LANGUAGES[0], key))
    }

    /// `key` in the current language, else English, else the key itself.
    pub fn tr<'a>(&'a self, key: &'a str) -> &'a str {
        self.get(key).unwrap_or(key)
    }

    /// [`Catalog::tr`] with each `{name}` replaced by its argument.
    pub fn format(&self, key: &str, args: &[(&str, &dyn Display)]) -> String {
        interpolate(self.tr(key), args)
    }

    /// The plural form of `key` for `count`: `key.one`, `key.few`, `key.many` or `key.other` by the
    /// language's rule, falling back to `other`, then to English. `{count}` is `count`.
    pub fn plural(&self, key: &str, count: u64, args: &[(&str, &dyn Display)]) -> String {
        let form = |lang: &str| {
            let exact = self.get_in(lang, &format!("{key}.{}", plural_category(lang, count)));
            exact.or_else(|| self.get_in(lang, &format!("{key}.other")))
        };
        let text = form(language())
            .or_else(|| form(LANGUAGES[0]))
            .unwrap_or(key);
        let mut all: Vec<(&str, &dyn Display)> = vec![("count", &count)];
        all.extend_from_slice(args);
        interpolate(text, &all)
    }
}

/// The suffixes of a key's plural forms, one per CLDR category a shipped language uses.
const PLURAL_FORMS: [&str; 4] = [".one", ".few", ".many", ".other"];

/// The CLDR cardinal category of a whole number in a shipped language: `one`, `few`, `many`
/// or `other`. From the CLDR chart's integer conditions (`v = 0`), linked in docs/translating.md.
pub fn plural_category(lang: &str, n: u64) -> &'static str {
    let (i10, i100) = (n % 10, n % 100);
    match lang {
        "ja" | "zh-Hans" | "ko" => "other",
        "pl" | "ru" | "uk" => match (i10, i100) {
            (1, _) if n == 1 || (lang != "pl" && i100 != 11) => "one",
            (2..=4, _) if !(12..=14).contains(&i100) => "few",
            _ => "many",
        },
        "cs" => match n {
            1 => "one",
            2..=4 => "few",
            // Czech `many` is for fractions only.
            _ => "other",
        },
        // French and Brazilian Portuguese say "0 clip" like "1 clip".
        "fr" | "pt-BR" if n == 0 => "one",
        _ if n == 1 => "one",
        // "1 millón de clips" takes "de"; CLDR calls that form `many`.
        "es" | "fr" | "it" | "pt-BR" | "ca" if n != 0 && n.is_multiple_of(1_000_000) => "many",
        _ => "other",
    }
}

/// Replaces `{name}` with the matching argument; unknown names stay as written.
pub fn interpolate(text: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        let arg = rest.find('}').and_then(|close| {
            let name = &rest[1..close];
            let (_, value) = args.iter().find(|(n, _)| *n == name)?;
            Some((value, close + 1))
        });
        match arg {
            Some((value, len)) => {
                out.push_str(&value.to_string());
                rest = &rest[len..];
            }
            None => {
                out.push('{');
                rest = &rest[1..];
            }
        }
    }
    out + rest
}

/// The `{name}` placeholders in `text`, sorted, for catalog parity checks.
pub fn placeholders(text: &str) -> Vec<&str> {
    let mut names: Vec<&str> = text
        .split('{')
        .skip(1)
        .filter_map(|s| s.split_once('}').map(|(n, _)| n))
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// A TOML source flattened to `section.key = text`. Only strings and tables are allowed.
fn parse(source: &str) -> Result<HashMap<String, String>, String> {
    fn walk(
        prefix: &str,
        table: toml::Table,
        out: &mut HashMap<String, String>,
    ) -> Result<(), String> {
        for (k, v) in table {
            let key = if prefix.is_empty() {
                k
            } else {
                format!("{prefix}.{k}")
            };
            match v {
                toml::Value::String(s) => {
                    out.insert(key, s);
                }
                toml::Value::Table(t) => walk(&key, t, out)?,
                other => return Err(format!("`{key}` is a {}, not text", other.type_str())),
            }
        }
        Ok(())
    }
    let table: toml::Table = source.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut out = HashMap::new();
    walk("", table, &mut out)?;
    Ok(out)
}

/// Text shown as written in every language: brand names, shortcut notation and other
/// identifiers. The translation guard test accepts literals passed through this.
pub const fn verbatim(text: &'static str) -> &'static str {
    text
}

/// A text looked up each time it's shown, so a `const` table of them follows the language.
pub type Label = fn() -> &'static str;

/// suite-ui's own strings.
pub static CATALOG: Catalog = Catalog::new([
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

/// Looks up a key in a crate's [`Catalog`]: `tr!(CATALOG, "key")` gives `&str`;
/// `tr!(CATALOG, "key", name = value, ..)` fills `{name}` placeholders and gives `String`.
/// Each crate wraps this in its own `tr!` bound to its catalog.
#[macro_export]
macro_rules! tr {
    ($catalog:expr, $key:literal) => {
        $catalog.tr($key)
    };
    ($catalog:expr, $key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $catalog.format($key, &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+])
    };
}

/// The plural form of a key for a count: `trn!(CATALOG, "key", n)` or with named arguments
/// after the count, as for [`tr!`]. The text gets the count as `{count}`.
#[macro_export]
macro_rules! trn {
    ($catalog:expr, $key:literal, $count:expr) => {
        $catalog.plural($key, ($count) as u64, &[])
    };
    ($catalog:expr, $key:literal, $count:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $catalog.plural(
            $key,
            ($count) as u64,
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+],
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_normalize_to_shipped_languages() {
        for (locale, want) in [
            ("es", Some("es")),
            ("es-MX", Some("es")),
            ("es_ES.UTF-8", Some("es")),
            ("ES", Some("es")),
            ("en-US", Some("en")),
            ("en_GB@euro", Some("en")),
            ("pl_PL.UTF-8", Some("pl")),
            ("fr_CA", Some("fr")),
            ("pt", Some("pt-BR")),
            ("pt-BR", Some("pt-BR")),
            ("pt_BR.UTF-8", Some("pt-BR")),
            ("pt.UTF-8", Some("pt-BR")),
            ("pt-PT", None),
            ("nb-NO", Some("nb")),
            ("no_NO", Some("nb")),
            ("zh", Some("zh-Hans")),
            ("zh-CN", Some("zh-Hans")),
            ("zh_CN.UTF-8", Some("zh-Hans")),
            ("zh-Hans-CN", Some("zh-Hans")),
            ("zh-Hant", None),
            ("zh-TW", None),
            ("zh_HK", None),
            ("ja-JP", Some("ja")),
            ("tlh-001", None),
            ("C", None),
            ("POSIX", None),
            ("", None),
        ] {
            assert_eq!(supported(locale), want, "{locale:?}");
        }
        assert_eq!(negotiate(["pt-PT", "es-AR", "en-US"]), "es");
        assert_eq!(negotiate(["eo", "C"]), "en");
        assert_eq!(negotiate(Vec::<String>::new()), "en");
    }

    #[test]
    fn settings_resolve_with_english_fallback() {
        assert_eq!(resolve("en"), "en");
        assert_eq!(resolve("es"), "es");
        assert_eq!(resolve(" es "), "es");
        // A hand-edited locale finds its language; an unknown one is English, not System.
        assert_eq!(resolve("es-MX"), "es");
        assert_eq!(resolve("eo"), "en");
        assert_eq!(resolve("klingon"), "en");
        assert_eq!(resolve(SYSTEM), system_language());
        assert_eq!(resolve(""), system_language());
        assert!(LANGUAGES.contains(&system_language()));
    }

    #[test]
    fn wave_one_plural_rules_match_cldr() {
        const AT: [u64; 11] = [0, 1, 2, 5, 11, 12, 21, 22, 25, 101, 111];
        let (o, f, m, x) = ("one", "few", "many", "other");
        for (langs, want) in [
            (&["pl"][..], [m, o, f, m, m, m, m, f, m, m, m]),
            (&["ru", "uk"], [m, o, f, m, m, m, o, f, m, o, m]),
            (&["cs"], [x, o, f, x, x, x, x, x, x, x, x]),
            (&["fr", "pt-BR"], [o, o, x, x, x, x, x, x, x, x, x]),
            (
                &["it", "ca", "de", "nl", "tr", "sv", "da", "nb"],
                [x, o, x, x, x, x, x, x, x, x, x],
            ),
            (&["ja", "zh-Hans", "ko"], [x; 11]),
        ] {
            for lang in langs {
                let got = AT.map(|n| plural_category(lang, n));
                assert_eq!(got, want, "{lang} at {AT:?}");
            }
        }
        // These, like Spanish, give exact millions their own form.
        for lang in ["fr", "it", "pt-BR", "ca"] {
            assert_eq!(plural_category(lang, 2_000_000), "many", "{lang}");
            assert_eq!(plural_category(lang, 2_000_001), "other", "{lang}");
        }
        assert_eq!(plural_category("de", 2_000_000), "other");
    }

    #[test]
    fn plural_categories_follow_cldr_for_whole_numbers() {
        for (n, en, es) in [
            (0, "other", "other"),
            (1, "one", "one"),
            (2, "other", "other"),
            (21, "other", "other"),
            (1_000_000, "other", "many"),
            (2_000_000, "other", "many"),
            (1_000_001, "other", "other"),
        ] {
            assert_eq!(plural_category("en", n), en, "en {n}");
            assert_eq!(plural_category("es", n), es, "es {n}");
        }
    }

    #[test]
    fn interpolation_fills_known_names_and_keeps_the_rest() {
        let name = "clip.mp4";
        assert_eq!(
            interpolate("Remove “{name}”?", &[("name", &name)]),
            "Remove “clip.mp4”?"
        );
        assert_eq!(interpolate("{a}{b} {a}", &[("a", &1), ("b", &"x")]), "1x 1");
        // A missing or malformed placeholder stays readable instead of panicking.
        assert_eq!(interpolate("{missing} ok", &[]), "{missing} ok");
        assert_eq!(interpolate("{ unclosed", &[("unclosed", &1)]), "{ unclosed");
        assert_eq!(interpolate("tail {", &[]), "tail {");
        // An argument's own braces are not expanded again.
        assert_eq!(interpolate("{a}", &[("a", &"{a}")]), "{a}");
        assert_eq!(placeholders("{b} {a} {b} {} {not a name}"), ["a", "b"]);
    }

    /// English and Spanish sources; the other languages are empty.
    const fn en_es(en: &'static str, es: &'static str) -> [&'static str; LANGUAGES.len()] {
        let mut s = [""; LANGUAGES.len()];
        s[0] = en;
        s[1] = es;
        s
    }

    static TEST: Catalog = Catalog::new(en_es(
        "plain = \"Plain\"\nonly_en = \"English only\"\n[clips]\none = \"{count} clip\"\nother = \"{count} clips\"\n",
        "plain = \"Llano\"\n[clips]\none = \"{count} clip\"\nother = \"{count} clips\"\nmany = \"{count} de clips\"\n",
    ));

    // Switching the language is global, so the tests that do it live in tests/i18n.rs,
    // their own process; these read a fixed language.
    #[test]
    fn catalogs_flatten_sections_into_keys() {
        assert!(TEST.errors().is_empty());
        assert_eq!(TEST.get_in("es", "plain"), Some("Llano"));
        assert_eq!(TEST.get_in("es", "only_en"), None);
        assert_eq!(TEST.get_in("fr", "plain"), None);
        assert!(TEST.is_stub("pl") && !TEST.is_stub("es"));
        assert_eq!(TEST.get_in("es", "clips.many"), Some("{count} de clips"));
        assert_eq!(
            TEST.keys("en"),
            ["clips.one", "clips.other", "only_en", "plain"]
        );
        assert!(TEST.keys("fr").is_empty());
    }

    #[test]
    fn mismatches_name_each_way_a_translation_drifts() {
        assert_eq!(TEST.mismatches(), ["es: `only_en` is missing"]);
        let bad = Catalog::new(en_es(
            "a = \"{name} and {n}\"\nb = \"B\"\n[x]\none = \"{count} x in {place}\"\nother = \"{count} xs in {place}\"\n",
            "a = \"{name} y {m}\"\nc = \"C\"\n[x]\none = \"{count} x\"\nother = \"{count} xs\"\n[y]\none = \"y\"\n",
        ));
        assert_eq!(
            bad.mismatches(),
            [
                "es: `a` has [\"m\", \"name\"], English [\"n\", \"name\"]",
                "es: `b` is missing",
                "es: `c` isn't in English",
                "es: `x.one` has [], English [\"place\"]",
                "es: `x.other` has [], English [\"place\"]",
                "es: `y.one` isn't in English",
                "es: `y` has no `y.other`",
            ]
        );
        // `{count}` may be left out of a form; the real catalogs are checked by their crates.
        assert!(
            CATALOG.mismatches().is_empty(),
            "{:?}",
            CATALOG.mismatches()
        );
    }

    #[test]
    fn non_text_values_are_reported_not_loaded() {
        let bad = Catalog::new(en_es("n = 3", "[a]\nb = \"ok\""));
        assert_eq!(bad.errors(), ["en: `n` is a integer, not text"]);
        assert_eq!(bad.get_in("en", "n"), None);
        assert_eq!(bad.get_in("es", "a.b"), Some("ok"));
    }
}
