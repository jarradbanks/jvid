//! Settings paths/read-write and docking-only window persistence.
//! The `settings.toml` format lives in `jvid_core::settings`.

use std::path::{Path, PathBuf};
use std::{fs, io};

use jvid_core::settings::Settings;

use crate::i18n::tr;
use crate::workspaces::{Panels, Snapshot, Workspaces};

const WORKSPACES_KEY: &str = "workspaces";
const WORKSPACE_LAYOUT_KEY: &str = "workspace_layout";

/// User layouts and current placement live beside the legacy `dock` in window storage.
/// A bad personal-layout collection never prevents restoring the current/legacy layout.
pub fn load_workspaces(
    storage: Option<&dyn eframe::Storage>,
) -> (Workspaces, egui_dock::DockState<crate::app::Tab>) {
    let panels = Panels::default();
    let personal = storage
        .and_then(|s| eframe::get_value(s, WORKSPACES_KEY))
        .unwrap_or_default();
    let snapshot: Option<Snapshot> = storage
        .and_then(|s| eframe::get_value(s, WORKSPACE_LAYOUT_KEY))
        .or_else(|| {
            let storage = storage?;
            let dock: egui_dock::DockState<crate::app::Tab> = eframe::get_value(storage, "dock")?;
            // Read the old private surface index separately, then bounds-check every step.
            // Calling DockState::focused_leaf on malformed persisted focus can panic.
            #[derive(serde::Deserialize)]
            struct Focus {
                focused_surface: Option<egui_dock::SurfaceIndex>,
            }
            let focused = eframe::get_value::<Focus>(storage, "dock")
                .and_then(|focus| focus.focused_surface)
                .and_then(|index| dock.get_surface(index))
                .and_then(|surface| surface.node_tree())
                .and_then(|tree| tree.focused_leaf().and_then(|node| tree.iter().nth(node.0)))
                .and_then(|node| node.get_leaf())
                .and_then(|leaf| leaf.tabs.get(leaf.active.0))
                .and_then(|tab| panels.id(*tab));
            let mut snapshot = Snapshot::legacy(&dock, panels);
            snapshot.focused = focused.map(str::to_owned);
            Some(snapshot)
        });
    let dock = snapshot.map_or_else(crate::app::default_dock, |snapshot| {
        snapshot.restore(panels)
    });
    (personal, dock)
}

/// Persist the full pre-maximize layout, never the single-panel temporary view.
/// Keep writing legacy `dock` so older builds can still restore all existing Tab names.
pub fn save_workspaces(
    storage: &mut dyn eframe::Storage,
    workspaces: &Workspaces,
    dock: &egui_dock::DockState<crate::app::Tab>,
) {
    let dock = workspaces.normal_dock(dock);
    eframe::set_value(storage, "dock", dock);
    eframe::set_value(storage, WORKSPACES_KEY, workspaces);
    eframe::set_value(
        storage,
        WORKSPACE_LAYOUT_KEY,
        &Snapshot::capture(dock, Panels::default()),
    );
}

/// `<platform config dir>/settings.toml`, or `None` if the platform has no home dir.
pub fn path() -> Option<PathBuf> {
    jvid_paths::config_dir().map(|d| d.join("settings.toml"))
}

/// `<platform config dir>/themes`, where theme `.toml` files go.
pub fn themes_dir() -> Option<PathBuf> {
    jvid_paths::config_dir().map(|d| d.join("themes"))
}

/// Never fails: a missing, unreadable or malformed file gives defaults.
pub fn load() -> Settings {
    path().map_or_else(Settings::default, |p| load_from(&p))
}

