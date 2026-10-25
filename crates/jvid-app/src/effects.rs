//! The effects folder: user `.wgsl` effects in the config directory, re-read while the app runs
//! so a new or edited file is usable without a restart.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jvid_render::effect::{self, Shader};

use crate::i18n::{CATALOG, tr};

/// An effect file by stem: its parsed shader, or why it doesn't load.
pub type Entry = Result<Arc<Shader>, String>;

/// `<platform config dir>/effects`; `None` in tests, so they never touch the real folder.
pub fn dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    directories::ProjectDirs::from("", "", "jvid").map(|d| d.config_dir().join("effects"))
}

type Stamp = (Option<std::time::SystemTime>, u64);

/// The folder's effects as last read.
#[derive(Default)]
pub struct Library {
    /// By file stem: the file's (modified time, length) when last read and what it parsed to.
    files: BTreeMap<String, (Stamp, Entry)>,
    /// By file stem: the first plain comment line of the file, saying what the effect does.
    descriptions: BTreeMap<String, String>,
    /// Stems of the bundled examples whose file is exactly as shipped, so shown translated.
    shipped: BTreeSet<String>,
    /// Stems hidden from the Inspector's Add Effect, kept in the folder's `disabled.txt`.
    pub disabled: BTreeSet<String>,
    /// Bumped whenever an entry is added, changed or removed.
    pub generation: u64,
}

impl Library {
    /// Re-reads `dir`, creating it and adding bundled examples it hasn't been offered yet (see
    /// [`offer_bundled`]); only files whose modified time or length changed are read and
    /// re-parsed.
    /// shortcut: polls the folder instead of watching it, as no file-watch crate is in the stack.
    pub fn scan(&mut self, dir: &Path) {
        if let Err(e) = offer_bundled(dir) {
            tracing::warn!("effects: can't add the examples to {}: {e}", dir.display());
        }
        let mut seen = BTreeMap::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "wgsl")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && let Ok(meta) = entry.metadata()
            {
                seen.insert(stem.to_owned(), (path, (meta.modified().ok(), meta.len())));
            }
        }
        let before = self.files.len();
        self.files.retain(|stem, _| seen.contains_key(stem));
        self.shipped.retain(|stem| seen.contains_key(stem));
        let mut changed = self.files.len() != before;
        let disabled = std::fs::read_to_string(dir.join(DISABLED)).unwrap_or_default();
        let disabled: BTreeSet<String> = disabled.lines().map(str::to_owned).collect();
        if disabled != self.disabled {
            (self.disabled, changed) = (disabled, true);
        }
        for (stem, (path, stamp)) in seen {
            if self.files.get(&stem).is_some_and(|(old, _)| *old == stamp) {
                continue;
            }
            let src = std::fs::read_to_string(&path).map_err(|e| e.to_string());
            let desc = src.as_deref().map(description).unwrap_or_default();
            self.descriptions.insert(stem.clone(), desc);
            // shortcut: a copy seeded by an older jvid whose example has since changed counts as
            // edited and shows its own English; also compare past sources if that matters.
            if shipped_source(&stem).is_some_and(|s| src.as_deref() == Ok(s)) {
                self.shipped.insert(stem.clone());
            } else {
                self.shipped.remove(&stem);
            }
            let parsed = src.and_then(|src| effect::parse(&src).map(Arc::new));
            if let Err(e) = &parsed {
                tracing::warn!("effect {stem}: {e}");
            }
            self.files.insert(stem, (stamp, parsed));
            changed = true;
        }
        self.generation += u64::from(changed);
    }

    /// Marks `stem` as failing with `error` (e.g. the GPU rejected it) until its file changes.
    fn fail(&mut self, stem: &str, error: String) {
        if let Some((_, entry)) = self.files.get_mut(stem) {
            *entry = Err(error);
            self.generation += 1;
        }
    }

    pub fn get(&self, stem: &str) -> Option<&Entry> {
        self.files.get(stem).map(|(_, e)| e)
    }

    /// Whether `stem` is a bundled example whose file hasn't been edited.
    pub fn shipped(&self, stem: &str) -> bool {
        self.shipped.contains(stem)
    }

    pub fn description(&self, stem: &str) -> &str {
        shipped_text(self.shipped(stem), stem, "_about")
            .unwrap_or_else(|| self.descriptions.get(stem).map_or("", String::as_str))
    }

    /// The effects whose name, file or description contains `query` (case-insensitive).
    pub fn rows(&self, query: &str) -> Vec<Row> {
        let q = query.trim().to_lowercase();
        self.entries()
            .map(|(stem, e)| Row {
                stem: stem.to_owned(),
                name: e.as_ref().map_or_else(
                    |_| format!("{stem}.wgsl"),
                    |s| name(stem, self.shipped(stem), s).to_owned(),
                ),
                description: self.description(stem).to_owned(),
                bundled: bundled(stem),
                shipped: self.shipped(stem),
                disabled: self.disabled.contains(stem),
                error: e.as_ref().err().cloned(),
            })
            .filter(|r| {
                [&r.name, &r.stem, &r.description]
                    .iter()
                    .any(|t| t.to_lowercase().contains(&q))
            })
            .collect()
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, &Entry)> {
        self.files.iter().map(|(k, (_, e))| (k.as_str(), e))
    }
}

