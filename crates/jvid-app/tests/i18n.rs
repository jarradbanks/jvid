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
    static HOME: std::sync::Once = std::sync::Once::new();
    HOME.call_once(|| {
        // The editor reads settings and seeds the effects folder; keep it off the user's own.
        let home = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("i18n-home");
        let _ = std::fs::remove_dir_all(&home);
        for var in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"] {
            // SAFETY: every test here takes LOCK first, so no other thread reads the environment.
            #[allow(unsafe_code)]
            unsafe {
                std::env::set_var(var, home.join(var))
            };
        }
    });
    i18n::set_language(code);
    Switched(guard)
}

impl Drop for Switched {
    fn drop(&mut self) {
        i18n::set_language("en");
    }
}

/// Whether the editor's folders are the test's own. Only Linux's move (XDG, in `switch`); on
/// Windows and macOS `ProjectDirs` asks the OS, so a test there would use the user's folders.
fn isolated() -> bool {
    let linux = cfg!(target_os = "linux");
    if !linux {
        eprintln!("skipped: only Linux moves the editor's folders away from the user's own");
    }
    linux
}

fn has(h: &Harness<'_, impl Sized>, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

#[test]
fn editor_menus_inspector_and_dialogs_show_spanish() {
    if !isolated() {
        return;
    }
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

    h.get_by_label("Edición").click();
    h.run();
    h.get_by_label("Preferencias…").click();
    h.run();
    assert!(has(&h, "Preferencias") && has(&h, "Idioma"));
    h.get_by_label("Listo").click();
    h.run();

    i18n::set_language("en");
    h.run();
    assert!(has(&h, "Select a clip to see its properties") && has(&h, "List View"));
    assert!(has(&h, "File") && has(&h, "Untitled") && !has(&h, "Archivo"));
}

fn content(h: &Harness<'_, impl Sized>) -> Option<String> {
    h.get_by_role(Role::MultilineTextInput).value()
}

/// A new project with one title clip, selected, made the way a user makes one: the bin's preset
/// dragged onto the timeline. Run in Spanish.
fn titled_project() -> Harness<'static, App> {
    let mut h = Harness::builder()
        .with_size([1280.0, 720.0])
        .build_eframe(|cc| App::new(cc, &Default::default()));
    h.run();
    h.get_by_label("Nuevo proyecto").click();
    h.run();
    h.get_by_label("Texto").click();
    h.run();
    let from = h.get_by_label("Título").rect().center();
    let to = egui::pos2(500.0, h.get_by_label("Pista").rect().center().y);
    h.hover_at(from);
    h.run();
    h.drag_at(from);
    h.run();
    for i in 1..=10 {
        h.hover_at(from + (to - from) * (i as f32 / 10.0));
        h.run();
    }
    h.drop_at(to);
    h.run();
    h.get_by_label("Edición").click();
    h.run();
    h.get_by_label("Seleccionar todo Ctrl+A").click();
    h.run();
    h
}

#[test]
fn selected_clip_inspector_switches_live() {
    if !isolated() {
        return;
    }
    let _es = switch("es");
    let mut h = titled_project();
    let spanish = [
        "Transformación",
        "Escala",
        "Rotación",
        "Anclaje",
        "Opacidad",
        "Fusión",
        "Animar: Posición",
        "Fotograma clave en todo: Transformación",
        "Fuente",
        "Contorno",
    ];
    let english = [
        "Transform",
        "Scale",
        "Rotation",
        "Anchor",
        "Opacity",
        "Animate Position",
        "Keyframe All Transform",
        "Font",
        "Outline",
    ];
    for label in spanish {
        assert!(has(&h, label), "no {label:?}");
    }
    assert!(!english.iter().any(|l| has(&h, l)));
    // The preset's text was made in Spanish; it's the project's now, not interface text.
    assert_eq!(content(&h).as_deref(), Some("Título"));

    // Section state is keyed by id, not caption: collapsed in Spanish, collapsed in English.
    h.get_all_by_label("Opacidad").next().unwrap().click();
    h.run();
    assert!(!has(&h, "Fusión"));
    // Typing into the text field is one undo step.
    h.get_by_role(Role::MultilineTextInput).scroll_to_me();
    h.run();
    h.get_by_role(Role::MultilineTextInput).click();
    h.run();
    h.get_by_role(Role::MultilineTextInput)
        .type_text(", toma 3");
    h.run();
    assert_eq!(content(&h).as_deref(), Some("Título, toma 3"));
    assert!(h.get_by_role(Role::MultilineTextInput).is_focused());

    i18n::set_language("en");
    h.run();
    for label in english {
        assert!(has(&h, label), "no {label:?}");
    }
    assert!(!spanish.iter().any(|l| has(&h, l)));
    assert!(
        has(&h, "Opacity") && !has(&h, "Blend"),
        "opacity section reopened"
    );
    assert!(
        !has(&h, "Select a clip to see its properties"),
        "selection lost"
    );
    assert_eq!(content(&h).as_deref(), Some("Título, toma 3"));
    assert!(h.get_by_role(Role::MultilineTextInput).is_focused());

    h.get_by_label("Undo").click();
    h.run();
    assert_eq!(content(&h).as_deref(), Some("Título"));
    h.get_by_label("Redo").click();
    h.run();
    assert_eq!(content(&h).as_deref(), Some("Título, toma 3"));
    assert!(has(&h, "Transform") && has(&h, "Font"), "selection lost");
}

