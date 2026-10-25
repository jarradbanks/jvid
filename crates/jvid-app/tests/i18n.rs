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
        i18n::set_pseudo(false);
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

/// New Project, Text, Title, Track, Edit and Select All in Spanish, for [`titled_project`].
const ES: [&str; 6] = [
    "Nuevo proyecto",
    "Texto",
    "Título",
    "Pista",
    "Edición",
    "Seleccionar todo Ctrl+A",
];

/// A new project with one title clip, selected, made the way a user makes one: the bin's preset
/// dragged onto the timeline. Run in the language `labels` are in (see [`ES`]).
fn titled_project(labels: [&str; 6]) -> Harness<'static, App> {
    let [new, text, title, track, edit, select_all] = labels;
    let mut h = Harness::builder()
        .with_size([1280.0, 720.0])
        .build_eframe(|cc| App::new(cc, &Default::default()));
    h.run();
    h.get_by_label(new).click();
    h.run();
    h.get_by_label(text).click();
    h.run();
    let from = h.get_by_label(title).rect().center();
    let to = egui::pos2(500.0, h.get_by_label(track).rect().center().y);
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
    h.get_by_label(edit).click();
    h.run();
    h.get_by_label(select_all).click();
    h.run();
    h
}

#[test]
fn selected_clip_inspector_switches_live() {
    if !isolated() {
        return;
    }
    let _es = switch("es");
    let mut h = titled_project(ES);
    let spanish = [
        "Fuente",
        "Contorno",
        "Transformación",
        "Escala",
        "Rotación",
        "Anclaje",
        "Opacidad",
        "Fusión",
        "Animar: Posición",
        "Fotograma clave en todo: Transformación",
    ];
    let english = [
        "Font",
        "Outline",
        "Transform",
        "Scale",
        "Rotation",
        "Anchor",
        "Opacity",
        "Animate Position",
        "Keyframe All Transform",
    ];
    // A title opens on the Inspector's Text tab; the rest is under Video.
    for label in &spanish[..2] {
        assert!(has(&h, label), "no {label:?}");
    }
    // The preset's text was made in Spanish; it's the project's now, not interface text.
    assert_eq!(content(&h).as_deref(), Some("Título"));
    click_rightmost(&mut h, "Vídeo");
    for label in &spanish[2..] {
        assert!(has(&h, label), "no {label:?}");
    }
    assert!(!english.iter().any(|l| has(&h, l)));

    // Section state is keyed by id, not caption: collapsed in Spanish, collapsed in English.
    h.get_all_by_label("Opacidad").next().unwrap().click();
    h.run();
    assert!(!has(&h, "Fusión"));
    // Typing into the text field is one undo step.
    click_rightmost(&mut h, "Texto");
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
    assert!(
        !has(&h, "Select a clip to see its properties"),
        "selection lost"
    );
    assert_eq!(content(&h).as_deref(), Some("Título, toma 3"));
    assert!(h.get_by_role(Role::MultilineTextInput).is_focused());
    for label in &english[..2] {
        assert!(has(&h, label), "no {label:?}");
    }
    click_rightmost(&mut h, "Video");
    for label in &english[2..] {
        assert!(has(&h, label), "no {label:?}");
    }
    assert!(!spanish.iter().any(|l| has(&h, l)));
    assert!(
        has(&h, "Opacity") && !has(&h, "Blend"),
        "opacity section reopened"
    );

    h.get_by_label("Undo").click();
    h.run();
    assert!(has(&h, "Transform"), "selection lost");
    click_rightmost(&mut h, "Text");
    assert_eq!(content(&h).as_deref(), Some("Título"));
    h.get_by_label("Redo").click();
    h.run();
    assert_eq!(content(&h).as_deref(), Some("Título, toma 3"));
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

/// The Inspector's tab, right of the bin's tab of the same name.
fn click_rightmost(h: &mut Harness<'_, impl Sized>, label: &str) {
    let x = |n: &egui_kittest::Node<'_>| n.rect().min.x;
    let tab = h
        .get_all_by_label(label)
        .max_by(|a, b| x(a).total_cmp(&x(b)));
    tab.unwrap().click();
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
    let mut h = titled_project(ES);
    click_rightmost(&mut h, "Efectos");
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
    for (tab, key, caption) in [
        ("Vídeo", "Posición", "Posición"),
        ("Efectos 1", "Intensidad", "Viñeta · Intensidad"),
    ] {
        click_rightmost(&mut h, tab);
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
    for text in ["Position", "Vignette · Strength", "Vignette"] {
        assert!(shown.iter().any(|t| t == text), "no {text:?}");
    }
    for text in ["Posición", "Viñeta · Intensidad", "Viñeta", corners] {
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
        !shown.iter().any(|t| t == "Film Grain"),
        "search kept a non-match"
    );

    // Keys are stored by id, so undo removes the effect's lane and redo brings back the same one.
    h.get_by_label("Undo").click();
    h.run();
    let shown = painted(&h);
    assert!(
        shown.iter().any(|t| t == "Position") && !shown.iter().any(|t| t == "Vignette · Strength")
    );
    h.get_by_label("Redo").click();
    h.run();
    assert!(painted(&h).iter().any(|t| t == "Vignette · Strength"));
    h.get_by_label("Add Effect").scroll_to_me();
    h.run();
    h.get_by_label("Add Effect").click();
    h.run();
    assert!(has(&h, "Film Grain") && has(&h, "Chromatic Aberration"));
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
    prefs.open(vec!["USB Mic".into()], None);
    let saved = path.clone();
    let mut fresh = true;
    type State = (Preferences, String, String, String);
    let mut h = Harness::builder().with_size([960.0, 600.0]).build_ui_state(
        move |ui, (p, language, theme, input): &mut State| {
            if !install_theme(ui, &mut fresh) {
                p.show(ui, language, theme, &mut false, input, |k, v| {
                    jvid_app::settings_file::save_value_to(&saved, k, v)
                });
            }
        },
        (
            prefs,
            i18n::SYSTEM.to_owned(),
            "light".into(),
            String::new(),
        ),
    );
    h.run();
    assert!(has(&h, "Idioma"));
    let system = jvid_app::preferences::options().remove(0);
    assert!(system.starts_with("Sistema ("), "{system}");

    let pick = |h: &mut Harness<'_, State>, current: &str, row: &str| {
        h.get_by_label(current).click();
        h.run();
        h.get_by_label(row).click();
        h.run();
    };
    pick(&mut h, &system, "English");
    let settings = |p: &std::path::Path| jvid_app::settings_file::load_from(p);
    assert_eq!(i18n::language(), "en");
    assert_eq!(settings(&path).language, "en");
    assert_eq!(settings(&path).theme, "light", "other settings kept");

    pick(&mut h, "English", "Español");
    assert_eq!(i18n::language(), "es");
    assert_eq!(settings(&path).language, "es");

    h.get_by_label("Alto contraste").click();
    h.run();
    assert_eq!(settings(&path).theme, "high-contrast");
    assert_eq!(h.state().2, "high-contrast");

    h.get_by_label("Audio").click();
    h.run();
    assert!(
        h.query_by_label("Idioma").is_none(),
        "one section at a time"
    );
    pick(&mut h, "Predeterminado del sistema", "USB Mic");
    assert_eq!(settings(&path).input_device, "USB Mic");
    assert_eq!(settings(&path).language, "es", "other settings kept");

    h.get_by_label("General").click();
    h.run();
    pick(&mut h, "Español", &system);
    assert_eq!(settings(&path).language, "system");
    assert_eq!(i18n::language(), i18n::system_language());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Every text drawn in the last frame that runs past its clip rect sideways without an ellipsis,
/// or into another text. Text cut off at a scroll area's top or bottom scrolls into view.
fn clashes(h: &Harness<'_, impl Sized>) -> Vec<String> {
    struct Text {
        text: String,
        ink: egui::Rect,
        clip: egui::Rect,
        elided: bool,
    }
    fn walk(shape: &egui::Shape, clip: egui::Rect, out: &mut Vec<Text>) {
        match shape {
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, out)),
            egui::Shape::Text(t) => {
                let ink = t.galley.mesh_bounds.translate(t.pos.to_vec2());
                if t.angle == 0.0 && ink.is_positive() && ink.intersects(clip) {
                    out.push(Text {
                        text: t.galley.text().to_owned(),
                        ink,
                        clip,
                        elided: t.galley.elided,
                    });
                }
            }
            _ => {}
        }
    }
    let mut texts = Vec::new();
    for c in &h.output().shapes {
        walk(&c.shape, c.clip_rect, &mut texts);
    }
    // Where each widget named by its text sits, to catch text spilling out of its own widget.
    let mut widgets: std::collections::HashMap<String, Vec<egui::Rect>> = Default::default();
    for n in h.query_all_by(|n| n.label().is_some()) {
        let label = n.accesskit_node().label().unwrap_or_default();
        widgets.entry(label).or_default().push(n.rect());
    }
    let mut out = Vec::new();
    for (i, a) in texts.iter().enumerate() {
        let sideways = a.ink.left() < a.clip.left() - 0.5 || a.ink.right() > a.clip.right() + 0.5;
        if sideways && !a.elided {
            out.push(format!("{:?} clipped at {:?}", a.text, a.clip));
        }
        // Only a widget the text touches: a caption shares its name with the field beside it.
        let ink = a.ink.intersect(a.clip);
        let mine = widgets
            .get(&a.text)
            .into_iter()
            .flatten()
            .filter(|r| r.intersects(ink));
        let fits = |r: &egui::Rect| r.left() - 1.0 <= ink.left() && ink.right() <= r.right() + 1.0;
        if mine.clone().next().is_some() && !mine.clone().any(fits) {
            out.push(format!("{:?} runs outside its widget", a.text));
        }
        for b in &texts[i + 1..] {
            let both = a.ink.intersect(a.clip).intersect(b.ink.intersect(b.clip));
            if both.width() > 1.0 && both.height() > 1.0 {
                out.push(format!("{:?} overlaps {:?}", a.text, b.text));
            }
        }
    }
    out
}

