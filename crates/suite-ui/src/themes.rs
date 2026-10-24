//! Colour themes: the `DESIGN.md` colour tokens as a TOML file, three built-ins and
//! live reload.
//!
//! A theme file lists tokens as `"#RRGGBB"` hex strings under their token names
//! (`panel = "#121416"`). A missing token takes the dark value.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::{fs, io};

use egui::{Color32, Context};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::i18n::CATALOG;
use crate::tokens::hex;
use crate::tr;

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
    pub ink_2: Color32,
    #[serde(with = "hex")]
    pub ink_3: Color32,
    #[serde(with = "hex")]
    pub ink_4: Color32,
}

impl Default for Palette {
    fn default() -> Self {
        DARK
    }
}

pub const DARK: Palette = Palette {
    blue: hex(0x2368C9),
    light_blue: hex(0x7AACF6),
    red: hex(0xEC5B57),
    amber: hex(0xF2AF48),
    green: hex(0x4EBE7D),
    ground: hex(0x0A0B0D),
    panel: hex(0x121416),
    raised: hex(0x1C1E21),
    field: hex(0x0D0E11),
    border: hex(0x232529),
    border_strong: hex(0x36383C),
    ink: hex(0xEDEEF1),
    ink_2: hex(0xADB1B8),
    ink_3: hex(0x82868E),
    ink_4: hex(0x5A5E63),
};

pub const LIGHT: Palette = Palette {
    blue: DARK.blue,
    light_blue: hex(0x1D5BB5),
    red: hex(0xC03632),
    amber: hex(0x8A5A00),
    green: hex(0x1E7A46),
    ground: hex(0xE6E7EA),
    panel: hex(0xF6F7F8),
    raised: Color32::WHITE,
    field: Color32::WHITE,
    border: hex(0xD9DBDF),
    border_strong: hex(0xB4B7BD),
    ink: hex(0x16181B),
    ink_2: hex(0x3F434A),
    ink_3: hex(0x5E6269),
    ink_4: hex(0x9A9EA5),
};

pub const HIGH_CONTRAST: Palette = Palette {
    blue: DARK.blue,
    light_blue: hex(0x9CC4FF),
    red: hex(0xFF7A75),
    amber: hex(0xFFC85C),
    green: hex(0x6BE09A),
    ground: Color32::BLACK,
    panel: Color32::BLACK,
    raised: hex(0x141414),
    field: Color32::BLACK,
    border: hex(0x8A8A8A),
    border_strong: Color32::WHITE,
    ink: Color32::WHITE,
    ink_2: hex(0xE0E0E0),
    ink_3: hex(0xC8C8C8),
    ink_4: hex(0x8A8A8A),
};

/// Built-in theme names, as the `theme` setting spells them.
pub const BUILT_IN: [(&str, Palette); 3] = [
    ("dark", DARK),
    ("light", LIGHT),
    ("high-contrast", HIGH_CONTRAST),
];

/// Theme files copied into a new themes folder, as (file name, contents). They double as
/// commented templates for custom themes.
pub const BUNDLED: [(&str, &str); 2] = [
    ("warm-dark.toml", include_str!("../themes/warm-dark.toml")),
    ("soft-light.toml", include_str!("../themes/soft-light.toml")),
];

/// One theme the picker offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub name: String,
    /// What the `theme` setting saves: a built-in name or the file's path.
    pub setting: String,
    /// The colours, or why the file doesn't load.
    pub palette: Result<Palette, String>,
}

/// Creates `dir` holding the bundled themes. Only the first time: a folder that exists is
/// left alone, so a deleted bundled theme stays deleted.
pub fn install_bundled(dir: &Path) -> io::Result<()> {
    if dir.exists() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    for (name, text) in BUNDLED {
        fs::write(dir.join(name), text)?;
    }
    Ok(())
}

/// The built-ins, then each `.toml` file in `dir` by file name, then `setting` when it names
/// a file outside `dir`, so the current choice always has a card.
pub fn choices(dir: Option<&Path>, setting: &str) -> Vec<Choice> {
    let mut out: Vec<Choice> = BUILT_IN
        .iter()
        .map(|&(s, p)| Choice {
            name: built_in_name(s).into(),
            setting: s.into(),
            palette: Ok(p),
        })
        .collect();
    let mut files: Vec<PathBuf> = dir
        .and_then(|d| fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    let known =
        |s: &str| BUILT_IN.iter().any(|t| t.0 == s) || files.iter().any(|f| f.as_os_str() == s);
    if !setting.is_empty() && !known(setting) {
        files.push(setting.into());
    }
    out.extend(files.into_iter().map(|path| Choice {
        name: file_title(&path),
        palette: load(&path),
        setting: path.to_string_lossy().into_owned(),
    }));
    out
}

fn built_in_name(setting: &str) -> &'static str {
    match setting {
        "light" => tr!(CATALOG, "theme.light"),
        "high-contrast" => tr!(CATALOG, "theme.high_contrast"),
        _ => tr!(CATALOG, "theme.dark"),
    }
}

