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
    /// Re-reads `dir`, creating it with the bundled examples if it doesn't exist (so deleting
    /// the whole folder brings the examples back); only files whose modified time or length
    /// changed are read and re-parsed.
    /// shortcut: polls the folder instead of watching it, as no file-watch crate is in the stack.
    pub fn scan(&mut self, dir: &Path) {
        if !dir.exists() {
            let made = std::fs::create_dir_all(dir).and_then(|()| {
                effect::BUNDLED
                    .iter()
                    .try_for_each(|(name, src)| std::fs::write(dir.join(name), src))
            });
            if let Err(e) = made {
                tracing::warn!("effects: can't create {}: {e}", dir.display());
            }
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
    pub disabled: bool,
    /// Why the file doesn't load, when it doesn't.
    pub error: Option<String>,
}

const DISABLED: &str = "disabled.txt";

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

/// The name to show for effect `stem`, see [`Library::shipped`].
pub fn name<'a>(stem: &str, shipped: bool, shader: &'a Shader) -> &'a str {
    shipped_text(shipped, stem, "").unwrap_or(&shader.name)
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
            // English reads exactly as the file and the old title-cased ids did.
            assert_eq!(name(stem, true, &shader), shader.name);
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
        assert_eq!(row("propia"), "Viñeta propia");
        assert_eq!(row("aberration copy"), "Chromatic aberration");
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
    fn creates_examples_and_picks_up_new_edited_and_removed_files() {
        let tmp = std::env::temp_dir().join(format!("jvid-effects-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut lib = Library::default();
        lib.scan(&tmp);
        let names: Vec<_> = lib.entries().map(|(k, _)| k.to_owned()).collect();
        assert_eq!(names, ["chromatic_aberration", "film_grain", "vignette"]);
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
        assert_eq!(lib.rows("").len(), 3);
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
