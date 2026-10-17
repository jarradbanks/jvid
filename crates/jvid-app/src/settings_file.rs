//! Where `settings.toml` lives and reading/writing it; the format lives in `jvid_core::settings`.

use std::path::PathBuf;
use std::{fs, io};

use jvid_core::settings::Settings;

/// `<platform config dir>/settings.toml`, or `None` if the platform has no home dir.
pub fn path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "jvid").map(|d| d.config_dir().join("settings.toml"))
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
}