/// A theme file's name in Title Case: `nord-night.toml` is Nord Night.
fn file_title(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let words = stem.split(['-', '_', ' ']).filter(|w| !w.is_empty());
    let title = words.map(|w| {
        let mut c = w.chars();
        c.next()
            .map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
    });
    title.collect::<Vec<_>>().join(" ")
}

/// Reads and parses a theme file; the error is the `theme.error` message, on one line.
pub fn load(path: &Path) -> Result<Palette, String> {
    let error =
        |e: &dyn std::fmt::Display| tr!(CATALOG, "theme.error", path = path.display(), error = e);
    let text = fs::read_to_string(path).map_err(|e| error(&e))?;
    Palette::from_toml(&text).map_err(|e| {
        // TOML's own message spans several lines with the source quoted; keep where and what.
        let at = e.span().map_or(0, |s| s.start).min(text.len());
        let line = text[..at].matches('\n').count() + 1;
        let column = at - text[..at].rfind('\n').map_or(0, |i| i + 1) + 1;
        let what = tr!(
            CATALOG,
            "theme.at",
            line = line,
            column = column,
            error = e.message()
        );
        error(&what)
    })
}

impl Palette {
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("palettes always serialize")
    }
}

thread_local! {
    // egui draws on one thread, so the applied palette lives there; parallel tests keep their own.
    static CURRENT: Cell<Palette> = const { Cell::new(DARK) };
}

/// The palette last applied on this thread; what `tokens::color`'s functions read.
pub fn current() -> Palette {
    CURRENT.get()
}

/// Applies `palette` to `ctx`'s egui visuals, keeping the rest of the style, and to the
/// widgets' colours.
pub fn apply(ctx: &Context, palette: &Palette) {
    CURRENT.set(*palette);
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
        let first = self.applied.replace(stamp).is_none();
        match load(path) {
            Ok(p) => {
                apply(ctx, &p);
                None
            }
            Err(e) => {
                // A theme that never loaded falls back to Dark; a broken edit keeps the last good one.
                if first {
                    apply(ctx, &DARK);
                }
                Some(e)
            }
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
        parse(&s)
            .ok_or_else(|| serde::de::Error::custom(tr!(CATALOG, "theme.bad_color", value = s)))
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
        let bundled = BUNDLED.map(|(name, text)| (name, Palette::from_toml(text).unwrap()));
        for (name, p) in BUILT_IN.into_iter().chain(bundled) {
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

    #[test]
    fn folder_themes_list_after_the_built_ins() {
        let dir = std::env::temp_dir().join(format!("suite-ui-choices-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        install_bundled(&dir).unwrap();
        std::fs::write(dir.join("nord-night.toml"), "panel = \"#2E3440\"").unwrap();
        std::fs::write(dir.join("broken.toml"), "panel = 3").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a theme").unwrap();
        // Only the first run copies the templates, so a deleted one stays deleted.
        std::fs::remove_file(dir.join("warm-dark.toml")).unwrap();
        install_bundled(&dir).unwrap();

        let outside = std::env::temp_dir().join("elsewhere.toml");
        let list = choices(Some(&dir), outside.to_str().unwrap());
        let names: Vec<_> = list.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Dark",
                "Light",
                "High Contrast",
                "Broken",
                "Nord Night",
                "Soft Light",
                "Elsewhere"
            ]
        );
        assert_eq!(list[0].setting, "dark");
        let err = list[3].palette.as_ref().unwrap_err();
        assert!(!err.contains('\n') && err.contains("line 1"), "{err}");
        assert_eq!(
            list[4].palette.as_ref().unwrap().panel,
            Color32::from_rgb(0x2E, 0x34, 0x40)
        );
        assert!(
            list[6].palette.is_err(),
            "a missing file is broken, not dropped"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn broken_theme_on_first_load_falls_back_to_dark() {
        let ctx = Context::default();
        apply(&ctx, &LIGHT);
        let path =
            std::env::temp_dir().join(format!("suite-ui-broken-{}.toml", std::process::id()));
        std::fs::write(&path, "panel = 3").unwrap();
        let mut t = LiveTheme::new(path.to_str().unwrap());
        assert!(t.poll(&ctx).is_some());
        assert_eq!(ctx.global_style().visuals.panel_fill, DARK.panel);
        assert_eq!(current(), DARK);
        std::fs::remove_file(&path).unwrap();
    }
}
