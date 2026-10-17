//! Colour themes: the `DESIGN.md` colour tokens as a TOML file, three built-ins and
//! live reload.
//!
//! A theme file lists tokens as `"#RRGGBB"` hex strings under their token names
//! (`panel = "#121416"`). A missing token takes the dark value.

use std::path::PathBuf;
use std::time::SystemTime;

use egui::{Color32, Context};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::tokens::color;

/// The colour tokens one theme sets.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Palette {
    #[serde(with = "hex")]
    pub blue: Color32,
    #[serde(with = "hex")]
    pub light_blue: Color32,
    #[serde(with = "hex")]
    pub red: Color32,
    #[serde(with = "hex")]
    pub amber: Color32,
    #[serde(with = "hex")]
    pub green: Color32,
    #[serde(with = "hex")]
    pub ground: Color32,
    #[serde(with = "hex")]
    pub panel: Color32,
    #[serde(with = "hex")]
    pub raised: Color32,
    #[serde(with = "hex")]
    pub field: Color32,
    #[serde(with = "hex")]
    pub border: Color32,
    #[serde(with = "hex")]
    pub border_strong: Color32,
    #[serde(with = "hex")]
    pub ink: Color32,
    #[serde(with = "hex")]
    pub ink_3: Color32,
}

impl Default for Palette {
    fn default() -> Self {
        DARK
    }
}

pub const DARK: Palette = Palette {
    blue: color::BLUE,
    light_blue: color::LIGHT_BLUE,
    red: color::RED,
    amber: color::AMBER,
    green: color::GREEN,
    ground: color::GROUND,
    panel: color::PANEL,
    raised: color::RAISED,
    field: color::FIELD,
    border: color::BORDER,
    border_strong: color::BORDER_STRONG,
    ink: color::INK,
    ink_3: color::INK_3,
};

pub const LIGHT: Palette = Palette {
    blue: color::BLUE,
    light_blue: Color32::from_rgb(0x1D, 0x5B, 0xB5),
    red: Color32::from_rgb(0xC0, 0x36, 0x32),
    amber: Color32::from_rgb(0x8A, 0x5A, 0x00),
    green: Color32::from_rgb(0x1E, 0x7A, 0x46),
    ground: Color32::from_rgb(0xE6, 0xE7, 0xEA),
    panel: Color32::from_rgb(0xF6, 0xF7, 0xF8),
    raised: Color32::WHITE,
    field: Color32::WHITE,
    border: Color32::from_rgb(0xD9, 0xDB, 0xDF),
    border_strong: Color32::from_rgb(0xB4, 0xB7, 0xBD),
    ink: Color32::from_rgb(0x16, 0x18, 0x1B),
    ink_3: Color32::from_rgb(0x5E, 0x62, 0x69),
};

pub const HIGH_CONTRAST: Palette = Palette {
    blue: color::BLUE,
    light_blue: Color32::from_rgb(0x9C, 0xC4, 0xFF),
    red: Color32::from_rgb(0xFF, 0x7A, 0x75),
    amber: Color32::from_rgb(0xFF, 0xC8, 0x5C),
    green: Color32::from_rgb(0x6B, 0xE0, 0x9A),
    ground: Color32::BLACK,
    panel: Color32::BLACK,
    raised: Color32::from_rgb(0x14, 0x14, 0x14),
    field: Color32::BLACK,
    border: Color32::from_rgb(0x8A, 0x8A, 0x8A),
    border_strong: Color32::WHITE,
    ink: Color32::WHITE,
    ink_3: Color32::from_rgb(0xC8, 0xC8, 0xC8),
};

/// Built-in theme names, as the `theme` setting spells them.
pub const BUILT_IN: [(&str, Palette); 3] = [
    ("dark", DARK),
    ("light", LIGHT),
    ("high-contrast", HIGH_CONTRAST),
];

impl Palette {
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("palettes always serialize")
    }
}

/// Applies `palette` to `ctx`'s egui visuals, keeping the rest of the style.
pub fn apply(ctx: &Context, palette: &Palette) {
    ctx.global_style_mut(|s| s.visuals = crate::theme::visuals_with(palette));
}

/// The theme named by a setting: a built-in name or a path to a theme file, which is
/// re-read whenever it changes on disk.
pub struct LiveTheme {
    file: Option<PathBuf>,
    /// The file's mtime when last applied; `None` before the first apply.
    applied: Option<SystemTime>,
    built_in: Option<Palette>,
    next_check: f64,
}

/// How often a theme file is checked for changes, in seconds.
const CHECK_SECS: f32 = 0.5;