/// Every text painted in the last frame, including what's drawn without a widget.
fn painted(h: &Harness<'_, impl Sized>) -> Vec<String> {
    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
            _ => {}
        }
    }
    let mut out = Vec::new();
    h.output()
        .shapes
        .iter()
        .for_each(|c| walk(&c.shape, &mut out));
    out
}

/// Types into the bin's search field, which shows `shows` (its hint while empty): `erase`
/// backspaces, then `text`.
fn search(h: &mut Harness<'_, App>, shows: &str, erase: usize, text: &str) {
    h.get_by(|n| {
        n.role() == Role::TextInput
            && (n.placeholder() == Some(shows) || n.value().as_deref() == Some(shows))
    })
    .click();
    h.run();
    for _ in 0..erase {
        h.key_press(egui::Key::Backspace);
    }
    h.event(egui::Event::Text(text.to_owned()));
    h.run();
}

fn click_leftmost(h: &mut Harness<'_, impl Sized>, label: &str) {
    let x = |n: &egui_kittest::Node<'_>| n.rect().min.x;
    let tab = h
        .get_all_by_label(label)
        .min_by(|a, b| x(a).total_cmp(&x(b)));
    tab.unwrap().click();
    h.run();
}

/// The shipped effects and the keyframe lanes they and built-in properties key, which are drawn
/// with the painter rather than as widgets.
#[test]
fn bundled_effects_and_keyframe_lanes_switch_live() {
    if !isolated() {
        return;
    }
    let _es = switch("es");
    let mut h = titled_project();
    h.get_by_label("Añadir efecto").scroll_to_me();
    h.run();
    h.get_by_label("Añadir efecto").click();
    h.run();
    for name in ["Viñeta", "Grano de película", "Aberración cromática"] {
        assert!(has(&h, name), "Add Effect lacks {name:?}");
    }
    h.get_by_label("Viñeta").click();
    h.run();
    // The Transform section's position comes before the text's own.
    for (key, caption) in [("Posición", "posición"), ("Intensidad", "fx0.intensidad")] {
        let label = format!("Animar: {key}");
        h.get_all_by_label(&label).next().unwrap().scroll_to_me();
        h.run();
        h.get_all_by_label(&label).next().unwrap().click();
        h.run();
        assert!(
            painted(&h).iter().any(|t| t == caption),
            "no {caption:?} lane"
        );
    }
    assert!(has(&h, "Intensidad") && has(&h, "Suavidad"));
    click_leftmost(&mut h, "Efectos");
    let corners =
        "Oscurece hacia las esquinas: toda la intensidad en las esquinas, nada dentro del radio.";
    let shown = painted(&h);
    for text in [
        "Viñeta",
        corners,
        "Grano de película",
        "Aberración cromática",
    ] {
        assert!(shown.iter().any(|t| t == text), "no {text:?} row");
    }
    search(&mut h, "Buscar", 0, "esquinas");
    assert!(painted(&h).iter().any(|t| t == "Viñeta"));
    assert!(
        !painted(&h).iter().any(|t| t == "Grano de película"),
        "search kept a non-match"
    );

    i18n::set_language("en");
    h.run();
    let shown = painted(&h);
    for text in ["position", "fx0.strength", "Vignette"] {
        assert!(shown.iter().any(|t| t == text), "no {text:?}");
    }
    for text in ["posición", "fx0.intensidad", "Viñeta", corners] {
        assert!(!shown.iter().any(|t| t == text), "{text:?} still drawn");
    }
    assert!(has(&h, "Strength") && has(&h, "Softness") && has(&h, "Animate Softness"));
    // The query is the user's, kept as typed; it searches what the rows now say.
    assert!(has(&h, "No effects match the search"));
    search(&mut h, "esquinas", "esquinas".len(), "corners");
    let shown = painted(&h);
    assert!(
        shown
            .iter()
            .any(|t| t.starts_with("Darkens towards the corners"))
    );
    assert!(
        !shown.iter().any(|t| t == "Film grain"),
        "search kept a non-match"
    );

    // Keys are stored by id, so undo removes the effect's lane and redo brings back the same one.
    h.get_by_label("Undo").click();
    h.run();
    let shown = painted(&h);
    assert!(shown.iter().any(|t| t == "position") && !shown.iter().any(|t| t == "fx0.strength"));
    h.get_by_label("Redo").click();
    h.run();
    assert!(painted(&h).iter().any(|t| t == "fx0.strength"));
    h.get_by_label("Add Effect").scroll_to_me();
    h.run();
    h.get_by_label("Add Effect").click();
    h.run();
    assert!(has(&h, "Film grain") && has(&h, "Chromatic aberration"));
}

/// 960 physical px at 150 % display scaling, in points: narrower than the minimum app.rs asks
/// for (`with_min_inner_size([960.0, 600.0])`, in logical points). Whether each platform's window
/// manager enforces that minimum isn't verified.
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
