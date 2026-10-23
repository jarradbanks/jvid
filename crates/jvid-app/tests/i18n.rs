//! The editor in Spanish and back without a restart. The language is global, so these tests
//! hold one lock and live in their own process, apart from the English unit tests.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use jvid_app::app::App;
use jvid_app::export::dialog::ExportDialog;
use jvid_app::preferences::Preferences;
use suite_ui::i18n;
use suite_ui::test_support::install_theme;

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

fn has(h: &Harness<'_, impl Sized>, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

#[test]
fn editor_menus_inspector_and_dialogs_show_spanish() {
    let _es = switch("es");
    let mut h = Harness::builder()
        .with_size([1280.0, 720.0])
        .build_eframe(|cc| App::new(cc, &Default::default()));
    h.run();
    h.get_by_label("Nuevo proyecto").click();
    h.run();
    assert!(has(&h, "Selecciona un clip para ver sus propiedades"));
    assert!(has(&h, "Vista de lista") && has(&h, "Vista de cuadrícula"));
    assert!(!has(&h, "Select a clip to see its properties"));

    h.get_by_label("Archivo").click();
    h.run();
    // Action names translate; the key notation after them doesn't.
    assert!(has(&h, "Guardar Ctrl+S"), "no Save row");
    assert!(has(&h, "Exportar medios Ctrl+M"), "no Export row");
    h.key_press(egui::Key::Escape);
    h.run();
    // The title bar's project menu: an untitled project's name is translated too.
    h.get_by_label("Sin título").click();
    h.run();
    h.get_by_label("Ajustes del proyecto…").click();
    h.run();
    assert!(has(&h, "Ajustes del proyecto"));
    h.key_press(egui::Key::Escape);
    h.run();

    h.get_by_label("Sin título").click();
    h.run();
    h.get_by_label("Ajustes…").click();
    h.run();
    assert!(has(&h, "Ajustes") && has(&h, "Idioma"));
    h.get_by_label("Listo").click();
    h.run();

    i18n::set_language("en");
    h.run();
    assert!(has(&h, "Select a clip to see its properties") && has(&h, "List View"));
    assert!(has(&h, "File") && has(&h, "Untitled") && !has(&h, "Archivo"));
}

/// The smallest window (960 px wide) at 150 % display scaling, in points.
const NARROWEST: f32 = 640.0;

/// Labels of everything that reaches past the right edge of the narrowest window.
fn spills(h: &Harness<'_, impl Sized>) -> Vec<String> {
    let root = h.root();
    root.children_recursive()
        .filter_map(|n| {
            let r = n.accesskit_node().bounding_box()?;
            let a = n.accesskit_node();
            // A path longer than its text field scrolls inside it, in any language.
            let in_field = a.parent().is_some_and(|p| p.role() == Role::TextInput);
            (r.x1 > NARROWEST as f64 && !in_field).then(|| format!("{:?} {r:?}", a.label()))
        })
        .collect()
}

#[test]
fn export_dialog_shows_spanish() {
    let _es = switch("es");
    let project = jvid_core::model::Project::default();
    let mut dialog = ExportDialog::default();
    dialog.open(project.sequence(), "Proyecto");
    let mut fresh = true;
    let mut h = Harness::builder()
        .with_size([NARROWEST, 720.0])
        .build_ui_state(
            move |ui, d: &mut ExportDialog| {
                if !install_theme(ui, &mut fresh) {
                    d.show(ui, &project, &HashSet::new());
                }
            },
            dialog,
        );
    h.run();
    assert_eq!(spills(&h), Vec::<String>::new());
    for label in [
        "Exportar",
        "Añadir a la cola",
        "Completo",
        "Destacado 1080p60",
    ] {
        assert!(has(&h, label), "no {label:?}");
    }
    i18n::set_language("en");
    h.run();
    assert!(has(&h, "Add to Queue") && has(&h, "Highlight 1080p60"));
    assert!(!has(&h, "Añadir a la cola"));
    assert_eq!(spills(&h), Vec::<String>::new());
}

#[test]
fn settings_picker_switches_at_once_and_saves() {
    let _es = switch("es");
    let dir = std::env::temp_dir().join(format!("jvid-i18n-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.toml");
    std::fs::write(&path, "theme = \"light\"\n").unwrap();
    let mut prefs = Preferences::default();
    prefs.open();
    let saved = path.clone();
    let mut fresh = true;
    let mut h = Harness::builder().with_size([800.0, 600.0]).build_ui_state(
        move |ui, (p, language): &mut (Preferences, String)| {
            if !install_theme(ui, &mut fresh) {
                p.show(ui, language, |l| {
                    jvid_app::settings_file::save_language_to(&saved, l)
                });
            }
        },
        (prefs, i18n::SYSTEM.to_owned()),
    );
    h.run();
    assert!(has(&h, "Idioma"));
    let system = jvid_app::preferences::options().remove(0);
    assert!(system.starts_with("Sistema ("), "{system}");

    let mut pick = |current: &str, row: &str| {
        h.get_by_label(current).click();
        h.run();
        h.get_by_label(row).click();
        h.run();
    };
    pick(&system, "English");
    let settings = |p: &std::path::Path| jvid_app::settings_file::load_from(p);
    assert_eq!(i18n::language(), "en");
    assert_eq!(settings(&path).language, "en");
    assert_eq!(settings(&path).theme, "light", "other settings kept");

    pick("English", "Español");
    assert_eq!(i18n::language(), "es");
    assert_eq!(settings(&path).language, "es");

    pick("Español", &system);
    assert_eq!(settings(&path).language, "system");
    assert_eq!(i18n::language(), i18n::system_language());
    std::fs::remove_dir_all(&dir).unwrap();
}
