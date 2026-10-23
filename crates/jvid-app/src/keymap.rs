//! Named actions, their Premiere default shortcuts, and `keymap.toml` overrides.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

use egui::{Event, Key, KeyboardShortcut, Modifiers};

use crate::i18n::CATALOG;

/// Where an action listens. Global fires in any panel; text fields swallow everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    Global,
    Timeline,
}

pub struct Action {
    pub id: &'static str,
    pub default: &'static str,
    pub context: Context,
}

impl Action {
    /// The action's name in the UI language, from the `action.<id>` key.
    pub fn name(&self) -> &'static str {
        CATALOG
            .get(&format!("action.{}", self.id))
            .unwrap_or(self.id)
    }
}

const fn a(id: &'static str, default: &'static str) -> Action {
    Action {
        id,
        default,
        context: Context::Global,
    }
}

const fn t(id: &'static str, default: &'static str) -> Action {
    Action {
        id,
        default,
        context: Context::Timeline,
    }
}

/// Every action. "Ctrl" means Cmd on macOS.
pub const ACTIONS: &[Action] = &[
    a("new_project", "Ctrl+Alt+N"),
    a("open_project", "Ctrl+O"),
    a("save", "Ctrl+S"),
    a("save_as", "Ctrl+Shift+S"),
    a("import", "Ctrl+I"),
    a("import_subtitles", "Ctrl+Shift+I"),
    a("export", "Ctrl+M"),
    a("quit", "Ctrl+Q"),
    a("play_stop", "Space"),
    a("shuttle_left", "J"),
    a("shuttle_stop", "K"),
    a("shuttle_right", "L"),
    a("loop_playback", "Ctrl+Alt+L"),
    a("audio_scrub", "Shift+S"),
    a("mark_in", "I"),
    a("mark_out", "O"),
    a("clear_in", "Alt+I"),
    a("clear_out", "Alt+O"),
    a("clear_in_out", "Alt+X"),
    a("go_to_in", "Shift+I"),
    a("go_to_out", "Shift+O"),
    a("mark_clip", "X"),
    a("mark_selection", "/"),
    a("add_marker", "M"),
    a("add_range_marker", "Alt+M"),
    a("next_marker", "Shift+M"),
    a("prev_marker", "Ctrl+Shift+M"),
    t("lift", ";"),
    t("extract", "'"),
    a("insert", ","),
    a("overwrite", "."),
    t("ripple_trim_prev", "Q"),
    t("ripple_trim_next", "W"),
    a("undo", "Ctrl+Z"),
    a("redo", "Ctrl+Shift+Z"),
    a("redo_alt", "Ctrl+Y"),
    t("select_all", "Ctrl+A"),
    t("copy", "Ctrl+C"),
    t("cut", "Ctrl+X"),
    t("paste", "Ctrl+V"),
    t("duplicate", "Ctrl+Shift+D"),
    t("delete", "Delete"),
    t("ripple_delete", "Shift+Delete"),
    t("link", "Ctrl+L"),
    t("enable", "Shift+E"),
    t("reverse", "Ctrl+R"),
    t("freeze_frame", "Shift+F"),
    t("detect_silence", "Ctrl+Alt+S"),
    t("crossfade", "Ctrl+D"),
    t("dip_to_black", "Ctrl+Alt+B"),
    t("dip_to_white", "Ctrl+Alt+W"),
    t("linked_selection", "Ctrl+Shift+L"),
    t("snapping", "S"),
    a("razor_tool", "C"),
    a("selection_tool", "V"),
    t("add_edit", "Ctrl+K"),
    a("step_back", "Left"),
    a("step_forward", "Right"),
    a("step_back_second", "Shift+Left"),
    a("step_forward_second", "Shift+Right"),
    a("prev_edit", "Ctrl+Left"),
    a("next_edit", "Ctrl+Right"),
    a("prev_edit_alt", "Up"),
    a("next_edit_alt", "Down"),
    a("clip_start", "Ctrl+Shift+Left"),
    a("clip_end", "Ctrl+Shift+Right"),
    a("go_to_start", "Home"),
    a("go_to_end", "End"),
    t("zoom_in", "="),
    t("zoom_out", "-"),
    t("zoom_fit", "\\"),
    a("focus_media", "Shift+1"),
    a("focus_source", "Shift+2"),
    a("focus_timeline", "Shift+3"),
    a("focus_program", "Shift+4"),
    a("focus_inspector", "Shift+5"),
    a("focus_effects", "Shift+7"),
    a("export_frame", "Ctrl+Shift+E"),
    a("record", "Shift+R"),
];