/// The app-wide library and when the folder was last read.
static LIBRARY: Mutex<(Library, Option<Instant>)> = Mutex::new((
    Library {
        files: BTreeMap::new(),
        descriptions: BTreeMap::new(),
        shipped: BTreeSet::new(),
        disabled: BTreeSet::new(),
        generation: 0,
    },
    None,
));

/// Calls `f` with the library, re-reading the folder if it's been a second since the last read.
pub fn with<R>(f: impl FnOnce(&Library) -> R) -> R {
    let mut lock = LIBRARY.lock().unwrap_or_else(|e| e.into_inner());
    let (lib, checked) = &mut *lock;
    if checked.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
        *checked = Some(Instant::now());
        if let Some(dir) = dir() {
            lib.scan(&dir);
        }
    }
    f(lib)
}

/// An Effects tab row.
#[derive(Debug, PartialEq)]
pub struct Row {
    pub stem: String,
    /// The effect's display name, or its file name when it doesn't load.
    pub name: String,
    pub description: String,
    pub bundled: bool,
    /// See [`Library::shipped`].
    pub shipped: bool,
    pub disabled: bool,
    /// Why the file doesn't load, when it doesn't.
    pub error: Option<String>,
}

const DISABLED: &str = "disabled.txt";
/// The bundled examples' stems already written to the folder once, one per line.
const OFFERED: &str = "bundled.txt";

/// Creates `dir` and writes each bundled example not yet listed in [`OFFERED`], so a new
/// release's examples reach an existing folder while one the user deleted stays deleted, and
/// deleting the whole folder brings them all back. A folder from before [`OFFERED`] counts the
/// examples it holds as offered.
fn offer_bundled(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let stem = |file: &'static str| file.strip_suffix(".wgsl").unwrap_or(file);
    let path = dir.join(OFFERED);
    let (mut offered, mut dirty): (BTreeSet<&str>, _) = match std::fs::read_to_string(&path) {
        Ok(text) => (
            effect::BUNDLED
                .iter()
                .map(|(f, _)| stem(f))
                .filter(|s| text.lines().any(|l| l == *s))
                .collect(),
            false,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
            effect::BUNDLED
                .iter()
                .filter(|(f, _)| dir.join(f).exists())
                .map(|(f, _)| stem(f))
                .collect(),
            true,
        ),
        Err(e) => return Err(e),
    };
    for (file, src) in effect::BUNDLED {
        if offered.insert(stem(file)) {
            dirty = true;
            if !dir.join(file).exists() {
                std::fs::write(dir.join(file), src)?;
            }
        }
    }
    if dirty {
        let out: String = offered.iter().map(|s| format!("{s}\n")).collect();
        std::fs::write(path, out)?;
    }
    Ok(())
}

/// What a timeline drop of an Effects tab row carries: the effect's file stem.
#[derive(Clone)]
pub struct Drag(pub String);

/// The first `//` line that isn't a `@jvid`/`@param` directive, without the slashes.
pub fn description(src: &str) -> String {
    let lines = src.lines().filter_map(|l| l.trim().strip_prefix("//"));
    let mut lines = lines
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('@'));
    lines.next().unwrap_or_default().to_owned()
}