pub fn load_from(path: &std::path::Path) -> Settings {
    match fs::read_to_string(path) {
        Ok(s) => Settings::from_toml(&s).unwrap_or_else(|e| {
            tracing::warn!(
                "settings: {} is not valid TOML, using defaults: {e}",
                path.display()
            );
            Settings::default()
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Settings::default(),
        Err(e) => {
            tracing::warn!(
                "settings: can't read {}, using defaults: {e}",
                path.display()
            );
            Settings::default()
        }
    }
}

/// Sets `language` in the settings file; see [`save_string_to`].
pub fn save_language_to(path: &Path, language: &str) -> io::Result<()> {
    save_string_to(path, "language", language)
}

/// Sets the string setting `key` in the settings file; see [`save_value_to`].
pub fn save_string_to(path: &Path, key: &str, value: &str) -> io::Result<()> {
    save_value_to(path, key, value.into())
}

/// Sets setting `key` in the settings file, keeping every other byte, comments included. A
/// file that isn't TOML, or whose `key` holds another type, is left as it is and reported, so
/// a hand edit with a typo isn't overwritten.
pub fn save_value_to(path: &Path, key: &str, value: toml::Value) -> io::Result<()> {
    let old = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let not_written = || io::Error::other(tr!("settings.not_written"));
    let parsed = toml::de::DeTable::parse(&old).map_err(io::Error::other)?;
    let mut line = toml::Table::new();
    line.insert(key.into(), value.clone());
    // The parser's spans see through quoted keys, multiline strings and comments.
    let found = parsed
        .get_ref()
        .iter()
        .find(|(k, _)| k.get_ref().as_ref() == key);
    let new = match found {
        Some((_, v)) if v.get_ref().type_str() == value.type_str() => {
            let at = v.span();
            let value = line[key].to_string();
            [&old[..at.start], &value, &old[at.end..]].concat()
        }
        Some(_) => return Err(not_written()),
        // The top of the file is always outside any table.
        None => line.to_string() + &old,
    };
    let mut before: toml::Table = old.parse().map_err(io::Error::other)?;
    let mut after: toml::Table = new.parse().map_err(io::Error::other)?;
    let written = after.remove(key);
    before.remove(key);
    if written.as_ref() != Some(&value) || before != after {
        return Err(not_written());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, new)
}

// shortcut: nothing saves settings until there is a settings UI; drop the allow then.
#[allow(dead_code)]
pub fn save_to(settings: &Settings, path: &std::path::Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, settings.to_toml())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Memory(std::collections::HashMap<String, String>);
    impl eframe::Storage for Memory {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.into(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn workspaces_restart_rename_delete_and_maximize_persist_the_normal_layout() {
        let (mut workspaces, mut dock) = load_workspaces(None);
        workspaces.apply(
            &mut dock,
            crate::workspaces::Builtin::Captions.layout(Panels::default()),
        );
        workspaces
            .save("Original", &dock, Panels::default())
            .unwrap();
        workspaces.save("Remove", &dock, Panels::default()).unwrap();
        workspaces.rename(0, "Personal captions").unwrap();
        assert!(workspaces.delete(1));
        workspaces.toggle_maximize(&mut dock);
        assert_eq!(dock.iter_all_tabs().count(), 1);
        let mut storage = Memory::default();
        save_workspaces(&mut storage, &workspaces, &dock);
        let dir = tempfile::tempdir().unwrap();
        // Round-trip actual persisted strings through a file and a fresh storage instance.
        let file = dir.path().join("window.toml");
        fs::write(&file, toml::to_string(&storage.0).unwrap()).unwrap();
        let storage = Memory(toml::from_str(&fs::read_to_string(file).unwrap()).unwrap());
        let (restored, mut dock) = load_workspaces(Some(&storage));
        assert!(!restored.is_maximized());
        assert_eq!(restored.personal.len(), 1);
        assert_eq!(restored.personal[0].name, "Personal captions");
        assert_eq!(dock.iter_all_tabs().count(), 5);
        assert_eq!(
            dock.find_active_focused().map(|(_, tab)| *tab),
            Some(crate::app::Tab::Transcript)
        );
        let legacy: egui_dock::DockState<crate::app::Tab> =
            eframe::get_value(&storage, "dock").unwrap();
        assert_eq!(legacy.iter_all_tabs().count(), 5);
        assert!(!storage.0[WORKSPACES_KEY].contains("normal:"));
    }

    #[test]
    fn corrupt_new_storage_recovers_legacy_without_losing_closed_panel_state() {
        let mut storage = Memory::default();
        let legacy = crate::workspaces::Builtin::Audio.layout(Panels::default());
        eframe::set_value(&mut storage, "dock", &legacy);
        storage.0.insert(WORKSPACES_KEY.into(), "broken".into());
        storage
            .0
            .insert(WORKSPACE_LAYOUT_KEY.into(), "broken".into());
        let (workspaces, restored) = load_workspaces(Some(&storage));
        assert!(workspaces.personal.is_empty());
        assert_eq!(
            restored.find_tab(&crate::app::Tab::Media),
            legacy.find_tab(&crate::app::Tab::Media)
        );
        assert_eq!(restored.iter_all_tabs().count(), 4);
        assert!(restored.find_tab(&crate::app::Tab::Transcript).is_none());
    }

    #[test]
    fn legacy_focus_survives_and_invalid_surface_focus_is_ignored() {
        let mut storage = Memory::default();
        let mut dock = crate::app::default_dock();
        let path = dock.find_tab(&crate::app::Tab::Inspector).unwrap();
        dock.set_focused_node_and_surface(path.node_path());
        eframe::set_value(&mut storage, "dock", &dock);
        let (_, mut restored) = load_workspaces(Some(&storage));
        assert_eq!(
            restored.find_active_focused().map(|(_, tab)| *tab),
            Some(crate::app::Tab::Inspector)
        );

        #[derive(serde::Serialize)]
        struct CorruptFocus {
            surfaces: Vec<egui_dock::Surface<crate::app::Tab>>,
            focused_surface: Option<egui_dock::SurfaceIndex>,
        }
        eframe::set_value(
            &mut storage,
            "dock",
            &CorruptFocus {
                surfaces: dock.iter_surfaces().cloned().collect(),
                focused_surface: Some(egui_dock::SurfaceIndex(999)),
            },
        );
        let (_, mut restored) = load_workspaces(Some(&storage));
        assert_eq!(restored.iter_all_tabs().count(), 4);
        assert!(restored.find_active_focused().is_none());
    }

    #[test]
    fn missing_file_is_default_and_save_round_trips() {
        let dir = std::env::temp_dir().join(format!("jvid-settings-{}", std::process::id()));
        let path = dir.join("sub").join("settings.toml");
        assert_eq!(load_from(&path), Settings::default());
        let s = Settings {
            autosave_secs: 15,
            ..Settings::default()
        };
        save_to(&s, &path).unwrap();
        assert_eq!(load_from(&path), s);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn language_saves_into_the_existing_file() {
        let dir = std::env::temp_dir().join(format!("jvid-language-{}", std::process::id()));
        let path = dir.join("settings.toml");
        // No file yet: just the key, and every other setting stays default.
        save_language_to(&path, "es").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "language = \"es\"\n");
        assert_eq!(load_from(&path).language, "es");

        // Comments, unknown keys and other settings survive; the key is replaced in place.
        let hand = "# mine\ntheme = \"light\"\nlanguage = \"es\" # was\nfuture_key = 1\n";
        fs::write(&path, hand).unwrap();
        save_language_to(&path, "system").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "# mine\ntheme = \"light\"\nlanguage = \"system\" # was\nfuture_key = 1\n"
        );
        let s = load_from(&path);
        assert_eq!((s.theme.as_str(), s.language.as_str()), ("light", "system"));

        // A new key goes at the top, where it is top-level whatever follows.
        // A language that isn't a string isn't guessed at.
        fs::write(&path, "language = 3\n").unwrap();
        assert!(save_language_to(&path, "es").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "language = 3\n");

        // A broken file is reported and left untouched.
        fs::write(&path, "theme = \"light").unwrap();
        assert!(save_language_to(&path, "es").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "theme = \"light");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_bool_saves_in_place_and_a_string_where_it_was_is_left() {
        let dir = std::env::temp_dir().join(format!("jvid-bool-{}", std::process::id()));
        let path = dir.join("settings.toml");
        save_value_to(&path, "reduce_motion", true.into()).unwrap();
        assert!(load_from(&path).reduce_motion);
        fs::write(&path, "theme = \"light\"\nreduce_motion = true # mine\n").unwrap();
        save_value_to(&path, "reduce_motion", false.into()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text, "theme = \"light\"\nreduce_motion = false # mine\n");
        fs::write(&path, "reduce_motion = \"yes\"\n").unwrap();
        assert!(save_value_to(&path, "reduce_motion", true.into()).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn language_saves_past_quoted_keys_and_multiline_strings() {
        let dir = std::env::temp_dir().join(format!("jvid-language-q-{}", std::process::id()));
        let path = dir.join("settings.toml");
        fs::create_dir_all(&dir).unwrap();
        // A multiline string can hold text that looks like a table or the key.
        let notes = "notes = \"\"\"\n[not_a_table]\nlanguage = \"fake\"\n\"\"\"\n";
        let tricky = format!("{notes}[future]\nlanguage = 1\n");
        fs::write(&path, &tricky).unwrap();
        save_language_to(&path, "en").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text, format!("language = \"en\"\n{tricky}"));
        assert_eq!(load_from(&path).language, "en");

        // Any valid spelling of the key is found and only its value replaced.
        for (key, quote) in [
            ("\"language\"", '"'),
            ("'language'", '\''),
            ("language", '\''),
        ] {
            let file = format!("{notes}{key} = {quote}es{quote} # mine\n[future]\nlanguage = 1\n");
            fs::write(&path, &file).unwrap();
            save_language_to(&path, "system").unwrap();
            let text = fs::read_to_string(&path).unwrap();
            let want = file.replacen(&format!("{quote}es{quote}"), "\"system\"", 1);
            assert_eq!(text, want, "{key}");
            assert_eq!(load_from(&path).language, "system");
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
