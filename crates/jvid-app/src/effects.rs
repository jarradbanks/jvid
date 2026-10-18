//! The effects folder: user `.wgsl` effects in the config directory, re-read while the app runs
//! so a new or edited file is usable without a restart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jvid_render::effect::{self, Shader};

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
        let mut changed = self.files.len() != before;
        for (stem, (path, stamp)) in seen {
            if self.files.get(&stem).is_some_and(|(old, _)| *old == stamp) {
                continue;
            }
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|src| effect::parse(&src).map(Arc::new));
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

    pub fn entries(&self) -> impl Iterator<Item = (&str, &Entry)> {
        self.files.iter().map(|(k, (_, e))| (k.as_str(), e))
    }
}

/// The app-wide library and when the folder was last read.
static LIBRARY: Mutex<(Library, Option<Instant>)> = Mutex::new((
    Library {
        files: BTreeMap::new(),
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
}
