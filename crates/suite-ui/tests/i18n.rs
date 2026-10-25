//! Switching the language while running. The language is global, so these tests hold one
//! lock and live in their own process, apart from the lib's unit tests.

#[allow(dead_code)]
#[path = "../examples/gallery.rs"]
mod gallery;

use std::sync::{Mutex, MutexGuard};

use egui_kittest::kittest::Queryable;
use suite_ui::i18n::{self, CATALOG, Catalog};
use suite_ui::test_support as common;

static LOCK: Mutex<()> = Mutex::new(());

/// The language set to `code` until dropped, then English again, even after a failed assert.
struct Switched(#[allow(dead_code)] MutexGuard<'static, ()>);

fn switch(code: &str) -> Switched {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    i18n::set_language(code);
    Switched(guard)
}

impl Drop for Switched {
    fn drop(&mut self) {
        i18n::set_language("en");
    }
}

static TEST: Catalog = Catalog::new(suite_ui::test_support::en_es(
    "greet = \"Hi {name}\"\nonly_en = \"English only\"\n[clips]\none = \"{count} clip\"\nother = \"{count} clips\"\n[tracks]\none = \"{count} track\"\nother = \"{count} tracks\"\n",
    "greet = \"Hola {name}\"\n[clips]\none = \"{count} clip\"\nother = \"{count} clips\"\nmany = \"{count} de clips\"\n[tracks]\none = \"{count} pista\"\nother = \"{count} pistas\"\n",
));

#[test]
fn every_character_has_a_glyph() {
    assert_eq!(common::missing_glyphs(&CATALOG), Vec::<String>::new());
    assert_eq!(
        common::missing_glyphs(&gallery::CATALOG),
        Vec::<String>::new()
    );
}

#[test]
fn widget_strings_follow_a_live_switch() {
    let _es = switch("es");
    assert_eq!(i18n::language(), "es");
    assert_eq!(CATALOG.tr("panel.more_tabs"), "Más pestañas");
    assert_eq!(CATALOG.tr("input.search"), "Buscar");
    assert_eq!(
        suite_ui::tr!(CATALOG, "theme.bad_color", value = "#FFF"),
        "`#FFF` no es #RRGGBB"
    );
    i18n::set_language("en");
    assert_eq!(CATALOG.tr("panel.more_tabs"), "More Tabs");
    i18n::set_language("es");
    assert_eq!(CATALOG.tr("panel.more_tabs"), "Más pestañas");
}

#[test]
fn unsupported_languages_show_english() {
    let _eo = switch("eo");
    assert_eq!(i18n::language(), "en");
    assert_eq!(CATALOG.tr("input.search"), "Search");
    i18n::set_language("es-MX");
    assert_eq!(i18n::language(), "en", "set_language takes a resolved code");
    assert_eq!(i18n::resolve("es-MX"), "es");
}

#[test]
fn missing_keys_and_placeholders_stay_readable() {
    let _es = switch("es");
    assert_eq!(TEST.tr("only_en"), "English only");
    assert_eq!(TEST.tr("no.such_key"), "no.such_key");
    assert_eq!(suite_ui::tr!(TEST, "greet", name = "Ana"), "Hola Ana");
    // A missing argument keeps its placeholder; an unknown one is ignored.
    assert_eq!(TEST.format("greet", &[]), "Hola {name}");
    assert_eq!(suite_ui::tr!(TEST, "greet", nmae = "Ana"), "Hola {name}");
}

#[test]
fn plurals_use_each_languages_rule() {
    let _es = switch("es");
    let clips = |n| suite_ui::trn!(TEST, "clips", n);
    assert_eq!(clips(1), "1 clip");
    assert_eq!(clips(0), "0 clips");
    assert_eq!(clips(1_000_000), "1000000 de clips");
    // Without a `many` form, `other` stands in.
    assert_eq!(suite_ui::trn!(TEST, "tracks", 1_000_000), "1000000 pistas");
    assert_eq!(suite_ui::trn!(TEST, "tracks", 1), "1 pista");
    i18n::set_language("en");
    assert_eq!(clips(1_000_000), "1000000 clips");
    assert_eq!(clips(0), "0 clips");
    assert_eq!(suite_ui::trn!(TEST, "no_such", 2), "no_such");
}

#[test]
fn gallery_switches_language_without_a_restart() {
    let _es = switch("es");
    let mut state = gallery::Gallery::new();
    let mut fresh = true;
    let mut harness = egui_kittest::Harness::builder()
        .with_size([1280.0, 860.0])
        .build_ui(|ui| {
            if !common::install_theme(ui, &mut fresh) {
                gallery::gallery(ui, &mut state);
            }
        });
    harness.run_steps(4);
    for label in ["Más pestañas", "Principal", "Controles", "Al pasar"] {
        assert!(harness.query_by_label(label).is_some(), "no {label:?}");
    }
    assert!(harness.query_by_label("More Tabs").is_none());

    i18n::set_language("en");
    harness.run_steps(4);
    for label in ["More Tabs", "Primary", "Widgets", "Hover"] {
        assert!(harness.query_by_label(label).is_some(), "no {label:?}");
    }
    assert!(harness.query_by_label("Más pestañas").is_none());
}

#[test]
fn every_language_is_listed_by_its_own_name_and_falls_back_to_english() {
    let names = [
        "English",
        "Español",
        "Français",
        "Deutsch",
        "Italiano",
        "Português (Brasil)",
        "Nederlands",
        "Polski",
        "Čeština",
        "Türkçe",
        "Русский",
        "Українська",
        "Svenska",
        "Dansk",
        "Norsk bokmål",
        "Català",
        "日本語",
        "简体中文",
        "한국어",
    ];
    assert_eq!(i18n::LANGUAGES.map(i18n::autonym), names);
    assert_eq!(CATALOG.errors(), Vec::<String>::new());
    let _guard = switch("en");
    for lang in i18n::LANGUAGES {
        i18n::set_language(lang);
        assert_eq!(i18n::language(), lang);
        if CATALOG.is_stub(lang) {
            assert_eq!(CATALOG.tr("input.search"), "Search", "{lang}");
        }
    }
}

#[test]
fn polish_russian_and_their_neighbours_have_glyphs() {
    let samples = [
        ("pl", "Zażółć gęślą jaźń. ŻÓŁW ĄĘĆŃŚŹ „cytat”"),
        ("cs", "Příliš žluťoučký kůň úpěl ďábelské ódy. ŘŠČŽŮĚŤĎŇ"),
        ("tr", "Pijamalı hasta yağız şoföre çabucak güvendi. İĞŞÇÖÜ"),
        (
            "ru",
            "Съешь же ещё этих мягких французских булок, да выпей чаю. ЁЖ «Ъ»",
        ),
        (
            "uk",
            "Чуєш їх, доцю, га? Кумедна ж ти, прощайся без ґольфів! ҐЄІЇ’",
        ),
        ("fr", "Œuvre cœur « déjà » à l’été"),
        ("ca", "Col·lecció l’àvia"),
    ];
    let mut sources = [""; i18n::LANGUAGES.len()];
    for (lang, text) in samples {
        let i = i18n::LANGUAGES.iter().position(|l| *l == lang).unwrap();
        sources[i] = Box::leak(format!("sample = {text:?}").into_boxed_str());
    }
    // Every language name, which the picker shows in English too.
    let names = i18n::LANGUAGES.map(i18n::autonym).join(" ");
    sources[0] = Box::leak(format!("names = {names:?}").into_boxed_str());
    let catalog = Catalog::new(sources);
    assert_eq!(catalog.errors(), Vec::<String>::new());
    assert_eq!(common::missing_glyphs(&catalog), Vec::<String>::new());
}

#[test]
fn japanese_loads_its_face_only_while_active() {
    let _en = switch("en");
    let ctx = egui::Context::default();
    suite_ui::theme::install(&ctx);
    let ja = suite_ui::theme::cjk_face("ja");
    // A switch shows from the next pass, so run two.
    let loaded = |ctx: &egui::Context| {
        for _ in 0..2 {
            ctx.run_ui(Default::default(), |_| {})
                .textures_delta
                .clear();
        }
        ctx.fonts(|f| f.definitions().font_data.contains_key(&ja))
    };
    assert!(!loaded(&ctx));
    i18n::set_language("ja");
    assert!(loaded(&ctx));
    i18n::set_language("ko");
    assert!(!loaded(&ctx));
    i18n::set_language("en");
    assert!(!loaded(&ctx));
}

#[test]
fn cjk_faces_stay_within_budget() {
    for (lang, face) in suite_ui::theme::CJK_FACES {
        assert!(face.len() <= 1_000_000, "{lang} is {} bytes", face.len());
    }
}