/// "Ctrl+Shift+K" → shortcut. Ctrl and Cmd both mean the platform command key.
pub fn parse(s: &str) -> Option<KeyboardShortcut> {
    let mut mods = Modifiers::NONE;
    let mut parts: Vec<&str> = s.split('+').map(str::trim).collect();
    let key = Key::from_name(parts.pop()?)?;
    for m in parts {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "cmd" => mods |= Modifiers::COMMAND,
            "alt" | "option" => mods |= Modifiers::ALT,
            "shift" => mods |= Modifiers::SHIFT,
            _ => return None,
        }
    }
    Some(KeyboardShortcut::new(mods, key))
}

/// Inverse of [`parse`]: "Ctrl+Shift+K".
pub fn format(sc: &KeyboardShortcut) -> String {
    let m = sc.modifiers;
    let mut s = String::new();
    for (on, name) in [
        (m.command || m.ctrl || m.mac_cmd, "Ctrl+"),
        (m.alt, "Alt+"),
        (m.shift, "Shift+"),
    ] {
        if on {
            s.push_str(name);
        }
    }
    s + sc.logical_key.name()
}

/// Action index → its shortcut, or `None` when unbound.
#[derive(Clone, PartialEq, Debug)]
pub struct Keymap {
    pub bindings: Vec<Option<KeyboardShortcut>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: ACTIONS.iter().map(|a| parse(a.default)).collect(),
        }
    }
}

impl Keymap {
    /// Defaults with `keymap.toml` applied: `action_id = "Ctrl+K"`, or `""` to unbind.
    /// Bad entries are logged and skipped.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        let overrides: BTreeMap<String, String> = toml::from_str(s)?;
        let mut map = Self::default();
        for (id, sc) in overrides {
            let Some(i) = ACTIONS.iter().position(|a| a.id == id) else {
                tracing::warn!("keymap: unknown action {id:?}");
                continue;
            };
            match (sc.trim(), parse(&sc)) {
                ("", _) => map.bindings[i] = None,
                (_, Some(p)) => map.bindings[i] = Some(p),
                _ => tracing::warn!("keymap: can't parse {sc:?} for {id}"),
            }
        }
        Ok(map)
    }

    /// The `keymap.toml` that gives this map: only bindings that differ from the defaults.
    pub fn to_toml(&self) -> String {
        let defaults = Self::default();
        let overrides: BTreeMap<&str, String> = ACTIONS
            .iter()
            .zip(&self.bindings)
            .zip(&defaults.bindings)
            .filter(|((_, b), d)| b != d)
            .map(|((a, b), _)| (a.id, b.as_ref().map(format).unwrap_or_default()))
            .collect();
        toml::to_string(&overrides).expect("string map serializes")
    }

    /// The action's name and its shortcut as text, empty when unbound.
    pub fn label(&self, id: &str) -> (&'static str, String) {
        let i = ACTIONS
            .iter()
            .position(|a| a.id == id)
            .expect("known action");
        let sc = self.bindings[i].as_ref().map(format).unwrap_or_default();
        (ACTIONS[i].name(), sc)
    }

    /// Pairs of action ids sharing a shortcut where both could fire.
    pub fn conflicts(&self) -> Vec<(&'static str, &'static str)> {
        let mut out = Vec::new();
        for (i, a) in ACTIONS.iter().enumerate() {
            for (j, b) in ACTIONS.iter().enumerate().skip(i + 1) {
                let overlap = a.context == b.context
                    || a.context == Context::Global
                    || b.context == Context::Global;
                if overlap && self.bindings[i].is_some() && self.bindings[i] == self.bindings[j] {
                    out.push((a.id, b.id));
                }
            }
        }
        out
    }

    /// Ids of actions whose shortcut was pressed this frame, first match per key press.
    /// None while a text field has focus.
    pub fn pressed(&self, ctx: &egui::Context, timeline_focused: bool) -> Vec<&'static str> {
        if ctx.text_edit_focused() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for e in ctx.input(|i| i.events.clone()) {
            // egui-winit turns these presses into clipboard events instead of key events;
            // on Windows Shift+Delete arrives as Cut.
            let clip = |modifiers, key| Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            };
            let shift = ctx.input(|i| i.modifiers.shift);
            let e = match e {
                Event::Copy => clip(Modifiers::COMMAND, Key::C),
                Event::Cut if shift => clip(Modifiers::SHIFT, Key::Delete),
                Event::Cut => clip(Modifiers::COMMAND, Key::X),
                Event::Paste(_) => clip(Modifiers::COMMAND, Key::V),
                e => e,
            };
            let Event::Key {
                key,
                physical_key,
                pressed: true,
                modifiers,
                ..
            } = e
            else {
                continue;
            };
            let hit = ACTIONS.iter().zip(&self.bindings).find(|(a, b)| {
                b.is_some_and(|b| {
                    // Physical too: Shift+1 arrives as `!` on most layouts.
                    (b.logical_key == key || Some(b.logical_key) == physical_key)
                        && modifiers.matches_exact(b.modifiers)
                }) && (a.context == Context::Global || timeline_focused)
            });
            if let Some((a, _)) = hit {
                out.push(a.id);
            }
        }
        out
    }
}

