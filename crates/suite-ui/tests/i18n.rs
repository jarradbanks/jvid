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

static TEST: Catalog = Catalog::new([
    "greet = \"Hi {name}\"\nonly_en = \"English only\"\n[clips]\none = \"{count} clip\"\nother = \"{count} clips\"\n[tracks]\none = \"{count} track\"\nother = \"{count} tracks\"\n",
    "greet = \"Hola {name}\"\n[clips]\none = \"{count} clip\"\nother = \"{count} clips\"\nmany = \"{count} de clips\"\n[tracks]\none = \"{count} pista\"\nother = \"{count} pistas\"\n",
]);

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
    let _fr = switch("fr");
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
