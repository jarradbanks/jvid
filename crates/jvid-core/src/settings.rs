//! User settings (`settings.toml`). Parsing is lenient: a missing key takes its
//! default, an unknown key or invalid value logs a warning and is ignored, so a
//! settings file never stops the app from starting. File access lives in jvid-app.

use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Default length of a still clip in seconds.
pub const DEFAULT_STILL_SECS: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewQuality {
    #[default]
    Full,
    Half,
    Quarter,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Settings {
    pub preview_quality: PreviewQuality,
    /// 0 turns autosave off.
    pub autosave_secs: u32,
    /// `None` means the platform cache dir.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<PathBuf>,
    pub cache_max_mb: u64,
    /// Most recent first.
    pub recent_projects: Vec<PathBuf>,
    pub still_duration_secs: u32,
    /// `dark`, `light`, `high-contrast` or the path of a theme `.toml` file.
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            preview_quality: PreviewQuality::Full,
            autosave_secs: 60,
            cache_dir: None,
            cache_max_mb: 10 * 1024,
            recent_projects: Vec::new(),
            still_duration_secs: DEFAULT_STILL_SECS,
            theme: "dark".into(),
        }
    }
}

pub const AUTOSAVE_SECS_MAX: u32 = 3600;
pub const CACHE_MAX_MB_MIN: u64 = 100;
pub const STILL_SECS_RANGE: std::ops::RangeInclusive<u32> = 1..=3600;
pub const RECENT_PROJECTS_MAX: usize = 20;

impl Settings {
    /// Parses `settings.toml`. Only a file that is not TOML at all is an error; callers
    /// should then fall back to [`Settings::default`].
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        let table: toml::Table = s.parse()?;
        let mut out = Self::default();
        for (key, value) in table {
            match key.as_str() {
                "preview_quality" => set(&mut out.preview_quality, &key, value, |_| true),
                "autosave_secs" => set(&mut out.autosave_secs, &key, value, |v| {
                    *v <= AUTOSAVE_SECS_MAX
                }),
                "cache_dir" => set(&mut out.cache_dir, &key, value, |v| {
                    v.as_ref().is_some_and(|p| p.is_absolute())
                }),
                "cache_max_mb" => set(&mut out.cache_max_mb, &key, value, |v| {
                    *v >= CACHE_MAX_MB_MIN
                }),
                "recent_projects" => {
                    set(&mut out.recent_projects, &key, value, |_| true);
                    out.recent_projects.truncate(RECENT_PROJECTS_MAX);
                }
                "still_duration_secs" => set(&mut out.still_duration_secs, &key, value, |v| {
                    STILL_SECS_RANGE.contains(v)
                }),
                "theme" => set(&mut out.theme, &key, value, |v: &String| !v.is_empty()),
                _ => tracing::warn!("settings: ignoring unknown key `{key}`"),
            }
        }
        Ok(out)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("settings always serialize")
    }
}

/// Overwrites `field` if `value` deserializes and passes `valid`; otherwise warns and
/// leaves the default.
fn set<T: DeserializeOwned>(
    field: &mut T,
    key: &str,
    value: toml::Value,
    valid: impl Fn(&T) -> bool,
) {
    match T::deserialize(value.clone()) {
        Ok(v) if valid(&v) => *field = v,
        _ => tracing::warn!("settings: invalid `{key}` = {value}, using the default"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        assert_eq!(Settings::from_toml("").unwrap(), Settings::default());
    }

    #[test]
    fn partial_keeps_defaults_for_missing_keys() {
        let s = Settings::from_toml("autosave_secs = 30\npreview_quality = \"half\"").unwrap();
        assert_eq!(s.autosave_secs, 30);
        assert_eq!(s.preview_quality, PreviewQuality::Half);
        assert_eq!(s.cache_max_mb, Settings::default().cache_max_mb);
    }

    #[test]
    fn unknown_key_is_ignored() {
        let s = Settings::from_toml("frobnicate = 1\nautosave_secs = 30").unwrap();
        assert_eq!(s.autosave_secs, 30);
    }

    #[test]
    fn invalid_values_fall_back_per_field() {
        let s = Settings::from_toml(
            "autosave_secs = -1\npreview_quality = \"eighth\"\ncache_max_mb = 1\n\
             still_duration_secs = 0\ncache_dir = \"relative\"\nrecent_projects = 3",
        )
        .unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn round_trip() {
        let s = Settings {
            preview_quality: PreviewQuality::Quarter,
            autosave_secs: 0,
            cache_dir: Some(std::env::temp_dir()),
            cache_max_mb: 500,
            recent_projects: vec!["a.jvid".into(), "b/c.jvid".into()],
            still_duration_secs: 3,
            theme: "/home/me/mine.toml".into(),
        };
        assert_eq!(Settings::from_toml(&s.to_toml()).unwrap(), s);
        let d = Settings::default();
        assert_eq!(Settings::from_toml(&d.to_toml()).unwrap(), d);
    }
}