/// The editor's English text padded about 40%, as a long translation runs (see
/// [`i18n::set_pseudo`]); English again when dropped.
fn pseudo() -> Switched {
    let s = switch("en");
    i18n::set_pseudo(true);
    s
}

fn en(key: &'static str) -> &'static str {
    jvid_app::i18n::CATALOG.tr(key)
}

/// The editor at its default 1280×720 with a title selected, each Inspector tab in turn, then
/// Preferences: no text clipped without an ellipsis or drawn over another.
#[test]
fn long_text_fits_the_editor_and_preferences() {
    if !isolated() {
        return;
    }
    let _p = pseudo();
    let select_all = format!("{} Ctrl+A", en("action.select_all"));
    let labels = [
        en("action.new_project"),
        en("bin.text"),
        en("bin.title"),
        en("timeline.track"),
        en("menu.edit"),
        &select_all,
    ];
    let mut h = titled_project(labels);
    // The Text tab's title thumbnails are drawn in system fonts, which differ between machines.
    click_leftmost(&mut h, en("panel.media"));
    let mut found = Vec::new();
    for tab in ["inspector.text", "inspector.video", "inspector.effects"] {
        click_rightmost(&mut h, en(tab));
        found.extend(clashes(&h).into_iter().map(|c| format!("{tab}: {c}")));
    }
    h.get_by_label(en("menu.edit")).click();
    h.run();
    h.get_by_label(en("menu.preferences")).click();
    h.run();
    for section in ["settings.general", "settings.audio"] {
        // The sidebar's row, before the section's own heading.
        h.get_all_by_label(en(section)).next().unwrap().click();
        h.run();
        found.extend(clashes(&h).into_iter().map(|c| format!("{section}: {c}")));
    }
    assert_eq!(found, Vec::<String>::new());
}

/// The Export dialog at the narrowest window and the Render Queue at half the default one.
#[test]
fn long_text_fits_export_dialog_and_render_queue() {
    let _p = pseudo();
    let project = jvid_core::model::Project::default();
    let mut dialog = ExportDialog::default();
    // A fixed folder, not the working directory, so the result matches on every machine.
    dialog.set_path("/exports/Project.mp4".into());
    dialog.open(project.sequence(), "Project");
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
    let mut found: Vec<String> = clashes(&h)
        .into_iter()
        .map(|c| format!("export: {c}"))
        .collect();

    use jvid_app::export::queue::{Row, Status, panel};
    let row = |name: &str, status| Row {
        path: format!("/exports/{name}").into(),
        preset: "YouTube 1080p".into(),
        duration: jvid_core::time::Time(154 * jvid_core::time::FLICKS_PER_SECOND),
        size: matches!(status, Status::Done).then_some(186_400_000),
        status,
    };
    let rows = vec![
        row("Intro Cut.mp4", Status::Done),
        row("Broken Take.mp4", Status::Failed("Disk full".into())),
        row("Old Draft.mp4", Status::Cancelled),
        row(
            "Main Edit.mp4",
            Status::Rendering {
                phase: jvid_app::export::Phase::Encoding { pass: 1 },
                retry: 0,
                progress: Some(0.42),
                left: Some(95.0),
            },
        ),
        row("Outro.mp4", Status::Waiting),
    ];
    for confirm in [false, true] {
        let rows = rows.clone();
        let mut fresh = true;
        let mut h = Harness::builder()
            .with_size([NARROWEST, 280.0])
            .build_ui(move |ui| {
                if !install_theme(ui, &mut fresh) {
                    panel(ui, &rows, confirm, true);
                }
            });
        h.run();
        let tag = if confirm { "queue confirm" } else { "queue" };
        found.extend(clashes(&h).into_iter().map(|c| format!("{tag}: {c}")));
    }
    assert_eq!(found, Vec::<String>::new());
}