/// Whether `stem` is one of the examples shipped with the app (recreated with the folder).
pub fn bundled(stem: &str) -> bool {
    shipped_source(stem).is_some()
}

fn shipped_source(stem: &str) -> Option<&'static str> {
    effect::BUNDLED
        .iter()
        .find(|(f, _)| f.strip_suffix(".wgsl") == Some(stem))
        .map(|(_, src)| *src)
}

/// A bundled example's `bundled.<stem><part>` text, when its file is as `shipped`. Any other
/// effect's name and words are its author's, shown as written in the file.
fn shipped_text(shipped: bool, stem: &str, part: &str) -> Option<&'static str> {
    shipped
        .then(|| CATALOG.get(&format!("bundled.{stem}{part}")))
        .flatten()
}

/// The name to show for effect `stem`, see [`Library::shipped`]: a shader header's in Title Case
/// whatever its file says.
pub fn name(stem: &str, shipped: bool, shader: &Shader) -> String {
    shipped_text(shipped, stem, "").map_or_else(|| title_case_name(&shader.name), str::to_owned)
}

/// `film grain` -> `Film Grain`, leaving short joining words lowercase after the first and the
/// rest of each word as written, so `LUT` stays.
pub(crate) fn title_case_name(name: &str) -> String {
    const SMALL: [&str; 15] = [
        "a", "an", "and", "as", "at", "but", "by", "for", "in", "nor", "of", "on", "or", "the",
        "to",
    ];
    name.split(' ')
        .enumerate()
        .map(|(i, w)| {
            if i > 0 && SMALL.contains(&w.to_lowercase().as_str()) {
                return w.to_lowercase();
            }
            let mut c = w.chars();
            c.next()
                .map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `desc` split at its backticks: text, then whether it's a `parameter` id.
fn param_spans(desc: &str) -> impl Iterator<Item = (&str, bool)> {
    desc.split('`')
        .enumerate()
        .map(|(i, s)| (s, i % 2 == 1))
        .filter(|(s, _)| !s.is_empty())
}

/// `row`'s description (or error) in `ink`, at most `rows` lines `width` wide, with its
/// backticked parameter ids as their labels in Ink 2.
pub fn about(row: &Row, width: f32, rows: usize, ink: egui::Color32) -> egui::text::LayoutJob {
    use suite_ui::tokens::color;
    let mut job = egui::text::LayoutJob::default();
    let format = |color| egui::TextFormat::simple(suite_ui::theme::body(), color);
    match &row.error {
        Some(e) => job.append(e, 0.0, format(ink)),
        None => {
            for (s, param) in param_spans(&row.description) {
                if param {
                    job.append(
                        &self::param(&row.stem, row.shipped, s),
                        0.0,
                        format(color::ink_2()),
                    );
                } else {
                    job.append(s, 0.0, format(ink));
                }
            }
        }
    }
    job.wrap.max_width = width;
    job.wrap.max_rows = rows;
    job
}

/// `row`'s thumbnail filling `rect`: its effect on the thumbnail frame, black until drawn, or
/// for one that doesn't load the frame dimmed under a warning.
pub fn paint_thumb(ui: &egui::Ui, rect: egui::Rect, row: &Row) {
    use suite_ui::tokens::{color, radius};
    let t = thumbs(ui.ctx());
    let corner = egui::CornerRadius::same(radius::CONTROL);
    ui.painter().rect_filled(rect, corner, egui::Color32::BLACK);
    let (tex, tint) = match &row.error {
        Some(_) => (t.frame, egui::Color32::from_gray(80)),
        None => (t.by_stem.get(&row.stem).copied(), egui::Color32::WHITE),
    };
    if let Some(id) = tex {
        egui::Image::new((id, rect.size()))
            .tint(tint)
            .corner_radius(radius::CONTROL)
            .paint_at(ui, rect);
    }
    if row.error.is_some() {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            suite_ui::theme::icon::CIRCLE_ALERT,
            suite_ui::theme::icon_font(),
            color::red(),
        );
    }
}

/// Each effect run over the thumbnail frame, kept in egui's memory for the Effects tab and the
/// Inspector's Add Effect menu; see [`draw_thumbs`].
#[derive(Clone, Default)]
pub struct Thumbs {
    /// The bare frame, which an effect that doesn't load shows dimmed.
    pub frame: Option<egui::TextureId>,
    pub by_stem: BTreeMap<String, egui::TextureId>,
    /// The frame and library generations they were drawn from.
    drawn: Option<(u64, u64)>,
}

fn thumbs_id() -> egui::Id {
    egui::Id::new("effect thumbnails")
}

fn wanted_id() -> egui::Id {
    egui::Id::new("effect thumbnails wanted")
}

/// The thumbnails as last drawn.
pub fn thumbs(ctx: &egui::Context) -> Arc<Thumbs> {
    ctx.data(|d| d.get_temp(thumbs_id())).unwrap_or_default()
}

/// Asks for the thumbnails to be kept current next frame, as the Add Effect menu shows them.
pub fn want_thumbs(ctx: &egui::Context) {
    let pass = ctx.cumulative_pass_nr();
    let last = ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Option<u64>>(wanted_id())
            .replace(pass)
    });
    // Just opened: one more frame draws them.
    if last.is_none_or(|l| l + 1 < pass) {
        ctx.request_repaint();
    }
}