/// `keymap.toml`, next to `settings.toml`.
pub fn path() -> Option<PathBuf> {
    crate::settings_file::path().map(|p| p.with_file_name("keymap.toml"))
}

/// Never fails: a missing or broken file gives defaults. Conflicts are logged.
pub fn load() -> Keymap {
    path().map_or_else(Keymap::default, |p| load_from(&p))
}

/// Writes `keymap.toml`.
pub fn save(map: &Keymap) -> io::Result<()> {
    let p = path().ok_or_else(|| io::Error::other("no config directory"))?;
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(p, map.to_toml())
}

pub fn load_from(path: &Path) -> Keymap {
    let map = match fs::read_to_string(path) {
        Ok(s) => Keymap::from_toml(&s).unwrap_or_else(|e| {
            tracing::warn!(
                "keymap: {} is not valid, using defaults: {e}",
                path.display()
            );
            Keymap::default()
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Keymap::default(),
        Err(e) => {
            tracing::warn!("keymap: can't read {}, using defaults: {e}", path.display());
            Keymap::default()
        }
    };
    for (a, b) in map.conflicts() {
        tracing::warn!("keymap: {a} and {b} share a shortcut; {a} wins");
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_names_are_title_case() {
        const SMALL: [&str; 7] = ["a", "and", "at", "from", "in", "of", "to"];
        for a in ACTIONS {
            for w in a.name().split([' ', '/']).skip(1) {
                assert!(
                    SMALL.contains(&w) || !w.starts_with(|c: char| c.is_lowercase()),
                    "{:?} isn't Title Case",
                    a.name()
                );
            }
        }
    }

    fn binding(map: &Keymap, id: &str) -> Option<KeyboardShortcut> {
        map.bindings[ACTIONS.iter().position(|a| a.id == id).unwrap()]
    }

    #[test]
    fn defaults_all_parse_without_conflicts() {
        for a in ACTIONS {
            assert!(parse(a.default).is_some(), "{}", a.default);
        }
        assert_eq!(Keymap::default().conflicts(), vec![]);
        assert_eq!(
            parse("Ctrl+K"),
            Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::K))
        );
        assert_eq!(parse("Hyper+K"), None);
    }

    #[test]
    fn override_in_file_changes_binding_on_load() {
        let dir = std::env::temp_dir().join(format!("jvid-keymap-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keymap.toml");
        fs::write(
            &path,
            "mark_in = \"Ctrl+Shift+M\"\nmark_out = \"\"\nbogus = \"K\"\n",
        )
        .unwrap();
        let map = load_from(&path);
        fs::remove_dir_all(dir).unwrap();
        assert_eq!(
            binding(&map, "mark_in"),
            Some(KeyboardShortcut::new(
                Modifiers::COMMAND | Modifiers::SHIFT,
                Key::M
            ))
        );
        assert_eq!(binding(&map, "mark_out"), None);
        assert_eq!(binding(&map, "play_stop"), parse("Space"));
    }

    #[test]
    fn format_and_toml_round_trip() {
        for a in ACTIONS {
            let sc = parse(a.default).unwrap();
            assert_eq!(parse(&format(&sc)), Some(sc), "{}", a.default);
        }
        assert_eq!(Keymap::default().to_toml(), "");
        let map = Keymap::from_toml("mark_in = \"Ctrl+Shift+M\"\nmark_out = \"\"").unwrap();
        assert_eq!(Keymap::from_toml(&map.to_toml()).unwrap(), map);
    }

    #[test]
    fn conflicts_respect_context() {
        // Global vs global clashes.
        let map = Keymap::from_toml("mark_in = \"K\"").unwrap();
        assert_eq!(map.conflicts(), vec![("shuttle_stop", "mark_in")]);
        // Two timeline-only actions clash too.
        let map = Keymap::from_toml("lift = \"Q\"").unwrap();
        assert_eq!(map.conflicts(), vec![("lift", "ripple_trim_prev")]);
    }

    #[test]
    fn shortcuts_dont_fire_while_typing() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;
        use std::cell::RefCell;
        let fired = RefCell::new(Vec::new());
        let map = Keymap::default();
        let mut text = String::new();
        let mut h = Harness::new_ui(|ui| {
            fired.borrow_mut().extend(map.pressed(ui.ctx(), false));
            ui.text_edit_singleline(&mut text);
        });
        h.run();
        h.key_press(Key::I);
        h.run();
        assert_eq!(*fired.borrow(), vec!["mark_in"]);
        fired.borrow_mut().clear();

        h.get_by_role(egui::accesskit::Role::TextInput).focus();
        h.run();
        h.key_press(Key::I);
        h.run();
        assert!(fired.borrow().is_empty());
    }

    /// docs/shortcuts.md is generated from ACTIONS; `UPDATE_SHORTCUTS=1` rewrites it.
    #[test]
    fn cheat_sheet_matches_defaults() {
        let mut md = String::from(
            "# Keyboard shortcuts\n\n<!-- Generated from crates/jvid-app/src/keymap.rs. \
             Run `UPDATE_SHORTCUTS=1 cargo test -p jvid-app cheat_sheet` to update. -->\n\n\
             The default keymap. Ctrl means Cmd on macOS. Remap any of these from \
             Keyboard shortcuts… in the project menu.\n",
        );
        for (context, heading, note) in [
            (
                Context::Global,
                "Anywhere",
                "Work in any panel except while typing in a text field.",
            ),
            (
                Context::Timeline,
                "Timeline",
                "Work while the timeline has focus.",
            ),
        ] {
            md += &format!("\n## {heading}\n\n{note}\n\n| Action | Shortcut |\n| --- | --- |\n");
            for a in ACTIONS.iter().filter(|a| a.context == context) {
                // Code spans don't take backslash escapes, but a table cell still needs `|` escaped.
                let key = a.default.replace('|', "\\|");
                md += &format!("| {} | `{key}` |\n", a.name());
            }
        }
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/shortcuts.md");
        if std::env::var_os("UPDATE_SHORTCUTS").is_some() {
            fs::write(path, &md).unwrap();
        }
        let on_disk = fs::read_to_string(path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            on_disk == md,
            "docs/shortcuts.md is out of date; run UPDATE_SHORTCUTS=1 cargo test -p jvid-app cheat_sheet"
        );
    }
}