impl LiveTheme {
    pub fn new(setting: &str) -> Self {
        let built_in = BUILT_IN.iter().find(|(n, _)| *n == setting).map(|t| t.1);
        Self {
            file: built_in.is_none().then(|| setting.into()),
            applied: None,
            built_in,
            next_check: 0.0,
        }
    }

    /// Call every frame. Applies the theme the first time and again after the file
    /// changes. A file that can't be read or parsed keeps the current theme and
    /// returns the error once per change.
    pub fn poll(&mut self, ctx: &Context) -> Option<String> {
        if let Some(p) = self.built_in.take() {
            apply(ctx, &p);
            return None;
        }
        let path = self.file.as_ref()?;
        let now = ctx.input(|i| i.time);
        if now < self.next_check {
            return None;
        }
        self.next_check = now + CHECK_SECS as f64;
        // Idle apps don't repaint, so ask for the next check.
        ctx.request_repaint_after_secs(CHECK_SECS);
        let stamp = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if self.applied == Some(stamp) {
            return None;
        }
        self.applied = Some(stamp);
        let result = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|s| Palette::from_toml(&s).map_err(|e| e.to_string()));
        match result {
            Ok(p) => {
                apply(ctx, &p);
                None
            }
            Err(e) => Some(format!("Theme {}: {e}", path.display())),
        }
    }
}

mod hex {
    use super::*;

    pub fn serialize<S: Serializer>(c: &Color32, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Color32, D::Error> {
        let s = String::deserialize(d)?;
        parse(&s).ok_or_else(|| serde::de::Error::custom(format!("`{s}` is not #RRGGBB")))
    }

    pub fn parse(s: &str) -> Option<Color32> {
        let h = s.strip_prefix('#')?;
        if h.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::contrast;

    #[test]
    fn partial_file_falls_back_to_dark() {
        let p = Palette::from_toml("panel = \"#FFFFFF\"").unwrap();
        assert_eq!(p.panel, Color32::WHITE);
        assert_eq!(p.ink, DARK.ink);
    }

    #[test]
    fn bad_hex_and_unknown_tokens_are_errors() {
        assert!(Palette::from_toml("panel = \"#FFF\"").is_err());
        assert!(Palette::from_toml("panel = \"#GGGGGG\"").is_err());
        assert!(Palette::from_toml("panle = \"#FFFFFF\"").is_err());
    }

    #[test]
    fn built_ins_round_trip_and_keep_text_readable() {
        for (name, p) in BUILT_IN {
            assert_eq!(Palette::from_toml(&p.to_toml()).unwrap(), p, "{name}");
            for (fg, what) in [(p.ink, "ink"), (p.ink_3, "ink_3"), (p.light_blue, "link")] {
                let r = contrast(fg, p.panel);
                assert!(r >= 4.5, "{name}: {what} on panel is {r:.2}:1");
            }
        }
        assert!(contrast(HIGH_CONTRAST.ink, HIGH_CONTRAST.panel) >= 7.0);
    }

    #[test]
    fn built_in_applies_once() {
        let ctx = Context::default();
        let mut t = LiveTheme::new("light");
        assert_eq!(t.poll(&ctx), None);
        assert_eq!(ctx.global_style().visuals.panel_fill, LIGHT.panel);
        assert!(!ctx.global_style().visuals.dark_mode);
    }

    #[test]
    fn file_change_reloads_without_restart() {
        let dir = std::env::temp_dir().join(format!("suite-ui-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mine.toml");
        std::fs::write(&path, "panel = \"#102030\"").unwrap();
        let ctx = Context::default();
        let mut t = LiveTheme::new(path.to_str().unwrap());
        assert_eq!(t.poll(&ctx), None);
        let panel = || ctx.global_style().visuals.panel_fill;
        assert_eq!(panel(), Color32::from_rgb(0x10, 0x20, 0x30));

        std::fs::write(&path, "panel = \"#405060\"").unwrap();
        // Make the change visible even on filesystems with coarse mtimes.
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        t.next_check = 0.0;
        assert_eq!(t.poll(&ctx), None);
        assert_eq!(panel(), Color32::from_rgb(0x40, 0x50, 0x60));

        // A broken edit reports once and keeps the last good theme.
        std::fs::write(&path, "panel = 3").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later + std::time::Duration::from_secs(5))
            .unwrap();
        t.next_check = 0.0;
        assert!(t.poll(&ctx).is_some());
        assert_eq!(panel(), Color32::from_rgb(0x40, 0x50, 0x60));
        t.next_check = 0.0;
        assert_eq!(t.poll(&ctx), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