/// Redraws the thumbnails if `shown` (the Effects tab is open) or [`want_thumbs`] asked last
/// frame, and the thumbnail frame or the effects changed since they were last drawn.
pub fn draw_thumbs(ctx: &egui::Context, preview: &crate::preview::Preview, shown: bool) {
    let pass = ctx.cumulative_pass_nr();
    let wanted = ctx.data(|d| d.get_temp::<Option<u64>>(wanted_id()).flatten());
    let wanted = wanted.is_some_and(|l| l + 1 >= pass);
    let Some((frame, generation)) = preview.thumb_frame().filter(|_| shown || wanted) else {
        return;
    };
    let old = thumbs(ctx);
    let (lib, shaders): (u64, Vec<(String, Arc<Shader>)>) = with(|l| {
        let ok = l
            .entries()
            .filter_map(|(k, e)| Some((k.to_owned(), e.clone().ok()?)));
        (l.generation, ok.collect())
    });
    if old.drawn == Some((generation, lib)) {
        return;
    }
    // Shaders build only when the effects change; a new frame just reruns them.
    let rebuild = old.drawn.is_none_or(|(_, g)| g != lib);
    let mut by_stem = BTreeMap::new();
    for (stem, shader) in &shaders {
        if rebuild {
            let built = preview.with_thumbs(|t, d, _| t.set_effect(d, stem, Some(shader)));
            if let Some(Err(e)) = built {
                tracing::warn!("effect {stem}: {e}");
                // The library changes, so the next frame draws again without it.
                fail(stem, e);
                ctx.request_repaint();
                continue;
            }
        }
        let fx = jvid_core::model::Effect {
            shader: stem.clone(),
            params: Default::default(),
            enabled: true,
            mask: None,
        };
        let id = preview.draw_thumb(old.by_stem.get(stem).copied(), |t, d, q| {
            t.effect(d, q, &fx)
        });
        by_stem.extend(id.map(|id| (stem.clone(), id)));
    }
    if rebuild {
        // shortcut: a removed effect's egui texture stays registered; free it if folders churn.
        for stem in old.by_stem.keys().filter(|k| !by_stem.contains_key(*k)) {
            preview.with_thumbs(|t, d, _| t.set_effect(d, stem, None));
        }
    }
    let new = Thumbs {
        frame: Some(frame),
        by_stem,
        drawn: Some((generation, lib)),
    };
    ctx.data_mut(|d| d.insert_temp(thumbs_id(), Arc::new(new)));
}

/// The caption to show for parameter `id` of effect `stem`: a bundled example's translated,
/// else the id as words (`grain_size` -> `Grain Size`).
pub fn param(stem: &str, shipped: bool, id: &str) -> String {
    shipped_text(shipped, stem, &format!("_{id}")).map_or_else(|| title_case(id), str::to_owned)
}

