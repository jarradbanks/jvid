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
    /// Decode H.264/HEVC on the GPU when a device opens; off forces software.
    pub hardware_decode: bool,
    /// UI language: `system` follows the OS, or a language code such as `en` or `es`.
    /// A language the app doesn't ship shows English.
    pub language: String,
    /// Voiceover input device name; empty follows the system default.
    pub input_device: String,
    /// Turns the UI's animations off, such as hover fades and the timeline's New Track slot.
    pub reduce_motion: bool,
    /// Check for stable GitHub releases at startup; installation always requires a click.
    pub automatic_update_checks: bool,
    /// Ids of the lessons finished in Help > Tutorials.
    pub tutorials_done: Vec<String>,
    /// The Welcome screen offered the tour, so it doesn't again.
    pub tour_offered: bool,
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
            hardware_decode: true,
            language: "system".into(),
            input_device: String::new(),
            reduce_motion: false,
            automatic_update_checks: true,
            tutorials_done: Vec::new(),
            tour_offered: false,
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
                "hardware_decode" => set(&mut out.hardware_decode, &key, value, |_| true),
                "language" => set(&mut out.language, &key, value, |v: &String| !v.is_empty()),
                "input_device" => set(&mut out.input_device, &key, value, |_| true),
                "reduce_motion" => set(&mut out.reduce_motion, &key, value, |_| true),
                "automatic_update_checks" => {
                    set(&mut out.automatic_update_checks, &key, value, |_| true)
                }
                "tutorials_done" => set(&mut out.tutorials_done, &key, value, |_| true),
                "tour_offered" => set(&mut out.tour_offered, &key, value, |_| true),
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
    fn automatic_update_checks_default_on_and_can_be_disabled_without_touching_other_settings() {
        let settings =
            Settings::from_toml("automatic_update_checks = false\ntheme = 'light'\n").unwrap();
        assert!(!settings.automatic_update_checks);
        assert_eq!(settings.theme, "light");
        assert_eq!(Settings::from_toml(&settings.to_toml()).unwrap(), settings);
        assert!(
            Settings::from_toml("automatic_update_checks = 'no'")
                .unwrap()
                .automatic_update_checks
        );
    }

    #[test]
    fn partial_keeps_defaults_for_missing_keys() {
        let s = Settings::from_toml("autosave_secs = 30\npreview_quality = \"half\"").unwrap();
        assert_eq!(s.autosave_secs, 30);
        assert_eq!(s.preview_quality, PreviewQuality::Half);
        assert_eq!(s.cache_max_mb, Settings::default().cache_max_mb);
    }

    #[test]
    fn language_defaults_to_system_and_keeps_any_code() {
        // Files from before the setting existed follow the OS.
        assert_eq!(
            Settings::from_toml("theme = \"light\"").unwrap().language,
            "system"
        );
        assert_eq!(
            Settings::from_toml("language = \"es\"").unwrap().language,
            "es"
        );
        // An unshipped code is kept, so the file round-trips; the app shows it in English.
        assert_eq!(
            Settings::from_toml("language = \"eo\"").unwrap().language,
            "eo"
        );
        assert_eq!(
            Settings::from_toml("language = \"\"").unwrap().language,
            "system"
        );
        assert_eq!(
            Settings::from_toml("language = 3").unwrap().language,
            "system"
        );
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
            hardware_decode: false,
            language: "es".into(),
            input_device: "USB Mic".into(),
            reduce_motion: true,
            automatic_update_checks: false,
            tutorials_done: vec!["basics".into()],
            tour_offered: true,
        };
        assert_eq!(Settings::from_toml(&s.to_toml()).unwrap(), s);
        let d = Settings::default();
        assert_eq!(Settings::from_toml(&d.to_toml()).unwrap(), d);
    }
}
