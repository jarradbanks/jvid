//! Where `settings.toml` lives and reading/writing it; the format lives in `jvid_core::settings`.

use std::path::{Path, PathBuf};
use std::{fs, io};

use jvid_core::settings::Settings;

use crate::i18n::tr;

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

/// Sets `language` in the settings file, keeping every other byte, comments included.
/// A file that isn't TOML is left as it is and reported, so a hand edit with a typo
/// isn't overwritten.
pub fn save_language_to(path: &Path, language: &str) -> io::Result<()> {
    let old = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let not_written = || io::Error::other(tr!("settings.not_written"));
    let parsed = toml::de::DeTable::parse(&old).map_err(io::Error::other)?;
    let mut line = toml::Table::new();
    line.insert("language".into(), language.into());
    // The parser's spans see through quoted keys, multiline strings and comments.
    let found = parsed
        .get_ref()
        .iter()
        .find(|(k, _)| k.get_ref().as_ref() == "language");
    let new = match found {
        Some((_, v)) if matches!(v.get_ref(), toml::de::DeValue::String(_)) => {
            let at = v.span();
            let value = line["language"].to_string();
            [&old[..at.start], &value, &old[at.end..]].concat()
        }
        Some(_) => return Err(not_written()),
        // The top of the file is always outside any table.
        None => line.to_string() + &old,
    };
    let mut before: toml::Table = old.parse().map_err(io::Error::other)?;
    let mut after: toml::Table = new.parse().map_err(io::Error::other)?;
    let written = after.remove("language");
    before.remove("language");
    if written.as_ref().and_then(|v| v.as_str()) != Some(language) || before != after {
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