fn title_case(id: &str) -> String {
    id.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What New Effect writes: a working effect with one parameter to start from. `EFFECT_NAME`
/// and `EFFECT_ABOUT` become the shown name and description in the UI language.
const TEMPLATE: &str = r#"// @jvid effect "EFFECT_NAME"
// @param amount float 0.0..1.0 = 0.0

// EFFECT_ABOUT
fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = textureSample(src, smp, uv);
    return vec4<f32>(mix(c.rgb, 1.0 - c.rgb, fx.amount), c.a);
}
"#;

/// `dir/<base>.wgsl`, or `<base> 2.wgsl` and so on if that's taken.
fn free_path(dir: &Path, base: &str) -> PathBuf {
    (1..)
        .map(|n| match n {
            1 => dir.join(format!("{base}.wgsl")),
            n => dir.join(format!("{base} {n}.wgsl")),
        })
        .find(|p| !p.exists())
        .expect("some name is free")
}

/// Writes the template as a new file in `dir`, returning its path.
pub fn create(dir: &Path) -> std::io::Result<PathBuf> {
    let path = free_path(dir, "new_effect");
    let name = tr!("bin.template_name").replace('"', "'");
    let about = tr!("bin.template_about").replace('\n', " ");
    let source = TEMPLATE
        .replacen("EFFECT_NAME", &name, 1)
        .replacen("EFFECT_ABOUT", &about, 1);
    std::fs::write(&path, source).map(|()| path)
}

/// Copies `stem`'s file to a free name beside it.
pub fn duplicate(dir: &Path, stem: &str) -> std::io::Result<PathBuf> {
    let path = free_path(dir, &format!("{stem} copy"));
    std::fs::copy(dir.join(format!("{stem}.wgsl")), &path).map(|_| path)
}

/// Deletes a user effect's file; bundled examples can only be disabled.
pub fn delete(dir: &Path, stem: &str) -> std::io::Result<()> {
    if bundled(stem) {
        return Err(std::io::Error::other("bundled effects can't be deleted"));
    }
    std::fs::remove_file(dir.join(format!("{stem}.wgsl")))
}

/// Hides `stem` from Add Effect, or shows it again.
pub fn set_disabled(dir: &Path, stem: &str, off: bool) -> std::io::Result<()> {
    let text = std::fs::read_to_string(dir.join(DISABLED)).unwrap_or_default();
    let mut set: BTreeSet<&str> = text.lines().collect();
    if off {
        set.insert(stem);
    } else {
        set.remove(stem);
    }
    let out: String = set.iter().map(|s| format!("{s}\n")).collect();
    std::fs::write(dir.join(DISABLED), out)
}

/// Loads `dir` into the app-wide library, as tests have no effects folder.
#[cfg(test)]
pub fn scan_for_test(dir: &Path) {
    LIBRARY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .0
        .scan(dir);
}

/// [`Library::fail`] on the app-wide library, so the Inspector and Effects tab show the error.
pub fn fail(stem: &str, error: String) {
    LIBRARY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .0
        .fail(stem, error);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_examples_show_translated_until_edited() {
        assert_eq!(param("mine", false, "grain_size"), "Grain Size");
        let tmp = std::env::temp_dir().join(format!("jvid-effects-shipped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut lib = Library::default();
        lib.scan(&tmp);
        for (file, src) in effect::BUNDLED {
            let stem = file.strip_suffix(".wgsl").unwrap();
            let shader = effect::parse(src).unwrap();
            assert!(lib.shipped(stem), "{stem}");
            let mut parts = vec![String::new(), "_about".into()];
            parts.extend(shader.params.iter().map(|p| format!("_{}", p.name)));
            for part in &parts {
                let key = format!("bundled.{stem}{part}");
                assert_eq!(shipped_text(true, stem, part), CATALOG.get(&key), "{key}");
                let [en, es] = ["en", "es"].map(|l| CATALOG.get_in(l, &key));
                assert!(en.is_some() && es.is_some() && en != es, "{key}");
            }
            // English reads as the file, in Title Case, and as the old title-cased ids did.
            assert_eq!(name(stem, true, &shader), title_case_name(&shader.name));
            assert_eq!(lib.description(stem), description(src));
            for p in &shader.params {
                assert_eq!(param(stem, true, &p.name), title_case(&p.name));
            }
        }

        // Edited, even by a byte, the example is the user's: shown as its file says.
        let vignette = shipped_source("vignette").unwrap();
        let mine = vignette.replace("\"Vignette\"", "\"Viñeta propia\"");
        std::fs::write(tmp.join("vignette.wgsl"), &mine).unwrap();
        let grain = format!("{}\n", shipped_source("film_grain").unwrap());
        std::fs::write(tmp.join("film_grain.wgsl"), grain).unwrap();
        duplicate(&tmp, "chromatic_aberration").unwrap();
        lib.scan(&tmp);
        assert!(!lib.shipped("vignette") && !lib.shipped("film_grain"));
        assert!(lib.shipped("chromatic_aberration") && !lib.shipped("chromatic_aberration copy"));
        let row = |q: &str| lib.rows(q).pop().unwrap().name;
        assert_eq!(row("propia"), "Viñeta Propia");
        assert_eq!(row("aberration copy"), "Chromatic Aberration");
        assert_eq!(
            param("film_grain", lib.shipped("film_grain"), "grain"),
            "Grain"
        );

        // Put back as shipped, it's translated again.
        std::fs::write(tmp.join("vignette.wgsl"), vignette).unwrap();
        lib.scan(&tmp);
        assert!(lib.shipped("vignette"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn names_are_title_case_and_descriptions_mark_params() {
        assert_eq!(title_case_name("film grain"), "Film Grain");
        assert_eq!(title_case_name("the look of a LUT"), "The Look of a LUT");
        assert_eq!(title_case_name(""), "");
        let spans: Vec<_> = param_spans("up to `amount` px, `grain`").collect();
        assert_eq!(
            spans,
            [
                ("up to ", false),
                ("amount", true),
                (" px, ", false),
                ("grain", true)
            ]
        );
        // An unclosed backtick marks the rest as a parameter, as a label it simply won't find.
        assert_eq!(param_spans("`a").collect::<Vec<_>>(), [("a", true)]);
    }

    #[test]
    fn a_broken_effect_shows_the_frame_dimmed_and_its_error() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let row = Row {
            stem: "broken".into(),
            name: "broken.wgsl".into(),
            description: "Does `amount` things".into(),
            bundled: false,
            shipped: false,
            disabled: false,
            error: Some("expected `)`".into()),
        };
        let about = about(&row, 200.0, 2, egui::Color32::RED);
        assert_eq!(about.text, "expected `)`", "the error, as written");
        let (mut frame, mut fresh) = (None, true);
        let mut h = egui_kittest::Harness::builder()
            .with_size([100.0, 60.0])
            .wgpu()
            .build_ui(move |ui| {
                if suite_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                // A mid-grey frame, as the thumbnail frame.
                let tex = frame.get_or_insert_with(|| {
                    let img =
                        egui::ColorImage::new([16, 9], vec![egui::Color32::from_gray(200); 144]);
                    ui.ctx().load_texture("frame", img, Default::default())
                });
                let t = Thumbs {
                    frame: Some(tex.id()),
                    ..Thumbs::default()
                };
                ui.data_mut(|d| d.insert_temp(thumbs_id(), Arc::new(t)));
                let rect = egui::Rect::from_min_size(egui::pos2(10.0, 7.0), egui::vec2(80.0, 45.0));
                paint_thumb(ui, rect, &row);
            });
        h.run();
        let img = h.render().unwrap();
        let near_corner = img.get_pixel(16, 12).0;
        assert!(
            near_corner[..3].iter().all(|c| (40..120).contains(c)),
            "dimmed: {near_corner:?}"
        );
        let warning = (30..70)
            .flat_map(|x| (20..40).map(move |y| (x, y)))
            .any(|(x, y)| {
                let p = img.get_pixel(x, y).0;
                p[0] > 150 && p[1] < 120
            });
        assert!(warning, "a red warning sits on it");
    }

    #[test]
    fn creates_examples_and_picks_up_new_edited_and_removed_files() {
        let tmp = std::env::temp_dir().join(format!("jvid-effects-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut lib = Library::default();
        lib.scan(&tmp);
        let names: Vec<_> = lib.entries().map(|(k, _)| k.to_owned()).collect();
        assert_eq!(
            names,
            [
                "chroma_key",
                "chromatic_aberration",
                "film_grain",
                "gaussian_blur",
                "pixelate",
                "vignette"
            ]
        );
        assert!(lib.entries().all(|(_, e)| e.is_ok()));
        let g = lib.generation;
        lib.scan(&tmp);
        assert_eq!(lib.generation, g, "unchanged files don't bump it");
        lib.fail("vignette", "GPU said no".into());
        assert!(lib.entries().any(|(k, e)| k == "vignette" && e.is_err()));
        assert!(lib.generation > g);
        let g = lib.generation;

        // Dropped in while running: usable on the next scan.
        let src = effect::BUNDLED[0].1.replace("\"Vignette\"", "\"Mine\"");
        std::fs::write(tmp.join("mine.wgsl"), &src).unwrap();
        lib.scan(&tmp);
        let mine = |lib: &Library| {
            lib.entries()
                .find(|(k, _)| *k == "mine")
                .map(|(_, e)| e.clone())
        };
        assert_eq!(mine(&lib).unwrap().unwrap().name, "Mine");
        assert!(lib.generation > g);

        std::fs::write(tmp.join("mine.wgsl"), "// @jvid effect \"Mine\"\n").unwrap();
        lib.scan(&tmp);
        assert!(mine(&lib).unwrap().is_err());
        std::fs::remove_file(tmp.join("mine.wgsl")).unwrap();
        lib.scan(&tmp);
        assert!(mine(&lib).is_none());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn new_examples_reach_an_existing_folder_once() {
        let tmp = std::env::temp_dir().join(format!("jvid-effects-offer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // A folder from before chroma_key and the offered list.
        for (file, src) in effect::BUNDLED {
            if file != "chroma_key.wgsl" {
                std::fs::write(tmp.join(file), src).unwrap();
            }
        }
        let mut lib = Library::default();
        lib.scan(&tmp);
        assert!(lib.shipped("chroma_key"));
        std::fs::remove_file(tmp.join("chroma_key.wgsl")).unwrap();
        lib.scan(&tmp);
        assert!(!tmp.join("chroma_key.wgsl").exists() && lib.get("chroma_key").is_none());
        assert_eq!(lib.entries().count(), effect::BUNDLED.len() - 1);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn describes_creates_duplicates_disables_and_deletes() {
        let tmp = std::env::temp_dir().join(format!("jvid-effects-ops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut lib = Library::default();
        lib.scan(&tmp);
        assert!(
            lib.description("vignette")
                .starts_with("Darkens towards the corners")
        );
        let rows = lib.rows("CORNERS");
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].name.as_str(), rows[0].bundled), ("Vignette", true));
        assert_eq!(lib.rows("").len(), effect::BUNDLED.len());
        assert!(bundled("vignette") && !bundled("new_effect"));

        // A broken user shader: listed by file name with the compiler's message.
        std::fs::write(
            tmp.join("broken.wgsl"),
            "// @jvid effect \"Broken\"\nfn nope(\n",
        )
        .unwrap();
        lib.scan(&tmp);
        let broken = lib.rows("broken").pop().unwrap();
        assert_eq!(broken.name, "broken.wgsl");
        assert!(!broken.bundled && broken.error.is_some_and(|e| !e.is_empty()));
        std::fs::remove_file(tmp.join("broken.wgsl")).unwrap();

        let made = create(&tmp).unwrap();
        assert_eq!(made, tmp.join("new_effect.wgsl"));
        assert_eq!(create(&tmp).unwrap(), tmp.join("new_effect 2.wgsl"));
        duplicate(&tmp, "vignette").unwrap();
        lib.scan(&tmp);
        assert!(
            lib.get("new_effect").unwrap().is_ok(),
            "the template parses"
        );
        assert!(lib.description("new_effect").starts_with("Blends towards"));
        assert!(lib.get("vignette copy").unwrap().is_ok());

        set_disabled(&tmp, "film_grain", true).unwrap();
        lib.scan(&tmp);
        assert!(lib.disabled.contains("film_grain"));
        set_disabled(&tmp, "film_grain", false).unwrap();
        lib.scan(&tmp);
        assert!(lib.disabled.is_empty());

        assert!(delete(&tmp, "vignette").is_err());
        delete(&tmp, "vignette copy").unwrap();
        lib.scan(&tmp);
        assert!(lib.get("vignette copy").is_none() && lib.get("vignette").is_some());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
